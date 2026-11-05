//! Roads, railroads and bridges as models (world-objects 4.7, WOB6): which plots a route joins,
//! the models that are laid along the way, and the bridges where a road crosses a river.
//!
//! `roads_rails.gr2` holds a piece of a road (`Road_reg`, 26 units wide, 128 long, along Z) and one
//! of a railroad (`RR_reg`), and their pillaged pieces (`Road_dam`, `RR_dam`). They are
//! `LandmarkShader` meshes: the original does not draw them but lays them along the spline of the
//! road into the layer of decals of the terrain (`cvGameplayRoadHandler::UpdateRoadGeometry` ->
//! `AddSplineDecal`), so this layer keeps only how a piece is textured ([`RoadSkin`]) and
//! `objects::decals` makes the strips. `bridges.gr2` holds the three pieces of a bridge, `ANC_`
//! (stone) and `IND_` (iron): `End1`, `Mid`, `End2` (`BuildingShader`, drawn as models). A bridge
//! stands on the middle of the edge when a river runs on it; the plot with the lower key draws it.
//! Every plot keeps its own links, so a link over the seam of a wrapping map needs no unwrapped
//! coordinates.
//!
//! Whose bridge it is follows the original (Lekmod `CvTeam::changeBridgeBuildingCount`, `GameplayBridgeChanged`): a
//! road over a river has a bridge only when the VIEWER's team knows the technology with `BridgeBuilding`
//! (Engineering), whoever owns the plot; the bridge is iron from the viewer's era `LAST_BRIDGE_ART_ERA` (Modern) on,
//! stone before. Without it the road crosses the river as a ford (the plain road). [`BridgeState`] is set by the
//! host (`RouteLayer::set_bridges`); the routes are placed again when it changes.
//!
//! SIMPLIFIED(SIMP-120): until the host tells the state (no replica: the interim UI, tests) every road over a river
//! has a bridge, iron when the owner is of the Industrial era or later.
//!
//! SIMPLIFIED(SIMP-047): one scale for the bridge models ([`ROUTE_SCALE`]: the four `HB` pieces of
//! `roadsandrails.gr2` together are one hex), no piece for the trade routes or for the junctions
//! (`PL`, `con`).

use std::collections::{BTreeMap, BTreeSet};

use asset_format::{ArtTable, LogicalId, ModelBlobV2, RouteArt};
use glam::{Mat4, Vec2, Vec3};

use crate::entities::roads::{RouteKind, dir_vector};
use crate::entities::{PlotDecor, PlotKey, plot_key};
use crate::hexgrid::{Dir, GridShape, HEX_WIDTH, cell_center};
use crate::world::objects::input::Keyed;
use crate::world::unit::B2Context;

/// Model units to world units: 512 units of the `HB` pieces of a road are one hex (64).
pub const ROUTE_SCALE: f32 = HEX_WIDTH / 512.0;

/// A route lies this far above the ground, so that it is not lost in it.
pub const LIFT: f32 = 0.08;

/// The distance from the middle of a plot to the middle of an edge.
pub const HALF: f32 = HEX_WIDTH * 0.5;

/// The viewer's bridges (`GameplayBridgeChanged(bridge, style)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BridgeState {
    /// The viewer's team has a technology with `BridgeBuilding`.
    pub bridge: bool,
    /// The bridge style is iron (the viewer's era is `LAST_BRIDGE_ART_ERA` or later).
    pub iron: bool,
}

impl BridgeState {
    /// The links with the viewer's bridges applied: no bridge without the technology, the viewer's style.
    pub fn apply(self, links: &mut [Link]) {
        for l in links {
            l.bridge &= self.bridge;
            l.iron = l.kind == RouteKind::Railroad || self.iron;
        }
    }
}

/// What a plot has of a route, as the links need it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Tile {
    pub route: Option<RouteKind>,
    pub pillaged: bool,
    /// Edge bits of the rivers (`Dir::edge_bit`).
    pub rivers: u8,
    pub city: bool,
    /// The owner is of the Industrial era or later (the bridge is iron).
    pub industrial: bool,
}

impl Tile {
    fn joins(&self) -> bool {
        self.route.is_some() || self.city
    }
}

/// One half of a connection, drawn by the plot it starts in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Link {
    pub dir: Dir,
    pub kind: RouteKind,
    pub pillaged: bool,
    /// A river runs on the edge.
    pub river: bool,
    /// This plot draws the bridge over the river.
    pub bridge: bool,
    /// The bridge is made of iron.
    pub iron: bool,
}

/// The links a plot draws. A plot with a route joins every neighbour that has a route or a city; a
/// city joins the neighbours that have a route. The kind is the lesser of the two (a city takes the
/// kind of the route it joins).
pub fn links(
    shape: &GridShape,
    (x, y): (i32, i32),
    key: &dyn Fn(i32, i32) -> u32,
    tile: &dyn Fn(i32, i32) -> Tile,
) -> Vec<Link> {
    let me = tile(x, y);
    if !me.joins() {
        return Vec::new();
    }
    Dir::ALL
        .into_iter()
        .filter_map(|dir| {
            let (nx, ny) = shape.neighbor(x, y, dir)?;
            let n = tile(nx, ny);
            if !n.joins() || (me.route.is_none() && n.route.is_none()) {
                return None;
            }
            let kind = match (me.route, n.route) {
                (Some(a), Some(b)) => a.min(b),
                (Some(a), None) | (None, Some(a)) => a,
                (None, None) => return None,
            };
            let river =
                me.rivers & dir.edge_bit() != 0 || n.rivers & dir.opposite().edge_bit() != 0;
            // The plot with the lower key draws it; a plot that is only a city does not.
            let bridge =
                river && me.route.is_some() && (n.route.is_none() || key(x, y) < key(nx, ny));
            Some(Link {
                dir,
                kind,
                pillaged: me.route.is_some() && me.pillaged,
                river,
                bridge,
                iron: kind == RouteKind::Railroad || me.industrial || n.industrial,
            })
        })
        .collect()
}

/// Where a piece of a route model is in its model.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PieceInfo {
    /// The mesh (and the part: they are one to one in the route models).
    pub mesh: u16,
    /// The middle of the box of the piece on the ground, model units.
    pub centre: Vec2,
    /// Its length along Z, model units.
    pub length: f32,
}

/// How a piece of a road model is textured, for the decal laid along the road: the textures of its
/// `LandmarkShader` material and the texture coordinates of its edges.
#[derive(Debug, Clone, PartialEq)]
pub struct RoadSkin {
    /// `BaseTextureMap` (slot 0) and `HeightTextureMap` (slot 1).
    pub albedo: LogicalId,
    pub height: Option<LogicalId>,
    /// The texture coordinate across the road at the model's +X edge and at its -X edge.
    pub across: [f32; 2],
    /// The texture coordinate along the road at the start (lowest Z) and at the end of the piece;
    /// the texture repeats along the road from one piece to the next.
    pub along: [f32; 2],
    /// Width (X) and length (Z) of the piece, model units.
    pub width: f32,
    pub length: f32,
    /// The coordinate that runs along the road is `u` (else `v`).
    pub along_is_u: bool,
}

