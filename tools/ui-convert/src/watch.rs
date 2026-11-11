//! `watch`: the package server for hot reload.
//!
//! One Node process stays open with Chromium, the page and the local server (`127.0.0.1:<port>`, 8095 by
//! default). The server is the preview of the skin and also answers
//!
//! ```text
//! GET /pkg/<id>.ui2           the last package that built
//! GET /pkg/<id>.rev           its number: 0 before the first one, one more with every package that builds
//! GET /pkg/<id>.errors.json   the errors of the last cycle ([] when it built)
//! ```
//!
//! with `Access-Control-Allow-Origin: *` and `Cross-Origin-Resource-Policy: cross-origin` (the game page is
//! cross-origin isolated); the client polls `.rev` and loads the package when the number grows. This path
//! is separate from installed mods: nothing here touches the mod set.
//!
//! The skin folder is polled every 100 ms (a change is taken once the files stayed the same for one more
//! poll). A change of the scripts only (`scripts/`), or of the texts or samples only (`l10n/`, `sample/`), is
//! built without Chromium from the scan of the last full cycle (new characters in the texts still make new
//! font subsets); anything else (HTML, CSS, `skin.toml`, pictures) is a full cycle: the page is loaded again
//! in the open session, scanned and extracted, its pictures rasterized (only those whose request or file
//! changed), the font subsets made again only when the glyph set changed (and taken from the font cache),
//! and the package written. A cycle with errors keeps the last package and its `.rev`, writes the errors
//! into `errors.json` and prints them. Every cycle prints `ui2.watch cycle_ms=<n>`. Layout is not verified
//! in watch mode.
//!
//! Package files are replaced through a temporary file and a rename, the `.rev` last, so a client that sees
//! a new number reads the package that goes with it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde_json::json;

use crate::assets::RasterCache;
use crate::build::{font_checks, package_of, scripts, target_dir};
use crate::errors::{self, ConvError, Place, codes};
use crate::fonts::{self, Subset};
use crate::ir::Dom;
use crate::node::NodeClient;
use crate::write::{self, Pictures};
use crate::{Input, Opened, Opts, Run, Source, check_skin_with};

/// The default port of the package server.
pub const DEFAULT_PORT: u16 = 8095;

/// How often the skin folder is polled.
pub const POLL: Duration = Duration::from_millis(100);

/// Options of a watch.
#[derive(Debug, Clone)]
pub struct WatchOpts {
    /// The port of the local server; 0 picks a free one.
    pub port: u16,
    /// The output folder (`target/ui2/<id>` when `None`): `fonts/` and `pkg/` go there.
    pub out: Option<PathBuf>,
}

impl Default for WatchOpts {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            out: None,
        }
    }
}

/// What a change of the skin folder needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// Only scripts changed: no Chromium.
    Scripts,
    /// Only texts or samples changed: no Chromium.
    Texts,
    /// Anything else: the page is loaded, scanned and extracted again.
    Full,
}

/// The classification of a set of changed paths (relative to the skin folder, `/` separators).
pub fn classify<'a>(changed: impl IntoIterator<Item = &'a str>) -> Change {
    let mut scripts = false;
    let mut texts = false;
    for p in changed {
        if p.starts_with("scripts/") && p.ends_with(".js") {
            scripts = true;
        } else if p.starts_with("l10n/") || p.starts_with("sample/") {
            texts = true;
        } else {
            return Change::Full;
        }
    }
    match (scripts, texts) {
        (true, false) => Change::Scripts,
        (false, true) => Change::Texts,
        // Both at once (or nothing): the texts path rebuilds the scripts too.
        _ => Change::Texts,
    }
}

/// One cycle.
#[derive(Debug, Clone)]
pub struct Cycle {
    /// What the cycle did.
    pub change: Change,
    /// The `.rev` after the cycle (it grew when the package built).
    pub rev: u64,
    /// Whether a new package was written.
    pub built: bool,
    /// How long the cycle took.
    pub ms: u128,
    /// The errors (empty when it built).
    pub errors: Vec<ConvError>,
}

/// The font subsets of the current glyph set.
struct FontState {
    ranges: Vec<(u32, u32)>,
    subsets: Vec<Subset>,
}

/// The scan of the last full cycle (the no-Chromium cycles start from it).
struct ScanState {
    dump: String,
    chrome: String,
    pictures: Option<Pictures>,
}

/// File name -> (modified, length).
type Snapshot = BTreeMap<String, (SystemTime, u64)>;

