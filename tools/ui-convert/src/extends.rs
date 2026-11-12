//! Derived skins (`[skin] extends = "<base>"`).
//!
//! A derived skin folder holds only the files it changes (a stylesheet of tokens, say) and a small `skin.toml`.
//! [`resolve`] lays the base skin and the derived one over each other in a folder under the target folder, and
//! the rest of the pipeline works on that folder as on any other skin:
//!
//! - every file of the base is there; a file of the derived skin replaces the base file of the same path;
//! - `skin.toml` is the base manifest with the tables the derived one writes (`[skin]`, `[fonts]`, `[state]`,
//!   `[themes]`, `[scripts]`, `[verify]`) replacing the base tables as a whole, so `[skin]` is always the
//!   derived skin's own, without `extends` (the merged folder is a plain skin);
//! - a text table (`l10n/<lang>.toml`) of the derived skin that the base also has is merged key by key: the
//!   derived keys win, the other keys stay the base's (a derived skin can give just its own title).
//!
//! The base is looked up as a folder next to the derived one.

use std::path::{Path, PathBuf};

use crate::errors::{ConvError, Place, codes};

/// How deep a chain of `extends` may go.
const MAX_DEPTH: usize = 4;

fn bad(why: impl Into<String>) -> ConvError {
    ConvError::new(codes::SKIN_DIR, Place::file("skin.toml"), why)
}

/// The folder the pipeline reads for a skin folder: the folder itself, or the merged folder of a derived skin.
pub fn resolve(dir: &Path) -> Result<PathBuf, ConvError> {
    resolve_at(dir, 0)
}

fn read_manifest(dir: &Path) -> Result<toml::Table, ConvError> {
    let text = std::fs::read_to_string(dir.join("skin.toml"))
        .map_err(|_| bad("the folder has no skin.toml"))?;
    text.replace("\r\n", "\n")
        .parse::<toml::Table>()
        .map_err(|e| bad(e.message().to_string()))
}

