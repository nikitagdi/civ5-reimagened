//! The game's pictures for UI v2's skins: `icon:<ATLAS>:<index>` (the model's icons: unit portraits, techs,
//! buildings, resources, unit flag glyphs, unit actions) is a cell of one of the game's icon atlases; `<ns>:<id>` and
//! `<ns>:<TYPE>` of the catalog's tables name the same cells; `font:<ICON_NAME>` is a glyph of the game's icon fonts
//! (the yield and resource pictures the original draws in texts and in its top panel, `[ICON_GOLD]`).
//!
//! The classic UI set (the game session's) carries the atlas table: for each atlas and icon size, the texture file and
//! how many icons it holds per row and per column. A picture is that texture of the set and the cell's rectangle in
//! it; the renderer's UI texture registry takes the texture under the set's id, and the host fills it after each frame
//! from the set (fetched in the background the first time a picture names it).
//!
//! Known limits: the external textures of UI v2 are read as RGBA (an `R8` or `Rg8` texture shows wrong); pictures
//! of a mod's panels are not drawn.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use asset_format::UiAtlas;
use ui_client::UiStore;
use ui2_host::{AssetResolver, ExtImage, ExtTexture};

/// The icon size a picture takes from an atlas that has several: the smallest one at least this big, else the
/// biggest. The resolver does not know the size the skin draws the picture at; the portraits of the unit panel are
/// about this big, and smaller pictures sample it down.
// SIMPLIFIED(SIMP-300): one icon size per atlas (the smallest of at least 128 px) whatever size the skin draws the
// picture at, instead of the atlas size nearest to the drawn size.
pub const ICON_SIZE: u32 = 128;

/// `icon:<ATLAS>:<index>`: the atlas and the index; `None` for any other name.
pub fn parse_icon(name: &str) -> Option<(&str, u32)> {
    let (namespace, rest) = name.split_once(':')?;
    if !matches!(
        namespace,
        "tech"
            | "unit"
            | "building"
            | "policy"
            | "belief"
            | "religion"
            | "resource"
            | "civ"
            | "leader"
            | "promotion"
            | "improvement"
            | "great_work"
            | "icon"
    ) {
        return None;
    }
    let (atlas, index) = rest.rsplit_once(':')?;
    if atlas.is_empty() {
        return None;
    }
    Some((atlas, index.parse().ok()?))
}

/// The row of an atlas a picture takes (see [`ICON_SIZE`]).
pub fn choose_row<'a>(rows: impl Iterator<Item = &'a UiAtlas>) -> Option<&'a UiAtlas> {
    let rows: Vec<&UiAtlas> = rows
        .filter(|r| r.icon_size > 0 && r.icons_per_row > 0 && r.icons_per_column > 0)
        .collect();
    rows.iter()
        .filter(|r| r.icon_size >= ICON_SIZE)
        .min_by_key(|r| r.icon_size)
        .or_else(|| rows.iter().max_by_key(|r| r.icon_size))
        .copied()
}

/// The picture `name` names in the set: its texture and the cell's rectangle in it; else why there is none (not an
/// icon name, an atlas the set does not have, an index past its cells, a texture the set does not have). The
/// texture is asked for in the background if it has not arrived.
pub fn icon_image(store: &UiStore, name: &str) -> Result<ExtImage, &'static str> {
    let (atlas, index) = parse_icon(name).ok_or("not an icon name")?;
    let row = choose_row(store.index().atlas_rows(atlas)).ok_or("the set has no such atlas")?;
    let (per_row, per_col) = (row.icons_per_row, row.icons_per_column);
    if index >= per_row.saturating_mul(per_col) {
        return Err("the index is past the atlas's cells");
    }
    // The size of the image (the index's), not of the stored variant: the renderer maps 0..1 onto the stored one.
    let info = store
        .texture_info(&row.file)
        .ok_or("the set has not the atlas's texture")?;
    if info.width == 0 || info.height == 0 {
        return Err("the atlas's texture has no size");
    }
    let size = row.icon_size as f32;
    let (w, h) = (info.width as f32, info.height as f32);
    let (c, r) = ((index % per_row) as f32, (index / per_row) as f32);
    Ok(ExtImage {
        texture: ExtTexture(info.id.0),
        uv: [
            c * size / w,
            r * size / h,
            ((c + 1.0) * size / w).min(1.0),
            ((r + 1.0) * size / h).min(1.0),
        ],
    })
}

