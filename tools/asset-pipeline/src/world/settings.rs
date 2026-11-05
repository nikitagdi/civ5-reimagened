//! World settings (`C5WS`): the XML and CSV files of the original to
//! typed fields.
//!
//! Every attribute of a file is either a field or in the section's `extra`; one that is a field but
//! does not parse is an error (AB-D9). Unknown ones are listed by [`WorldSettings::extras`] and
//! reported. The numbers of a colour are kept as written: the five numbers of the `EnemyOutline`
//! colour in Lekmod's `Highlights.xml` are a warning, not an error (RW-20).

use std::collections::BTreeMap;

use asset_format::{
    Arrows, COAST_NONE, CoastLevel, CoastProfile, CoastSettings, CoastStyle, ColorKey,
    CultureBorders, DecalDef, Extra, FluidFow, HexGrid, HighlightKind, HighlightStyle, IceParams,
    IceSettings, LeanInfo, LogicalId, OverlayDefs, Rotation, TerrainDefRow, UnitSelectionHighlight,
    Variant, VariantFormat, WORLD_SETTINGS_ID, WaveSettings, WorldSettings,
};
use roxmltree::{Document, Node};

use crate::game::GameSource;
use crate::game::source::{Entry, Layer, Origin};
use crate::models::ModelError;
use crate::sets::terrain::ReadFile;
use crate::sets::tx_id;
use crate::store::{BlobStore, ManifestBuilder};
use crate::world::groups::BOOT;
use crate::world::lut::lut_files;

pub const FOG_COLORS_FILE: &str = "fogcolor.csv";
pub const FLUID_FOW_FILE: &str = "fluidfowsettings.xml";
pub const HEX_GRID_FILE: &str = "hexgrid.xml";
pub const CULTURE_BORDERS_FILE: &str = "cultureborders.xml";
pub const HIGHLIGHTS_FILE: &str = "highlights.xml";
pub const OVERLAY_DEFS_FILE: &str = "civ5artdefines_overlay.xml";
pub const ARROWS_FILE: &str = "arrowsettings.xml";
pub const TERRAIN_DEF_FILE: &str = "terraindef.csv";
pub const DECALS_FILE: &str = "decal_filenames.xml";

/// Files the settings are read from.
pub const SETTINGS_FILES: [&str; 7] = [
    FOG_COLORS_FILE,
    FLUID_FOW_FILE,
    HEX_GRID_FILE,
    CULTURE_BORDERS_FILE,
    HIGHLIGHTS_FILE,
    OVERLAY_DEFS_FILE,
    ARROWS_FILE,
];

/// Why a file did not parse: the message names the file and the attribute.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{file}: {msg}")]
pub struct SettingsError {
    pub file: String,
    pub msg: String,
}

type Res<T> = Result<T, SettingsError>;

fn err<T>(file: &str, msg: impl Into<String>) -> Res<T> {
    Err(SettingsError {
        file: file.to_owned(),
        msg: msg.into(),
    })
}

// ---- values --------------------------------------------------------------------------------

/// A number as the original's files write it: `.25`, `50.0f`, `1`.
fn number(file: &str, what: &str, text: &str) -> Res<f32> {
    let t = text.trim().trim_end_matches(['f', 'F']);
    match t.parse::<f32>() {
        Ok(v) if v.is_finite() => Ok(v),
        _ => err(file, format!("{what}: {text:?} is not a number")),
    }
}

/// `255,128,0,200`: byte values separated by commas, as many as the file has.
fn color(file: &str, what: &str, text: &str) -> Res<Vec<u8>> {
    text.split(',')
        .map(|p| {
            p.trim().parse::<u8>().or_else(|_| {
                err(
                    file,
                    format!("{what}: {p:?} is not a byte value in {text:?}"),
                )
            })
        })
        .collect()
}

fn boolean(file: &str, what: &str, text: &str) -> Res<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => err(file, format!("{what}: {text:?} is not true or false")),
    }
}

/// The attributes of one element, taken one by one; what is left is the `extra`.
struct Attrs<'f> {
    file: &'f str,
    element: String,
    left: BTreeMap<String, String>,
}

impl<'f> Attrs<'f> {
    fn of(file: &'f str, node: Node<'_, '_>) -> Self {
        Attrs {
            file,
            element: node.tag_name().name().to_owned(),
            left: node
                .attributes()
                .map(|a| (a.name().to_owned(), a.value().to_owned()))
                .collect(),
        }
    }

    fn what(&self, name: &str) -> String {
        format!("{}@{name}", self.element)
    }

    fn text(&mut self, name: &str) -> Res<String> {
        match self.left.remove(name) {
            Some(v) => Ok(v),
            None => err(self.file, format!("{} is missing", self.what(name))),
        }
    }

    fn opt_text(&mut self, name: &str) -> Option<String> {
        self.left.remove(name)
    }

    fn f32(&mut self, name: &str) -> Res<f32> {
        let t = self.text(name)?;
        number(self.file, &self.what(name), &t)
    }

    fn opt_f32(&mut self, name: &str) -> Res<Option<f32>> {
        self.opt_text(name)
            .map(|t| number(self.file, &self.what(name), &t))
            .transpose()
    }

    fn color(&mut self, name: &str) -> Res<Vec<u8>> {
        let t = self.text(name)?;
        color(self.file, &self.what(name), &t)
    }

    fn opt_color(&mut self, name: &str) -> Res<Vec<u8>> {
        self.opt_text(name)
            .map(|t| color(self.file, &self.what(name), &t))
            .transpose()
            .map(Option::unwrap_or_default)
    }

    fn extra(self) -> Extra {
        self.left
    }
}

fn parse_xml<'a>(file: &str, text: &'a str, root: &str) -> Res<Document<'a>> {
    let text = text.trim_start_matches('\u{feff}');
    let doc = Document::parse(text).or_else(|e| err(file, format!("not XML: {e}")))?;
    if doc.root_element().tag_name().name() != root {
        return err(
            file,
            format!(
                "the root element is <{}>, expected <{root}>",
                doc.root_element().tag_name().name()
            ),
        );
    }
    Ok(doc)
}

/// `text` with every `-` inside an XML comment turned into `_`, so a comment with `--` in it (not
/// well-formed, but read by the game: the copies in `MiscData.fpk`, the ones the exe uses, have
/// such comments) does not stop a strict parser. Nothing outside comments
/// changes.
fn lenient_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some((before, after)) = rest.split_once("<!--") {
        out.push_str(before);
        out.push_str("<!--");
        let (body, tail) = match after.split_once("-->") {
            Some((b, t)) => (b, Some(t)),
            None => (after, None),
        };
        out.push_str(&body.replace('-', "_"));
        match tail {
            Some(t) => {
                out.push_str("-->");
                rest = t;
            }
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

fn utf8<'a>(file: &str, bytes: &'a [u8]) -> Res<&'a str> {
    std::str::from_utf8(bytes).or_else(|e| err(file, format!("not UTF-8: {e}")))
}

/// Text of an element, trimmed.
fn node_text(node: Node<'_, '_>) -> String {
    node.text().unwrap_or("").trim().to_owned()
}

// ---- files ---------------------------------------------------------------------------------

/// `fogcolor.csv`: `r,g,b` per line.
pub fn parse_fog_colors(bytes: &[u8]) -> Res<Vec<[u8; 3]>> {
    let text = utf8(FOG_COLORS_FILE, bytes)?;
    let mut rows = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim_start_matches('\u{feff}').trim();
        if line.is_empty() {
            continue;
        }
        let c = color(FOG_COLORS_FILE, &format!("line {}", i + 1), line)?;
        let Ok(rgb) = <[u8; 3]>::try_from(c.as_slice()) else {
            return err(
                FOG_COLORS_FILE,
                format!("line {} has {} numbers, expected 3", i + 1, c.len()),
            );
        };
        rows.push(rgb);
    }
    if rows.is_empty() {
        return err(FOG_COLORS_FILE, "no colour");
    }
    Ok(rows)
}

/// `fluidfowsettings.xml`: one number per child element.
pub fn parse_fluid_fow(bytes: &[u8]) -> Res<FluidFow> {
    let file = FLUID_FOW_FILE;
    let text = lenient_comments(utf8(file, bytes)?);
    let doc = parse_xml(file, &text, "FluidFOWSettings")?;
    let mut left: BTreeMap<String, String> = doc
        .root_element()
        .children()
        .filter(Node::is_element)
        .map(|n| (n.tag_name().name().to_owned(), node_text(n)))
        .collect();
    let mut take = |name: &str| -> Res<f32> {
        match left.remove(name) {
            Some(t) => number(file, name, &t),
            None => err(file, format!("<{name}> is missing")),
        }
    };
    let mut f = FluidFow {
        spawn_frequency: take("SpawnFrequency")?,
        size_mean: take("SizeMean")?,
        size_min: take("SizeMin")?,
        size_max: take("SizeMax")?,
        size_deviation: take("SizeDeviation")?,
        mass_mean: take("MassMean")?,
        mass_min: take("MassMin")?,
        mass_max: take("MassMax")?,
        mass_deviation: take("MassDeviation")?,
        mass_and_size_correlation: take("MassAndSizeCorrelation")?,
        height_variation: take("HeightVariation")?,
        fluid_density: take("FluidDensity")?,
        speed: take("Speed")?,
        drag: take("Drag")?,
        master_size: take("MasterSize")?,
        force_strength: take("ForceStrength")?,
        max_rotate: take("MaxRotate")?,
        fog_height: take("FogHeight")?,
        extra: Extra::new(),
    };
    f.extra = left;
    Ok(f)
}

