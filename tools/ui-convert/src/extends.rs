//! Derived skins (`[skin] extends = "<base>"`).
//!
//! A derived skin folder holds only the files it changes (a stylesheet of tokens, say) and a small `skin.toml`.
//! [`resolve`] lays the base skin and the derived one over each other in a folder under the target folder, and
//! the rest of the pipeline works on that folder as on any other skin:
//!
//! - every file of the base is there; a file of the derived skin replaces the base file of the same path;
//! - `skin.toml` is the base manifest with the tables the derived one writes (`[skin]`, `[fonts]`, `[state]`,
//!   `[themes]`, `[scripts]`, `[verify]`) replacing the base tables as a whole, so `[skin]` is always the
//!   derived skin's own.
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
    let text = toml::to_string(&merged).map_err(|e| bad(e.to_string()))?;
    std::fs::write(out.join("skin.toml"), text).map_err(io)?;
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
        std::fs::remove_dir_all(&root).expect("clean");
    }

    #[test]
    fn a_plain_skin_resolves_to_itself() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skins/engraved");
        assert_eq!(resolve(&dir).expect("resolves"), dir);
    }
}
