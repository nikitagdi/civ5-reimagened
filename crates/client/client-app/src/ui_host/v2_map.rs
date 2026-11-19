//! The map options of UI v2's minimap panel, as the original's `MiniMapPanel.lua` switches them: the hex grid
//! (`UI.SetGridVisibleMode`, the renderer's grid), the trade routes (`g_bShowTradeRoutes`, the renderer's routes), the
//! yield icons (`UI.SetYieldVisibleMode`, YieldIconManager) and the resource icons (`UI.SetResourceVisibleMode`,
//! ResourceIconManager).
//!
//! The switches are the skin's own state (`state.minimap.grid`, `.routes`, `.yields`, `.resources`): the player
//! flips them in the panel, the host follows them every frame. The host keeps them for the player in the page's
//! store (as the classic client keeps its map options) and puts them back into the skin's state when a skin comes up.
//! The grid and the routes are the renderer's; the yield and resource icons are the skin's, hung on the plots in view
//! (`ui.map_icons`, map anchors), as the original's Lua managers hang theirs on the plots the exe names.

use std::rc::Rc;

use platform::KeyValueStore;
use protocol::{Catalog, PlotFlags, WorldView};
use ui2_host::{MapIcon, MapYield, ModelAsset, ModelList, ModelValue, V2Ui};

/// The page store's key of the map options.
pub const STORE_KEY: &str = "ui2.map_options";

/// The most plots with icons at once (a far zoom out shows many plots; the original's managers keep to the view too).
const MAX_ICONS: usize = 600;

/// The four switches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapOptions {
    pub grid: bool,
    pub routes: bool,
    pub yields: bool,
    pub resources: bool,
}

impl Default for MapOptions {
    /// As a new game of the original: the trade routes shown, the grid and the icons not.
    fn default() -> Self {
        MapOptions {
            grid: false,
            routes: true,
            yields: false,
            resources: false,
        }
    }
}

const PATHS: [&str; 4] = [
    "minimap.grid",
    "minimap.routes",
    "minimap.yields",
    "minimap.resources",
];

impl MapOptions {
    fn get(&self) -> [bool; 4] {
        [self.grid, self.routes, self.yields, self.resources]
    }

    fn from(v: [bool; 4]) -> Self {
        MapOptions {
            grid: v[0],
            routes: v[1],
            yields: v[2],
            resources: v[3],
        }
    }

    /// The stored text: `grid=1,routes=1,yields=0,resources=0`.
    pub fn store_text(&self) -> String {
        PATHS
            .iter()
            .zip(self.get())
            .map(|(p, on)| format!("{}={}", &p["minimap.".len()..], u8::from(on)))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// The options of a stored text; a switch it does not name keeps its default, a malformed text is none.
    pub fn parse(text: &str) -> Option<Self> {
        let mut v = MapOptions::default().get();
        for part in text.split(',').filter(|p| !p.trim().is_empty()) {
            let (name, value) = part.split_once('=')?;
            let i = PATHS
                .iter()
                .position(|p| &p["minimap.".len()..] == name.trim())?;
            v[i] = match value.trim() {
                "1" => true,
                "0" => false,
                _ => return None,
            };
        }
        Some(MapOptions::from(v))
    }

    /// The skin's switches; `None` when it does not declare all four (a skin without the panel).
    fn read(ui: &V2Ui) -> Option<Self> {
        let mut v = [false; 4];
        for (i, p) in PATHS.iter().enumerate() {
            v[i] = ui.skin_bool(p)?;
        }
        Some(MapOptions::from(v))
    }

    fn write(&self, ui: &mut V2Ui) {
        for (p, on) in PATHS.iter().zip(self.get()) {
            ui.set_skin_bool(p, on);
        }
    }
}

/// What the renderer has to change for a new set of options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RendererChange {
    pub grid: Option<bool>,
    pub routes: Option<bool>,
}

type IconsKey = ([i32; 4], u64, bool, bool, Vec<(i32, i32)>);

