//! Non-cargo checks: `.wasm` imports, forbidden game files (S8), the gamecore ABI header copy and the
//! gateway without a brain (GH-D2).

use anyhow::{Result, bail};

fn leb(b: &[u8], pos: &mut usize) -> Result<u64> {
    let mut result = 0u64;
    let mut shift = 0u32;
    loop {
        let Some(&byte) = b.get(*pos) else {
            bail!("unexpected end of wasm while reading LEB128")
        };
        *pos += 1;
        if shift < 64 {
            result |= u64::from(byte & 0x7f) << shift;
        }
        if byte & 0x80 == 0 {
            return Ok(result);
        }
        shift += 7;
        if shift > 70 {
            bail!("LEB128 too long in wasm");
        }
    }
}

fn name(b: &[u8], pos: &mut usize) -> Result<String> {
    let len = leb(b, pos)? as usize;
    let end = pos.checked_add(len).filter(|&e| e <= b.len());
    let Some(end) = end else {
        bail!("name runs past the wasm bounds")
    };
    let s = String::from_utf8_lossy(&b[*pos..end]).into_owned();
    *pos = end;
    Ok(s)
}

fn limits(b: &[u8], pos: &mut usize) -> Result<()> {
    let Some(&flags) = b.get(*pos) else {
        bail!("unexpected end of wasm in limits")
    };
    *pos += 1;
    leb(b, pos)?; // min
    if flags & 1 != 0 {
        leb(b, pos)?; // max
    }
    Ok(())
}

/// (module, field) pairs from the import section (id 2).
pub fn parse_imports(wasm: &[u8]) -> Result<Vec<(String, String)>> {
    if wasm.len() < 8 || &wasm[..4] != b"\0asm" {
        bail!("not a wasm module: missing \\0asm signature");
    }
    let mut pos = 8;
    let mut out = Vec::new();
    while pos < wasm.len() {
        let id = wasm[pos];
        pos += 1;
        let size = leb(wasm, &mut pos)? as usize;
        let end = pos.checked_add(size).filter(|&e| e <= wasm.len());
        let Some(end) = end else {
            bail!("section {id} runs past the end of the file")
        };
        if id == 2 {
            let mut p = pos;
            let count = leb(wasm, &mut p)?;
            for _ in 0..count {
                let module = name(wasm, &mut p)?;
                let field = name(wasm, &mut p)?;
                let Some(&kind) = wasm.get(p) else {
                    bail!("unexpected end of the import section")
                };
                p += 1;
                match kind {
                    0 => {
                        leb(wasm, &mut p)?; // typeidx
                    }
                    1 => {
                        p += 1; // reftype
                        limits(wasm, &mut p)?;
                    }
                    2 => limits(wasm, &mut p)?,
                    3 => p += 2, // valtype + mutability
                    4 => {
                        p += 1; // attribute
                        leb(wasm, &mut p)?; // typeidx
                    }
                    k => bail!("unknown import kind {k} ({module}.{field})"),
                }
                out.push((module, field));
            }
            return Ok(out);
        }
        pos = end;
    }
    Ok(out)
}

// Imports come from `./<name>_bg.js` (`wbg` in older versions); WASI and `env` are forbidden.
pub fn is_allowed_import_module(module: &str, out_name: &str) -> bool {
    module.starts_with("__wbindgen_") || module == "wbg" || module == format!("./{out_name}_bg.js")
}

pub fn forbidden_imports(wasm: &[u8], out_name: &str) -> Result<Vec<(String, String)>> {
    Ok(parse_imports(wasm)?
        .into_iter()
        .filter(|(m, _)| !is_allowed_import_module(m, out_name))
        .collect())
}

const FORBIDDEN_EXT: [&str; 10] = [
    "exe", "dll", "dds", "gr2", "fpk", "civ5save", "db", "fxsxml", "wav", "ogg",
];

/// S8: paths with a game-file extension outside `third_party/`.
pub fn forbidden_game_files<'a>(paths: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    paths
        .into_iter()
        .filter(|p| {
            let norm = p.replace('\\', "/");
            if norm.starts_with("third_party/") {
                return false;
            }
            let file = norm.rsplit('/').next().unwrap_or("");
            match file.rsplit_once('.') {
                Some((_, ext)) => {
                    let ext = ext.to_ascii_lowercase();
                    FORBIDDEN_EXT.contains(&ext.as_str())
                }
                None => false,
            }
        })
        .map(str::to_string)
        .collect()
}

