//! The Great Wall (world-objects 4.4, WOB5, card R5.11): the wonder's wall along the border of the
//! owner's land around the city that has it (`ART_DEF_BUILDING_GREAT_WALL`).
//!
//! The original (`cvGreatWallSim::BuildGreatWall`) keeps a hex grid of the owners
//! (`GreatWallHexData`, fed by `OnHexCultureChanged`) and, for the city of the wonder:
//!
//! 1. floods the hexes of the city's owner that lie at most [`RADIUS`] from the city
//!    (`HexGridFloodFill<..GreatWallHexData..>`, its callback checks the owner and the distance);
//! 2. walks from the city along `HEXVec_i::TRANSLATIONS[0]` to the last hex of that region at most
//!    five steps out and takes its side 0 (code 0, the hex on the right) as the first edge;
//! 3. walks the edges of the region's border (at most [`MAX_STEPS`] steps) back to the first one and
//!    asks `GameSplineDatabase::QueryHexSegment` for the control points of every edge;
//! 4. puts the points into the original's world (`x - half.x`, `y * 0.866 - half.y`, the height of
//!    `cvTerrainSampler::GetWorldHeight`), closes the loop with the first two points and cuts the
//!    corners twice (`B_SPLINE::ReconstructScalingFunc`): `n` points become `4n + 2`;
//! 5. stands a tower on every 16th point but the first (`AddGreatWallTower`, `t = i - 0.5`, the yaw
//!    of the spline's gradient) and hands every point to `AddGreatWallVerts`;
//! 6. `cvDynamicGeometryVisSystem` (`BuildGreatWall`) bends the model `WALL1` of `HB_Great_Wall.gr2`
//!    along the whole spline at the scale 0.1 (`AddSplineModel`: as many copies as the spline is
//!    long, each stretched over its share of the length) and adds `TOWER1` or `TOWER2` at random
//!    (`rand() % 2`) at the towers (`AddModel`: scale 0.1, turned about z by the yaw).
//!
//! [`place`] is that placement as a pure function: territory and city in, models out, in the
//! original's world (x east, y north, z up). [`placed`] turns it into the pieces of a city.
//!
//! SIMPLIFIED(SIMP-067): the wall is rigid copies of `WALL1` along the spline (each at the middle of
//! its share of the length, turned along the gradient there and stretched to fill the share) where
//! the original bends every vertex of every copy along the spline (`AddSplineModel`); the edge
//! segments are the generated sides of the hexes (`GenerateHexSide`), not the saved segments of the
//! rivers; the border walk is the geometric one (the next edge with the region on its right), the
//! comparator of the original (`0x087c992a`) is not read line by line; the flag `0x2` of
//! `GreatWallHexData` that the flood callback tests is not known and not tested; `rand() % 2` of the
//! towers is a hash of the city and the point; the tree cuts (`*_TREECUT`, `AddSplineDecal`,
//! `AddDecal` into the forest stencil) are not made; the arc length is the length of the polyline
//! of the knots (`CalculateApproxSplineLength` is not read).

use std::collections::{HashSet, VecDeque};

use asset_format::{LogicalId, ModelBlobV2};
use glam::{Mat4, Vec2, Vec3};

use super::{Piece, Placed};
use crate::hexgrid::GridShape;
use crate::world::landmark::select::mix;
use crate::world::terrain::sampler::TerrainFrame;
use crate::world::water::coast::bspline::{gradient_weights, locate, value_weights};
use crate::world::water::river::hex::{HexEdge, SIDE_DIR};

/// The art tag of the building of the wonder (`Buildings.ArtDefineTag` of `BUILDING_GREAT_WALL`).
pub const GREAT_WALL_TAG: &str = "ART_DEF_BUILDING_GREAT_WALL";

/// The model of the wall and its towers: `HB_Great_Wall.gr2`, built by the set `b2-city` with its
/// parts kept.
// re: the strings after `Wall_of_Babylon.gr2` in .rodata: `HB_Great_Wall.gr2`, `WALL1`, `WALL2`,
// `TOWER1`, `TOWER2`, `WALL1_TREECUT` .. (cvGreatWallFilenames, cvGreatWallModelNames)
pub const GREAT_WALL_MODEL: &str = "md:hb_great_wall";

/// The slot of the wall among the models of a city (see [`super::Placed::slot`]).
pub const GREAT_WALL_SLOT: u16 = 3;

/// How far from the city the region of the wall reaches (hexes).
// re: cvGreatWallSim::BuildGreatWall 0x087c83e8 (`[ebp-0x28] = 5`, the radius the flood callback
// 0x087c9a90 compares the hex distance with)
pub const RADIUS: i32 = 5;

