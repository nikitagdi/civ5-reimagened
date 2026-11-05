//! The `trade` ribbon of the bridge (card R5.9): `WorldView::trade_routes` -> the lines of the
//! renderer's `Trade` subsystem, as the [`TradeFeed`] (R4.10). It reads the sections `CITIES` (whose
//! city lies at the end of a route), `PLAYERS` (the colours) and `TRADE_ROUTES`; the active player
//! (`WorldView::human`) never changes in a game, and a new game runs every feed whole.
//!
//! The view says which routes there are; their plots come from the replica core (`Q_TRADE_PATHS`, the path
//! the DLL's trade route path finder laid, which the caravan walks and the exe's `TradeVisSystem` draws), handed
//! in by [`TradeFeed::set_core_paths`]. A path belongs to the route whose cities are its ends. A route the core
//! has no path for (no replica, or the replica has not yet replayed the route's creation) is the shortest hex
//! line between the cities (SIMP-069, `render::world::trade::hex_line`) until the path comes. The protocol
//! carries no connection type either: a route to a city of another civilization is international
//! (`arrow.dds`), one to an own city is drawn with the food texture (`arrow_food.dds`) because food
//! and production cannot be told apart.

use client_core::ViewChange;
use exe_api::PopupRoute;
use protocol::{Catalog, SectionMask, WorldView};
use render::Renderer;
use render::world::trade::{TradeKind, TradeRoute, hex_line};

use super::{FeedCx, ViewFeed, mask, section};

/// A path of the core: the plots `(x, y)` from the origin city to the destination city.
pub type CorePath = Vec<(i32, i32)>;

/// The path of the core whose ends are the two cities, if there is one.
fn core_path_of(paths: &[CorePath], from: (i32, i32), to: (i32, i32)) -> Option<&CorePath> {
    paths
        .iter()
        .find(|p| p.len() >= 2 && p.first() == Some(&from) && p.last() == Some(&to))
}

/// The core's plots run in `0..width`; over the seam of a map that wraps along X a step jumps across the whole
/// map. The renderer wants one run (as `hex_line` lays it): every column after the first goes to the copy of the
/// map nearest the column before it, so the columns may leave `0..width`.
fn unrolled(path: &[(i32, i32)], width: i32, wrap_x: bool) -> Vec<(i32, i32)> {
    if !wrap_x || width <= 0 {
        return path.to_vec();
    }
    let mut out: Vec<(i32, i32)> = Vec::with_capacity(path.len());
    for &(x, y) in path {
        let x = match out.last() {
            Some(&(px, _)) => [x - width, x, x + width]
                .into_iter()
                .min_by_key(|c| (c - px).abs())
                .unwrap_or(x),
            None => x,
        };
        out.push((x, y));
    }
    out
}

/// What the feed keeps of a view: the routes with everything but their plots, and the map's shape.
#[derive(Debug, Clone)]
struct ViewRoutes {
    active: i8,
    width: i32,
    wrap_x: bool,
    /// Each route (no plots yet) with its two cities' plots.
    routes: Vec<(TradeRoute, Plot, Plot)>,
    /// The players' primary colours (the popup route has the colour of its owner).
    colours: Vec<(i8, [u8; 3])>,
}

/// A plot `(x, y)`.
type Plot = (i32, i32);

impl ViewRoutes {
    /// Every route on the core's path when `paths` has one for it, else on the hex line between its cities.
    fn lay(&self, paths: &[CorePath]) -> (i8, Vec<TradeRoute>) {
        let routes = self
            .routes
            .iter()
            .map(|(r, from, to)| TradeRoute {
                plots: match core_path_of(paths, *from, *to) {
                    Some(p) => unrolled(p, self.width, self.wrap_x),
                    // SIMPLIFIED(SIMP-069): no path of the core yet: a hex line between the cities stands in.
                    None => hex_line(*from, *to, self.width, self.wrap_x),
                },
                ..r.clone()
            })
            .collect();
        (self.active, routes)
    }

