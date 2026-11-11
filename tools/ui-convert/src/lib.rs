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
//!           ─► plan (conditions, cells, beacons) ─► extract (Node) ─► styles (E1 E2 E3 E7)
//! ```
//!
//! Every stage runs on what the earlier ones produced, also when they found errors, so one run
//! reports as much as it can.

#![forbid(unsafe_code)]
// A converter error is a full record (place, element, property, why); it is returned rarely and
// never on a hot path, so it is not boxed.
#![allow(clippy::result_large_err)]

pub mod assets;
pub mod atlas;
pub mod bindings;
pub mod build;
pub mod check;
pub mod errors;
pub mod fonts;
pub mod index;
pub mod ir;
pub mod l10n;
pub mod node;
pub mod numbering;
pub mod plan;
pub mod script;
pub mod select;
pub mod skin;
pub mod styles;
pub mod verify;
pub mod write;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::check::Statics;
use crate::errors::{ConvError, Place, codes};
use crate::ir::Ir;
use crate::node::NodeClient;
use crate::plan::Plan;
use crate::skin::Skin;
use crate::styles::Compiled;

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

/// Options of a run.
#[derive(Debug, Clone, Default)]
pub struct Opts {
    /// A folder the preview server serves font files from first (the subsets, under the paths the
    /// stylesheets name).
    pub fonts_dir: Option<PathBuf>,
}

/// Opens the skin in Chromium (the preview script on, fonts served from `opts.fonts_dir`). Returns the
/// answer of `open`.
pub fn open(node: &mut NodeClient, src: &Source, opts: &Opts) -> Result<Value, ConvError> {
    let dir = std::path::absolute(&src.dir)
        .unwrap_or_else(|_| src.dir.clone())
        .to_string_lossy()
        .to_string();
    let mut open = json!({ "skin_dir": dir, "entry": src.skin.skin.entry, "preview": true });
    if let Some(f) = &opts.fonts_dir {
        let f = std::path::absolute(f).unwrap_or_else(|_| f.clone());
        open["fonts_dir"] = json!(f.to_string_lossy());
    }
    node.call("open", open)
}

/// Opens the skin in Chromium and scans the entry page. Returns the scan and the Chromium version.
pub fn scan(
    node: &mut NodeClient,
    src: &Source,
    opts: &Opts,
) -> Result<(Scanned, String), ConvError> {
    // Node runs in its own folder: the skin folder goes over as an absolute path (it never reaches
    // an output).
    let dir = std::path::absolute(&src.dir)
        .unwrap_or_else(|_| src.dir.clone())
        .to_string_lossy()
        .to_string();
    let mut open = json!({ "skin_dir": dir, "entry": src.skin.skin.entry, "preview": true });
    if let Some(f) = &opts.fonts_dir {
        let f = std::path::absolute(f).unwrap_or_else(|_| f.clone());
        open["fonts_dir"] = json!(f.to_string_lossy());
    }
    let opened = node.call("open", open)?;
    let chrome = opened
        .get("chrome")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let preview = opened.get("preview") == Some(&Value::Bool(true));
    let json = node.call("scan", json!({ "state": src.skin.scan_state() }))?;
    Ok((Scanned::from_json(json, preview)?, chrome))
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
    /// The style plan.
    pub plan: Option<Plan>,
    /// Chrome's answer to the plan, as Node wrote it (kept for the dump).
    pub extracted_json: Option<Value>,
    /// The compiled styles.
    pub styles: Option<Compiled>,
    /// The typed bindings.
    pub bindings: Option<bindings::Bound>,
    /// The texts and the glyph set.
    pub texts: Option<l10n::Texts>,
    /// Every error, sorted once the run ends.
    pub errors: Vec<ConvError>,
    /// The Chromium version of the run (empty for a dump).
    pub chrome: String,
}

impl Run {
    /// The stored form of the run (`--dump-ir`): the IR and Chrome's answers to the plan.
    pub fn dump(&self) -> Option<String> {
        let scanned = self.scanned.as_ref()?;
        let mut v = json!({
            "v": DUMP_VERSION,
            "preview": scanned.preview,
            "scan": scanned.json,
        });
        if let (Some(plan), Some(out)) = (&self.plan, &self.extracted_json) {
            v["extract"] = json!({ "request": plan.request(), "result": out });
        }
        // Compact: a dump is data for the tests, not for reading (pretty printing triples it).
        let mut s = serde_json::to_string(&v).ok()?;
        s.push('\n');
        Some(s)
    }
}