/// How many steps from the city the first edge is looked for.
// re: BuildGreatWall 0x087c852b (`lea edx, [eax + 4*eax]`: five times TRANSLATIONS[0])
pub const START_STEPS: i32 = 5;

/// The most steps of the border walk.
// re: BuildGreatWall 0x087c86ab (`cmp esi, 0xc7; ja`)
pub const MAX_STEPS: usize = 200;

/// The scale of the wall and of the towers.
// re: cvDynamicGeometryVisSystem 0x087d1c54, 0x087d1d10 (`0x3dcccccd`)
pub const SCALE: f32 = 0.1;

/// A tower every this many points of the spline.
// re: BuildGreatWall 0x087c8a57 (`and eax, 0xf`)
pub const TOWER_EVERY: usize = 16;

/// The parameter of a tower is its point minus this.
// re: BuildGreatWall 0x087c8a30 (the float at 0x09140430: -0.5)
pub const TOWER_T_OFFSET: f32 = -0.5;

/// A model of `HB_Great_Wall.gr2` that the wall uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Node {
    Wall1,
    Tower1,
    Tower2,
}

impl Node {
    /// The name of the model in the gr2.
    pub fn name(self) -> &'static str {
        match self {
            Node::Wall1 => "WALL1",
            Node::Tower1 => "TOWER1",
            Node::Tower2 => "TOWER2",
        }
    }
}

/// One model of the wall, in the original's world.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WallItem {
    pub node: Node,
    /// The origin of the model (a tower) or the middle of a copy of the wall: x east, y north, z up.
    pub pos: Vec3,
    /// The turn of the model about z, radians: the model's +x goes to `(cos, sin)`. A copy of the
    /// wall is turned so that its length (the model's +y) runs along the spline.
    pub yaw: f32,
    pub scale: f32,
    /// A copy of the wall is stretched along its length by this to fill its share; 1 for a tower.
    pub stretch: f32,
}

/// The owner of every hex, row-major from y = 0; `-1` is none.
#[derive(Debug, Clone, Copy)]
pub struct Territory<'a> {
    pub shape: GridShape,
    pub owners: &'a [i8],
}

impl Territory<'_> {
    /// The owner of the cell `(col, row)` (unwrapped), `None` for none or off the map.
    pub fn owner(&self, col: i32, row: i32) -> Option<i8> {
        let (c, r) = self.shape.normalize(col, row)?;
        let o = *self
            .owners
            .get(r as usize * self.shape.width as usize + c as usize)?;
        (o >= 0).then_some(o)
    }
}

/// What [`place`] reads.
pub struct WallInput<'a> {
    pub territory: Territory<'a>,
    pub frame: &'a TerrainFrame,
    /// The cell of the city.
    pub city: (i32, i32),
    /// The control points of an edge in the texel plane (`QueryHexSegment`).
    pub segment: &'a dyn Fn(HexEdge) -> Vec<Vec2>,
    /// The height of the land at a point `(x, y)` of the original's world (`GetWorldHeight`).
    pub height: &'a dyn Fn(Vec2) -> f32,
    /// The length of `WALL1` along its +y, model units (the y of its box).
    pub wall_length: f32,
    /// The seed of the towers' pick.
    pub seed: u32,
}

/// A hex in cube coordinates `(x, y)` (`x = col - (row >> 1)`, like [`HexEdge`]).
type Cube = (i32, i32);

fn cube_of(col: i32, row: i32) -> Cube {
    (col - (row >> 1), row)
}

fn cell_of(h: Cube) -> (i32, i32) {
    (h.0 + (h.1 >> 1), h.1)
}

fn step(h: Cube, side: usize, n: i32) -> Cube {
    let (dx, dy) = SIDE_DIR[side].hex_delta();
    (h.0 + n * dx, h.1 + n * dy)
}

/// The hex distance.
// re: the flood callback 0x087c9a90 (`min(|dx| + |dy|, |dx| + |dz|, |dy| + |dz|)`, the same)
fn distance(a: Cube, b: Cube) -> i32 {
    let (dx, dy) = (a.0 - b.0, a.1 - b.1);
    let dz = -(dx + dy);
    dx.abs().max(dy.abs()).max(dz.abs())
}