/// `HexGrid.xml`.
pub fn parse_hex_grid(bytes: &[u8]) -> Res<HexGrid> {
    let file = HEX_GRID_FILE;
    let text = lenient_comments(utf8(file, bytes)?);
    let doc = parse_xml(file, &text, "HexGrid")?;
    let mut a = Attrs::of(file, doc.root_element());
    Ok(HexGrid {
        color: a.color("color")?,
        width: a.f32("width")?,
        extra: a.extra(),
    })
}

/// `CultureBorders.xml`.
pub fn parse_culture_borders(bytes: &[u8]) -> Res<CultureBorders> {
    let file = CULTURE_BORDERS_FILE;
    let text = lenient_comments(utf8(file, bytes)?);
    let doc = parse_xml(file, &text, "CultureBorders")?;
    let mut a = Attrs::of(file, doc.root_element());
    let mut c = CultureBorders {
        primary_width: a.f32("primary_width")?,
        secondary_width: a.f32("secondary_width")?,
        primary_aa_width: a.f32("primary_aa_width")?,
        secondary_aa_width: a.f32("secondary_aa_width")?,
        primary_alpha: a.f32("primary_alpha")?,
        secondary_alpha: a.f32("secondary_alpha")?,
        corner_bias: a.f32("corner_bias")?,
        cs_primary_width: a.f32("cs_primary_width")?,
        cs_outline_width: a.f32("cs_outline_width")?,
        cs_aa_width: a.f32("cs_aa_width")?,
        cs_primary_alpha: a.f32("cs_primary_alpha")?,
        cs_outline_alpha: a.f32("cs_outline_alpha")?,
        cs_dash_width: a.f32("cs_dash_width")?,
        randomness: a.f32("randomness")?,
        expansion: a.f32("expansion")?,
        extra: Extra::new(),
    };
    c.extra = a.extra();
    Ok(c)
}

fn highlight_kind(text: &str) -> Option<HighlightKind> {
    Some(match text {
        "HexContour" => HighlightKind::HexContour,
        "FilledHex" => HighlightKind::FilledHex,
        "StaticTexture" => HighlightKind::StaticTexture,
        "HexModel" => HighlightKind::HexModel,
        "SplineBorder" => HighlightKind::SplineBorder,
        _ => return None,
    })
}

/// `Highlights.xml`: the styles in file order. A colour that does not have 3 or 4 numbers is a
/// warning (the `EnemyOutline` style of Lekmod has 5); so is an element that is not a `<style>`.
pub fn parse_highlights(bytes: &[u8], warnings: &mut Vec<String>) -> Res<Vec<HighlightStyle>> {
    let file = HIGHLIGHTS_FILE;
    let text = lenient_comments(utf8(file, bytes)?);
    let doc = parse_xml(file, &text, "Highlights")?;
    let mut styles = Vec::new();
    for node in doc.root_element().children().filter(Node::is_element) {
        if node.tag_name().name() != "style" {
            warnings.push(format!(
                "{file}: unexpected element <{}> ignored",
                node.tag_name().name()
            ));
            continue;
        }
        let mut a = Attrs::of(file, node);
        let name = a.text("name")?;
        let kind_text = a.text("type")?;
        let Some(kind) = highlight_kind(&kind_text) else {
            return err(file, format!("style {name:?}: unknown type {kind_text:?}"));
        };
        let colour = a.opt_color("color")?;
        if !colour.is_empty() && !matches!(colour.len(), 3 | 4) {
            warnings.push(format!(
                "{file}: style {name:?}: colour has {} numbers, 3 or 4 expected (kept as written)",
                colour.len()
            ));
        }
        styles.push(HighlightStyle {
            width: a.opt_f32("width")?,
            texture: a.opt_text("texture").map(|t| tx_id(t.trim())),
            model: a.opt_text("model").map(|m| m.trim().to_owned()),
            color: colour,
            kind,
            name,
            extra: a.extra(),
        });
    }
    if styles.is_empty() {
        return err(file, "no <style>");
    }
    Ok(styles)
}

/// `Civ5ArtDefines_Overlay.xml`.
pub fn parse_overlay_defs(bytes: &[u8]) -> Res<OverlayDefs> {
    let file = OVERLAY_DEFS_FILE;
    let text = lenient_comments(utf8(file, bytes)?);
    let doc = parse_xml(file, &text, "Overlays")?;
    let mut selection = None;
    let mut extra = Extra::new();
    for node in doc.root_element().children().filter(Node::is_element) {
        match node.tag_name().name() {
            "UnitSelectionHighlight" => selection = Some(parse_selection(file, node)?),
            other => {
                extra.insert(other.to_owned(), node_text(node));
            }
        }
    }
    let Some(unit_selection_highlight) = selection else {
        return err(file, "<UnitSelectionHighlight> is missing");
    };
    Ok(OverlayDefs {
        unit_selection_highlight,
        extra,
    })
}

fn parse_selection(file: &str, node: Node<'_, '_>) -> Res<UnitSelectionHighlight> {
    let mut left: BTreeMap<String, Node<'_, '_>> = node
        .children()
        .filter(Node::is_element)
        .map(|n| (n.tag_name().name().to_owned(), n))
        .collect();
    let mut child = |name: &str| -> Res<Node<'_, '_>> {
        left.remove(name)
            .map_or_else(|| err(file, format!("<{name}> is missing")), Ok)
    };
    let mut extra = Extra::new();

    let mut c = Attrs::of(file, child("Color")?);
    let rgba = [
        c.f32("red")?,
        c.f32("green")?,
        c.f32("blue")?,
        c.f32("alpha")?,
    ];
    extra.extend(
        c.extra()
            .into_iter()
            .map(|(k, v)| (format!("Color@{k}"), v)),
    );

    let pulse = boolean(file, "Pulse", &node_text(child("Pulse")?))?;
    let pulse_speed = number(file, "PulseSpeed", &node_text(child("PulseSpeed")?))?;
    // The file's comment: "off" and "clockwise" are read, anything else is counter-clockwise.
    let rotation = match node_text(child("Rotation")?).to_ascii_lowercase().as_str() {
        "off" => Rotation::Off,
        "clockwise" => Rotation::Clockwise,
        _ => Rotation::CounterClockwise,
    };
    let rotation_speed = number(file, "RotationSpeed", &node_text(child("RotationSpeed")?))?;
    extra.extend(left.into_iter().map(|(k, n)| (k, node_text(n))));
    Ok(UnitSelectionHighlight {
        color: rgba,
        pulse,
        pulse_speed,
        rotation,
        rotation_speed,
        extra,
    })
}

/// `ArrowSettings.xml`.
pub fn parse_arrows(bytes: &[u8]) -> Res<Arrows> {
    let file = ARROWS_FILE;
    let text = lenient_comments(utf8(file, bytes)?);
    let doc = parse_xml(file, &text, "Arrow")?;
    let mut a = Attrs::of(file, doc.root_element());
    let mut r = Arrows {
        min_width: a.f32("min_width")?,
        max_width: a.f32("max_width")?,
        width2: a.f32("width2")?,
        arc_height: a.f32("arc_height")?,
        outline_width: a.f32("outline_width")?,
        outline_width2: a.f32("outline_width2")?,
        body_color: a.color("body_color")?,
        outline_color: a.color("outline_color")?,
        alpha: a.f32("alpha")?,
        target_height: a.f32("target_height")?,
        anim_speed: a.f32("anim_speed")?,
        ambient_light_level: a.f32("ambient_light_level")?,
        extra: Extra::new(),
    };
    r.extra = a.extra();
    Ok(r)
}

/// The colour-key table as the file list gives it, until RW-4 says what `ColorKeyData[5][5]` means:
/// `terrainshadow_europe_colorkey.tga` is system `terrainshadow`, kind `europe`; a file without a
/// kind (`terraincolorkey.tga`) has an empty one.
pub fn color_keys() -> Vec<ColorKey> {
    lut_files()
        .into_iter()
        .map(|file| {
            let stem = file.strip_suffix(".tga").unwrap_or(&file);
            let stem = stem.strip_suffix("_colorkey").unwrap_or(stem);
            let (system, kind) = stem.split_once('_').unwrap_or((stem, ""));
            ColorKey {
                system: system.to_owned(),
                kind: kind.to_owned(),
                file: tx_id(&file),
            }
        })
        .collect()
}

// ---- terrain -------------------------------------------------------------------------------

/// A path of `decal_filenames.xml` (`assets/Decals/Trees/Tree_Shadow_2.dds`) to the id of the file:
/// the install is flat by name.
fn decal_texture(path: &str) -> LogicalId {
    let name = path.trim().rsplit(['/', '\\']).next().unwrap_or(path);
    tx_id(name)
}

/// `decal_filenames.xml`: `<Floodplains><Floodplains_01 albedo=... heightmap=... priority=.../>`.
pub fn parse_decals(bytes: &[u8]) -> Res<Vec<DecalDef>> {
    let file = DECALS_FILE;
    let text = lenient_comments(utf8(file, bytes)?);
    let doc = parse_xml(file, &text, "DecalTypes")?;
    let mut out = Vec::new();
    for kind in doc.root_element().children().filter(Node::is_element) {
        let kind_name = kind.tag_name().name().to_owned();
        if let Some(a) = kind.attributes().next() {
            return err(
                file,
                format!(
                    "<{kind_name}> has the attribute {:?}, expected only elements",
                    a.name()
                ),
            );
        }
        let mut any = false;
        for def in kind.children().filter(Node::is_element) {
            any = true;
            let mut a = Attrs::of(file, def);
            out.push(DecalDef {
                kind: kind_name.clone(),
                name: def.tag_name().name().to_owned(),
                albedo: a.opt_text("albedo").map(|p| decal_texture(&p)),
                heightmap: a.opt_text("heightmap").map(|p| decal_texture(&p)),
                priority: a.opt_f32("priority")?,
                extra: a.extra(),
            });
        }
        if !any {
            return err(file, format!("<{kind_name}> has no decal element"));
        }
    }
    if out.is_empty() {
        return err(file, "no decal");
    }
    Ok(out)
}