    /// The popup route the exe shows, as the renderer draws it (`TradeVisSystem::TogglePopup_Handler`).
    fn popup(&self, sel: &PopupRoute, paths: &[CorePath], ids: &[i32]) -> Option<TradeRoute> {
        match sel {
            PopupRoute::Existing(id) => {
                let path = paths.get(ids.iter().position(|i| i == id)?)?;
                let (r, _, _) = self
                    .routes
                    .iter()
                    .find(|(_, from, to)| path.first() == Some(from) && path.last() == Some(to))?;
                Some(TradeRoute {
                    plots: unrolled(path, self.width, self.wrap_x),
                    ..r.clone()
                })
            }
            PopupRoute::Temp { route, dest } => {
                let mut plots: Vec<Plot> = route.plots.clone();
                // The fork's event carries every plot of the path (b10); only an old core's 9-plot event is
                // completed (SIMPLIFIED(SIMP-exe-921)) by the hex line from the 9th plot to the asked plot.
                if route.count > route.plots.len() as i32
                    && let (Some(&last), Some(dest)) = (plots.last(), dest)
                {
                    plots.extend(
                        hex_line(last, *dest, self.width, self.wrap_x)
                            .into_iter()
                            .skip(1),
                    );
                }
                let owner = route.player as i8;
                Some(TradeRoute {
                    owner,
                    color: self
                        .colours
                        .iter()
                        .find(|(p, _)| *p == owner)
                        .map_or([255; 3], |c| c.1),
                    kind: match route.kind {
                        1 => TradeKind::Food,
                        2 => TradeKind::Production,
                        _ => TradeKind::International,
                    },
                    plots: unrolled(&plots, self.width, self.wrap_x),
                })
            }
        }
    }

    fn without_path(&self, paths: &[CorePath]) -> usize {
        self.routes
            .iter()
            .filter(|(_, from, to)| core_path_of(paths, *from, *to).is_none())
            .count()
    }
}

/// What the renderer gets: the active player and every route of the view, each on the core's path when
/// `paths` has one for it, else on the hex line between its cities.
pub fn trade_input(world: &WorldView, paths: &[CorePath]) -> (i8, Vec<TradeRoute>) {
    view_routes(world).lay(paths)
}

fn view_routes(world: &WorldView) -> ViewRoutes {
    let colour_of = |owner: i8| {
        world
            .players
            .iter()
            .find(|p| p.id == owner)
            .map_or([255; 3], |p| {
                let [r, g, b, _] = p.color_primary.to_be_bytes();
                [r, g, b]
            })
    };
    let routes = world
        .trade_routes
        .iter()
        .map(|r| {
            let (from, to) = (
                (i32::from(r.from.0), i32::from(r.from.1)),
                (i32::from(r.to.0), i32::from(r.to.1)),
            );
            let abroad = world
                .cities
                .iter()
                .find(|c| (i32::from(c.x), i32::from(c.y)) == to)
                .is_some_and(|c| c.owner != r.owner);
            let route = TradeRoute {
                owner: r.owner,
                color: colour_of(r.owner),
                kind: if abroad {
                    TradeKind::International
                } else {
                    TradeKind::Food
                },
                plots: Vec::new(),
            };
            (route, from, to)
        })
        .collect();
    ViewRoutes {
        colours: world
            .players
            .iter()
            .map(|p| {
                let [r, g, b, _] = p.color_primary.to_be_bytes();
                (p.id, [r, g, b])
            })
            .collect(),
        active: world.human,
        width: i32::from(world.map.width),
        wrap_x: world.map.wrap_x,
        routes,
    }
}

/// The feed of the trade route lines. The renderer compares the list with the last one and does
/// nothing when they are equal.
#[derive(Debug, Default)]
pub struct TradeFeed {
    pending: Option<(i8, Vec<TradeRoute>)>,
    /// The routes of the last view, to lay them again when the core's paths change.
    view: Option<ViewRoutes>,
    /// The paths of the replica core, as the last [`set_core_paths`](Self::set_core_paths) gave them.
    paths: Vec<CorePath>,
    /// The `TradeConnection` index of each of `paths`.
    ids: Vec<i32>,
    /// What the exe shows as the popup route, and what the renderer is to be given for it.
    popup_sel: Option<PopupRoute>,
    popup: Option<TradeRoute>,
    popup_pending: bool,
}

impl TradeFeed {
    /// The popup route the exe shows now. True when what is drawn for it changed (the next `flush` hands it over).
    pub fn set_popup(&mut self, sel: Option<PopupRoute>) -> bool {
        if sel == self.popup_sel {
            return false;
        }
        self.popup_sel = sel;
        self.refresh_popup()
    }

    /// The `TradeConnection` index of each of the paths given to `set_core_paths`.
    pub fn set_core_ids(&mut self, ids: Vec<i32>) -> bool {
        if ids == self.ids {
            return false;
        }
        self.ids = ids;
        self.refresh_popup()
    }

    fn refresh_popup(&mut self) -> bool {
        let now = match (&self.popup_sel, &self.view) {
            (Some(sel), Some(view)) => view.popup(sel, &self.paths, &self.ids),
            _ => None,
        };
        if now == self.popup {
            return false;
        }
        self.popup = now;
        self.popup_pending = true;
        true
    }

