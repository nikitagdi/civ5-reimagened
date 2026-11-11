//! The `[ui]` part of the mods (mod API 1.3): what `mods check` checks, and how `mods build` turns the panels into
//! fragment packages next to the web bundle.
//!
//! ```text
//! check   [ui] shape (Manifest::parse) ─► points and modes against UI_CATALOG ─► the files exist
//!         ─► ui/model.toml is a model fragment ─► reads start at a root of the shared model
//!         ─► the set: two mods replacing one point is an error (the lobby's text)
//! build   per mod with [ui], in load order: converter (fragment mode) ─► <web>/ui2/mods/<file>
//!         ─► <web>/ui2/mods-ui.json [{id, version, file, hash, api, panels: [{id, point, mode, fallback}]}]
//! ```
//!
//! The converter is a separate program (`ui2-convert`, Node and Chromium behind it), run once per mod as
//! `ui2-convert fragment <mod dir> --out <dir>`. It reads the mod's `mod.toml`, converts every panel of `[ui]` into
//! one package of kind `Fragment`, writes it to `<dir>` and prints one line `ui2-fragment <file name> <hash>` (the
//! package's blake3 in hex). `CIV5_SKIP_UI2=1` skips the conversion: the index is written without those mods and
//! their panels are not shown (their Rust halves still run).
//
// SIMPLIFIED(SIMP-271): the pictures of a mod panel live only inside its fragment package; the converter does not
// take pictures from the mod's asset pack, so a panel cannot reference `pack:<file-id>`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use mod_api::check_ui_points;
use mod_manifest::{Manifest, ModId, UiSection, ui_conflicts};

use super::check::Prepared;

/// The index of the fragments, below `<web>/ui2/`.
pub const INDEX: &str = "mods-ui.json";
/// The folder of the fragment packages, below `<web>/ui2/`.
pub const DIR: &str = "mods";

/// Checks of one mod's `[ui]` that need no other mod: the catalogue, the files, the model fragment, the reads.
pub fn check_mod(m: &Manifest, dir: &Path) -> Result<()> {
    let Some(ui) = &m.ui else { return Ok(()) };
    check_ui_points(ui).map_err(|e| anyhow!("{e}"))?;
    for (key, rel) in ui.files() {
        if !dir.join(rel).is_file() {
            bail!("{key}: the file {rel} does not exist");
        }
    }
    if let Some(model) = &ui.model {
        let text = std::fs::read_to_string(dir.join(model))
            .with_context(|| format!("ui.model: cannot read {model}"))?;
        ui2_model::Fragment::parse(&text.replace("\r\n", "\n"))
            .map_err(|e| anyhow!("ui.model: {model}: {e}"))?;
    }
    let schema = ui2_model::Schema::v1();
    for (i, path) in ui.reads.iter().enumerate() {
        let root = path.split('.').next().unwrap_or_default();
        if schema.root_by_name(root).is_none() {
            bail!("ui.reads[{i}]: {path:?} does not start at a root of the shared model");
        }
    }
    Ok(())
}

/// The `[ui]` sections of the resolved set, in load order.
fn sections(p: &Prepared) -> Vec<(&ModId, &Manifest, &Path)> {
    p.resolution
        .set
        .mods()
        .iter()
        .filter_map(|rm| {
            let lm = p.mods.iter().find(|m| {
                m.installed.manifest.id == rm.id && m.installed.manifest.version == rm.version
            })?;
            lm.installed
                .manifest
                .ui
                .as_ref()
                .map(|_| (&rm.id, &lm.installed.manifest, lm.dir.as_path()))
        })
        .collect()
}

/// How many mods of the resolved set have `[ui]`.
pub fn sections_count(p: &Prepared) -> usize {
    sections(p).len()
}

/// The set-level check: two mods replacing one point. The text is the one the lobby shows.
pub fn check_set(p: &Prepared) -> Result<()> {
    let found = sections(p);
    let pairs: Vec<(&ModId, &UiSection)> = found
        .iter()
        .filter_map(|(id, m, _)| m.ui.as_ref().map(|u| (*id, u)))
        .collect();
    let conflicts = ui_conflicts(&pairs);
    if conflicts.is_empty() {
        return Ok(());
    }
    let lines: Vec<String> = conflicts.iter().map(ToString::to_string).collect();
    bail!("{}", lines.join("\n"))
}

/// One mod for the converter.
#[derive(Debug, Clone)]
pub struct FragmentJob {
    pub id: String,
    pub version: String,
    /// The mod directory (its `mod.toml` has the `[ui]`).
    pub dir: PathBuf,
    /// Where the package goes (`<web>/ui2/mods`).
    pub out_dir: PathBuf,
}

