//! `Trade` subsystem (world-overlays, card R5.9): the trade route lines,
//! `TradeVisSystem` of the original (RW-22 (g), RW-22b, ra6c-1-trade).
//!
//! A route is a list of plots. The original makes it a chain of Hermite pieces (Catmull-Rom
//! tangents), one per hex step, each a strip of 9 stations across four vertices (a solid core of
//! half width 5 and a feather of 0.35 that fades to alpha 0), and ends the chain in a short stub of
//! the last step (the first 0.15 of it) with an arrow head. The vertex is 0x18 bytes: `x`, `y`, `z`
//! (always 0, nothing samples the relief), the colour and `u`, `v`; `v = -2t` runs two repeats of the
//! texture over a step. The colour is the owner's primary colour, with alpha 0.4 for the routes of
//! the active player (`BuildRouteGeometry` 0x089f2983).
//!
//! The mesh is drawn into the map of the overlay ("Terrain Overlay", after the hex overlay) by
//! `Overlay::encode_screen_overlay`, through `TradeArrowShader` (`shaders/overlay/trade.wgsl`): the
//! system keeps the routes and the mesh and draws nothing in a slot of its own, because the map of the
//! overlay is cleared by the overlay's own pass and a system that runs before it could not draw
//! into it. The textures are per route type: `arrow.dds` (international), `arrow_food.dds`,
//! `arrow_prod.dds`, drawn production first (`OverlaySystem::Render` 0x088c659e).
//!
//! The plots of a route are the path of the DLL's trade route path finder, read from the replica core
//! (`Q_TRADE_PATHS`, client-app `bridge::trade`). SIMPLIFIED(SIMP-069): a route the core has given no
//! path for yet is the shortest hex line between its cities ([`hex_line`]) until the path comes.
//!
//! The popup route (`TradeVisuals_ActivatePopupRoute`, alpha 0.95, the `arrow_popup*` textures) is one
//! more route ([`Trade::set_popup`]) drawn last, whoever owns it and even with the display option off
//! (`Update` 0x089f2428, `Render` 0x089f2ec8): the route of the caravan's target choice, of the trade
//! unit selected or under the mouse.

pub mod gpu;

use std::cell::Cell;
use std::ops::Range;

use bytemuck::{Pod, Zeroable};
use glam::Vec2;

use self::gpu::{TradeFrame, TradeGpu};
use crate::frame::{DrawContext, InitContext, PassSlot, PrepareContext, RenderSystem, SystemId};
use crate::hexgrid::cell_center;

/// What a route carries, which picks the texture of its batch (`TradeConnectionType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TradeKind {
    /// To a city of another civilization: `arrow.dds`.
    International,
    /// Food to one of the player's own cities: `arrow_food.dds`.
    Food,
    /// Production to one of the player's own cities: `arrow_prod.dds`.
    Production,
}

impl TradeKind {
    pub const ALL: [TradeKind; 3] = [
        TradeKind::International,
        TradeKind::Food,
        TradeKind::Production,
    ];

    /// The texture of the batch (`TEXTURE_PATHS` 0x0a30ff78, [0..3]).
    pub const fn texture(self) -> &'static str {
        match self {
            TradeKind::International => "tx:arrow.dds",
            TradeKind::Food => "tx:arrow_food.dds",
            TradeKind::Production => "tx:arrow_prod.dds",
        }
    }

    pub(crate) const fn index(self) -> usize {
        match self {
            TradeKind::International => 0,
            TradeKind::Food => 1,
            TradeKind::Production => 2,
        }
    }

    /// The texture of the popup route (`TEXTURE_PATHS` [3..6]: `arrow_popup*.dds`).
    pub const fn popup_texture(self) -> &'static str {
        match self {
            TradeKind::International => "tx:arrow_popup.dds",
            TradeKind::Food => "tx:arrow_popup_food.dds",
            TradeKind::Production => "tx:arrow_popup_prod.dds",
        }
    }

    /// The batch of the popup route of this type (after the three route batches).
    pub(crate) const fn popup_index(self) -> usize {
        3 + self.index()
    }
}

/// The textures of the routes and of the popup route, for the loader.
pub const TEXTURES: [&str; 6] = [
    TradeKind::International.texture(),
    TradeKind::Food.texture(),
    TradeKind::Production.texture(),
    TradeKind::International.popup_texture(),
    TradeKind::Food.popup_texture(),
    TradeKind::Production.popup_texture(),
];

/// One route as the client gives it: the owner, his colour, the type and the plots `(col, row)`
/// from the origin to the destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TradeRoute {
    pub owner: i8,
    /// The owner's primary colour. The original takes it from the player at the start of the game
    /// (`StartupGame` 0x089f2070); white for a player without one.
    pub color: [u8; 3],
    pub kind: TradeKind,
    pub plots: Vec<(i32, i32)>,
}

/// The vertex of the original (`TradeRouteVertex`, 0x18 bytes, `_FGX_VERTEXFORMATS` 9).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct TradeVertex {
    pub position: [f32; 3],
    /// Bytes `R, G, B, A`.
    pub color: [u8; 4],
    pub uv: [f32; 2],
}

