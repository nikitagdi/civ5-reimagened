//! Set `web-lobby`: the art of the DOM lobby page (the browser shows it through `<img>` / CSS, not
//! through the engine).
//!
//! Blobs, all `Generic` variants without a prefetch group (loaded on demand):
//! - `web:lobby`: one JSON document (UTF-8, compact, no header), see [`LobbyDoc`];
//! - `web:png:civ-<civ type, lower case>`: a sprite strip per civilization: the leader portrait
//!   128x128 at (0,0), then the civ icon and each unique icon at 64x64 in two columns to the right
//!   (x = 128 + col * 64, y = row * 64);
//! - `web:png:fonticons-<n>`: a whole icon-font texture; `icons` of the JSON point into it.
//!
//! Textures resolve through the UI file system exactly as the in-game UI resolves them (the
//! `IconTextureAtlases.Filename` is a bare file name; block-dictionary textures are expanded).
//!
//! SIMPLIFIED(SIMP-exe-1050): the strips hold the first leader of each civilization and no
//! variants: the lobby picks a civilization only (the core's `CIV_INFO` query does the same).
//! SIMPLIFIED(SIMP-exe-1051): the icons the lobby draws are the DB-named icons at fixed sizes
//! (128 leader, 64 others) cut from the atlases and resampled with a box filter when the atlas has
//! no row of that size; the colors and icon glyphs of the page are rects of the same PNGs, so the
//! page does not draw the engine's text layers (outline, shadow, color runs).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex};

use asset_format::{LogicalId, PrefetchGroup, TextureBlob, TextureFormat, Variant, VariantFormat};
use serde::Serialize;
use serde::ser::{SerializeMap, Serializer};

use crate::db::{DbError, SnapDb, open_snapshot};
use crate::gamedata::{GameDataError, LOC_DB_FILE};
use crate::models::png::crc32;
use crate::report::{ReportEntry, SetReport};
use crate::store::{BlobStore, ManifestBuilder, StoreError};
use crate::textures::Rgba8Image;
use crate::ui::atlas::AtlasRow;
use crate::ui::tex::ui_texture;
use crate::ui::vfs::{UiVfs, bare_name};
use crate::ui::{UiError, UiOptions, open_vfs};

pub const SET_NAME: &str = "web-lobby";
pub const LOBBY_ID: &str = "web:lobby";
/// Version field of the JSON document.
pub const DOC_VERSION: u32 = 1;
/// Side of the leader portrait in a strip.
pub const LEADER_PX: u32 = 128;
/// Side of the civ icon and the unique icons in a strip.
pub const ICON_PX: u32 = 64;
/// Columns of the small icons right of the portrait.
pub const STRIP_COLS: u32 = 2;
/// Cached source textures above this many bytes are dropped (they are read again when needed).
const CACHE_LIMIT: usize = 768 << 20;
const LANG: &str = "en_US";

pub fn civ_png_id(civ_type: &str) -> String {
    format!("web:png:civ-{}", civ_type.to_ascii_lowercase())
}

pub fn font_png_id(n: usize) -> String {
    format!("web:png:fonticons-{n}")
}

#[derive(Debug, thiserror::Error)]
pub enum WebLobbyError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Loc(#[from] GameDataError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Ui(#[from] UiError),
    #[error("{0}")]
    Other(String),
}

// ---- input: the rows of the snapshot ----------------------------------------------------------

/// `IconAtlas` + `PortraitIndex` of a row; either may be NULL or negative (no icon).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IconRef {
    pub atlas: Option<String>,
    pub index: Option<u32>,
}