/// `terraindef.csv`: a header line, then `continent,diffuse,heightmap,specmap,dominance,scale,
/// projection,id`. `NULL` for the specmap is `None`. The columns of every row are checked.
pub fn parse_terrain_defs(bytes: &[u8]) -> Res<Vec<TerrainDefRow>> {
    let file = TERRAIN_DEF_FILE;
    let text = utf8(file, bytes)?.trim_start_matches('\u{feff}');
    let mut lines = text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty());
    let Some((_, header)) = lines.next() else {
        return err(file, "empty");
    };
    if !header
        .trim_start()
        .to_ascii_lowercase()
        .starts_with("continent id")
    {
        return err(
            file,
            format!("the header line is {header:?}, expected `Continent ID,...`"),
        );
    }
    let mut rows = Vec::new();
    for (i, line) in lines {
        let at = format!("line {}", i + 1);
        let c: Vec<&str> = line.split(',').map(str::trim).collect();
        if c.len() != 8 {
            return err(file, format!("{at}: {} columns, 8 expected", c.len()));
        }
        let int = |what: &str, s: &str| -> Res<u32> {
            s.parse::<u32>()
                .or_else(|_| err(file, format!("{at}: {what} {s:?} is not a whole number")))
        };
        let small = |what: &str, s: &str, max: u32| -> Res<u8> {
            let v = int(what, s)?;
            if v > max {
                return err(file, format!("{at}: {what} {v} is above {max}"));
            }
            Ok(v as u8)
        };
        // `NULL` (or nothing) is no texture. The packed copy of the file has it for the diffuse
        // and height map of its first row.
        let name = |s: &str| -> Option<LogicalId> {
            (!s.is_empty() && !s.eq_ignore_ascii_case("null")).then(|| tx_id(s))
        };
        rows.push(TerrainDefRow {
            continent: small("continent", c[0], 4)?,
            diffuse: name(c[1]),
            heightmap: name(c[2]),
            specmap: name(c[3]),
            base_dominance: small("BaseDominance", c[4], 1)?,
            texture_scale: number(file, &format!("{at}: Texture Scale"), c[5])?,
            projection: small("Projection", c[6], 1)?,
            id: int("ID", c[7])?,
        });
    }
    if rows.is_empty() {
        return err(file, "no row");
    }
    Ok(rows)
}

/// Which copy of `terraindef.csv` the build takes. The install has two:
/// `Assets/DLC/Shared/Terrain/TerrainDef.csv` (loose) and the one in `TerrainModels.fpk`. They
/// differ by a few bytes. Decided by the exe (RW-9c, `civ5-re/build/b2/rw-9bc-answers.txt`): a loose
/// file of a content package (Upgrade1 = `Shared`) beats every root file, so the loose copy is the
/// one in use, and `GameSource` layers the same way. The other copy is kept in
/// `GameSource::shadowed()` and listed by [`terrain_def_note`].
pub const TERRAIN_DEF_COPY: TerrainDefCopy = TerrainDefCopy::DlcShared;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerrainDefCopy {
    /// The loose file under `Assets/DLC/Shared`.
    DlcShared,
    /// The copy packed in `TerrainModels.fpk`.
    TerrainModelsPack,
}

impl TerrainDefCopy {
    fn is(self, game: &GameSource, e: &Entry) -> bool {
        let path = game.origin_path(e).unwrap_or_default();
        let has = |s: &str| {
            path.components()
                .any(|c| c.as_os_str().eq_ignore_ascii_case(s))
        };
        match (self, &e.origin) {
            (Self::DlcShared, Origin::Loose { .. }) => e.layer == Layer::Dlc && has("shared"),
            (Self::TerrainModelsPack, Origin::Packed { .. }) => path
                .file_name()
                .is_some_and(|n| n.eq_ignore_ascii_case("TerrainModels.fpk")),
            _ => false,
        }
    }
}

/// The entry of the chosen copy of `terraindef.csv`; the other copies stay in `shadowed()`.
pub fn terrain_def_entry(game: &GameSource, copy: TerrainDefCopy) -> Result<&Entry, ModelError> {
    game.copies(TERRAIN_DEF_FILE)
        .into_iter()
        .find(|e| copy.is(game, e))
        .ok_or_else(|| {
            ModelError::invalid(format!(
                "{TERRAIN_DEF_FILE}: the {copy:?} copy is not in the install"
            ))
        })
}

/// The rows of the chosen copy.
pub fn read_terrain_defs(game: &GameSource) -> Result<Vec<TerrainDefRow>, ModelError> {
    let e = terrain_def_entry(game, TERRAIN_DEF_COPY)?;
    parsed(parse_terrain_defs(&game.read(e)?))
}

/// Which copy of `terraindef.csv` is taken and how the others differ, row by (continent, id).
pub fn terrain_def_note(game: &GameSource) -> Result<String, ModelError> {
    let show = |e: &Entry| {
        let path = game.origin_path(e).unwrap_or_default();
        let shown = path
            .strip_prefix(game.root())
            .unwrap_or(&path)
            .display()
            .to_string();
        format!("{:?} {shown} ({} B)", e.layer, e.size())
    };
    let taken = terrain_def_entry(game, TERRAIN_DEF_COPY)?;
    let taken_rows = parsed(parse_terrain_defs(&game.read(taken)?))?;
    let mut note = format!(
        "{TERRAIN_DEF_FILE}: taken {} with {} rows [switch: TERRAIN_DEF_COPY = {TERRAIN_DEF_COPY:?}]",
        show(taken),
        taken_rows.len()
    );
    for other in game
        .copies(TERRAIN_DEF_FILE)
        .into_iter()
        .filter(|e| *e != taken)
    {
        let rows = parsed(parse_terrain_defs(&game.read(other)?))?;
        fn key(r: &TerrainDefRow) -> (u8, u32) {
            (r.continent, r.id)
        }
        fn find(from: &[TerrainDefRow], k: (u8, u32)) -> Option<&TerrainDefRow> {
            from.iter().find(|r| key(r) == k)
        }
        let mut only_taken = Vec::new();
        let mut changed = Vec::new();
        for r in &taken_rows {
            match find(&rows, key(r)) {
                None => only_taken.push(format!("{}/{}", r.continent, r.id)),
                Some(o) if o != r => changed.push(format!("{}/{}", r.continent, r.id)),
                Some(_) => {}
            }
        }
        let only_other: Vec<String> = rows
            .iter()
            .filter(|r| find(&taken_rows, key(r)).is_none())
            .map(|r| format!("{}/{}", r.continent, r.id))
            .collect();
        let list = |v: &[String]| {
            if v.is_empty() {
                "none".to_owned()
            } else {
                v.join(", ")
            }
        };
        note.push_str(&format!(
            "; other copy {} with {} rows (only in the taken copy: {}; only in the other: {}; changed in the other: {})",
            show(other),
            rows.len(),
            list(&only_taken),
            list(&only_other),
            list(&changed)
        ));
    }
    Ok(note)
}

// ---- water ---------------------------------------------------------------------------------

pub const COAST_STYLES_FILE: &str = "coaststyles.xml";
pub const COAST_SPLINE_FILE: &str = "coastspline01.txt";
pub const WAVES_FILE: &str = "wavesettings.xml";
pub const ICE_FILE: &str = "icesettings.xml";
pub const ICE_LOW_FILE: &str = "icesettings_low.xml";

fn byte(file: &str, what: &str, text: &str) -> Res<u8> {
    text.trim().parse::<u8>().or_else(|_| {
        err(
            file,
            format!("{what}: {text:?} is not a whole number from 0 to 255"),
        )
    })
}

/// A whole number up to `max` (the counts and indices of the water files).
fn whole(file: &str, what: &str, text: &str, max: u32) -> Res<u32> {
    match text.trim().parse::<u32>() {
        Ok(v) if v <= max => Ok(v),
        _ => err(
            file,
            format!("{what}: {text:?} is not a whole number up to {max}"),
        ),
    }
}

impl Attrs<'_> {
    fn whole(&mut self, name: &str, max: u32) -> Res<u32> {
        let t = self.text(name)?;
        whole(self.file, &self.what(name), &t, max)
    }

    fn byte(&mut self, name: &str) -> Res<u8> {
        let t = self.text(name)?;
        byte(self.file, &self.what(name), &t)
    }

    fn bool(&mut self, name: &str) -> Res<bool> {
        Ok(self.f32(name)? != 0.0)
    }

    fn rgb(&mut self, r: &str, g: &str, b: &str) -> Res<[u8; 3]> {
        Ok([self.byte(r)?, self.byte(g)?, self.byte(b)?])
    }
}

/// A coast spline file: `[x,y,0]` per line. The third coordinate is checked to be 0 and dropped
///.
pub fn parse_spline(file: &str, bytes: &[u8]) -> Res<Vec<[f32; 2]>> {
    let text = utf8(file, bytes)?.trim_start_matches('\u{feff}');
    let mut points = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let at = format!("line {}", i + 1);
        let Some(inner) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) else {
            return err(file, format!("{at}: {line:?} is not [x,y,z]"));
        };
        let parts: Vec<&str> = inner.split(',').collect();
        let [x, y, z] = parts[..] else {
            return err(file, format!("{at}: {} numbers, expected 3", parts.len()));
        };
        if number(file, &format!("{at} z"), z)? != 0.0 {
            return err(file, format!("{at}: the third coordinate {z:?} is not 0"));
        }
        points.push([
            number(file, &format!("{at} x"), x)?,
            number(file, &format!("{at} y"), y)?,
        ]);
    }
    if points.is_empty() {
        return err(file, "no point");
    }
    Ok(points)
}

