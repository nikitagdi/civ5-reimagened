//! `build --set ui`: the steps of the `ui` set build.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use asset_format::{
    AudioTable, ContentHash, LinkTable, LocaleTable, LogicalId, PrefetchGroup, TextureFormat,
    UI_INDEX_ID, UI_TABLES_ID, UiAtlas, UiFileInfo, UiFileKind, UiFont, UiGroup, UiIndex,
    UiLayerInfo, UiTexDim, Variant, VariantFormat, ui_file_id, ui_sound_file_id, ui_text_id,
    ui_texture_id,
};
use rayon::prelude::*;
use serde::Serialize;

use super::atlas::{self, AtlasRow, AtlasSet};
use super::oracle;
use super::overrides::{self, OverrideReport};
use super::refs::{self, LuaRefs, XmlRefs};
use super::tables::{self, TableRows, TableSet};
use super::text::{self, TextRow, TextSet};
use super::vfs::{LayerKind, UiEntry, UiLayer, UiVfs, UiVfsConfig, VfsError, ext_of};
use super::{audio, audio3d, cyrillic, tex};
use crate::report::{ReportEntry, SetReport};
use crate::store::{BlobStore, ManifestBuilder, StoreError};
use crate::textures::variant_format;

pub const SET_NAME: &str = "ui";
const UI_EXTS: &[&str] = &["xml", "lua", "ggxml", "ani", "cur"];
/// Skin directories that hold art or gameplay scripts, not UI files.
const GENERIC_SKIN_DIRS: &[&str] = &["art", "lua"];
/// Other roots of UI files (any layer): library Lua the UI includes, font patches.
const EXTRA_PREFIXES: &[&str] = &["gameplay/lua/", "fonts/"];
/// A Lua library included by at least this many windows goes to `ui:boot`.
const SHARED_LIB_MIN_WINDOWS: usize = 8;
/// Names of the style and colour sheets that exist before any skin names them.
const BASE_SHEETS: &[&str] = &["styles.xml", "coloratlas.xml"];
const HIGHLIGHTS: &str = "highlights.xml";
const ENTRY_CONTEXT: &str = "ingame";
const COMPARE_EXTS: &[&str] = &["xml", "lua", "ggxml", "dds", "ani", "txt"];

#[derive(Debug, thiserror::Error)]
pub enum UiError {
    #[error(transparent)]
    Vfs(#[from] VfsError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("i/o error at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{0}")]
    Other(String),
}

#[derive(Debug, Clone)]
pub struct UiOptions {
    /// `basegame`, `expansion1` or `expansion2`.
    pub set: String,
    pub include_override: bool,
    pub priority_overrides: BTreeMap<String, i32>,
    pub default_priority: i32,
    /// `None`: every language found.
    pub langs: Option<Vec<String>>,
    pub voice: bool,
    /// Also the music (`--ui-music`): the playlists, the leader themes and the main menu loop, streamed by the client.
    pub music: bool,
    /// The real game's `Localization-Merged.db` (the DB snapshot): the texts are taken from it, not merged from XML.
    pub loc_db: Option<std::path::PathBuf>,
}

impl Default for UiOptions {
    fn default() -> Self {
        Self {
            set: "expansion2".to_owned(),
            include_override: false,
            priority_overrides: BTreeMap::new(),
            default_priority: 0,
            langs: Some(vec!["en_US".to_owned(), "ru_RU".to_owned()]),
            voice: false,
            music: false,
            loc_db: None,
        }
    }
}

impl UiOptions {
    fn vfs_config(&self, game: &Path) -> Result<UiVfsConfig, UiError> {
        let mut c = UiVfsConfig::new(game, &self.set)
            .ok_or_else(|| UiError::Other(format!("unknown UI set {:?}", self.set)))?;
        c.include_override = self.include_override;
        c.default_priority = self.default_priority;
        c.priority_overrides = self.priority_overrides.clone();
        Ok(c)
    }
}

pub fn open_vfs(game: &Path, opts: &UiOptions) -> Result<UiVfs, UiError> {
    Ok(UiVfs::open(opts.vfs_config(game)?)?)
}

/// `ui-override`: the report of what `Override/` would change, without building the set.
pub fn override_report(game: &Path, opts: &UiOptions) -> Result<OverrideSummary, UiError> {
    let vfs = open_vfs(game, opts)?;
    let (texts, _) = baseline_texts(&vfs);
    let ui_names: BTreeSet<String> = vfs
        .iter_visible()
        .filter(|e| is_ui_file(&vfs, e) || e.extension() == "dds")
        .map(|e| e.name.clone())
        .collect();
    let r = overrides::analyze(&vfs, &texts, &ui_names)?;
    Ok(OverrideSummary::from(&r))
}

/// Texts of all files the VFS indexes (the override folder only with the flag).
fn baseline_texts(vfs: &UiVfs) -> (TextSet, usize) {
    let mut cands: Vec<&UiEntry> = vfs
        .iter_names()
        .flat_map(|(_, c)| c.iter())
        .filter(|e| !e.is_packed() && matches!(e.extension(), "xml" | "sql"))
        .collect();
    cands.sort_by_cached_key(|e| (e.layer, e.rel.to_lowercase()));
    let mut texts = TextSet::default();
    for e in cands {
        let Ok(b) = read_loose(e) else { continue };
        let src = String::from_utf8_lossy(&b);
        if !src.contains("Language_") {
            continue;
        }
        let rows = if e.extension() == "sql" {
            text::parse_sql(&src)
        } else {
            text::parse_xml(&src).map(|x| x.rows).unwrap_or_default()
        };
        texts.add(&e.rel, rows);
    }
    let n = texts.rows_total;
    (texts, n)
}

fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> UiError + '_ {
    move |source| UiError::Io {
        path: path.display().to_string(),
        source,
    }
}

/// Whether a file at `rel` inside `layer` is a UI file.
pub fn in_ui_roots(layer: &UiLayer, rel: &str) -> bool {
    let lower = rel.to_ascii_lowercase();
    let comps: Vec<&str> = lower.split('/').collect();
    if comps[..comps.len() - 1].contains(&"ui") {
        return true;
    }
    if EXTRA_PREFIXES.iter().any(|p| lower.starts_with(p)) {
        return true;
    }
    layer
        .skin_dirs
        .iter()
        .any(|d| !GENERIC_SKIN_DIRS.contains(&d.as_str()) && lower.starts_with(&format!("{d}/")))
}

fn is_ui_file(vfs: &UiVfs, e: &UiEntry) -> bool {
    !e.is_packed() && UI_EXTS.contains(&e.extension()) && in_ui_roots(vfs.layer(e), &e.rel)
}

fn kind_of(name: &str) -> UiFileKind {
    match ext_of(name) {
        "xml" => UiFileKind::Xml,
        "lua" => UiFileKind::Lua,
        "ggxml" => UiFileKind::Font,
        "dds" => UiFileKind::Texture,
        _ => UiFileKind::Other,
    }
}

#[derive(Debug, Default, Serialize)]
pub struct LayerRow {
    pub name: String,
    pub kind: String,
    pub priority: i32,
    pub priority_defaulted: bool,
    pub packages: Vec<String>,
    pub packs: Vec<String>,
    pub files: usize,
    pub bytes: u64,
    pub visible: usize,
}