fn snapshot(dir: &Path) -> Snapshot {
    fn walk(root: &Path, dir: &Path, out: &mut Snapshot) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(md) = e.metadata() else { continue };
            if md.is_dir() {
                walk(root, &p, out);
            } else if let Ok(rel) = p.strip_prefix(root) {
                let rel = rel.to_string_lossy().replace('\\', "/");
                let modified = md.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                out.insert(rel, (modified, md.len()));
            }
        }
    }
    let mut out = Snapshot::new();
    walk(dir, dir, &mut out);
    out
}

fn changed(a: &Snapshot, b: &Snapshot) -> Vec<String> {
    let mut out: Vec<String> = a
        .iter()
        .filter(|(k, v)| b.get(*k) != Some(v))
        .map(|(k, _)| k.clone())
        .collect();
    out.extend(b.keys().filter(|k| !a.contains_key(*k)).cloned());
    out.sort();
    out
}

/// Writes a file through a temporary file and a rename (a reader sees the old or the new bytes).
fn replace(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    // The server may hold the old file open for a moment: retry the rename briefly.
    let mut last = None;
    for _ in 0..20 {
        match std::fs::rename(&tmp, path) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("rename failed")))
}

/// A running watch: the open session and the state of the last cycles.
pub struct Watcher {
    dir: PathBuf,
    id: String,
    out: PathBuf,
    node: Option<NodeClient>,
    opened: Opened,
    rev: u64,
    files: Snapshot,
    fonts: Option<FontState>,
    scan: Option<ScanState>,
    rasters: RasterCache,
}

impl Watcher {
    /// Opens the session on the skin folder, starts the server and runs the first full cycle (its result is
    /// the second value; a skin with errors still starts, at `.rev` 0).
    pub fn start(dir: &Path, opts: &WatchOpts) -> Result<(Watcher, Cycle), Vec<ConvError>> {
        let src = Source::load(dir)?;
        let id = src.skin.skin.id.clone();
        let out = opts
            .out
            .clone()
            .unwrap_or_else(|| target_dir().join("ui2").join(&id));
        let pkg = out.join("pkg");
        let fonts_dir = out.join("fonts");
        let io = |e: std::io::Error| {
            vec![ConvError::new(
                codes::WRITE,
                Place::default(),
                format!("the watch folder: {e}"),
            )]
        };
        std::fs::create_dir_all(&pkg).map_err(io)?;
        std::fs::create_dir_all(&fonts_dir).map_err(io)?;
        // No package yet: rev 0, nothing to load.
        let _ = std::fs::remove_file(pkg.join(format!("{id}.ui2")));
        replace(&pkg.join(format!("{id}.rev")), b"0").map_err(io)?;
        replace(&pkg.join(format!("{id}.errors.json")), b"[]\n").map_err(io)?;
        let abs = |p: &Path| {
            std::path::absolute(p)
                .unwrap_or_else(|_| p.to_path_buf())
                .to_string_lossy()
                .to_string()
        };
        let mut node = NodeClient::start().map_err(|e| vec![e])?;
        let answer = node
            .call(
                "open",
                json!({
                    "skin_dir": abs(dir),
                    "entry": src.skin.skin.entry,
                    "preview": true,
                    "port": opts.port,
                    "fonts_dir": abs(&fonts_dir),
                    "pkg_dir": abs(&pkg),
                }),
            )
            .map_err(|e| vec![e])?;
        let mut w = Watcher {
            dir: dir.to_path_buf(),
            id,
            out,
            node: Some(node),
            opened: Opened::from_answer(&answer),
            rev: 0,
            files: snapshot(dir),
            fonts: None,
            scan: None,
            rasters: RasterCache::default(),
        };
        let first = w.cycle(Change::Full);
        Ok((w, first))
    }

    /// The server, `http://127.0.0.1:<port>/`.
    pub fn url(&self) -> &str {
        &self.opened.base_url
    }

    /// The skin id (the package file names).
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The number of packages built so far.
    pub fn rev(&self) -> u64 {
        self.rev
    }

    /// The folder the package files are served from.
    pub fn pkg_dir(&self) -> PathBuf {
        self.out.join("pkg")
    }

    /// Looks at the skin folder once; when it changed (and stayed the same for one more poll), runs a cycle.
    pub fn poll(&mut self) -> Option<Cycle> {
        let now = snapshot(&self.dir);
        if now == self.files {
            return None;
        }
        // Debounce: wait until the files stop changing.
        let mut last = now;
        loop {
            std::thread::sleep(POLL);
            let again = snapshot(&self.dir);
            if again == last {
                break;
            }
            last = again;
        }
        let list = changed(&self.files, &last);
        self.files = last;
        let change = classify(list.iter().map(String::as_str));
        Some(self.cycle(change))
    }

