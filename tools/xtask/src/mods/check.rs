//! `cargo xtask mods check --mods <file>`: loads every `mod.toml`, resolves the set deterministically, checks the
//! licences and the Rust entries, and writes `mods.lock` next to the list file.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use mod_api::MOD_API_VERSION;
use mod_manifest::{Installed, Manifest, Part, Pick, Resolution, ResolveError, write_lock};

use super::local::{self, MANIFEST, Source};

/// A mod directory with its parsed manifest and content hash.
#[derive(Debug, Clone)]
pub struct LoadedMod {
    pub dir: PathBuf,
    pub installed: Installed,
}

/// Everything `mods check` and `mods build` need from the list.
#[derive(Debug)]
pub struct Prepared {
    pub list_path: PathBuf,
    /// All candidates (listed and from `mods_dir`).
    pub mods: Vec<LoadedMod>,
    pub resolution: Resolution,
}

impl Prepared {
    /// The directory of a resolved mod.
    pub fn dir_of(
        &self,
        id: &mod_manifest::ModId,
        version: mod_manifest::Version,
    ) -> Option<&Path> {
        self.mods
            .iter()
            .find(|m| m.installed.manifest.id == *id && m.installed.manifest.version == version)
            .map(|m| m.dir.as_path())
    }

    /// The text of `mods.lock`.
    pub fn lock_text(&self) -> String {
        write_lock(&self.resolution.set)
    }
}

/// Reads the list, loads and checks the mods, resolves. Errors carry the chain list -> mod -> cause.
pub fn prepare(list_path: &Path, stage_root: &Path) -> Result<Prepared> {
    let text = std::fs::read_to_string(list_path)
        .with_context(|| format!("cannot read the mods list {}", list_path.display()))?;
    let list = local::parse(&text).with_context(|| format!("mods list {}", list_path.display()))?;
    let base = list_path.parent().unwrap_or(Path::new("."));
    let sources = local::sources(&list, base, &stage_root.join("git"))
        .with_context(|| format!("mods list {}", list_path.display()))?;
    let extra = local::extra_dirs(&list, base)
        .with_context(|| format!("mods list {}", list_path.display()))?;

    let mut mods: Vec<LoadedMod> = Vec::new();
    let mut picks: Vec<Pick> = Vec::new();
    for s in &sources {
        let m = load_mod(&s.dir, stage_root)
            .with_context(|| format!("mods list {}: {}", list_path.display(), describe(s)))?;
        picks.push(Pick {
            id: m.installed.manifest.id.clone(),
            req: s.req.clone(),
        });
        mods.push(m);
    }
    for d in &extra {
        let m = load_mod(d, stage_root).with_context(|| {
            format!(
                "mods list {}: mods_dir entry {}",
                list_path.display(),
                d.display()
            )
        })?;
        mods.push(m);
    }
    // A directory reached twice (listed, and inside `mods_dir`) is one candidate.
    mods.sort_by(|a, b| {
        let key = |m: &LoadedMod| {
            (
                m.installed.manifest.id.clone(),
                m.installed.manifest.version,
            )
        };
        key(a).cmp(&key(b))
    });
    mods.dedup_by(|a, b| same_dir(&a.dir, &b.dir));

    let installed: Vec<Installed> = mods.iter().map(|m| m.installed.clone()).collect();
    let resolution = mod_manifest::resolve(&picks, &installed, &MOD_API_VERSION)
        .map_err(|e: ResolveError| anyhow!("{e}"))
        .with_context(|| format!("resolving the mods of {}", list_path.display()))?;
    Ok(Prepared {
        list_path: list_path.to_path_buf(),
        mods,
        resolution,
    })
}

fn describe(s: &Source) -> String {
    format!("{} ({})", s.origin, s.dir.display())
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}

/// Loads one mod directory: manifest, per-mod checks, content hash.
pub fn load_mod(dir: &Path, stage_root: &Path) -> Result<LoadedMod> {
    let manifest_path = dir.join(MANIFEST);
    let text = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("cannot read {}", manifest_path.display()))?;
    let manifest =
        Manifest::parse(&text).map_err(|e| anyhow!("{}: {e}", manifest_path.display()))?;
    check_mod(&manifest, dir).with_context(|| {
        format!(
            "mod `{}` {} ({})",
            manifest.id,
            manifest.version,
            dir.display()
        )
    })?;
    let content_hash = content_hash(dir, stage_root, manifest.id.as_str())
        .with_context(|| format!("hashing the content of {}", dir.display()))?;
    Ok(LoadedMod {
        dir: dir.to_path_buf(),
        installed: Installed {
            manifest,
            content_hash,
        },
    })
}

