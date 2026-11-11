//! Fonts: the faces a skin declares, and their subsets.
//!
//! A face is an `@font-face` rule of the skin's stylesheets: family, weight (the lower end of a range: a
//! variable face is chosen by its first weight and keeps its `wght` axis), style and a `src` that names a
//! path inside the skin (`fonts/FiraSans-Regular.ttf`); `skin.toml [fonts.files]` maps that path to the font
//! file in the repository. Every declared face is subset to the skin's glyph set (the named ranges of
//! `[fonts] subset` and every character of the texts) by `fonts/subset.py` (Python `fontTools`, the version
//! pinned in `fonts/requirements.txt`), with the format, the layout features and the variation axes kept.
//!
//! The subsets are written to `<out>/<path>`: the preview server serves them under the path the stylesheet
//! names, so Chrome lays out the page with the same bytes that go into the package and that the runtime
//! shapes. A cache under `target/ui2/font-cache/` is keyed by the source bytes, the arguments, the subsetter
//! script and its version.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::errors::{ConvError, Place, codes};
use crate::ir::{CssItem, Ir};
use crate::skin::Skin;

/// The pinned fontTools version (the line of `fonts/requirements.txt`).
pub const FONTTOOLS: &str = "4.62.1";

/// One `@font-face` of the skin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaceDecl {
    /// `font-family`, unquoted.
    pub family: String,
    /// The weight, or the lower end of a weight range.
    pub weight: u16,
    /// `font-style: italic`.
    pub italic: bool,
    /// The path the stylesheet names, relative to the skin (`fonts/FiraSans-Regular.ttf`).
    pub path: String,
    /// The font file, relative to the repository (`third_party/fonts/...`).
    pub file: String,
    /// The rule's place.
    pub place: Place,
}

/// A face with its subset.
#[derive(Debug, Clone)]
pub struct Subset {
    /// The face.
    pub decl: FaceDecl,
    /// The subset font file.
    pub bytes: Vec<u8>,
}

/// The repository root (the converter is a tool of this repository).
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn unquote(s: &str) -> String {
    s.trim().trim_matches(|c| c == '"' || c == '\'').to_string()
}

/// `url("../fonts/x.ttf")` in a sheet of `css/base.css` → `fonts/x.ttf`.
fn resolve_url(value: &str, sheet_file: &str) -> Option<String> {
    let start = value.find("url(")? + 4;
    let end = start + value[start..].find(')')?;
    let raw = unquote(&value[start..end]);
    if raw.contains(':') || raw.starts_with('/') {
        return None;
    }
    let mut parts: Vec<&str> = sheet_file.split('/').collect();
    parts.pop();
    for seg in raw.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    Some(parts.join("/"))
}

/// The `@font-face` rules of the page's stylesheets, in document order (E5 for a face without a file).
pub fn faces(ir: &Ir, skin: &Skin) -> (Vec<FaceDecl>, Vec<ConvError>) {
    let mut out = Vec::new();
    let mut errs = Vec::new();
    for sheet in &ir.sheets {
        let file = sheet.source_file();
        for item in &sheet.rules {
            let CssItem::Atrule {
                name,
                line,
                col,
                items: Some(decls),
                ..
            } = item
            else {
                continue;
            };
            if name != "font-face" {
                continue;
            }
            let place = Place::at(&file, *line, *col);
            let get = |p: &str| {
                decls.iter().find_map(|d| match d {
                    CssItem::Decl { prop, value, .. } if prop == p => Some(value.clone()),
                    _ => None,
                })
            };
            let family = get("font-family").map(|f| unquote(&f)).unwrap_or_default();
            let weight = get("font-weight")
                .and_then(|w| {
                    let first = w.split_whitespace().next()?.to_string();
                    match first.as_str() {
                        "normal" => Some(400),
                        "bold" => Some(700),
                        n => n.parse().ok(),
                    }
                })
                .unwrap_or(400);
            let italic = get("font-style").is_some_and(|s| s.contains("italic"));
            let src = get("src").unwrap_or_default();
            let Some(path) = resolve_url(&src, &file) else {
                errs.push(
                    ConvError::new(
                        codes::FONT_FILE,
                        place.clone(),
                        format!("`src: {src}` is not a file of the skin"),
                    )
                    .prop("src"),
                );
                continue;
            };
            let Some(repo_file) = skin.fonts.files.get(&path) else {
                errs.push(
                    ConvError::new(
                        codes::FONT_FILE,
                        place.clone(),
                        format!("`{path}` is not listed in skin.toml [fonts.files]"),
                    )
                    .prop("src"),
                );
                continue;
            };
            out.push(FaceDecl {
                family,
                weight,
                italic,
                path,
                file: repo_file.clone(),
                place,
            });
        }
    }
    (out, errs)
}