fn resolve_at(dir: &Path, depth: usize) -> Result<PathBuf, ConvError> {
    let mut own = read_manifest(dir)?;
    let Some(base_id) = own
        .get("skin")
        .and_then(|s| s.get("extends"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
    else {
        return Ok(dir.to_path_buf());
    };
    if depth >= MAX_DEPTH {
        return Err(bad(format!(
            "[skin] extends: more than {MAX_DEPTH} levels (a loop?)"
        )));
    }
    let own_id = own
        .get("skin")
        .and_then(|s| s.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let abs = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
    let base_dir = abs
        .parent()
        .map(|p| p.join(&base_id))
        .filter(|p| p.join("skin.toml").is_file())
        .ok_or_else(|| bad(format!("[skin] extends `{base_id}`: no such skin folder")))?;
    let base_root = resolve_at(&base_dir, depth + 1)?;
    let mut merged = read_manifest(&base_root)?;
    for (k, v) in std::mem::take(&mut own) {
        merged.insert(k, v);
    }
    if let Some(skin) = merged.get_mut("skin").and_then(|s| s.as_table_mut()) {
        skin.remove("extends");
    }
    let texts = merged_texts(&base_root, &abs)?;
    let out = crate::build::target_dir()
        .join("ui2")
        .join("merged")
        .join(&own_id);
    let io = |e: std::io::Error| bad(format!("cannot lay out the derived skin `{own_id}`: {e}"));
    if out.exists() {
        std::fs::remove_dir_all(&out).map_err(io)?;
    }
    copy_tree(&base_root, &out).map_err(io)?;
    copy_tree(&abs, &out).map_err(io)?;
    for (rel, text) in texts {
        std::fs::write(out.join(rel), text).map_err(io)?;
    }
    let text = toml::to_string(&merged).map_err(|e| bad(e.to_string()))?;
    std::fs::write(out.join("skin.toml"), text).map_err(io)?;
    Ok(out)
}

/// The folders a skin's merged copy is made of: the skin folder, then its bases (nearest first). A plain skin
/// is just itself. A folder whose manifest cannot be read ends the list.
pub fn sources(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut cur = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
    while out.len() <= MAX_DEPTH {
        out.push(cur.clone());
        let base = read_manifest(&cur)
            .ok()
            .and_then(|m| m.get("skin")?.get("extends")?.as_str().map(str::to_string));
        match base.and_then(|b| cur.parent().map(|p| p.join(b))) {
            Some(next) if next.join("skin.toml").is_file() => cur = next,
            _ => break,
        }
    }
    out
}

/// Puts the keys of `over` into `base`, descending into tables.
fn merge_keys(base: &mut toml::Table, over: toml::Table) {
    for (k, v) in over {
        match (base.get_mut(&k), v) {
            (Some(toml::Value::Table(b)), toml::Value::Table(o)) => merge_keys(b, o),
            (_, v) => {
                base.insert(k, v);
            }
        }
    }
}

/// The text tables the derived skin overrides key by key: (path relative to the skin, merged text), for each
/// `l10n/*.toml` that both the base and the derived folder have.
fn merged_texts(base: &Path, own: &Path) -> Result<Vec<(String, String)>, ConvError> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(own.join("l10n")) else {
        return Ok(out);
    };
    let mut names: Vec<String> = rd
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".toml") && base.join("l10n").join(n).is_file())
        .collect();
    names.sort();
    for n in names {
        let load = |dir: &Path| -> Result<toml::Table, ConvError> {
            let rel = format!("l10n/{n}");
            std::fs::read_to_string(dir.join(&rel))
                .map_err(|e| bad(format!("{rel}: {e}")))?
                .replace("\r\n", "\n")
                .parse::<toml::Table>()
                .map_err(|e| bad(format!("{rel}: {}", e.message())))
        };
        let mut table = load(base)?;
        merge_keys(&mut table, load(own)?);
        let text = toml::to_string(&table).map_err(|e| bad(e.to_string()))?;
        out.push((format!("l10n/{n}"), text));
    }
    Ok(out)
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let target = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_tree(&e.path(), &target)?;
        } else {
            std::fs::copy(e.path(), &target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_derived_skin_is_the_base_with_its_own_files_over_it() {
        let root = crate::build::target_dir().join("ui2").join("extends-test");
        let _ = std::fs::remove_dir_all(&root);
        let (base, derived) = (root.join("base"), root.join("derived-t"));
        for d in [&base, &derived] {
            std::fs::create_dir_all(d.join("css")).expect("dir");
        }
        let head = "version = \"0.1.0\"\ntitle = \"t\"\nmodel = \"1.0\"\napi = \"1.0\"\n";
        std::fs::write(
            base.join("skin.toml"),
            format!("[skin]\nid = \"base\"\n{head}default = true\n[verify]\nstates = [\"hud\"]\n"),
        )
        .expect("write");
        std::fs::write(
            derived.join("skin.toml"),
            format!("[skin]\nid = \"derived-t\"\n{head}extends = \"base\"\n"),
        )
        .expect("write");
        std::fs::write(base.join("index.html"), "base page").expect("write");
        std::fs::write(base.join("css/tokens.css"), "base tokens").expect("write");
        std::fs::write(derived.join("css/tokens.css"), "own tokens").expect("write");

        let out = resolve(&derived).expect("resolves");
        let read = |p: &str| std::fs::read_to_string(out.join(p)).expect(p);
        assert_eq!(read("index.html"), "base page");
        assert_eq!(read("css/tokens.css"), "own tokens");
        let manifest = read("skin.toml");
        assert!(manifest.contains("derived-t") && manifest.contains("states"));
        assert!(!manifest.contains("default"), "{manifest}");
        assert!(!manifest.contains("extends"), "{manifest}");
        // The merged folder is a plain skin: resolving it again changes nothing.
        assert_eq!(resolve(&out).expect("resolves"), out);
        assert_eq!(sources(&derived).len(), 2);
        std::fs::remove_dir_all(&root).expect("clean");
    }

    #[test]
    fn a_derived_text_table_overrides_single_keys() {
        let root = crate::build::target_dir().join("ui2").join("extends-l10n");
        let _ = std::fs::remove_dir_all(&root);
        let (base, derived) = (root.join("base"), root.join("derived-l"));
        for d in [&base, &derived] {
            std::fs::create_dir_all(d.join("l10n")).expect("dir");
        }
        let head = "version = \"0.1.0\"\ntitle = \"t\"\nmodel = \"1.0\"\napi = \"1.0\"\n";
        let put = |p: PathBuf, text: &str| std::fs::write(p, text).expect("write");
        put(
            base.join("skin.toml"),
            &format!("[skin]\nid = \"base\"\n{head}"),
        );
        put(
            derived.join("skin.toml"),
            &format!("[skin]\nid = \"derived-l\"\n{head}extends = \"base\"\n"),
        );
        put(
            base.join("l10n/en_US.toml"),
            "\"skin.title\" = \"Base\"\n\"hud.a\" = \"A\"\n",
        );
        put(base.join("l10n/ru_RU.toml"), "\"hud.a\" = \"RU\"\n");
        put(
            derived.join("l10n/en_US.toml"),
            "\"skin.title\" = \"Own\"\n",
        );
        put(derived.join("l10n/de_DE.toml"), "\"hud.a\" = \"DE\"\n");

        let out = resolve(&derived).expect("resolves");
        let table = |f: &str| -> toml::Table {
            std::fs::read_to_string(out.join("l10n").join(f))
                .expect(f)
                .parse()
                .expect(f)
        };
        let en = table("en_US.toml");
        assert_eq!(en["skin.title"].as_str(), Some("Own"));
        assert_eq!(en["hud.a"].as_str(), Some("A"));
        assert_eq!(table("ru_RU.toml")["hud.a"].as_str(), Some("RU"));
        assert_eq!(table("de_DE.toml")["hud.a"].as_str(), Some("DE"));
        std::fs::remove_dir_all(&root).expect("clean");
    }

    #[test]
    fn an_unresolved_derived_skin_is_refused_by_the_entry_points() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skins/vellum");
        let first = |errs: &[ConvError]| errs.first().map(|e| e.code.clone()).unwrap_or_default();
        let errs = crate::Source::load(&dir).expect_err("refused");
        assert_eq!(first(&errs), codes::SKIN_DIR);
        assert!(
            errs[0].to_string().contains("extends::resolve"),
            "{}",
            errs[0]
        );
        let run = crate::check_skin(&dir, crate::Input::Dump(""));
        assert_eq!(first(&run.errors), codes::SKIN_DIR);
        let built = crate::build::build(&dir, false);
        assert_eq!(first(&built.run.errors), codes::SKIN_DIR);
        let errs = crate::verify::verify(&dir, "hud").expect_err("refused");
        assert_eq!(first(&errs), codes::SKIN_DIR);
        let merged = resolve(&dir).expect("resolves");
        assert!(crate::Source::load(&merged).is_ok());
    }

    #[test]
    fn a_plain_skin_resolves_to_itself() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skins/engraved");
        assert_eq!(resolve(&dir).expect("resolves"), dir);
    }
}