/// The hexes of the wall: those of the city's owner at most [`RADIUS`] from it, joined to it.
/// Empty when the city's hex has no owner.
// re: HexGridFloodFill<DefaultStaticHexGrid<GreatWallHexData,..>,FloodFillCallback> 0x087c90cd and
// its callback 0x087c9a90 (the owner byte, the flag 0x1, the distance)
pub fn region(territory: &Territory<'_>, city: (i32, i32)) -> HashSet<Cube> {
    let mut out = HashSet::new();
    let Some(owner) = territory.owner(city.0, city.1) else {
        return out;
    };
    let c = cube_of(city.0, city.1);
    let mut queue = VecDeque::from([c]);
    out.insert(c);
    while let Some(h) = queue.pop_front() {
        for side in 0..6 {
            let n = step(h, side, 1);
            let (col, row) = cell_of(n);
            if !out.contains(&n)
                && distance(c, n) <= RADIUS
                && territory.owner(col, row) == Some(owner)
            {
                out.insert(n);
                queue.push_back(n);
            }
        }
    }
    out
}

/// The hex on the right of an edge (the inside of the border).
fn right_hex(e: HexEdge) -> Cube {
    if e.code < 6 {
        (e.x, e.y)
    } else {
        let t = e.twin();
        (t.x, t.y)
    }
}

/// The next edge of the border: at the end of `e` three hexes meet, the inside one on the right,
/// the outside one on the left and a third; the border goes on round the third when it is inside
/// (it is then on the right), else round the inside hex.
fn next_border_edge(region: &HashSet<Cube>, e: HexEdge) -> Option<HexEdge> {
    let around_inside = e.next_right();
    let around_outside = e.next_left();
    let t = around_inside.twin();
    let next = if region.contains(&(t.x, t.y)) {
        around_outside
    } else {
        around_inside
    };
    // Both are the border by construction; keep the check against a region that is not one piece.
    let ok = region.contains(&right_hex(next)) && !region.contains(&right_hex(next.reversed()));
    ok.then_some(next)
}

/// The edges of the border of the region, the inside on the right, from the first edge of the
/// original (side 0 of the farthest hex of the region along `TRANSLATIONS[0]`, five steps at most).
// re: BuildGreatWall 0x087c851a..0x087c871a (the start: city + 5 TRANSLATIONS[0], back one step
// while the hex is not flagged 0x8; the walk: 0x087c925a, FUN_0865f1be on the result, the compare
// with the first edge)
pub fn border(region: &HashSet<Cube>, city: (i32, i32)) -> Vec<HexEdge> {
    let c = cube_of(city.0, city.1);
    if !region.contains(&c) {
        return Vec::new();
    }
    let mut n = START_STEPS;
    while n > 0 && !region.contains(&step(c, 0, n)) {
        n -= 1;
    }
    let h = step(c, 0, n);
    let start = HexEdge::new(h.0, h.1, 0);
    let mut out = vec![start];
    let mut e = start;
    for _ in 0..MAX_STEPS {
        let Some(next) = next_border_edge(region, e) else {
            break;
        };
        if next.owned() == start.owned() {
            break;
        }
        out.push(next);
        e = next;
    }
    out
}

/// One step of the corner cutting on points with a height: `m` points become `2m - 2`.
// re: B_SPLINE::ReconstructScalingFunc<FGXVector3,..> 0x087c9333 (0.75 at 0x09149c84, 0.25 at
// 0x09149c6c; the same as `coast::bspline::reconstruct`)
fn reconstruct3(c: &[Vec3]) -> Vec<Vec3> {
    let mut out = Vec::with_capacity(c.len().saturating_sub(1) * 2);
    for w in c.windows(2) {
        out.push(w[0] * 0.75 + w[1] * 0.25);
        out.push(w[0] * 0.25 + w[1] * 0.75);
    }
    out
}

fn at(pts: &[Vec3], i: usize) -> Vec3 {
    pts[i.min(pts.len() - 1)]
}

/// The value of the spline at `t`.
// re: B_SPLINE::EvalScalingFunc<FGXVector3,..> 0x087c9578
fn value3(pts: &[Vec3], t: f32) -> Vec3 {
    let (i, f) = locate(pts.len(), t);
    let w = value_weights(f);
    at(pts, i) * w[0] + at(pts, i + 1) * w[1] + at(pts, i + 2) * w[2]
}

/// The gradient of the spline at `t`.
// re: B_SPLINE::EvalGradient<FGXVector3,..> 0x087c96cb
fn gradient3(pts: &[Vec3], t: f32) -> Vec3 {
    let (i, f) = locate(pts.len(), t);
    let w = gradient_weights(f);
    at(pts, i) * w[0] + at(pts, i + 1) * w[1] + at(pts, i + 2) * w[2]
}