/// Names at the root of a mod directory that are not part of the mod: a build directory (a mod with its own Cargo
/// workspace) and the list file and lock of a distribution that sit next to the mod (the example `hello` list is in
/// the mod directory; the lock changes with the hash, so it cannot be part of it).
const NOT_CONTENT: &[&str] = &["target", "mods.toml", "mods.lock"];

/// The hash the server computes for the installed directory (without [`NOT_CONTENT`], through a staged copy when one
/// of them is there).
fn content_hash(dir: &Path, stage_root: &Path, id: &str) -> Result<mod_manifest::ContentHash> {
    if !NOT_CONTENT.iter().any(|n| dir.join(n).exists()) {
        return Ok(mod_host_server::hash_mod_dir(dir)?);
    }
    let stage = stage_root.join("stage").join(id);
    if stage.exists() {
        std::fs::remove_dir_all(&stage)?;
    }
    copy_without(dir, &stage, true)?;
    let h = mod_host_server::hash_mod_dir(&stage);
    let _ = std::fs::remove_dir_all(&stage);
    Ok(h?)
}

fn copy_without(from: &Path, to: &Path, root: bool) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for item in std::fs::read_dir(from)? {
        let item = item?;
        let name = item.file_name();
        let n = name.to_string_lossy();
        if n.starts_with('.') || (root && NOT_CONTENT.contains(&&*n)) {
            continue;
        }
        let ty = item.file_type()?;
        if ty.is_dir() {
            copy_without(&item.path(), &to.join(&name), false)?;
        } else if ty.is_file() {
            std::fs::copy(item.path(), to.join(&name))?;
        }
    }
    Ok(())
}

/// Checks of one mod that need no other mod: SPDX licence, Rust crates and entries.
pub fn check_mod(m: &Manifest, dir: &Path) -> Result<()> {
    check_spdx(&m.license).with_context(|| format!("mod.license {:?}", m.license))?;
    for (part, entry) in [
        (Part::ServerRs, &m.rust.server),
        (Part::ClientRs, &m.rust.client),
    ] {
        let Some(e) = entry else { continue };
        let which = if part == Part::ServerRs {
            "server"
        } else {
            "client"
        };
        let (module, _) = e.entry.split_once("::").unwrap_or((&e.entry, ""));
        if module != e.crate_name.replace('-', "_") {
            bail!(
                "rust.{which}: entry `{}` is not a function of crate `{}` (expected `{}::...`)",
                e.entry,
                e.crate_name,
                e.crate_name.replace('-', "_")
            );
        }
        find_crate(dir, &e.crate_name)
            .with_context(|| format!("rust.{which}: crate `{}`", e.crate_name))?;
    }
    Ok(())
}

