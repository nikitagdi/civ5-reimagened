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
use ui2_host::{MapIcon, ModelAsset, V2Ui};

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

/// The host side of the map options of a running UI v2.
#[derive(Default)]
pub struct MapLink {
    kv: Option<Rc<dyn KeyValueStore>>,
    /// The stored options were put into the running skin's state (once per skin).
    restored_for: Option<String>,
    /// The options the renderer and the store follow now.
    applied: Option<MapOptions>,
    /// What the icons were made for: the hex rectangle of the view, the world revision, the switches.
    icons_for: Option<([i32; 4], u64, bool, bool)>,
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
        if !opts.yields && !opts.resources {
            if self.icons_for.take().is_some() {
                ui.set_map_icons(Vec::new());
            }
            return;
        }
        let Some(rect) = rect else {
            return;
        };
        let key = (rect, revision, opts.yields, opts.resources);
        if self.icons_for == Some(key) {
            return;
        }
        self.icons_for = Some(key);
        ui.set_map_icons(map_icons(world, catalog, rect, opts));
    }
}

/// `icon:<atlas>:<index>` of a resource of the catalog.
fn resource_icon(catalog: Option<&Catalog>, id: i16) -> Option<ModelAsset> {
    let e = catalog?.resources.iter().find(|e| e.id == id)?;
    (!e.art.icon_atlas.is_empty() && e.art.icon_index >= 0)
        .then(|| ModelAsset::new(&format!("icon:{}:{}", e.art.icon_atlas, e.art.icon_index)))
}

/// The rows of the revealed plots of `rect` (unwrapped hex rectangle, upper bounds exclusive) that have something to
/// show: their yields while the yield icons are on, their resource (as the seat sees it) while the resource icons are.
pub fn map_icons(
    world: &WorldView,
    catalog: Option<&Catalog>,
    rect: [i32; 4],
    opts: MapOptions,
) -> Vec<MapIcon> {
    let m = &world.map;
    let (w, h) = (i32::from(m.width), i32::from(m.height));
    let mut out = Vec::new();
    if w <= 0 || h <= 0 {
        return out;
    }
    let [x0, y0, x1, y1] = rect;
    let mut seen = std::collections::BTreeSet::new();
    for y in y0.max(0)..y1.min(h) {
        for ux in x0..x1 {
            let x = if m.wrap_x { ux.rem_euclid(w) } else { ux };
            if !(0..w).contains(&x) || !seen.insert((x, y)) {
                continue;
            }
            let Some(p) = m.plots.get((y * w + x) as usize) else {
                continue;
            };
            if !p.flags.contains(PlotFlags::REVEALED) {
                continue;
            }
            let yields = if opts.yields { p.yields } else { [0; 6] };
            let resource = if opts.resources && p.resource >= 0 {
                resource_icon(catalog, p.resource)
            } else {
                None
            };
            if yields.iter().all(|v| *v <= 0) && resource.is_none() {
                continue;
            }
            let y16 = |i: usize| i16::from(yields[i].max(0));
            out.push(MapIcon {
                cell: x * 65_536 + y,
                food: y16(0),
                production: y16(1),
                gold: y16(2),
                science: y16(3),
                culture: y16(4),
                faith: y16(5),
                has_resource: resource.is_some(),
                resource: resource.unwrap_or_default(),
            });
            if out.len() >= MAX_ICONS {
                return out;
            }
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
        let rows = map_icons(&w, Some(&c), [-1, 0, 4, 2], both);
        let cells: Vec<i32> = rows.iter().map(|r| r.cell).collect();
        assert_eq!(
            cells,
            [0, 1],
            "(0,0) and (0,1); the unrevealed and bare plots have none"
        );
        assert_eq!((rows[0].food, rows[0].gold), (2, 1));
        assert!(rows[0].has_resource && &*rows[0].resource.0 == "icon:RESOURCE_ATLAS:7");
        // Resources only: the ocean has nothing to show.
        let res = MapOptions {
            resources: true,
            ..MapOptions::default()
        };
        let rows = map_icons(&w, Some(&c), [0, 0, 3, 2], res);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].food, 0, "the yields are off");
        // Yields only: no resource picture.
        let ys = MapOptions {
            yields: true,
            ..MapOptions::default()
        };
        assert!(
            map_icons(&w, Some(&c), [0, 0, 3, 2], ys)
                .iter()
                .all(|r| !r.has_resource)
        );
    }
}