#[derive(Debug, Default, Serialize)]
pub struct CollisionSummary {
    pub names: usize,
    pub colliding: usize,
    pub differing: usize,
    /// ext -> (colliding, differing)
    pub by_ext: BTreeMap<String, (usize, usize)>,
    /// "winner layer > loser layer" -> (count, differing)
    pub by_pair: BTreeMap<String, (usize, usize)>,
    /// UI-relevant names whose content differs: name, winner, losers.
    pub differing_samples: Vec<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct OverrideSummary {
    pub total: usize,
    pub zero_bytes: usize,
    pub with_content: usize,
    pub blanks: usize,
    pub replaces: usize,
    pub same: usize,
    pub adds: usize,
    pub empty_new: usize,
    pub ui_overlap: Vec<String>,
    pub text_changed: usize,
    pub text_new: usize,
    pub text_files: usize,
    pub blanked_by_layer: BTreeMap<String, usize>,
    pub replaced_by_layer: BTreeMap<String, usize>,
    pub blanked_ext: BTreeMap<String, usize>,
}

impl From<&OverrideReport> for OverrideSummary {
    fn from(r: &OverrideReport) -> Self {
        use overrides::Effect;
        let mut s = OverrideSummary {
            total: r.total,
            zero_bytes: r.zero_bytes,
            with_content: r.with_content,
            blanks: r.blanks,
            replaces: r.replaces,
            same: r.same,
            adds: r.adds,
            empty_new: r.empty_new,
            ui_overlap: r.ui_overlap.clone(),
            text_changed: r.text_changed,
            text_new: r.text_new,
            text_files: r.text_files,
            ..Self::default()
        };
        for i in &r.items {
            match &i.effect {
                Effect::Blanks { layer, .. } => {
                    *s.blanked_by_layer.entry(layer.clone()).or_default() += 1;
                    *s.blanked_ext.entry(ext_of(&i.name).to_owned()).or_default() += 1;
                }
                Effect::Replaces { layer, .. } => {
                    *s.replaced_by_layer.entry(layer.clone()).or_default() += 1;
                }
                _ => {}
            }
        }
        s
    }
}

#[derive(Debug, Default, Serialize)]
pub struct GroupRow {
    pub id: String,
    pub members: usize,
    pub bytes: u64,
}

#[derive(Debug, Default, Serialize)]
pub struct UiReportDoc {
    pub layers: Vec<LayerRow>,
    pub collisions: CollisionSummary,
    pub override_: OverrideSummary,
    pub counts: BTreeMap<String, u64>,
    pub bytes: BTreeMap<String, u64>,
    pub groups: Vec<GroupRow>,
    pub gaps: Vec<(String, String, String)>,
    pub texts: BTreeMap<String, (usize, usize)>,
    pub notes: Vec<String>,
}

pub struct UiSummary {
    pub report: SetReport,
    pub doc: UiReportDoc,
    pub seconds: f64,
}

impl UiReportDoc {
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        let mb = |b: u64| format!("{:.2} MB", b as f64 / 1e6);
        let _ = writeln!(s, "== layers (override order, lowest first) ==");
        for l in &self.layers {
            let _ = writeln!(
                s,
                "{:<18} {:<8} priority {:>4}{} files {:>6} ({}) visible {:>6} packages [{}] packs [{}]",
                l.name,
                l.kind,
                l.priority,
                if l.priority_defaulted { "*" } else { " " },
                l.files,
                mb(l.bytes),
                l.visible,
                l.packages.join(", "),
                l.packs.len()
            );
        }
        let _ = writeln!(s, "(* = no <Priority> in the package, default applied)\n");
        let c = &self.collisions;
        let _ = writeln!(
            s,
            "== collisions ==\nnames {} colliding {} differing {}",
            c.names, c.colliding, c.differing
        );
        for (e, (n, d)) in &c.by_ext {
            let _ = writeln!(s, "  .{e:<8} colliding {n:>6} differing {d:>6}");
        }
        let _ = writeln!(s, "winner > loser:");
        for (k, (n, d)) in &c.by_pair {
            let _ = writeln!(s, "  {k:<44} {n:>6} (differing {d})");
        }
        for n in &c.differing_samples {
            let _ = writeln!(s, "  differs: {n}");
        }
        let o = &self.override_;
        let _ = writeln!(
            s,
            "\n== Override/ ==\nfiles {} (zero bytes {}, with content {}): blank {} replace {} same {} add {} empty-new {}\nUI names also in Override/: {}\ntext files {} rows changing a baseline text {} new tags {}",
            o.total,
            o.zero_bytes,
            o.with_content,
            o.blanks,
            o.replaces,
            o.same,
            o.adds,
            o.empty_new,
            if o.ui_overlap.is_empty() {
                "none".to_owned()
            } else {
                o.ui_overlap.join(", ")
            },
            o.text_files,
            o.text_changed,
            o.text_new
        );
        let _ = writeln!(
            s,
            "blanked by layer: {:?}\nreplaced by layer: {:?}\nblanked by ext: {:?}",
            o.blanked_by_layer, o.replaced_by_layer, o.blanked_ext
        );
        let _ = writeln!(s, "\n== set ==");
        for (k, v) in &self.counts {
            let _ = writeln!(s, "{k}: {v}");
        }
        for (k, v) in &self.bytes {
            let _ = writeln!(s, "{k}: {} ({v} B)", mb(*v));
        }
        let _ = writeln!(s, "\n== texts (keys, keys of the UI) ==");
        for (l, (n, u)) in &self.texts {
            let _ = writeln!(s, "{l}: {n} keys, {u} UI keys translated");
        }
        let _ = writeln!(s, "\n== groups ==");
        for g in &self.groups {
            let _ = writeln!(s, "{:<40} members {:>5} {}", g.id, g.members, mb(g.bytes));
        }
        for n in &self.notes {
            let _ = writeln!(s, "note: {n}");
        }
        let _ = writeln!(s, "\n== gaps ({}) ==", self.gaps.len());
        for (k, a, b) in &self.gaps {
            let _ = writeln!(s, "{k}: {a}: {b}");
        }
        s
    }
}

struct Loaded {
    name: String,
    bytes: Vec<u8>,
}

#[derive(Default)]
struct FileScan {
    layer: usize,
    rel: String,
    rows: Vec<TextRow>,
    atlas: Vec<AtlasRow>,
    tables: TableRows,
    error: Option<String>,
}

/// Collisions with a comparison of the bytes for the file types the UI cares about.
pub fn collision_summary(vfs: &UiVfs) -> Result<CollisionSummary, UiError> {
    let mut out = CollisionSummary {
        names: vfs.len(),
        ..CollisionSummary::default()
    };
    let rows: Vec<(String, Option<bool>, Vec<usize>, usize)> = vfs
        .iter_names()
        .filter(|(_, c)| c.len() > 1)
        .collect::<Vec<_>>()
        .par_iter()
        .map(|(name, copies)| {
            let win = copies.last().expect("len > 1");
            let diff = if COMPARE_EXTS.contains(&ext_of(name)) {
                let mut same = true;
                for l in &copies[..copies.len() - 1] {
                    if !vfs.same_bytes(win, l).unwrap_or(false) {
                        same = false;
                        break;
                    }
                }
                Some(!same)
            } else {
                None
            };
            let losers = copies[..copies.len() - 1].iter().map(|e| e.layer).collect();
            ((*name).clone(), diff, losers, win.layer)
        })
        .collect();
    for (name, diff, losers, win) in rows {
        out.colliding += 1;
        let ext = ext_of(&name).to_owned();
        let e = out.by_ext.entry(ext).or_default();
        e.0 += 1;
        if diff == Some(true) {
            e.1 += 1;
            out.differing += 1;
        }
        for l in losers {
            let k = format!("{} > {}", vfs.layers[win].name, vfs.layers[l].name);
            let p = out.by_pair.entry(k).or_default();
            p.0 += 1;
            if diff == Some(true) {
                p.1 += 1;
            }
        }
        if diff == Some(true)
            && matches!(ext_of(&name), "xml" | "lua" | "ggxml" | "dds" | "ani")
            && out.differing_samples.len() < 80
        {
            out.differing_samples
                .push(format!("{name} (winner {})", vfs.layers[win].name));
        }
    }
    Ok(out)
}

pub fn layer_rows(vfs: &UiVfs) -> Vec<LayerRow> {
    let mut rows: Vec<LayerRow> = vfs
        .layers
        .iter()
        .map(|l| LayerRow {
            name: l.name.clone(),
            kind: match l.kind {
                LayerKind::Root => "root",
                LayerKind::Orphan => "orphan",
                LayerKind::Package => "package",
            }
            .to_owned(),
            priority: l.priority,
            priority_defaulted: l.priority_defaulted,
            packages: l.packages.clone(),
            packs: l.packs.clone(),
            ..LayerRow::default()
        })
        .collect();
    for (_, copies) in vfs.iter_names() {
        for (i, e) in copies.iter().enumerate() {
            rows[e.layer].files += 1;
            rows[e.layer].bytes += e.size();
            if i + 1 == copies.len() {
                rows[e.layer].visible += 1;
            }
        }
    }
    rows
}

fn read_loose(e: &UiEntry) -> Result<Vec<u8>, UiError> {
    match &e.origin {
        super::vfs::Origin::Loose { path, .. } => std::fs::read(path).map_err(io_err(path)),
        super::vfs::Origin::Packed { .. } => Err(UiError::Other("packed entry".into())),
    }
}

/// The text, atlas and table rows of every XML / SQL file of the VFS, merged in layer order.
fn scan_sources(
    vfs: &UiVfs,
    gaps: &mut Vec<(String, String, String)>,
) -> (TextSet, AtlasSet, TableSet) {
    // ---- text and atlas files (all copies: texts merge by row, not by file name) ----
    let candidates: Vec<&UiEntry> = vfs
        .iter_names()
        .flat_map(|(_, c)| c.iter())
        .filter(|e| !e.is_packed() && matches!(e.extension(), "xml" | "sql"))
        .collect();
    let mut scans: Vec<FileScan> = candidates
        .par_iter()
        .filter_map(|e| {
            let bytes = read_loose(e).ok()?;
            let src = String::from_utf8_lossy(&bytes);
            let has_text = src.contains("Language_");
            let has_atlas = src.contains("IconTextureAtlases");
            let has_tables = ["IconFontMapping", "IconFontTextures", "Colors", "Languages"]
                .iter()
                .any(|t| src.contains(t));
            if !has_text && !has_atlas && !has_tables {
                return None;
            }
            let mut s = FileScan {
                layer: e.layer,
                rel: e.rel.clone(),
                ..FileScan::default()
            };
            if has_tables {
                s.tables = if e.extension() == "sql" {
                    tables::parse_sql(&src)
                } else {
                    tables::parse_xml(&src)
                };
            }
            if e.extension() == "sql" {
                if has_text {
                    s.rows = text::parse_sql(&src);
                }
                if has_atlas {
                    s.atlas = atlas::parse_sql(&src);
                }
            } else {
                if has_text {
                    match text::parse_xml(&src) {
                        Ok(x) => s.rows = x.rows,
                        Err(m) => s.error = Some(m),
                    }
                }
                if has_atlas {
                    match atlas::parse_xml(&src) {
                        Ok(r) => s.atlas = r,
                        Err(m) => s.error = Some(m),
                    }
                }
            }
            Some(s)
        })
        .collect();
    // The Override folders hold the game's tables as the mod changes them. Their texts are not
    // read here (the lowest-layer fallback below reads them); their icon fonts, colors and atlases are,
    // tolerantly: some are not valid XML.
    let override_scans: Vec<FileScan> = vfs
        .override_files
        .par_iter()
        .filter(|o| matches!(ext_of(&o.name), "xml" | "sql"))
        .filter_map(|o| {
            let bytes = std::fs::read(&o.path).ok()?;
            let src = String::from_utf8_lossy(&bytes);
            let sql = ext_of(&o.name) == "sql";
            let t = if sql {
                tables::parse_sql(&src)
            } else {
                tables::parse_xml(&src)
            };
            let atlas = if sql {
                atlas::parse_sql(&src)
            } else {
                tables::parse_atlas_xml(&src)
            };
            (!t.is_empty() || !atlas.is_empty()).then(|| FileScan {
                layer: o.layer,
                rel: format!("Override/{}", o.name),
                tables: t,
                atlas,
                ..FileScan::default()
            })
        })
        .collect();
    scans.extend(override_scans);
    // The texts of the Override folders are the lowest layer: they only fill the tags no other source has (Lekmod's
    // `TXT_KEY_CITYVIEW_GOLDEN_AGE_TEXT` lives only there). A later row of any real layer replaces them.
    let override_texts: Vec<FileScan> = vfs
        .override_files
        .par_iter()
        .filter(|o| ext_of(&o.name) == "xml")
        .filter_map(|o| {
            let bytes = std::fs::read(&o.path).ok()?;
            let src = String::from_utf8_lossy(&bytes);
            if !src.contains("Language_") {
                return None;
            }
            let rows = text::parse_xml(&src).ok()?.rows;
            (!rows.is_empty()).then(|| FileScan {
                layer: 0,
                rel: format!("!Override/{}", o.name),
                rows,
                ..FileScan::default()
            })
        })
        .collect();
    scans.extend(override_texts);
    scans.sort_by_cached_key(|s| (s.layer, s.rel.to_lowercase()));
    let mut texts = TextSet::default();
    let mut atlases = AtlasSet::default();
    let mut table_set = TableSet::default();
    for s in scans {
        if let Some(m) = &s.error {
            gaps.push(("xml-parse".into(), s.rel.clone(), m.clone()));
        }
        let label = format!("{}:{}", vfs.layers[s.layer].name, s.rel);
        texts.add(&label, s.rows);
        atlases.add(s.atlas);
        table_set.add(s.tables);
    }
    (texts, atlases, table_set)
}