/// The numbers of a unique unit or building the original tooltip lists (`GetHelpTextForUnit`,
/// `GetHelpTextForBuilding`); a column that is NULL stays `None`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Numbers {
    pub moves: Option<i64>,
    pub combat: Option<i64>,
    pub ranged: Option<i64>,
    pub range: Option<i64>,
    pub cost: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UniqueIn {
    /// `Unit`, `Building` or `Improvement`.
    pub kind: &'static str,
    pub ty: String,
    /// The `Description` text tag.
    pub desc: String,
    pub icon: IconRef,
    /// The `Help` text tag (the original's "pre-written help text"); empty when the row has none.
    pub help: String,
    /// The `Description` tag of the default item of the class this one replaces (the standard
    /// unit or building of the class); `None` when the class default is this item itself.
    pub replaces: Option<String>,
    pub numbers: Numbers,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaderIn {
    pub ty: String,
    pub desc: String,
    pub icon: IconRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CivIn {
    pub ty: String,
    pub short_desc: String,
    pub desc: String,
    pub icon: IconRef,
    pub leader: Option<LeaderIn>,
    pub uniques: Vec<UniqueIn>,
}

/// Everything the set reads from the DB snapshot and the merged localization.
#[derive(Debug, Clone, Default)]
pub struct Input {
    /// `Colors`: name and red, green, blue, alpha in 0..1.
    pub colors: Vec<(String, [f32; 4])>,
    /// `IconTextureAtlases` by atlas name; one row per size, a later row replaces an earlier one.
    pub atlases: BTreeMap<String, Vec<AtlasRow>>,
    /// `IconFontTextures`: key to font file stem.
    pub font_files: BTreeMap<String, String>,
    /// `IconFontMapping`: icon name, glyph code, texture key.
    pub icons: Vec<(String, u32, String)>,
    /// In the order of the core's `CIV_INFO` query (`Civilizations.ID`).
    pub civs: Vec<CivIn>,
    /// en_US texts by tag.
    pub text: HashMap<String, String>,
}

fn cell_u32(v: &Option<String>) -> Option<u32> {
    v.as_deref()?.trim().parse::<i64>().ok()?.try_into().ok()
}

fn cell_i64(v: &Option<String>) -> Option<i64> {
    let t = v.as_deref()?.trim();
    t.parse::<i64>()
        .ok()
        .or_else(|| t.parse::<f64>().ok().map(|f| f as i64))
}

fn icon_of(atlas: &Option<String>, index: &Option<String>) -> IconRef {
    IconRef {
        atlas: atlas.clone().filter(|a| !a.is_empty()),
        index: cell_u32(index),
    }
}

fn text(v: &Option<String>) -> String {
    v.clone().unwrap_or_default()
}

/// A color channel in 0..=1 as the exe packs it: `(int)(v * 255)`, truncated (the rule of
/// `ui-text`'s `channel_u8`, `UIManager::GetGameplayColorByName`).
pub fn channel_u8(v: f32) -> u8 {
    (v * 255.0) as i32 as u8
}

pub fn color_rgba(c: [f32; 4]) -> [u8; 4] {
    c.map(channel_u8)
}

/// Reads the rows of `<db_dir>/Civ5DebugDatabase.db` and the en_US texts of
/// `<db_dir>/Localization-Merged.db`.
pub fn load_input(db_dir: &Path) -> Result<Input, WebLobbyError> {
    let db = open_snapshot(db_dir)?;
    let mut input = Input::default();
    read_colors(&db, &mut input)?;
    read_atlases(&db, &mut input)?;
    read_fonts(&db, &mut input)?;
    read_civs(&db, &mut input)?;
    for r in crate::ui::oracle::read_language(&db_dir.join(LOC_DB_FILE), LANG)? {
        input.text.insert(r.tag, r.text);
    }
    Ok(input)
}

fn read_colors(db: &SnapDb, input: &mut Input) -> Result<(), WebLobbyError> {
    for r in db.query(
        "SELECT Type, Red, Green, Blue, Alpha FROM Colors ORDER BY ID",
        5,
    )? {
        let name = text(&r[0]);
        let mut c = [0f32; 4];
        for (i, slot) in c.iter_mut().enumerate() {
            let t = text(&r[i + 1]);
            *slot = t.trim().parse::<f64>().map_err(|_| {
                WebLobbyError::Other(format!("Colors {name}: {t:?} is not a number"))
            })? as f32;
        }
        input.colors.push((name, c));
    }
    Ok(())
}

fn read_atlases(db: &SnapDb, input: &mut Input) -> Result<(), WebLobbyError> {
    let rows = db.query(
        "SELECT Atlas, IconSize, Filename, IconsPerRow, IconsPerColumn FROM IconTextureAtlases ORDER BY rowid",
        5,
    )?;
    for r in rows {
        let num = |i: usize| text(&r[i]).trim().parse::<u32>().ok();
        let (Some(size), Some(per_row), Some(per_col)) = (num(1), num(3), num(4)) else {
            continue;
        };
        let row = AtlasRow {
            atlas: text(&r[0]),
            icon_size: size,
            filename: text(&r[2]),
            icons_per_row: per_row,
            icons_per_column: per_col,
        };
        let list = input.atlases.entry(row.atlas.clone()).or_default();
        match list.iter_mut().find(|x| x.icon_size == size) {
            Some(old) => *old = row,
            None => list.push(row),
        }
    }
    Ok(())
}

fn read_fonts(db: &SnapDb, input: &mut Input) -> Result<(), WebLobbyError> {
    for r in db.query(
        "SELECT IconFontTexture, IconFontTextureFile FROM IconFontTextures ORDER BY rowid",
        2,
    )? {
        input.font_files.insert(text(&r[0]), text(&r[1]));
    }
    for r in db.query(
        "SELECT IconName, IconMapping, IconFontTexture FROM IconFontMapping ORDER BY rowid",
        3,
    )? {
        if let Ok(ch) = text(&r[1]).trim().parse::<u32>() {
            input.icons.push((text(&r[0]), ch, text(&r[2])));
        }
    }
    Ok(())
}

fn read_civs(db: &SnapDb, input: &mut Input) -> Result<(), WebLobbyError> {
    // The first leader by rowid of `Civilization_Leaders`, as the core's CIV_INFO query takes it.
    let mut leaders: HashMap<String, LeaderIn> = HashMap::new();
    for r in db.query(
        "SELECT cl.CivilizationType, l.Type, l.Description, l.IconAtlas, l.PortraitIndex \
         FROM Civilization_Leaders cl INNER JOIN Leaders l ON l.Type = cl.LeaderheadType \
         ORDER BY cl.rowid",
        5,
    )? {
        leaders.entry(text(&r[0])).or_insert_with(|| LeaderIn {
            ty: text(&r[1]),
            desc: text(&r[2]),
            icon: icon_of(&r[3], &r[4]),
        });
    }
    // Unique units, then buildings (rowid order of the override tables), then improvements.
    let mut uniques: HashMap<String, Vec<UniqueIn>> = HashMap::new();
    // Columns 0 to 4: civ, type, description, atlas, portrait; 5: Help tag; 6: the default item of
    // the class (type) and 7: its Description tag; 8 to 12: moves, combat, ranged, range, cost.
    // The filters are those of the original's `PopulateUniques` (EUI): units shown in the pedia,
    // buildings that are no great-work-less placeholders.
    let queries: [(&'static str, &str); 3] = [
        (
            "Unit",
            "SELECT o.CivilizationType, u.Type, u.Description, u.IconAtlas, u.PortraitIndex, u.Help, \
                    d.Type, d.Description, u.Moves, u.Combat, u.RangedCombat, u.Range, u.Cost \
             FROM Civilization_UnitClassOverrides o INNER JOIN Units u ON u.Type = o.UnitType \
             LEFT JOIN UnitClasses c ON c.Type = u.Class LEFT JOIN Units d ON d.Type = c.DefaultUnit \
             WHERE u.ShowInPedia IS NOT 0 ORDER BY o.rowid",
        ),
        (
            "Building",
            "SELECT o.CivilizationType, b.Type, b.Description, b.IconAtlas, b.PortraitIndex, b.Help, \
                    d.Type, d.Description, NULL, NULL, NULL, NULL, b.Cost \
             FROM Civilization_BuildingClassOverrides o INNER JOIN Buildings b ON b.Type = o.BuildingType \
             LEFT JOIN BuildingClasses c ON c.Type = b.BuildingClass LEFT JOIN Buildings d ON d.Type = c.DefaultBuilding \
             WHERE b.GreatWorkCount IS NOT -1 ORDER BY o.rowid",
        ),
        (
            "Improvement",
            "SELECT CivilizationType, Type, Description, IconAtlas, PortraitIndex, Help, \
                    NULL, NULL, NULL, NULL, NULL, NULL, NULL FROM Improvements \
             WHERE CivilizationType IS NOT NULL AND CivilizationType <> '' ORDER BY rowid",
        ),
    ];
    for (kind, sql) in queries {
        for r in db.query(sql, 13)? {
            let ty = text(&r[1]);
            let replaces = match (&r[6], &r[7]) {
                (Some(d), Some(tag)) if *d != ty && !tag.is_empty() => Some(tag.clone()),
                _ => None,
            };
            uniques.entry(text(&r[0])).or_default().push(UniqueIn {
                kind,
                ty,
                desc: text(&r[2]),
                icon: icon_of(&r[3], &r[4]),
                help: text(&r[5]),
                replaces,
                numbers: Numbers {
                    moves: cell_i64(&r[8]),
                    combat: cell_i64(&r[9]),
                    ranged: cell_i64(&r[10]),
                    range: cell_i64(&r[11]),
                    cost: cell_i64(&r[12]),
                },
            });
        }
    }
    // The civilizations the staging room lists: `Playable = 1`, as its pull-down query (Venice, the
    // barbarians and the city-states have 0).
    for r in db.query(
        "SELECT Type, ShortDescription, Description, IconAtlas, PortraitIndex FROM Civilizations \
         WHERE Playable = 1 ORDER BY ID",
        5,
    )? {
        let ty = text(&r[0]);
        input.civs.push(CivIn {
            short_desc: text(&r[1]),
            desc: text(&r[2]),
            icon: icon_of(&r[3], &r[4]),
            leader: leaders.remove(&ty),
            uniques: uniques.remove(&ty).unwrap_or_default(),
            ty,
        });
    }
    Ok(())
}

// ---- textures ---------------------------------------------------------------------------------

/// Where the textures and icon fonts come from (the UI file system; a fake in the tests).
pub trait Source {
    /// The RGBA8 pixels of the visible texture of a bare file name. `Ok(None)`: no such file.
    fn texture(&self, file: &str) -> Result<Option<Arc<Rgba8Image>>, String>;
    /// The `.ggxml` text of an icon font file stem.
    fn ggxml(&self, stem: &str) -> Option<String>;
}

/// Level 0 of a UI texture as RGBA8.
pub fn blob_to_rgba(blob: &TextureBlob) -> Result<Rgba8Image, String> {
    let (w, h) = (blob.width as usize, blob.height as usize);
    let src = blob.mips.first().ok_or("texture without pixels")?;
    let need = |n: usize| {
        if src.len() < n {
            Err(format!("texture data of {} bytes, need {n}", src.len()))
        } else {
            Ok(())
        }
    };
    let mut out = vec![0u8; w * h * 4];
    match blob.format {
        TextureFormat::Rgba8 => {
            need(w * h * 4)?;
            out.copy_from_slice(&src[..w * h * 4]);
        }
        TextureFormat::R8 => {
            need(w * h)?;
            for (d, &v) in out.chunks_exact_mut(4).zip(src) {
                d.copy_from_slice(&[v, v, v, 255]);
            }
        }
        TextureFormat::Rg8 => {
            need(w * h * 2)?;
            for (d, p) in out.chunks_exact_mut(4).zip(src.chunks_exact(2)) {
                d.copy_from_slice(&[p[0], p[0], p[0], p[1]]);
            }
        }
        TextureFormat::Bc1 | TextureFormat::Bc2 | TextureFormat::Bc3 => {
            let block = blob.format.block_bytes() as usize;
            let (bw, bh) = (w.div_ceil(4), h.div_ceil(4));
            need(bw * bh * block)?;
            let decode: fn(&[u8], &mut [u8], usize) = match blob.format {
                TextureFormat::Bc1 => bcdec_rs::bc1,
                TextureFormat::Bc2 => bcdec_rs::bc2,
                _ => bcdec_rs::bc3,
            };
            for (i, b) in src.chunks_exact(block).take(bw * bh).enumerate() {
                let (x0, y0) = (i % bw * 4, i / bw * 4);
                let mut tmp = [0u8; 64];
                decode(b, &mut tmp, 16);
                for row in 0..4.min(h - y0) {
                    let n = 4.min(w - x0);
                    let d = ((y0 + row) * w + x0) * 4;
                    out[d..d + n * 4].copy_from_slice(&tmp[row * 16..row * 16 + n * 4]);
                }
            }
        }
        f => return Err(format!("texture format {f:?} has no RGBA8 reading")),
    }
    Ok(Rgba8Image::new(blob.width, blob.height, out))
}

/// Decoded textures by lower-case name (`None`: not in the install) and their total bytes.
type TextureCache = (HashMap<String, Option<Arc<Rgba8Image>>>, usize);

/// The UI file system as a [`Source`]; decoded textures are cached up to [`CACHE_LIMIT`].
pub struct VfsSource {
    vfs: UiVfs,
    cache: Mutex<TextureCache>,
}

impl VfsSource {
    pub fn new(vfs: UiVfs) -> Self {
        Self {
            vfs,
            cache: Mutex::new((HashMap::new(), 0)),
        }
    }

    fn load(&self, name: &str) -> Result<Option<Rgba8Image>, String> {
        let Some(e) = self.vfs.lookup(name) else {
            return Ok(None);
        };
        let bytes = self.vfs.read(e).map_err(|e| e.to_string())?;
        // A block dictionary has its index in `<stem>-index.dds`.
        let index = match name.strip_suffix(".dds") {
            Some(stem) => match self.vfs.lookup(&format!("{stem}-index.dds")) {
                Some(i) => Some(self.vfs.read(i).map_err(|e| e.to_string())?),
                None => None,
            },
            None => None,
        };
        let blob = ui_texture(&bytes, index.as_deref()).map_err(|e| e.to_string())?;
        blob_to_rgba(&blob).map(Some)
    }
}

impl Source for VfsSource {
    fn texture(&self, file: &str) -> Result<Option<Arc<Rgba8Image>>, String> {
        let name = bare_name(file).to_ascii_lowercase();
        if let Some(hit) = self.cache.lock().map_err(|e| e.to_string())?.0.get(&name) {
            return Ok(hit.clone());
        }
        let img = self.load(&name)?.map(Arc::new);
        let mut c = self.cache.lock().map_err(|e| e.to_string())?;
        if c.1 > CACHE_LIMIT {
            c.0.clear();
            c.1 = 0;
        }
        c.1 += img.as_ref().map_or(0, |i| i.pixels.len());
        c.0.insert(name, img.clone());
        Ok(img)
    }

    fn ggxml(&self, stem: &str) -> Option<String> {
        let e = self
            .vfs
            .lookup(&format!("{}.ggxml", bare_name(stem).to_ascii_lowercase()))?;
        let b = self.vfs.read(e).ok()?;
        Some(String::from_utf8_lossy(&b).into_owned())
    }
}

// ---- pixels -----------------------------------------------------------------------------------

/// The atlas row to cut an icon of `want` pixels from: that size, else the smallest larger one
/// (downscaled), else the largest smaller one (upscaled).
pub fn pick_row(rows: &[AtlasRow], want: u32) -> Option<&AtlasRow> {
    rows.iter().find(|r| r.icon_size == want).or_else(|| {
        rows.iter()
            .filter(|r| r.icon_size > want)
            .min_by_key(|r| r.icon_size)
            .or_else(|| {
                rows.iter()
                    .filter(|r| r.icon_size < want)
                    .max_by_key(|r| r.icon_size)
            })
    })
}

/// Cell `index` of an atlas texture: column `index % IconsPerRow`, row `index / IconsPerRow`,
/// `IconSize` pixels each. `None` when the cell is outside the atlas grid or the texture.
pub fn crop_cell(tex: &Rgba8Image, row: &AtlasRow, index: u32) -> Option<Rgba8Image> {
    let s = row.icon_size;
    if s == 0 || row.icons_per_row == 0 {
        return None;
    }
    let (col, line) = (index % row.icons_per_row, index / row.icons_per_row);
    if row.icons_per_column != 0 && line >= row.icons_per_column {
        return None;
    }
    let (x0, y0) = (col.checked_mul(s)?, line.checked_mul(s)?);
    if x0.checked_add(s)? > tex.width || y0.checked_add(s)? > tex.height {
        return None;
    }
    let mut out = Vec::with_capacity((s * s * 4) as usize);
    for y in y0..y0 + s {
        let a = ((y * tex.width + x0) * 4) as usize;
        out.extend_from_slice(&tex.pixels[a..a + (s * 4) as usize]);
    }
    Some(Rgba8Image::new(s, s, out))
}

/// Box filter (the area of each output pixel averaged over the source, alpha-weighted so the
/// transparent edge of a round icon does not darken it). Also used to scale up as a last resort.
pub fn resize_box(src: &Rgba8Image, w: u32, h: u32) -> Rgba8Image {
    if (src.width, src.height) == (w, h) {
        return src.clone();
    }
    let (sw, sh) = (f64::from(src.width), f64::from(src.height));
    let (fx, fy) = (sw / f64::from(w), sh / f64::from(h));
    let mut out = vec![0u8; (w * h * 4) as usize];
    for oy in 0..h {
        let (y0, y1) = (f64::from(oy) * fy, f64::from(oy + 1) * fy);
        for ox in 0..w {
            let (x0, x1) = (f64::from(ox) * fx, f64::from(ox + 1) * fx);
            let (mut sum_w, mut sum_a, mut rgb) = (0.0f64, 0.0f64, [0.0f64; 3]);
            let mut y = y0.floor() as u32;
            while f64::from(y) < y1 && y < src.height {
                let wy = (f64::from(y + 1).min(y1) - f64::from(y).max(y0)).max(0.0);
                let mut x = x0.floor() as u32;
                while f64::from(x) < x1 && x < src.width {
                    let wx = (f64::from(x + 1).min(x1) - f64::from(x).max(x0)).max(0.0);
                    let wt = wx * wy;
                    let p = ((y * src.width + x) * 4) as usize;
                    let a = f64::from(src.pixels[p + 3]);
                    sum_w += wt;
                    sum_a += wt * a;
                    for (c, v) in rgb.iter_mut().zip(&src.pixels[p..p + 3]) {
                        *c += wt * a * f64::from(*v);
                    }
                    x += 1;
                }
                y += 1;
            }
            let o = ((oy * w + ox) * 4) as usize;
            if sum_w > 0.0 && sum_a > 0.0 {
                for (d, c) in out[o..o + 3].iter_mut().zip(rgb) {
                    *d = (c / sum_a).round().clamp(0.0, 255.0) as u8;
                }
                out[o + 3] = (sum_a / sum_w).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    Rgba8Image::new(w, h, out)
}

/// Copies `img` into `dst` (`dst_w` pixels wide) at (x, y).
fn blit(dst: &mut [u8], dst_w: u32, img: &Rgba8Image, x: u32, y: u32) {
    for row in 0..img.height {
        let s = (row * img.width * 4) as usize;
        let d = (((y + row) * dst_w + x) * 4) as usize;
        let n = (img.width * 4) as usize;
        dst[d..d + n].copy_from_slice(&img.pixels[s..s + n]);
    }
}

/// A civ strip: the portrait at (0,0), the small icons in [`STRIP_COLS`] columns right of it.
/// Returns the image and the (x, y) of each small icon, in order. The strip is always
/// `LEADER_PX + STRIP_COLS * ICON_PX` wide and at least `LEADER_PX` high.
pub fn compose_strip(
    portrait: Option<&Rgba8Image>,
    smalls: &[Rgba8Image],
) -> (Rgba8Image, Vec<(u32, u32)>) {
    let width = LEADER_PX + STRIP_COLS * ICON_PX;
    let rows = (smalls.len() as u32).div_ceil(STRIP_COLS);
    let height = LEADER_PX.max(rows * ICON_PX);
    let mut px = vec![0u8; (width * height * 4) as usize];
    if let Some(p) = portrait {
        blit(&mut px, width, p, 0, 0);
    }
    let mut at = Vec::new();
    for (i, s) in smalls.iter().enumerate() {
        let i = i as u32;
        let pos = (
            LEADER_PX + i % STRIP_COLS * ICON_PX,
            i / STRIP_COLS * ICON_PX,
        );
        blit(&mut px, width, s, pos.0, pos.1);
        at.push(pos);
    }
    (Rgba8Image::new(width, height, px), at)
}

// ---- PNG --------------------------------------------------------------------------------------

fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (ia, ib, ic) = (i32::from(a), i32::from(b), i32::from(c));
    let p = ia + ib - ic;
    let (pa, pb, pc) = ((p - ia).abs(), (p - ib).abs(), (p - ic).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// One scanline filtered with PNG filter `kind` (0 none, 1 sub, 2 up, 3 average, 4 Paeth).
fn filter_row(kind: u8, row: &[u8], prev: &[u8]) -> Vec<u8> {
    (0..row.len())
        .map(|i| {
            let a = if i >= 4 { row[i - 4] } else { 0 };
            let b = prev[i];
            let c = if i >= 4 { prev[i - 4] } else { 0 };
            let x = row[i];
            match kind {
                0 => x,
                1 => x.wrapping_sub(a),
                2 => x.wrapping_sub(b),
                3 => x.wrapping_sub(((u16::from(a) + u16::from(b)) / 2) as u8),
                _ => x.wrapping_sub(paeth(a, b, c)),
            }
        })
        .collect()
}

/// A PNG (RGBA8, deflate level 9, per-row filter chosen by the smallest sum of absolute values).
pub fn encode_png(img: &Rgba8Image) -> Result<Vec<u8>, String> {
    use std::io::Write as _;
    let (w, h) = (img.width as usize, img.height as usize);
    if w == 0 || h == 0 || img.pixels.len() != w * h * 4 {
        return Err(format!(
            "bad image: {w}x{h} with {} bytes",
            img.pixels.len()
        ));
    }
    let row_len = w * 4;
    let zero = vec![0u8; row_len];
    let mut raw = Vec::with_capacity((row_len + 1) * h);
    for y in 0..h {
        let row = &img.pixels[y * row_len..(y + 1) * row_len];
        let prev = if y == 0 {
            &zero[..]
        } else {
            &img.pixels[(y - 1) * row_len..y * row_len]
        };
        let (kind, best) = (0u8..5)
            .map(|k| (k, filter_row(k, row, prev)))
            .min_by_key(|(_, f)| f.iter().map(|&b| i32::from(b as i8).abs()).sum::<i32>())
            .unwrap_or((0, row.to_vec()));
        raw.push(kind);
        raw.extend_from_slice(&best);
    }
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
    z.write_all(&raw).map_err(|e| e.to_string())?;
    let z = z.finish().map_err(|e| e.to_string())?;
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&img.width.to_be_bytes());
    ihdr.extend_from_slice(&img.height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    png_chunk(&mut out, b"IHDR", &ihdr);
    png_chunk(&mut out, b"IDAT", &z);
    png_chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

// ---- icon fonts -------------------------------------------------------------------------------

/// What an icon font names: the texture its first style draws and the glyph rects by character.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontGlyphs {
    /// Bare, lower-case file name of the texture.
    pub texture: String,
    /// `<textures width height>`: the size the `u`, `v` of the glyphs are in. The engine takes the
    /// uv as `u / width`, so a smaller or larger file (Lekmod's 20x20 resource icons under a
    /// "22x22" font) is sampled whole; [`scale_rect`] maps the rects to the pixels of the file.
    pub size: Option<(u32, u32)>,
    /// `ch` to (u, v, width, height) in pixels; a later glyph of the same `ch` wins.
    pub rects: HashMap<u32, [u32; 4]>,
}

/// A glyph rect given in a texture of size `from` as the rect of the same uv area in a texture of
/// size `to` (edges rounded, at least one pixel).
pub fn scale_rect(r: [u32; 4], from: (u32, u32), to: (u32, u32)) -> [u32; 4] {
    if from == to {
        return r;
    }
    let edge = |v: u32, f: u32, t: u32| (f64::from(v) * f64::from(t) / f64::from(f)).round() as u32;
    let (x0, y0) = (edge(r[0], from.0, to.0), edge(r[1], from.1, to.1));
    let x1 = edge(r[0] + r[2], from.0, to.0).max(x0 + 1);
    let y1 = edge(r[1] + r[3], from.1, to.1).max(y0 + 1);
    [x0, y0, x1 - x0, y1 - y0]
}

fn child<'a, 'i>(n: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

/// Reads a `.ggxml` icon font (the rules of `ui-text`'s `Font::parse` for what is needed here).
pub fn parse_font_glyphs(ggxml: &str) -> Option<FontGlyphs> {
    let doc = roxmltree::Document::parse(ggxml.trim_start_matches('\u{feff}')).ok()?;
    let root = doc.root_element();
    let textures = child(root, "textures")?;
    let list: Vec<(String, String)> = textures
        .children()
        .filter(|c| c.is_element() && c.tag_name().name() == "texture")
        .map(|t| {
            (
                t.attribute("name").unwrap_or("").to_owned(),
                t.attribute("src").unwrap_or("").to_owned(),
            )
        })
        .collect();
    // The first layer of the first style names the texture; else the first texture.
    let layer_tex = child(root, "styles")
        .and_then(|s| {
            s.children()
                .find(|c| c.is_element() && c.tag_name().name() == "style")
        })
        .and_then(|s| child(s, "layers"))
        .and_then(|l| {
            l.children()
                .find(|c| c.is_element() && c.tag_name().name() == "layer")
        })
        .and_then(|l| l.attribute("tex").map(str::to_owned));
    let src = layer_tex
        .and_then(|n| list.iter().rfind(|t| t.0 == n))
        .or(list.first())
        .map(|t| t.1.clone())?;
    let mut rects = HashMap::new();
    for g in child(root, "glyphs")?
        .children()
        .filter(|c| c.is_element() && c.tag_name().name() == "glyph")
    {
        let n = |k: &str| g.attribute(k)?.trim().parse::<u32>().ok();
        if let (Some(ch), Some(u), Some(v), Some(w), Some(h)) =
            (n("ch"), n("u"), n("v"), n("width"), n("height"))
        {
            rects.insert(ch, [u, v, w, h]);
        }
    }
    let dim = |k: &str| textures.attribute(k)?.trim().parse::<u32>().ok();
    Some(FontGlyphs {
        texture: bare_name(&src).to_ascii_lowercase(),
        size: dim("width")
            .zip(dim("height"))
            .filter(|d| d.0 > 0 && d.1 > 0),
        rects,
    })
}

// ---- the JSON document ------------------------------------------------------------------------

/// A rect of one of the PNGs, in pixels of that PNG.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CellOut {
    pub img: String,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LeaderOut {
    #[serde(rename = "type")]
    pub ty: String,
    pub name: String,
    pub cell: Option<CellOut>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UniqueOut {
    pub kind: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub name: String,
    pub cell: Option<CellOut>,
    /// The line "Replaces: <name>" (localized, game markup); absent for an item that replaces
    /// nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replaces: Option<String>,
    /// The numbers lines of the original tooltip in game markup ("Movement: 2[ICON_MOVES]",
    /// "Combat: 26[ICON_STRENGTH]", "Cost: 160[ICON_PRODUCTION]"), in its order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stats: Vec<String>,
    /// The `Help` text, localized, game markup; absent when the row has none.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub help: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CivOut {
    pub name: String,
    pub leader: Option<LeaderOut>,
    pub icon: Option<CellOut>,
    pub uniques: Vec<UniqueOut>,
}

/// A JSON object that keeps the order of its entries (the order of the core's query).
struct Ordered<'a, T>(&'a [(String, T)]);

impl<T: Serialize> Serialize for Ordered<'_, T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(Some(self.0.len()))?;
        for (k, v) in self.0 {
            m.serialize_entry(k, v)?;
        }
        m.end()
    }
}

/// The document of `web:lobby`: `{"version":1,"colors":{..},"icons":{..},"civs":{..}}`.
#[derive(Serialize)]
pub struct LobbyDoc<'a> {
    version: u32,
    /// `COLOR_*` to `[r, g, b, a]`, bytes.
    colors: BTreeMap<String, [u8; 4]>,
    icons: BTreeMap<String, CellOut>,
    civs: Ordered<'a, CivOut>,
}

// ---- the build --------------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub colors: usize,
    pub icons: usize,
    pub icons_dropped: usize,
    pub font_pngs: usize,
    pub civs: usize,
    pub civ_pngs: usize,
    pub civ_portraits: usize,
    pub civ_icons: usize,
    pub uniques: usize,
    pub unique_cells: usize,
    pub leaders: usize,
}

/// A thing the set could not draw: the cell stays `null` in the JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub kind: &'static str,
    pub subject: String,
    pub reason: String,
}

#[derive(Debug)]
pub struct Output {
    /// The bytes of `web:lobby`.
    pub lobby: Vec<u8>,
    /// Id and PNG bytes.
    pub pngs: Vec<(String, Vec<u8>)>,
    pub stats: Stats,
    pub warnings: Vec<Warning>,
}

struct Ctx<'a> {
    input: &'a Input,
    src: &'a dyn Source,
    warnings: Vec<Warning>,
}

impl Ctx<'_> {
    fn warn(&mut self, kind: &'static str, subject: &str, reason: impl Into<String>) {
        self.warnings.push(Warning {
            kind,
            subject: subject.to_owned(),
            reason: reason.into(),
        });
    }

    fn name(&mut self, tag: &str, subject: &str) -> String {
        match self.input.text.get(tag).filter(|t| !t.is_empty()) {
            Some(t) => t.clone(),
            None => {
                self.warn("missing-text", subject, format!("no en_US text for {tag}"));
                tag.to_owned()
            }
        }
    }

    /// A text that may be missing without a warning (an optional label); `fallback` then.
    fn label(&self, tag: &str, fallback: &str) -> String {
        self.input
            .text
            .get(tag)
            .filter(|t| !t.is_empty())
            .cloned()
            .unwrap_or_else(|| fallback.to_owned())
    }

    /// The tooltip lines of a unique item, as `GetHelpTextForUnit` / `GetHelpTextForBuilding`
    /// order them: movement, ranged combat, combat, cost, then "Replaces:" separately.
    ///
    /// SIMPLIFIED(SIMP-exe-1062): the free-promotion bonuses to moves and range, the resource
    /// requirements, the gold or faith purchase price and the other `TXT_KEY_EUI_*` tag lines of
    /// the EUI tooltip are not listed (they need the active player or the whole Lua text
    /// machinery); the lobby shows the numbers of the row and the pre-written help text.
    fn stat_lines(&self, u: &UniqueIn) -> Vec<String> {
        let n = &u.numbers;
        let mut out = Vec::new();
        if u.kind == "Unit" {
            if let Some(m) = n.moves.filter(|m| *m > 0) {
                let l = self.label("TXT_KEY_PEDIA_MOVEMENT_LABEL", "Movement:");
                out.push(format!("{l} {m}[ICON_MOVES]"));
            }
            if let Some(r) = n.ranged.filter(|r| *r > 0) {
                let l = self.label("TXT_KEY_PEDIA_RANGEDCOMBAT_LABEL", "Ranged Combat:");
                out.push(format!(
                    "{l} {r}[ICON_RANGE_STRENGTH]{}",
                    n.range.unwrap_or_default()
                ));
            }
            if let Some(c) = n.combat.filter(|c| *c > 0) {
                let l = self.label("TXT_KEY_PEDIA_COMBAT_LABEL", "Combat:");
                out.push(format!("{l} {c}[ICON_STRENGTH]"));
            }
        }
        // The original lists a production cost only above 1 (wonders and free items have none).
        if let Some(c) = n.cost.filter(|c| *c > 1) {
            let l = self.label("TXT_KEY_PEDIA_COST_LABEL", "Cost:");
            out.push(format!("{l} {c}[ICON_PRODUCTION]"));
        }
        out
    }

    /// The icon of `want` pixels, cut from its atlas; `None` with a warning when it cannot be.
    fn icon(&mut self, icon: &IconRef, want: u32, subject: &str) -> Option<Rgba8Image> {
        let (Some(atlas), Some(index)) = (&icon.atlas, icon.index) else {
            self.warn("no-icon", subject, "the row names no icon");
            return None;
        };
        let Some(rows) = self.input.atlases.get(atlas) else {
            self.warn("no-atlas", subject, format!("atlas {atlas} has no row"));
            return None;
        };
        let Some(row) = pick_row(rows, want).cloned() else {
            self.warn("no-atlas", subject, format!("atlas {atlas} has no size"));
            return None;
        };
        let tex = match self.src.texture(&row.filename) {
            Ok(Some(t)) => t,
            Ok(None) => {
                self.warn(
                    "no-texture",
                    subject,
                    format!("{} (atlas {atlas}) is not in the install", row.filename),
                );
                return None;
            }
            Err(e) => {
                self.warn(
                    "bad-texture",
                    subject,
                    format!("{} (atlas {atlas}): {e}", row.filename),
                );
                return None;
            }
        };
        let Some(cell) = crop_cell(&tex, &row, index) else {
            self.warn(
                "no-cell",
                subject,
                format!(
                    "cell {index} of atlas {atlas} ({}) is outside {} {}x{}",
                    row.icon_size, row.filename, tex.width, tex.height
                ),
            );
            return None;
        };
        Some(resize_box(&cell, want, want))
    }
}

/// Builds the document and the PNGs from the rows and the textures.
/// One icon-font glyph the JSON lists (the tuple the font pass collects before the PNGs are written).
type WantedGlyph<'a> = (&'a str, String, [u32; 4], Option<(u32, u32)>);