impl RoadSkin {
    fn of(blob: &ModelBlobV2, mesh: &asset_format::MeshV2) -> Option<Self> {
        use asset_format::TexSource;
        let desc = blob.materials.get(usize::from(mesh.material))?;
        let file = |name: &str| match desc.texture(name).map(|t| &t.src) {
            Some(TexSource::File(id)) => Some(id.clone()),
            _ => None,
        };
        let albedo = file("BaseTextureMap").or_else(|| desc.texture_files().next().cloned())?;
        let height = file("HeightTextureMap").or_else(|| desc.texture_files().nth(1).cloned());
        let pos = mesh.positions()?;
        // The pipeline stores `1 - v`; the decal samples the D3D texture with the original's `v`
        // (as `unit_skinned.wgsl` does): the strips of `roads_rails.dds` are at its top.
        let uv: Vec<[f32; 2]> = mesh.uvs()?.into_iter().map(|[u, v]| [u, 1.0 - v]).collect();
        if pos.is_empty() || pos.len() != uv.len() {
            return None;
        }
        let pick = |key: &dyn Fn(&[f32; 3]) -> f32, max: bool| {
            let mut best = 0;
            for i in 1..pos.len() {
                let (a, b) = (key(&pos[i]), key(&pos[best]));
                if (max && a > b) || (!max && a < b) {
                    best = i;
                }
            }
            best
        };
        let (x_hi, x_lo) = (pick(&|p| p[0], true), pick(&|p| p[0], false));
        let (z_lo, z_hi) = (pick(&|p| p[2], false), pick(&|p| p[2], true));
        // The coordinate that changes most from one end to the other runs along.
        let du = (uv[z_hi][0] - uv[z_lo][0]).abs();
        let dv = (uv[z_hi][1] - uv[z_lo][1]).abs();
        let along_is_u = du >= dv;
        let (a, c) = if along_is_u { (0, 1) } else { (1, 0) };
        Some(Self {
            albedo,
            height,
            across: [uv[x_hi][c], uv[x_lo][c]],
            along: [uv[z_lo][a], uv[z_hi][a]],
            width: pos[x_hi][0] - pos[x_lo][0],
            length: pos[z_hi][2] - pos[z_lo][2],
            along_is_u,
        })
    }
}

/// The pieces of a route model by the lower case name of their part.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RouteMeta {
    pieces: BTreeMap<String, PieceInfo>,
    skins: BTreeMap<String, RoadSkin>,
}

fn route_like(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("road_") || n.starts_with("rr_") || n.contains("bridge_")
}

impl RouteMeta {
    /// The pieces of a model; a model that has none of the parts of a route gives an empty meta.
    pub fn of(blob: &ModelBlobV2) -> Self {
        let mut pieces = BTreeMap::new();
        let mut skins = BTreeMap::new();
        for part in blob.parts.iter().filter(|p| route_like(&p.name)) {
            let Some(mesh) = blob.meshes.get(usize::from(part.first_mesh)) else {
                continue;
            };
            if let Some(skin) = RoadSkin::of(blob, mesh) {
                skins.insert(part.name.to_ascii_lowercase(), skin);
            }
            let Some(pos) = mesh.positions() else {
                continue;
            };
            if pos.is_empty() {
                continue;
            }
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for p in &pos {
                lo = lo.min(Vec3::from(*p));
                hi = hi.max(Vec3::from(*p));
            }
            pieces.insert(
                part.name.to_ascii_lowercase(),
                PieceInfo {
                    mesh: part.first_mesh,
                    centre: Vec2::new((lo.x + hi.x) * 0.5, (lo.z + hi.z) * 0.5),
                    length: hi.z - lo.z,
                },
            );
        }
        // The road of the original is the gravel strip of the hex border pieces (`Road_HB1..4`), not the tan
        // cobbles of `Road_reg` (S3 capture: a grey track, darker than the ground, with light edges). Of the four
        // quarters of the strip in the atlas only the first is whole in `roadsandrails_d.dds` (the second is empty,
        // the fourth ragged), so the first piece is the skin laid under the name of the plain piece, repeating
        // end to end; the same for the rails.
        // SIMPLIFIED(SIMP-121): which piece the original lays for a road (its road type table, RW-17) is not read: the
        // first border piece, by the look of the S3 capture.
        // The road model of the original (`RoadsandRails_new.gr2`, material `RoadsandRails_new_D.dds`) is not
        // the older one: its `Road_reg` and `RR_reg` are the plain road and railroad (`cvRoadTypeNames`,
        // categories 0 and 1), kept as they are.
        // re: re-b13-answers.txt 1a section 3 (ROAD_REG, RR_REG), 1d section 0
        for (plain, hb) in [("road_reg", "road_hb1"), ("rr_reg", "rr_hb1")] {
            let own = skins
                .get(plain)
                .is_some_and(|s| s.albedo.as_str().contains("_new_"));
            if !own && let Some(s) = skins.get(hb).cloned() {
                skins.insert(plain.to_owned(), s);
            }
        }
        Self { pieces, skins }
    }

    pub fn get(&self, name: &str) -> Option<PieceInfo> {
        self.pieces.get(name).copied()
    }

    /// How the piece is laid as a decal, if its material has a texture.
    pub fn skin(&self, name: &str) -> Option<&RoadSkin> {
        self.skins.get(name)
    }

    pub fn has_all(&self, names: &[&str]) -> bool {
        names.iter().all(|n| self.pieces.contains_key(*n))
    }
}

/// The part names of a plain piece and a pillaged one for a kind.
pub fn road_names(kind: RouteKind) -> [&'static str; 2] {
    match kind {
        RouteKind::Road => ["road_reg", "road_dam"],
        RouteKind::Railroad => ["rr_reg", "rr_dam"],
    }
}

/// The part names of a bridge: stone or iron.
pub fn bridge_names(iron: bool) -> [&'static str; 3] {
    if iron {
        ["ind_bridge_end1", "ind_bridge_mid", "ind_bridge_end2"]
    } else {
        ["anc_bridge_end1", "anc_bridge_mid", "anc_bridge_end2"]
    }
}

/// The model of a class (`road`, `bridge`) that has the parts asked for.
pub enum Pick<'a> {
    Ready(&'a LogicalId, &'a RouteMeta),
    /// The model has not come; ask for it.
    Pending(&'a LogicalId),
    None,
}