/// What `coaststyles.xml` says: the file names of the character splines in the order of the list,
/// and the styles. The splines are read by [`build_coast`].
pub fn parse_coast_xml(bytes: &[u8]) -> Res<(Vec<String>, Vec<CoastStyle>)> {
    let file = COAST_STYLES_FILE;
    let text = lenient_comments(utf8(file, bytes)?);
    let doc = parse_xml(file, &text, "CoastRoot")?;
    let list = |name: &str| -> Res<Node<'_, '_>> {
        doc.root_element()
            .children()
            .find(|n| n.is_element() && n.tag_name().name() == name)
            .map_or_else(|| err(file, format!("<{name}> is missing")), Ok)
    };
    let mut profiles = Vec::new();
    for c in list("SplineCharacterList")?
        .children()
        .filter(Node::is_element)
    {
        if c.tag_name().name() != "SplineCharacter" {
            return err(
                file,
                format!("<{}> in the spline list", c.tag_name().name()),
            );
        }
        let mut a = Attrs::of(file, c);
        profiles.push(a.text("Filename")?);
        if let Some(k) = a.extra().keys().next() {
            return err(file, format!("SplineCharacter has the attribute {k:?}"));
        }
    }
    let mut styles = Vec::new();
    for s in list("CoastStyleList")?.children().filter(Node::is_element) {
        if s.tag_name().name() != "CoastStyle" {
            return err(file, format!("<{}> in the style list", s.tag_name().name()));
        }
        let mut a = Attrs::of(file, s);
        let name = a.text("type")?;
        let mut style = CoastStyle {
            name: name.clone(),
            ..CoastStyle::default()
        };
        style.extra = a.extra();
        let mut seen: BTreeMap<String, ()> = BTreeMap::new();
        for child in s.children().filter(Node::is_element) {
            let tag = child.tag_name().name().to_owned();
            if seen.insert(tag.clone(), ()).is_some() {
                return err(file, format!("style {name}: <{tag}> twice"));
            }
            let mut a = Attrs::of(file, child);
            let level = |a: &mut Attrs<'_>, terrain: bool| -> Res<CoastLevel> {
                Ok(CoastLevel {
                    height: a.f32("Height")?,
                    offset: a.f32("Offset")?,
                    character: a.whole("Character", u32::from(COAST_NONE))? as u16,
                    terrain_type: if terrain {
                        Some(a.whole("TerrainType", u32::from(COAST_NONE))? as u16)
                    } else {
                        None
                    },
                })
            };
            match tag.as_str() {
                "SmoothingOffset" => {
                    style.smoothing_high_side = a.f32("HighSide")?;
                    style.smoothing_low_side = a.f32("LowSide")?;
                }
                "HighBeach" => style.high_beach = level(&mut a, true)?,
                "LowBeach" => style.low_beach = level(&mut a, true)?,
                "Waterline" => style.waterline = level(&mut a, false)?,
                other => {
                    // Not a part of the file as we know it: kept as written.
                    let attrs: Vec<String> =
                        a.extra().iter().map(|(k, v)| format!("{k}={v}")).collect();
                    style.extra.insert(other.to_owned(), attrs.join(","));
                    continue;
                }
            }
            for (k, v) in a.extra() {
                style.extra.insert(format!("{tag}@{k}"), v);
            }
        }
        for need in ["SmoothingOffset", "HighBeach", "LowBeach", "Waterline"] {
            if !seen.contains_key(need) {
                return err(file, format!("style {name}: <{need}> is missing"));
            }
        }
        styles.push(style);
    }
    if styles.is_empty() {
        return err(file, "no coast style");
    }
    if profiles.is_empty() {
        return err(file, "no character spline");
    }
    Ok((profiles, styles))
}

/// `coaststyles.xml` with its character splines and `coastspline01.txt`, read through `read`. A
/// style that names a spline the list does not have is an error.
pub fn build_coast(read: ReadFile<'_>) -> Result<CoastSettings, ModelError> {
    let (files, styles) = parsed(parse_coast_xml(&read_file(read, COAST_STYLES_FILE)?))?;
    let mut profiles = Vec::new();
    for f in files {
        let lower = f.to_ascii_lowercase();
        let points = parsed(parse_spline(&f, &read_file(read, &lower)?))?;
        profiles.push(CoastProfile { file: f, points });
    }
    for s in &styles {
        for (what, l) in [
            ("HighBeach", &s.high_beach),
            ("LowBeach", &s.low_beach),
            ("Waterline", &s.waterline),
        ] {
            if l.character != COAST_NONE && usize::from(l.character) >= profiles.len() {
                return Err(ModelError::invalid(format!(
                    "{COAST_STYLES_FILE}: style {} {what} has the character {}, the list has {} splines",
                    s.name,
                    l.character,
                    profiles.len()
                )));
            }
        }
    }
    let spline = parsed(parse_spline(
        COAST_SPLINE_FILE,
        &read_file(read, COAST_SPLINE_FILE)?,
    ))?;
    Ok(CoastSettings {
        profiles,
        styles,
        spline,
    })
}

/// `wavesettings.xml`: the attributes of `<WaveSettings>`.
pub fn parse_waves(bytes: &[u8]) -> Res<WaveSettings> {
    let file = WAVES_FILE;
    let text = lenient_comments(utf8(file, bytes)?);
    let doc = parse_xml(file, &text, "WaveSettings")?;
    let mut a = Attrs::of(file, doc.root_element());
    Ok(WaveSettings {
        waves_per_edge: a.f32("WavesPerEdge")?,
        random_seed: a.f32("RandomSeed")?,
        crest_pages_u: a.whole("CrestPagesU", 1024)?,
        crest_pages_v: a.whole("CrestPagesV", 1024)?,
        crest_pages_in_use: a.whole("CrestPagesInUse", 1024)?,
        min_size: a.f32("MinSize")?,
        max_size: a.f32("MaxSize")?,
        start_distance: a.f32("StartDistance")?,
        end_distance: a.f32("EndDistance")?,
        cycle_time: a.f32("CycleTime")?,
        fade_in_start: a.f32("FadeInStart")?,
        fade_in_end: a.f32("FadeInEnd")?,
        fade_out_start: a.f32("FadeOutStart")?,
        fade_out_end: a.f32("FadeOutEnd")?,
        restart_delay_min: a.f32("RestartDelayMin")?,
        restart_delay_max: a.f32("RestartDelayMax")?,
        color: a.rgb("Color_R", "Color_G", "Color_B")?,
        extra: a.extra(),
    })
}

/// `icesettings.xml` and `icesettings_low.xml` (the same attributes, `file` names the one parsed).
pub fn parse_ice(file: &str, bytes: &[u8]) -> Res<IceParams> {
    let text = lenient_comments(utf8(file, bytes)?);
    let doc = parse_xml(file, &text, "IceSettings")?;
    let mut a = Attrs::of(file, doc.root_element());
    Ok(IceParams {
        dump_input: a.bool("DumpInput")?,
        far_sample_dist: a.f32("FarSampleDist")?,
        far_samples: a.f32("FarSamples")?,
        near_sample_dist: a.f32("NearSampleDist")?,
        near_samples: a.f32("NearSamples")?,
        crack_cost: a.f32("CrackCost")?,
        initial_potential: a.f32("InitialPotential")?,
        crack_min_clip: a.f32("CrackMinClip")?,
        crack_max_clip: a.f32("CrackMaxClip")?,
        edge_pull_amount: a.f32("EdgePullAmount")?,
        small_poly_shrink_amount: a.f32("SmallPolyShrinkAmount")?,
        lattice_verts_per_hex: a.f32("LatticeVertsPerHex")?,
        displace_frequency: a.f32("DisplaceFrequency")?,
        displace_min: a.f32("DisplaceMin")?,
        displace_max: a.f32("DisplaceMax")?,
        displace_exponent: a.f32("DisplaceExponent")?,
        small_poly_depth_min: a.f32("SmallPolyDepthMin")?,
        small_poly_depth_max: a.f32("SmallPolyDepthMax")?,
        small_poly_height_min: a.f32("SmallPolyHeightMin")?,
        small_poly_height_max: a.f32("SmallPolyHeightMax")?,
        large_poly_edge_height_min: a.f32("LargePolyEdgeHeightMin")?,
        large_poly_edge_height_max: a.f32("LargePolyEdgeHeightMax")?,
        large_poly_edge_height_freq: a.f32("LargePolyEdgeHeightFreq")?,
        skirt_slope_small: a.f32("SkirtSlopeSmall")?,
        skirt_slope_large: a.f32("SkirtSlopeLarge")?,
        large_poly_height: a.f32("LargePolyHeight")?,
        large_poly_depth: a.f32("LargePolyDepth")?,
        boundary_kill_rate: a.f32("BoundaryKillRate")?,
        interior_kill_rate: a.f32("InteriorKillRate")?,
        subsurface: a.rgb("Subsurface_R", "Subsurface_G", "Subsurface_B")?,
        specular_power: a.f32("SpecularPower")?,
        texture_repeat: a.f32("TextureRepeat")?,
        bump_repeat: a.f32("BumpRepeat")?,
        side_texture_repeat_u: a.f32("SideTextureRepeat_U")?,
        side_bump_repeat_u: a.f32("SideBumpRepeat_U")?,
        side_texture_repeat_v: a.f32("SideTextureRepeat_V")?,
        side_bump_repeat_v: a.f32("SideBumpRepeat_V")?,
        underwater_1: a.rgb("Underwater_R1", "Underwater_G1", "Underwater_B1")?,
        underwater_2: a.rgb("Underwater_R2", "Underwater_G2", "Underwater_B2")?,
        underwater_color_ramp: a.f32("Underwater_ColorRamp")?,
        underwater_alpha_ramp: a.f32("Underwater_AlphaRamp")?,
        extra: a.extra(),
    })
}