/// What the converter wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FragmentOut {
    /// The file name inside `out_dir`.
    pub file: String,
    /// blake3 of the package, hex (what `Package::hash` gives).
    pub hash: String,
}

/// Turns the panels of one mod into a fragment package (the real one runs `ui2-convert`; tests pass a fake).
pub type Converter<'a> = dyn FnMut(&FragmentJob) -> Result<FragmentOut> + 'a;

/// One entry of `mods-ui.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    pub id: String,
    pub version: String,
    /// Relative to `ui2/` (`mods/<file>`).
    pub file: String,
    pub hash: String,
    /// `[ui] api` as written.
    pub api: String,
    /// `(id, point, mode, fallback)` in the order of `[[ui.panels]]`.
    pub panels: Vec<(String, String, String, String)>,
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The text of `mods-ui.json`: entries in load order, one per line, stable bytes.
pub fn index_json(entries: &[IndexEntry]) -> String {
    let mut s = String::from("[");
    for (i, e) in entries.iter().enumerate() {
        s.push_str(if i == 0 { "\n  " } else { ",\n  " });
        let panels: Vec<String> = e
            .panels
            .iter()
            .map(|(id, point, mode, fallback)| {
                format!(
                    "{{\"id\": {}, \"point\": {}, \"mode\": {}, \"fallback\": {}}}",
                    json_str(id),
                    json_str(point),
                    json_str(mode),
                    json_str(fallback)
                )
            })
            .collect();
        let _ = write!(
            s,
            "{{\"id\": {}, \"version\": {}, \"file\": {}, \"hash\": {}, \"api\": {}, \"panels\": [{}]}}",
            json_str(&e.id),
            json_str(&e.version),
            json_str(&e.file),
            json_str(&e.hash),
            json_str(&e.api),
            panels.join(", ")
        );
    }
    if !entries.is_empty() {
        s.push('\n');
    }
    s.push_str("]\n");
    s
}

/// Converts the panels of every `[ui]` mod of the set with `convert` and writes `<web>/ui2/mods/` and the index.
/// Old packages in `<web>/ui2/mods/` are removed. `convert = None`: the conversion is skipped (no entries).
pub fn build_fragments(
    p: &Prepared,
    web: &Path,
    mut convert: Option<&mut Converter<'_>>,
) -> Result<PathBuf> {
    let ui2 = web.join("ui2");
    let out_dir = ui2.join(DIR);
    if out_dir.exists() {
        std::fs::remove_dir_all(&out_dir)
            .with_context(|| format!("cannot clear {}", out_dir.display()))?;
    }
    let mods = sections(p);
    if !mods.is_empty() && convert.is_some() {
        std::fs::create_dir_all(&out_dir)
            .with_context(|| format!("cannot create {}", out_dir.display()))?;
    }
    let mut entries = Vec::new();
    for (id, m, dir) in mods {
        let Some(ui) = &m.ui else { continue };
        let Some(conv) = convert.as_deref_mut() else {
            println!(
                "ui2 mods: `{id}` has panels; skipped ({})",
                crate::ui::web::SKIP_VAR
            );
            continue;
        };
        let job = FragmentJob {
            id: id.to_string(),
            version: m.version.to_string(),
            dir: dir.to_path_buf(),
            out_dir: out_dir.clone(),
        };
        let out = conv(&job).with_context(|| format!("converting the panels of mod `{id}`"))?;
        if !out_dir.join(&out.file).is_file() {
            bail!(
                "the converter did not write {} for mod `{id}`",
                out_dir.join(&out.file).display()
            );
        }
        entries.push(IndexEntry {
            id: job.id,
            version: job.version,
            file: format!("{DIR}/{}", out.file),
            hash: out.hash,
            api: ui.api.to_string(),
            panels: ui
                .panels
                .iter()
                .map(|pl| {
                    (
                        pl.id.clone(),
                        pl.point.clone(),
                        pl.mode.as_str().to_string(),
                        pl.fallback.as_str().to_string(),
                    )
                })
                .collect(),
        });
    }
    std::fs::create_dir_all(&ui2).with_context(|| format!("cannot create {}", ui2.display()))?;
    let index = ui2.join(INDEX);
    std::fs::write(&index, index_json(&entries))
        .with_context(|| format!("cannot write {}", index.display()))?;
    println!(
        "ui2 mods: {} fragment(s), index {}",
        entries.len(),
        index.display()
    );
    Ok(index)
}