/// Of the models of the class, in the order of the art (`roadsandrails` before `roads_rails`): the
/// first that has all the parts. A model that has not come is asked for. The dirt road of
/// `roadsandrails_d.dds` is the brown track with light edges of the S3 capture; the grey band of
/// `roads_rails.dds` is not.
// TODO(RW-17): which model `cvGameplayRoadHandler` lays (its road type table, stride 0x28)
pub fn pick<'a>(
    art: &'a RouteArt,
    class: &str,
    needs: &[&str],
    metas: &'a BTreeMap<LogicalId, RouteMeta>,
) -> Pick<'a> {
    for m in art.models.iter().filter(|m| m.class == class) {
        match metas.get(&m.model) {
            None => return Pick::Pending(&m.model),
            Some(meta) if meta.has_all(needs) => return Pick::Ready(&m.model, meta),
            Some(_) => {}
        }
    }
    Pick::None
}

/// The route art of a kind in the table.
pub fn route_art(table: &ArtTable, kind: RouteKind) -> Option<&RouteArt> {
    table.route(match kind {
        RouteKind::Road => "ROUTE_ROAD",
        RouteKind::Railroad => "ROUTE_RAILROAD",
    })
}

/// The matrix of a piece that lies from `a` to `b` (points of the world), whose middle is at the
/// middle of the two, turned to run along them and sloped with them.
fn along(a: Vec3, b: Vec3, piece: &PieceInfo) -> Mat4 {
    let f = (b - a).normalize_or_zero();
    let f = if f == Vec3::ZERO { Vec3::Z } else { f };
    let x = Vec3::Y.cross(f).normalize_or_zero();
    let x = if x == Vec3::ZERO { Vec3::X } else { x };
    let y = f.cross(x);
    let mid = (a + b) * 0.5;
    Mat4::from_cols(
        (x * ROUTE_SCALE).extend(0.0),
        (y * ROUTE_SCALE).extend(0.0),
        (f * ROUTE_SCALE).extend(0.0),
        mid.extend(1.0),
    ) * Mat4::from_translation(Vec3::new(-piece.centre.x, 0.0, -piece.centre.y))
}

/// A piece on the line from the middle of the plot `from`, towards `dir`, for `t0..t1` of it.
fn on_line(
    from: Vec2,
    dir: Dir,
    t0: f32,
    t1: f32,
    piece: &PieceInfo,
    surface: &dyn Fn(Vec2) -> f32,
) -> (u16, Mat4) {
    let v = dir_vector(dir);
    let point = |t: f32| {
        let p = from + v * t;
        Vec3::new(p.x, surface(p) + LIFT, p.y)
    };
    (piece.mesh, along(point(t0), point(t1), piece))
}

/// The length of a bridge, world units.
pub fn bridge_length(parts: &[PieceInfo; 3]) -> f32 {
    parts.iter().map(|p| p.length * ROUTE_SCALE).sum()
}

/// The pieces of the bridge on the edge of `link`: end, middle, end, in a row across the edge.
pub fn bridge_pieces(
    plot: (i32, i32),
    link: &Link,
    parts: &[PieceInfo; 3],
    surface: &dyn Fn(Vec2) -> f32,
) -> Vec<(u16, Mat4)> {
    let from = cell_center(plot.0, plot.1);
    let total = bridge_length(parts);
    let mut t = HALF - total * 0.5;
    parts
        .iter()
        .map(|p| {
            let len = p.length * ROUTE_SCALE;
            let piece = on_line(from, link.dir, t, t + len, p, surface);
            t += len;
            piece
        })
        .collect()
}

// ---- the layer: the routes of all the plots ----

/// The pieces of one model that a plot draws for its routes.
#[derive(Debug, Clone, PartialEq)]
pub struct RoutePlaced {
    pub model: LogicalId,
    /// Mesh (= part) and model-to-world matrix of each piece.
    pub pieces: Vec<(u16, Mat4)>,
    /// The model has not come: the plot is placed again when it does.
    pub pending: bool,
}

/// The skin of a kind of route, plain (`false`) or pillaged (`true`).
pub type RoadSkins = BTreeMap<(RouteKind, bool), RoadSkin>;

/// Every link and the plots the road decals are made for.
pub type DecalWork = (Vec<DrawnLink>, BTreeSet<(i32, i32)>);

/// A connection that a plot draws, with the plot, for the road decals.
pub type DrawnLink = ((i32, i32), Link);

/// The routes of the map: what each plot draws of them and the models they wait for.
#[derive(Default)]
pub struct RouteLayer {
    net: Option<(GridShape, BTreeSet<(i32, i32)>)>,
    dirty: BTreeSet<PlotKey>,
    all: bool,
    metas: BTreeMap<LogicalId, RouteMeta>,
    placed: BTreeMap<PlotKey, Vec<RoutePlaced>>,
    links: BTreeMap<PlotKey, Vec<Link>>,
    /// The links changed since the decals were last made.
    links_changed: bool,
    /// The plots the decals of the roads were last made for.
    decaled: BTreeSet<(i32, i32)>,
    /// How the road pieces of the models that came are textured.
    skins: RoadSkins,
    /// The viewer's bridges; `None` until the host tells (SIMP-120).
    bridges: Option<BridgeState>,
}

fn coords(key: PlotKey) -> (i32, i32) {
    ((key & 0xffff) as i32, (key >> 16) as i32)
}

impl RouteLayer {
    /// The plot changed (or its route did): it and its neighbours are placed again.
    pub fn touch(&mut self, key: PlotKey) {
        self.dirty.insert(key);
    }

    /// Everything is placed again at the next refresh.
    pub fn touch_all(&mut self) {
        self.all = true;
    }

    /// The viewer's bridges changed (or became known): everything is placed again. True if it changed.
    pub fn set_bridges(&mut self, state: Option<BridgeState>) -> bool {
        if self.bridges == state {
            return false;
        }
        self.bridges = state;
        self.all = true;
        true
    }

    /// The viewer's bridges, if the host told.
    pub fn bridges(&self) -> Option<BridgeState> {
        self.bridges
    }

    /// The shape of the map and the plots with a city. A change places everything again.
    pub fn set_net(&mut self, shape: GridShape, cities: BTreeSet<(i32, i32)>) -> bool {
        if self
            .net
            .as_ref()
            .is_some_and(|(s, c)| *s == shape && *c == cities)
        {
            return false;
        }
        self.net = Some((shape, cities));
        self.all = true;
        true
    }

    /// A route model arrived: its pieces are kept and the plots that wait for it placed again.
    pub fn register(&mut self, id: LogicalId, blob: &ModelBlobV2) {
        self.metas.insert(id.clone(), RouteMeta::of(blob));
        self.invalidate_model(&id);
    }

    /// The plots that wait for `model`, or draw it, are placed again.
    pub fn invalidate_model(&mut self, model: &LogicalId) {
        for (key, list) in &self.placed {
            if list.iter().any(|p| &p.model == model) {
                self.dirty.insert(*key);
            }
        }
    }