/// The coast, wave and ice settings of the install.
pub fn build_water_settings(
    read: ReadFile<'_>,
) -> Result<(CoastSettings, WaveSettings, IceSettings), ModelError> {
    let coast = build_coast(read)?;
    let waves = parsed(parse_waves(&read_file(read, WAVES_FILE)?))?;
    let ice = IceSettings {
        full: parsed(parse_ice(ICE_FILE, &read_file(read, ICE_FILE)?))?,
        low: parsed(parse_ice(ICE_LOW_FILE, &read_file(read, ICE_LOW_FILE)?))?,
    };
    Ok((coast, waves, ice))
}

/// Replaces the sections `b2-water` builds (coast, waves, ice, lean) in `into`.
pub fn apply_water(
    into: &mut WorldSettings,
    water: (CoastSettings, WaveSettings, IceSettings),
    mut lean: Vec<LeanInfo>,
) {
    lean.sort_by(|a, b| a.file.cmp(&b.file));
    (into.coast, into.waves, into.ice) = water;
    into.lean = lean;
}

// ---- merging into the blob in the manifest -------------------------------------------------

/// The `tb:world-b2` blob already in the manifest; empty settings when there is none. Every set
/// that writes settings starts from it and replaces its own sections, so building the sets in any
/// order gives the same blob.
pub fn load_existing(
    store: &BlobStore,
    manifest: &ManifestBuilder,
) -> Result<WorldSettings, ModelError> {
    let Some(v) = manifest
        .manifest()
        .variants(&LogicalId::new(WORLD_SETTINGS_ID))
        .first()
    else {
        return Ok(WorldSettings::default());
    };
    let path = store.blob_path(&v.hash);
    let bytes = std::fs::read(&path).map_err(|e| {
        ModelError::invalid(format!("reading the settings {}: {e}", path.display()))
    })?;
    // C5WS stays version 1 until the first release, so a blob written before a field was added does
    // not read. It is not an error to rebuild over it: the sections the other sets wrote are
    // missing until they are built again.
    Ok(WorldSettings::decode(&bytes).unwrap_or_else(|e| {
        tracing::warn!(
            "the settings blob in the manifest does not read ({e}); starting from empty settings, rebuild the other B2 sets"
        );
        WorldSettings::default()
    }))
}

/// Stores `settings` as `tb:world-b2` in the boot prefetch group. Returns the blob size.
pub fn store_settings(
    store: &BlobStore,
    manifest: &mut ManifestBuilder,
    settings: &WorldSettings,
) -> Result<u64, ModelError> {
    let (hash, size) = store.put(&settings.encode())?;
    manifest.add(
        LogicalId::new(WORLD_SETTINGS_ID),
        Variant {
            format: VariantFormat::Generic,
            hash,
            size,
            prefetch: BOOT,
        },
    );
    Ok(size)
}

/// Replaces the sections `b2-light` builds (fog, overlay, colour keys) in `into`.
pub fn apply_light(into: &mut WorldSettings, light: WorldSettings) {
    into.fog_colors = light.fog_colors;
    into.fluid_fow = light.fluid_fow;
    into.hex_grid = light.hex_grid;
    into.culture_borders = light.culture_borders;
    into.highlights = light.highlights;
    into.overlay_defs = light.overlay_defs;
    into.arrows = light.arrows;
    into.color_keys = light.color_keys;
}

/// Replaces the sections `b2-terrain` builds (terrain definitions, decals) in `into`.
pub fn apply_terrain(
    into: &mut WorldSettings,
    terrain_defs: Vec<TerrainDefRow>,
    decals: Vec<DecalDef>,
) {
    into.terrain_defs = terrain_defs;
    into.decals = complete_crops(decals);
}

/// How many field textures the original has per style (`crops_europe_01_d.dds` .. `_08_d.dds`).
pub const CROPS_VARIANTS: u32 = 8;

/// The `Crops` group of `decal_filenames.xml` names no texture: `cvFarmSimSystem::LoadFarmDecalTextures`
/// loads the fields by file name, the European albedos `crops_europe_NN_d.dds` with the shared heights
/// `crops_generic_NN_h.dds` of `DecalTextures.fpk`. The group is made whole here: its types without an
/// albedo are replaced by one type per field texture, so the farms of the world have fields.
// re: `cvFarmSimSystem::LoadFarmDecalTextures` (the texture names are in the binary, not in the XML)
pub fn complete_crops(decals: Vec<DecalDef>) -> Vec<DecalDef> {
    // The priority the XML gives the group (`<Crops_01 priority="0.5"/>`): the fields lie over the
    // floodplains (0.4) and under the roads (0.7).
    let priority = decals
        .iter()
        .find(|d| d.kind == "Crops" && d.albedo.is_none())
        .and_then(|d| d.priority);
    let mut out: Vec<DecalDef> = decals
        .into_iter()
        .filter(|d| !(d.kind == "Crops" && d.albedo.is_none()))
        .collect();
    if out.iter().any(|d| d.kind == "Crops") {
        return out;
    }
    out.extend((1..=CROPS_VARIANTS).map(|n| DecalDef {
        kind: "Crops".into(),
        name: format!("Crops_{n:02}"),
        albedo: Some(tx_id(&format!("crops_europe_{n:02}_d.dds"))),
        heightmap: Some(tx_id(&format!("crops_generic_{n:02}_h.dds"))),
        priority,
        extra: Default::default(),
    }));
    out
}

// ---- the set -------------------------------------------------------------------------------

fn read_file(read: ReadFile<'_>, name: &str) -> Result<Vec<u8>, ModelError> {
    read(name)?.ok_or_else(|| ModelError::invalid(format!("{name} is missing from the install")))
}

fn parsed<T>(r: Res<T>) -> Result<T, ModelError> {
    r.map_err(|e| ModelError::invalid(e.to_string()))
}

/// Reads and parses all the settings files. The second value is the warnings: unknown attributes
/// and elements, odd colours.
pub fn build_settings(read: ReadFile<'_>) -> Result<(WorldSettings, Vec<String>), ModelError> {
    let mut warnings = Vec::new();
    let settings = WorldSettings {
        fog_colors: parsed(parse_fog_colors(&read_file(read, FOG_COLORS_FILE)?))?,
        fluid_fow: parsed(parse_fluid_fow(&read_file(read, FLUID_FOW_FILE)?))?,
        hex_grid: parsed(parse_hex_grid(&read_file(read, HEX_GRID_FILE)?))?,
        culture_borders: parsed(parse_culture_borders(&read_file(
            read,
            CULTURE_BORDERS_FILE,
        )?))?,
        highlights: parsed(parse_highlights(
            &read_file(read, HIGHLIGHTS_FILE)?,
            &mut warnings,
        ))?,
        overlay_defs: parsed(parse_overlay_defs(&read_file(read, OVERLAY_DEFS_FILE)?))?,
        arrows: parsed(parse_arrows(&read_file(read, ARROWS_FILE)?))?,
        color_keys: color_keys(),
        ..WorldSettings::default()
    };
    for e in settings.extras() {
        warnings.push(format!("unknown in the settings files: {e}"));
    }
    Ok((settings, warnings))
}

/// The textures the settings name that `known` does not have: an error if there are any, because
/// the overlay would reach for a file that is not in the build.
pub fn check_references(
    settings: &WorldSettings,
    known: impl Fn(&LogicalId) -> bool,
) -> Result<(), ModelError> {
    let mut missing: Vec<String> = settings
        .highlights
        .iter()
        .filter_map(|h| h.texture.as_ref().map(|t| (&h.name, t)))
        .filter(|(_, t)| !known(t))
        .map(|(name, t)| format!("{} (style {name:?})", t.as_str()))
        .collect();
    missing.sort();
    missing.dedup();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(ModelError::invalid(format!(
            "highlight textures that are not in the build: {}",
            missing.join(", ")
        )))
    }
}

