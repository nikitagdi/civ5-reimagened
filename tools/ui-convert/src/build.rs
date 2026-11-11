//! `build` and `check`: the whole pipeline.
//!
//! ```text
//! pass 1  scan + extract in Chromium (fonts not served yet) ─► texts ─► the glyph set
//!         @font-face rules ─► subsets of the font files to the glyph set ─► <out>/fonts/<path>
//! pass 2  scan + extract again, the preview server serving the subsets: Chrome lays out and shapes with the
//!         bytes that go into the package; every face must load (E5-004) and the glyph set must not change
//!         pictures ─► raster in Chromium ─► atlas pages (assets.rs, atlas.rs)
//!         write ─► budget (E8) ─► <out>/<id>-<hash16>.ui2, build.json, errors.json, ir.json
//! ```
//!
//! `check` runs the same passes and stops before writing the package.

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::errors::{self, ConvError, Place, codes};
use crate::fonts::{self, Subset};
use crate::ir::Dom;
use crate::write::{self, Inputs, Pictures, Tools, Written};
use crate::{Input, Opts, Run, check_skin_with};

/// The result of a build.
#[derive(Debug, Default)]
pub struct BuildOut {
    /// The run that went into the package (pass 2), with every error.
    pub run: Run,
    /// The package, when it was written.
    pub written: Option<Written>,
    /// The output folder.
    pub out_dir: PathBuf,
}

/// The output folder of a skin: `<target>/ui2/<id>`.
pub fn out_dir(id: &str) -> PathBuf {
    target_dir().join("ui2").join(id)
}

/// The cargo target folder (`CARGO_TARGET_DIR`, else `target/` of the repository).
pub fn target_dir() -> PathBuf {
    match std::env::var_os("CARGO_TARGET_DIR") {
        Some(d) => PathBuf::from(d),
        None => fonts::repo_root().join("target"),
    }
}

fn flags_hash() -> String {
    let bytes =
        std::fs::read(crate::node::node_dir().join("chromium-flags.json")).unwrap_or_default();
    // The flags list as a file of the repository: hash it with LF line ends whatever the checkout has.
    let text = String::from_utf8_lossy(&bytes).replace("\r\n", "\n");
    blake3::hash(text.as_bytes()).to_hex().to_string()
}

fn scripts(dir: &Path, run: &Run) -> (Vec<(String, Vec<u8>)>, Vec<ConvError>) {
    let mut out = Vec::new();
    let mut errs = Vec::new();
    let Some(src) = &run.source else {
        return (out, errs);
    };
    for f in &src.skin.scripts.files {
        match std::fs::read_to_string(dir.join(f)) {
            Ok(t) => out.push((f.clone(), t.replace("\r\n", "\n").into_bytes())),
            Err(e) => errs.push(ConvError::new(
                codes::RESOURCE,
                Place::file("skin.toml"),
                format!("[scripts] files: cannot read {f}: {e}"),
            )),
        }
    }
    (out, errs)
}