    /// Runs one cycle of the given kind (a no-Chromium kind without the scan of a full cycle runs in full).
    pub fn cycle(&mut self, change: Change) -> Cycle {
        let t = Instant::now();
        let ready =
            self.scan.as_ref().is_some_and(|s| s.pictures.is_some()) && self.fonts.is_some();
        let change = if ready { change } else { Change::Full };
        let result = match change {
            Change::Full => self.full(),
            Change::Scripts | Change::Texts => self.without_chrome(),
        };
        let mut errors = match result {
            Ok(written) => match self.publish(&written) {
                Ok(()) => Vec::new(),
                Err(e) => vec![ConvError::new(codes::WRITE, Place::default(), e)],
            },
            Err(e) => e,
        };
        errors::sort(&mut errors);
        let built = errors.is_empty();
        if !built {
            let text = errors::to_json(&errors);
            if let Err(e) = replace(
                &self.pkg_dir().join(format!("{}.errors.json", self.id)),
                text.as_bytes(),
            ) {
                errors.push(ConvError::new(
                    codes::WRITE,
                    Place::default(),
                    e.to_string(),
                ));
            }
        }
        Cycle {
            change,
            rev: self.rev,
            built,
            ms: t.elapsed().as_millis(),
            errors,
        }
    }

    fn opts(&self) -> Opts {
        Opts {
            fonts_dir: Some(self.out.join("fonts")),
        }
    }

    /// The font subsets for the glyph set of a run: kept while the set is the same, else made again (from
    /// the font cache when it has them). Returns whether they changed.
    fn subsets_for(&mut self, run: &Run) -> Result<bool, Vec<ConvError>> {
        let (Some(src), Some(scanned), Some(texts)) = (&run.source, &run.scanned, &run.texts)
        else {
            return Err(run.errors.clone());
        };
        let ranges = texts.glyphs.codepoints();
        if self.fonts.as_ref().is_some_and(|f| f.ranges == ranges) {
            return Ok(false);
        }
        let (decls, errs) = fonts::faces(&scanned.ir, &src.skin);
        if !errs.is_empty() {
            return Err(errs);
        }
        let subsets = fonts::subset_all(
            &decls,
            &ranges,
            &self.out.join("fonts"),
            &target_dir().join("ui2").join("font-cache"),
        )?;
        self.fonts = Some(FontState { ranges, subsets });
        Ok(true)
    }

    fn full(&mut self) -> Result<write::Written, Vec<ConvError>> {
        self.scan = None;
        let opts = self.opts();
        let Some(mut node) = self.node.take() else {
            return Err(vec![ConvError::new(
                codes::NODE,
                Place::default(),
                "the watch session is closed",
            )]);
        };
        let r = self.full_in(&mut node, &opts);
        self.node = Some(node);
        r
    }

    fn full_in(
        &mut self,
        node: &mut NodeClient,
        opts: &Opts,
    ) -> Result<write::Written, Vec<ConvError>> {
        let opened = self.opened.clone();
        let mut run = check_skin_with(&self.dir, Input::Session(node, &opened), opts);
        if self.subsets_for(&run)? {
            // New subsets: Chromium lays out and shapes with them (the second pass of a build).
            run = check_skin_with(&self.dir, Input::Session(node, &opened), opts);
        }
        let ranges = self
            .fonts
            .as_ref()
            .map(|f| f.ranges.clone())
            .unwrap_or_default();
        font_checks(&mut run, &ranges);
        errors::sort(&mut run.errors);
        if let Some(dump) = run.dump() {
            self.scan = Some(ScanState {
                dump,
                chrome: run.chrome.clone(),
                pictures: None,
            });
        }
        let (scr, e) = scripts(&self.dir, &run);
        run.errors.extend(e);
        if !run.errors.is_empty() {
            return Err(run.errors);
        }
        let (Some(scanned), Some(styles)) = (&run.scanned, &run.styles) else {
            return Err(run.errors);
        };
        let dom = Dom::new(&scanned.ir.dom);
        let pictures =
            crate::assets::pictures_cached(node, &self.dir, &dom, styles, &mut self.rasters)?;
        if let Some(s) = &mut self.scan {
            s.pictures = Some(pictures.clone());
        }
        self.package(&run, &pictures, &scr)
    }