// re: TradeVisSystem.cpp constants (ra6c-1-trade 1..5)
/// Alpha of the routes of the active player.
const ROUTE_ALPHA: f32 = 0.4;
/// Alpha of the popup route (`kPopupRouteAlpha`).
const POPUP_ALPHA: f32 = 0.95;
/// Stations of a strip piece: 4 vertices each, 8 rows of 18 indices.
const STATIONS: usize = 9;
const STRIP_VERTICES: usize = STATIONS * 4;
const STRIP_INDICES: usize = (STATIONS - 1) * 18;
/// Parameter step of a full piece and of the stub of the end cap.
const STRIP_STEP: f32 = 0.125;
const STUB_STEP: f32 = 0.01875;
const SHAFT_HALF: f32 = 5.0;
const SHAFT_FEATHER_HALF: f32 = 5.35;
/// `v` per unit of the curve parameter.
const V_PER_PARAM: f32 = -2.0;
pub const HEAD_VERTICES: usize = 14;
pub const HEAD_INDICES: usize = 51;
const HEAD_HALF: f32 = 10.0;
const HEAD_FEATHER_HALF: f32 = 10.500_215;
const HEAD_BACK_SHIFT: f32 = 0.5;
const HEAD_FEATHER_BACK_SHIFT: f32 = 0.15;
const HEAD_TIP: f32 = 23.8;
const HEAD_TIP_FEATHER: f32 = 24.6155;
/// Capacity of the buffers of the original (`AllocateRouteGeometry` 0x089f439e).
pub const VERTEX_CAPACITY: usize = 0x12000;
pub const INDEX_CAPACITY: usize = 0x48000;
/// The template of a strip row; the next row is 4 vertices higher.
const ROW: [u32; 18] = [0, 4, 5, 0, 5, 1, 1, 5, 6, 1, 6, 2, 2, 6, 7, 2, 7, 3];
/// The triangles of the head (table 0x092237f4).
const HEAD: [u32; HEAD_INDICES] = [
    0, 1, 4, 4, 1, 3, 3, 1, 2, 0, 4, 9, 0, 9, 5, 0, 5, 6, 0, 6, 1, 2, 1, 6, 2, 6, 7, 2, 7, 8, 2, 8,
    3, 3, 8, 10, 3, 10, 11, 4, 3, 11, 4, 11, 12, 4, 12, 13, 4, 13, 9,
];
/// Scroll phases per second (`g_fTradeArrowScroll += dt * 0.75`).
pub const SCROLL_SPEED: f32 = 0.75;

/// The vertices and indices of the routes and the index range of each type.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TradeMesh {
    pub vertices: Vec<TradeVertex>,
    pub indices: Vec<u32>,
    /// Index ranges of the batches, by [`TradeKind`].
    /// Then the popup route's, by [`TradeKind::popup_index`].
    pub batches: [Range<u32>; 6],
}

impl TradeMesh {
    /// The range of the batch of `kind`.
    pub fn batch(&self, kind: TradeKind) -> Range<u32> {
        self.batches[kind.index()].clone()
    }
}

/// The world point of a plot: its centre in the plane of the map (z of the original is 0).
// re: GetHexWorldLocation -> cvTerrainSampler::GetWorldLocation2D (ra6c-1-trade 1)
fn plot_point(plot: (i32, i32)) -> Vec2 {
    cell_center(plot.0, plot.1)
}

/// The points of a route in one run: each next point is the copy of the plot nearest to the
/// previous one across the seam of the map (`wrap_width` 0: no wrap).
// re: get_unwrapped_pos 0x089f235c (a window of hexes unwrapped against its current hex)
pub fn route_points(plots: &[(i32, i32)], wrap_width: f32) -> Vec<Vec2> {
    let mut out: Vec<Vec2> = Vec::with_capacity(plots.len());
    for &plot in plots {
        let mut p = plot_point(plot);
        if wrap_width > 0.0
            && let Some(prev) = out.last()
        {
            p.x += ((prev.x - p.x) / wrap_width).round() * wrap_width;
        }
        out.push(p);
    }
    out
}

/// The curve of a piece: `P0`, `P1` and the tangents.
struct Piece {
    p0: Vec2,
    p1: Vec2,
    t0: Vec2,
    t1: Vec2,
}

impl Piece {
    /// Catmull-Rom piece from `cur` to `next`.
    fn segment(prev: Vec2, cur: Vec2, next: Vec2, next2: Vec2) -> Self {
        Piece {
            p0: cur,
            p1: next,
            t0: (next - prev) * 0.5,
            t1: (next2 - cur) * 0.5,
        }
    }

    // re: EvaluateHermite 0x089f4420
    fn point(&self, t: f32) -> Vec2 {
        let t2 = t * t;
        let t3 = t2 * t;
        let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
        let h01 = -2.0 * t3 + 3.0 * t2;
        let h10 = t3 - 2.0 * t2 + t;
        let h11 = t3 - t2;
        self.p0 * h00 + self.p1 * h01 + self.t0 * h10 + self.t1 * h11
    }

    // re: BuildRouteStrip 0x089f3eba (the derivative weights)
    fn tangent(&self, t: f32) -> Vec2 {
        let t2 = t * t;
        self.p0 * (6.0 * t2 - 6.0 * t)
            + self.p1 * (-6.0 * t2 + 6.0 * t)
            + self.t0 * (3.0 * t2 - 4.0 * t + 1.0)
            + self.t1 * (3.0 * t2 - 2.0 * t)
    }
}