/// `font:<ICON_NAME>`: the name of a glyph of the game's icon fonts (the pictures the original draws inside texts,
/// `[ICON_GOLD]`); `None` for any other name.
pub fn parse_font_icon(name: &str) -> Option<&str> {
    let icon = name.strip_prefix("font:")?;
    (icon.starts_with("ICON_")
        && icon.len() > 5
        && icon.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
    .then_some(icon)
}

/// The icon fonts of the set, read on the first `font:` picture: the font file and glyph code of each icon name
/// (`IconFontMapping`), and the parsed `.ggxml` of each font file.
#[derive(Default)]
struct IconFonts {
    /// Icon name to (font file stem, glyph code).
    icons: BTreeMap<String, (String, u32)>,
    fonts: BTreeMap<String, Option<ui_text::Font>>,
}

impl IconFonts {
    fn read(store: &UiStore) -> IconFonts {
        let Some(tables) = store
            .bytes(asset_format::UI_TABLES_ID)
            .and_then(|b| asset_format::UiTables::decode(&b).ok())
        else {
            return IconFonts::default();
        };
        let files: BTreeMap<&str, &str> = tables
            .icon_textures
            .iter()
            .map(|(k, f)| (k.as_str(), f.as_str()))
            .collect();
        let icons = tables
            .icons
            .iter()
            .filter_map(|i| {
                Some((
                    i.name.clone(),
                    ((*files.get(i.texture.as_str())?).to_owned(), i.glyph),
                ))
            })
            .collect();
        IconFonts {
            icons,
            fonts: BTreeMap::new(),
        }
    }

    /// The glyph's texture and its rectangle in it.
    fn image(&mut self, store: &UiStore, icon: &str) -> Result<ExtImage, &'static str> {
        let (stem, code) = self
            .icons
            .get(icon)
            .cloned()
            .ok_or("the set has no such font icon")?;
        let font = self
            .fonts
            .entry(stem.clone())
            .or_insert_with(|| {
                let text = store.text_of_name(&format!("{stem}.ggxml"))?;
                ui_text::Font::parse(&stem, &text).ok()
            })
            .as_ref()
            .ok_or("the icon font is not in the set")?;
        if !font.has_glyph(code) {
            return Err("the icon font has no such glyph");
        }
        let layer = font.style(0).map_or(0, |s| s.layers[0]);
        let texture = usize::try_from(layer)
            .ok()
            .and_then(|i| font.textures().get(i))
            .or_else(|| font.textures().first())
            .ok_or("the icon font has no texture")?;
        let info = store
            .texture_info(&texture.src)
            .ok_or("the set has not the icon font's texture")?;
        Ok(ExtImage {
            texture: ExtTexture(info.id.0),
            uv: font.glyph(code).uv,
        })
    }
}

/// At most this many names without a picture are logged.
const LOGGED_MISSES: usize = 32;

/// The skins' resolver of game pictures over the session's UI set (which may come after the skin).
#[derive(Clone, Default)]
pub struct GameIcons {
    store: Rc<RefCell<Option<Rc<UiStore>>>>,
    aliases: Rc<RefCell<BTreeMap<String, String>>>,
    /// The textures pictures named so far.
    named: Rc<RefCell<BTreeSet<u64>>>,
    /// Names resolved and names the set has no picture for.
    resolved: Rc<Cell<u64>>,
    missed: Rc<Cell<u64>>,
    /// The names without a picture logged so far.
    logged: Rc<RefCell<BTreeSet<String>>>,
    /// The icon fonts of the current set (`font:` names), read on first use.
    fonts: Rc<RefCell<Option<IconFonts>>>,
}