/// The points of the spline of the wall in the original's world: the segments of the border,
/// closed and cut twice.
pub fn spline(input: &WallInput<'_>) -> Vec<Vec3> {
    let region = region(&input.territory, input.city);
    let edges = border(&region, input.city);
    let mut pts: Vec<Vec3> = edges
        .iter()
        .flat_map(|e| (input.segment)(*e))
        .map(|tex| {
            // re: BuildGreatWall 0x087c882e (`x - [g + 0xb4]`, `y * 0.866 - [g + 0xb8]`, then
            // cvTerrainSampler::GetWorldHeight)
            let p = input.frame.orig_point(tex, 0.0);
            let xy = Vec2::new(p.x, p.y);
            xy.extend((input.height)(xy))
        })
        .collect();
    if pts.len() < 2 {
        return Vec::new();
    }
    // re: BuildGreatWall 0x087c897a (the first two points again at the end), 0x087c89a5 (twice)
    pts.push(pts[0]);
    pts.push(pts[1]);
    reconstruct3(&reconstruct3(&pts))
}

/// The cumulative length along the knots of the spline (the middles of two points, in x and y),
/// the parameter of knot `j` being `j`.
// re: cvBaseDynamicGeometryLibrary::AddSplineModel 0x08a166b7 (the table of `(p[i] + p[i+1]) * 0.5`
// distances; the last entry is the approximate length)
fn knot_lengths(pts: &[Vec3]) -> Vec<f32> {
    let knots: Vec<Vec2> = pts
        .windows(2)
        .map(|w| ((w[0] + w[1]) * 0.5).truncate())
        .collect();
    let mut out = Vec::with_capacity(knots.len());
    let mut sum = 0.0;
    out.push(0.0);
    for w in knots.windows(2) {
        sum += w[0].distance(w[1]);
        out.push(sum);
    }
    out
}

/// The parameter at the arc length `s`.
// re: AddSplineModel 0x08a16941..0x08a1698f (the first entry past `s`, the fraction before it)
fn param_at(table: &[f32], s: f32) -> f32 {
    match table.iter().position(|&l| l > s) {
        None => (table.len() - 1) as f32,
        Some(0) => 0.0,
        Some(j) => {
            let (a, b) = (table[j - 1], table[j]);
            let f = if b > a { (s - a) / (b - a) } else { 0.0 };
            (j - 1) as f32 + f.clamp(0.0, 1.0)
        }
    }
}

/// The wall of the city: the copies of `WALL1` and the towers, in the original's world. Empty when
/// the city's hex has no owner.
pub fn place(input: &WallInput<'_>) -> Vec<WallItem> {
    let pts = spline(input);
    if pts.len() < 4 {
        return Vec::new();
    }
    let mut out = Vec::new();
    // The wall: as many copies as the length holds, each over its share.
    // re: AddSplineModel 0x08a16625..0x08a16665 (`len / ((max.y - min.y) * scale)`, at least 1)
    let table = knot_lengths(&pts);
    let length = *table.last().unwrap_or(&0.0);
    let piece = input.wall_length * SCALE;
    if length > 0.0 && piece > 0.0 {
        let copies = ((length / piece) as usize).max(1);
        let share = length / copies as f32;
        for k in 0..copies {
            let t = param_at(&table, (k as f32 + 0.5) * share);
            let g = gradient3(&pts, t);
            out.push(WallItem {
                node: Node::Wall1,
                pos: value3(&pts, t),
                yaw: g.y.atan2(g.x) - std::f32::consts::FRAC_PI_2,
                scale: SCALE,
                stretch: share / piece,
            });
        }
    }
    // The towers.
    // re: BuildGreatWall 0x087c8a53..0x087c8b52 (EvalScalingFunc, EvalGradient, atan2f(g.y, g.x),
    // AddGreatWallTower); cvDynamicGeometryVisSystem 0x087d0c40 (`rand() % 2`), 0x087d1c0a
    // (TOWER2 at 0x508 for 1, TOWER1 at 0x4e0 for 0)
    for i in (TOWER_EVERY..pts.len()).step_by(TOWER_EVERY) {
        let t = i as f32 + TOWER_T_OFFSET;
        let g = gradient3(&pts, t);
        let node = if mix(input.seed, i as u32) % 2 == 1 {
            Node::Tower2
        } else {
            Node::Tower1
        };
        out.push(WallItem {
            node,
            pos: value3(&pts, t),
            yaw: g.y.atan2(g.x),
            scale: SCALE,
            stretch: 1.0,
        });
    }
    out
}

// ---------------------------------------------------------------- the pieces of a city

/// One model of the gr2 as the draw has it: its meshes (the parts of the split model) and its box.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeMeta {
    pub meshes: Vec<u16>,
    /// `InitialPlacement` in our basis.
    pub placement: Mat4,
    /// The box of its meshes in the frame of the placement (our basis: the length of the wall,
    /// the model's +y of the original, is our -z).
    pub lo: Vec3,
    pub hi: Vec3,
}