    /// The popup route as drawn (tests).
    pub fn popup(&self) -> Option<&TradeRoute> {
        self.popup.as_ref()
    }

    /// The input that waits for the next `flush` (tests).
    pub fn pending(&self) -> Option<&(i8, Vec<TradeRoute>)> {
        self.pending.as_ref()
    }

    /// The trade route paths of the replica core (`Q_TRADE_PATHS`). True when they changed and the routes
    /// were laid again (the feed then has something for the next `flush`).
    pub fn set_core_paths(&mut self, paths: Vec<CorePath>) -> bool {
        if paths == self.paths {
            return false;
        }
        self.paths = paths;
        self.refresh_popup();
        match &self.view {
            Some(view) => {
                self.pending = Some(view.lay(&self.paths));
                true
            }
            None => false,
        }
    }

    /// Routes of the last view that are still drawn on a hex line (the core had no path for them).
    pub fn routes_without_path(&self) -> usize {
        self.view
            .as_ref()
            .map_or(0, |v| v.without_path(&self.paths))
    }
}

impl ViewFeed for TradeFeed {
    fn sections(&self) -> SectionMask {
        mask(&[section::CITIES, section::PLAYERS, section::TRADE_ROUTES])
    }

    fn on_catalog(&mut self, _catalog: &Catalog) {}

    fn apply(&mut self, view: &WorldView, _change: &ViewChange, _cx: &mut FeedCx<'_>) {
        let routes = view_routes(view);
        self.pending = Some(routes.lay(&self.paths));
        self.view = Some(routes);
        self.refresh_popup();
    }