/// Which copy of `Highlights.xml` is taken and how it differs from the ones it hides (AB-D15).
pub fn highlights_copy_note(game: &GameSource) -> Result<String, ModelError> {
    let Some(winner) = game.lookup(HIGHLIGHTS_FILE) else {
        return Err(ModelError::invalid(format!(
            "{HIGHLIGHTS_FILE} is missing from the install"
        )));
    };
    let describe =
        |e: &crate::game::source::Entry| -> Result<(String, Vec<HighlightStyle>), ModelError> {
            let mut ignored = Vec::new();
            let styles = parsed(parse_highlights(&game.read(e)?, &mut ignored))?;
            let path = game.origin_path(e).unwrap_or_default();
            let shown = path.strip_prefix(game.root()).unwrap_or(&path).display();
            Ok((format!("{:?} {shown}", e.layer), styles))
        };
    let (taken, taken_styles) = describe(winner)?;
    let mut note = format!(
        "{HIGHLIGHTS_FILE}: taken {taken} with {} styles",
        taken_styles.len()
    );
    for hidden in game.shadowed().iter().filter(|e| e.name == HIGHLIGHTS_FILE) {
        let (name, styles) = describe(hidden)?;
        let by_name: BTreeMap<&str, &HighlightStyle> =
            styles.iter().map(|s| (s.name.as_str(), s)).collect();
        let only_taken: Vec<&str> = taken_styles
            .iter()
            .map(|s| s.name.as_str())
            .filter(|n| !by_name.contains_key(n))
            .collect();
        let changed: Vec<&str> = taken_styles
            .iter()
            .filter(|s| by_name.get(s.name.as_str()).is_some_and(|o| **o != **s))
            .map(|s| s.name.as_str())
            .collect();
        note.push_str(&format!(
            "; hides {name} with {} styles (only in the taken copy: {}; changed in it: {})",
            styles.len(),
            if only_taken.is_empty() {
                "none".to_owned()
            } else {
                only_taken.join(", ")
            },
            if changed.is_empty() {
                "none".to_owned()
            } else {
                changed.join(", ")
            },
        ));
    }
    Ok(note)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dashes_in_comments_are_made_readable_and_nothing_else_changes() {
        let t = "<!-- a --- b -->\n<A x=\"1-2\"><!--c--></A>";
        let fixed = lenient_comments(t);
        assert_eq!(fixed, "<!-- a ___ b -->\n<A x=\"1-2\"><!--c--></A>");
        assert!(Document::parse(t).is_err());
        assert!(Document::parse(&fixed).is_ok());
        assert_eq!(lenient_comments("<A/>"), "<A/>");
        assert_eq!(lenient_comments("<!-- open --"), "<!-- open __");
    }

    const FLUID: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<FluidFOWSettings>
	<SpawnFrequency>22</SpawnFrequency><SizeMean>2.5</SizeMean><SizeMin>1.0</SizeMin>
	<SizeMax>4.0</SizeMax><SizeDeviation>1.0</SizeDeviation> <MassMean>1</MassMean>
	<MassMin>.25</MassMin><MassMax>4.0</MassMax><MassDeviation>1</MassDeviation>
	<MassAndSizeCorrelation>.5f</MassAndSizeCorrelation><HeightVariation>50.0f</HeightVariation>
	<FluidDensity>45</FluidDensity><Speed>3.5</Speed><Drag>.004</Drag><MasterSize>0.8</MasterSize>
	<ForceStrength>.000035</ForceStrength><MaxRotate>.0025</MaxRotate><FogHeight>11</FogHeight>
</FluidFOWSettings>"#;

    const HIGHLIGHTS: &str = r#"<Highlights>
    <style name="" type="HexContour" width=".2" texture="hex_contour2.dds"/>
    <style name="Fill" type="FilledHex" width ="1" color ="0,0,0,128"/>
    <style name="Model" type="HexModel" model = "SelectedAniOneTurn.fxsxml" />
    <style name="EnemyOutline" type="SplineBorder" width ="7" texture="hex_contour1.dds" color="255,0,0,0,200" />
</Highlights>"#;

    #[test]
    fn numbers_as_the_files_write_them() {
        assert_eq!(number("f", "x", ".5f").unwrap(), 0.5);
        assert_eq!(number("f", "x", " 50.0f ").unwrap(), 50.0);
        assert_eq!(number("f", "x", ".000035").unwrap(), 0.000035);
        assert!(number("f", "x", "abc").is_err());
        assert!(number("f", "x", "").is_err());
        assert!(number("f", "x", "inf").is_err());
    }

    #[test]
    fn fluid_fow_reads_every_element_and_leaves_no_extra() {
        let f = parse_fluid_fow(FLUID.as_bytes()).unwrap();
        assert_eq!(f.spawn_frequency, 22.0);
        assert_eq!(f.mass_min, 0.25);
        assert_eq!(f.mass_and_size_correlation, 0.5);
        assert_eq!(f.height_variation, 50.0);
        assert_eq!(f.fog_height, 11.0);
        assert!(f.extra.is_empty());
    }

    #[test]
    fn fluid_fow_unknown_element_is_extra_and_a_missing_one_an_error() {
        let with = FLUID.replace(
            "</FluidFOWSettings>",
            "<Turbulence>3</Turbulence></FluidFOWSettings>",
        );
        let f = parse_fluid_fow(with.as_bytes()).unwrap();
        assert_eq!(f.extra["Turbulence"], "3");
        let without = FLUID.replace("<Drag>.004</Drag>", "");
        let e = parse_fluid_fow(without.as_bytes()).unwrap_err();
        assert!(e.to_string().contains("<Drag> is missing"), "{e}");
        let bad = FLUID.replace("<Drag>.004</Drag>", "<Drag>slow</Drag>");
        assert!(parse_fluid_fow(bad.as_bytes()).is_err());
    }

    #[test]
    fn highlights_keep_colours_as_written_and_warn_about_five_numbers() {
        let mut w = Vec::new();
        let s = parse_highlights(HIGHLIGHTS.as_bytes(), &mut w).unwrap();
        assert_eq!(s.len(), 4);
        assert_eq!(s[0].name, "");
        assert_eq!(s[0].kind, HighlightKind::HexContour);
        assert_eq!(s[0].texture, Some(LogicalId::new("tx:hex_contour2.dds")));
        assert_eq!(s[0].width, Some(0.2));
        assert_eq!(s[1].color, [0, 0, 0, 128]);
        assert_eq!(s[2].model.as_deref(), Some("SelectedAniOneTurn.fxsxml"));
        assert_eq!(s[3].color, [255, 0, 0, 0, 200]);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(
            w[0].contains("EnemyOutline") && w[0].contains("5 numbers"),
            "{w:?}"
        );
        assert!(s.iter().all(|s| s.extra.is_empty()));
    }

    #[test]
    fn highlights_unknown_type_or_bad_colour_is_an_error() {
        let mut w = Vec::new();
        let bad_type = r#"<Highlights><style name="a" type="Glow"/></Highlights>"#;
        assert!(parse_highlights(bad_type.as_bytes(), &mut w).is_err());
        let bad_colour =
            r#"<Highlights><style name="a" type="FilledHex" color="1,2,300"/></Highlights>"#;
        assert!(parse_highlights(bad_colour.as_bytes(), &mut w).is_err());
        let extra = r#"<Highlights><style name="a" type="FilledHex" glow="1"/></Highlights>"#;
        let s = parse_highlights(extra.as_bytes(), &mut w).unwrap();
        assert_eq!(s[0].extra["glow"], "1");
    }

    #[test]
    fn hex_grid_culture_borders_and_arrows() {
        let h =
            parse_hex_grid(br#"<!-- c --><HexGrid color = "255,255,255,75" width=".2"/>"#).unwrap();
        assert_eq!(
            (h.color.as_slice(), h.width),
            (&[255, 255, 255, 75][..], 0.2)
        );
        assert!(h.extra.is_empty());

        let c = parse_culture_borders(
            br#"<CultureBorders primary_width = "1.4" secondary_width = "20"
              primary_aa_width = "0.5" secondary_aa_width = ".3" primary_alpha = "200"
              secondary_alpha = "90" corner_bias = "0.105" cs_primary_width = "2.8"
              cs_outline_width =".4" cs_aa_width = ".5" cs_primary_alpha = "130"
              cs_outline_alpha ="160" cs_dash_width ="8" randomness = "6" expansion = "2" />"#,
        )
        .unwrap();
        assert_eq!(
            (c.primary_width, c.cs_dash_width, c.corner_bias),
            (1.4, 8.0, 0.105)
        );
        assert!(c.extra.is_empty());

        let a = parse_arrows(
            br#"<Arrow min_width = "3" max_width = "5" width2 = "1.5" arc_height = "75"
              outline_width = ".65" outline_width2 = ".65" body_color = "255,0,0"
              outline_color = "255,255,255" alpha = ".5" target_height = "20" anim_speed = ".5"
              ambient_light_level = ".7" new_one = "1" />"#,
        )
        .unwrap();
        assert_eq!((a.arc_height, a.alpha), (75.0, 0.5));
        assert_eq!(a.body_color, [255, 0, 0]);
        assert_eq!(a.extra.keys().collect::<Vec<_>>(), ["new_one"]);
    }

    #[test]
    fn overlay_defs_selection_highlight() {
        let o = parse_overlay_defs(
            br#"<Overlays><UnitSelectionHighlight>
              <Color red=".9" green=".9" blue=".45" alpha="1" />
              <Pulse>true</Pulse><PulseSpeed>2</PulseSpeed>
              <Rotation>clockwise</Rotation> <!-- c --> <RotationSpeed>1</RotationSpeed>
            </UnitSelectionHighlight></Overlays>"#,
        )
        .unwrap();
        let s = o.unit_selection_highlight;
        assert_eq!(s.color, [0.9, 0.9, 0.45, 1.0]);
        assert!(s.pulse);
        assert_eq!(s.rotation, Rotation::Clockwise);
        assert!(s.extra.is_empty() && o.extra.is_empty());
        let other =
            br#"<Overlays><UnitSelectionHighlight><Color red="0" green="0" blue="0" alpha="1"/>
            <Pulse>false</Pulse><PulseSpeed>1</PulseSpeed><Rotation>spin</Rotation>
            <RotationSpeed>1</RotationSpeed></UnitSelectionHighlight></Overlays>"#;
        let o = parse_overlay_defs(other).unwrap();
        assert_eq!(
            o.unit_selection_highlight.rotation,
            Rotation::CounterClockwise
        );
    }

    #[test]
    fn fog_colors_rows() {
        assert_eq!(
            parse_fog_colors(b"146,182,243\r\n").unwrap(),
            [[146, 182, 243]]
        );
        assert!(parse_fog_colors(b"1,2").is_err());
        assert!(parse_fog_colors(b"").is_err());
    }

    #[test]
    fn colour_keys_are_the_file_list() {
        let k = color_keys();
        assert_eq!(k.len(), 22);
        let find = |f: &str| k.iter().find(|c| c.file == tx_id(f)).unwrap();
        let c = find("terrainshadow_europe_colorkey.tga");
        assert_eq!(
            (c.system.as_str(), c.kind.as_str()),
            ("terrainshadow", "europe")
        );
        let c = find("rivers_generic_colorkey.tga");
        assert_eq!((c.system.as_str(), c.kind.as_str()), ("rivers", "generic"));
        let c = find("terraincolorkey.tga");
        assert_eq!(
            (c.system.as_str(), c.kind.as_str()),
            ("terraincolorkey", "")
        );
    }

    const TERRAIN_DEF: &str = "Continent ID,Diffuse,Heightmap,Specmap,BaseDominance,Texture Scale,Projection(0=horizontal 1=vertical), ID (unique for continent)\r\n\
        0,Asia_Tundra_D.dds,Generic_Tundra_H.dds,NULL,0,0.014,0,0\r\n\
        0,Generic_Snow_D.dds,Generic_Snow_H.dds,Generic_Snow_S.dds,0,0.014,0,30\r\n\
        4,Euro_Mountain_Top_D.dds,Euro_Mountain_Top_H.dds,Euro_Mountain_Top_S.dds,0,0.1,1,8\r\n\r\n";

    #[test]
    fn terrain_defs_rows_and_null_specmaps() {
        let r = parse_terrain_defs(TERRAIN_DEF.as_bytes()).unwrap();
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].diffuse, Some(tx_id("asia_tundra_d.dds")));
        assert_eq!(r[0].specmap, None);
        // The packed copy of the file has no texture at all in a row.
        let packed = "Continent ID,x\n0,NULL,NULL,NULL,0,0.007,0,0\n";
        let r = parse_terrain_defs(packed.as_bytes()).unwrap();
        assert_eq!((r[0].diffuse.clone(), r[0].heightmap.clone()), (None, None));
        let r = parse_terrain_defs(TERRAIN_DEF.as_bytes()).unwrap();
        assert_eq!(r[1].specmap, Some(tx_id("generic_snow_s.dds")));
        assert_eq!(
            (r[1].continent, r[1].id, r[1].texture_scale),
            (0, 30, 0.014)
        );
        assert_eq!(
            (r[2].continent, r[2].base_dominance, r[2].projection),
            (4, 0, 1)
        );
    }

    #[test]
    fn terrain_defs_bad_files_are_errors_that_name_the_line() {
        for (bad, want) in [
            ("", "empty"),
            ("a,b,c\n", "header"),
            ("Continent ID,x\n", "no row"),
            (
                "Continent ID,x\n0,a.dds,b.dds,NULL,0,0.1,0\n",
                "line 2: 7 columns",
            ),
            (
                "Continent ID,x\n9,a.dds,b.dds,NULL,0,0.1,0,1\n",
                "continent 9",
            ),
            (
                "Continent ID,x\n0,a.dds,b.dds,NULL,0,fast,0,1\n",
                "Texture Scale",
            ),
            ("Continent ID,x\n0,a.dds,b.dds,NULL,0,0.1,0,one\n", "ID"),
            (
                "Continent ID,x\n0,a.dds,b.dds,NULL,2,0.1,0,1\n",
                "BaseDominance",
            ),
        ] {
            let e = parse_terrain_defs(bad.as_bytes()).unwrap_err().to_string();
            assert!(e.contains(want), "{bad:?}: {e}");
        }
    }

    #[test]
    fn the_crops_group_without_a_texture_gets_the_field_textures_and_keeps_its_priority() {
        let d = complete_crops(
            parse_decals(
                br#"<DecalTypes><Crops><Crops_01 priority="0.5"/></Crops>
                    <Main_Road><Main_Road_01 priority="0.7"/></Main_Road></DecalTypes>"#,
            )
            .unwrap(),
        );
        let crops: Vec<&DecalDef> = d.iter().filter(|x| x.kind == "Crops").collect();
        assert_eq!(crops.len(), CROPS_VARIANTS as usize);
        assert!(crops.iter().all(|c| c.priority == Some(0.5)));
        assert_eq!(
            crops[2].albedo.as_ref().map(LogicalId::as_str),
            Some("tx:crops_europe_03_d.dds")
        );
        assert_eq!(
            crops[2].heightmap.as_ref().map(LogicalId::as_str),
            Some("tx:crops_generic_03_h.dds")
        );
        // The other groups are left alone, and a group that names its textures is not touched.
        assert!(
            d.iter()
                .any(|x| x.kind == "Main_Road" && x.albedo.is_none())
        );
        let named = complete_crops(
            parse_decals(br#"<DecalTypes><Crops><Crops_01 albedo="a.dds" /></Crops></DecalTypes>"#)
                .unwrap(),
        );
        assert_eq!(named.len(), 1);
    }

    #[test]
    fn decals_keep_their_groups_and_turn_paths_into_ids() {
        let d = parse_decals(
            br#"<?xml version="1.0"?><DecalTypes>
              <Main_Road><Main_Road_01 priority="0.7"/></Main_Road>
              <Tree_Shadow><Tree_Shadow_01 albedo="assets/Decals/Trees/Tree_Shadow_2.dds"
                 heightmap="assets/Decals/Trees/Tree_Shadow_2.dds"/></Tree_Shadow>
              <Riverbank_EndCap2><Floodplains_01 albedo="assets/Decals/Riverbanks/riverbank_D_Endcap2.dds"
                 heightmap ="assets/Decals/grey.dds" priority="0.4" glow="1"/></Riverbank_EndCap2>
            </DecalTypes>"#,
        )
        .unwrap();
        assert_eq!(d.len(), 3);
        assert_eq!(
            (d[0].kind.as_str(), d[0].priority, d[0].albedo.is_none()),
            ("Main_Road", Some(0.7), true)
        );
        assert_eq!(d[1].albedo, Some(tx_id("tree_shadow_2.dds")));
        assert_eq!(d[2].name, "Floodplains_01");
        assert_eq!(d[2].albedo, Some(tx_id("riverbank_d_endcap2.dds")));
        assert_eq!(d[2].heightmap, Some(tx_id("grey.dds")));
        assert_eq!(d[2].extra["glow"], "1");
        // A group without a decal, a group with an attribute, a wrong root.
        assert!(parse_decals(b"<DecalTypes><Crops/></DecalTypes>").is_err());
        assert!(parse_decals(b"<DecalTypes><Crops x='1'><C/></Crops></DecalTypes>").is_err());
        assert!(parse_decals(b"<Decals/>").is_err());
        assert!(parse_decals(b"<DecalTypes/>").is_err());
    }

    #[test]
    fn sets_merge_into_the_settings_already_in_the_manifest() {
        let dir = std::env::temp_dir().join(format!("civ5-merge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = BlobStore::open(&dir).unwrap();
        let mut m = ManifestBuilder::new();
        // Terrain first, then light: neither may drop the other's sections.
        let mut w = load_existing(&store, &m).unwrap();
        assert_eq!(w, WorldSettings::default());
        let defs = parse_terrain_defs(TERRAIN_DEF.as_bytes()).unwrap();
        apply_terrain(&mut w, defs.clone(), vec![]);
        store_settings(&store, &mut m, &w).unwrap();
        m.finish(&store).unwrap();

        let mut m = ManifestBuilder::open(&store).unwrap();
        let mut w = load_existing(&store, &m).unwrap();
        assert_eq!(w.terrain_defs, defs);
        let light = WorldSettings {
            fog_colors: vec![[1, 2, 3]],
            ..WorldSettings::default()
        };
        apply_light(&mut w, light);
        let a = store_settings(&store, &mut m, &w).unwrap();
        assert!(a > 0);
        let w = load_existing(&store, &m).unwrap();
        assert_eq!((w.fog_colors.len(), w.terrain_defs.len()), (1, 3));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn references_to_missing_textures_are_an_error() {
        let mut w = Vec::new();
        let styles = parse_highlights(HIGHLIGHTS.as_bytes(), &mut w).unwrap();
        let settings = WorldSettings {
            fog_colors: vec![],
            fluid_fow: parse_fluid_fow(FLUID.as_bytes()).unwrap(),
            hex_grid: HexGrid::default(),
            culture_borders: CultureBorders::default(),
            highlights: styles,
            overlay_defs: parse_overlay_defs(
                br#"<Overlays><UnitSelectionHighlight><Color red="0" green="0" blue="0" alpha="1"/>
                <Pulse>false</Pulse><PulseSpeed>1</PulseSpeed><Rotation>off</Rotation>
                <RotationSpeed>1</RotationSpeed></UnitSelectionHighlight></Overlays>"#,
            )
            .unwrap(),
            arrows: Arrows::default(),
            color_keys: vec![],
            ..WorldSettings::default()
        };
        assert!(check_references(&settings, |_| true).is_ok());
        let e = check_references(&settings, |t| t.as_str() != "tx:hex_contour1.dds")
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("tx:hex_contour1.dds") && e.contains("EnemyOutline"),
            "{e}"
        );
    }

    // ---- water ----

    const COAST: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<CoastRoot>
  <SplineCharacterList xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
    <!-- listed order is the index -->
    <SplineCharacter Filename="CoastCharacterSpline_01_Water.txt" />
    <SplineCharacter Filename="CoastCharacterSpline_01_Surf.txt" />
  </SplineCharacterList>
  <CoastStyleList>
    <CoastStyle type="Default">
      <SmoothingOffset HighSide="20" LowSide="21" />
      <HighBeach Height="0.2244" Offset="1.5" Character="1" TerrainType="5"/>
      <LowBeach Height="0.19226" Offset="1.5" Character="1" TerrainType="5"/>
      <Waterline Height="0.19836" Offset="0" Character="0" />
    </CoastStyle>
    <CoastStyle type="DefaultShallows">
      <SmoothingOffset HighSide="20" LowSide="20" />
      <HighBeach Height="0.11" Offset="1.5" Character="65535" TerrainType="65535" />
      <LowBeach Height="0.09" Offset="1.5" Character="65535" TerrainType="65535"/>
      <Waterline Height="0.1" Offset="0" Character="65535" />
    </CoastStyle>
  </CoastStyleList>
</CoastRoot>"#;

    #[test]
    fn coast_styles_parse_with_their_levels() {
        let (files, styles) = parse_coast_xml(COAST.as_bytes()).unwrap();
        assert_eq!(
            files,
            [
                "CoastCharacterSpline_01_Water.txt",
                "CoastCharacterSpline_01_Surf.txt"
            ]
        );
        assert_eq!(styles.len(), 2);
        let d = &styles[0];
        assert_eq!(d.name, "Default");
        assert_eq!((d.smoothing_high_side, d.smoothing_low_side), (20.0, 21.0));
        assert_eq!(
            d.high_beach,
            CoastLevel {
                height: 0.2244,
                offset: 1.5,
                character: 1,
                terrain_type: Some(5)
            }
        );
        assert_eq!(d.waterline.terrain_type, None);
        assert_eq!(d.waterline.character, 0);
        assert_eq!(styles[1].low_beach.character, COAST_NONE);
        assert!(d.extra.is_empty() && styles[1].extra.is_empty());
    }

    #[test]
    fn coast_style_extras_and_gaps_are_found() {
        let with_extra = COAST
            .replacen(
                r#"<Waterline Height="0.19836" Offset="0" Character="0" />"#,
                r#"<Waterline Height="0.19836" Offset="0" Character="0" Foam="2" /><Reef Depth="3"/>"#,
                1,
            )
            .replacen(r#"<CoastStyle type="Default">"#, r#"<CoastStyle type="Default" tint="red">"#, 1);
        let (_, styles) = parse_coast_xml(with_extra.as_bytes()).unwrap();
        let keys: Vec<&str> = styles[0].extra.keys().map(String::as_str).collect();
        assert_eq!(keys, ["Reef", "Waterline@Foam", "tint"]);
        // A level missing, a bad number.
        let no_water = COAST.replacen(
            "<Waterline Height=\"0.1\" Offset=\"0\" Character=\"65535\" />",
            "",
            1,
        );
        assert!(
            parse_coast_xml(no_water.as_bytes())
                .unwrap_err()
                .msg
                .contains("Waterline")
        );
        let bad = COAST.replacen(
            "Character=\"1\" TerrainType=\"5\"",
            "Character=\"x\" TerrainType=\"5\"",
            1,
        );
        assert!(parse_coast_xml(bad.as_bytes()).is_err());
    }

    #[test]
    fn splines_drop_and_check_the_third_coordinate() {
        let p = parse_spline("s.txt", b"[400,0.10114,0]\r\n[395.96,0.858047,0]\r\n").unwrap();
        assert_eq!(p, [[400.0, 0.10114], [395.96, 0.858047]]);
        assert!(
            parse_spline("s.txt", b"[1,2,0.5]\n")
                .unwrap_err()
                .msg
                .contains("third")
        );
        assert!(parse_spline("s.txt", b"[1,2]\n").is_err());
        assert!(parse_spline("s.txt", b"1,2,0\n").is_err());
        assert!(parse_spline("s.txt", b"\n").is_err());
    }

    #[test]
    fn build_coast_reads_the_splines_by_name_and_checks_the_indices() {
        let files: BTreeMap<&str, &[u8]> = BTreeMap::from([
            ("coaststyles.xml", COAST.as_bytes()),
            (
                "coastcharacterspline_01_water.txt",
                b"[0,1,0]\n[1,2,0]\n" as &[u8],
            ),
            ("coastcharacterspline_01_surf.txt", b"[0,3,0]\n"),
            ("coastspline01.txt", b"[-4,5,0]\n"),
        ]);
        let read = |n: &str| -> Result<Option<Vec<u8>>, ModelError> {
            Ok(files.get(n).map(|b| b.to_vec()))
        };
        let c = build_coast(&read).unwrap();
        assert_eq!((c.profiles.len(), c.styles.len()), (2, 2));
        assert_eq!(c.profiles[0].points, [[0.0, 1.0], [1.0, 2.0]]);
        assert_eq!(c.profiles[0].file, "CoastCharacterSpline_01_Water.txt");
        assert_eq!(c.spline, [[-4.0, 5.0]]);
        // A style that points at a third spline.
        let wide = COAST.replacen(
            "Character=\"1\" TerrainType=\"5\"",
            "Character=\"2\" TerrainType=\"5\"",
            1,
        );
        let mut files2 = files.clone();
        files2.insert("coaststyles.xml", wide.as_bytes());
        let read2 = |n: &str| -> Result<Option<Vec<u8>>, ModelError> {
            Ok(files2.get(n).map(|b| b.to_vec()))
        };
        let e = build_coast(&read2).unwrap_err().to_string();
        assert!(e.contains("Default") && e.contains("character 2"), "{e}");
        // A missing spline file.
        let mut files3 = files.clone();
        files3.remove("coastcharacterspline_01_surf.txt");
        let read3 = |n: &str| -> Result<Option<Vec<u8>>, ModelError> {
            Ok(files3.get(n).map(|b| b.to_vec()))
        };
        assert!(
            build_coast(&read3)
                .unwrap_err()
                .to_string()
                .contains("surf")
        );
    }

    #[test]
    fn wave_settings_parse() {
        let w = parse_waves(
            br#"<!-- c --><WaveSettings WavesPerEdge="1.13" RandomSeed ="0.0" CrestPagesU="8" CrestPagesV="2"
 CrestPagesInUse="16" MinSize="0.6" MaxSize="1.4" StartDistance="0.08" EndDistance="0.02" CycleTime="9"
 FadeInStart="0.05" FadeInEnd="4.0" FadeOutStart="6.5" FadeOutEnd="8.7" RestartDelayMin="2.1"
 RestartDelayMax="5" Color_R ="64" Color_G ="65" Color_B ="66" />"#,
        )
        .unwrap();
        assert_eq!(
            (w.crest_pages_u, w.crest_pages_v, w.crest_pages_in_use),
            (8, 2, 16)
        );
        assert_eq!(w.color, [64, 65, 66]);
        assert_eq!(w.fade_out_end, 8.7);
        assert!(w.extra.is_empty());
        // A number that does not parse names the attribute.
        let e = parse_waves(br#"<WaveSettings WavesPerEdge="many"/>"#).unwrap_err();
        assert!(e.msg.contains("WavesPerEdge"), "{e}");
    }

    #[test]
    fn ice_settings_parse_with_padded_values() {
        let mut xml = String::from("<IceSettings DumpInput=\"0\" ");
        for (k, v) in [
            ("FarSampleDist", "2.8"),
            ("FarSamples", "48"),
            ("NearSampleDist", "0.1"),
            ("NearSamples", "16"),
            ("CrackCost", "1"),
            ("InitialPotential", "40"),
            ("CrackMinClip", "0.05"),
            ("CrackMaxClip", "0.8"),
            ("EdgePullAmount", "0.25"),
            ("SmallPolyShrinkAmount", "0.35"),
            ("LatticeVertsPerHex", "12.0"),
            ("DisplaceFrequency", "0.1"),
            ("DisplaceMin", "-8"),
            ("DisplaceMax", "15"),
            ("DisplaceExponent", "1"),
            ("SmallPolyDepthMin", "6.1"),
            ("SmallPolyDepthMax", "6.6"),
            ("SmallPolyHeightMin", "0.1"),
            ("SmallPolyHeightMax", "3.6"),
            ("LargePolyEdgeHeightMin", "0"),
            ("LargePolyEdgeHeightMax", "0"),
            ("LargePolyEdgeHeightFreq", "0.5"),
            ("SkirtSlopeSmall", "0.0015625"),
            ("SkirtSlopeLarge", "0.0078125"),
            ("LargePolyHeight", "10"),
            ("LargePolyDepth", "8"),
            ("BoundaryKillRate", "0.60"),
            ("InteriorKillRate", "0.05"),
            ("Subsurface_R", "19"),
            ("Subsurface_G", "142"),
            ("Subsurface_B", "205"),
            ("SpecularPower", "150"),
            ("TextureRepeat", "0.005"),
            ("BumpRepeat", "0.009"),
            ("SideTextureRepeat_U", " 0.005"),
            ("SideBumpRepeat_U", "0.05"),
            ("SideTextureRepeat_V", " 0.005"),
            ("SideBumpRepeat_V", "0.05"),
            ("Underwater_R1", "54"),
            ("Underwater_G1", "154"),
            ("Underwater_B1", "106"),
            ("Underwater_R2", "55"),
            ("Underwater_G2", "155"),
            ("Underwater_B2", "107"),
            ("Underwater_ColorRamp", "10"),
            ("Underwater_AlphaRamp", "2"),
        ] {
            xml.push_str(&format!("{k} = \"{v}\"\n"));
        }
        xml.push_str("/>");
        let i = parse_ice("icesettings.xml", xml.as_bytes()).unwrap();
        assert_eq!(i.far_samples, 48.0);
        assert_eq!(i.side_texture_repeat_u, 0.005);
        assert_eq!(i.subsurface, [19, 142, 205]);
        assert_eq!(i.underwater_2, [55, 155, 107]);
        assert!(!i.dump_input && i.extra.is_empty());
        // An attribute the file has and we do not know goes to the extra.
        let more = xml.replace("/>", "Snow=\"1\" />");
        let i = parse_ice("icesettings.xml", more.as_bytes()).unwrap();
        assert_eq!(i.extra.get("Snow").map(String::as_str), Some("1"));
        // One missing is an error that names it.
        let less = xml.replace("CrackCost = \"1\"", "");
        assert!(
            parse_ice("icesettings.xml", less.as_bytes())
                .unwrap_err()
                .msg
                .contains("CrackCost")
        );
    }
}