/// What the wall needs of `HB_Great_Wall.gr2`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GreatWallMeta {
    pub wall: Option<NodeMeta>,
    pub tower1: Option<NodeMeta>,
    pub tower2: Option<NodeMeta>,
}

impl GreatWallMeta {
    /// The models by their names (`WALL1`, `TOWER1`, `TOWER2`); the others are left.
    pub fn of(blob: &ModelBlobV2) -> Self {
        let node = |name: &str| {
            let part = blob
                .parts
                .iter()
                .find(|p| p.name.trim().eq_ignore_ascii_case(name))?;
            let placement = part.placement.to_mat4();
            let first = usize::from(part.first_mesh);
            let mut lo = Vec3::splat(f32::MAX);
            let mut hi = Vec3::splat(f32::MIN);
            let mut meshes = Vec::new();
            for k in first..first + usize::from(part.mesh_count) {
                let Some(pos) = blob.meshes.get(k).and_then(|m| m.positions()) else {
                    continue;
                };
                for p in &pos {
                    let v = placement.transform_point3(Vec3::from(*p));
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
                meshes.push(k as u16);
            }
            (!meshes.is_empty()).then_some(NodeMeta {
                meshes,
                placement,
                lo,
                hi,
            })
        };
        Self {
            wall: node(Node::Wall1.name()),
            tower1: node(Node::Tower1.name()),
            tower2: node(Node::Tower2.name()),
        }
    }

    fn node(&self, n: Node) -> Option<&NodeMeta> {
        match n {
            Node::Wall1 => self.wall.as_ref(),
            Node::Tower1 => self.tower1.as_ref(),
            Node::Tower2 => self.tower2.as_ref(),
        }
    }
}

/// The owners and the frame the wall is placed on (`set_territory` of the city system).
#[derive(Debug, Clone, PartialEq)]
pub struct TerritoryInput {
    pub frame: TerrainFrame,
    pub owners: Vec<i8>,
}

/// The pieces of the wall of a city whose hex is `city`: our world, each a mesh of the split model.
/// `ground` is the height of the land at our `(X, Z)`.
pub fn pieces(
    meta: &GreatWallMeta,
    territory: &TerritoryInput,
    city: (i32, i32),
    ground: &dyn Fn(Vec2) -> f32,
    seed: u32,
) -> Vec<Piece> {
    let frame = &territory.frame;
    let to_orig = frame.ours_to_orig();
    let to_ours = to_orig.inverse();
    let height = |xy: Vec2| {
        let p = to_ours.transform_point3(xy.extend(0.0));
        ground(Vec2::new(p.x, p.z))
    };
    let segment = |e: HexEdge| e.side_points(frame).to_vec();
    let wall_length = meta.wall.as_ref().map_or(0.0, |w| w.hi.z - w.lo.z);
    let items = place(&WallInput {
        territory: Territory {
            shape: frame.grid_shape(),
            owners: &territory.owners,
        },
        frame,
        city,
        segment: &segment,
        height: &height,
        wall_length,
        seed,
    });
    let mut out = Vec::new();
    for item in items {
        let Some(node) = meta.node(item.node) else {
            continue;
        };
        // A copy of the wall stands on the middle of its length; a tower on its origin. The turn
        // about the original's z is the same turn about our y (the basis of the models and of the
        // world is the same rotation).
        let anchor = match item.node {
            Node::Wall1 => Vec3::new(0.0, 0.0, 0.5 * (node.lo.z + node.hi.z)),
            _ => Vec3::ZERO,
        };
        let at = to_ours.transform_point3(item.pos);
        let matrix = Mat4::from_translation(at)
            * Mat4::from_rotation_y(item.yaw)
            * Mat4::from_scale(Vec3::new(item.scale, item.scale, item.scale * item.stretch))
            * Mat4::from_translation(-anchor)
            * node.placement;
        out.extend(node.meshes.iter().map(|&m| Piece {
            part: Some(m),
            matrix,
        }));
    }
    out
}

/// The wall of a city that has the wonder, as one model of the city; `None` without the wonder or
/// the owners. The model may not be here yet: the wall waits for it.
pub fn placed(
    meta: Option<&GreatWallMeta>,
    territory: Option<&TerritoryInput>,
    tags: &[String],
    city: (i32, i32),
    ground: &dyn Fn(Vec2) -> f32,
    seed: u32,
) -> Option<Placed> {
    if !tags.iter().any(|t| t == GREAT_WALL_TAG) {
        return None;
    }
    let territory = territory?;
    let model = LogicalId::new(GREAT_WALL_MODEL);
    Some(match meta {
        None => Placed {
            slot: GREAT_WALL_SLOT,
            model,
            pieces: Vec::new(),
            pending: true,
            animated: false,
        },
        Some(meta) => Placed {
            slot: GREAT_WALL_SLOT,
            model,
            pieces: pieces(meta, territory, city, ground, seed),
            pending: false,
            animated: false,
        },
    })
}

#[cfg(test)]
mod tests {
    use asset_format::{MaterialDesc, MeshArrays, MeshV2, Part, Transform};