    /// Scripts, texts or samples changed: the stored scan with the files as they are now.
    fn without_chrome(&mut self) -> Result<write::Written, Vec<ConvError>> {
        let Some(scan) = &self.scan else {
            return self.full();
        };
        let dump = scan.dump.clone();
        let chrome = scan.chrome.clone();
        let pictures = scan.pictures.clone().unwrap_or_default();
        let mut run = check_skin_with(&self.dir, Input::Dump(&dump), &self.opts());
        run.chrome = chrome;
        self.subsets_for(&run)?;
        let (scr, e) = scripts(&self.dir, &run);
        run.errors.extend(e);
        if !run.errors.is_empty() {
            return Err(run.errors);
        }
        self.package(&run, &pictures, &scr)
    }

    fn package(
        &self,
        run: &Run,
        pictures: &Pictures,
        scr: &[(String, Vec<u8>)],
    ) -> Result<write::Written, Vec<ConvError>> {
        let subsets = self
            .fonts
            .as_ref()
            .map(|f| f.subsets.as_slice())
            .unwrap_or_default();
        let w = package_of(run, subsets, pictures, scr)?;
        let budget = write::check_budget(&w.sections);
        if budget.is_empty() {
            Ok(w)
        } else {
            Err(budget)
        }
    }

    /// Serves a new package: the package, no errors, then the next `.rev`.
    fn publish(&mut self, w: &write::Written) -> Result<(), String> {
        let pkg = self.pkg_dir();
        let err = |e: std::io::Error| format!("cannot write the package files: {e}");
        replace(&pkg.join(format!("{}.ui2", self.id)), &w.bytes).map_err(err)?;
        replace(&pkg.join(format!("{}.errors.json", self.id)), b"[]\n").map_err(err)?;
        let rev = self.rev + 1;
        replace(
            &pkg.join(format!("{}.rev", self.id)),
            rev.to_string().as_bytes(),
        )
        .map_err(err)?;
        self.rev = rev;
        Ok(())
    }

    /// Closes the session: Chromium, the page and the server stop.
    pub fn close(mut self) {
        if let Some(n) = self.node.take() {
            n.close();
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        if let Some(n) = self.node.take() {
            n.close();
        }
    }
}

/// Prints one cycle: the timing line, then the errors.
pub fn report(c: &Cycle) {
    println!(
        "ui2.watch cycle_ms={} change={:?} rev={} {}",
        c.ms,
        c.change,
        c.rev,
        if c.built {
            "built"
        } else {
            "kept the last package"
        }
    );
    for e in &c.errors {
        eprintln!("{e}\n");
    }
    if !c.errors.is_empty() {
        eprintln!("ui2-convert: {} error(s)", c.errors.len());
    }
}

/// `ui2-convert watch <skin> [--port <n>]`: runs until the process is stopped.
pub fn run(dir: &Path, opts: &WatchOpts) -> Result<(), Vec<ConvError>> {
    let (mut w, first) = Watcher::start(dir, opts)?;
    println!(
        "ui2 watch: {} -> {}pkg/{}.ui2 (.rev, .errors.json); preview {}{}",
        dir.display(),
        w.url(),
        w.id(),
        w.url(),
        Source::load(dir)
            .map(|s| s.skin.skin.entry)
            .unwrap_or_default()
    );
    report(&first);
    loop {
        if let Some(c) = w.poll() {
            report(&c);
        }
        std::thread::sleep(POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_changes_are_classified() {
        assert_eq!(classify(["scripts/hud.js"]), Change::Scripts);
        assert_eq!(classify(["l10n/en_US.toml"]), Change::Texts);
        assert_eq!(
            classify(["sample/hud.json", "l10n/ru_RU.toml"]),
            Change::Texts
        );
        assert_eq!(classify(["scripts/a.js", "l10n/en_US.toml"]), Change::Texts);
        assert_eq!(classify(["css/hud.css"]), Change::Full);
        assert_eq!(classify(["scripts/a.js", "parts/top.html"]), Change::Full);
        assert_eq!(classify(["skin.toml"]), Change::Full);
        assert_eq!(classify(["images/a.png"]), Change::Full);
    }

    #[test]
    fn watch_snapshots_list_changed_added_and_removed_files() {
        let t = SystemTime::UNIX_EPOCH;
        let a: Snapshot = [("a".to_string(), (t, 1)), ("b".to_string(), (t, 2))].into();
        let b: Snapshot = [("a".to_string(), (t, 3)), ("c".to_string(), (t, 1))].into();
        assert_eq!(changed(&a, &b), ["a", "b", "c"]);
        assert!(changed(&a, &a).is_empty());
    }
}