/// The texts the XML and SQL files of the VFS merge to (what the build uses without a snapshot).
pub fn xml_texts(game: &Path, opts: &UiOptions) -> Result<TextSet, UiError> {
    let vfs = open_vfs(game, opts)?;
    Ok(scan_sources(&vfs, &mut Vec::new()).0)
}

/// The UI's texts of the languages `opts.langs` that the snapshot has are the snapshot's (`Localization-Merged.db`, what
/// the real game merged), not a merge of the XML files in layer order. SIMPLIFIED(SIMP-uic-40): without a snapshot the XML
/// merge stays, whose order is not the game's loader order (Scientific Revolution read "Grants a free Technology." from the No Quitters Mod's file
/// instead of the Great Scientist of Lekmod's `Override` file). Tags only the XML files have (Zimbabwe, the icebreaker: files
/// the snapshot's game did not load) are dropped, so the language is the snapshot's exactly (`ui-loc-diff` shows 0).
fn apply_snapshot_texts(
    texts: &mut TextSet,
    loc_db: &Path,
    opts: &UiOptions,
    notes: &mut Vec<String>,
) -> Result<(), UiError> {
    let langs: Vec<String> = match &opts.langs {
        Some(l) => l.clone(),
        None => texts.langs.values().map(|(n, _)| n.clone()).collect(),
    };
    for lang in langs {
        let rows =
            oracle::read_language(loc_db, &lang).map_err(|e| UiError::Other(e.to_string()))?;
        if rows.is_empty() {
            notes.push(format!(
                "snapshot texts {lang}: the snapshot has none, the XML merge stays"
            ));
            continue;
        }
        let d = oracle::diff(texts, &rows, &lang);
        notes.push(format!(
            "snapshot texts {lang}: {} rows taken; the XML merge differed in {} texts, lacked {} tags, had {} tags the snapshot lacks (dropped), {} grammar rows",
            rows.len(),
            d.differing.len(),
            d.missing.len(),
            d.extra.len(),
            d.grammar.len()
        ));
        oracle::apply(texts, rows, &lang, false);
    }
    Ok(())
}