pub fn build_blobs(input: &Input, src: &dyn Source) -> Result<Output, WebLobbyError> {
    let mut cx = Ctx {
        input,
        src,
        warnings: Vec::new(),
    };
    let mut stats = Stats::default();
    let mut pngs: Vec<(String, Vec<u8>)> = Vec::new();
    let png = |img: &Rgba8Image| encode_png(img).map_err(WebLobbyError::Other);

    let colors: BTreeMap<String, [u8; 4]> = input
        .colors
        .iter()
        .map(|(n, c)| (n.clone(), color_rgba(*c)))
        .collect();
    stats.colors = colors.len();

    // ---- icon fonts: one PNG per texture an icon needs, numbered by sorted texture name ----
    let mut fonts: HashMap<String, Option<FontGlyphs>> = HashMap::new();
    let mut wanted: Vec<WantedGlyph<'_>> = Vec::new();
    for (name, ch, key) in &input.icons {
        let Some(stem) = input.font_files.get(key) else {
            cx.warn(
                "icon-dropped",
                name,
                format!("texture key {key} has no file"),
            );
            stats.icons_dropped += 1;
            continue;
        };
        let font = fonts
            .entry(stem.to_ascii_lowercase())
            .or_insert_with(|| cx.src.ggxml(stem).and_then(|x| parse_font_glyphs(&x)));
        match font.as_ref().and_then(|f| Some((f, f.rects.get(ch)?))) {
            Some((f, r)) => wanted.push((name.as_str(), f.texture.clone(), *r, f.size)),
            None => {
                cx.warn(
                    "icon-dropped",
                    name,
                    format!("font {stem} has no glyph {ch}"),
                );
                stats.icons_dropped += 1;
            }
        }
    }
    let tex_names: BTreeSet<&str> = wanted.iter().map(|w| w.1.as_str()).collect();
    let mut font_ids: HashMap<String, (String, u32, u32)> = HashMap::new();
    for name in tex_names {
        match cx.src.texture(name) {
            Ok(Some(t)) => {
                let id = font_png_id(font_ids.len());
                pngs.push((id.clone(), png(&t)?));
                font_ids.insert(name.to_owned(), (id, t.width, t.height));
            }
            Ok(None) => cx.warn(
                "no-texture",
                name,
                "icon font texture is not in the install",
            ),
            Err(e) => cx.warn("bad-texture", name, e),
        }
    }
    stats.font_pngs = font_ids.len();
    let mut icons: BTreeMap<String, CellOut> = BTreeMap::new();
    for (name, tex, rect, size) in wanted {
        match font_ids.get(&tex) {
            Some((id, tw, th)) => {
                let [u, v, w, h] = scale_rect(rect, size.unwrap_or((*tw, *th)), (*tw, *th));
                if u + w > *tw || v + h > *th {
                    cx.warn("icon-dropped", name, format!("glyph is outside {tex}"));
                    stats.icons_dropped += 1;
                    continue;
                }
                icons.insert(
                    name.to_owned(),
                    CellOut {
                        img: id.clone(),
                        x: u,
                        y: v,
                        w,
                        h,
                    },
                );
            }
            None => stats.icons_dropped += 1,
        }
    }
    stats.icons = icons.len();

    // ---- civilizations ----
    let mut civs: Vec<(String, CivOut)> = Vec::new();
    for civ in &input.civs {
        let img = civ_png_id(&civ.ty);
        let tag = if civ.short_desc.is_empty() {
            &civ.desc
        } else {
            &civ.short_desc
        };
        let name = cx.name(tag, &civ.ty);
        let portrait = civ
            .leader
            .as_ref()
            .and_then(|l| cx.icon(&l.icon, LEADER_PX, &l.ty));
        let civ_icon = cx.icon(&civ.icon, ICON_PX, &civ.ty);

        // Uniques with a name; an entry whose kind and name repeat one before (the water twin of a
        // civ's improvement) is dropped.
        let mut seen: BTreeSet<(&'static str, String)> = BTreeSet::new();
        let mut uniques: Vec<(&UniqueIn, String, Option<Rgba8Image>)> = Vec::new();
        for u in &civ.uniques {
            let uname = cx.name(&u.desc, &u.ty);
            if !seen.insert((u.kind, uname.clone())) {
                continue;
            }
            let cell = cx.icon(&u.icon, ICON_PX, &u.ty);
            uniques.push((u, uname, cell));
        }

        // Strip slots go to the icons that exist, in order: the civ icon, then the uniques.
        let mut smalls: Vec<Rgba8Image> = Vec::new();
        smalls.extend(civ_icon.iter().cloned());
        smalls.extend(uniques.iter().filter_map(|u| u.2.clone()));
        let (strip, at) = compose_strip(portrait.as_ref(), &smalls);
        let cell_at = |x: u32, y: u32, side: u32| CellOut {
            img: img.clone(),
            x,
            y,
            w: side,
            h: side,
        };
        let mut slots = at.into_iter();
        let icon = civ_icon
            .as_ref()
            .and_then(|_| slots.next())
            .map(|(x, y)| cell_at(x, y, ICON_PX));
        let mut out_uniques = Vec::new();
        for (u, uname, cell) in &uniques {
            let replaces = match &u.replaces {
                Some(tag) => {
                    let l = cx.label("TXT_KEY_PEDIA_REPLACES_LABEL", "Replaces:");
                    Some(format!("{l} {}", cx.name(tag, &u.ty)))
                }
                None => None,
            };
            let help = match cx.input.text.get(&u.help).filter(|t| !t.is_empty()) {
                Some(t) => t.clone(),
                None => {
                    if !u.help.is_empty() {
                        cx.warn(
                            "missing-text",
                            &u.ty,
                            format!("no en_US text for {}", u.help),
                        );
                    }
                    String::new()
                }
            };
            out_uniques.push(UniqueOut {
                kind: u.kind.to_owned(),
                ty: u.ty.clone(),
                name: uname.clone(),
                cell: cell
                    .as_ref()
                    .and_then(|_| slots.next())
                    .map(|(x, y)| cell_at(x, y, ICON_PX)),
                replaces,
                stats: cx.stat_lines(u),
                help,
            });
        }
        let leader = match &civ.leader {
            Some(l) => {
                let lname = cx.name(&l.desc, &l.ty);
                stats.leaders += 1;
                Some(LeaderOut {
                    ty: l.ty.clone(),
                    name: lname,
                    cell: portrait.as_ref().map(|_| cell_at(0, 0, LEADER_PX)),
                })
            }
            None => {
                cx.warn("no-leader", &civ.ty, "Civilization_Leaders has no row");
                None
            }
        };
        if portrait.is_some() || !smalls.is_empty() {
            pngs.push((img.clone(), png(&strip)?));
            stats.civ_pngs += 1;
        }
        stats.civs += 1;
        stats.civ_portraits += usize::from(portrait.is_some());
        stats.civ_icons += usize::from(icon.is_some());
        stats.uniques += out_uniques.len();
        stats.unique_cells += out_uniques.iter().filter(|u| u.cell.is_some()).count();
        civs.push((
            civ.ty.clone(),
            CivOut {
                name,
                leader,
                icon,
                uniques: out_uniques,
            },
        ));
    }

    let doc = LobbyDoc {
        version: DOC_VERSION,
        colors,
        icons,
        civs: Ordered(&civs),
    };
    let lobby = serde_json::to_vec(&doc).map_err(|e| WebLobbyError::Other(e.to_string()))?;
    Ok(Output {
        lobby,
        pngs,
        stats,
        warnings: cx.warnings,
    })
}

// ---- the set ----------------------------------------------------------------------------------

#[derive(Debug)]
pub struct Summary {
    pub stats: Stats,
    pub lobby_bytes: u64,
    pub png_bytes: u64,
    pub png_count: usize,
    pub report: SetReport,
    pub warnings: Vec<Warning>,
    pub seconds: f64,
}

/// Builds the set into `out`: the blobs go in, the manifest of `out` gets the ids on top of the
/// entries it has.
pub fn build(
    game: &Path,
    db_dir: &Path,
    out: &Path,
    opts: &UiOptions,
) -> Result<Summary, WebLobbyError> {
    let started = std::time::Instant::now();
    let src = VfsSource::new(open_vfs(game, opts)?);
    let input = load_input(db_dir)?;
    let built = build_blobs(&input, &src)?;
    let store = BlobStore::open(out)?;
    let mut manifest = ManifestBuilder::open(&store)?;
    let add = |manifest: &mut ManifestBuilder, id: &str, bytes: &[u8]| -> Result<u64, StoreError> {
        let (hash, size) = store.put(bytes)?;
        manifest.add(
            LogicalId::new(id),
            Variant {
                format: VariantFormat::Generic,
                hash,
                size,
                prefetch: PrefetchGroup(0),
            },
        );
        Ok(size)
    };
    let lobby_bytes = add(&mut manifest, LOBBY_ID, &built.lobby)?;
    let mut png_bytes = 0;
    for (id, bytes) in &built.pngs {
        png_bytes += add(&mut manifest, id, bytes)?;
    }
    manifest.finish(&store)?;
    let s = &built.stats;
    let report = SetReport {
        entries: built
            .warnings
            .iter()
            .map(|w| ReportEntry::new(w.kind, w.subject.as_str(), w.reason.as_str()))
            .collect(),
        counts: BTreeMap::from([
            ("colors".to_owned(), s.colors as u64),
            ("icons".to_owned(), s.icons as u64),
            ("icons dropped".to_owned(), s.icons_dropped as u64),
            ("civs".to_owned(), s.civs as u64),
            ("civ portraits".to_owned(), s.civ_portraits as u64),
            ("civ icons".to_owned(), s.civ_icons as u64),
            ("uniques".to_owned(), s.uniques as u64),
            ("unique cells".to_owned(), s.unique_cells as u64),
        ]),
        bytes: BTreeMap::from([
            ("lobby".to_owned(), lobby_bytes),
            ("pngs".to_owned(), png_bytes),
        ]),
    };
    Ok(Summary {
        stats: built.stats,
        lobby_bytes,
        png_bytes,
        png_count: built.pngs.len(),
        report,
        warnings: built.warnings,
        seconds: started.elapsed().as_secs_f64(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atlas(name: &str, size: u32, file: &str, per_row: u32, per_col: u32) -> AtlasRow {
        AtlasRow {
            atlas: name.into(),
            icon_size: size,
            filename: file.into(),
            icons_per_row: per_row,
            icons_per_column: per_col,
        }
    }

    /// An image whose pixel at (x, y) is (x, y, tag, 255).
    fn grid(w: u32, h: u32, tag: u8) -> Rgba8Image {
        let mut px = Vec::new();
        for y in 0..h {
            for x in 0..w {
                px.extend_from_slice(&[x as u8, y as u8, tag, 255]);
            }
        }
        Rgba8Image::new(w, h, px)
    }

    fn flat(w: u32, h: u32, rgba: [u8; 4]) -> Rgba8Image {
        Rgba8Image::new(w, h, rgba.repeat((w * h) as usize))
    }

    #[test]
    fn colors_truncate_like_the_exe() {
        assert_eq!(color_rgba([1.0, 0.0, 0.0, 1.0]), [255, 0, 0, 255]);
        assert_eq!(color_rgba([0.5, 0.5, 0.5, 0.45]), [127, 127, 127, 114]);
        assert_eq!(color_rgba([1.0, 1.0, 1.0, 0.0]), [255, 255, 255, 0]);
        // DB text goes through f64 and then f32, as the core reads a real column.
        assert_eq!(channel_u8("0.1".parse::<f64>().unwrap() as f32), 25);
    }

    #[test]
    fn atlas_size_choice() {
        let rows = vec![
            atlas("A", 256, "a256", 2, 2),
            atlas("A", 64, "a64", 2, 2),
            atlas("A", 80, "a80", 2, 2),
        ];
        assert_eq!(pick_row(&rows, 64).unwrap().filename, "a64");
        // No 128: the next larger one, not the largest.
        assert_eq!(pick_row(&rows, 128).unwrap().filename, "a256");
        // Nothing larger than 300: the largest smaller one.
        assert_eq!(pick_row(&rows, 300).unwrap().filename, "a256");
        assert!(pick_row(&[], 64).is_none());
    }

    #[test]
    fn cell_crop_follows_the_grid() {
        // 3 columns, 2 rows of 4 px cells.
        let tex = grid(12, 8, 7);
        let row = atlas("A", 4, "t", 3, 2);
        let c = crop_cell(&tex, &row, 4).unwrap(); // column 1, row 1
        assert_eq!((c.width, c.height), (4, 4));
        assert_eq!(&c.pixels[..4], &[4, 4, 7, 255]);
        assert_eq!(&c.pixels[c.pixels.len() - 4..], &[7, 7, 7, 255]);
        assert!(crop_cell(&tex, &row, 6).is_none(), "row 2 is outside");
        // A texture smaller than the grid says.
        assert!(crop_cell(&grid(8, 8, 0), &row, 2).is_none());
    }

    #[test]
    fn box_filter_averages_and_keeps_transparent_edges_clean() {
        let mut img = flat(2, 2, [200, 100, 0, 255]);
        // One transparent pixel with a bright color must not tint the result.
        img.pixels[0..4].copy_from_slice(&[255, 255, 255, 0]);
        let r = resize_box(&img, 1, 1);
        assert_eq!(&r.pixels[..], &[200, 100, 0, 191]);
        let up = resize_box(&flat(1, 1, [9, 8, 7, 255]), 2, 2);
        assert_eq!(up.pixels, [9, 8, 7, 255].repeat(4));
        let same = grid(3, 3, 1);
        assert_eq!(resize_box(&same, 3, 3), same);
        // 4 to 2: each output pixel is the mean of a 2x2 block of the red ramp.
        let mut ramp = flat(4, 1, [0, 0, 0, 255]);
        for x in 0..4 {
            ramp.pixels[x * 4] = (x as u8) * 10;
        }
        assert_eq!(
            &resize_box(&ramp, 2, 1).pixels,
            &[5, 0, 0, 255, 25, 0, 0, 255]
        );
    }

    #[test]
    fn strip_layout() {
        let portrait = flat(LEADER_PX, LEADER_PX, [1, 2, 3, 255]);
        let smalls: Vec<Rgba8Image> = (0..5)
            .map(|i| flat(ICON_PX, ICON_PX, [10 + i, 0, 0, 255]))
            .collect();
        let (strip, at) = compose_strip(Some(&portrait), &smalls);
        assert_eq!((strip.width, strip.height), (256, 192));
        assert_eq!(
            at,
            vec![(128, 0), (192, 0), (128, 64), (192, 64), (128, 128)]
        );
        let px = |x: u32, y: u32| {
            let p = ((y * strip.width + x) * 4) as usize;
            [
                strip.pixels[p],
                strip.pixels[p + 1],
                strip.pixels[p + 2],
                strip.pixels[p + 3],
            ]
        };
        assert_eq!(px(0, 0), [1, 2, 3, 255]);
        assert_eq!(px(127, 127), [1, 2, 3, 255]);
        assert_eq!(px(128, 0), [10, 0, 0, 255]);
        assert_eq!(px(192, 64), [13, 0, 0, 255]);
        assert_eq!(px(128, 128), [14, 0, 0, 255]);
        assert_eq!(px(0, 128), [0, 0, 0, 0], "below the portrait is clear");
        // Few icons: the strip keeps the portrait's height.
        let (s, _) = compose_strip(None, &smalls[..2]);
        assert_eq!((s.width, s.height), (256, 128));
    }

    #[test]
    fn glyph_rects_from_a_ggxml() {
        let x = r#"<?xml version="1.0"?>
<glyphgen>
  <textures width="256" height="256">
    <texture name="" src="Some/Dir\FontIcons.DDS" allowcolor="1"/>
  </textures>
  <styles count="1"><style name=""><layers count="1"><layer tex=""/></layers></style></styles>
  <glyphs spacing="0" ascent="0" descent="0">
    <glyph ch="1" u="0" v="0" width="22" height="22" a="0" b="22" c="0" originx="0" originy="22"/>
    <glyph ch="3" u="44" v="22" width="22" height="22" a="0" b="22" c="0" originx="0" originy="22"/>
    <glyph ch="3" u="66" v="22" width="22" height="22" a="0" b="22" c="0" originx="0" originy="22"/>
  </glyphs>
</glyphgen>"#;
        let f = parse_font_glyphs(x).unwrap();
        assert_eq!(f.texture, "fonticons.dds");
        assert_eq!(f.rects[&1], [0, 0, 22, 22]);
        assert_eq!(f.rects[&3], [66, 22, 22, 22], "the later glyph wins");
        assert!(!f.rects.contains_key(&2));
        assert_eq!(f.size, Some((256, 256)));
        assert!(parse_font_glyphs("<glyphgen/>").is_none());
    }

    #[test]
    fn glyph_rects_follow_the_file_size() {
        assert_eq!(
            scale_rect([22, 0, 22, 22], (256, 256), (256, 256)),
            [22, 0, 22, 22]
        );
        // A "22x22" font whose file is 20x20: the glyph covers the whole file.
        assert_eq!(
            scale_rect([0, 0, 22, 22], (22, 22), (20, 20)),
            [0, 0, 20, 20]
        );
        // Half size: the edges halve.
        assert_eq!(
            scale_rect([44, 22, 22, 22], (256, 256), (128, 128)),
            [22, 11, 11, 11]
        );
        // Never empty.
        assert_eq!(
            scale_rect([1, 1, 1, 1], (1000, 1000), (10, 10)),
            [0, 0, 1, 1]
        );
    }

    /// Inflates the IDAT of a PNG and undoes the row filters.
    fn decode_png(png: &[u8]) -> (u32, u32, Vec<u8>) {
        use std::io::Read as _;
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 13, 10, 26, 10]);
        let (mut at, mut idat, mut dims) = (8, Vec::new(), (0, 0));
        while at < png.len() {
            let len = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
            let kind = &png[at + 4..at + 8];
            let data = &png[at + 8..at + 8 + len];
            let crc = u32::from_be_bytes(png[at + 8 + len..at + 12 + len].try_into().unwrap());
            assert_eq!(crc, crc32(&png[at + 4..at + 8 + len]), "chunk crc");
            match kind {
                b"IHDR" => {
                    dims = (
                        u32::from_be_bytes(data[0..4].try_into().unwrap()),
                        u32::from_be_bytes(data[4..8].try_into().unwrap()),
                    );
                    assert_eq!(&data[8..], &[8, 6, 0, 0, 0]);
                }
                b"IDAT" => idat.extend_from_slice(data),
                _ => {}
            }
            at += 12 + len;
        }
        let mut raw = Vec::new();
        flate2::read::ZlibDecoder::new(&idat[..])
            .read_to_end(&mut raw)
            .unwrap();
        let row_len = dims.0 as usize * 4;
        assert_eq!(raw.len(), (row_len + 1) * dims.1 as usize);
        let mut px: Vec<u8> = Vec::new();
        for (y, line) in raw.chunks_exact(row_len + 1).enumerate() {
            for i in 0..row_len {
                let a = if i >= 4 { px[y * row_len + i - 4] } else { 0 };
                let b = if y > 0 { px[(y - 1) * row_len + i] } else { 0 };
                let c = if i >= 4 && y > 0 {
                    px[(y - 1) * row_len + i - 4]
                } else {
                    0
                };
                let x = line[1 + i];
                px.push(match line[0] {
                    0 => x,
                    1 => x.wrapping_add(a),
                    2 => x.wrapping_add(b),
                    3 => x.wrapping_add(((u16::from(a) + u16::from(b)) / 2) as u8),
                    4 => x.wrapping_add(paeth(a, b, c)),
                    k => panic!("filter {k}"),
                });
            }
        }
        (dims.0, dims.1, px)
    }

    #[test]
    fn png_round_trip_with_every_filter() {
        // Noise and ramps, so the encoder picks different filters over the rows.
        let (w, h) = (19u32, 23u32);
        let mut px = Vec::new();
        let mut seed = 12345u32;
        for y in 0..h {
            for x in 0..w {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let noise = (seed >> 24) as u8;
                px.extend_from_slice(&match y % 4 {
                    0 => [noise, noise, noise, 255],
                    1 => [x as u8 * 9, y as u8, 3, 255],
                    2 => [x as u8 * 3, x as u8 * 3, 0, (x * 11) as u8],
                    _ => [200, 100, 50, 25],
                });
            }
        }
        let img = Rgba8Image::new(w, h, px.clone());
        let png = encode_png(&img).unwrap();
        let (dw, dh, back) = decode_png(&png);
        assert_eq!((dw, dh), (w, h));
        assert_eq!(back, px);
        assert!(
            encode_png(&Rgba8Image {
                width: 0,
                height: 1,
                pixels: vec![]
            })
            .is_err()
        );
    }

    struct Fake {
        textures: HashMap<String, Rgba8Image>,
        fonts: HashMap<String, String>,
    }

    impl Source for Fake {
        fn texture(&self, file: &str) -> Result<Option<Arc<Rgba8Image>>, String> {
            Ok(self
                .textures
                .get(&file.to_ascii_lowercase())
                .map(|t| Arc::new(t.clone())))
        }
        fn ggxml(&self, stem: &str) -> Option<String> {
            self.fonts.get(&stem.to_ascii_lowercase()).cloned()
        }
    }

    fn icon(atlas: &str, i: u32) -> IconRef {
        IconRef {
            atlas: Some(atlas.into()),
            index: Some(i),
        }
    }

    fn sample() -> (Input, Fake) {
        let mut input = Input::default();
        input.colors.push(("COLOR_A".into(), [1.0, 0.5, 0.0, 1.0]));
        input.atlases.insert(
            "LEADERS".into(),
            vec![atlas("LEADERS", 128, "leaders.dds", 2, 1)],
        );
        input.atlases.insert(
            "ICONS".into(),
            vec![
                atlas("ICONS", 128, "icons128.dds", 2, 1),
                atlas("ICONS", 256, "icons256.dds", 1, 1),
            ],
        );
        input
            .atlases
            .insert("BLDG".into(), vec![atlas("BLDG", 64, "missing.dds", 1, 1)]);
        input
            .font_files
            .insert("FONT_KEY".into(), "FontIcons".into());
        input.icons.push(("ICON_GOLD".into(), 3, "FONT_KEY".into()));
        input
            .icons
            .push(("ICON_NOGLYPH".into(), 9, "FONT_KEY".into()));
        input.icons.push(("ICON_NOKEY".into(), 1, "NOPE".into()));
        input.civs.push(CivIn {
            ty: "CIVILIZATION_ROME".into(),
            short_desc: "TXT_ROME".into(),
            desc: "TXT_ROME_LONG".into(),
            icon: icon("ICONS", 1),
            leader: Some(LeaderIn {
                ty: "LEADER_AUGUSTUS".into(),
                desc: "TXT_AUGUSTUS".into(),
                icon: icon("LEADERS", 1),
            }),
            uniques: vec![
                UniqueIn {
                    kind: "Unit",
                    ty: "UNIT_LEGION".into(),
                    desc: "TXT_LEGION".into(),
                    icon: icon("ICONS", 0),
                    help: "TXT_LEGION_HELP".into(),
                    replaces: Some("TXT_SWORDSMAN".into()),
                    numbers: Numbers {
                        moves: Some(2),
                        combat: Some(16),
                        ranged: Some(0),
                        range: Some(0),
                        cost: Some(75),
                    },
                },
                UniqueIn {
                    kind: "Building",
                    ty: "BUILDING_BALLISTA".into(),
                    desc: "TXT_BALLISTA".into(),
                    icon: icon("BLDG", 0),
                    help: String::new(),
                    replaces: None,
                    numbers: Numbers {
                        cost: Some(1),
                        ..Numbers::default()
                    },
                },
                UniqueIn {
                    kind: "Unit",
                    ty: "UNIT_LEGION_TWIN".into(),
                    desc: "TXT_LEGION".into(),
                    icon: icon("ICONS", 0),
                    help: String::new(),
                    replaces: None,
                    numbers: Numbers::default(),
                },
                UniqueIn {
                    kind: "Unit",
                    ty: "UNIT_ARCHER".into(),
                    desc: "TXT_ARCHER".into(),
                    icon: icon("ICONS", 1),
                    help: "TXT_NOWHERE".into(),
                    replaces: None,
                    numbers: Numbers {
                        moves: Some(2),
                        combat: Some(0),
                        ranged: Some(9),
                        range: Some(2),
                        cost: Some(40),
                    },
                },
            ],
        });
        input.civs.push(CivIn {
            ty: "CIVILIZATION_ALPHA".into(),
            short_desc: String::new(),
            desc: "TXT_ALPHA".into(),
            icon: IconRef::default(),
            leader: None,
            uniques: vec![],
        });
        for (k, v) in [
            ("TXT_ROME", "Rome"),
            ("TXT_AUGUSTUS", "Augustus Caesar"),
            ("TXT_LEGION", "Legion"),
            ("TXT_BALLISTA", "Ballista"),
            ("TXT_ARCHER", "Archer"),
            ("TXT_SWORDSMAN", "Swordsman"),
            (
                "TXT_LEGION_HELP",
                "Strong [COLOR_POSITIVE_TEXT]melee[ENDCOLOR].",
            ),
            ("TXT_KEY_PEDIA_MOVEMENT_LABEL", "Movement:"),
            ("TXT_KEY_PEDIA_REPLACES_LABEL", "Replaces:"),
            ("TXT_ALPHA", "Alpha"),
        ] {
            input.text.insert(k.into(), v.into());
        }
        let mut textures = HashMap::new();
        // Leaders: two 128 cells; cell 1 is red.
        let mut leaders = flat(256, 128, [0, 0, 255, 255]);
        for y in 0..128 {
            for x in 128..256 {
                let p = ((y * 256 + x) * 4) as usize;
                leaders.pixels[p..p + 4].copy_from_slice(&[255, 0, 0, 255]);
            }
        }
        textures.insert("leaders.dds".to_owned(), leaders);
        // Icons at 128: cell 0 green, cell 1 white (scaled down to 64).
        let mut icons = flat(256, 128, [0, 255, 0, 255]);
        for y in 0..128 {
            for x in 128..256 {
                let p = ((y * 256 + x) * 4) as usize;
                icons.pixels[p..p + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        textures.insert("icons128.dds".to_owned(), icons);
        textures.insert("fonticons.dds".to_owned(), grid(64, 64, 1));
        let ggxml = r#"<glyphgen><textures width="64" height="64"><texture name="" src="FontIcons.dds"/></textures>
            <styles><style name=""><layers><layer tex=""/></layers></style></styles>
            <glyphs><glyph ch="3" u="22" v="22" width="22" height="22"/></glyphs></glyphgen>"#;
        let fonts = HashMap::from([("fonticons".to_owned(), ggxml.to_owned())]);
        (input, Fake { textures, fonts })
    }

    #[test]
    fn json_shape_and_strip_cells() {
        let (input, fake) = sample();
        let out = build_blobs(&input, &fake).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out.lobby).unwrap();
        assert_eq!(v["version"], 1);
        assert_eq!(
            v["colors"]["COLOR_A"],
            serde_json::json!([255, 127, 0, 255])
        );
        assert_eq!(
            v["icons"]["ICON_GOLD"],
            serde_json::json!({"img": "web:png:fonticons-0", "x": 22, "y": 22, "w": 22, "h": 22})
        );
        assert!(v["icons"].get("ICON_NOGLYPH").is_none());
        assert!(v["icons"].get("ICON_NOKEY").is_none());
        assert_eq!(out.stats.icons, 1);
        assert_eq!(out.stats.icons_dropped, 2);

        let rome = &v["civs"]["CIVILIZATION_ROME"];
        assert_eq!(rome["name"], "Rome");
        assert_eq!(rome["leader"]["type"], "LEADER_AUGUSTUS");
        assert_eq!(rome["leader"]["name"], "Augustus Caesar");
        let strip = "web:png:civ-civilization_rome";
        assert_eq!(
            rome["leader"]["cell"],
            serde_json::json!({"img": strip, "x": 0, "y": 0, "w": 128, "h": 128})
        );
        // The civ icon takes slot 0, the Legion slot 1; the Ballista has no texture (null) and
        // takes no slot; the twin Legion is dropped as a repeat.
        assert_eq!(
            rome["icon"],
            serde_json::json!({"img": strip, "x": 128, "y": 0, "w": 64, "h": 64})
        );
        let u = rome["uniques"].as_array().unwrap();
        assert_eq!(u.len(), 3, "{u:?}");
        assert_eq!(u[0]["kind"], "Unit");
        assert_eq!(u[0]["name"], "Legion");
        assert_eq!(u[0]["cell"]["x"], 192);
        assert_eq!(u[1]["kind"], "Building");
        assert!(u[1]["cell"].is_null());
        // What the original tooltip lists: the numbers, the replaced item and the help text.
        assert_eq!(u[0]["replaces"], "Replaces: Swordsman");
        assert_eq!(
            u[0]["stats"],
            serde_json::json!([
                "Movement: 2[ICON_MOVES]",
                "Combat: 16[ICON_STRENGTH]",
                "Cost: 75[ICON_PRODUCTION]"
            ])
        );
        assert_eq!(u[0]["help"], "Strong [COLOR_POSITIVE_TEXT]melee[ENDCOLOR].");
        // A cost of 1 is no cost; no replaced item, no stats and no help: the keys are absent.
        let b = u[1].as_object().unwrap();
        assert!(!b.contains_key("replaces") && !b.contains_key("stats") && !b.contains_key("help"));
        // Ranged: the range follows the strength; a help tag without text is dropped (a warning).
        assert_eq!(
            u[2]["stats"],
            serde_json::json!([
                "Movement: 2[ICON_MOVES]",
                "Ranged Combat: 9[ICON_RANGE_STRENGTH]2",
                "Cost: 40[ICON_PRODUCTION]"
            ])
        );
        assert!(u[2].get("help").is_none());
        assert!(
            out.warnings
                .iter()
                .any(|w| w.kind == "missing-text" && w.subject == "UNIT_ARCHER")
        );

        // Order of the civs is the order of the input, not sorted.
        let keys: Vec<&String> = v["civs"].as_object().unwrap().keys().collect();
        let text = String::from_utf8(out.lobby.clone()).unwrap();
        assert!(text.find("CIVILIZATION_ROME").unwrap() < text.find("CIVILIZATION_ALPHA").unwrap());
        assert_eq!(keys.len(), 2);

        // A civ without a leader or icons: name from Description, nulls, no PNG.
        let alpha = &v["civs"]["CIVILIZATION_ALPHA"];
        assert_eq!(alpha["name"], "Alpha");
        assert!(alpha["leader"].is_null() && alpha["icon"].is_null());
        assert!(alpha["uniques"].as_array().unwrap().is_empty());

        // PNGs: the Rome strip and the font texture.
        let ids: Vec<&str> = out.pngs.iter().map(|p| p.0.as_str()).collect();
        assert_eq!(ids, ["web:png:fonticons-0", strip]);
        let (w, h, px) = decode_png(&out.pngs[1].1);
        assert_eq!((w, h), (256, 128));
        let at = |x: usize, y: usize| px[(y * 256 + x) * 4..(y * 256 + x) * 4 + 4].to_vec();
        assert_eq!(at(10, 10), [255, 0, 0, 255], "portrait is leader cell 1");
        assert_eq!(at(130, 2), [255, 255, 255, 255], "civ icon: ICONS cell 1");
        assert_eq!(at(194, 2), [0, 255, 0, 255], "Legion: ICONS cell 0");
        assert_eq!(out.stats.civ_pngs, 1);
        assert_eq!(out.stats.civ_portraits, 1);
        assert_eq!(out.stats.civ_icons, 1);
        assert_eq!((out.stats.uniques, out.stats.unique_cells), (3, 2));
        assert!(
            out.warnings
                .iter()
                .any(|w| w.kind == "no-texture" && w.subject == "BUILDING_BALLISTA")
        );
    }

    /// The rows of a mini DB: only `Playable = 1` civilizations, and per unique the replaced
    /// class default, the help tag and the numbers.
    #[test]
    fn rows_of_the_playable_civs_with_uniques_details() {
        let db = SnapDb::open_memory().unwrap();
        db.exec(
            "CREATE TABLE Civilizations (ID integer, Type text, ShortDescription text, Description text,
               IconAtlas text, PortraitIndex integer, Playable integer);
             INSERT INTO Civilizations VALUES
               (0,'CIVILIZATION_MINOR','T_MINOR','T_MINOR',NULL,-1,0),
               (1,'CIVILIZATION_SPAIN','T_SPAIN','T_SPAINL','CIV_A',3,1),
               (2,'CIVILIZATION_BARBARIAN','T_BARB','T_BARB',NULL,-1,0),
               (3,'CIVILIZATION_VENICE','T_VENICE','T_VENICE','CIV_A',4,0),
               (4,'CIVILIZATION_ROME','T_ROME','T_ROMEL','CIV_A',5,1);
             CREATE TABLE Leaders (Type text, Description text, IconAtlas text, PortraitIndex integer);
             CREATE TABLE Civilization_Leaders (CivilizationType text, LeaderheadType text);
             CREATE TABLE UnitClasses (Type text, DefaultUnit text);
             INSERT INTO UnitClasses VALUES ('UC_MUSKET','UNIT_MUSKETMAN'), ('UC_UNIQUE','UNIT_SOLO');
             CREATE TABLE Units (Type text, Description text, IconAtlas text, PortraitIndex integer,
               Help text, Class text, Moves integer, Combat integer, RangedCombat integer, Range integer,
               Cost integer, ShowInPedia integer);
             INSERT INTO Units VALUES
               ('UNIT_MUSKETMAN','T_MUSKETMAN','UA',1,NULL,'UC_MUSKET',2,24,0,0,120,1),
               ('UNIT_TERCIO','T_TERCIO','UA',2,'T_TERCIO_HELP','UC_MUSKET',2,26,0,0,160,1),
               ('UNIT_SOLO','T_SOLO','UA',3,'','UC_UNIQUE',2,10,0,0,50,1),
               ('UNIT_HIDDEN','T_HIDDEN','UA',4,'','UC_UNIQUE',2,10,0,0,50,0);
             CREATE TABLE Civilization_UnitClassOverrides (CivilizationType text, UnitClassType text, UnitType text);
             INSERT INTO Civilization_UnitClassOverrides VALUES
               ('CIVILIZATION_SPAIN','UC_MUSKET','UNIT_TERCIO'), ('CIVILIZATION_ROME','UC_UNIQUE','UNIT_SOLO'),
               ('CIVILIZATION_ROME','UC_UNIQUE','UNIT_HIDDEN');
             CREATE TABLE BuildingClasses (Type text, DefaultBuilding text);
             INSERT INTO BuildingClasses VALUES ('BC_CIRCUS','BUILDING_CIRCUS');
             CREATE TABLE Buildings (Type text, Description text, IconAtlas text, PortraitIndex integer,
               Help text, BuildingClass text, Cost integer, GreatWorkCount integer);
             INSERT INTO Buildings VALUES
               ('BUILDING_CIRCUS','T_CIRCUS','BA',1,NULL,'BC_CIRCUS',100,0),
               ('BUILDING_TOROS','T_TOROS','BA',2,'T_TOROS_HELP','BC_CIRCUS',75,0),
               ('BUILDING_HOLDER','T_HOLDER','BA',3,'','BC_CIRCUS',75,-1);
             CREATE TABLE Civilization_BuildingClassOverrides (CivilizationType text, BuildingClassType text, BuildingType text);
             INSERT INTO Civilization_BuildingClassOverrides VALUES
               ('CIVILIZATION_SPAIN','BC_CIRCUS','BUILDING_TOROS'), ('CIVILIZATION_SPAIN','BC_CIRCUS','BUILDING_HOLDER');
             CREATE TABLE Improvements (Type text, Description text, IconAtlas text, PortraitIndex integer,
               Help text, CivilizationType text);
             INSERT INTO Improvements VALUES
               ('IMP_FARM','T_FARM','IA',1,'T_FARM_HELP',NULL), ('IMP_KASBAH','T_KASBAH','IA',2,'T_KASBAH_HELP','CIVILIZATION_ROME');",
        )
        .unwrap();
        let mut input = Input::default();
        read_civs(&db, &mut input).unwrap();
        let types: Vec<&str> = input.civs.iter().map(|c| c.ty.as_str()).collect();
        assert_eq!(types, ["CIVILIZATION_SPAIN", "CIVILIZATION_ROME"]);

        let spain = &input.civs[0].uniques;
        assert_eq!(
            spain.len(),
            2,
            "the great-work-less building is dropped: {spain:?}"
        );
        let tercio = &spain[0];
        assert_eq!((tercio.kind, tercio.ty.as_str()), ("Unit", "UNIT_TERCIO"));
        assert_eq!(tercio.help, "T_TERCIO_HELP");
        assert_eq!(tercio.replaces.as_deref(), Some("T_MUSKETMAN"));
        assert_eq!(
            tercio.numbers,
            Numbers {
                moves: Some(2),
                combat: Some(26),
                ranged: Some(0),
                range: Some(0),
                cost: Some(160)
            }
        );
        let toros = &spain[1];
        assert_eq!(toros.replaces.as_deref(), Some("T_CIRCUS"));
        assert_eq!(toros.numbers.cost, Some(75));
        assert_eq!(toros.numbers.moves, None);

        // An item that is its class default replaces nothing; a hidden unit is not listed.
        let rome = &input.civs[1].uniques;
        let rome_types: Vec<&str> = rome.iter().map(|u| u.ty.as_str()).collect();
        assert_eq!(rome_types, ["UNIT_SOLO", "IMP_KASBAH"]);
        assert_eq!(rome[0].replaces, None);
        assert_eq!(rome[1].kind, "Improvement");
        assert_eq!(rome[1].help, "T_KASBAH_HELP");
    }
}