    use super::*;
    use crate::hexgrid::{HEX_WIDTH, cell_center};

    const W: i32 = 16;
    const H: i32 = 16;
    const CITY: (i32, i32) = (7, 7);

    fn frame() -> TerrainFrame {
        TerrainFrame::new(W as u32, H as u32, false)
    }

    /// Player 0 owns the hexes at most `r` from the city, player 1 the rest.
    fn owners(r: i32) -> Vec<i8> {
        let c = cube_of(CITY.0, CITY.1);
        (0..H)
            .flat_map(|row| (0..W).map(move |col| (col, row)))
            .map(|(col, row)| {
                if distance(c, cube_of(col, row)) <= r {
                    0
                } else {
                    1
                }
            })
            .collect()
    }

    fn territory(o: &[i8]) -> Territory<'_> {
        Territory {
            shape: frame().grid_shape(),
            owners: o,
        }
    }

    fn input<'a>(
        o: &'a [i8],
        f: &'a TerrainFrame,
        seg: &'a dyn Fn(HexEdge) -> Vec<Vec2>,
        h: &'a dyn Fn(Vec2) -> f32,
    ) -> WallInput<'a> {
        WallInput {
            territory: territory(o),
            frame: f,
            city: CITY,
            segment: seg,
            height: h,
            wall_length: 200.0,
            seed: 11,
        }
    }

    #[test]
    fn great_wall_region_is_the_owners_land_within_five() {
        let o = owners(2);
        let r = region(&territory(&o), CITY);
        assert_eq!(r.len(), 19, "a hex of radius 2");
        // Owning the whole map: the radius stops it.
        let all = vec![0i8; (W * H) as usize];
        assert_eq!(
            region(&territory(&all), CITY).len(),
            91,
            "a hex of radius 5"
        );
        // No owner: nothing.
        let none = vec![-1i8; (W * H) as usize];
        assert!(region(&territory(&none), CITY).is_empty());
    }

    #[test]
    fn great_wall_border_is_a_closed_loop_with_the_land_on_its_right() {
        for r in [0, 1, 2, 5] {
            let o = owners(r);
            let reg = region(&territory(&o), CITY);
            let edges = border(&reg, CITY);
            // The border of a hex of radius r has 6 (2r + 1) edges.
            assert_eq!(edges.len() as i32, 6 * (2 * r + 1), "radius {r}");
            for (i, e) in edges.iter().enumerate() {
                assert!(reg.contains(&right_hex(*e)), "{r} {i}");
                assert!(!reg.contains(&right_hex(e.reversed())), "{r} {i}");
                let next = edges[(i + 1) % edges.len()];
                assert!(
                    e.world_ends().1.distance(next.world_ends().0) < 1e-2,
                    "{r} {i}: the edges join"
                );
            }
            // The first edge is side 0 of the region's farthest hex along TRANSLATIONS[0].
            assert_eq!(edges[0].code, 0);
            let c = cube_of(CITY.0, CITY.1);
            assert_eq!((edges[0].x, edges[0].y), step(c, 0, r.min(START_STEPS)));
        }
    }

    #[test]
    fn great_wall_spline_has_four_n_plus_two_points_on_the_ground() {
        let (o, f) = (owners(1), frame());
        let seg = |e: HexEdge| e.side_points(&frame()).to_vec();
        let h = |_: Vec2| 7.0;
        let pts = spline(&input(&o, &f, &seg, &h));
        // 18 edges of two points: n = 36.
        assert_eq!(pts.len(), 4 * 36 + 2);
        assert!(pts.iter().all(|p| (p.z - 7.0).abs() < 1e-4));
    }

    #[test]
    fn great_wall_towers_stand_every_sixteenth_point_along_the_border() {
        let (o, f) = (owners(1), frame());
        let seg = |e: HexEdge| e.side_points(&frame()).to_vec();
        let h = |_: Vec2| 3.0;
        let items = place(&input(&o, &f, &seg, &h));
        let towers: Vec<&WallItem> = items.iter().filter(|i| i.node != Node::Wall1).collect();
        // 146 points: 16, 32, .., 144.
        assert_eq!(towers.len(), 9);
        assert!(towers.iter().any(|t| t.node == Node::Tower1));
        assert!(towers.iter().any(|t| t.node == Node::Tower2));
        let to_ours = f.ours_to_orig().inverse();
        let centre = cell_center(CITY.0, CITY.1);
        let mut turns = Vec::new();
        for t in &towers {
            assert_eq!((t.scale, t.stretch), (SCALE, 1.0));
            assert!((t.pos.z - 3.0).abs() < 1e-4);
            let at = to_ours.transform_point3(t.pos);
            let radial = Vec2::new(at.x, at.z) - centre;
            let d = radial.length();
            assert!(
                d > 0.9 * HEX_WIDTH && d < 2.1 * HEX_WIDTH,
                "on the border: {d}"
            );
            // The yaw runs along the border (our z is south): never straight out of the city (the
            // border zigzags up to 60 degrees off the round), and always the same way round.
            let along = Vec2::new(t.yaw.cos(), -t.yaw.sin());
            assert!(
                along.dot(radial.normalize()).abs() < 0.9,
                "{along} {radial}"
            );
            turns.push(radial.perp_dot(along) > 0.0);
        }
        assert!(turns.iter().all(|&s| s == turns[0]), "{turns:?}");
    }

    #[test]
    fn great_wall_copies_fill_the_length_of_the_spline() {
        let (o, f) = (owners(2), frame());
        let seg = |e: HexEdge| e.side_points(&frame()).to_vec();
        let h = |_: Vec2| 0.0;
        let inp = input(&o, &f, &seg, &h);
        let pts = spline(&inp);
        let length = *knot_lengths(&pts).last().unwrap();
        let items = place(&inp);
        let walls: Vec<&WallItem> = items.iter().filter(|i| i.node == Node::Wall1).collect();
        let piece = inp.wall_length * SCALE;
        assert_eq!(walls.len(), (length / piece) as usize);
        let filled: f32 = walls.iter().map(|w| w.stretch * piece).sum();
        assert!((filled - length).abs() < 1e-2 * length, "{filled} {length}");
        assert!(walls.iter().all(|w| w.stretch >= 1.0 && w.scale == SCALE));
        // Neighbouring copies are about a share apart.
        let share = length / walls.len() as f32;
        for w in walls.windows(2) {
            let d = w[0].pos.truncate().distance(w[1].pos.truncate());
            assert!(d < 1.2 * share && d > 0.6 * share, "{d} {share}");
        }
    }

    #[test]
    fn great_wall_a_city_without_an_owner_has_no_wall() {
        let o = vec![-1i8; (W * H) as usize];
        let f = frame();
        let seg = |e: HexEdge| e.side_points(&frame()).to_vec();
        let h = |_: Vec2| 0.0;
        assert!(place(&input(&o, &f, &seg, &h)).is_empty());
    }

    fn box_mesh(name: &str, half: Vec3) -> MeshV2 {
        let a = MeshArrays {
            positions: vec![
                [-half.x, 0.0, -half.z],
                [half.x, 0.0, -half.z],
                [half.x, half.y, half.z],
                [-half.x, half.y, half.z],
            ],
            normals: vec![[0.0, 1.0, 0.0]; 4],
            uvs: vec![[0.0, 0.0]; 4],
            joints: vec![],
            weights: vec![],
            indices: vec![0, 1, 2, 0, 2, 3],
        };
        MeshV2::pack(name, 0, 0, &a, false, false).unwrap()
    }

    fn wall_blob() -> ModelBlobV2 {
        let names = ["WALL1", "WALL2", "TOWER1", "TOWER2", "WALL1_TREECUT"];
        ModelBlobV2 {
            meshes: names
                .iter()
                .map(|n| box_mesh(n, Vec3::new(20.0, 60.0, 100.0)))
                .collect(),
            parts: names
                .iter()
                .enumerate()
                .map(|(i, n)| Part {
                    name: (*n).into(),
                    first_mesh: i as u16,
                    mesh_count: 1,
                    placement: Transform::IDENTITY,
                })
                .collect(),
            materials: vec![MaterialDesc {
                package: "BuildingShader".into(),
                ..MaterialDesc::default()
            }],
            ..ModelBlobV2::default()
        }
    }

    #[test]
    fn great_wall_meta_takes_its_models_by_name() {
        let m = GreatWallMeta::of(&wall_blob());
        assert_eq!(m.wall.as_ref().unwrap().meshes, vec![0]);
        assert_eq!(m.tower1.as_ref().unwrap().meshes, vec![2]);
        assert_eq!(m.tower2.as_ref().unwrap().meshes, vec![3]);
        let w = m.wall.unwrap();
        assert!((w.hi.z - w.lo.z - 200.0).abs() < 1e-3);
    }

    #[test]
    fn great_wall_pieces_stand_on_the_ground_along_the_border() {
        let meta = GreatWallMeta::of(&wall_blob());
        let t = TerritoryInput {
            frame: frame(),
            owners: owners(1),
        };
        let ground = |p: Vec2| 5.0 + p.x * 0.001;
        let pieces = pieces(&meta, &t, CITY, &ground, 3);
        assert!(pieces.len() > 9);
        let centre = cell_center(CITY.0, CITY.1);
        for p in &pieces {
            let at = p.matrix.transform_point3(Vec3::ZERO);
            assert!((at.y - ground(Vec2::new(at.x, at.z))).abs() < 0.05, "{at}");
            let d = Vec2::new(at.x, at.z).distance(centre);
            assert!(d > 0.9 * HEX_WIDTH && d < 2.1 * HEX_WIDTH, "{d}");
        }
        // A copy of the wall runs along the border: its length (our z) is across the way out.
        let wall = pieces.iter().find(|p| p.part == Some(0)).unwrap();
        let at = wall.matrix.transform_point3(Vec3::ZERO);
        let len = wall.matrix.transform_vector3(Vec3::Z).normalize();
        let out = (Vec2::new(at.x, at.z) - centre).normalize();
        assert!(Vec2::new(len.x, len.z).dot(out).abs() < 0.9);
    }

    #[test]
    fn great_wall_the_city_system_waits_for_the_model_and_follows_the_owners() {
        use crate::entities::{CityEntity, Owners};
        use crate::world::city::City;
        use crate::world::overlay::OwnersInput;
        use crate::world::unit::B2Context;

        let mut system = City {
            loading: true,
            ..City::default()
        };
        system.upsert(CityEntity {
            id: 1,
            owner: 0,
            x: CITY.0,
            y: CITY.1,
            buildings: vec![GREAT_WALL_TAG.into()],
            ..CityEntity::default()
        });
        let (art, owners_of) = (asset_format::ArtTable::new(), Owners::default());
        let ground = |_: Vec2| 0.0;
        let ctx = B2Context {
            art: &art,
            owners: &owners_of,
            ground: &ground,
            water_level: None,
        };
        let wall = |s: &City| {
            s.placed_at(&(0, 1))
                .iter()
                .find(|p| p.slot == GREAT_WALL_SLOT)
                .cloned()
        };
        // No owners yet: no wall.
        system.refresh_b2(&ctx);
        assert!(wall(&system).is_none());
        let input = |r| OwnersInput::new(W as u32, H as u32, owners(r), vec![[0; 3]; 2]).unwrap();
        system.set_territory(Some(&input(1)), Some(frame()));
        assert_eq!(system.refresh_b2(&ctx), 1);
        assert!(wall(&system).unwrap().pending);
        let id = LogicalId::new(GREAT_WALL_MODEL);
        assert!(system.wants_model(&id));
        system.register_model(id, &wall_blob());
        system.refresh_b2(&ctx);
        let small = wall(&system).unwrap();
        assert!(!small.pending && !small.pieces.is_empty());
        // The same owners again place nothing; wider land a longer wall.
        system.set_territory(Some(&input(1)), Some(frame()));
        assert_eq!(system.refresh_b2(&ctx), 0);
        system.set_territory(Some(&input(2)), Some(frame()));
        assert_eq!(system.refresh_b2(&ctx), 1);
        assert!(wall(&system).unwrap().pieces.len() > small.pieces.len());
    }

    #[test]
    fn great_wall_only_a_city_with_the_wonder_and_owners_has_it() {
        let meta = GreatWallMeta::of(&wall_blob());
        let t = TerritoryInput {
            frame: frame(),
            owners: owners(1),
        };
        let g = |_: Vec2| 0.0;
        let tags = vec![GREAT_WALL_TAG.to_owned()];
        assert!(placed(Some(&meta), Some(&t), &[], CITY, &g, 1).is_none());
        assert!(placed(Some(&meta), None, &tags, CITY, &g, 1).is_none());
        let waiting = placed(None, Some(&t), &tags, CITY, &g, 1).unwrap();
        assert!(waiting.pending && waiting.model.as_str() == GREAT_WALL_MODEL);
        let p = placed(Some(&meta), Some(&t), &tags, CITY, &g, 1).unwrap();
        assert!(!p.pending && p.slot == GREAT_WALL_SLOT && !p.pieces.is_empty());
    }
}