/// The host side of the map options of a running UI v2.
#[derive(Default)]
pub struct MapLink {
    kv: Option<Rc<dyn KeyValueStore>>,
    /// The stored options were put into the running skin's state (once per skin).
    restored_for: Option<String>,
    /// The options the renderer and the store follow now.
    applied: Option<MapOptions>,
    /// What the icons were made for: the hex rectangle of the view, the world revision, the switches, the open city's
    /// plots.
    icons_for: Option<IconsKey>,
}

impl MapLink {
    /// The page store the options are kept in (`None`: kept for the session only).
    pub fn set_store(&mut self, kv: Option<Rc<dyn KeyValueStore>>) {
        self.kv = kv;
    }

    /// A skin came up or the game changed: its state gets the stored options again, the renderer follows anew.
    pub fn reset(&mut self) {
        self.restored_for = None;
        self.applied = None;
        self.icons_for = None;
    }

    /// One frame: the stored options go into a new skin's state, then the skin's switches are followed. Returns what
    /// the renderer must change.
    pub fn follow(&mut self, ui: &mut V2Ui) -> RendererChange {
        let skin = ui.skin_id().to_owned();
        if self.restored_for.as_deref() != Some(&skin) {
            if MapOptions::read(ui).is_none() {
                return RendererChange::default();
            }
            let stored = self
                .kv
                .as_ref()
                .and_then(|kv| kv.get(STORE_KEY))
                .and_then(|t| MapOptions::parse(&t))
                .unwrap_or_default();
            stored.write(ui);
            self.restored_for = Some(skin);
            self.applied = None;
        }
        let Some(now) = MapOptions::read(ui) else {
            return RendererChange::default();
        };
        if self.applied == Some(now) {
            return RendererChange::default();
        }
        let before = self.applied;
        self.applied = Some(now);
        if before.is_some()
            && let Some(kv) = &self.kv
        {
            kv.set(STORE_KEY, &now.store_text());
        }
        RendererChange {
            grid: (before.map(|b| b.grid) != Some(now.grid)).then_some(now.grid),
            routes: (before.map(|b| b.routes) != Some(now.routes)).then_some(now.routes),
        }
    }

    /// The icons of the plots in the hex rectangle `rect` (`m2::visible_cells`), made again when the view, the world
    /// or the switches changed.
    pub fn icons(
        &mut self,
        ui: &mut V2Ui,
        rect: Option<[i32; 4]>,
        world: &WorldView,
        catalog: Option<&Catalog>,
        revision: u64,
    ) {
        let Some(opts) = self.applied else {
            return;
        };
        let forced = city_plots(ui);
        if !opts.yields && !opts.resources && forced.is_empty() {
            if self.icons_for.take().is_some() {
                ui.set_map_icons(Vec::new());
            }
            return;
        }
        let Some(rect) = rect else {
            return;
        };
        let key = (rect, revision, opts.yields, opts.resources, forced);
        if self.icons_for.as_ref() == Some(&key) {
            return;
        }
        ui.set_map_icons(map_icons(world, catalog, rect, opts, &key.4));
        self.icons_for = Some(key);
    }
}

/// The yield atlas of the original's `YieldIconManager.lua` (`g_Textures`, `g_OffsetXs`): food, production, gold and
/// science are columns of `YieldAtlas.dds`, culture and faith have a texture each; a cell is 128 px.
const YIELD_TEXTURES: [(&str, u32); 6] = [
    ("YieldAtlas.dds", 0),
    ("YieldAtlas.dds", 128),
    ("YieldAtlas.dds", 256),
    ("YieldAtlas.dds", 384),
    ("YieldAtlas_128_Culture.dds", 0),
    ("YieldAtlas_128_Faith.dds", 0),
];

