//! UI v2 skin converter.
//!
//! The source of a skin is a folder (`skins/<id>/`) that opens in a normal browser. The converter
//! loads it in Chromium (the Node side, `node/`), takes the DOM and the stylesheets as an IR,
//! checks them against the UI profile, plans which style variants to compute, has Chromium
//! compute them, and turns the answers into the package's typed records. Every problem is an
//! error with its place; nothing is approximated.
//!
//! ```text
//! skin.toml ─► scan (Node) ─► IR ─► check: E1 E3 E5 E6 E7, the CSS as written (E2 E3)
//! ```
//!
//! Every stage runs on what the earlier ones produced, also when they found errors, so one run
//! reports as much as it can.

#![forbid(unsafe_code)]
// A converter error is a full record (place, element, property, why); it is returned rarely and
// never on a hot path, so it is not boxed.
#![allow(clippy::result_large_err)]

pub mod check;
pub mod errors;
pub mod ir;
pub mod node;
pub mod script;
pub mod select;
pub mod skin;

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::check::Statics;
use crate::errors::{ConvError, Place, codes};
use crate::ir::Ir;
use crate::node::NodeClient;
use crate::skin::Skin;

/// Named character ranges a skin may keep in its font subsets (`[fonts] subset`).
pub const SUBSETS: &[(&str, &[(u32, u32)])] = &[
    (
        "latin",
        &[
            (0x20, 0x7e),
            (0xa0, 0xff),
            (0x2013, 0x2014),
            (0x2018, 0x201e),
            (0x2022, 0x2022),
            (0x2026, 0x2026),
            (0x2212, 0x2212),
        ],
    ),
    ("latin-ext", &[(0x100, 0x24f), (0x1e00, 0x1eff)]),
    (
        "cyrillic",
        &[(0x400, 0x45f), (0x490, 0x491), (0x2116, 0x2116)],
    ),
];

/// The format version of the IR dump (`--dump-ir`).
pub const DUMP_VERSION: u32 = 1;

/// A skin folder with its parsed manifest.
#[derive(Debug, Clone)]
pub struct Source {
    /// The folder.
    pub dir: PathBuf,
    /// `skin.toml`.
    pub skin: Skin,
}

impl Source {
    /// Reads `skin.toml` of a folder and checks that the entry file exists.
    pub fn load(dir: &Path) -> Result<Source, Vec<ConvError>> {
        let text = std::fs::read_to_string(dir.join("skin.toml")).map_err(|_| {
            vec![ConvError::new(
                codes::SKIN_DIR,
                Place::file("skin.toml"),
                "the folder has no skin.toml",
            )]
        })?;
        let skin = Skin::parse(&text.replace("\r\n", "\n"))?;
        if !dir.join(&skin.skin.entry).is_file() {
            return Err(vec![ConvError::new(
                codes::SKIN_DIR,
                Place::file("skin.toml"),
                format!("the entry `{}` does not exist", skin.skin.entry),
            )]);
        }
        Ok(Source {
            dir: dir.to_path_buf(),
            skin,
        })
    }
}

/// The scan of the entry page.
#[derive(Debug, Clone)]
pub struct Scanned {
    /// The IR as Node wrote it (kept verbatim for the dump).
    pub json: Value,
    /// The IR, typed.
    pub ir: Ir,
    /// Whether the preview script expanded includes and bindings in the page.
    pub preview: bool,
}

impl Scanned {
    /// From an IR as Node wrote it.
    pub fn from_json(json: Value, preview: bool) -> Result<Scanned, ConvError> {
        let ir: Ir = serde_json::from_value(json.clone()).map_err(|e| {
            ConvError::new(
                codes::PROTOCOL,
                Place::default(),
                format!("the IR does not parse: {e}"),
            )
        })?;
        Ok(Scanned { json, ir, preview })
    }
}

/// Opens the skin in Chromium and scans the entry page.
pub fn scan(node: &mut NodeClient, src: &Source) -> Result<Scanned, ConvError> {
    // Node runs in its own folder: the skin folder goes over as an absolute path (it never reaches
    // an output).
    let dir = std::path::absolute(&src.dir)
        .unwrap_or_else(|_| src.dir.clone())
        .to_string_lossy()
        .to_string();
    let opened = node.call(
        "open",
        json!({ "skin_dir": dir, "entry": src.skin.skin.entry, "preview": true }),
    )?;
    let preview = opened.get("preview") == Some(&Value::Bool(true));
    let json = node.call("scan", json!({}))?;
    Scanned::from_json(json, preview)
}