/// The colour of a route's vertices: the core and the feather (alpha 0).
#[derive(Clone, Copy)]
struct Paint {
    solid: [u8; 4],
    feather: [u8; 4],
}

impl Paint {
    /// `uRgba` and `uRgb` of `BuildRouteStrip`: the channels are `(int)(c * 255)`.
    fn new(rgb: [u8; 3], alpha: f32) -> Self {
        Paint {
            solid: [rgb[0], rgb[1], rgb[2], (alpha * 255.0) as u8],
            feather: [rgb[0], rgb[1], rgb[2], 0],
        }
    }
}

/// What the strips of a build write into, with the room left in the buffers.
struct Builder<'a> {
    mesh: &'a mut TradeMesh,
}

/// What a strip leaves: the last centre, the unit normal there and the phase `-t`.
struct StripEnd {
    centre: Vec2,
    normal: Vec2,
    phase: f32,
}

impl Builder<'_> {
    fn room(&self, vertices: usize, indices: usize) -> bool {
        self.mesh.vertices.len() + vertices < VERTEX_CAPACITY
            && self.mesh.indices.len() + indices < INDEX_CAPACITY
    }

    fn vertex(&mut self, x: f32, y: f32, color: [u8; 4], u: f32, v: f32) {
        self.mesh.vertices.push(TradeVertex {
            position: [x, y, 0.0],
            color,
            uv: [u, v],
        });
    }

    /// A strip of 9 stations along the piece, `step` apart in the parameter; `None` if the buffers
    /// are full.
    // re: BuildRouteStrip 0x089f3eba
    fn strip(&mut self, piece: &Piece, paint: Paint, step: f32) -> Option<StripEnd> {
        if !self.room(STRIP_VERTICES, STRIP_INDICES) {
            return None;
        }
        let first = self.mesh.vertices.len() as u32;
        let mut centre = Vec2::ZERO;
        let mut normal = Vec2::ZERO;
        for station in 0..STATIONS {
            let t = station as f32 * step;
            centre = piece.point(t);
            let d = piece.tangent(t);
            normal = Vec2::new(-d.y, d.x) / d.length();
            let edge = normal * SHAFT_HALF;
            let feather = normal * SHAFT_FEATHER_HALF;
            let v = t * V_PER_PARAM;
            self.vertex(
                centre.x + feather.x,
                centre.y + feather.y,
                paint.feather,
                0.667_248_55,
                v,
            );
            self.vertex(
                centre.x + edge.x,
                centre.y + edge.y,
                paint.solid,
                0.656_25,
                v,
            );
            self.vertex(
                centre.x - edge.x,
                centre.y - edge.y,
                paint.solid,
                0.343_75,
                v,
            );
            self.vertex(
                centre.x - feather.x,
                centre.y - feather.y,
                paint.feather,
                0.332_812_5,
                v,
            );
        }
        for row in 0..(STATIONS - 1) as u32 {
            self.mesh
                .indices
                .extend(ROW.iter().map(|i| i + first + row * 4));
        }
        Some(StripEnd {
            centre,
            normal,
            phase: -(STATIONS as f32 * step),
        })
    }

    /// A full piece of the chain.
    // re: BuildRouteSegment 0x089f3ac8
    fn segment(&mut self, paint: Paint, prev: Vec2, cur: Vec2, next: Vec2, next2: Vec2) -> bool {
        self.strip(&Piece::segment(prev, cur, next, next2), paint, STRIP_STEP)
            .is_some()
    }

    /// The last piece: a stub of the step from `cur` and the head, pointing to `end`.
    // re: BuildRouteEndCap 0x089f2faa
    fn end_cap(&mut self, paint: Paint, prev: Vec2, cur: Vec2, end: Vec2) {
        let piece = Piece {
            p0: cur,
            p1: end,
            t0: (end - prev) * 0.5,
            t1: (end - cur) * 0.5,
        };
        let Some(stub) = self.strip(&piece, paint, STUB_STEP) else {
            return;
        };
        if !self.room(HEAD_VERTICES, HEAD_INDICES) {
            return;
        }
        let forward = (end - stub.centre).normalize_or_zero();
        let first = self.mesh.vertices.len() as u32;
        let v = 2.0 * stub.phase;
        let (n, c) = (stub.normal, stub.centre);
        // A vertex `across` the route and `along` it, in the frame of the stub end.
        let at = |across: f32, along: f32| {
            (
                c.x + across * n.x + along * n.y,
                c.y + across * n.y - along * n.x,
            )
        };
        let mut put = |p: (f32, f32), color: [u8; 4], u: f32, dv: f32| {
            self.vertex(p.0, p.1, color, u, v + dv);
        };
        let (s, f) = (paint.solid, paint.feather);
        put(at(HEAD_HALF, HEAD_BACK_SHIFT), s, 0.8125, -0.015_625);
        put(
            (c.x + forward.x * HEAD_TIP, c.y + forward.y * HEAD_TIP),
            s,
            0.5,
            -0.743_75,
        );
        put(at(-HEAD_HALF, HEAD_BACK_SHIFT), s, 0.1875, -0.015_625);
        put(at(-SHAFT_HALF, HEAD_BACK_SHIFT), s, 0.343_75, -0.015_625);
        put(at(SHAFT_HALF, HEAD_BACK_SHIFT), s, 0.656_25, -0.015_625);
        put(
            at(HEAD_FEATHER_HALF, HEAD_FEATHER_BACK_SHIFT),
            f,
            0.828_131_7,
            -0.004_687_5,
        );
        put(
            (
                c.x + forward.x * HEAD_TIP_FEATHER,
                c.y + forward.y * HEAD_TIP_FEATHER,
            ),
            f,
            0.5,
            -0.769_234_36,
        );
        put(
            at(-HEAD_FEATHER_HALF, HEAD_FEATHER_BACK_SHIFT),
            f,
            0.171_868_3,
            -0.004_687_5,
        );
        put(
            at(-SHAFT_FEATHER_HALF, HEAD_FEATHER_BACK_SHIFT),
            f,
            0.332_812_5,
            -0.004_687_5,
        );
        put(
            at(SHAFT_FEATHER_HALF, HEAD_FEATHER_BACK_SHIFT),
            f,
            0.667_187_5,
            -0.004_687_5,
        );
        put(at(-SHAFT_FEATHER_HALF, 0.0), f, 0.332_812_5, 0.0);
        put(at(-SHAFT_HALF, 0.0), s, 0.343_75, 0.0);
        put(at(SHAFT_HALF, 0.0), s, 0.656_25, 0.0);
        put(at(SHAFT_FEATHER_HALF, 0.0), f, 0.667_187_5, 0.0);
        self.mesh.indices.extend(HEAD.iter().map(|i| i + first));
    }

    /// One route; fewer than two plots draw nothing.
    // re: BuildRouteGeometry 0x089f2983
    fn route(&mut self, points: &[Vec2], paint: Paint) {
        let n = points.len();
        match n {
            0 | 1 => {}
            2 => self.end_cap(paint, points[0], points[0], points[1]),
            _ => {
                if !self.segment(paint, points[0], points[0], points[1], points[2]) {
                    return;
                }
                for j in 1..n - 2 {
                    if !self.segment(
                        paint,
                        points[j - 1],
                        points[j],
                        points[j + 1],
                        points[j + 2],
                    ) {
                        return;
                    }
                }
                self.end_cap(paint, points[n - 3], points[n - 2], points[n - 1]);
            }
        }
    }
}