    fn flush(&mut self, renderer: &mut Renderer) {
        if let Some((active, routes)) = self.pending.take() {
            renderer.set_trade_routes(active, routes);
        }
        if std::mem::take(&mut self.popup_pending) {
            renderer.set_trade_popup(self.popup.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use protocol::{CityView, TradeRouteView};

    use super::*;
    use crate::bridge::tests::fixture_world;

    fn route(owner: i8, from: (i16, i16), to: (i16, i16)) -> TradeRouteView {
        TradeRouteView {
            owner,
            from,
            to,
            domain: 0,
        }
    }

    #[test]
    fn a_route_without_a_core_path_is_a_hex_line_between_its_cities_in_the_owners_colour() {
        let (mut world, _) = fixture_world();
        let me = world.human;
        world.trade_routes = vec![route(me, (3, 3), (8, 3))];
        let (active, routes) = trade_input(&world, &[]);
        assert_eq!(active, me);
        assert_eq!(routes.len(), 1);
        let r = &routes[0];
        assert_eq!(
            (r.owner, r.plots.first(), r.plots.last()),
            (me, Some(&(3, 3)), Some(&(8, 3)))
        );
        assert_eq!(r.plots.len(), 6);
        let p = world.players.iter().find(|p| p.id == me).unwrap();
        let [cr, cg, cb, _] = p.color_primary.to_be_bytes();
        assert_eq!(r.color, [cr, cg, cb]);
    }

    /// The live report (2026-10-31): the line went straight over a mountain, while the caravan walks the
    /// path of the DLL's trade route path finder. With the core's path the line is that path, plot by plot.
    #[test]
    fn a_route_takes_the_path_of_the_core_and_not_the_straight_line() {
        let (mut world, _) = fixture_world();
        let me = world.human;
        world.trade_routes = vec![route(me, (3, 3), (8, 3))];
        // Around a mountain at (5, 3)..(6, 3): down a row and back up.
        let around: CorePath = vec![(3, 3), (4, 3), (4, 4), (5, 4), (6, 4), (7, 3), (8, 3)];
        let other: CorePath = vec![(1, 1), (2, 1)];
        let (_, routes) = trade_input(&world, &[other, around.clone()]);
        assert_eq!(routes[0].plots, around);
        assert!(!routes[0].plots.contains(&(5, 3)));
        // The straight line would have crossed it.
        let (_, straight) = trade_input(&world, &[]);
        assert!(
            straight[0].plots.contains(&(5, 3)),
            "{:?}",
            straight[0].plots
        );
    }

    /// A path whose ends are other cities does not stand in for the route.
    #[test]
    fn only_a_path_between_the_routes_cities_is_taken() {
        let (mut world, _) = fixture_world();
        world.trade_routes = vec![route(world.human, (3, 3), (8, 3))];
        let reversed: CorePath = vec![(8, 3), (7, 3), (6, 3), (5, 3), (4, 3), (3, 3)];
        let (_, routes) = trade_input(&world, &[reversed]);
        assert_eq!(routes[0].plots.first(), Some(&(3, 3)));
        assert_eq!(routes[0].plots.len(), 6);
    }

    /// The core's paths may come after the view (the replica replays the route's creation later): the
    /// feed lays the routes again then, and says nothing when the paths did not change.
    #[test]
    fn new_core_paths_lay_the_routes_of_the_last_view_again() {
        let (mut world, _) = fixture_world();
        world.trade_routes = vec![route(world.human, (3, 3), (8, 3))];
        let mut feed = TradeFeed::default();
        assert!(
            !feed.set_core_paths(vec![vec![(0, 0), (1, 0)]]),
            "no view yet"
        );
        let mut camera = render::GameCamera::default();
        let mut cx = FeedCx {
            basis: None,
            camera: &mut camera,
        };
        feed.apply(&world, &ViewChange::all(), &mut cx);
        assert_eq!(feed.routes_without_path(), 1);
        assert_eq!(feed.pending().unwrap().1[0].plots.len(), 6);
        feed.pending = None;
        let around: CorePath = vec![(3, 3), (4, 4), (5, 4), (6, 4), (7, 4), (8, 3)];
        assert!(feed.set_core_paths(vec![around.clone()]));
        assert_eq!(feed.pending().unwrap().1[0].plots, around);
        assert_eq!(feed.routes_without_path(), 0);
        feed.pending = None;
        assert!(!feed.set_core_paths(vec![around]));
        assert!(feed.pending().is_none());
    }

    /// The popup route of the caravan's target choice: the core's 9 plots, then the hex line to the asked plot; the
    /// colour of the owner; whoever owns it. A selected trade unit's route is the core's path of its index.
    #[test]
    fn the_popup_route_is_the_temp_route_completed_or_the_path_of_an_index() {
        use exe_api::TempRoute;
        let (mut world, _) = fixture_world();
        let me = world.human;
        world.trade_routes = vec![route(me, (3, 3), (8, 3))];
        let mut feed = TradeFeed::default();
        let mut camera = render::GameCamera::default();
        let mut cx = FeedCx {
            basis: None,
            camera: &mut camera,
        };
        feed.apply(&world, &ViewChange::all(), &mut cx);
        // Short route: all the core's plots.
        let temp = |count, plots: Vec<(i32, i32)>| PopupRoute::Temp {
            route: TempRoute {
                player: i32::from(me),
                kind: 2,
                count,
                plots,
            },
            dest: Some((20, 3)),
        };
        assert!(feed.set_popup(Some(temp(2, vec![(3, 3), (4, 3)]))));
        let p = feed.popup().unwrap();
        assert_eq!(
            (p.kind, p.plots.clone()),
            (TradeKind::Production, vec![(3, 3), (4, 3)])
        );
        let pl = world.players.iter().find(|p| p.id == me).unwrap();
        let [r, g, b, _] = pl.color_primary.to_be_bytes();
        assert_eq!(p.color, [r, g, b]);
        // A long one (the event carried 9 of 14): the line to the asked plot goes on from the 9th.
        let nine: Vec<_> = (3..12).map(|x| (x, 3)).collect();
        assert!(feed.set_popup(Some(temp(14, nine))));
        let p = feed.popup().unwrap();
        assert_eq!(
            (p.plots.first(), p.plots.last()),
            (Some(&(3, 3)), Some(&(20, 3)))
        );
        assert!(p.plots.len() > 9);
        // The whole route in the event (14 of 14): drawn as the core gave it, no line added.
        let full: Vec<_> = (3..17).map(|x| (x, 3)).collect();
        assert!(feed.set_popup(Some(temp(14, full))));
        let p = feed.popup().unwrap();
        assert_eq!((p.plots.len(), p.plots.last()), (14, Some(&(16, 3))));
        // An existing route by index: the core's path whose ends are the route's cities.
        let around: CorePath = vec![(3, 3), (4, 4), (5, 4), (8, 3)];
        feed.set_core_paths(vec![around.clone()]);
        feed.set_core_ids(vec![7]);
        assert!(feed.set_popup(Some(PopupRoute::Existing(7))));
        assert_eq!(feed.popup().unwrap().plots, around);
        assert_eq!(feed.popup().unwrap().owner, me);
        // An index without a path, and none at all, draw nothing.
        assert!(feed.set_popup(Some(PopupRoute::Existing(9))));
        assert!(feed.popup().is_none());
        assert!(!feed.set_popup(None) || feed.popup().is_none());
    }

    /// The live report (2026-11-03, "the preview is red"): Vietnam's primary colour is `COLOR_PLAYER_VIET_ICON`, a
    /// dark red, and its secondary (the green of the city banners) is the background. The exe draws the route in the
    /// owner's primary colour (`GetColorTypePrimary`), so the red is the owner's colour, not a swapped channel or
    /// another owner: pinned here from the player's packed `0xRRGGBBAA` to the vertex bytes `R, G, B, A`.
    #[test]
    fn the_popup_route_is_the_owners_primary_colour_and_not_the_banners_secondary() {
        use exe_api::TempRoute;
        let (mut world, _) = fixture_world();
        let me = world.human;
        let other = me + 1;
        for p in &mut world.players {
            if p.id == me {
                p.color_primary = 0x80_04_1C_FF;
                p.color_secondary = 0x01_CC_7E_FF;
            } else if p.id == other {
                p.color_primary = 0x20_60_C0_FF;
            }
        }
        let mut feed = TradeFeed::default();
        let mut camera = render::GameCamera::default();
        let mut cx = FeedCx {
            basis: None,
            camera: &mut camera,
        };
        feed.apply(&world, &ViewChange::all(), &mut cx);
        let temp = |player: i32| PopupRoute::Temp {
            route: TempRoute {
                player,
                kind: 0,
                count: 2,
                plots: vec![(3, 3), (4, 3)],
            },
            dest: None,
        };
        assert!(feed.set_popup(Some(temp(i32::from(me)))));
        let p = feed.popup().unwrap().clone();
        assert_eq!(
            p.color,
            [0x80, 0x04, 0x1C],
            "the primary, not [0x01, 0xCC, 0x7E]"
        );
        // The vertices carry it as R, G, B at alpha 0.95 (242) and the feather with none.
        let mesh = render::world::trade::build_mesh_with_popup(&[], me, Some(&p), 0.0);
        let solid: Vec<_> = mesh
            .vertices
            .iter()
            .filter(|v| v.color[3] != 0)
            .map(|v| v.color)
            .collect();
        assert!(!solid.is_empty());
        assert!(
            solid.iter().all(|c| *c == [0x80, 0x04, 0x1C, 242]),
            "{solid:?}"
        );
        // The owner is the one the event names, whoever looks.
        assert!(feed.set_popup(Some(temp(i32::from(other)))));
        assert_eq!(feed.popup().unwrap().color, [0x20, 0x60, 0xC0]);
    }

    /// Over the seam the core's columns jump from the last to the first; the line stays one run.
    #[test]
    fn a_core_path_over_the_seam_is_one_run() {
        let (mut world, _) = fixture_world();
        let w = i32::from(world.map.width);
        world.map.wrap_x = true;
        let (a, b) = ((w - 2) as i16, 1i16);
        world.trade_routes = vec![route(world.human, (a, 5), (b, 5))];
        let core: CorePath = vec![(w - 2, 5), (w - 1, 5), (0, 5), (1, 5)];
        let (_, routes) = trade_input(&world, &[core]);
        assert_eq!(
            routes[0].plots,
            vec![(w - 2, 5), (w - 1, 5), (w, 5), (w + 1, 5)]
        );
    }

    #[test]
    fn a_route_to_a_foreign_city_is_international_and_an_own_one_is_food() {
        let (mut world, _) = fixture_world();
        let me = world.human;
        let other = me + 1;
        world.cities = vec![
            CityView {
                id: 1,
                owner: other,
                x: 8,
                y: 3,
                ..CityView::default()
            },
            CityView {
                id: 2,
                owner: me,
                x: 3,
                y: 9,
                ..CityView::default()
            },
        ];
        world.trade_routes = vec![route(me, (3, 3), (8, 3)), route(me, (3, 3), (3, 9))];
        let (_, routes) = trade_input(&world, &[]);
        assert_eq!(routes[0].kind, TradeKind::International);
        assert_eq!(routes[1].kind, TradeKind::Food);
    }

    #[test]
    fn on_a_wrapping_map_the_line_takes_the_short_way() {
        let (mut world, _) = fixture_world();
        let w = world.map.width as i16;
        world.map.wrap_x = true;
        world.trade_routes = vec![route(world.human, (w - 2, 5), (1, 5))];
        let (_, routes) = trade_input(&world, &[]);
        assert_eq!(routes[0].plots.len(), 4, "{:?}", routes[0].plots);
        world.map.wrap_x = false;
        let (_, routes) = trade_input(&world, &[]);
        assert!(routes[0].plots.len() > 4);
    }
}