/// Where the page comes from.
pub enum Input<'a> {
    /// Load it in Chromium through Node.
    Chrome,
    /// A dump written by an earlier run (`--dump-ir`); no browser is needed.
    Dump(&'a str),
}

/// One run of the pipeline: what each stage produced and every error found.
#[derive(Debug, Clone, Default)]
pub struct Run {
    /// The manifest.
    pub source: Option<Source>,
    /// The scan.
    pub scanned: Option<Scanned>,
    /// The static pass.
    pub statics: Option<Statics>,
    /// Every error, sorted once the run ends.
    pub errors: Vec<ConvError>,
}

impl Run {
    /// The stored form of the run (`--dump-ir`): the IR and what later stages read from Chrome.
    pub fn dump(&self) -> Option<String> {
        let scanned = self.scanned.as_ref()?;
        let v = json!({
            "v": DUMP_VERSION,
            "preview": scanned.preview,
            "scan": scanned.json,
        });
        let mut s = serde_json::to_string_pretty(&v).ok()?;
        s.push('\n');
        Some(s)
    }
}

/// The parts of a dump.
pub struct Dump {
    /// The scan.
    pub scanned: Scanned,
    /// The whole dump, for the later stages' parts.
    pub json: Value,
}

/// Reads a dump written by [`Run::dump`].
pub fn read_dump(text: &str) -> Result<Dump, ConvError> {
    let json: Value = serde_json::from_str(text).map_err(|e| {
        ConvError::new(
            codes::PROTOCOL,
            Place::default(),
            format!("the dump does not parse: {e}"),
        )
    })?;
    if json.get("v") != Some(&json!(DUMP_VERSION)) {
        return Err(ConvError::new(
            codes::PROTOCOL,
            Place::default(),
            "the dump has another format version",
        ));
    }
    let scan = json.get("scan").cloned().unwrap_or(Value::Null);
    let preview = json.get("preview") == Some(&Value::Bool(true));
    Ok(Dump {
        scanned: Scanned::from_json(scan, preview)?,
        json,
    })
}

/// Whether the page still holds includes or loop bindings that only the preview expands.
fn unexpanded(ir: &Ir) -> bool {
    let dom = ir::Dom::new(&ir.dom);
    dom.elems.iter().any(|e| {
        e.attr("data-include").is_some()
            || e.attr("data-bind-for").is_some()
            || e.attr("data-bind-if").is_some()
    })
}

/// `check`: loads the manifest, scans the page (or reads a dump) and runs every check.
pub fn check_skin(dir: &Path, input: Input) -> Run {
    let mut run = Run::default();
    let src = match Source::load(dir) {
        Ok(s) => s,
        Err(e) => {
            run.errors = e;
            errors::sort(&mut run.errors);
            return run;
        }
    };
    let mut node = None;
    let scanned = match input {
        Input::Dump(text) => read_dump(text).map(|d| d.scanned),
        Input::Chrome => match NodeClient::start() {
            Ok(mut n) => {
                let s = scan(&mut n, &src);
                node = Some(n);
                s
            }
            Err(e) => Err(e),
        },
    };
    match scanned {
        Ok(s) => run.scanned = Some(s),
        Err(e) => run.errors.push(e),
    }
    if let Some(scanned) = &run.scanned {
        let (statics, errs) = check::check(&src.skin, &scanned.ir, &src.dir);
        run.errors.extend(errs);
        if !scanned.preview && unexpanded(&scanned.ir) {
            run.errors.push(ConvError::new(
                codes::PREVIEW,
                Place::file(&src.skin.skin.entry),
                "the page has includes or loop bindings that only the preview script expands; the checks saw the entry file as written",
            ));
        }
        run.statics = Some(statics);
    }
    if let Some(n) = node {
        n.close();
    }
    run.source = Some(src);
    errors::sort(&mut run.errors);
    run
}