/// The mesh of the routes of `owner`, batched by type. `wrap_width` is the period of a wrapping
/// map (0 for none).
// re: TradeVisSystem::Update 0x089f2428 (three passes, one per connection type)
pub fn build_mesh(routes: &[TradeRoute], owner: i8, wrap_width: f32) -> TradeMesh {
    build_mesh_with_popup(routes, owner, None, wrap_width)
}

/// [`build_mesh`] and the popup route, which is built whoever owns it, with alpha 0.95 and the popup
/// texture of its type.
pub fn build_mesh_with_popup(
    routes: &[TradeRoute],
    owner: i8,
    popup: Option<&TradeRoute>,
    wrap_width: f32,
) -> TradeMesh {
    let mut mesh = TradeMesh::default();
    let mut batches: [Range<u32>; 6] = [0..0, 0..0, 0..0, 0..0, 0..0, 0..0];
    for kind in TradeKind::ALL {
        let start = mesh.indices.len() as u32;
        let mut b = Builder { mesh: &mut mesh };
        for r in routes.iter().filter(|r| r.owner == owner && r.kind == kind) {
            let points = route_points(&r.plots, wrap_width);
            b.route(&points, Paint::new(r.color, ROUTE_ALPHA));
        }
        batches[kind.index()] = start..mesh.indices.len() as u32;
    }
    if let Some(r) = popup {
        let start = mesh.indices.len() as u32;
        let mut b = Builder { mesh: &mut mesh };
        b.route(
            &route_points(&r.plots, wrap_width),
            Paint::new(r.color, POPUP_ALPHA),
        );
        batches[r.kind.popup_index()] = start..mesh.indices.len() as u32;
    }
    mesh.batches = batches;
    mesh
}

/// The premultiplied pixel `ps_trade` writes for a texel `tex` (rgb, alpha 1) and a vertex colour
/// `[r, g, b, a]` in bytes: the texture is a mask (white arrow on black, alpha 255), the colour is the
/// player's.
// SIMPLIFIED(SIMP-exe-920): the pixel shader's bytecode is not in the game files; the mask reading is
// ours (the old `tex * colour` drew the black of the texture as a dark box in the route's width).
pub fn arrow_pixel(tex: [f32; 3], vertex: [u8; 4]) -> [f32; 4] {
    let a = tex[0] * f32::from(vertex[3]) / 255.0;
    [
        f32::from(vertex[0]) / 255.0 * a,
        f32::from(vertex[1]) / 255.0 * a,
        f32::from(vertex[2]) / 255.0 * a,
        a,
    ]
}

// ---- the path between two cities (SIMP-069) ----

/// The cube coordinates of an offset cell: `x = col - (row >> 1)`, `y = row`.
fn cube(cell: (i32, i32)) -> (i32, i32) {
    (cell.0 - (cell.1 >> 1), cell.1)
}