/// The code points as the subsetter's `--unicodes` (`20-7E,A0-FF`).
pub fn unicodes(ranges: &[(u32, u32)]) -> String {
    ranges
        .iter()
        .map(|(a, b)| {
            if a == b {
                format!("{a:X}")
            } else {
                format!("{a:X}-{b:X}")
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn python() -> String {
    std::env::var("CIV5_PYTHON").unwrap_or_else(|_| "python".to_string())
}

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fonts")
        .join("subset.py")
}

fn tool_error(why: String) -> ConvError {
    ConvError::new(codes::FONTTOOLS, Place::default(), why)
}

/// Runs the subsetter script. `-E` ignores Python's environment variables; the user's site packages stay
/// visible (fontTools is installed with `pip install --user`).
fn run(args: &[&str]) -> Result<String, ConvError> {
    let out = Command::new(python())
        .arg("-E")
        .arg(script())
        .args(args)
        .output()
        .map_err(|e| tool_error(format!("python does not start ({e}); set CIV5_PYTHON")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let last = err.lines().last().unwrap_or("").to_string();
        return Err(tool_error(format!("the subsetter failed: {last}")));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The fontTools version, checked against the pin.
pub fn fonttools_version() -> Result<String, ConvError> {
    let v = run(&["--version"])?;
    if v != FONTTOOLS {
        return Err(tool_error(format!(
            "fontTools {v} is installed, the converter needs {FONTTOOLS}"
        )));
    }
    Ok(v)
}

/// Subsets every face to `ranges`, writes the subsets to `<out>/<path>` and returns them in the order of
/// `decls`. `cache` holds earlier results.
pub fn subset_all(
    decls: &[FaceDecl],
    ranges: &[(u32, u32)],
    out: &Path,
    cache: &Path,
) -> Result<Vec<Subset>, Vec<ConvError>> {
    let version = fonttools_version().map_err(|e| vec![e])?;
    let script_bytes = std::fs::read(script()).unwrap_or_default();
    let uni = unicodes(ranges);
    let mut done = Vec::new();
    let mut errs = Vec::new();
    for d in decls {
        let src = repo_root().join(&d.file);
        let bytes = match std::fs::read(&src) {
            Ok(b) => b,
            Err(e) => {
                errs.push(
                    ConvError::new(
                        codes::FONT_FILE,
                        d.place.clone(),
                        format!("cannot read {}: {e}", d.file),
                    )
                    .prop("src"),
                );
                continue;
            }
        };
        let mut h = blake3::Hasher::new();
        for part in [
            &bytes[..],
            uni.as_bytes(),
            version.as_bytes(),
            &script_bytes[..],
        ] {
            h.update(&(part.len() as u64).to_le_bytes());
            h.update(part);
        }
        let key = h.finalize().to_hex().to_string();
        let cached = cache.join(format!("{key}.ttf"));
        let subset = match std::fs::read(&cached) {
            Ok(b) => b,
            Err(_) => {
                let _ = std::fs::create_dir_all(cache);
                let tmp = cache.join(format!("{key}.tmp.ttf"));
                let r = run(&[&src.to_string_lossy(), &tmp.to_string_lossy(), uni.as_str()]);
                if let Err(e) = r {
                    errs.push(e);
                    continue;
                }
                match std::fs::read(&tmp) {
                    Ok(b) => {
                        let _ = std::fs::rename(&tmp, &cached);
                        b
                    }
                    Err(e) => {
                        errs.push(tool_error(format!("the subset was not written: {e}")));
                        continue;
                    }
                }
            }
        };
        let dest = out.join(&d.path);
        if let Some(parent) = dest.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&dest, &subset) {
            errs.push(tool_error(format!(
                "cannot write the subset {}: {e}",
                d.path
            )));
            continue;
        }
        done.push(Subset {
            decl: d.clone(),
            bytes: subset,
        });
    }
    if errs.is_empty() { Ok(done) } else { Err(errs) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_resolve_against_the_sheet() {
        assert_eq!(
            resolve_url("url(\"../fonts/A.ttf\")", "css/base.css").as_deref(),
            Some("fonts/A.ttf")
        );
        assert_eq!(
            resolve_url("url(fonts/A.ttf)", "index.html").as_deref(),
            Some("fonts/A.ttf")
        );
        assert_eq!(resolve_url("url(https://x/a.ttf)", "css/a.css"), None);
        assert_eq!(resolve_url("url(../../a.ttf)", "css/a.css"), None);
    }

    #[test]
    fn unicodes_are_hex_ranges() {
        assert_eq!(unicodes(&[(0x20, 0x7e), (0xa0, 0xa0)]), "20-7E,A0");
    }
}