/// The pictures of a plot's yields as `BuildAnchorYields` lays them out: one picture per yield above zero, in the order
/// of the yields; the cell of `amount` icons (row `amount - 1`, the fifth row for five and more) and, from six on, the
/// number over it (`YieldAtlas.dds`, rows 640 and 768); the first `m` pictures in the first stack, the rest in the
/// second (`m` is 4 under five yields, else half of them, rounded up).
pub fn yield_pictures(yields: [i8; 6]) -> Vec<MapYield> {
    let shown: Vec<(usize, i16)> = yields
        .iter()
        .enumerate()
        .filter(|(_, v)| **v > 0)
        .map(|(i, v)| (i, i16::from(*v)))
        .collect();
    let n = shown.len();
    let m = if n < 5 { 4 } else { n.div_ceil(2) };
    shown
        .into_iter()
        .enumerate()
        .map(|(slot, (yield_id, amount))| {
            let (file, x) = YIELD_TEXTURES[yield_id];
            let y = if amount > 4 {
                512
            } else {
                128 * (amount as u32 - 1)
            };
            let number = (amount > 5).then(|| {
                let nx = if amount > 12 {
                    256
                } else {
                    128 * ((amount as u32 - 6) % 4)
                };
                let ny = if amount > 9 { 768 } else { 640 };
                ModelAsset::new(&format!("ui:tx:YieldAtlas.dds@{nx},{ny},128,128"))
            });
            MapYield {
                slot: slot as i16,
                stack: if slot < m { 1 } else { 2 },
                amount,
                icon: ModelAsset::new(&format!("ui:tx:{file}@{x},{y},128,128")),
                number,
            }
        })
        .collect()
}

/// `icon:<atlas>:<index>` of a resource of the catalog.
fn resource_icon(catalog: Option<&Catalog>, id: i16) -> Option<ModelAsset> {
    let e = catalog?.resources.iter().find(|e| e.id == id)?;
    (!e.art.icon_atlas.is_empty() && e.art.icon_index >= 0)
        .then(|| ModelAsset::new(&format!("icon:{}:{}", e.art.icon_atlas, e.art.icon_index)))
}

/// The plots of the open city screen (`city.tiles` while `ui.windows.city.open`): the original's city view asks for
/// their yields (`Events.RequestYieldDisplay(YieldDisplayTypes.AREA, ...)` of the city's radius) whatever the options.
fn city_plots(ui: &V2Ui) -> Vec<(i32, i32)> {
    use ModelValue as Value;
    let store = ui.store();
    let field = |path: &str| store.resolve(path).ok().map(|f| f.id);
    let open = field("ui.windows.city.open")
        .and_then(|f| store.value(f, &[]))
        .is_some_and(|v| matches!(v, Value::Bool(true)));
    let (Some(tiles), Some(xf), Some(yf)) = (
        field("city.tiles"),
        field("city.tiles[].x"),
        field("city.tiles[].y"),
    ) else {
        return Vec::new();
    };
    if !open {
        return Vec::new();
    }
    let Some(keys) = store.list_keys(tiles, &[]) else {
        return Vec::new();
    };
    let int = |f, k| match store.value(f, &[k]) {
        Some(Value::I32(v)) => Some(*v),
        _ => None,
    };
    keys.filter_map(|k| Some((int(xf, k)?, int(yf, k)?)))
        .collect()
}