impl GameIcons {
    /// The set to read from (the session's, once it loaded). `true`: it changed, and the pictures the skin could
    /// not find before may be found now (the host sets the resolver again, which paints them again).
    pub fn set_store(&self, store: Option<Rc<UiStore>>) -> bool {
        let mut cur = self.store.borrow_mut();
        let same = match (&*cur, &store) {
            (Some(a), Some(b)) => Rc::ptr_eq(a, b),
            (a, b) => a.is_none() && b.is_none(),
        };
        if !same {
            *cur = store;
            *self.fonts.borrow_mut() = None;
        }
        !same
    }

    /// Namespaced numeric ids and database type names from the server's catalog.
    pub fn set_catalog(&self, catalog: Option<&protocol::Catalog>) -> bool {
        let mut aliases = BTreeMap::new();
        if let Some(c) = catalog {
            for (ns, rows) in [
                ("tech", &c.techs),
                ("unit", &c.units),
                ("building", &c.buildings),
                ("policy", &c.policies),
                ("resource", &c.resources),
                ("civ", &c.civs),
                ("leader", &c.leaders),
                ("promotion", &c.promotions),
                ("improvement", &c.improvements),
            ] {
                for row in rows {
                    if row.art.icon_atlas.is_empty() || row.art.icon_index < 0 {
                        continue;
                    }
                    let target = format!("icon:{}:{}", row.art.icon_atlas, row.art.icon_index);
                    aliases.insert(format!("{ns}:{}", row.id), target.clone());
                    if !row.type_name.is_empty() {
                        aliases.insert(format!("{ns}:{}", row.type_name), target);
                    }
                }
            }
        }
        let mut old = self.aliases.borrow_mut();
        if *old == aliases {
            return false;
        }
        *old = aliases;
        self.logged.borrow_mut().clear();
        true
    }

    /// The resolver for a skin's runtime.
    pub fn resolver(&self) -> AssetResolver {
        let me = self.clone();
        Rc::new(move |name: &str| me.resolve(name))
    }

    fn resolve(&self, name: &str) -> Option<ExtImage> {
        let found = match self.store.borrow().as_ref() {
            Some(s) => match parse_font_icon(name) {
                Some(icon) => {
                    let mut fonts = self.fonts.borrow_mut();
                    fonts
                        .get_or_insert_with(|| IconFonts::read(s))
                        .image(s, icon)
                }
                None => {
                    let aliases = self.aliases.borrow();
                    icon_image(s, aliases.get(name).map_or(name, String::as_str))
                }
            },
            None => Err("the UI set is not loaded yet"),
        };
        match found {
            Ok(img) => {
                self.resolved.set(self.resolved.get() + 1);
                self.named.borrow_mut().insert(img.texture.0);
                Some(img)
            }
            Err(why) => {
                self.missed.set(self.missed.get() + 1);
                let mut logged = self.logged.borrow_mut();
                if logged.len() < LOGGED_MISSES && logged.insert(name.to_owned()) {
                    tracing::info!(target: "ui2.assets", "no game picture {name}: {why}");
                }
                None
            }
        }
    }

    /// Whether the set is here (pictures named from now on are found, or missing for good).
    pub fn has_store(&self) -> bool {
        self.store.borrow().is_some()
    }

    /// The textures pictures named so far.
    pub fn named(&self) -> Vec<u64> {
        self.named.borrow().iter().copied().collect()
    }