/// Runs the pipeline; writes the package when `write_package`.
pub fn build(dir: &Path, write_package: bool) -> BuildOut {
    let pass1 = check_skin_with(dir, Input::Chrome, &Opts::default());
    let (Some(src), Some(scanned), Some(texts)) = (&pass1.source, &pass1.scanned, &pass1.texts)
    else {
        return BuildOut {
            run: pass1,
            ..BuildOut::default()
        };
    };
    let id = src.skin.skin.id.clone();
    let out = out_dir(&id);
    let fonts_dir = out.join("fonts");
    let (decls, mut errs) = fonts::faces(&scanned.ir, &src.skin);
    let ranges = texts.glyphs.codepoints();
    let subsets: Vec<Subset> = if errs.is_empty() {
        match fonts::subset_all(
            &decls,
            &ranges,
            &fonts_dir,
            &target_dir().join("ui2").join("font-cache"),
        ) {
            Ok(s) => s,
            Err(e) => {
                errs.extend(e);
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    if !errs.is_empty() {
        let mut run = pass1;
        run.errors.extend(errs);
        errors::sort(&mut run.errors);
        return BuildOut {
            run,
            out_dir: out,
            ..BuildOut::default()
        };
    }

    let mut run = check_skin_with(
        dir,
        Input::Chrome,
        &Opts {
            fonts_dir: Some(fonts_dir),
        },
    );
    if let Some(s) = &run.scanned {
        for f in &s.ir.fonts {
            if f.status != "loaded" {
                run.errors.push(ConvError::new(
                    codes::FONT_LOAD,
                    Place::default(),
                    format!(
                        "{} {} {}: status {} in Chromium",
                        f.family, f.weight, f.style, f.status
                    ),
                ));
            }
        }
    }
    if let Some(t) = &run.texts
        && t.glyphs.codepoints() != ranges
    {
        run.errors.push(ConvError::new(
            codes::PROTOCOL,
            Place::default(),
            "the glyph set changed between the two passes",
        ));
    }
    let (scr, e) = scripts(dir, &run);
    run.errors.extend(e);

    let pictures = if run.errors.is_empty() {
        match pictures_of(&run, &fonts_dir_of(&out)) {
            Ok(p) => p,
            Err(e) => {
                run.errors.extend(e);
                Pictures::default()
            }
        }
    } else {
        Pictures::default()
    };
    let mut written = None;
    if run.errors.is_empty() {
        match package_of(&run, &subsets, &pictures, &scr) {
            Ok(w) => {
                let e = write::check_budget(&w.sections);
                if e.is_empty() {
                    written = Some(w);
                } else {
                    run.errors.extend(e);
                }
            }
            Err(e) => run.errors.extend(e),
        }
    }
    errors::sort(&mut run.errors);

    if let Err(e) = save(&out, &run, written.as_ref(), write_package) {
        run.errors
            .push(ConvError::new(codes::WRITE, Place::default(), e));
    }
    if !write_package {
        written = written.map(|mut w| {
            w.bytes.clear();
            w
        });
    }
    BuildOut {
        run,
        written,
        out_dir: out,
    }
}

fn fonts_dir_of(out: &Path) -> PathBuf {
    out.join("fonts")
}

/// The pictures of a run, rasterized in a Chromium session of their own.
pub fn pictures_of(run: &Run, fonts_dir: &Path) -> Result<Pictures, Vec<ConvError>> {
    let (Some(src), Some(scanned), Some(styles)) = (&run.source, &run.scanned, &run.styles) else {
        return Ok(Pictures::default());
    };
    let dom = Dom::new(&scanned.ir.dom);
    let mut node = crate::node::NodeClient::start().map_err(|e| vec![e])?;
    let opts = Opts {
        fonts_dir: Some(fonts_dir.to_path_buf()),
    };
    let r = crate::open(&mut node, src, &opts)
        .map_err(|e| vec![e])
        .and_then(|_| crate::assets::pictures(&mut node, &src.dir, &dom, styles));
    node.close();
    r
}

/// The package of a run (every stage present) with its fonts, pictures and scripts.
pub fn package_of(
    run: &Run,
    fonts: &[Subset],
    pictures: &Pictures,
    scripts: &[(String, Vec<u8>)],
) -> Result<Written, Vec<ConvError>> {
    let (Some(src), Some(scanned), Some(styles), Some(bound), Some(texts)) = (
        &run.source,
        &run.scanned,
        &run.styles,
        &run.bindings,
        &run.texts,
    ) else {
        return Err(vec![ConvError::new(
            codes::WRITE,
            Place::default(),
            "a stage of the run is missing",
        )]);
    };
    let dom = Dom::new(&scanned.ir.dom);
    let tools = Tools {
        chrome: run.chrome.split('.').next().unwrap_or_default().to_string(),
        chrome_flags: flags_hash(),
        fonttools: fonts::FONTTOOLS.to_string(),
    };
    write::write(&Inputs {
        skin: &src.skin,
        dom: &dom,
        styles,
        bound,
        texts,
        fonts,
        pictures,
        scripts,
        tools: &tools,
    })
}

/// Writes the outputs: the package (when asked), `build.json`, `errors.json`, `ir.json`.
fn save(out: &Path, run: &Run, w: Option<&Written>, package: bool) -> Result<(), String> {
    std::fs::create_dir_all(out).map_err(|e| format!("cannot create the output folder: {e}"))?;
    let errs: Vec<serde_json::Value> = run
        .errors
        .iter()
        .map(|e| serde_json::to_value(e).unwrap_or_default())
        .collect();
    let text = serde_json::to_string_pretty(&errs).unwrap_or_default() + "\n";
    std::fs::write(out.join("errors.json"), text).map_err(|e| e.to_string())?;
    if let Some(d) = run.dump() {
        std::fs::write(out.join("ir.json"), d).map_err(|e| e.to_string())?;
    }
    let Some(w) = w else {
        return Ok(());
    };
    if !package {
        return Ok(());
    }
    // One package per skin in the folder: older builds are removed.
    if let Ok(rd) = std::fs::read_dir(out) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.ends_with(".ui2") && n != w.file_name {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    std::fs::write(out.join(&w.file_name), &w.bytes).map_err(|e| e.to_string())?;
    std::fs::write(out.join("build.json"), build_json(run, w)).map_err(|e| e.to_string())?;
    Ok(())
}

/// `build.json`: the file, its hash, the section sizes and the budget.
pub fn build_json(run: &Run, w: &Written) -> String {
    let src = run.source.as_ref();
    let sections: serde_json::Map<String, serde_json::Value> = w
        .sections
        .iter()
        .map(|(t, n)| (t.clone(), json!(n)))
        .collect();
    let budget: Vec<serde_json::Value> = write::budget(&w.sections)
        .into_iter()
        .map(|(what, n, max)| json!({ "what": what, "bytes": n, "limit": max }))
        .collect();
    let v = json!({
        "id": src.map(|s| s.skin.skin.id.clone()),
        "file": w.file_name,
        "hash": w.hash,
        "size": w.bytes.len(),
        "sections": sections,
        "budget": budget,
        "title_key": src.map(|s| s.skin.skin.title.clone()),
        "default": src.map(|s| s.skin.skin.default),
        "api": src.map(|s| s.skin.skin.api.clone()),
    });
    serde_json::to_string_pretty(&v).unwrap_or_default() + "\n"
}