    /// True if a plot waits for `model`.
    pub fn wants_model(&self, model: &LogicalId) -> bool {
        self.placed
            .values()
            .flatten()
            .any(|p| p.pending && &p.model == model)
    }

    /// The models that plots wait for.
    pub fn models_wanted(&self) -> BTreeSet<LogicalId> {
        self.placed
            .values()
            .flatten()
            .filter(|p| p.pending)
            .map(|p| p.model.clone())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.placed.len()
    }

    pub fn is_empty(&self) -> bool {
        self.placed.is_empty()
    }

    /// Every piece to draw: the model, the mesh, the matrix.
    pub fn pieces(&self) -> impl Iterator<Item = (&LogicalId, u16, &Mat4)> {
        self.placed
            .values()
            .flatten()
            .filter(|p| !p.pending)
            .flat_map(|p| p.pieces.iter().map(move |(m, mat)| (&p.model, *m, mat)))
    }

    /// The plots to place: the dirty ones and their neighbours (a link has two ends), or all.
    fn work(
        &mut self,
        plots: &Keyed<PlotKey, PlotDecor>,
        shape: &GridShape,
        cities: &BTreeSet<(i32, i32)>,
    ) -> BTreeSet<PlotKey> {
        if std::mem::take(&mut self.all) {
            self.dirty.clear();
            return plots
                .keys()
                .copied()
                .chain(cities.iter().map(|&(x, y)| plot_key(x, y)))
                .chain(self.placed.keys().copied())
                .collect();
        }
        let dirty = std::mem::take(&mut self.dirty);
        let mut out = dirty.clone();
        for key in dirty {
            let (x, y) = coords(key);
            for dir in Dir::ALL {
                if let Some((nx, ny)) = shape.neighbor(x, y, dir) {
                    out.insert(plot_key(nx, ny));
                }
            }
        }
        out
    }

    /// Places again what changed; returns how many plots.
    pub fn refresh(&mut self, plots: &Keyed<PlotKey, PlotDecor>, ctx: &B2Context<'_>) -> usize {
        let Some((shape, cities)) = self.net.clone() else {
            return 0;
        };
        // The skins of the road models that came; the decals are made again when one comes.
        let mut skins = RoadSkins::new();
        for kind in [RouteKind::Road, RouteKind::Railroad] {
            let Some(route) = route_art(ctx.art, kind) else {
                continue;
            };
            for (pillaged, name) in road_names(kind).into_iter().enumerate() {
                if let Pick::Ready(_, meta) = pick(route, "road", &[name], &self.metas)
                    && let Some(skin) = meta.skin(name)
                {
                    skins.insert((kind, pillaged == 1), skin.clone());
                }
            }
        }
        if skins != self.skins {
            self.skins = skins;
            self.links_changed = true;
        }
        let work = self.work(plots, &shape, &cities);
        let tile = |x: i32, y: i32| {
            let p = plots.get(&plot_key(x, y));
            Tile {
                route: p.and_then(|p| p.route),
                pillaged: p.is_some_and(|p| p.route_pillaged),
                rivers: p.map_or(0, |p| p.rivers),
                city: cities.contains(&(x, y)),
                industrial: p
                    .and_then(|p| p.owner)
                    .and_then(|o| ctx.owners.player(o))
                    .is_some_and(|pl| pl.industrial),
            }
        };
        let surface = |p: Vec2| {
            let g = (ctx.ground)(p);
            ctx.water_level.map_or(g, |w| g.max(w))
        };
        let n = work.len();
        for key in work {
            let (x, y) = coords(key);
            let mut mine = links(&shape, (x, y), &|x, y| plot_key(x, y), &tile);
            if let Some(state) = self.bridges {
                state.apply(&mut mine);
            }
            if mine.is_empty() {
                if self.links.remove(&key).is_some() {
                    self.links_changed = true;
                }
                self.placed.remove(&key);
                continue;
            }
            if self.links.get(&key) != Some(&mine) {
                self.links_changed = true;
            }
            let list = self.place(ctx.art, (x, y), &mine, &surface);
            self.links.insert(key, mine);
            if list.is_empty() {
                self.placed.remove(&key);
            } else {
                self.placed.insert(key, list);
            }
        }
        n
    }

    /// The models of the links of one plot.
    fn place(
        &self,
        art: &ArtTable,
        plot: (i32, i32),
        links: &[Link],
        surface: &dyn Fn(Vec2) -> f32,
    ) -> Vec<RoutePlaced> {
        let mut by_model: BTreeMap<LogicalId, (Vec<(u16, Mat4)>, bool)> = BTreeMap::new();
        let mut add = |id: &LogicalId, pieces: Vec<(u16, Mat4)>, pending: bool| {
            let e = by_model.entry(id.clone()).or_default();
            e.0.extend(pieces);
            e.1 |= pending;
        };
        for link in links {
            let Some(route) = route_art(art, link.kind) else {
                continue;
            };
            let [plain, damaged] = road_names(link.kind);
            let name = if link.pillaged { damaged } else { plain };
            if link.river && link.bridge {
                let names = bridge_names(link.iron);
                match pick(route, "bridge", &names, &self.metas) {
                    Pick::Ready(id, meta) => {
                        let parts = names.map(|n| meta.get(n).expect("has_all"));
                        add(id, bridge_pieces(plot, link, &parts, surface), false);
                    }
                    Pick::Pending(id) => add(id, Vec::new(), true),
                    Pick::None => {}
                }
            }
            // The road itself is a decal (`objects::decals`): the model is only asked for, for the
            // skin of its piece.
            match pick(route, "road", &[name], &self.metas) {
                Pick::Ready(id, _) => add(id, Vec::new(), false),
                Pick::Pending(id) => add(id, Vec::new(), true),
                Pick::None => {}
            }
        }
        by_model
            .into_iter()
            .map(|(model, (pieces, pending))| RoutePlaced {
                model,
                pieces,
                pending,
            })
            .collect()
    }

    /// What the decals of the roads need if the links changed: every link, and the plots to give
    /// the layer (those with links now and those that had them). `None` when nothing changed.
    pub fn decal_update(&self) -> Option<DecalWork> {
        if !self.links_changed {
            return None;
        }
        let mut all: Vec<DrawnLink> = self
            .links
            .iter()
            .flat_map(|(k, l)| l.iter().map(move |l| (coords(*k), *l)))
            .collect();
        all.sort_by_key(|((x, y), l)| (*y, *x, l.dir.index()));
        let touched = all
            .iter()
            .map(|(p, _)| *p)
            .chain(self.decaled.iter().copied())
            .collect();
        Some((all, touched))
    }