/// Full build into `out`. The caller writes the report files.
pub fn build_ui(game: &Path, out: &Path, opts: &UiOptions) -> Result<UiSummary, UiError> {
    let started = std::time::Instant::now();
    let vfs = open_vfs(game, opts)?;
    let store = BlobStore::open(out)?;
    let mut manifest = ManifestBuilder::open(&store)?;
    let mut doc = UiReportDoc::default();
    let mut entries: Vec<ReportEntry> = Vec::new();
    let mut gaps: Vec<(String, String, String)> = Vec::new();
    let mut sizes: BTreeMap<String, u64> = BTreeMap::new();

    for (pk, err) in &vfs.pack_errors {
        gaps.push(("pack-error".into(), pk.display().to_string(), err.clone()));
    }
    for e in &vfs.package_errors {
        gaps.push(("package-error".into(), e.clone(), String::new()));
    }
    for (pkg, pack) in &vfs.missing_packs {
        gaps.push(("missing-pack".into(), pkg.clone(), pack.clone()));
    }
    doc.layers = layer_rows(&vfs);
    doc.collisions = collision_summary(&vfs)?;

    let (mut texts, atlases, table_set) = scan_sources(&vfs, &mut gaps);
    let mut loc_notes = Vec::new();
    if let Some(db) = &opts.loc_db {
        apply_snapshot_texts(&mut texts, db, opts, &mut loc_notes)?;
    }
    let ui_tables = table_set.into_tables();

    // ---- UI files ----
    let mut ui_entries: Vec<&UiEntry> =
        vfs.iter_visible().filter(|e| is_ui_file(&vfs, e)).collect();
    // The icon fonts the `IconFontTextures` rows name belong to the set wherever their `.ggxml` sits: Lekmod keeps the
    // fonts of its resources (`CoconutFontIcon_22.ggxml`, ...) under `Art/`, outside the UI roots, and without them
    // `[ICON_RES_COCONUT]` draws nothing (the trade screen lists resources by their font icon).
    for e in icon_font_entries(&vfs, &ui_tables) {
        if !ui_entries.iter().any(|x| x.name == e.name) {
            ui_entries.push(e);
        }
    }
    let mut loaded: Vec<Loaded> = ui_entries
        .par_iter()
        .map(|e| {
            Ok(Loaded {
                name: e.name.clone(),
                bytes: vfs.read(e)?,
            })
        })
        .collect::<Result<_, UiError>>()?;
    // Deliberate layout improvements (SIMP-uic-20).
    for l in loaded.iter_mut() {
        let (bytes, missed) = super::patches::apply(&l.name, std::mem::take(&mut l.bytes));
        l.bytes = bytes;
        for p in missed {
            gaps.push((
                "ui-patch (text not found)".into(),
                l.name.clone(),
                p.old.to_owned(),
            ));
        }
    }
    // Cyrillic for the Tw Cen MT fonts (SIMP-exe-716): a plan changes the `.ggxml` text here and its atlases in the
    // texture step below (they are found by name).
    let mut cyrillic_atlases: BTreeMap<
        String,
        (std::sync::Arc<cyrillic::FontPlan>, cyrillic::Role),
    > = BTreeMap::new();
    for l in loaded.iter_mut() {
        if !cyrillic::is_twcen(&l.name) {
            continue;
        }
        let text = String::from_utf8_lossy(&l.bytes).into_owned();
        match cyrillic::plan(&text) {
            Ok(Some(plan)) => {
                let stem = l.name.trim_end_matches(".ggxml").to_owned();
                l.bytes = plan.ggxml(&text).into_bytes();
                let plan = std::sync::Arc::new(plan);
                for t in font_textures(&text) {
                    let role = cyrillic::role_of(&t, &stem);
                    cyrillic_atlases.insert(t, (std::sync::Arc::clone(&plan), role));
                }
                *doc.counts
                    .entry("fonts with added Cyrillic".into())
                    .or_default() += 1;
            }
            Ok(None) => {}
            Err(m) => gaps.push(("cyrillic-font".into(), l.name.clone(), m)),
        }
    }
    let mut xml_refs: BTreeMap<String, XmlRefs> = BTreeMap::new();
    let mut lua_refs: BTreeMap<String, LuaRefs> = BTreeMap::new();
    let mut fonts: Vec<UiFont> = Vec::new();
    for l in &loaded {
        let text = String::from_utf8_lossy(&l.bytes);
        match ext_of(&l.name) {
            "xml" => match refs::scan_xml(&text) {
                Ok(r) => {
                    xml_refs.insert(l.name.clone(), r);
                }
                Err(m) => {
                    // Civ5 files that the strict parser refuses are scanned as text. A file that
                    // is only comments (an EUI stub) has nothing to scan and is no gap.
                    let stub = !text.contains("<") || strip_comments(&text).trim().is_empty();
                    if !stub {
                        gaps.push(("xml-parse (read as text)".into(), l.name.clone(), m));
                    }
                    xml_refs.insert(l.name.clone(), refs::scan_xml_lenient(&text));
                }
            },
            "lua" => {
                lua_refs.insert(l.name.clone(), refs::scan_lua(&text));
            }
            "ggxml" => {
                let texs = font_textures(&text);
                fonts.push(UiFont {
                    name: l.name.trim_end_matches(".ggxml").to_owned(),
                    ggxml: l.name.clone(),
                    textures: texs,
                });
            }
            _ => {}
        }
    }
    // Libraries the UI includes belong to the set wherever they sit.
    loop {
        let have: BTreeSet<&str> = ui_entries.iter().map(|e| e.name.as_str()).collect();
        let mut add: Vec<&UiEntry> = Vec::new();
        for r in lua_refs.values() {
            for inc in &r.includes {
                if let Some(e) = vfs.find_stem(inc, &["lua"])
                    && !e.is_packed()
                    && !have.contains(e.name.as_str())
                    && !add.iter().any(|a| a.name == e.name)
                {
                    add.push(e);
                }
            }
            for pat in &r.wild_includes {
                for e in vfs.find_wild(pat) {
                    if !e.is_packed()
                        && !have.contains(e.name.as_str())
                        && !add.iter().any(|a| a.name == e.name)
                    {
                        add.push(e);
                    }
                }
            }
        }
        if add.is_empty() {
            break;
        }
        for e in add {
            let bytes = vfs.read(e)?;
            lua_refs.insert(
                e.name.clone(),
                refs::scan_lua(&String::from_utf8_lossy(&bytes)),
            );
            loaded.push(Loaded {
                name: e.name.clone(),
                bytes,
            });
            ui_entries.push(e);
        }
    }
    let by_name: BTreeMap<&str, &Loaded> = loaded.iter().map(|l| (l.name.as_str(), l)).collect();
    fonts.sort_by(|a, b| a.name.cmp(&b.name));

    // ---- styles: base sheets, then the skins' sheets in layer order ----
    let mut sheet_names: Vec<String> = BASE_SHEETS.iter().map(|s| (*s).to_owned()).collect();
    let mut color_names: Vec<String> = Vec::new();
    for l in &vfs.layers {
        for f in &l.style_files {
            sheet_names.push(refs::bare(f));
        }
        for f in &l.color_files {
            color_names.push(refs::bare(f));
        }
    }
    let mut seen = BTreeSet::new();
    sheet_names.retain(|n| seen.insert(n.clone()));
    let mut styles: BTreeMap<String, XmlRefs> = BTreeMap::new();
    let mut skin_styles: Vec<String> = Vec::new();
    for n in &sheet_names {
        let Some(e) = vfs.lookup(n) else { continue };
        let text = String::from_utf8_lossy(&vfs.read(e)?).into_owned();
        match refs::scan_styles(&text) {
            Ok(defs) => {
                skin_styles.push(n.clone());
                for (name, r) in defs {
                    styles.insert(name, r);
                }
            }
            Err(m) => gaps.push(("xml-parse".into(), n.clone(), m)),
        }
    }
    let mut skin_colors: Vec<String> = BASE_SHEETS
        .iter()
        .filter(|s| s.contains("color"))
        .map(|s| (*s).to_owned())
        .collect();
    skin_colors.extend(color_names);
    let mut seen = BTreeSet::new();
    skin_colors.retain(|n| vfs.lookup(n).is_some() && seen.insert(n.clone()));

    // ---- wanted textures ----
    let mut wanted: BTreeSet<String> = BTreeSet::new();
    let mut named_by: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut want = |tex: &str, by: &str, wanted: &mut BTreeSet<String>| {
        wanted.insert(tex.to_owned());
        named_by
            .entry(tex.to_owned())
            .or_default()
            .insert(by.to_owned());
    };
    for (n, r) in &xml_refs {
        for d in &r.dds {
            want(d, n, &mut wanted);
        }
    }
    for (n, r) in &lua_refs {
        for d in &r.dds {
            want(d, n, &mut wanted);
        }
    }
    for (n, r) in &styles {
        for d in &r.dds {
            want(d, &format!("style {n}"), &mut wanted);
        }
    }
    for f in &fonts {
        for t in &f.textures {
            want(t, &f.ggxml, &mut wanted);
        }
    }
    let mut atlas_rows: Vec<UiAtlas> = Vec::new();
    for ((a, size), r) in &atlases.rows {
        let file = refs::bare(&r.filename);
        want(&file, &format!("atlas {a}"), &mut wanted);
        atlas_rows.push(UiAtlas {
            atlas: a.clone(),
            icon_size: *size,
            file,
            icons_per_row: r.icons_per_row,
            icons_per_column: r.icons_per_column,
        });
    }
    // Textures of the UI packs and loose in the UI roots are part of the set whether named or not.
    for (name, copies) in vfs.iter_names() {
        if ext_of(name) != "dds" {
            continue;
        }
        let e = copies.last().expect("not empty");
        let ui_pack = e.is_packed() && e.rel.to_ascii_lowercase().ends_with("uitextures.fpk");
        let loose_ui = !e.is_packed() && in_ui_roots(vfs.layer(e), &e.rel);
        if ui_pack || loose_ui {
            wanted.insert(name.clone());
        }
    }
    // The index of a palette and the palette of an index.
    let extra: Vec<String> = wanted
        .iter()
        .flat_map(|n| {
            let mut v = Vec::new();
            if let Some(base) = tex::index_of(n) {
                v.push(base);
            } else if let Some(stem) = n.strip_suffix(".dds") {
                v.push(format!("{stem}-index.dds"));
            }
            v
        })
        .filter(|n| vfs.lookup(n).is_some())
        .collect();
    wanted.extend(extra);
    let missing: Vec<&String> = wanted.iter().filter(|n| vfs.lookup(n).is_none()).collect();
    let mut missing_set = BTreeSet::new();
    for m in &missing {
        // A partial name from a run-time concatenation is no texture.
        let by = named_by
            .get(*m)
            .map(|s| s.iter().cloned().collect::<Vec<_>>().join(", "))
            .unwrap_or_default();
        gaps.push(("missing-texture".into(), (*m).clone(), by.clone()));
        entries.push(ReportEntry::new("missing-texture", (*m).clone(), by));
        missing_set.insert((*m).clone());
    }
    wanted.retain(|n| !missing_set.contains(n));

    // ---- store textures ----
    // Block dictionaries (font and icon atlases, palettes) are stored as the game's shaders see them: expanded.
    /// Format, hash and stored size, texture format, pixel size.
    type Stored = (VariantFormat, ContentHash, u64, TextureFormat, (u32, u32));
    struct Done {
        name: String,
        /// `None`: an index texture whose dictionary is stored expanded, so it is not stored itself.
        res: Result<Option<Stored>, String>,
    }
    let done: Vec<Done> = wanted
        .par_iter()
        .map(|name| {
            let e = vfs.lookup(name).expect("checked");
            let res = (|| {
                let bytes = vfs.read(e).map_err(|e| e.to_string())?;
                if let Some(base) = tex::index_of(name)
                    && let Some(be) = vfs.lookup(&base)
                    && tex::block_size(&vfs.read(be).map_err(|e| e.to_string())?).is_some()
                {
                    return Ok(None);
                }
                // A dictionary without an index (a blank stub) is kept as it is stored.
                let index = if tex::block_size(&bytes).is_some() && tex::index_of(name).is_none() {
                    let stem = name.strip_suffix(".dds").unwrap_or(name);
                    match vfs.lookup(&format!("{stem}-index.dds")) {
                        Some(pe) => Some(vfs.read(pe).map_err(|e| e.to_string())?),
                        None => None,
                    }
                } else {
                    None
                };
                let blob = tex::ui_texture(&bytes, index.as_deref()).map_err(|e| e.to_string())?;
                let blob = match cyrillic_atlases.get(name.as_str()) {
                    Some((plan, role)) => plan.apply(*role, blob)?,
                    None => blob,
                };
                let (h, sz) = store.put(&blob.encode()).map_err(|e| e.to_string())?;
                Ok(Some((
                    variant_format(blob.format),
                    h,
                    sz,
                    blob.format,
                    (blob.width, blob.height),
                )))
            })();
            Done {
                name: name.clone(),
                res,
            }
        })
        .collect();
    let mut tex_bytes: BTreeMap<String, u64> = BTreeMap::new();
    let mut tex_names: BTreeSet<String> = BTreeSet::new();
    let mut tex_dims: BTreeMap<String, (u32, u32)> = BTreeMap::new();
    for d in done {
        match d.res {
            Ok(None) => {}
            Ok(Some((vf, hash, size, fmt, dims))) => {
                tex_dims.insert(d.name.clone(), dims);
                let id = ui_texture_id(&d.name);
                manifest.add(
                    LogicalId::new(id.clone()),
                    Variant {
                        format: vf,
                        hash,
                        size,
                        prefetch: PrefetchGroup(0),
                    },
                );
                sizes.insert(id, size);
                *tex_bytes.entry(format!("{fmt:?}")).or_default() += size;
                *doc.counts.entry(format!("textures {fmt:?}")).or_default() += 1;
                tex_names.insert(d.name);
            }
            Err(m) => {
                gaps.push(("unsupported-texture".into(), d.name.clone(), m.clone()));
                entries.push(ReportEntry::new("unsupported-texture", d.name, m));
            }
        }
    }
    // The client answers `SetTexture` of a texture that has not arrived yet with the size the index gives
    // (`UiIndex::tex_dims`, every texture). The rows of the atlas tables say the size of an atlas as `icon size *
    // icons per row` by `icon size * icons per column`; a mod's rows are not always right, so the size comes from the
    // texture and the rows that disagree are one line of the report.
    let mut rows_differ: Vec<String> = Vec::new();
    for a in &atlas_rows {
        if let Some(&(w, h)) = tex_dims.get(&a.file) {
            let (ew, eh) = (
                a.icon_size * a.icons_per_row,
                a.icon_size * a.icons_per_column,
            );
            if (w, h) != (ew, eh) {
                rows_differ.push(format!(
                    "{} ({} at {}: rows say {ew}x{eh}, texture {w}x{h})",
                    a.file, a.atlas, a.icon_size
                ));
            }
        }
    }
    if !rows_differ.is_empty() {
        gaps.push((
            "atlas-rows-differ".into(),
            format!("{} atlas textures", rows_differ.len()),
            rows_differ
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join("; "),
        ));
    }

    // ---- store files ----
    let mut files: Vec<UiFileInfo> = Vec::new();
    let layer_index = |e: &UiEntry| e.layer as u32;
    let mut file_bytes: BTreeMap<String, u64> = BTreeMap::new();
    for e in &ui_entries {
        let l = by_name[e.name.as_str()];
        let (hash, size) = store.put(&l.bytes)?;
        let id = ui_file_id(&e.name);
        manifest.add(
            LogicalId::new(id.clone()),
            Variant {
                format: VariantFormat::Generic,
                hash,
                size,
                prefetch: PrefetchGroup(0),
            },
        );
        sizes.insert(id.clone(), size);
        *file_bytes.entry(ext_of(&e.name).to_owned()).or_default() += size;
        *doc.counts
            .entry(format!("files .{}", ext_of(&e.name)))
            .or_default() += 1;
        files.push(UiFileInfo {
            name: e.name.clone(),
            id,
            kind: kind_of(&e.name),
            layer: layer_index(e),
            size,
            path: e.rel.clone(),
            hidden: (vfs.copies(&e.name).len() - 1) as u32,
            index_of: None,
        });
    }
    for n in &tex_names {
        let e = vfs.lookup(n).expect("checked");
        files.push(UiFileInfo {
            name: n.clone(),
            id: ui_texture_id(n),
            kind: UiFileKind::Texture,
            layer: layer_index(e),
            size: sizes[&ui_texture_id(n)],
            path: if e.is_packed() {
                format!("{}/{}", e.rel, n)
            } else {
                e.rel.clone()
            },
            hidden: (vfs.copies(n).len() - 1) as u32,
            index_of: tex::index_of(n),
        });
    }
    files.sort_by(|a, b| a.name.cmp(&b.name));
    files.dedup_by(|a, b| a.name == b.name);

    // ---- game tables (icon fonts, colors, languages) ----
    {
        let bytes = ui_tables.encode();
        let (hash, size) = store.put(&bytes)?;
        manifest.add(
            LogicalId::new(UI_TABLES_ID),
            Variant {
                format: VariantFormat::Generic,
                hash,
                size,
                prefetch: PrefetchGroup(0),
            },
        );
        sizes.insert(UI_TABLES_ID.to_owned(), size);
        doc.counts
            .insert("icon font mappings".into(), ui_tables.icons.len() as u64);
        doc.counts
            .insert("colors".into(), ui_tables.colors.len() as u64);
        *doc.bytes.entry("tables".into()).or_default() += size;
        // An icon whose font file is not in the set cannot be drawn.
        for i in &ui_tables.icons {
            let font = ui_tables
                .icon_textures
                .iter()
                .find(|(k, _)| *k == i.texture)
                .map(|(_, f)| f.as_str());
            let ok = font.is_some_and(|f| fonts.iter().any(|x| x.name.eq_ignore_ascii_case(f)));
            if !ok {
                entries.push(ReportEntry::new(
                    "icon-font-missing",
                    i.name.clone(),
                    format!(
                        "{} -> {}",
                        i.texture,
                        font.unwrap_or("no IconFontTextures row")
                    ),
                ));
            }
        }
    }

    // ---- texts ----
    let mut locale_ids = Vec::new();
    let ui_keys = ui_text_keys(&loaded);
    let langs: Vec<String> = match &opts.langs {
        Some(l) => l.clone(),
        None => texts.langs.values().map(|(n, _)| n.clone()).collect(),
    };
    for lang in &langs {
        let Some(map) = texts.table(lang) else {
            gaps.push(("missing-language".into(), lang.clone(), "no rows".into()));
            continue;
        };
        let mut t = LocaleTable::new(lang.clone());
        t.entries = map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        if let Some(g) = texts.grammar_table(lang) {
            t.grammar = g
                .iter()
                .map(|(tag, (gender, plurality))| asset_format::LocaleGrammar {
                    tag: tag.clone(),
                    gender: gender.clone(),
                    plurality: plurality.clone(),
                })
                .collect();
        }
        let (hash, size) = store.put(&t.encode())?;
        let id = ui_text_id(lang);
        manifest.add(
            LogicalId::new(id.clone()),
            Variant {
                format: VariantFormat::Generic,
                hash,
                size,
                prefetch: PrefetchGroup(0),
            },
        );
        sizes.insert(id.clone(), size);
        locale_ids.push(id);
        let covered = ui_keys.iter().filter(|k| map.contains_key(*k)).count();
        doc.texts.insert(lang.clone(), (map.len(), covered));
        *doc.bytes.entry(format!("texts {lang}")).or_default() += size;
        let missing_keys = ui_keys.len() - covered;
        if missing_keys > 0 {
            entries.push(ReportEntry::new(
                "ui-text-missing",
                lang.clone(),
                format!(
                    "{missing_keys} of {} TXT_KEY names in UI files have no text",
                    ui_keys.len()
                ),
            ));
        }
    }
    if let (Some(en), Some(ru)) = (texts.table("en_US"), texts.table("ru_RU")) {
        let only_ru = ru.keys().filter(|k| !en.contains_key(*k)).count();
        doc.notes
            .push(format!("ru_RU keys without en_US: {only_ru}"));
    }
    doc.notes
        .push(format!("TXT_KEY names in UI files: {}", ui_keys.len()));
    doc.notes.extend(loc_notes);
    doc.notes.push(format!(
        "text rows read {}, replaced with other text {}",
        texts.rows_total, texts.replaced_total
    ));
    doc.notes.push(format!(
        "atlas rows {} ({} atlases), replaced {}",
        atlases.rows.len(),
        atlas_rows
            .iter()
            .map(|a| a.atlas.as_str())
            .collect::<BTreeSet<_>>()
            .len(),
        atlases.replaced
    ));

    // ---- Lua includes that nothing provides ----
    for (n, r) in &lua_refs {
        for inc in &r.includes {
            if vfs.find_stem(inc, &["lua"]).is_none() {
                gaps.push(("missing-include".into(), n.clone(), inc.clone()));
                entries.push(ReportEntry::new("missing-include", n.clone(), inc.clone()));
            }
        }
    }
    // Cyrillic is not in the shipped fonts: the Tw Cen MT atlases get letters of Ubuntu Light (SIMP-exe-716).
    doc.notes.push(
        "shipped Tw Cen MT glyphs cover codes 32-345 only: Cyrillic letters of Ubuntu Light are added to each Tw Cen MT atlas"
            .to_owned(),
    );

    // ---- sounds ----
    let mut parsed = audio::ParsedAudio::default();
    let mut parsed_3d = audio3d::Parsed3D::default();
    for (name, copies) in vfs.iter_names() {
        let lower = name.as_str();
        if ext_of(lower) != "xml" {
            continue;
        }
        for e in copies {
            if e.is_packed() || !e.rel.to_ascii_lowercase().contains("sounds") {
                continue;
            }
            let Ok(b) = read_loose(e) else { continue };
            let src = String::from_utf8_lossy(&b);
            let res = if src.contains("<Script2DFile") {
                Some(
                    audio::parse_scripts(&src, &mut parsed)
                        .and_then(|()| audio3d::parse_pitch_2d(&src, &mut parsed_3d)),
                )
            } else if src.contains("<Script3DFile") {
                Some(audio3d::parse_scripts_3d(&src, &mut parsed_3d))
            } else if src.contains("<AudioDefinesFile") {
                Some(audio::parse_defines(&src, &mut parsed))
            } else {
                None
            };
            if let Some(Err(m)) = res {
                gaps.push(("xml-parse".into(), e.rel.clone(), m));
            }
        }
    }
    for (file, table) in [
        ("notificationsounds.xml", LinkTable::Notification),
        ("buttonpopupsounds.xml", LinkTable::ButtonPopup),
        ("eventsoundconnections.xml", LinkTable::Event),
        ("advisorsoundconnections.xml", LinkTable::Advisor),
        ("minorcivsounds.xml", LinkTable::MinorCiv),
    ] {
        if let Some(e) = vfs.lookup(file) {
            let src = String::from_utf8_lossy(&vfs.read(e)?).into_owned();
            if let Err(m) = audio::parse_links(&src, table, &mut parsed) {
                gaps.push(("xml-parse".into(), file.into(), m));
            }
        }
    }
    // The world soundscapes (SIMP-exe-1025): the loops and the rows that use them. The files of an expansion come later in
    // the copies of a name and replace the rows of the base game.
    if let Some(copies) = vfs
        .iter_names()
        .find(|(n, _)| *n == "audiosoundscapescripts.xml")
        .map(|(_, c)| c)
    {
        for e in copies.iter().filter(|e| !e.is_packed()) {
            let src = String::from_utf8_lossy(&read_loose(e)?).into_owned();
            if let Err(m) = audio::parse_soundscapes(&src, &mut parsed) {
                gaps.push(("xml-parse".into(), e.rel.clone(), m));
            }
        }
    }
    for file in [
        "civ5terrains.xml",
        "civ5features.xml",
        "civ5improvements.xml",
    ] {
        let Some((_, copies)) = vfs.iter_names().find(|(n, _)| *n == file) else {
            continue;
        };
        for e in copies.iter().filter(|e| !e.is_packed()) {
            let src = String::from_utf8_lossy(&read_loose(e)?).into_owned();
            if let Err(m) = audio::parse_scape_bindings(&src, &mut parsed) {
                gaps.push(("xml-parse".into(), e.rel.clone(), m));
            }
        }
    }
    let scape_table = audio::soundscape_table(&parsed);
    let mut literal: BTreeSet<String> = BTreeSet::new();
    literal.extend(scape_table.scripts().into_iter().map(str::to_owned));
    for r in lua_refs.values() {
        literal.extend(r.sounds.iter().cloned());
        literal.extend(r.strings.iter().filter(|s| s.starts_with("AS2D_")).cloned());
    }
    for l in &loaded {
        if ext_of(&l.name) == "xml" {
            let t = String::from_utf8_lossy(&l.bytes);
            for w in t.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
                if w.starts_with("AS2D_") {
                    literal.insert(w.to_owned());
                }
            }
        }
    }
    let mut want_scripts = audio::wanted(&parsed, &literal, opts.voice);
    if opts.music {
        want_scripts.extend(audio::wanted_music(&parsed));
    }
    let names = audio::audio_names(&vfs);
    // The unit sounds (re-b15 ITEM 4): the sound rows of every model's timed triggers, the tags of the unit members, and
    // the 3D scripts the rows can name, whose files join the set.
    for (name, _) in vfs.iter_names().filter(|(n, _)| ext_of(n) == "fxsxml") {
        let Some(e) = vfs.lookup(name) else { continue };
        let Ok(b) = vfs.read(e) else { continue };
        if let Some(trig) = audio3d::timed_trigger_of(&String::from_utf8_lossy(&b)) {
            parsed_3d
                .model_triggers
                .insert(name.trim_end_matches(".fxsxml").to_owned(), trig);
        }
    }
    let trigger_files: BTreeSet<String> = parsed_3d.model_triggers.values().cloned().collect();
    for f in trigger_files {
        let Some(e) = vfs.lookup(&f) else { continue };
        let Ok(b) = vfs.read(e) else { continue };
        match audio3d::parse_ftsxml(&String::from_utf8_lossy(&b)) {
            Ok(rows) => {
                parsed_3d.triggers.insert(f, rows);
            }
            Err(m) => gaps.push(("xml-parse".into(), f, m)),
        }
    }
    if let Some((_, copies)) = vfs
        .iter_names()
        .find(|(n, _)| *n == "civ5artdefines_unitmembers.xml")
    {
        for e in copies {
            let Ok(b) = vfs.read(e) else { continue };
            if let Err(m) = audio3d::parse_member_tags(&String::from_utf8_lossy(&b), &mut parsed_3d)
            {
                gaps.push(("xml-parse".into(), e.rel.clone(), m));
            }
        }
    }
    let table_3d = audio3d::table(&parsed_3d, &want_scripts);
    let mut ab = audio::build(&parsed, &names, &want_scripts);
    let files_2d = ab.files.clone();
    audio::add_sounds(
        &mut ab,
        &parsed,
        &names,
        table_3d.sounds(),
        asset_format::SOUND3D_TABLE_ID,
    );
    let mut unit_audio_bytes = 0u64;
    let mut audio_bytes = 0u64;
    for n in &ab.files {
        let e = vfs.lookup(n).expect("name came from the vfs");
        let (hash, size) = store.put(&vfs.read(e)?)?;
        audio_bytes += size;
        if !files_2d.contains(n) {
            unit_audio_bytes += size;
        }
        let id = ui_sound_file_id(n);
        manifest.add(
            LogicalId::new(id.clone()),
            Variant {
                format: VariantFormat::Generic,
                hash,
                size,
                prefetch: PrefetchGroup(0),
            },
        );
        sizes.insert(id, size);
    }
    for g in &ab.gaps {
        gaps.push(("missing-sound".into(), g.script.clone(), g.reason.clone()));
        entries.push(ReportEntry::new(
            "missing-sound",
            g.script.clone(),
            g.reason.clone(),
        ));
    }
    let snd_bytes = ab.table.encode();
    let (snd_hash, snd_size) = store.put(&snd_bytes)?;
    manifest.add(
        LogicalId::new(asset_format::AUDIO_TABLE_ID),
        Variant {
            format: VariantFormat::Generic,
            hash: snd_hash,
            size: snd_size,
            prefetch: PrefetchGroup(0),
        },
    );
    sizes.insert(asset_format::AUDIO_TABLE_ID.to_owned(), snd_size);
    // The unit sound table: an older client or set without it plays no unit sound.
    let s3d_bytes = table_3d.encode();
    let (s3d_hash, s3d_size) = store.put(&s3d_bytes)?;
    manifest.add(
        LogicalId::new(asset_format::SOUND3D_TABLE_ID),
        Variant {
            format: VariantFormat::Generic,
            hash: s3d_hash,
            size: s3d_size,
            prefetch: PrefetchGroup(0),
        },
    );
    sizes.insert(asset_format::SOUND3D_TABLE_ID.to_owned(), s3d_size);
    let _ = asset_format::Sound3DTable::decode(&s3d_bytes)
        .map_err(|e| UiError::Other(e.to_string()))?;
    doc.counts
        .insert("unit sound scripts".into(), table_3d.scripts.len() as u64);
    doc.counts.insert(
        "unit models with sound rows".into(),
        table_3d.models.len() as u64,
    );
    doc.bytes
        .insert("unit sound files".into(), unit_audio_bytes);
    // The soundscape table: an older client or set without it plays no world ambience.
    let scape_bytes = scape_table.encode();
    let (scape_hash, scape_size) = store.put(&scape_bytes)?;
    manifest.add(
        LogicalId::new(asset_format::SOUNDSCAPE_TABLE_ID),
        Variant {
            format: VariantFormat::Generic,
            hash: scape_hash,
            size: scape_size,
            prefetch: PrefetchGroup(0),
        },
    );
    sizes.insert(asset_format::SOUNDSCAPE_TABLE_ID.to_owned(), scape_size);
    doc.counts
        .insert("world soundscapes".into(), scape_table.scapes.len() as u64);
    doc.counts.insert(
        "world soundscape bindings".into(),
        scape_table.bindings.len() as u64,
    );
    let _ = asset_format::SoundscapeTable::decode(&scape_bytes)
        .map_err(|e| UiError::Other(e.to_string()))?;
    doc.counts
        .insert("sound scripts in the set".into(), ab.scripts_kept as u64);
    doc.counts
        .insert("sound scripts in the files".into(), ab.scripts_total as u64);
    doc.counts
        .insert("sound files".into(), ab.files.len() as u64);
    doc.bytes.insert("sound files".into(), audio_bytes);
    let _ = AudioTable::decode(&snd_bytes).map_err(|e| UiError::Other(e.to_string()))?;

    // ---- windows, boot, hud, atlases ----
    let atlas_names: BTreeSet<&str> = atlas_rows.iter().map(|a| a.atlas.as_str()).collect();
    let id_f = |n: &str| ui_file_id(n);
    // contexts: stems with an xml
    let mut contexts: BTreeMap<String, (Option<String>, Option<String>)> = BTreeMap::new();
    for n in xml_refs.keys() {
        let stem = n.trim_end_matches(".xml").to_owned();
        let lua = format!("{stem}.lua");
        let lua = by_name.contains_key(lua.as_str()).then_some(lua);
        contexts.insert(stem, (Some(n.clone()), lua));
    }
    // Only files that make a context: they hold a Context root or LuaContext children, or have a Lua twin.
    contexts.retain(|stem, (_, lua)| {
        lua.is_some() || !xml_refs[&format!("{stem}.xml")].contexts.is_empty()
    });
    // library closure per Lua file
    let lib_closure = |start: &str| -> BTreeSet<String> {
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut stack = vec![start.to_owned()];
        while let Some(n) = stack.pop() {
            if !seen.insert(n.clone()) {
                continue;
            }
            if let Some(r) = lua_refs.get(&n) {
                for inc in &r.includes {
                    if let Some(e) = vfs.find_stem(inc, &["lua"]) {
                        stack.push(e.name.clone());
                    }
                }
                for pat in &r.wild_includes {
                    stack.extend(vfs.find_wild(pat).into_iter().map(|e| e.name.clone()));
                }
            }
        }
        seen
    };
    let mut lib_use: BTreeMap<String, usize> = BTreeMap::new();
    let mut closures: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (stem, (_, lua)) in &contexts {
        if let Some(l) = lua {
            let c = lib_closure(l);
            for lib in c.iter().filter(|x| *x != l) {
                *lib_use.entry(lib.clone()).or_default() += 1;
            }
            closures.insert(stem.clone(), c);
        }
    }
    let shared_libs: BTreeSet<String> = lib_use
        .iter()
        .filter(|(_, n)| **n >= SHARED_LIB_MIN_WINDOWS)
        .map(|(l, _)| l.clone())
        .collect();
    let style_closure = |names: &BTreeSet<String>| -> (BTreeSet<String>, BTreeSet<String>) {
        let mut dds = BTreeSet::new();
        let mut unknown = BTreeSet::new();
        let mut seen = BTreeSet::new();
        let mut stack: Vec<String> = names.iter().cloned().collect();
        while let Some(s) = stack.pop() {
            if !seen.insert(s.clone()) {
                continue;
            }
            match styles.get(&s) {
                Some(r) => {
                    dds.extend(r.dds.iter().cloned());
                    stack.extend(r.styles.iter().cloned());
                }
                None => {
                    unknown.insert(s);
                }
            }
        }
        (dds, unknown)
    };
    let ok_tex = |n: &str| tex_names.contains(n);
    let size_of = |id: &str| sizes.get(id).copied().unwrap_or(0);
    let mut groups: Vec<UiGroup> = Vec::new();
    let mut boot_members: BTreeSet<String> = BTreeSet::new();
    boot_members.insert(UI_INDEX_ID.to_owned());
    boot_members.insert(UI_TABLES_ID.to_owned());
    boot_members.insert(asset_format::AUDIO_TABLE_ID.to_owned());
    boot_members.extend(locale_ids.iter().cloned());
    for f in &fonts {
        boot_members.insert(id_f(&f.ggxml));
        for t in f.textures.iter().filter(|t| ok_tex(t)) {
            boot_members.insert(ui_texture_id(t));
        }
    }
    for n in skin_styles.iter().chain(skin_colors.iter()) {
        if by_name.contains_key(n.as_str()) || sizes.contains_key(&id_f(n)) {
            boot_members.insert(id_f(n));
        }
    }
    if by_name.contains_key(HIGHLIGHTS) {
        boot_members.insert(id_f(HIGHLIGHTS));
    }
    for l in &shared_libs {
        boot_members.insert(id_f(l));
        if let Some(r) = lua_refs.get(l) {
            boot_members.extend(r.dds.iter().filter(|t| ok_tex(t)).map(|t| ui_texture_id(t)));
        }
    }
    let boot_bytes = |m: &BTreeSet<String>| m.iter().map(|i| size_of(i)).sum::<u64>();
    let mut group_members: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut group_requires: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut unknown_styles: BTreeMap<String, usize> = BTreeMap::new();
    for (stem, (xml, lua)) in &contexts {
        let gid = format!("ui:win:{stem}");
        let mut m: BTreeSet<String> = BTreeSet::new();
        let mut dds: BTreeSet<String> = BTreeSet::new();
        let mut style_names: BTreeSet<String> = BTreeSet::new();
        if let Some(x) = xml {
            m.insert(id_f(x));
            let r = &xml_refs[x];
            dds.extend(r.dds.iter().cloned());
            style_names.extend(r.styles.iter().cloned());
        }
        if let Some(l) = lua {
            for lib in &closures[stem] {
                if shared_libs.contains(lib) {
                    continue;
                }
                m.insert(id_f(lib));
                if let Some(r) = lua_refs.get(lib) {
                    dds.extend(r.dds.iter().cloned());
                }
            }
            let _ = l;
        }
        let (sd, unk) = style_closure(&style_names);
        for u in unk {
            *unknown_styles.entry(u).or_default() += 1;
        }
        dds.extend(sd);
        m.extend(dds.iter().filter(|t| ok_tex(t)).map(|t| ui_texture_id(t)));
        m.retain(|i| !boot_members.contains(i));
        // A window does not require the atlases its Lua names (M3 WO-I): the client fetches an icon atlas when a
        // control takes its texture (`UiStore::texture_info`), so the closure of a window holds no icon sheets.
        let req: BTreeSet<String> = BTreeSet::from(["ui:boot".to_owned()]);
        group_members.insert(gid.clone(), m);
        group_requires.insert(gid, req);
    }
    for (s, n) in &unknown_styles {
        gaps.push(("unknown-style".into(), s.clone(), format!("{n} windows")));
    }
    // atlas groups
    for a in &atlas_names {
        let m: BTreeSet<String> = atlas_rows
            .iter()
            .filter(|r| r.atlas == *a && ok_tex(&r.file))
            .map(|r| ui_texture_id(&r.file))
            .collect();
        group_members.insert(format!("ui:atlas:{a}"), m);
        group_requires.insert(format!("ui:atlas:{a}"), BTreeSet::new());
    }
    group_members.insert("ui:boot".into(), boot_members.clone());
    group_requires.insert("ui:boot".into(), BTreeSet::new());
    // HUD: contexts that ingame.xml shows at start, and what they show.
    let mut hud: BTreeSet<String> = BTreeSet::new();
    let mut stack: Vec<String> = vec![ENTRY_CONTEXT.to_owned()];
    while let Some(c) = stack.pop() {
        let Some(r) = xml_refs.get(&format!("{c}.xml")) else {
            continue;
        };
        for (child, hidden) in &r.contexts {
            if *hidden {
                continue;
            }
            let stem = child
                .trim_end_matches(".xml")
                .trim_end_matches(".lua")
                .to_owned();
            if hud.insert(stem.clone()) {
                stack.push(stem);
            }
        }
    }
    let hud_windows: BTreeSet<String> = hud
        .iter()
        .filter(|h| group_members.contains_key(&format!("ui:win:{h}")))
        .cloned()
        .collect();
    group_members.insert("ui:hud".into(), BTreeSet::new());
    let mut hud_req: BTreeSet<String> = BTreeSet::from(["ui:boot".to_owned()]);
    hud_req.extend(hud_windows.iter().map(|h| format!("ui:win:{h}")));
    if group_members.contains_key("ui:win:ingame") {
        hud_req.insert("ui:win:ingame".into());
    }
    for h in &hud_windows {
        if let Some(r) = group_requires.get(&format!("ui:win:{h}")) {
            hud_req.extend(r.iter().cloned());
        }
    }
    group_requires.insert("ui:hud".into(), hud_req);
    // bytes: union of the group and everything it requires (for hud, the amount to fetch)
    let closure_bytes = |gid: &str| -> u64 {
        let mut all: BTreeSet<&str> = BTreeSet::new();
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let mut st = vec![gid];
        while let Some(g) = st.pop() {
            if !seen.insert(g) {
                continue;
            }
            if let Some(m) = group_members.get(g) {
                all.extend(m.iter().map(String::as_str));
            }
            if let Some(r) = group_requires.get(g) {
                st.extend(r.iter().map(String::as_str));
            }
        }
        all.iter().map(|i| size_of(i)).sum()
    };
    for (gid, m) in &group_members {
        let own = boot_bytes(m);
        groups.push(UiGroup {
            id: gid.clone(),
            members: m.iter().cloned().collect(),
            bytes: own,
            requires: group_requires[gid].iter().cloned().collect(),
        });
        doc.groups.push(GroupRow {
            id: gid.clone(),
            members: m.len(),
            bytes: if gid == "ui:hud" {
                closure_bytes(gid)
            } else {
                own
            },
        });
    }
    doc.groups.sort_by_key(|g| std::cmp::Reverse(g.bytes));
    let hud_total = closure_bytes("ui:hud");
    doc.bytes.insert(
        "HUD download (boot + hud windows + their atlases)".into(),
        hud_total,
    );
    for (label, mbit) in [("10 Mbit/s", 10.0), ("50 Mbit/s", 50.0)] {
        doc.notes.push(format!(
            "AC-P2: ui:hud closure {:.2} MB, {:.1} s at {label}",
            hud_total as f64 / 1e6,
            hud_total as f64 * 8.0 / (mbit * 1e6)
        ));
    }
    doc.notes.push(format!(
        "HUD windows: {}",
        hud_windows.iter().cloned().collect::<Vec<_>>().join(", ")
    ));

    // ---- Override/ ----
    let ui_names: BTreeSet<String> = files.iter().map(|f| f.name.clone()).collect();
    let override_rep = overrides::analyze(&vfs, &texts, &ui_names)?;
    doc.override_ = OverrideSummary::from(&override_rep);

    // ---- index ----
    let index = UiIndex {
        layers: vfs
            .layers
            .iter()
            .enumerate()
            .map(|(i, l)| UiLayerInfo {
                name: l.name.clone(),
                dir: l
                    .dir
                    .strip_prefix(game.join("Assets"))
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_default(),
                priority: l.priority,
                order: i as u32,
                packages: l.packages.clone(),
                priority_defaulted: l.priority_defaulted,
            })
            .collect(),
        files,
        fonts,
        atlases: atlas_rows,
        tex_dims: tex_dims
            .iter()
            .map(|(file, &(width, height))| UiTexDim {
                file: file.clone(),
                width,
                height,
            })
            .collect(),
        skin_styles: skin_styles.clone(),
        skin_colors: skin_colors.clone(),
        groups,
    };
    let idx_bytes = index.encode();
    UiIndex::decode(&idx_bytes).map_err(|e| UiError::Other(e.to_string()))?;
    let (ih, isz) = store.put(&idx_bytes)?;
    manifest.add(
        LogicalId::new(UI_INDEX_ID),
        Variant {
            format: VariantFormat::Generic,
            hash: ih,
            size: isz,
            prefetch: PrefetchGroup(0),
        },
    );
    manifest.finish(&store)?;

    // ---- report ----
    doc.counts
        .insert("ui files".into(), ui_entries.len() as u64);
    doc.counts.insert("textures".into(), tex_names.len() as u64);
    doc.counts.insert("fonts".into(), index.fonts.len() as u64);
    doc.counts
        .insert("atlas rows".into(), index.atlases.len() as u64);
    doc.counts.insert("windows".into(), contexts.len() as u64);
    doc.counts.insert("gaps".into(), gaps.len() as u64);
    doc.counts.insert("vfs names".into(), vfs.len() as u64);
    for (e, b) in &file_bytes {
        doc.bytes.insert(format!("files .{e}"), *b);
    }
    for (f, b) in &tex_bytes {
        doc.bytes.insert(format!("textures {f}"), *b);
    }
    doc.bytes.insert("index".into(), isz);
    doc.bytes.insert("sound table".into(), snd_size);
    let total: u64 = sizes.values().sum::<u64>() + isz;
    doc.bytes.insert("total".into(), total);
    doc.gaps = gaps;
    let mut report = SetReport {
        entries,
        counts: doc.counts.clone(),
        bytes: doc.bytes.clone(),
    };
    report.entries.push(ReportEntry::new(
        "override-summary",
        "Override/",
        format!(
            "{} files, {} zero-byte, {} would blank a file, {} would replace, {} UI names",
            doc.override_.total,
            doc.override_.zero_bytes,
            doc.override_.blanks,
            doc.override_.replaces,
            doc.override_.ui_overlap.len()
        ),
    ));
    Ok(UiSummary {
        report,
        doc,
        seconds: started.elapsed().as_secs_f64(),
    })
}