/// The parts of a dump.
pub struct Dump {
    /// The scan.
    pub scanned: Scanned,
    /// The plan the dump was extracted for.
    pub request: Option<Value>,
    /// Chrome's answer.
    pub result: Option<Value>,
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
    let ex = json.get("extract");
    Ok(Dump {
        scanned: Scanned::from_json(scan, preview)?,
        request: ex.and_then(|e| e.get("request")).cloned(),
        result: ex.and_then(|e| e.get("result")).cloned(),
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

/// Text the markup keeps as written (outside `data-l10n` and `data-bind-value` elements).
fn static_text(dom: &ir::Dom, statics: &Statics) -> Vec<String> {
    let mut out = Vec::new();
    for &ix in &statics.ui {
        let bound = std::iter::once(ix).chain(dom.ancestors(ix)).any(|a| {
            let e = dom.elems[a];
            e.attr("data-l10n").is_some() || e.attr("data-bind-value").is_some()
        });
        if bound {
            continue;
        }
        for c in &dom.elems[ix].children {
            if let ir::Node::Text(t) = c
                && !t.ws
            {
                out.push(t.text.clone());
            }
        }
    }
    out
}

/// The properties the static pass already reported as written (E2 at a declaration).
fn statically_reported(errors: &[ConvError]) -> BTreeSet<String> {
    errors
        .iter()
        .filter(|e| {
            e.code.starts_with("E2-") && !e.prop.is_empty() && e.place.file.ends_with(".css")
        })
        .map(|e| e.prop.clone())
        .collect()
}

/// `check`: loads the manifest, scans the page (or reads a dump), runs the static checks, plans
/// and extracts the styles and compiles them.
pub fn check_skin(dir: &Path, input: Input) -> Run {
    check_skin_with(dir, input, &Opts::default())
}

/// `check` with options.
pub fn check_skin_with(dir: &Path, input: Input, opts: &Opts) -> Run {
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
    let mut stored: Option<Dump> = None;
    let scanned = match input {
        Input::Dump(text) => read_dump(text).map(|d| {
            let s = d.scanned.clone();
            stored = Some(d);
            s
        }),
        Input::Chrome => match NodeClient::start() {
            Ok(mut n) => {
                let s = scan(&mut n, &src, opts).map(|(s, chrome)| {
                    run.chrome = chrome;
                    s
                });
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
        let reported = statically_reported(&errs);
        run.errors.extend(errs);
        if !scanned.preview && unexpanded(&scanned.ir) {
            run.errors.push(ConvError::new(
                codes::PREVIEW,
                Place::file(&src.skin.skin.entry),
                "the page has includes or loop bindings that only the preview script expands; the checks saw the entry file as written",
            ));
        }
        let dom = ir::Dom::new(&scanned.ir.dom);
        if scanned.preview {
            run.errors.extend(numbering::check(&dom).1);
        }
        let (p, errs) = plan::plan(&dom, &statics);
        run.errors.extend(errs);
        let request = p.request();
        let answer = match (&mut node, &stored) {
            (Some(n), _) => n.call("extract", request).map(Some),
            (None, Some(d)) => match (&d.request, &d.result) {
                (Some(req), Some(res)) if *req == request => Ok(Some(res.clone())),
                (Some(_), Some(_)) => Err(ConvError::new(
                    codes::PROTOCOL,
                    Place::default(),
                    "the stored extraction was made for another plan; dump the IR again",
                )),
                _ => Ok(None),
            },
            (None, None) => Ok(None),
        };
        match answer {
            Ok(Some(json)) => {
                match serde_json::from_value::<styles::Extracted>(json.clone()) {
                    Ok(out) => {
                        let (compiled, errs) = styles::compile(&dom, &p, &out, &reported);
                        run.errors.extend(errs);
                        run.styles = Some(compiled);
                    }
                    Err(e) => run.errors.push(ConvError::new(
                        codes::PROTOCOL,
                        Place::default(),
                        format!("the extraction does not parse: {e}"),
                    )),
                }
                run.extracted_json = Some(json);
            }
            Ok(None) => {}
            Err(e) => run.errors.push(e),
        }
        run.plan = Some(p);

        let schema = ui2_model::Schema::v1();
        let (bound, errs) = bindings::check(&dom, &statics, &src.skin, schema);
        run.errors.extend(errs);
        let (samples, errs) = bindings::check_samples(&src.dir, &src.skin, schema);
        run.errors.extend(errs);
        let (texts, errs) = l10n::check(
            &src.dir,
            &src.skin,
            &bound,
            &samples,
            &static_text(&dom, &statics),
        );
        run.errors.extend(errs);
        run.bindings = Some(bound);
        run.texts = Some(texts);
        run.statics = Some(statics);
    }
    if let Some(n) = node {
        n.close();
    }
    run.source = Some(src);
    errors::sort(&mut run.errors);
    run
}
impl Run {
    /// One line per stage: what the plan asked and what the styles hold.
    pub fn summary(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(p) = &self.plan {
            let with = p.nodes.iter().filter(|n| !n.conds.is_empty()).count();
            out.push(format!(
                "plan: {} nodes ({} with conditions), {} cells, {} reads, {} token candidates, {} synonyms",
                p.nodes.len(),
                with,
                p.cells.len(),
                p.reads(),
                p.tokens.len(),
                p.aliases.len()
            ));
        }
        if let Some(s) = &self.styles {
            let varied = s
                .node_set
                .keys()
                .filter(|&&id| s.variant_count(id) > 1)
                .count();
            let conds: usize = s.sets.iter().map(|x| x.conds.len()).sum();
            out.push(format!(
                "styles: {} elements, {} sets ({} conditions kept), {} records, {} elements with variants, {} root tokens, {} synonyms",
                s.node_set.len(),
                s.sets.len(),
                conds,
                s.records.len(),
                varied,
                s.tokens.roots.len(),
                s.tokens.aliases.len()
            ));
        }
        if let Some(b) = &self.bindings {
            out.push(format!(
                "bindings: {} elements, {} model paths, {} intents, {} handlers, {} text keys used",
                b.nodes.len(),
                b.paths.len(),
                b.intents.len(),
                b.handlers.len(),
                b.keys.len()
            ));
        }
        if let Some(t) = &self.texts {
            let cps: u32 = t.glyphs.codepoints().iter().map(|(a, b)| b - a + 1).sum();
            out.push(format!(
                "texts: {} language(s), {} code points to keep ({} outside the named ranges)",
                t.langs.len(),
                cps,
                t.glyphs.extra.len()
            ));
        }
        out
    }
}