fn cube_distance(a: (i32, i32), b: (i32, i32)) -> i32 {
    let (dx, dy) = (a.0 - b.0, a.1 - b.1);
    dx.abs().max(dy.abs()).max((dx + dy).abs())
}

/// The cells of the shortest hex line from `from` to `to`, both ends included. On a map that wraps
/// along X (`width` cells round) the line takes the shorter way round: the columns may then lie
/// outside `0..width` (the copy of the map the route runs through).
// SIMPLIFIED(SIMP-069): only for a route the replica core has given no path for yet (the client
// lays the routes on the core's `Q_TRADE_PATHS`, SIMP-exe-660); a straight hex line between the two
// cities stands in for that moment.
pub fn hex_line(from: (i32, i32), to: (i32, i32), width: i32, wrap_x: bool) -> Vec<(i32, i32)> {
    let a = cube(from);
    let copies: &[i32] = if wrap_x && width > 0 {
        &[-1, 0, 1]
    } else {
        &[0]
    };
    let (b, n) = copies
        .iter()
        .map(|k| cube((to.0 + k * width, to.1)))
        .map(|b| (b, cube_distance(a, b)))
        .min_by_key(|&(_, d)| d)
        .expect("at least one copy");
    if n == 0 {
        return vec![from];
    }
    (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            // Cube rounding of the interpolated point (x, y, z = -x - y).
            let fx = a.0 as f32 + (b.0 - a.0) as f32 * t;
            let fy = a.1 as f32 + (b.1 - a.1) as f32 * t;
            let fz = -fx - fy;
            let (mut rx, mut ry, rz) = (fx.round(), fy.round(), fz.round());
            let (dx, dy, dz) = ((rx - fx).abs(), (ry - fy).abs(), (rz - fz).abs());
            if dx > dy && dx > dz {
                rx = -ry - rz;
            } else if dy > dz {
                ry = -rx - rz;
            }
            let (x, y) = (rx as i32, ry as i32);
            (x + (y >> 1), y)
        })
        .collect()
}

// ---- the subsystem ----

/// The trade routes of the active player.
pub struct Trade {
    routes: Vec<TradeRoute>,
    /// The active player: only his routes are drawn (`Update` 0x089f2428).
    active: i8,
    wrap_width: f32,
    /// The popup route (`g_iTradePopupRoute`), drawn whoever owns it.
    popup: Option<TradeRoute>,
    /// `g_bShowTradeRoutes`: the "Show Trade" option of the minimap.
    show: bool,
    /// The layer switch (`[DisableSystems] Arrows`), set by the renderer each frame.
    layer: Cell<bool>,
    time: f32,
    mesh: TradeMesh,
    dirty: bool,
    gpu: Option<TradeGpu>,
}

impl Default for Trade {
    fn default() -> Self {
        Self {
            routes: Vec::new(),
            active: -1,
            wrap_width: 0.0,
            popup: None,
            show: true,
            layer: Cell::new(true),
            time: 0.0,
            mesh: TradeMesh::default(),
            dirty: false,
            gpu: None,
        }
    }
}

impl Trade {
    /// The routes and the active player. The same list again does nothing.
    pub fn set_routes(&mut self, active: i8, routes: Vec<TradeRoute>) {
        if self.active == active && self.routes == routes {
            return;
        }
        self.active = active;
        self.routes = routes;
        self.dirty = true;
    }

    /// The popup route, or none. The same route again does nothing.
    pub fn set_popup(&mut self, popup: Option<TradeRoute>) {
        if self.popup != popup {
            self.popup = popup;
            self.dirty = true;
        }
    }

    pub fn popup(&self) -> Option<&TradeRoute> {
        self.popup.as_ref()
    }

    pub fn routes(&self) -> &[TradeRoute] {
        &self.routes
    }

    /// The period of the map in X (0 for a map that does not wrap).
    pub fn set_wrap_width(&mut self, width: f32) {
        if self.wrap_width != width {
            self.wrap_width = width;
            self.dirty = true;
        }
    }

    /// `g_bShowTradeRoutes`.
    pub fn set_show(&mut self, show: bool) {
        if self.show != show {
            self.show = show;
            self.dirty = true;
        }
    }

    pub fn show(&self) -> bool {
        self.show
    }

    /// The layer switch of `Arrows`.
    pub(crate) fn set_layer(&self, on: bool) {
        self.layer.set(on);
    }

    /// Seconds since the start; the scroll phase follows it.
    pub fn set_time(&mut self, seconds: f32) {
        self.time = seconds;
    }

    /// The phase of the arrow texture: `frac(t * 0.75)`.
    pub fn phase(&self) -> f32 {
        (self.time * SCROLL_SPEED).rem_euclid(1.0)
    }

    /// The mesh of the routes of the active player.
    pub fn mesh(&self) -> &TradeMesh {
        &self.mesh
    }

    /// Takes the device (once, after it exists).
    pub fn bind_device(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.gpu = Some(TradeGpu::new(device, queue));
        self.dirty = true;
    }

    /// The textures of [`TEXTURES`] from `textures`; returns how many were taken.
    pub fn set_textures(
        &mut self,
        textures: &std::collections::HashMap<asset_format::LogicalId, asset_format::TextureBlob>,
    ) -> usize {
        self.gpu.as_mut().map_or(0, |g| g.set_textures(textures))
    }