/// What only `crates/server/server/src/worker/` may name (GH-D2): the gateway process never builds a core.
const BRAIN_TOKENS: [&str; 3] = ["game_host::real", "RealCore", "gamecore::Game"];

/// GH-D2: lines in the gateway code (`crates/server/server/src` outside `worker/`) that name the brain.
/// `files` are `(path relative to the repository root, text)`. Comment lines do not count: the rule is about
/// code that creates a core.
pub fn brain_in_gateway<'a>(files: impl IntoIterator<Item = (&'a str, &'a str)>) -> Vec<String> {
    const GATEWAY: &str = "crates/server/server/src/";
    const WORKER: &str = "crates/server/server/src/worker/";
    let mut found = Vec::new();
    for (path, text) in files {
        let norm = path.replace('\\', "/");
        if !norm.starts_with(GATEWAY) || norm.starts_with(WORKER) || !norm.ends_with(".rs") {
            continue;
        }
        for (i, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            if let Some(t) = BRAIN_TOKENS.iter().find(|t| line.contains(**t)) {
                found.push(format!("{norm}:{}: `{t}` outside src/worker/", i + 1));
            }
        }
    }
    found
}

/// Line endings normalized to LF: the header is compared as text, git may check it out with CRLF.
fn normalized(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
            i += 1;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

/// FNV-1a 64 of the LF-normalized header, for the check's messages.
pub fn header_hash(bytes: &[u8]) -> u64 {
    normalized(bytes)
        .iter()
        .fold(0xcbf2_9ce4_8422_2325u64, |h, &b| {
            (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
        })
}

/// `Ok` when both headers are the same text; otherwise the first differing line (1-based) and both hashes.
pub fn compare_headers(ours: &[u8], lekmod: &[u8]) -> Result<(), String> {
    let (a, b) = (normalized(ours), normalized(lekmod));
    if a == b {
        return Ok(());
    }
    let (a_text, b_text) = (String::from_utf8_lossy(&a), String::from_utf8_lossy(&b));
    let line = a_text
        .lines()
        .zip(b_text.lines())
        .position(|(x, y)| x != y)
        .unwrap_or_else(|| a_text.lines().count().min(b_text.lines().count()))
        + 1;
    Err(format!(
        "copy fnv1a64 {:016x} ({} bytes), Lekmod fnv1a64 {:016x} ({} bytes), first difference at line {line}",
        header_hash(ours),
        a.len(),
        header_hash(lekmod),
        b.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push_name(v: &mut Vec<u8>, s: &str) {
        v.push(s.len() as u8);
        v.extend_from_slice(s.as_bytes());
    }

    fn push_leb(v: &mut Vec<u8>, mut n: usize) {
        loop {
            let b = (n & 0x7f) as u8;
            n >>= 7;
            if n == 0 {
                v.push(b);
                return;
            }
            v.push(b | 0x80);
        }
    }

    fn module_with(imports: &[(&str, &str)]) -> Vec<u8> {
        let mut w = b"\0asm\x01\0\0\0".to_vec();
        w.extend_from_slice(&[1, 4, 1, 0x60, 0, 0]); // type section: () -> ()
        let mut sec = vec![imports.len() as u8];
        for (m, f) in imports {
            push_name(&mut sec, m);
            push_name(&mut sec, f);
            sec.extend_from_slice(&[0, 0]); // func, typeidx 0
        }
        w.push(2);
        push_leb(&mut w, sec.len());
        w.extend_from_slice(&sec);
        w
    }

    #[test]
    fn parses_imports_of_all_kinds() {
        let mut w = b"\0asm\x01\0\0\0".to_vec();
        let mut sec = vec![4u8];
        for (m, f, tail) in [
            ("a", "f", vec![0u8, 0]),
            ("b", "t", vec![1, 0x70, 1, 0x80, 0x01, 0xff, 0x01]), // table, limits with max (LEB 2 bytes)
            ("c", "m", vec![2, 0, 1]),
            ("d", "g", vec![3, 0x7f, 1]),
        ] {
            push_name(&mut sec, m);
            push_name(&mut sec, f);
            sec.extend_from_slice(&tail);
        }
        w.push(2);
        w.push(sec.len() as u8);
        w.extend_from_slice(&sec);
        w.extend_from_slice(&[3, 1, 0]); // a following section must not disturb parsing
        let imps = parse_imports(&w).unwrap();
        let mods: Vec<_> = imps.iter().map(|(m, _)| m.as_str()).collect();
        assert_eq!(mods, ["a", "b", "c", "d"]);
    }

    #[test]
    fn wasi_and_env_are_forbidden() {
        let w = module_with(&[
            ("__wbindgen_placeholder__", "__wbg_x"),
            (
                "__wbindgen_externref_xform__",
                "__wbindgen_externref_table_grow",
            ),
            ("wasi_snapshot_preview1", "fd_write"),
            ("env", "memory"),
        ]);
        let bad = forbidden_imports(&w, "app_web").unwrap();
        let mods: Vec<_> = bad.iter().map(|(m, _)| m.as_str()).collect();
        assert_eq!(mods, ["wasi_snapshot_preview1", "env"]);
    }

    #[test]
    fn bindgen_output_module_is_allowed() {
        let w = module_with(&[("./app_web_bg.js", "__wbg_x"), ("wbg", "y")]);
        assert!(forbidden_imports(&w, "app_web").unwrap().is_empty());
        let w = module_with(&[("./other_bg.js", "z")]);
        assert_eq!(forbidden_imports(&w, "app_web").unwrap().len(), 1);
    }

    #[test]
    fn no_import_section_is_ok_and_garbage_errors() {
        assert!(parse_imports(b"\0asm\x01\0\0\0").unwrap().is_empty());
        assert!(parse_imports(b"MZ\x90\0").is_err());
    }

    #[test]
    fn game_files_outside_third_party_are_forbidden() {
        let paths = [
            "src/main.rs",
            "assets/Tex.DDS",
            "bin/CivilizationV.exe",
            "third_party/lua-5.1.5/foo.dll",
            "data/save.Civ5Save",
            "a/b/game.db",
            "sounds/x.OGG",
            "sounds/x.wav",
            "docs/dll.txt",
            "Makefile",
            "x/.db",
        ];
        let bad = forbidden_game_files(paths);
        assert_eq!(
            bad,
            [
                "assets/Tex.DDS",
                "bin/CivilizationV.exe",
                "data/save.Civ5Save",
                "a/b/game.db",
                "sounds/x.OGG",
                "sounds/x.wav",
                "x/.db"
            ]
        );
        assert!(forbidden_game_files(["third_party/a.exe"]).is_empty());
    }

    #[test]
    fn the_gateway_does_not_name_the_brain() {
        let code = "use game_host::real::GameHost;
let g = gamecore::Game::new();
// RealCore is for the worker
";
        let files = [
            ("crates/server/server/src/supervisor/mod.rs", code),
            ("crates/server/server/src/worker/core_boot.rs", code),
            (
                "crates/server/server/src/lib.rs",
                "pub mod worker;
struct RealCoreX;
",
            ),
            ("crates/client/x/src/lib.rs", code),
        ];
        let bad = brain_in_gateway(files);
        assert_eq!(
            bad,
            [
                "crates/server/server/src/supervisor/mod.rs:1: `game_host::real` outside src/worker/",
                "crates/server/server/src/supervisor/mod.rs:2: `gamecore::Game` outside src/worker/",
                "crates/server/server/src/lib.rs:2: `RealCore` outside src/worker/",
            ]
        );
    }

    #[test]
    fn header_comparison_ignores_line_endings_only() {
        let lf = b"#define A 1\n#define B 2\n";
        assert!(compare_headers(lf, b"#define A 1\r\n#define B 2\r\n").is_ok());
        assert_eq!(
            header_hash(lf),
            header_hash(b"#define A 1\r\n#define B 2\r\n")
        );
        let err = compare_headers(lf, b"#define A 1\n#define B 3\n").unwrap_err();
        assert!(err.contains("first difference at line 2"), "{err}");
        // A prefix differs by length: the difference is reported after the common lines.
        let err = compare_headers(lf, b"#define A 1\n").unwrap_err();
        assert!(err.contains("line 2"), "{err}");
    }
}