/// The directory of the package `name` below `dir` (up to four levels, no `target`, no dot directories).
pub fn find_crate(dir: &Path, name: &str) -> Result<PathBuf> {
    fn walk(dir: &Path, name: &str, depth: usize, out: &mut Vec<PathBuf>) {
        if let Ok(text) = std::fs::read_to_string(dir.join("Cargo.toml"))
            && let Ok(t) = text.parse::<toml::Table>()
            && t.get("package")
                .and_then(|p| p.get("name"))
                .and_then(|n| n.as_str())
                == Some(name)
        {
            out.push(dir.to_path_buf());
        }
        if depth == 0 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        let mut subs: Vec<PathBuf> = rd
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter(|e| {
                let n = e.file_name();
                let n = n.to_string_lossy();
                !n.starts_with('.') && n != "target"
            })
            .map(|e| e.path())
            .collect();
        subs.sort();
        for s in subs {
            walk(&s, name, depth - 1, out);
        }
    }
    let mut found = Vec::new();
    walk(dir, name, 4, &mut found);
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => bail!(
            "no `Cargo.toml` with `package.name = \"{name}\"` below {}",
            dir.display()
        ),
        _ => bail!(
            "several packages are named `{name}`: {}",
            found
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

const SPDX_IDS: &[&str] = &[
    "0BSD",
    "AGPL-3.0-only",
    "AGPL-3.0-or-later",
    "Apache-2.0",
    "Artistic-2.0",
    "BlueOak-1.0.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "BSL-1.0",
    "CC-BY-4.0",
    "CC-BY-SA-4.0",
    "CC0-1.0",
    "CDDL-1.0",
    "EPL-2.0",
    "EUPL-1.2",
    "GPL-2.0-only",
    "GPL-2.0-or-later",
    "GPL-3.0-only",
    "GPL-3.0-or-later",
    "ISC",
    "LGPL-2.1-only",
    "LGPL-2.1-or-later",
    "LGPL-3.0-only",
    "LGPL-3.0-or-later",
    "MIT",
    "MIT-0",
    "MPL-2.0",
    "NCSA",
    "OFL-1.1",
    "Unicode-3.0",
    "Unlicense",
    "WTFPL",
    "Zlib",
];

const SPDX_EXCEPTIONS: &[&str] = &[
    "Autoconf-exception-3.0",
    "Classpath-exception-2.0",
    "GCC-exception-3.1",
    "LLVM-exception",
    "Linux-syscall-note",
];

/// An SPDX licence expression: known ids (optionally `+`), `LicenseRef-...`, `WITH` a known exception, `AND`, `OR`
/// and parentheses. The error names the offending token.
pub fn check_spdx(expr: &str) -> Result<()> {
    let spaced = expr.replace('(', " ( ").replace(')', " ) ");
    let tokens: Vec<&str> = spaced.split_whitespace().collect();
    if tokens.is_empty() {
        bail!("empty licence expression");
    }
    let mut pos = 0;
    spdx_expr(&tokens, &mut pos)?;
    if pos != tokens.len() {
        bail!(
            "unexpected `{}` (an operator `AND` or `OR` is missing?)",
            tokens[pos]
        );
    }
    Ok(())
}

fn spdx_expr(t: &[&str], pos: &mut usize) -> Result<()> {
    spdx_term(t, pos)?;
    while matches!(t.get(*pos), Some(&"AND") | Some(&"OR")) {
        *pos += 1;
        spdx_term(t, pos)?;
    }
    Ok(())
}

fn spdx_term(t: &[&str], pos: &mut usize) -> Result<()> {
    let Some(&tok) = t.get(*pos) else {
        bail!("the expression ends where a licence id is expected");
    };
    *pos += 1;
    if tok == "(" {
        spdx_expr(t, pos)?;
        if t.get(*pos) != Some(&")") {
            bail!("a `)` is missing");
        }
        *pos += 1;
        return Ok(());
    }
    let id = tok.strip_suffix('+').unwrap_or(tok);
    let custom = id.strip_prefix("LicenseRef-").is_some_and(|r| {
        !r.is_empty()
            && r.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
    });
    if !custom && !SPDX_IDS.contains(&id) {
        let hint = if matches!(tok, "AND" | "OR" | "WITH" | ")") {
            format!("`{tok}` is not a licence id")
        } else {
            format!("unknown SPDX id `{tok}` (a custom licence is written `LicenseRef-Name`)")
        };
        bail!("{hint}");
    }
    if t.get(*pos) == Some(&"WITH") {
        *pos += 1;
        let Some(&ex) = t.get(*pos) else {
            bail!("`WITH` needs an exception id");
        };
        if !SPDX_EXCEPTIONS.contains(&ex) {
            bail!("unknown SPDX exception `{ex}`");
        }
        *pos += 1;
    }
    Ok(())
}

/// `--mods <file>` of the `mods` subcommands.
pub fn cmd_check(root: &Path, args: &[String]) -> Result<()> {
    let list = super::mods_arg(args)?;
    let work = super::work_dir(root)?;
    let prepared = prepare(&list, &work)?;
    report(&prepared);
    write_lock_file(&prepared)?;
    super::dist::graph_step(root, &prepared)
}

pub fn report(p: &Prepared) {
    for w in &p.resolution.warnings {
        println!("WARNING {w}");
    }
    let set = &p.resolution.set;
    println!("mods: {} in the set, hash {}", set.len(), set.modset_hash());
    for m in set.mods() {
        println!(
            "  [{}] {} {} ({}, {})",
            m.load_idx,
            m.id,
            m.version,
            m.sync.as_str(),
            p.dir_of(&m.id, m.version)
                .map(|d| d.display().to_string())
                .unwrap_or_default()
        );
    }
}

/// `mods.lock` next to the list file; untouched when the bytes are the same.
pub fn write_lock_file(p: &Prepared) -> Result<PathBuf> {
    let path = p.list_path.with_file_name("mods.lock");
    let text = p.lock_text();
    if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
        std::fs::write(&path, &text).with_context(|| format!("cannot write {}", path.display()))?;
        println!("wrote {}", path.display());
    } else {
        println!("{} is up to date", path.display());
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static N: AtomicU32 = AtomicU32::new(0);

    /// A scratch directory below the system temp dir, removed on drop.
    pub struct Scratch(pub PathBuf);
    impl Scratch {
        pub fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "xtask-mods-test-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&p).unwrap();
            Scratch(p)
        }
        pub fn file(&self, rel: &str, text: &str) {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub fn manifest(id: &str, version: &str, extra: &str, license: &str) -> String {
        format!(
            "[mod]\nid = \"{id}\"\nname = \"{id}\"\nversion = \"{version}\"\napi = \"^1\"\nlicense = \"{license}\"\n\
             sync = \"gameplay\"\nparts = [\"data\"]\n{extra}\n"
        )
    }

    fn prep(s: &Scratch) -> Result<Prepared> {
        prepare(&s.0.join("mods.toml"), &s.0.join("work"))
    }

    fn err(s: &Scratch) -> String {
        format!("{:#}", prep(s).unwrap_err())
    }

    #[test]
    fn spdx_accepts_forms_and_rejects_the_rest() {
        for ok in [
            "MIT",
            "Apache-2.0 OR MIT",
            "LicenseRef-Proprietary",
            "(MIT OR Apache-2.0) AND BSD-3-Clause",
            "GPL-3.0-or-later WITH Classpath-exception-2.0",
            "Apache-2.0+",
        ] {
            check_spdx(ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
        }
        for (bad, why) in [
            ("", "empty"),
            ("MiT", "unknown SPDX id `MiT`"),
            ("MIT Apache-2.0", "operator"),
            ("MIT OR", "ends"),
            ("(MIT", "`)`"),
            ("LicenseRef-", "unknown"),
            ("MIT WITH nope", "exception"),
            ("Proprietary", "LicenseRef-Name"),
        ] {
            let e = check_spdx(bad).unwrap_err().to_string();
            assert!(e.contains(why), "{bad:?}: {e}");
        }
    }

    #[test]
    fn a_valid_list_resolves_and_writes_a_stable_lock() {
        let s = Scratch::new();
        s.file(
            "aa/mod.toml",
            &manifest("aa", "1.0.0", "[deps]\nbb = \"^1\"", "MIT"),
        );
        s.file("bb/mod.toml", &manifest("bb", "1.2.0", "", "Apache-2.0"));
        s.file("bb/data.txt", "x");
        s.file(
            "mods.toml",
            "[[mod]]\npath = \"aa\"\n[[mod]]\npath = \"bb\"\n",
        );
        let p = prep(&s).unwrap();
        let ids: Vec<_> = p
            .resolution
            .set
            .mods()
            .iter()
            .map(|m| m.id.to_string())
            .collect();
        assert_eq!(ids, ["bb", "aa"], "a dependency loads first");
        let lock = write_lock_file(&p).unwrap();
        let first = std::fs::read(&lock).unwrap();
        // The order of the list does not matter.
        s.file(
            "mods.toml",
            "[[mod]]\npath = \"bb\"\n[[mod]]\npath = \"aa\"\n",
        );
        write_lock_file(&prep(&s).unwrap()).unwrap();
        assert_eq!(std::fs::read(&lock).unwrap(), first);
        assert!(String::from_utf8(first).unwrap().contains("id = \"aa\""));
    }

    #[test]
    fn mods_dir_supplies_dependencies_without_picking_them() {
        let s = Scratch::new();
        s.file(
            "aa/mod.toml",
            &manifest("aa", "1.0.0", "[deps]\nbb = \"^1\"", "MIT"),
        );
        s.file("inst/bb/mod.toml", &manifest("bb", "1.0.0", "", "MIT"));
        s.file("inst/cc/mod.toml", &manifest("cc", "1.0.0", "", "MIT"));
        s.file("mods.toml", "mods_dir = \"inst\"\n[[mod]]\npath = \"aa\"\n");
        let p = prep(&s).unwrap();
        assert_eq!(
            p.resolution.set.len(),
            2,
            "c is not picked and nothing needs it"
        );
    }

    #[test]
    fn a_missing_dependency_names_the_chain() {
        let s = Scratch::new();
        s.file(
            "aa/mod.toml",
            &manifest("aa", "1.0.0", "[deps]\nbb = \"^1\"", "MIT"),
        );
        s.file("mods.toml", "[[mod]]\npath = \"aa\"\n");
        let e = err(&s);
        assert!(e.contains("resolving the mods of"), "{e}");
        assert!(e.contains("`bb` is not installed"), "{e}");
        assert!(e.contains("aa 1.0.0"), "{e}");
    }

    #[test]
    fn the_licence_is_checked_and_the_chain_names_list_and_mod() {
        let s = Scratch::new();
        s.file("aa/mod.toml", &manifest("aa", "1.0.0", "", "Proprietary"));
        s.file("mods.toml", "[[mod]]\npath = \"aa\"\n");
        let e = err(&s);
        assert!(e.contains("mods list"), "{e}");
        assert!(e.contains("mod `aa` 1.0.0"), "{e}");
        assert!(e.contains("mod.license \"Proprietary\""), "{e}");
        assert!(e.contains("LicenseRef-Name"), "{e}");
    }

    #[test]
    fn the_api_requirement_must_admit_the_host() {
        let s = Scratch::new();
        let m = manifest("aa", "1.0.0", "", "MIT").replace("api = \"^1\"", "api = \"^2\"");
        s.file("aa/mod.toml", &m);
        s.file("mods.toml", "[[mod]]\npath = \"aa\"\n");
        let e = err(&s);
        assert!(e.contains("api"), "{e}");
        assert!(e.contains("^2"), "{e}");
    }

    #[test]
    fn a_manifest_error_names_the_file_and_the_key() {
        let s = Scratch::new();
        s.file("aa/mod.toml", &manifest("aa", "1.0.0", "bogus = 1", "MIT"));
        s.file("mods.toml", "[[mod]]\npath = \"aa\"\n");
        let e = err(&s);
        assert!(e.contains("mod.toml"), "{e}");
        assert!(e.contains("unknown key"), "{e}");
    }

    #[test]
    fn a_missing_directory_and_two_copies_of_one_version_fail() {
        let s = Scratch::new();
        s.file("mods.toml", "[[mod]]\npath = \"nope\"\n");
        assert!(err(&s).contains("cannot read"), "{}", err(&s));
        s.file("aa/mod.toml", &manifest("aa", "1.0.0", "", "MIT"));
        s.file("aa2/mod.toml", &manifest("aa", "1.0.0", "", "MIT"));
        s.file(
            "mods.toml",
            "[[mod]]\npath = \"aa\"\n[[mod]]\npath = \"aa2\"\n",
        );
        let e = err(&s);
        assert!(e.contains("aa") && e.contains("1.0.0"), "{e}");
    }

    #[test]
    fn rust_entries_are_matched_with_a_crate_on_disk() {
        let s = Scratch::new();
        let extra = "[rust]\nserver = { crate = \"aa-server\", entry = \"aa_server::register\" }\n";
        let m = manifest("aa", "1.0.0", extra, "MIT")
            .replace("parts = [\"data\"]", "parts = [\"data\", \"server-rs\"]");
        s.file("aa/mod.toml", &m);
        s.file("mods.toml", "[[mod]]\npath = \"aa\"\n");
        let e = err(&s);
        assert!(e.contains("rust.server: crate `aa-server`"), "{e}");
        s.file(
            "aa/server/Cargo.toml",
            "[package]\nname = \"aa-server\"\nversion = \"0.1.0\"\n",
        );
        prep(&s).unwrap();
        let bad = m.replace("aa_server::register", "other::register");
        s.file("aa/mod.toml", &bad);
        let e = err(&s);
        assert!(e.contains("is not a function of crate `aa-server`"), "{e}");
    }

    #[test]
    fn the_list_and_the_lock_next_to_a_mod_are_not_part_of_its_hash() {
        let s = Scratch::new();
        s.file("m/mods.toml", "[[mod]]\npath = \".\"\n");
        s.file("m/mod.toml", &manifest("aa", "1.0.0", "", "MIT"));
        let prep_m = || prepare(&s.0.join("m/mods.toml"), &s.0.join("work")).unwrap();
        let p = prep_m();
        let h = p.resolution.set.mods()[0].content_hash;
        write_lock_file(&p).unwrap();
        assert!(s.0.join("m/mods.lock").exists());
        assert_eq!(prep_m().resolution.set.mods()[0].content_hash, h);
    }

    #[test]
    fn a_target_directory_is_not_part_of_the_hash() {
        let s = Scratch::new();
        s.file("aa/mod.toml", &manifest("aa", "1.0.0", "", "MIT"));
        s.file("mods.toml", "[[mod]]\npath = \"aa\"\n");
        let h1 = prep(&s).unwrap().resolution.set.mods()[0].content_hash;
        s.file("aa/target/debug/junk.bin", "junk");
        let h2 = prep(&s).unwrap().resolution.set.mods()[0].content_hash;
        assert_eq!(h1, h2);
        s.file("aa/extra.txt", "real content");
        assert_ne!(prep(&s).unwrap().resolution.set.mods()[0].content_hash, h1);
    }
}