    /// True when the overlay has something to draw this frame.
    pub fn drawn(&self) -> bool {
        self.layer.get()
            && (self.show || self.popup.is_some())
            && self.gpu.is_some()
            && !self.mesh.indices.is_empty()
    }

    /// Draws the routes into the pass of the map of the overlay; returns the index counts of the
    /// draws. Nothing when the layer, the option or the routes are off.
    pub(crate) fn draw(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        projection: &glam::Mat4,
        ours_to_orig: &glam::Mat4,
        shifts: &[f32],
    ) -> Vec<u32> {
        let Some(gpu) = self.gpu.as_ref().filter(|_| self.drawn()) else {
            return Vec::new();
        };
        gpu.set_frame(&TradeFrame {
            projection,
            ours_to_orig,
            shifts,
            phase: self.phase(),
        });
        gpu.draw(pass)
    }
}

impl RenderSystem for Trade {
    fn id(&self) -> SystemId {
        SystemId::Trade
    }

    /// No slot of its own: `Overlay` draws the mesh inside its pass into the map of the overlay.
    fn passes(&self) -> &'static [PassSlot] {
        &[]
    }

    fn init(&mut self, _cx: &mut InitContext<'_>) {}

    fn prepare(&mut self, cx: &mut PrepareContext<'_>) {
        if std::mem::take(&mut self.dirty) {
            // With the option off only the popup route is built (`Update`: the routes wait for `g_bShowTradeRoutes`).
            let routes: &[TradeRoute] = if self.show { &self.routes } else { &[] };
            self.mesh =
                build_mesh_with_popup(routes, self.active, self.popup.as_ref(), self.wrap_width);
            if let Some(gpu) = &mut self.gpu {
                gpu.set_mesh(cx.device, &self.mesh);
            }
        }
    }

    fn record(&self, _slot: PassSlot, _cx: &mut DrawContext<'_, '_>) {}

    fn counters(&self) -> Vec<(&'static str, u64)> {
        vec![
            ("trade.routes", self.routes.len() as u64),
            ("trade.indices", self.mesh.indices.len() as u64),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hexgrid::HEX_WIDTH;

    fn route(owner: i8, kind: TradeKind, plots: &[(i32, i32)]) -> TradeRoute {
        TradeRoute {
            owner,
            color: [200, 100, 50],
            kind,
            plots: plots.to_vec(),
        }
    }

    fn mesh_of(plots: &[(i32, i32)], wrap: f32) -> TradeMesh {
        build_mesh(&[route(0, TradeKind::Food, plots)], 0, wrap)
    }

    #[test]
    fn a_piece_is_36_vertices_and_144_indices_and_the_head_14_and_51() {
        // Two plots: the end cap alone, a stub and a head.
        let m = mesh_of(&[(3, 3), (4, 3)], 0.0);
        assert_eq!(m.vertices.len(), STRIP_VERTICES + HEAD_VERTICES);
        assert_eq!(m.indices.len(), STRIP_INDICES + HEAD_INDICES);
        assert_eq!((STRIP_VERTICES, STRIP_INDICES), (36, 144));
        // Three plots: one full piece and the end cap.
        let m = mesh_of(&[(3, 3), (4, 3), (5, 3)], 0.0);
        assert_eq!(m.vertices.len(), 2 * STRIP_VERTICES + HEAD_VERTICES);
        assert_eq!(m.indices.len(), 2 * STRIP_INDICES + HEAD_INDICES);
        // n plots: n - 2 full pieces.
        let plots: Vec<_> = (0..7).map(|i| (i, 2)).collect();
        let m = mesh_of(&plots, 0.0);
        assert_eq!(m.vertices.len(), 6 * STRIP_VERTICES + HEAD_VERTICES);
        // Every index is inside the vertices.
        assert!(m.indices.iter().all(|&i| (i as usize) < m.vertices.len()));
    }

    #[test]
    fn fewer_than_two_plots_draw_nothing() {
        assert!(mesh_of(&[], 0.0).vertices.is_empty());
        assert!(mesh_of(&[(1, 1)], 0.0).vertices.is_empty());
    }

    #[test]
    fn the_curve_passes_through_the_plot_centres() {
        let pts = route_points(&[(1, 2), (2, 2), (3, 3), (4, 3)], 0.0);
        for j in 1..pts.len() - 2 {
            let piece = Piece::segment(pts[j - 1], pts[j], pts[j + 1], pts[j + 2]);
            assert!(piece.point(0.0).distance(pts[j]) < 1e-3);
            assert!(piece.point(1.0).distance(pts[j + 1]) < 1e-3);
        }
        // The first vertex pair of the first piece straddles the first plot.
        let m = mesh_of(&[(1, 2), (2, 2), (3, 3), (4, 3)], 0.0);
        let mid = (Vec2::from_slice(&m.vertices[1].position)
            + Vec2::from_slice(&m.vertices[2].position))
            * 0.5;
        assert!(mid.distance(pts[0]) < 1e-3, "{mid:?} {:?}", pts[0]);
    }

    #[test]
    fn a_strip_has_the_core_the_feather_and_the_texture_coordinates() {
        let m = mesh_of(&[(3, 3), (4, 3), (5, 3)], 0.0);
        let row = &m.vertices[..4];
        // Feather, core, core, feather; the feather is invisible.
        assert_eq!(row[0].color[3], 0);
        assert_eq!(row[3].color[3], 0);
        assert_eq!(row[1].color, [200, 100, 50, 102]);
        assert_eq!(row[2].color, row[1].color);
        assert_eq!(
            [row[0].uv[0], row[1].uv[0], row[2].uv[0], row[3].uv[0]],
            [0.667_248_55, 0.656_25, 0.343_75, 0.332_812_5]
        );
        // The core is 10 wide, the feather 10.7, for a route running along X.
        let w = |a: usize, b: usize| {
            Vec2::from_slice(&row[a].position).distance(Vec2::from_slice(&row[b].position))
        };
        assert!((w(1, 2) - 10.0).abs() < 1e-3);
        assert!((w(0, 3) - 10.7).abs() < 1e-3);
        // v runs -2t over the piece: 0 at the start, -2 at the end.
        assert_eq!(m.vertices[0].uv[1], 0.0);
        assert!((m.vertices[(STATIONS - 1) * 4].uv[1] + 2.0).abs() < 1e-5);
        // All of it on the plane.
        assert!(m.vertices.iter().all(|v| v.position[2] == 0.0));
    }

    #[test]
    fn the_stub_covers_a_sixth_of_the_last_step_and_the_head_follows_it() {
        let plots = [(3, 3), (4, 3), (5, 3)];
        let pts = route_points(&plots, 0.0);
        let m = mesh_of(&plots, 0.0);
        let stub = &m.vertices[STRIP_VERTICES..2 * STRIP_VERTICES];
        let last = &stub[(STATIONS - 1) * 4..];
        let mid = (Vec2::from_slice(&last[1].position) + Vec2::from_slice(&last[2].position)) * 0.5;
        // t = 0.15 of the Hermite piece from the middle plot to the last (10.2 units of 64: the
        // tangents are 64 and 32).
        assert!((mid.x - (pts[1].x + 10.21)).abs() < 0.05, "{mid:?}");
        // The head: the tip lies HEAD_TIP further along the route, and v follows 2 * phase.
        let head = &m.vertices[2 * STRIP_VERTICES..];
        assert_eq!(head.len(), HEAD_VERTICES);
        let tip = Vec2::from_slice(&head[1].position);
        assert!(
            (tip.distance(mid) - HEAD_TIP).abs() < 0.5,
            "{tip:?} {mid:?}"
        );
        assert!(tip.x > mid.x);
        assert!((head[4].uv[1] - (2.0 * -0.168_75 - 0.015_625)).abs() < 1e-5);
        assert!((head[1].uv[1] - (2.0 * -0.168_75 - 0.743_75)).abs() < 1e-5);
        // The indices of the head are the table, after the strip.
        let first = (2 * STRIP_VERTICES) as u32;
        assert_eq!(m.indices[2 * STRIP_INDICES], first);
        assert_eq!(
            &m.indices[2 * STRIP_INDICES..],
            &HEAD.map(|i| i + first)[..]
        );
    }

    #[test]
    fn a_route_across_the_seam_stays_continuous() {
        let width = 40;
        let w = HEX_WIDTH * width as f32;
        // Columns 38, 39, 0, 1 on one row: the points run on without a jump.
        let pts = route_points(&[(38, 4), (39, 4), (0, 4), (1, 4)], w);
        for pair in pts.windows(2) {
            assert!((pair[1].x - pair[0].x - HEX_WIDTH).abs() < 1e-3, "{pts:?}");
        }
        // Without the width they jump.
        let raw = route_points(&[(39, 4), (0, 4)], 0.0);
        assert!(raw[1].x < raw[0].x);
        // The mesh is continuous too: no vertex is a map-width away from its neighbours.
        let m = mesh_of(&[(38, 4), (39, 4), (0, 4), (1, 4)], w);
        for row in m.vertices[..3 * STRIP_VERTICES].chunks(4) {
            assert!((row[0].position[0] - row[3].position[0]).abs() < 12.0);
        }
        let xs: Vec<f32> = m.vertices.iter().map(|v| v.position[0]).collect();
        let span = xs.iter().cloned().fold(f32::MIN, f32::max)
            - xs.iter().cloned().fold(f32::MAX, f32::min);
        assert!(span < 4.0 * HEX_WIDTH, "span {span}");
    }

    #[test]
    fn only_the_active_players_routes_are_built_with_their_colour_and_batch() {
        let routes = vec![
            route(0, TradeKind::International, &[(1, 1), (2, 1)]),
            route(1, TradeKind::International, &[(5, 5), (6, 5)]),
            TradeRoute {
                color: [1, 2, 3],
                ..route(0, TradeKind::Production, &[(8, 8), (9, 8)])
            },
        ];
        let m = build_mesh(&routes, 0, 0.0);
        let per = (STRIP_INDICES + HEAD_INDICES) as u32;
        assert_eq!(m.batch(TradeKind::International), 0..per);
        assert_eq!(m.batch(TradeKind::Food), per..per);
        assert_eq!(m.batch(TradeKind::Production), per..2 * per);
        assert_eq!(m.vertices.len(), 2 * (STRIP_VERTICES + HEAD_VERTICES));
        assert_eq!(
            m.vertices[STRIP_VERTICES + HEAD_VERTICES + 1].color,
            [1, 2, 3, 102]
        );
        // The other player's routes are the other player's mesh.
        let theirs = build_mesh(&routes, 1, 0.0);
        assert_eq!(theirs.vertices.len(), STRIP_VERTICES + HEAD_VERTICES);
        assert!(build_mesh(&routes, 5, 0.0).vertices.is_empty());
    }

    #[test]
    fn the_phase_is_the_fraction_of_three_quarters_per_second() {
        let mut t = Trade::default();
        t.set_time(2.0);
        assert!((t.phase() - 0.5).abs() < 1e-6);
        t.set_time(4.0);
        assert!(t.phase().abs() < 1e-6);
    }

    #[test]
    fn the_same_routes_again_change_nothing() {
        let mut t = Trade::default();
        let r = vec![route(0, TradeKind::Food, &[(1, 1), (2, 1)])];
        t.set_routes(0, r.clone());
        assert!(t.dirty);
        t.dirty = false;
        t.set_routes(0, r);
        assert!(!t.dirty);
        t.set_routes(1, Vec::new());
        assert!(t.dirty);
    }

    #[test]
    fn the_hex_line_runs_cell_to_adjacent_cell() {
        let line = hex_line((2, 2), (9, 6), 40, false);
        assert_eq!(line.first(), Some(&(2, 2)));
        assert_eq!(line.last(), Some(&(9, 6)));
        assert_eq!(
            line.len() as i32,
            cube_distance(cube((2, 2)), cube((9, 6))) + 1
        );
        for pair in line.windows(2) {
            assert_eq!(cube_distance(cube(pair[0]), cube(pair[1])), 1, "{line:?}");
        }
        assert_eq!(hex_line((4, 4), (4, 4), 40, false), vec![(4, 4)]);
    }

    #[test]
    fn the_hex_line_takes_the_short_way_round_a_wrapping_map() {
        // 38 to 2 on a map of 40: 4 steps through the seam, not 36 the other way.
        let line = hex_line((38, 4), (2, 4), 40, true);
        assert_eq!(line.len(), 5, "{line:?}");
        assert_eq!(line.first(), Some(&(38, 4)));
        assert_eq!(line.last(), Some(&(42, 4)));
        assert_eq!(
            line.iter().map(|c| c.0.rem_euclid(40)).collect::<Vec<_>>(),
            vec![38, 39, 0, 1, 2]
        );
        // Without wrap the long way.
        assert_eq!(hex_line((38, 4), (2, 4), 40, false).len(), 37);
        // The way back runs the other copy.
        let back = hex_line((2, 4), (38, 4), 40, true);
        assert_eq!(back.last(), Some(&(-2, 4)));
    }

    #[test]
    fn the_popup_route_has_alpha_95_its_own_batch_and_any_owner() {
        let routes = vec![route(0, TradeKind::International, &[(1, 1), (2, 1)])];
        let popup = TradeRoute {
            color: [9, 8, 7],
            ..route(3, TradeKind::Food, &[(4, 4), (5, 4)])
        };
        let m = build_mesh_with_popup(&routes, 0, Some(&popup), 0.0);
        let per = (STRIP_INDICES + HEAD_INDICES) as u32;
        assert_eq!(m.batch(TradeKind::International), 0..per);
        assert_eq!(m.batches[TradeKind::Food.popup_index()], per..2 * per);
        assert_eq!(
            m.vertices[STRIP_VERTICES + HEAD_VERTICES + 1].color,
            [9, 8, 7, 242]
        );
        assert!(m.batches[TradeKind::International.popup_index()].is_empty());
        assert_eq!(
            TEXTURES[TradeKind::Food.popup_index()],
            "tx:arrow_popup_food.dds"
        );
    }

    #[test]
    fn with_the_option_off_only_the_popup_route_is_drawn() {
        let mut t = Trade::default();
        t.set_routes(0, vec![route(0, TradeKind::Food, &[(1, 1), (2, 1)])]);
        t.set_show(false);
        t.set_popup(Some(route(1, TradeKind::Food, &[(3, 3), (4, 3)])));
        t.dirty = false;
        t.set_popup(Some(route(1, TradeKind::Food, &[(3, 3), (4, 3)])));
        assert!(!t.dirty, "the same popup again changes nothing");
        let routes: &[TradeRoute] = if t.show { &t.routes } else { &[] };
        let m = build_mesh_with_popup(routes, 0, t.popup(), 0.0);
        assert_eq!(m.vertices.len(), STRIP_VERTICES + HEAD_VERTICES);
    }

    #[test]
    fn the_arrow_pixel_is_the_player_colour_where_the_texture_is_white() {
        // arrow.dds is white on black with alpha 255 everywhere.
        let v = [200, 100, 50, 102];
        let on = arrow_pixel([1.0, 1.0, 1.0], v);
        assert!((on[0] - 200.0 / 255.0 * 0.4).abs() < 1e-3 && (on[3] - 0.4).abs() < 1e-3);
        let off = arrow_pixel([0.0, 0.0, 0.0], v);
        assert_eq!(off, [0.0; 4], "the black of the texture draws nothing");
    }
}