/// The text without `<!-- -->` comments and the XML declaration.
fn strip_comments(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(p) = rest.find("<!--") {
        out.push_str(&rest[..p]);
        match rest[p..].find("-->") {
            Some(e) => rest = &rest[p + e + 3..],
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    if let (Some(a), Some(b)) = (out.find("<?"), out.find("?>")) {
        out.replace_range(a..b + 2, "");
    }
    out
}

/// `TXT_KEY_*` names in UI files (XML attributes and Lua strings).
fn ui_text_keys(loaded: &[Loaded]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for l in loaded {
        if !matches!(ext_of(&l.name), "xml" | "lua") {
            continue;
        }
        let t = String::from_utf8_lossy(&l.bytes);
        for w in t.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
            if w.starts_with("TXT_KEY_") && w.len() > 8 {
                out.insert(w.to_owned());
            }
        }
    }
    out
}

/// The loose `.ggxml` files of the icon fonts the tables name (`IconFontTextures.IconFontTextureFile`), whichever
/// folder of the VFS they sit in.
pub fn icon_font_entries<'a>(vfs: &'a UiVfs, tables: &asset_format::UiTables) -> Vec<&'a UiEntry> {
    let mut out: Vec<&UiEntry> = Vec::new();
    for (_, file) in &tables.icon_textures {
        let name = format!("{}.ggxml", file.to_ascii_lowercase());
        if let Some(e) = vfs.lookup(&name)
            && !e.is_packed()
            && !out.iter().any(|x| x.name == e.name)
        {
            out.push(e);
        }
    }
    out
}