    /// How the road pieces are textured, by kind and pillaged.
    pub fn skins(&self) -> &RoadSkins {
        &self.skins
    }

    /// The plot has a city (its roads are spokes).
    pub fn is_city(&self, x: i32, y: i32) -> bool {
        self.net.as_ref().is_some_and(|(_, c)| c.contains(&(x, y)))
    }

    /// The decals are made again at the next `decal_update` (the coastal roads came).
    pub fn redo_decals(&mut self) {
        self.links_changed = true;
    }

    /// The decals were made for these plots.
    pub fn decals_applied(&mut self, links: &[DrawnLink]) {
        self.decaled = links.iter().map(|(p, _)| *p).collect();
        self.links_changed = false;
    }
}

#[cfg(test)]
mod tests {
    use asset_format::{MaterialDesc, MeshArrays, MeshV2, Part, RouteModel, Transform};

    use super::*;

    fn shape() -> GridShape {
        GridShape {
            width: 8,
            height: 8,
            wrap_x: false,
        }
    }

    fn key(x: i32, y: i32) -> u32 {
        (y * 8 + x) as u32
    }

    fn road() -> Tile {
        Tile {
            route: Some(RouteKind::Road),
            ..Tile::default()
        }
    }

    fn rail() -> Tile {
        Tile {
            route: Some(RouteKind::Railroad),
            ..Tile::default()
        }
    }

    fn city() -> Tile {
        Tile {
            city: true,
            ..Tile::default()
        }
    }