    /// Names resolved and names without a picture so far.
    pub fn counts(&self) -> (u64, u64) {
        (self.resolved.get(), self.missed.get())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use asset_format::{UiFileInfo, UiFileKind, UiIndex, UiLayerInfo, UiTexDim};

    fn file(name: &str) -> UiFileInfo {
        UiFileInfo {
            name: name.to_owned(),
            id: format!("ui:tx:{name}"),
            kind: UiFileKind::Texture,
            layer: 0,
            size: 1,
            path: format!("UI/{name}"),
            hidden: 0,
            index_of: None,
        }
    }

    fn atlas(size: u32, file: &str, per_row: u32, per_col: u32) -> UiAtlas {
        UiAtlas {
            atlas: "UNIT_ATLAS_1".into(),
            icon_size: size,
            file: file.into(),
            icons_per_row: per_row,
            icons_per_column: per_col,
        }
    }

    /// A set with the unit atlas at 64, 128 and 256 px (8 by 4 icons; the 128 one has a size in the index that is
    /// not 8 x 128 by 4 x 128, as a padded sheet), and an atlas whose texture the set lacks.
    fn store() -> Rc<UiStore> {
        let index = UiIndex {
            layers: vec![UiLayerInfo {
                name: "base".into(),
                dir: String::new(),
                priority: 0,
                order: 0,
                packages: vec![],
                priority_defaulted: false,
            }],
            files: vec![
                file("units128.dds"),
                file("units256.dds"),
                file("units64.dds"),
            ],
            tex_dims: vec![UiTexDim {
                file: "units128.dds".into(),
                width: 1024,
                height: 1024,
            }],
            atlases: vec![
                atlas(64, "units64.dds", 8, 4),
                atlas(256, "units256.dds", 8, 4),
                atlas(128, "units128.dds", 8, 4),
                UiAtlas {
                    atlas: "GONE".into(),
                    ..atlas(128, "gone.dds", 8, 8)
                },
            ],
            ..UiIndex::default()
        };
        UiStore::new(index, Vec::new()).expect("store")
    }

    #[test]
    fn every_game_namespace_accepts_atlas_cells_and_catalog_aliases_resolve() {
        let namespaces = [
            "tech",
            "unit",
            "building",
            "policy",
            "belief",
            "religion",
            "resource",
            "civ",
            "leader",
            "promotion",
            "improvement",
            "great_work",
            "icon",
        ];
        let icons = GameIcons::default();
        icons.set_store(Some(store()));
        let resolve = icons.resolver();
        for ns in namespaces {
            assert!(resolve(&format!("{ns}:UNIT_ATLAS_1:9")).is_some(), "{ns}");
        }
        let entry = protocol::CatalogEntry {
            id: 5,
            type_name: "UNIT_SAMPLE".into(),
            art: protocol::ArtRefs {
                icon_atlas: "UNIT_ATLAS_1".into(),
                icon_index: 9,
                ..Default::default()
            },
            ..Default::default()
        };
        let catalog = protocol::Catalog {
            units: vec![entry],
            ..Default::default()
        };
        assert!(icons.set_catalog(Some(&catalog)));
        assert!(!icons.set_catalog(Some(&catalog)));
        assert_eq!(resolve("unit:5"), resolve("unit:UNIT_SAMPLE"));
        assert!(resolve("unit:5").is_some());
        resolve("unit:missing");
        resolve("unit:missing");
        assert_eq!(icons.logged.borrow().len(), 1);
        assert!(icons.set_catalog(None));
        assert!(resolve("unit:5").is_none());
    }

    #[test]
    fn font_icon_names_are_read() {
        assert_eq!(parse_font_icon("font:ICON_GOLD"), Some("ICON_GOLD"));
        assert_eq!(parse_font_icon("font:ICON_RES_IRON"), Some("ICON_RES_IRON"));
        for bad in [
            "font:",
            "font:ICON_",
            "font:GOLD",
            "font:ICON_A:1",
            "icon:ICON_GOLD",
            "ICON_GOLD",
        ] {
            assert_eq!(parse_font_icon(bad), None, "{bad:?}");
        }
        // Without an icon table in the set, a font icon has no picture (and is not taken for an atlas cell).
        let icons = GameIcons::default();
        icons.set_store(Some(store()));
        assert!(icons.resolver()("font:ICON_GOLD").is_none());
        assert_eq!(icons.counts(), (0, 1));
    }

    #[test]
    fn icon_names_are_read() {
        assert_eq!(
            parse_icon("icon:UNIT_ATLAS_1:12"),
            Some(("UNIT_ATLAS_1", 12))
        );
        assert_eq!(parse_icon("icon:A:B:3"), Some(("A:B", 3)));
        for bad in [
            "",
            "icon:",
            "icon::3",
            "icon:A",
            "icon:A:-1",
            "icon:A:x",
            "ui:tx:a.dds",
        ] {
            assert_eq!(parse_icon(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_128_px_row_is_taken_else_the_nearest_bigger_else_the_biggest() {
        let s = store();
        let rows: Vec<UiAtlas> = s.index().atlas_rows("UNIT_ATLAS_1").cloned().collect();
        assert_eq!(choose_row(rows.iter()).map(|r| r.icon_size), Some(128));
        let small: Vec<UiAtlas> = rows.iter().filter(|r| r.icon_size == 64).cloned().collect();
        assert_eq!(choose_row(small.iter()).map(|r| r.icon_size), Some(64));
        let big: Vec<UiAtlas> = rows
            .iter()
            .filter(|r| r.icon_size != 128)
            .cloned()
            .collect();
        assert_eq!(choose_row(big.iter()).map(|r| r.icon_size), Some(256));
        assert!(choose_row(std::iter::empty()).is_none());
    }

    #[test]
    fn an_icon_is_its_cell_of_the_atlas_texture_and_the_texture_is_asked_for() {
        let s = store();
        let id = s.texture_id_of_name("units128.dds").expect("id");
        // Index 9: column 1, row 1 of 8 x 4 cells of 128 px on a 1024 x 1024 image.
        let img = icon_image(&s, "icon:UNIT_ATLAS_1:9").expect("a picture");
        assert_eq!(img.texture, ExtTexture(id.0));
        assert_eq!(img.uv, [0.125, 0.125, 0.25, 0.25]);
        assert!(s.is_loading(id), "the texture is fetched in the background");
        // The last cell; past it nothing.
        let last = icon_image(&s, "icon:UNIT_ATLAS_1:31").expect("the last cell");
        assert_eq!(last.uv, [0.875, 0.375, 1.0, 0.5]);
        assert_eq!(
            icon_image(&s, "icon:UNIT_ATLAS_1:32"),
            Err("the index is past the atlas's cells")
        );
        assert_eq!(
            icon_image(&s, "icon:NO_ATLAS:0"),
            Err("the set has no such atlas")
        );
        assert_eq!(
            icon_image(&s, "icon:GONE:0"),
            Err("the set has not the atlas's texture")
        );
        assert_eq!(icon_image(&s, "render:minimap"), Err("not an icon name"));
    }

    #[test]
    fn the_resolver_waits_for_the_set_and_counts_what_it_named() {
        let icons = GameIcons::default();
        let resolve = icons.resolver();
        assert_eq!(resolve("icon:UNIT_ATLAS_1:0"), None, "no set yet");
        assert!(!icons.set_store(None), "no change");
        assert!(!icons.has_store());
        let s = store();
        assert!(icons.set_store(Some(Rc::clone(&s))), "the set came");
        assert!(icons.has_store());
        assert!(!icons.set_store(Some(Rc::clone(&s))), "the same set");
        let img = resolve("icon:UNIT_ATLAS_1:0").expect("a picture");
        assert_eq!(img.uv, [0.0, 0.0, 0.125, 0.125]);
        resolve("icon:UNIT_ATLAS_1:1");
        resolve("icon:NO_ATLAS:1");
        assert_eq!(icons.named(), [img.texture.0]);
        assert_eq!(icons.counts(), (2, 2));
        assert_eq!(icons.logged.borrow().len(), 2, "each miss logged once");
    }
}