/// Texture names a `.ggxml` font names (`src="X.dds"`).
fn font_textures(src: &str) -> Vec<String> {
    let Ok(doc) = roxmltree::Document::parse(src.trim_start_matches('\u{feff}')) else {
        return Vec::new();
    };
    let mut out: Vec<String> = doc
        .descendants()
        .filter(|n| n.tag_name().name() == "texture")
        .filter_map(|n| n.attribute("src"))
        .filter(|s| s.to_ascii_lowercase().ends_with(".dds"))
        .map(refs::bare)
        .collect();
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_roots() {
        let mut l = UiLayer {
            name: "UI_bc1".into(),
            kind: LayerKind::Package,
            dir: "x".into(),
            priority: 250,
            packages: vec![],
            priority_defaulted: false,
            packs: vec![],
            style_files: vec![],
            color_files: vec![],
            skin_dirs: vec!["core".into(), "art".into()],
        };
        assert!(in_ui_roots(&l, "Core/EUI_context.lua"));
        assert!(!in_ui_roots(&l, "Art/Blank.dds"));
        assert!(in_ui_roots(&l, "UI/InGame/TopPanel.xml"));
        assert!(in_ui_roots(&l, "Lua/UI/TopPanel.lua"));
        assert!(!in_ui_roots(&l, "Lua/Lekmod_policies.lua"));
        assert!(in_ui_roots(&l, "Fonts/FontIcons_Patch.ggxml"));
        l.skin_dirs.clear();
        assert!(!in_ui_roots(&l, "Core/EUI_context.lua"));
    }

    #[test]
    fn font_texture_names() {
        let x = r#"<glyphgen><textures><texture name="" src="TwCenMT16.dds"/><texture name="a" src="D:\x\y.png"/><texture src="Sub/Base.DDS"/></textures></glyphgen>"#;
        assert_eq!(font_textures(x), ["base.dds", "twcenmt16.dds"]);
    }

    #[test]
    fn text_keys_found_in_attributes_and_strings() {
        let l = vec![
            Loaded {
                name: "a.xml".into(),
                bytes: br#"<L String="TXT_KEY_A_B" ToolTip="TXT_KEY_"/>"#.to_vec(),
            },
            Loaded {
                name: "a.lua".into(),
                bytes: b"Locale.ConvertTextKey(\"TXT_KEY_C\")".to_vec(),
            },
            Loaded {
                name: "a.dds".into(),
                bytes: b"TXT_KEY_NO".to_vec(),
            },
        ];
        assert_eq!(
            ui_text_keys(&l).into_iter().collect::<Vec<_>>(),
            ["TXT_KEY_A_B", "TXT_KEY_C"]
        );
    }

    fn write(root: &Path, rel: &str, bytes: &[u8]) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    fn argb_dds() -> Vec<u8> {
        use ddsfile::{D3DFormat, Dds, NewD3dParams};
        let mut d = Dds::new_d3d(NewD3dParams {
            height: 2,
            width: 2,
            depth: None,
            format: D3DFormat::A8R8G8B8,
            mipmap_levels: None,
            caps2: None,
        })
        .unwrap();
        d.data.fill(9);
        let mut out = Vec::new();
        d.write(&mut out).unwrap();
        out
    }

    /// A small install: base UI, EUI over it, a text, an atlas, a sound.
    fn mini_install() -> std::path::PathBuf {
        use crate::game::fpk::testutil::build_fpk;
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "civ5-ui-build-{}-{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let dds = argb_dds();
        write(
            &root,
            "Resource/DX9/UITextures.fpk",
            &build_fpk(&[
                ("Frame.dds", &dds),
                ("Frame-index.dds", &dds),
                ("Icons.dds", &dds),
            ]),
        );
        write(
            &root,
            "Resource/Common/SoundData.fpk",
            &build_fpk(&[("Click.wav", b"RIFFxxxx")]),
        );
        write(&root, "Assets/UI/InGame/InGame.xml", br#"<Context><LuaContext FileName="Assets/UI/InGame/TopPanel" ID="TopPanel"/><LuaContext FileName="Popup" Hidden="1"/></Context>"#);
        write(&root, "Assets/UI/InGame/InGame.lua", b"-- ingame");
        write(&root, "Assets/UI/InGame/TopPanel.xml", br#"<Context><Grid Style="Frame9" /><Image Texture="Assets\UI\Art\Frame.dds"/><Label String="TXT_KEY_HELLO"/></Context>"#);
        write(&root, "Assets/UI/InGame/TopPanel.lua", b"include(\"Lib\")\nEvents.AudioPlay2DSound(\"AS2D_IF_SELECT\")\nIconHookup(1, 64, \"MY_ATLAS\", img)");
        write(
            &root,
            "Assets/UI/InGame/Popup.xml",
            br#"<Context><Image Texture="Big.dds"/></Context>"#,
        );
        write(&root, "Assets/UI/InGame/Popup.lua", b"-- popup");
        write(&root, "Assets/UI/Lib.lua", b"-- lib");
        write(
            &root,
            "Assets/UI/Styles.xml",
            br#"<StyleSheet><Frame9 Texture="Frame.dds"/></StyleSheet>"#,
        );
        write(&root, "Assets/Gameplay/XML/NewText/EN_US/T.xml", br#"<GameData><Language_en_US><Row Tag="TXT_KEY_HELLO"><Text>Hello</Text></Row></Language_en_US></GameData>"#);
        write(&root, "Assets/Gameplay/XML/Atlases.xml", br#"<GameData><IconTextureAtlases><Row><Atlas>MY_ATLAS</Atlas><IconSize>64</IconSize><Filename>Icons.dds</Filename><IconsPerRow>2</IconsPerRow><IconsPerColumn>2</IconsPerColumn></Row></IconTextureAtlases></GameData>"#);
        write(&root, "Assets/Sounds/XML/Audio2DScripts.xml", br#"<Script2DFile><Script2DSounds><Script2DSound><ScriptID>AS2D_IF_SELECT</ScriptID><SoundID>SND_S</SoundID><SoundType>GAME_SFX</SoundType></Script2DSound></Script2DSounds></Script2DFile>"#);
        write(&root, "Assets/Sounds/XML/AudioDefines.xml", br#"<AudioDefinesFile><SoundDatas><SoundData><SoundID>SND_S</SoundID><Filename>Click</Filename><LoadType>RESIDENT</LoadType></SoundData></SoundDatas></AudioDefinesFile>"#);
        write(&root, "Assets/DLC/UI_bc1/EUI_2.Civ5Pkg", b"<Civ5Package><Priority>250</Priority><UISkin name=\"e\" set=\"Expansion2\" platform=\"Common\"><Skin><Directory>Core</Directory></Skin></UISkin></Civ5Package>");
        write(&root, "Assets/DLC/UI_bc1/Core/Lib.lua", b"-- eui lib");
        // A mod package whose Override folder holds a text the game database names (Lekmod's `CIV5Units_Mongol.xml`).
        write(
            &root,
            "Assets/DLC/LEKMOD/M.Civ5Pkg",
            b"<Civ5Package><Priority>300</Priority></Civ5Package>",
        );
        write(&root, "Assets/DLC/LEKMOD/Override/Units.xml", br#"<GameData><Units/><Language_en_US><Replace Tag="TXT_KEY_CIV_X_SHORT_DESC"><Text>Xland</Text></Replace></Language_en_US></GameData>"#);
        root
    }

    #[test]
    fn builds_a_small_install_end_to_end() {
        let game = mini_install();
        let out = game.join("out");
        let s = build_ui(&game, &out, &UiOptions::default()).unwrap();
        let store = BlobStore::open(&out).unwrap();
        let m = ManifestBuilder::open(&store).unwrap();
        let man = m.manifest();
        for id in [
            "ui:index",
            "ui:snd",
            "ui:text:en_us",
            "ui:f:toppanel.xml",
            "ui:f:lib.lua",
            "ui:tx:frame.dds",
            "ui:tx:frame-index.dds",
            "ui:tx:icons.dds",
            "ui:au:click.wav",
        ] {
            assert!(
                !man.variants(&LogicalId::new(id)).is_empty(),
                "{id} missing: {:?}",
                s.doc.gaps
            );
        }
        // `Big.dds` is named but does not exist: a gap, not a failure.
        assert!(
            s.doc
                .gaps
                .iter()
                .any(|g| g.0 == "missing-texture" && g.1 == "big.dds")
        );
        let v = &man.variants(&LogicalId::new("ui:index"))[0];
        let idx = UiIndex::decode(&std::fs::read(store.blob_path(&v.hash)).unwrap()).unwrap();
        // EUI (250) hides the base `Lib.lua`.
        let lib = idx.find("LIB.lua").unwrap();
        assert_eq!(idx.layers[lib.layer as usize].name, "UI_bc1");
        assert_eq!(lib.hidden, 1);
        assert_eq!(
            idx.find_path("assets\\UI\\Art\\frame-index.dds")
                .unwrap()
                .index_of
                .as_deref(),
            Some("frame.dds")
        );
        // The window group holds its files and its style texture; the atlas its Lua names is a group of its own that the
        // window does not require (the client fetches an atlas when a control takes it).
        let g = idx.group("ui:win:toppanel").unwrap();
        for m in ["ui:f:toppanel.xml", "ui:f:toppanel.lua", "ui:tx:frame.dds"] {
            assert!(
                g.members.iter().any(|x| x == m),
                "{m} not in {:?}",
                g.members
            );
        }
        assert!(!g.requires.contains(&"ui:atlas:MY_ATLAS".to_owned()));
        assert!(
            idx.group("ui:atlas:MY_ATLAS")
                .unwrap()
                .members
                .contains(&"ui:tx:icons.dds".to_owned())
        );
        // The popup is hidden in InGame.xml, so it is not in the HUD.
        let hud = idx.group("ui:hud").unwrap();
        assert!(hud.requires.contains(&"ui:win:toppanel".to_owned()));
        assert!(!hud.requires.contains(&"ui:win:popup".to_owned()));
        assert_eq!(idx.atlases[0].atlas, "MY_ATLAS");
        let t = &man.variants(&LogicalId::new("ui:text:en_us"))[0];
        let loc = LocaleTable::decode(&std::fs::read(store.blob_path(&t.hash)).unwrap()).unwrap();
        assert_eq!(loc.get("TXT_KEY_HELLO"), Some("Hello"));
        assert_eq!(
            loc.get("TXT_KEY_CIV_X_SHORT_DESC"),
            Some("Xland"),
            "text of an Override file"
        );
        let _ = std::fs::remove_dir_all(&game);
    }
}