    fn map(tiles: &[((i32, i32), Tile)]) -> impl Fn(i32, i32) -> Tile + '_ {
        move |x, y| {
            tiles
                .iter()
                .find(|(p, _)| *p == (x, y))
                .map_or(Tile::default(), |(_, t)| *t)
        }
    }

    fn dirs(l: &[Link]) -> Vec<Dir> {
        l.iter().map(|l| l.dir).collect()
    }

    #[test]
    fn a_road_joins_the_neighbours_that_have_a_route_or_a_city() {
        let t = [((3, 2), road()), ((4, 2), road()), ((2, 2), city())];
        let f = map(&t);
        let l = links(&shape(), (3, 2), &key, &f);
        assert_eq!(dirs(&l), [Dir::East, Dir::West]);
        // A plot with nothing draws nothing, and a lone road has no links.
        assert!(links(&shape(), (6, 6), &key, &f).is_empty());
        let lone = [((3, 2), road())];
        assert!(links(&shape(), (3, 2), &key, &map(&lone)).is_empty());
    }

    #[test]
    fn a_city_joins_only_the_routes_next_to_it() {
        let t = [((3, 2), city()), ((4, 2), road()), ((2, 2), city())];
        let f = map(&t);
        let l = links(&shape(), (3, 2), &key, &f);
        assert_eq!(dirs(&l), [Dir::East]);
        assert_eq!(l[0].kind, RouteKind::Road);
        assert!(!l[0].bridge);
    }

    #[test]
    fn the_kind_of_a_link_is_the_lesser_of_its_ends() {
        let t = [((3, 2), rail()), ((4, 2), road()), ((2, 2), rail())];
        let f = map(&t);
        let l = links(&shape(), (3, 2), &key, &f);
        assert_eq!(l.len(), 2);
        let east = l.iter().find(|l| l.dir == Dir::East).unwrap();
        let west = l.iter().find(|l| l.dir == Dir::West).unwrap();
        assert_eq!(east.kind, RouteKind::Road);
        assert_eq!(west.kind, RouteKind::Railroad);
        assert!(west.iron && !east.iron);
    }

    #[test]
    fn a_river_on_the_edge_gives_one_of_the_two_plots_the_bridge() {
        let mut a = road();
        a.rivers = Dir::East.edge_bit();
        let b = road();
        let t = [((3, 2), a), ((4, 2), b)];
        let f = map(&t);
        let la = links(&shape(), (3, 2), &key, &f);
        let lb = links(&shape(), (4, 2), &key, &f);
        assert!(la[0].river && lb[0].river, "both see the river");
        assert!(la[0].bridge && !lb[0].bridge, "the lower key builds it");
        // A river that only the neighbour has on its side is the same edge.
        let mut b2 = road();
        b2.rivers = Dir::West.edge_bit();
        let t2 = [((3, 2), road()), ((4, 2), b2)];
        let f2 = map(&t2);
        assert!(links(&shape(), (3, 2), &key, &f2)[0].bridge);
        // A road to a city: the road draws the bridge.
        let t3 = [((3, 2), a), ((4, 2), city())];
        assert!(links(&shape(), (3, 2), &key, &map(&t3))[0].bridge);
    }

    #[test]
    fn the_pillaged_plot_pillages_its_own_half() {
        let mut a = road();
        a.pillaged = true;
        let t = [((3, 2), a), ((4, 2), road())];
        let f = map(&t);
        assert!(links(&shape(), (3, 2), &key, &f)[0].pillaged);
        assert!(!links(&shape(), (4, 2), &key, &f)[0].pillaged);
    }

    fn textured(files: &[&str]) -> MaterialDesc {
        use asset_format::{ParamSet, TexKind, TexParam, TexSource};
        MaterialDesc {
            package: "LandmarkShader".into(),
            sets: vec![ParamSet {
                blocks: vec![],
                textures: files
                    .iter()
                    .zip(["BaseTextureMap", "HeightTextureMap"])
                    .map(|(f, name)| TexParam {
                        name: name.into(),
                        kind: TexKind::Tex2D,
                        src: TexSource::File(LogicalId::new(*f)),
                        semantic: 0,
                        reg: 0,
                        usage: 0,
                        flags: 0,
                        active: true,
                    })
                    .collect(),
                constants: vec![],
            }],
            ..MaterialDesc::default()
        }
    }

    #[test]
    fn the_skin_of_a_road_piece_is_read_from_its_mesh_and_material() {
        // As `Road_reg` of `roads_rails.gr2`: u runs along Z from 1 to 0, v across X.
        let a = MeshArrays {
            positions: vec![
                [369.7, 0.0, 176.7],
                [344.0, 0.0, 176.7],
                [344.0, 0.0, 304.7],
                [369.7, 0.0, 304.7],
            ],
            normals: vec![[0.0, 1.0, 0.0]; 4],
            uvs: vec![[1.0, 0.93], [1.0, 0.99], [0.0, 0.99], [0.0, 0.93]],
            joints: vec![],
            weights: vec![],
            indices: vec![0, 1, 2, 0, 2, 3],
        };
        let blob = ModelBlobV2 {
            parts: vec![Part {
                name: "Road_reg".into(),
                first_mesh: 0,
                mesh_count: 1,
                placement: Transform::IDENTITY,
            }],
            meshes: vec![MeshV2::pack("Road_reg", 0, 0, &a, false, false).unwrap()],
            materials: vec![textured(&["tx:roads_rails.dds", "tx:roads_rails_h.dds"])],
            ..ModelBlobV2::default()
        };
        let s = RouteMeta::of(&blob).skin("road_reg").cloned().unwrap();
        assert_eq!(s.albedo.as_str(), "tx:roads_rails.dds");
        assert_eq!(s.height.unwrap().as_str(), "tx:roads_rails_h.dds");
        assert!(s.along_is_u);
        assert_eq!(s.along, [1.0, 0.0]);
        // The `v` of the blob is turned back to the original's.
        assert!((s.across[0] - 0.07).abs() < 1e-3 && (s.across[1] - 0.01).abs() < 1e-3);
        assert!((s.width - 25.7).abs() < 1e-3 && (s.length - 128.0).abs() < 1e-3);
        // A piece without a texture has no skin.
        assert!(
            RouteMeta::of(&piece_blob(&["Road_reg"]))
                .skin("road_reg")
                .is_none()
        );
    }

    #[test]
    fn the_plain_road_is_the_gravel_of_the_first_border_piece() {
        let mut blob = piece_blob(&["Road_reg", "Road_HB1"]);
        blob.materials = vec![textured(&["tx:roadsandrails_d.dds"])];
        let m = RouteMeta::of(&blob);
        assert_eq!(m.skin("road_reg"), m.skin("road_hb1"));
        assert!(m.skin("road_reg").is_some());
    }

    #[test]
    fn the_plain_road_of_the_new_model_is_its_own_road_reg_and_not_the_border_piece() {
        let mut blob = piece_blob(&["Road_reg", "Road_HB1", "RR_reg", "RR_HB1"]);
        blob.materials = vec![textured(&["tx:roadsandrails_new_d.dds"])];
        // The border pieces lie elsewhere in the atlas than the plain ones.
        for i in [1usize, 3] {
            let x = 100.0 * i as f32;
            let a = MeshArrays {
                positions: vec![
                    [x, 0.0, -50.0],
                    [x + 26.0, 0.0, -50.0],
                    [x + 26.0, 0.0, 78.0],
                    [x, 0.0, 78.0],
                ],
                normals: vec![[0.0, 1.0, 0.0]; 4],
                uvs: vec![[0.0, 0.0], [0.5, 0.0], [0.5, 0.5], [0.0, 0.5]],
                joints: vec![],
                weights: vec![],
                indices: vec![0, 1, 2, 0, 2, 3],
            };
            blob.meshes[i] = MeshV2::pack("HB", i as u16, 0, &a, false, false).unwrap();
        }
        let m = RouteMeta::of(&blob);
        assert_ne!(m.skin("road_reg"), m.skin("road_hb1"));
        assert_ne!(m.skin("rr_reg"), m.skin("rr_hb1"));
        assert!(m.skin("road_reg").is_some() && m.skin("rr_reg").is_some());
    }

    fn piece_blob(names: &[&str]) -> ModelBlobV2 {
        let mut meshes = Vec::new();
        let mut parts = Vec::new();
        for (i, name) in names.iter().enumerate() {
            let x = 100.0 * i as f32;
            let a = MeshArrays {
                positions: vec![
                    [x, 0.0, -50.0],
                    [x + 26.0, 0.0, -50.0],
                    [x + 26.0, 0.0, 78.0],
                    [x, 0.0, 78.0],
                ],
                normals: vec![[0.0, 1.0, 0.0]; 4],
                uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                joints: vec![],
                weights: vec![],
                indices: vec![0, 1, 2, 0, 2, 3],
            };
            meshes.push(MeshV2::pack(*name, i as u16, 0, &a, false, false).unwrap());
            parts.push(Part {
                name: (*name).into(),
                first_mesh: i as u16,
                mesh_count: 1,
                placement: Transform::IDENTITY,
            });
        }
        ModelBlobV2 {
            parts,
            meshes,
            materials: vec![MaterialDesc::default()],
            ..ModelBlobV2::default()
        }
    }

    #[test]
    fn the_pieces_of_a_route_model_are_found_by_the_name_of_the_part() {
        let m = RouteMeta::of(&piece_blob(&["Road_reg", "RR_dam", "Scene"]));
        assert!(m.has_all(&["road_reg", "rr_dam"]));
        assert!(!m.has_all(&["road_reg", "road_dam"]));
        let p = m.get("rr_dam").unwrap();
        assert_eq!(p.mesh, 1);
        assert_eq!(p.length, 128.0);
        assert_eq!(p.centre, Vec2::new(113.0, 14.0));
        assert!(
            RouteMeta::of(&piece_blob(&["Scene"]))
                .get("scene")
                .is_none()
        );
    }

    fn art() -> RouteArt {
        let m = |class: &str, id: &str| RouteModel {
            class: class.into(),
            model: LogicalId::new(id),
        };
        RouteArt {
            route: "ROUTE_ROAD".into(),
            models: vec![
                m("road", "md:roadsandrails"),
                m("road", "md:roads_rails"),
                m("bridge", "md:bridges"),
            ],
        }
    }

    #[test]
    fn the_model_with_the_parts_is_picked_and_one_that_has_not_come_is_asked_for() {
        let art = art();
        let mut metas = BTreeMap::new();
        let need = ["road_reg", "road_dam"];
        let Pick::Pending(id) = pick(&art, "road", &need, &metas) else {
            panic!("asks");
        };
        assert_eq!(id.as_str(), "md:roadsandrails", "in the order of the art");
        metas.insert(
            LogicalId::new("md:roadsandrails"),
            RouteMeta::of(&piece_blob(&["Road_reg"])),
        );
        // The first has no pillaged piece: the next one is asked for.
        let Pick::Pending(id) = pick(&art, "road", &need, &metas) else {
            panic!("asks");
        };
        assert_eq!(id.as_str(), "md:roads_rails");
        metas.insert(
            LogicalId::new("md:roads_rails"),
            RouteMeta::of(&piece_blob(&["Road_reg", "Road_dam"])),
        );
        let Pick::Ready(id, _) = pick(&art, "road", &need, &metas) else {
            panic!("ready");
        };
        assert_eq!(id.as_str(), "md:roads_rails");
        assert!(matches!(
            pick(&art, "bridge", &bridge_names(false), &metas),
            Pick::Pending(_)
        ));
        assert!(matches!(pick(&art, "river", &need, &metas), Pick::None));
    }

    #[test]
    fn a_bridge_piece_lies_along_the_line_with_its_middle_in_the_middle() {
        let m = RouteMeta::of(&piece_blob(&["Road_reg"]));
        let p = m.get("road_reg").unwrap();
        let ground = |_: Vec2| 5.0;
        let c = cell_center(2, 2);
        let (mesh, mat) = on_line(c, Dir::East, 0.0, 16.0, &p, &ground);
        assert_eq!(mesh, 0);
        // The middle of the piece (model centre) lands on the line, at the right distance.
        let at = mat.transform_point3(Vec3::new(p.centre.x, 0.0, p.centre.y));
        assert!(
            (at.x - (c.x + 8.0)).abs() < 1e-3 && (at.z - c.y).abs() < 1e-3,
            "{at:?}"
        );
        assert!((at.y - (5.0 + LIFT)).abs() < 1e-3);
        // Its length runs along the line (east): the model's Z axis points east.
        let along = mat.transform_vector3(Vec3::Z);
        assert!(along.x > 0.0 && along.z.abs() < 1e-4);
        assert!((along.length() - ROUTE_SCALE).abs() < 1e-5);
        // Its width runs across: the model's X is horizontal and across the line.
        let across = mat.transform_vector3(Vec3::X);
        assert!(across.dot(along).abs() < 1e-6 && across.y.abs() < 1e-5);
    }

    fn route_table() -> ArtTable {
        let mut t = ArtTable::new();
        for route in ["ROUTE_ROAD", "ROUTE_RAILROAD"] {
            t.insert_route(RouteArt {
                route: route.into(),
                models: vec![
                    RouteModel {
                        class: "road".into(),
                        model: LogicalId::new("md:roads_rails"),
                    },
                    RouteModel {
                        class: "bridge".into(),
                        model: LogicalId::new("md:bridges"),
                    },
                ],
            });
        }
        t
    }

    fn road_plot(x: i32, y: i32) -> PlotDecor {
        PlotDecor {
            x,
            y,
            route: Some(RouteKind::Road),
            ..PlotDecor::default()
        }
    }

    fn layer_for(
        layer: &mut RouteLayer,
        plots: &Keyed<PlotKey, PlotDecor>,
        art: &ArtTable,
    ) -> usize {
        let owners = crate::entities::Owners::default();
        let ground = |_: Vec2| 0.0;
        layer.refresh(
            plots,
            &B2Context {
                art,
                owners: &owners,
                ground: &ground,
                water_level: None,
            },
        )
    }

    #[test]
    fn the_layer_waits_for_the_model_places_the_links_and_follows_the_plots() {
        let art = route_table();
        let mut plots: Keyed<PlotKey, PlotDecor> = Keyed::default();
        for x in [3, 4] {
            plots.upsert(plot_key(x, 2), road_plot(x, 2));
        }
        let mut layer = RouteLayer::default();
        // Without the shape of the map nothing is placed.
        assert_eq!(layer_for(&mut layer, &plots, &art), 0);
        layer.set_net(shape(), BTreeSet::new());
        assert!(layer_for(&mut layer, &plots, &art) >= 2);
        // The model has not come: it is asked for and nothing is drawn.
        let id = LogicalId::new("md:roads_rails");
        assert_eq!(layer.models_wanted(), BTreeSet::from([id.clone()]));
        assert!(layer.wants_model(&id) && layer.pieces().count() == 0);
        // It comes: the road is a decal, so nothing is drawn as a model; the plots keep it.
        layer.register(id.clone(), &piece_blob(&["Road_reg", "Road_dam"]));
        layer_for(&mut layer, &plots, &art);
        assert!(layer.models_wanted().is_empty());
        assert_eq!(layer.pieces().count(), 0);
        assert!(!layer.is_empty());
        // The plot that is pillaged has a pillaged link on its half only.
        let mut pillaged = road_plot(3, 2);
        pillaged.route_pillaged = true;
        plots.upsert(plot_key(3, 2), pillaged);
        layer.touch(plot_key(3, 2));
        layer_for(&mut layer, &plots, &art);
        let (links, _) = layer.decal_update().unwrap();
        assert_eq!(links.iter().filter(|(_, l)| l.pillaged).count(), 1);
        // The neighbour is placed again when a plot changes: a third road joins the line.
        plots.upsert(plot_key(5, 2), road_plot(5, 2));
        layer.touch(plot_key(5, 2));
        layer_for(&mut layer, &plots, &art);
        // The decals ask for every link once, and again only when the links change.
        let (links, touched) = layer.decal_update().unwrap();
        assert_eq!(links.len(), 4);
        assert!(touched.contains(&(4, 2)));
        layer.decals_applied(&links);
        assert!(layer.decal_update().is_none());
        // A road that goes leaves nothing, and its plot is still to be cleaned of decals.
        plots.remove(&plot_key(5, 2));
        layer.touch(plot_key(5, 2));
        layer_for(&mut layer, &plots, &art);
        let (links, touched) = layer.decal_update().unwrap();
        assert_eq!(links.len(), 2);
        assert!(touched.contains(&(5, 2)));
    }

    #[test]
    fn the_bridge_stands_on_the_edge_and_the_road_is_no_model() {
        let art = route_table();
        let mut plots: Keyed<PlotKey, PlotDecor> = Keyed::default();
        let mut a = road_plot(3, 2);
        a.rivers = Dir::East.edge_bit();
        plots.upsert(plot_key(3, 2), a);
        plots.upsert(plot_key(4, 2), road_plot(4, 2));
        let mut layer = RouteLayer::default();
        layer.set_net(shape(), BTreeSet::new());
        layer.register(
            LogicalId::new("md:roads_rails"),
            &piece_blob(&["Road_reg", "Road_dam"]),
        );
        layer.register(
            LogicalId::new("md:bridges"),
            &piece_blob(&["ANC_Bridge_End1", "ANC_Bridge_Mid", "ANC_Bridge_End2"]),
        );
        layer_for(&mut layer, &plots, &art);
        assert!(layer.models_wanted().is_empty());
        let bridge: Vec<_> = layer
            .pieces()
            .filter(|(m, _, _)| m.as_str() == "md:bridges")
            .collect();
        // Three pieces, drawn by one of the two plots.
        assert_eq!(bridge.len(), 3);
        let roads = layer
            .pieces()
            .filter(|(m, _, _)| m.as_str() == "md:roads_rails")
            .count();
        assert_eq!(roads, 0);
    }

    fn bridge_pieces_of(layer: &RouteLayer) -> usize {
        layer
            .pieces()
            .filter(|(m, _, _)| m.as_str() == "md:bridges")
            .count()
    }

    /// The bridge is the viewer's: no bridge over a river until the viewer's team has the technology, and the
    /// routes are placed again when it arrives (the players' report 2026-10-30).
    #[test]
    fn the_bridge_follows_the_technology_of_the_viewer() {
        let art = route_table();
        let mut plots: Keyed<PlotKey, PlotDecor> = Keyed::default();
        let mut a = road_plot(3, 2);
        a.rivers = Dir::East.edge_bit();
        plots.upsert(plot_key(3, 2), a);
        plots.upsert(plot_key(4, 2), road_plot(4, 2));
        let mut layer = RouteLayer::default();
        layer.set_net(shape(), BTreeSet::new());
        layer.register(
            LogicalId::new("md:roads_rails"),
            &piece_blob(&["Road_reg", "Road_dam"]),
        );
        layer.register(
            LogicalId::new("md:bridges"),
            &piece_blob(&[
                "ANC_Bridge_End1",
                "ANC_Bridge_Mid",
                "ANC_Bridge_End2",
                "IND_Bridge_End1",
                "IND_Bridge_Mid",
                "IND_Bridge_End2",
            ]),
        );
        // Not told yet: the old behaviour, a bridge.
        layer_for(&mut layer, &plots, &art);
        assert_eq!(bridge_pieces_of(&layer), 3);
        // Told: the viewer has no bridge building. The road is a ford: no bridge, the link keeps its river.
        assert!(layer.set_bridges(Some(BridgeState {
            bridge: false,
            iron: false
        })));
        assert!(!layer.set_bridges(Some(BridgeState {
            bridge: false,
            iron: false
        })));
        layer_for(&mut layer, &plots, &art);
        assert_eq!(bridge_pieces_of(&layer), 0);
        let (links, _) = layer.decal_update().unwrap();
        assert!(links.iter().all(|(_, l)| l.river && !l.bridge));
        // The technology arrives: the bridge is made, stone.
        assert!(layer.set_bridges(Some(BridgeState {
            bridge: true,
            iron: false
        })));
        layer_for(&mut layer, &plots, &art);
        assert_eq!(bridge_pieces_of(&layer), 3);
        let (links, _) = layer.decal_update().unwrap();
        assert!(links.iter().any(|(_, l)| l.bridge && !l.iron));
        // The viewer's era is the Modern one: iron, whoever owns the plot.
        layer.set_bridges(Some(BridgeState {
            bridge: true,
            iron: true,
        }));
        layer_for(&mut layer, &plots, &art);
        assert_eq!(bridge_pieces_of(&layer), 3);
        let (links, _) = layer.decal_update().unwrap();
        assert!(links.iter().any(|(_, l)| l.bridge && l.iron));
    }

    /// The real models of the pack (`CIV5_ASSETS_B2`) have the pieces the layer asks for.
    #[test]
    fn the_real_route_models_have_the_pieces_of_the_layer() {
        let Some(dir) = std::env::var_os("CIV5_ASSETS_B2") else {
            eprintln!("skipped: CIV5_ASSETS_B2 is not set");
            return;
        };
        let mut metas = BTreeMap::new();
        let mut tables = Vec::new();
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let Ok(bytes) = std::fs::read(e.path()) else {
                continue;
            };
            match bytes.get(..4) {
                Some(b"C5MD") => {
                    if let Ok(m) = ModelBlobV2::decode(&bytes) {
                        let id = LogicalId::new(format!("md:{}", m.source.to_ascii_lowercase()));
                        if matches!(
                            id.as_str(),
                            "md:roads_rails"
                                | "md:roadsandrails"
                                | "md:roadsandrails_new"
                                | "md:bridges"
                        ) {
                            metas.insert(id, RouteMeta::of(&m));
                        }
                    }
                }
                Some(b"C5AT") => {
                    if let Ok(t) = ArtTable::decode(&bytes) {
                        tables.push(t);
                    }
                }
                _ => {}
            }
        }
        let Some(table) = tables.iter().find(|t| t.route("ROUTE_ROAD").is_some()) else {
            panic!("no table with the routes");
        };
        for kind in [RouteKind::Road, RouteKind::Railroad] {
            let art = route_art(table, kind).unwrap();
            for name in road_names(kind) {
                let Pick::Ready(id, meta) = pick(art, "road", &[name], &metas) else {
                    panic!("no road model with {name}");
                };
                // The piece is laid as a decal: its material names the texture of its model, and
                // the texture runs along the piece.
                let skin = meta.skin(name).expect("a textured piece");
                let want = match id.as_str() {
                    "md:roadsandrails_new" => "tx:roadsandrails_new_d.dds",
                    "md:roadsandrails" => "tx:roadsandrails_d.dds",
                    _ => "tx:roads_rails.dds",
                };
                assert_eq!(skin.albedo.as_str(), want, "{name}");
                assert!(skin.length > skin.width, "{name} {skin:?}");
                assert!(
                    skin.along
                        .iter()
                        .chain(&skin.across)
                        .all(|t| (0.0..=1.0).contains(t))
                );
            }
            for iron in [false, true] {
                let names = bridge_names(iron);
                let Pick::Ready(_, meta) = pick(art, "bridge", &names, &metas) else {
                    panic!("no bridge for {names:?}");
                };
                let parts = names.map(|n| meta.get(n).unwrap());
                let total = bridge_length(&parts);
                // A bridge is about a river wide: well inside the hex, well over a road's width.
                assert!((8.0..20.0).contains(&total), "{total}");
            }
        }
    }

    #[test]
    fn a_piece_is_sloped_with_the_ground() {
        let m = RouteMeta::of(&piece_blob(&["Road_reg"]));
        let p = m.get("road_reg").unwrap();
        let slope = |q: Vec2| q.x * 0.5;
        let (_, mat) = on_line(cell_center(2, 2), Dir::East, 0.0, 16.0, &p, &slope);
        let f = mat.transform_vector3(Vec3::Z);
        assert!((f.y / f.x - 0.5).abs() < 1e-3, "{f:?}");
    }

    #[test]
    fn a_bridge_is_end_middle_end_across_the_edge() {
        let m = RouteMeta::of(&piece_blob(&[
            "ANC_Bridge_End1",
            "ANC_Bridge_Mid",
            "ANC_Bridge_End2",
        ]));
        let parts = bridge_names(false).map(|n| m.get(n).unwrap());
        let link = Link {
            dir: Dir::East,
            kind: RouteKind::Road,
            pillaged: false,
            river: true,
            bridge: true,
            iron: false,
        };
        let ground = |_: Vec2| 0.0;
        let pieces = bridge_pieces((2, 2), &link, &parts, &ground);
        assert_eq!(pieces.len(), 3);
        let c = cell_center(2, 2);
        let total = bridge_length(&parts);
        let xs: Vec<f32> = pieces
            .iter()
            .zip(&parts)
            .map(|((_, mat), p)| {
                mat.transform_point3(Vec3::new(p.centre.x, 0.0, p.centre.y))
                    .x
                    - c.x
            })
            .collect();
        // In a row from the plot's side outwards, centred on the edge (32 from the middle).
        assert!(xs[0] < xs[1] && xs[1] < xs[2]);
        assert!(((xs[0] + xs[2]) / 2.0 - HALF).abs() < 1e-3);
        assert!((xs[2] - xs[0] - (total - parts[0].length * ROUTE_SCALE)).abs() < 1e-2);
    }
}