/// The rows of the revealed plots of `rect` (unwrapped hex rectangle, upper bounds exclusive) that have something to
/// show: their yields while the yield icons are on (and always on the plots of `forced`, the open city's), their
/// resource (as the seat sees it) while the resource icons are. The plots of `forced` come first.
pub fn map_icons(
    world: &WorldView,
    catalog: Option<&Catalog>,
    rect: [i32; 4],
    opts: MapOptions,
    forced: &[(i32, i32)],
) -> Vec<MapIcon> {
    let m = &world.map;
    let (w, h) = (i32::from(m.width), i32::from(m.height));
    let mut out = Vec::new();
    if w <= 0 || h <= 0 {
        return out;
    }
    let [x0, y0, x1, y1] = rect;
    let mut seen = std::collections::BTreeSet::new();
    let in_rect = (y0.max(0)..y1.min(h)).flat_map(|y| (x0..x1).map(move |ux| (ux, y)));
    let cells = forced.iter().copied().chain(in_rect);
    for (ux, y) in cells {
        let x = if m.wrap_x { ux.rem_euclid(w) } else { ux };
        if !(0..w).contains(&x) || !(0..h).contains(&y) || !seen.insert((x, y)) {
            continue;
        }
        let Some(p) = m.plots.get((y * w + x) as usize) else {
            continue;
        };
        if !p.flags.contains(PlotFlags::REVEALED) {
            continue;
        }
        let yields = if opts.yields || forced.contains(&(x, y)) {
            p.yields
        } else {
            [0; 6]
        };
        let resource = if opts.resources && p.resource >= 0 {
            resource_icon(catalog, p.resource)
        } else {
            None
        };
        if yields.iter().all(|v| *v <= 0) && resource.is_none() {
            continue;
        }
        let mut list = ModelList::default();
        for pic in yield_pictures(yields) {
            let _ = list.try_push(pic);
        }
        out.push(MapIcon {
            cell: x * 65_536 + y,
            yields: list,
            has_resource: resource.is_some(),
            resource: resource.unwrap_or_default(),
        });
        if out.len() >= MAX_ICONS {
            return out;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{ArtRefs, CatalogEntry, PlotView};

    #[test]
    fn the_stored_text_reads_back_and_keeps_defaults_for_what_it_does_not_name() {
        let o = MapOptions {
            grid: true,
            routes: false,
            yields: true,
            resources: false,
        };
        assert_eq!(o.store_text(), "grid=1,routes=0,yields=1,resources=0");
        assert_eq!(MapOptions::parse(&o.store_text()), Some(o));
        assert_eq!(
            MapOptions::parse("yields=1"),
            Some(MapOptions {
                yields: true,
                ..MapOptions::default()
            })
        );
        assert_eq!(MapOptions::parse(""), Some(MapOptions::default()));
        assert_eq!(MapOptions::parse("grid=2"), None);
        assert_eq!(MapOptions::parse("fog=1"), None);
    }

    /// A 3 x 2 map that wraps: (0, 0) grassland 2/0/1 with iron, (1, 0) unrevealed, (2, 0) bare, (0, 1) ocean 1/0/2.
    fn world() -> (WorldView, Catalog) {
        let mut w = crate::test_world::world();
        let seen = PlotFlags(PlotFlags::REVEALED);
        let plot = |flags: PlotFlags, yields: [i8; 6], resource: i16| PlotView {
            flags,
            yields,
            resource,
            ..PlotView::default()
        };
        w.map.width = 3;
        w.map.height = 2;
        w.map.wrap_x = true;
        w.map.plots = vec![
            plot(seen, [2, 0, 1, 0, 0, 0], 5),
            plot(PlotFlags::default(), [3, 3, 3, 0, 0, 0], 5),
            plot(seen, [0; 6], -1),
            plot(seen, [1, 0, 2, 0, 0, 0], -1),
            plot(seen, [0; 6], -1),
            plot(seen, [0; 6], -1),
        ];
        let catalog = Catalog {
            resources: vec![CatalogEntry {
                id: 5,
                art: ArtRefs {
                    icon_atlas: "RESOURCE_ATLAS".into(),
                    icon_index: 7,
                    ..ArtRefs::default()
                },
                ..CatalogEntry::default()
            }],
            ..Catalog::default()
        };
        (w, catalog)
    }

    #[test]
    fn icons_cover_the_revealed_plots_of_the_view_with_something_to_show() {
        let (w, c) = world();
        let both = MapOptions {
            yields: true,
            resources: true,
            ..MapOptions::default()
        };
        // The view spans one plot past each side of the wrapping map: each plot once.
        let rows = map_icons(&w, Some(&c), [-1, 0, 4, 2], both, &[]);
        let cells: Vec<i32> = rows.iter().map(|r| r.cell).collect();
        assert_eq!(
            cells,
            [0, 1],
            "(0,0) and (0,1); the unrevealed and bare plots have none"
        );
        let amounts: Vec<i16> = rows[0].yields.iter().map(|y| y.amount).collect();
        assert_eq!(amounts, [2, 1], "food 2 and gold 1");
        assert!(rows[0].has_resource && &*rows[0].resource.0 == "icon:RESOURCE_ATLAS:7");
        // Resources only: the ocean has nothing to show.
        let res = MapOptions {
            resources: true,
            ..MapOptions::default()
        };
        let rows = map_icons(&w, Some(&c), [0, 0, 3, 2], res, &[]);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].yields.is_empty(), "the yields are off");
        // Yields only: no resource picture.
        let ys = MapOptions {
            yields: true,
            ..MapOptions::default()
        };
        assert!(
            map_icons(&w, Some(&c), [0, 0, 3, 2], ys, &[])
                .iter()
                .all(|r| !r.has_resource)
        );
    }

    #[test]
    fn the_open_city_shows_the_yields_of_its_plots_whatever_the_options() {
        let (w, c) = world();
        let off = MapOptions::default();
        assert!(map_icons(&w, Some(&c), [0, 0, 3, 2], off, &[]).is_empty());
        // The city's plots: (0,0) and the ocean (0,1); (1,0) is unrevealed, so nothing is shown there.
        let rows = map_icons(&w, Some(&c), [0, 0, 3, 2], off, &[(0, 1), (1, 0), (0, 0)]);
        let cells: Vec<i32> = rows.iter().map(|r| r.cell).collect();
        assert_eq!(cells, [1, 0], "the city's plots first, in their order");
        assert!(rows.iter().all(|r| !r.yields.is_empty() && !r.has_resource));
        // With the yields on, the other plots in view come after the city's.
        let ys = MapOptions {
            yields: true,
            ..MapOptions::default()
        };
        assert_eq!(
            map_icons(&w, Some(&c), [0, 0, 3, 2], ys, &[(0, 1)])[0].cell,
            1
        );
    }

    #[test]
    fn a_yield_picture_is_the_atlas_cell_of_its_amount_as_the_original_lays_it_out() {
        let names = |v: &[MapYield]| -> Vec<(i16, String, Option<String>)> {
            v.iter()
                .map(|y| {
                    (
                        y.stack,
                        y.icon.0.to_string(),
                        y.number.as_ref().map(|n| n.0.to_string()),
                    )
                })
                .collect()
        };
        // Food 1, production 4, gold 5, culture 2: four yields stand in one row.
        let v = yield_pictures([1, 4, 5, 0, 2, 0]);
        assert_eq!(
            names(&v),
            [
                (1, "ui:tx:YieldAtlas.dds@0,0,128,128".into(), None),
                (1, "ui:tx:YieldAtlas.dds@128,384,128,128".into(), None),
                (1, "ui:tx:YieldAtlas.dds@256,512,128,128".into(), None),
                (
                    1,
                    "ui:tx:YieldAtlas_128_Culture.dds@0,128,128,128".into(),
                    None
                ),
            ]
        );
        // Six and more: the fifth row and the number over it (6..9 on row 640, 10 and more on 768, past 12 one cell).
        let v = yield_pictures([6, 9, 10, 13, 0, 1]);
        assert_eq!(
            names(&v),
            [
                (
                    1,
                    "ui:tx:YieldAtlas.dds@0,512,128,128".into(),
                    Some("ui:tx:YieldAtlas.dds@0,640,128,128".into())
                ),
                (
                    1,
                    "ui:tx:YieldAtlas.dds@128,512,128,128".into(),
                    Some("ui:tx:YieldAtlas.dds@384,640,128,128".into())
                ),
                (
                    1,
                    "ui:tx:YieldAtlas.dds@256,512,128,128".into(),
                    Some("ui:tx:YieldAtlas.dds@0,768,128,128".into())
                ),
                (
                    2,
                    "ui:tx:YieldAtlas.dds@384,512,128,128".into(),
                    Some("ui:tx:YieldAtlas.dds@256,768,128,128".into())
                ),
                (2, "ui:tx:YieldAtlas_128_Faith.dds@0,0,128,128".into(), None),
            ]
        );
        assert!(yield_pictures([0; 6]).is_empty());
    }
}