/// Parses the converter's answer line `ui2-fragment <file> <hash>`.
pub fn parse_answer(stdout: &str) -> Result<FragmentOut> {
    let line = stdout
        .lines()
        .rev()
        .find_map(|l| l.trim().strip_prefix("ui2-fragment "))
        .ok_or_else(|| anyhow!("the converter printed no `ui2-fragment <file> <hash>` line"))?;
    let mut parts = line.split_whitespace();
    let (Some(file), Some(hash), None) = (parts.next(), parts.next(), parts.next()) else {
        bail!("the converter's answer {line:?} is not `<file> <hash>`");
    };
    let plain = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
    };
    if !plain(file) || !file.ends_with(".ui2") || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("the converter's answer {line:?} names no package");
    }
    Ok(FragmentOut {
        file: file.to_string(),
        hash: hash.to_ascii_lowercase(),
    })
}

/// The real converter: `cargo run -p ui2-convert -- fragment <dir> --out <out>` in the repository.
pub fn run_converter(root: &Path, job: &FragmentJob) -> Result<FragmentOut> {
    let mut c = crate::cargo();
    c.current_dir(root)
        .args(["run", "-q", "-p", "ui2-convert", "--", "fragment"])
        .arg(&job.dir)
        .arg("--out")
        .arg(&job.out_dir);
    println!("$ {c:?}");
    let out = c
        .output()
        .with_context(|| "cannot run the converter (ui2-convert)".to_string())?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        bail!(
            "the converter failed ({}); it needs the `fragment` command to build mod panels \
             ({}=1 builds the bundle without them):\n{}{}",
            out.status,
            crate::ui::web::SKIP_VAR,
            stdout,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    parse_answer(&stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::check::{self, tests::Scratch};

    fn repo() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap()
            .to_path_buf()
    }

    const FIXTURE: &str = "tools/xtask/tests/fixtures/ui-mod";

    /// The fixture copied into a scratch folder, to be edited.
    fn copy_fixture(s: &Scratch, sub: &str) -> PathBuf {
        let to = s.0.join(sub);
        check::copy_without(&repo().join(FIXTURE), &to, false).unwrap();
        to
    }

    fn err_of(list: &Path, s: &Scratch) -> String {
        format!("{:#}", check::prepare(list, &s.0.join("work")).unwrap_err())
    }

    #[test]
    fn the_fixture_passes_the_check() {
        let s = Scratch::new();
        let p = check::prepare(&repo().join(FIXTURE).join("mods.toml"), &s.0.join("work")).unwrap();
        assert_eq!(p.resolution.set.len(), 1);
        let ui = p.mods[0].installed.manifest.ui.as_ref().unwrap();
        assert_eq!(ui.panels[0].point, "hud.right");
    }

    #[test]
    fn check_catches_an_unknown_point_with_its_key() {
        let s = Scratch::new();
        let dir = copy_fixture(&s, "a");
        let toml = std::fs::read_to_string(dir.join("mod.toml")).unwrap();
        std::fs::write(
            dir.join("mod.toml"),
            toml.replace("point    = \"hud.right\"", "point    = \"hud.nowhere\""),
        )
        .unwrap();
        let e = err_of(&dir.join("mods.toml"), &s);
        assert!(
            e.contains("ui.panels[0].point: unknown UI point \"hud.nowhere\""),
            "{e}"
        );
        assert!(e.contains("mod `uifixture`"), "{e}");
    }

    #[test]
    fn check_catches_missing_files_a_bad_model_and_unknown_reads() {
        let s = Scratch::new();
        let dir = copy_fixture(&s, "a");
        std::fs::remove_file(dir.join("ui/echo.html")).unwrap();
        let e = err_of(&dir.join("mods.toml"), &s);
        assert!(
            e.contains("ui.panels[0].source: the file ui/echo.html does not exist"),
            "{e}"
        );

        let dir = copy_fixture(&s, "b");
        std::fs::write(dir.join("ui/model.toml"), "[model.x]\na = \"i32\"\n").unwrap();
        let e = err_of(&dir.join("mods.toml"), &s);
        assert!(e.contains("ui.model: ui/model.toml"), "{e}");

        let dir = copy_fixture(&s, "c");
        let toml = std::fs::read_to_string(dir.join("mod.toml")).unwrap();
        std::fs::write(
            dir.join("mod.toml"),
            toml.replace("reads   = [\"game.turn\"]", "reads   = [\"nowhere.turn\"]"),
        )
        .unwrap();
        let e = err_of(&dir.join("mods.toml"), &s);
        assert!(e.contains("ui.reads[0]"), "{e}");
    }

    fn replacer(s: &Scratch, id: &str) {
        s.file(
            &format!("{id}/mod.toml"),
            &format!(
                "[mod]\nid = \"{id}\"\nname = \"{id}\"\nversion = \"1.0.0\"\napi = \"^1.3\"\nlicense = \"MIT\"\n\
                 sync = \"client\"\nparts = [\"ui\"]\n[ui]\napi = \"^1\"\n[[ui.panels]]\nid = \"m\"\n\
                 source = \"ui/m.html\"\npoint = \"hud.minimap\"\nmode = \"replace\"\n"
            ),
        );
        s.file(&format!("{id}/ui/m.html"), "<div class=\"m-x\"></div>\n");
    }

    #[test]
    fn check_refuses_two_mods_replacing_one_point() {
        let s = Scratch::new();
        replacer(&s, "map-a");
        replacer(&s, "map-b");
        s.file("mods.toml", "[[mod]]\npath = \"map-a\"\n");
        check::prepare(&s.0.join("mods.toml"), &s.0.join("work")).unwrap();
        s.file(
            "mods.toml",
            "[[mod]]\npath = \"map-a\"\n[[mod]]\npath = \"map-b\"\n",
        );
        let e = err_of(&s.0.join("mods.toml"), &s);
        assert!(
            e.contains(
                "mods `map-a` and `map-b` both replace the interface element `hud.minimap`; keep one of them"
            ),
            "{e}"
        );
    }

    #[test]
    fn build_writes_the_fragment_and_the_index() {
        let s = Scratch::new();
        let p = check::prepare(&repo().join(FIXTURE).join("mods.toml"), &s.0.join("work")).unwrap();
        let web = s.0.join("web");
        // A stale package from an earlier build goes away.
        s.file("web/ui2/mods/old-0.0.1-0000.ui2", "x");
        let mut jobs = Vec::new();
        let mut fake = |job: &FragmentJob| -> Result<FragmentOut> {
            jobs.push(job.clone());
            let file = format!("{}-{}-0123456789abcdef.ui2", job.id, job.version);
            std::fs::write(job.out_dir.join(&file), b"C5UI2PKG")?;
            Ok(FragmentOut {
                file,
                hash: "0123456789abcdef".repeat(4),
            })
        };
        let index = build_fragments(&p, &web, Some(&mut fake)).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, "uifixture");
        assert!(
            web.join("ui2/mods/uifixture-0.2.0-0123456789abcdef.ui2")
                .is_file()
        );
        assert!(!web.join("ui2/mods/old-0.0.1-0000.ui2").exists());
        let text = std::fs::read_to_string(&index).unwrap();
        assert_eq!(
            text,
            format!(
                "[\n  {{\"id\": \"uifixture\", \"version\": \"0.2.0\", \"file\": \"mods/uifixture-0.2.0-0123456789abcdef.ui2\", \
                 \"hash\": \"{}\", \"api\": \"^1\", \"panels\": [{{\"id\": \"echo\", \"point\": \"hud.right\", \"mode\": \"append\", \
                 \"fallback\": \"mods\"}}]}}\n]\n",
                "0123456789abcdef".repeat(4)
            )
        );
        // Skipped: an empty index, no packages.
        build_fragments(&p, &web, None).unwrap();
        assert_eq!(std::fs::read_to_string(&index).unwrap(), "[]\n");
        assert!(!web.join("ui2/mods").exists());
        // A converter that writes nothing is an error.
        let mut liar = |_: &FragmentJob| -> Result<FragmentOut> {
            Ok(FragmentOut {
                file: "nope.ui2".into(),
                hash: "00".into(),
            })
        };
        let e = format!(
            "{:#}",
            build_fragments(&p, &web, Some(&mut liar)).unwrap_err()
        );
        assert!(e.contains("did not write"), "{e}");
    }

    #[test]
    fn a_set_without_ui_gets_an_empty_index_without_the_converter() {
        let s = Scratch::new();
        let p = check::prepare(
            &repo().join("examples/mods/hello/mods.toml"),
            &s.0.join("work"),
        )
        .unwrap();
        let mut never = |_: &FragmentJob| -> Result<FragmentOut> { bail!("not called") };
        let index = build_fragments(&p, &s.0.join("web"), Some(&mut never)).unwrap();
        assert_eq!(std::fs::read_to_string(index).unwrap(), "[]\n");
    }

    #[test]
    fn the_converter_answer_and_json_strings() {
        let a = parse_answer("noise\nui2-fragment hello-0.3.0-abcd.ui2 ABCD\n").unwrap();
        assert_eq!(
            a,
            FragmentOut {
                file: "hello-0.3.0-abcd.ui2".into(),
                hash: "abcd".into()
            }
        );
        assert!(parse_answer("nothing").is_err());
        assert!(parse_answer("ui2-fragment ../x.ui2 ab").is_err());
        assert!(parse_answer("ui2-fragment x.ui2 zz").is_err());
        assert!(parse_answer("ui2-fragment x.ui2").is_err());
        assert_eq!(json_str("a\"b\\c\n"), "\"a\\\"b\\\\c\\u000a\"");
        assert_eq!(index_json(&[]), "[]\n");
    }
}
