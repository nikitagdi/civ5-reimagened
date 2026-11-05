//! The decals of the roads (world-objects WO-D7, 4.7, WOB6): a road is not geometry but the piece of
//! its model (`Road_reg`, `RR_reg`, the `_dam` ones when pillaged: `LandmarkShader` meshes of
//! `roads_rails.gr2`) laid along the spline of the road into the layer of decals of the terrain, which
//! bakes it into the page of the cell (`DecalLayer::set_mesh_decals`, source `Roads`), with the strips
//! of the coastal roads.
//!
//! The spline of a plot is the original's (`cvGameplayRoadHandler`): the sides the plot joins are paired
//! (`UpdateRoadTopology`: two sides at a time, the last one with its opposite, or with a joined side),
//! each pair has two control points per side, at a quarter and three quarters of the way to the
//! neighbour's middle, the corners are cut (`TightenRoadCurve`, Chaikin with 1/3) and the curve is the
//! uniform B-spline of the points, so it starts and ends on the middles of the two edges along the line
//! to the neighbour and joins the neighbour's road there. The strip follows the curve, the texture of
//! the piece repeating along it.
//!
//! What the strip is (`re-b13-answers.txt` 1a, 1c, 1d): the pieces `Road_reg` and `RR_reg` of
//! `RoadsandRails_new.gr2` are swept along the curve, `AddSplineDecal` with `fScale` 0.1: the strip is
//! `0.1 * width` of the piece wide (4.74 texels for `Road_reg`), the piece is laid in tiles of `0.1 *
//! length` that are stretched, never squeezed, to fill the curve (`K = max(1, floor(length / tile))`),
//! each with the texture coordinates of the piece as they are (`u` along the road, `v` across), over
//! the albedo and the height of the road material (`RoadsandRails_new_D.dds`, `_H.dds`) with the
//! alpha edge of the albedo; the sweep is made in the texel plane, where a row is 0.866 of ours.
//! Besides, every road writes a 12 texel wide ribbon of its spline (12 samples, 11 quads) into the
//! stencil with bit 0x02, which the trees read: a tree whose point is on the ribbon has scale 0
//! ([`road_cuts`]). A city plot has a ring of the corners of its hex at 0.45 and a spoke to each link.
// re: rw-w7r-answers.txt 1b (CalculateRoadSpline 0x087dd2f2, TightenRoadCurve 0x087de366: k = 1/3,
// n -> 2n - 2; UpdateRoadGeometry 0x087dd3e2: B_SPLINE::EvalScalingFunc / EvalGradient at uniform steps
// of (n - 2) spans, the strip offset along the unit normal, CreateDecal + AddSplineDecal),
// cvGameplayRoadHandler.cpp (UpdateRoadTopology 0x087dd9c8, ms_aaiSegmentSides), cvGameplayRoadHandler.h
// (m_avSideMid[6][2]: two points per side)
//!
//! SIMPLIFIED(SIMP-047): the two points of a side are taken at 1/4 and 3/4 of the line to the
//! neighbour's middle (`m_avSideMid` is filled at run time, not read), the B-spline is the quadratic
//! one (`n - 2` spans), the numbers `UpdateRoadTopology` draws with `rand()` come from the plot, a
//! lone road plot draws nothing, and the plain `Main_Road` / `Detail_Road` strips of the original (types
//! without a texture in `decal_filenames.xml`) are not made. The coastal roads
//! (`products().coastal_roads`) are drawn only where the cell they pass through has a route.
//!
//! SIMPLIFIED(SIMP-exe-1160): `g_HexToTex` is not read: a spline unit is a texel, which is one world unit
//! along X, and a row of texels is 0.866 world units (the frame of the terrain), so the width and the
//! tile length of the sweep and the ribbon of the stencil are measured in the texel plane and a road
//! across the rows is 0.866 as wide as one along them.
//! SIMPLIFIED(SIMP-exe-1163): the pieces of the other route categories of the original (`PL`, `HB`,
//! the trade road, picked by the route state and by `rand()` for each tile) are not laid: a road is
//! `Road_reg`, a railroad `RR_reg`; the sampler of the bake (`ClampToEdge`, linear, mip linear) and the
//! texel of the page are the ones of the other decals, the original's are not read.

use std::collections::{BTreeMap, BTreeSet};

use glam::Vec2;

use crate::entities::roads::{RouteKind, dir_vector};
use crate::hexgrid::{Dir, HEX_WIDTH, cell_center, nearest_cell};
use crate::world::landmark::routes::{DrawnLink, Link, RoadSkin, RoadSkins};
use crate::world::landmark::select::mix;
use crate::world::terrain::decals::{DecalLayer, DecalSource, DecalVertex, MeshDecal};

/// What the sweep of `AddSplineDecal` multiplies the units of the model by, across and along: the strip
/// of a piece is `SWEEP_SCALE * width` wide and its tile `SWEEP_SCALE * length` long.
// re: re-b13-answers.txt 1c section 3 (fScale 0.1f 0x3dcccccd; Road_reg 47.37 wide -> 4.74, 192.33 long
// -> 19.23)
pub const SWEEP_SCALE: f32 = 0.1;

/// Half the width of the ribbon a road writes into the stencil, texels.
// re: re-b13-answers.txt 1a section 2.5 (L = pos + 6.0 * n, DAT_0914aaf0)
pub const STENCIL_HALF_WIDTH: f32 = 6.0;

/// Samples of the spline of the ribbon: twice its control points (6 for a road between two sides).
// re: re-b13-answers.txt 1a section 2.5 (M = 2N samples, 11 quads)
pub const RIBBON_SAMPLES: usize = 12;

/// The radius of the ring of a city, as a part of its hex.
// re: re-b13-answers.txt 1a section 2.4 (kCityRingRadius 0.45)
pub const CITY_RING_RADIUS: f32 = 0.45;

/// How the roads of a city plot are drawn (the bits 6..7 of the cell of the original).
// re: re-b13-answers.txt 1a section 1 (city mode: 0 open, 1 ring + spokes, 2 spokes only)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CityMode {
    /// A ring round the middle and a spoke from it to each link.
    RingAndSpokes,
    /// A spoke from the middle to each link.
    SpokesOnly,
}

/// SIMPLIFIED(SIMP-exe-1162): what sets the mode of a city (the bits 6..7 of the cell) is not read, the
/// answer leaves the initial value of the corner flag open: every city plot has the ring and the spokes,
/// and the ring and the spoke ends are a quadratic B-spline of the corners, not the arc-length one.
const CITY_MODE: CityMode = CityMode::RingAndSpokes;

/// The priority of the road decals: `<Main_Road_01 priority="0.7"/>` (`decal_filenames.xml`), over
/// the fields (0.5) and the floodplains (0.4).
pub const ROAD_PRIORITY: f32 = 0.7;

/// Samples of the curve per span of the B-spline.
const STEPS_PER_SPAN: usize = 4;

/// A straight piece (a spoke, a coastal road) has a pair of points every so far, so that the strip
/// follows the relief.
const STEP: f32 = HEX_WIDTH * 0.125;

/// `TightenRoadCurve`: every segment `p q` gives `(2p + q) / 3` and `(p + 2q) / 3`, so `n` points
/// become `2n - 2`.
// re: TightenRoadCurve 0x087de366 (k = 0.33333334 @0x0920c424)
pub fn tighten(points: &[Vec2]) -> Vec<Vec2> {
    const K: f32 = 0.333_333_34;
    points
        .windows(2)
        .flat_map(|w| [(w[0] * 2.0 + w[1]) * K, (w[0] + w[1] * 2.0) * K])
        .collect()
}

/// The control points of the road of the plot centred on `c` from the side `a` to the side `b`:
/// two per side, at three quarters and a quarter of the line to the neighbour's middle.
pub fn control_points(c: Vec2, a: Dir, b: Dir) -> [Vec2; 4] {
    let (da, db) = (dir_vector(a) * HEX_WIDTH, dir_vector(b) * HEX_WIDTH);
    [c + da * 0.75, c + da * 0.25, c + db * 0.25, c + db * 0.75]
}

/// The uniform quadratic B-spline of `points`, `STEPS_PER_SPAN` samples per span: it starts half
/// way between the first two points and ends half way between the last two.
pub fn bspline(points: &[Vec2]) -> Vec<Vec2> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let spans = points.len() - 2;
    let mut out = Vec::with_capacity(spans * STEPS_PER_SPAN + 1);
    for j in 0..spans {
        let (p0, p1, p2) = (points[j], points[j + 1], points[j + 2]);
        let last = if j + 1 == spans {
            STEPS_PER_SPAN
        } else {
            STEPS_PER_SPAN - 1
        };
        for k in 0..=last {
            let t = k as f32 / STEPS_PER_SPAN as f32;
            let w0 = 0.5 * (1.0 - t) * (1.0 - t);
            let w2 = 0.5 * t * t;
            out.push(p0 * w0 + p1 * (1.0 - w0 - w2) + p2 * w2);
        }
    }
    out
}

/// The curve of a road through the plot centred on `c` from the side `a` to the side `b`.
pub fn road_curve(c: Vec2, a: Dir, b: Dir) -> Vec<Vec2> {
    bspline(&tighten(&control_points(c, a, b)))
}

/// A straight line from `from` to `to`, a point every `STEP` or less.
fn line(from: Vec2, to: Vec2) -> Vec<Vec2> {
    let n = ((from.distance(to) / STEP).ceil() as usize).max(1);
    (0..=n)
        .map(|i| from.lerp(to, i as f32 / n as f32))
        .collect()
}

/// `UpdateRoadTopology`: the pairs of sides the roads of a plot in open country join. While two or
/// more sides are left, two of them (picked by `roll`) are joined; the last one is joined with its
/// opposite when the plot has a single side, else with one of the sides (picked by `roll`, not
/// itself). At most five pairs.
// re: cvGameplayRoadHandler.cpp:118-175 (UpdateRoadTopology 0x087dd9c8)
pub fn pair_sides(sides: &[Dir], roll: &mut dyn FnMut() -> u32) -> Vec<(Dir, Dir)> {
    let mut connected: Vec<Dir> = sides.to_vec();
    connected.sort_by_key(|d| d.index());
    connected.dedup();
    let mut left = connected.clone();
    let mut out = Vec::new();
    while !left.is_empty() && out.len() < 5 {
        if left.len() >= 2 {
            let n = left.len() as u32;
            let mut first = roll() % n;
            let second = roll() % n;
            if first == second {
                first = (first + 1) % n;
            }
            let (a, b) = (left[first as usize], left[second as usize]);
            out.push((a, b));
            left.retain(|d| *d != a && *d != b);
        } else {
            let side = left[0];
            let partner = if connected.len() == 1 {
                side.opposite()
            } else {
                let n = connected.len();
                let pick = roll() as usize % n;
                let p = connected[pick];
                if p == side {
                    connected[(pick + 1) % n]
                } else {
                    p
                }
            };
            out.push((side, partner));
            left.clear();
        }
    }
    out
}

/// The rows of the road layout are scaled by this in the texel plane.
// re: TerrainSystem.cpp:53 (kHexHeightRatio), as `water::coast::roads::HEX_HEIGHT_RATIO`
const LAYOUT_ROW_RATIO: f32 = 0.866;

/// A vector of our world in the texel plane (a row of texels is `LAYOUT_ROW_RATIO` of ours).
fn to_tex(v: Vec2) -> Vec2 {
    Vec2::new(v.x, v.y / LAYOUT_ROW_RATIO)
}

/// A vector of the texel plane in our world.
fn from_tex(v: Vec2) -> Vec2 {
    Vec2::new(v.x, v.y * LAYOUT_ROW_RATIO)
}

/// The offset of `half` texels to the side of the way `dir` (our world) runs, in our world: the
/// normal is taken in the texel plane.
// re: re-b13-answers.txt 1a section 2.5 (n = (tan.y, -tan.x) / |tan| of the spline, texel plane)
fn side_offset(dir: Vec2, half: f32) -> Vec2 {
    let t = to_tex(dir).normalize_or_zero();
    from_tex(Vec2::new(-t.y, t.x) * half)
}

/// The length of a way in texels.
fn tex_length(points: &[Vec2]) -> f32 {
    points
        .windows(2)
        .map(|w| to_tex(w[1] - w[0]).length())
        .sum()
}

/// The length of the tile of a piece on the way, texels: the piece is `SWEEP_SCALE * length` long, a
/// whole number of tiles fills the way (at least one: a short way stretches a tile, a long one never
/// squeezes it).
// re: re-b13-answers.txt 1c section 3.1 (K = max(1, (int)(total / tileLenScaled)), tile = total / K)
fn tile_length(skin: &RoadSkin, total: f32) -> f32 {
    let tile = SWEEP_SCALE * skin.length;
    let k = (total / tile).floor().max(1.0);
    total / k
}

/// The length of road one piece covers, texels, when the way is as long as the piece.
#[cfg(test)]
fn repeat_of(skin: &RoadSkin) -> f32 {
    SWEEP_SCALE * skin.length
}

/// The texture coordinate of a point of the strip, `f` of the way along its piece (0 to 1), on the
/// left (`false`) or the right edge. A piece covers its own rectangle of the atlas only.
fn skin_uv(skin: &RoadSkin, f: f32, right: bool) -> Vec2 {
    let t = skin.along[0] + (skin.along[1] - skin.along[0]) * f;
    let a = skin.across[usize::from(right)];
    if skin.along_is_u {
        Vec2::new(t, a)
    } else {
        Vec2::new(a, t)
    }
}

/// The strip of the piece `skin` along `points`, its texture starting `along0` texels in: the piece
/// repeated end to end (`AddSplineDecal`, the tile stretched to fill the way), a strip of quads cut
/// where one piece ends and the next starts, so each quad reads its piece's rectangle of the atlas.
/// The model's +X edge is on the left of the way the points run.
fn strip_decal(points: &[Vec2], along0: f32, skin: &RoadSkin, cell: u32) -> Option<MeshDecal> {
    if points.len() < 2 {
        return None;
    }
    let total = tex_length(points);
    if !total.is_finite() || total <= 0.0 || skin.length <= 0.0 || skin.width <= 0.0 {
        return None;
    }
    let half = SWEEP_SCALE * skin.width * 0.5;
    let repeat = tile_length(skin, total);
    // The side offset at each point: the normal of the segments round it.
    let sides: Vec<Vec2> = (0..points.len())
        .map(|i| {
            let a = points[i.saturating_sub(1)];
            let b = points[(i + 1).min(points.len() - 1)];
            side_offset(b - a, half)
        })
        .collect();
    let mut verts: Vec<DecalVertex> = Vec::with_capacity(points.len() * 4);
    let mut tris: Vec<[u16; 3]> = Vec::with_capacity(points.len() * 2);
    let mut quad = |p0: Vec2, s0: Vec2, f0: f32, p1: Vec2, s1: Vec2, f1: f32| {
        let n = verts.len() as u16;
        verts.extend([
            DecalVertex {
                pos: p0 - s0,
                uv: skin_uv(skin, f0, false),
            },
            DecalVertex {
                pos: p0 + s0,
                uv: skin_uv(skin, f0, true),
            },
            DecalVertex {
                pos: p1 - s1,
                uv: skin_uv(skin, f1, false),
            },
            DecalVertex {
                pos: p1 + s1,
                uv: skin_uv(skin, f1, true),
            },
        ]);
        tris.push([n, n + 1, n + 2]);
        tris.push([n + 1, n + 3, n + 2]);
    };
    let mut along = along0;
    let mut quads = 0usize;
    for i in 0..points.len() - 1 {
        let (a, b) = (points[i], points[i + 1]);
        let (sa, sb) = (sides[i], sides[i + 1]);
        let len = to_tex(b - a).length();
        let end = along + len;
        // Cut the segment where a piece ends.
        let mut s = along;
        while s < end - 1e-4 {
            // A strip past what a u16 index holds is dropped below; stop growing it here (a run that
            // never ends would grow the vertex list until the allocation overflows).
            if quads * 4 > usize::from(u16::MAX) {
                return None;
            }
            let mut piece = (s / repeat).floor();
            let mut next = ((piece + 1.0) * repeat).min(end);
            // On a piece boundary `s / repeat` can round down to the piece before (`s = k * repeat`,
            // `floor(s / repeat) = k - 1`): its end is `s` itself and the cut would never move on.
            if next <= s {
                piece += 1.0;
                next = ((piece + 1.0) * repeat).min(end);
            }
            if next <= s {
                // No float step left at this distance (or not finite): end the segment here.
                break;
            }
            let (t0, t1) = ((s - along) / len, (next - along) / len);
            let f0 = s / repeat - piece;
            let f1 = next / repeat - piece;
            quad(
                a.lerp(b, t0),
                sa.lerp(sb, t0),
                f0,
                a.lerp(b, t1),
                sa.lerp(sb, t1),
                f1,
            );
            quads += 1;
            s = next;
        }
        along = end;
    }
    if verts.len() > usize::from(u16::MAX) {
        return None;
    }
    // The road is not a mesh decal of the container but the `Main_Road_01` type with its three textures
    // overwritten by the road material: the height is `RoadsandRails_new_H.dds`, the step of the road.
    // re: re-b13-answers.txt 1d section 1.3 (UpdateRoadGeometry 0x087dd786..0x087dd7a8: priority kept,
    // albedo / heightmap / spec taken from g_DynamicGeometryLibrary +0xd24 / +0xd28 / +0xd2c)
    Some(MeshDecal {
        albedo: skin.albedo.clone(),
        height: skin.height.clone(),
        priority: ROAD_PRIORITY,
        verts,
        tris,
        cell,
    })
}

fn skin_of<'a>(skins: &'a RoadSkins, link: &Link) -> Option<&'a RoadSkin> {
    skins
        .get(&(link.kind, link.pillaged))
        .or_else(|| skins.get(&(link.kind, false)))
}

/// The decals of a curve whose first half is the road of `la` and whose second half the road of
/// `lb`: one decal, or two when the two halves are textured otherwise.
fn curve_decals(
    points: &[Vec2],
    la: &Link,
    lb: &Link,
    skins: &RoadSkins,
    cell: u32,
    out: &mut Vec<MeshDecal>,
) {
    let (Some(sa), Some(sb)) = (skin_of(skins, la), skin_of(skins, lb)) else {
        return;
    };
    if sa == sb {
        out.extend(strip_decal(points, 0.0, sa, cell));
        return;
    }
    let mid = points.len() / 2;
    out.extend(strip_decal(&points[..=mid], 0.0, sa, cell));
    out.extend(strip_decal(&points[mid..], 0.0, sb, cell));
}

/// The corners of a hex in the plane of the hex (`kHexCorners`: axial, two thirds and a third of the
/// way), north east first, clockwise.
// re: re-b13-answers.txt 1a section 2 (kHexCorners[6][2])
const HEX_CORNERS: [(f32, f32); 6] = [
    (2.0 / 3.0, -1.0 / 3.0),
    (1.0 / 3.0, 1.0 / 3.0),
    (-1.0 / 3.0, 2.0 / 3.0),
    (-2.0 / 3.0, 1.0 / 3.0),
    (-1.0 / 3.0, -1.0 / 3.0),
    (1.0 / 3.0, -2.0 / 3.0),
];

/// The corner `k` of a hex in our world, from its middle.
fn hex_corner(k: usize) -> Vec2 {
    let (q, r) = HEX_CORNERS[k];
    Vec2::new(q + r * 0.5, r * 0.866) * HEX_WIDTH
}

/// The curve of the ring of a city plot centred on `c`: the six corners at `CITY_RING_RADIUS` and
/// the first two again, so that the curve of those points closes.
// re: re-b13-answers.txt 1a section 2.4 (CalculateRoadSplineCity: 6 corners scaled by 0.45, the first
// two points appended again)
pub fn ring_curve(c: Vec2) -> Vec<Vec2> {
    let mut points: Vec<Vec2> = (0..6)
        .map(|k| c + hex_corner(k) * CITY_RING_RADIUS)
        .collect();
    points.extend([points[0], points[1]]);
    bspline(&points)
}

/// The curve of the spoke of a city plot centred on `c` to the side `dir`: from the ring (half its
/// radius from the middle, on the way to the neighbour) to the middle of the edge; without the ring
/// from the middle.
// re: re-b13-answers.txt 1a section 2.4 (the end point at 0.45 * 0.5 * TRANSLATIONS[side])
pub fn spoke_curve(c: Vec2, dir: Dir, mode: CityMode) -> Vec<Vec2> {
    let inner = match mode {
        CityMode::RingAndSpokes => CITY_RING_RADIUS * 0.5 * HEX_WIDTH,
        CityMode::SpokesOnly => 0.0,
    };
    line(
        c + dir_vector(dir) * inner,
        c + dir_vector(dir) * (HEX_WIDTH * 0.5),
    )
}

/// One road of a plot: the curve its strip and its ribbon follow, and the links whose kinds it takes
/// (the first half from `a`, the second from `b`).
struct Route {
    cell: u32,
    curve: Vec<Vec2>,
    a: Link,
    b: Link,
}

/// The roads of the links of every plot: the pairs of sides of a plot in open country, the ring and
/// the spokes of a city.
fn routes(
    links: &[DrawnLink],
    width: i32,
    is_city: &dyn Fn(i32, i32) -> bool,
    mode: CityMode,
) -> Vec<Route> {
    let mut by_plot: BTreeMap<(i32, i32), Vec<Link>> = BTreeMap::new();
    for &(plot, link) in links {
        by_plot.entry(plot).or_default().push(link);
    }
    let mut out = Vec::new();
    for ((x, y), mut mine) in by_plot {
        mine.sort_by_key(|l| l.dir.index());
        let cell = (y * width + x) as u32;
        let c = cell_center(x, y);
        if is_city(x, y) {
            if mode == CityMode::RingAndSpokes {
                out.push(Route {
                    cell,
                    curve: ring_curve(c),
                    a: mine[0],
                    b: mine[0],
                });
            }
            for l in &mine {
                out.push(Route {
                    cell,
                    curve: spoke_curve(c, l.dir, mode),
                    a: *l,
                    b: *l,
                });
            }
            continue;
        }
        let link_of = |d: Dir| mine.iter().find(|l| l.dir == d).copied();
        let sides: Vec<Dir> = mine.iter().map(|l| l.dir).collect();
        let mut k = 0u32;
        let mut roll = || {
            k += 1;
            mix(cell, 0x5d0a_d000 + k)
        };
        for (a, b) in pair_sides(&sides, &mut roll) {
            let Some(la) = link_of(a) else {
                continue;
            };
            // The opposite side of a plot with one link is not a link: it takes the link's road.
            let lb = link_of(b).unwrap_or(la);
            out.push(Route {
                cell,
                curve: road_curve(c, a, b),
                a: la,
                b: lb,
            });
        }
    }
    out
}

/// The decals of the links, `width` the width of the map in plots; `is_city` tells the plots whose
/// roads are a ring and spokes.
pub fn road_decals(
    links: &[DrawnLink],
    width: i32,
    skins: &RoadSkins,
    is_city: &dyn Fn(i32, i32) -> bool,
) -> Vec<MeshDecal> {
    city_road_decals(links, width, skins, is_city, CITY_MODE)
}

fn city_road_decals(
    links: &[DrawnLink],
    width: i32,
    skins: &RoadSkins,
    is_city: &dyn Fn(i32, i32) -> bool,
    mode: CityMode,
) -> Vec<MeshDecal> {
    let mut out = Vec::new();
    for r in routes(links, width, is_city, mode) {
        curve_decals(&r.curve, &r.a, &r.b, skins, r.cell, &mut out);
    }
    out
}

/// The ribbon of a curve in the stencil: `RIBBON_SAMPLES` points of it at equal steps of its
/// parameter, each with a point `STENCIL_HALF_WIDTH` texels to either side of it, and two triangles
/// for each of the quads between them (`s + 1, s, s + 2` and `s + 3, s + 1, s + 2`).
// re: re-b13-answers.txt 1a section 2.5 (UpdateRoadGeometry: L_k = pos + 6 n, R_k = pos - 6 n, vertex 2k
// = L_k, 2k + 1 = R_k, 11 quads, 22 triangles; the stencil bit 0x02)
pub fn ribbon(curve: &[Vec2]) -> Vec<[Vec2; 3]> {
    if curve.len() < 2 {
        return Vec::new();
    }
    let last = (curve.len() - 1) as f32;
    let mut sides: Vec<[Vec2; 2]> = Vec::with_capacity(RIBBON_SAMPLES);
    for k in 0..RIBBON_SAMPLES {
        let f = last * k as f32 / (RIBBON_SAMPLES - 1) as f32;
        let i = (f.floor() as usize).min(curve.len() - 2);
        let p = curve[i].lerp(curve[i + 1], f - i as f32);
        let side = side_offset(curve[i + 1] - curve[i], STENCIL_HALF_WIDTH);
        sides.push([p + side, p - side]);
    }
    let mut out = Vec::with_capacity((RIBBON_SAMPLES - 1) * 2);
    for w in sides.windows(2) {
        let (l0, r0, l1, r1) = (w[0][0], w[0][1], w[1][0], w[1][1]);
        for t in [[r0, l0, l1], [r1, r0, l1]] {
            // A triangle with no area covers nothing.
            if (t[1] - t[0]).perp_dot(t[2] - t[0]).abs() > 1e-6 {
                out.push(t);
            }
        }
    }
    out
}

/// The ground the roads clear of trees, by the cell (plot) that has them: the ribbon of every road,
/// as triangles of our world.
// SIMPLIFIED(SIMP-exe-1161): the ribbon goes to the cuts the trees test on the CPU (as the `Tree Cut` meshes
// of the landmarks, SIMP-065), not into the bit 0x02 of the stencil in cells of 2 x 2 texels, which the
// trees read with the mask 7; the test is the same point of the tree, the edge of the ribbon is exact
// instead of in cells of two texels, and the coastal roads clear nothing.
pub fn road_cuts(
    links: &[DrawnLink],
    width: i32,
    is_city: &dyn Fn(i32, i32) -> bool,
) -> BTreeMap<u32, Vec<[Vec2; 3]>> {
    let mut out: BTreeMap<u32, Vec<[Vec2; 3]>> = BTreeMap::new();
    for r in routes(links, width, is_city, CITY_MODE) {
        out.entry(r.cell).or_default().extend(ribbon(&r.curve));
    }
    out
}

/// The strips of the coastal roads: each polyline (in our world) is cut where the cell it passes
/// has no route, and every run of at least two points that is left is a strip of the cell of its
/// first point, textured as a plain road.
pub fn coastal_decals(
    roads: &[Vec<Vec2>],
    width: i32,
    skins: &RoadSkins,
    has_route: &dyn Fn(i32, i32) -> bool,
) -> Vec<MeshDecal> {
    let Some(skin) = skins.get(&(RouteKind::Road, false)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for road in roads {
        let mut run: Vec<Vec2> = Vec::new();
        let mut flush = |run: &mut Vec<Vec2>| {
            if run.len() >= 2 {
                let (x, y) = nearest_cell(run[0]);
                if x >= 0 && y >= 0 && x < width {
                    out.extend(strip_decal(run, 0.0, skin, (y * width + x) as u32));
                }
            }
            run.clear();
        };
        for &p in road {
            let (x, y) = nearest_cell(p);
            if has_route(x, y) {
                run.push(p);
            } else {
                flush(&mut run);
            }
        }
        flush(&mut run);
    }
    out
}

/// The point of our world under a point of the road layout plane (`water::coast::roads::to_layout`
/// the other way): the layout plane is the texel plane minus half the world, rows scaled.
pub fn from_layout(p: Vec2, frame: &crate::world::terrain::sampler::TerrainFrame) -> Vec2 {
    let half = frame.half_world();
    frame.tex_to_world(Vec2::new(p.x + half.x, (p.y + half.y) / LAYOUT_ROW_RATIO))
}

/// The roads in the layer: replaces the decals of the plots in `touched` (every plot that has a
/// link now or had one) with the ones for `links` and the coastal strips. Returns how many decals
/// were made.
pub fn apply(
    layer: &mut DecalLayer,
    links: &[DrawnLink],
    coastal: &[MeshDecal],
    touched: &BTreeSet<(i32, i32)>,
    width: i32,
    skins: &RoadSkins,
    is_city: &dyn Fn(i32, i32) -> bool,
) -> usize {
    let mut decals = road_decals(links, width, skins, is_city);
    decals.extend(coastal.iter().cloned());
    let mut cells: BTreeSet<u32> = touched
        .iter()
        .map(|&(x, y)| (y * width + x) as u32)
        .collect();
    cells.extend(decals.iter().map(|d| d.cell));
    let cells: Vec<u32> = cells.into_iter().collect();
    layer
        .set_mesh_decals(DecalSource::Roads, &cells, decals)
        .len()
}

#[cfg(test)]
mod tests {
    use asset_format::LogicalId;

    use super::*;

    fn link(dir: Dir, kind: RouteKind) -> Link {
        Link {
            dir,
            kind,
            pillaged: false,
            river: false,
            bridge: false,
            iron: false,
        }
    }

    fn skin(name: &str) -> RoadSkin {
        RoadSkin {
            albedo: LogicalId::new(format!("tx:{name}.dds")),
            height: Some(LogicalId::new(format!("tx:{name}_h.dds"))),
            across: [0.93, 0.99],
            along: [1.0, 0.0],
            width: 25.7,
            length: 128.0,
            along_is_u: true,
        }
    }

    fn skins() -> RoadSkins {
        RoadSkins::from([
            ((RouteKind::Road, false), skin("road")),
            ((RouteKind::Road, true), skin("road_dam")),
            ((RouteKind::Railroad, false), skin("rail")),
        ])
    }

    fn near(a: Vec2, b: Vec2) -> bool {
        a.distance(b) < 1e-3
    }

    /// The Late Huge save (B2 huge-black): a coastal strip whose texture distance landed on a piece
    /// boundary that `s / repeat` rounds down. The cut never moved on, the vertex list grew until
    /// the allocation overflowed ("capacity overflow" in `strip_decal`, the frame panicked).
    #[test]
    fn a_strip_starting_on_a_rounded_down_piece_boundary_ends() {
        let sk = skin("road");
        // The way is one tile and a half: the tile is stretched to the whole way.
        let way = [Vec2::ZERO, Vec2::new(repeat_of(&sk) * 1.5, 0.0)];
        let repeat = tile_length(&sk, tex_length(&way));
        let s = (1..10_000_000u32)
            .map(|k| k as f32 * repeat)
            .find(|&s| ((s / repeat).floor() + 1.0) * repeat <= s)
            .expect("a boundary that rounds down");
        let d = strip_decal(&way, s, &sk, 0).expect("a strip");
        assert!(d.verts.len() <= 3 * 4, "{} vertices", d.verts.len());
        assert!(
            d.tris
                .iter()
                .flatten()
                .all(|&i| usize::from(i) < d.verts.len())
        );
    }

    #[test]
    fn a_strip_too_long_or_too_far_is_dropped_not_grown_forever() {
        let sk = skin("road");
        // Far more pieces than a u16 index holds (a run across the whole world).
        let long = [Vec2::ZERO, Vec2::new(repeat_of(&sk) * 100_000.0, 0.0)];
        assert!(strip_decal(&long, 0.0, &sk, 0).is_none());
        // Distances where a float has no step left, and no finite distance at all.
        let short = [Vec2::ZERO, Vec2::new(1.0, 0.0)];
        let _ = strip_decal(&short, 1e30, &sk, 0);
        let _ = strip_decal(&short, f32::INFINITY, &sk, 0);
        let _ = strip_decal(&[Vec2::ZERO, Vec2::new(f32::INFINITY, 0.0)], 0.0, &sk, 0);
    }

    #[test]
    fn the_corners_are_cut_by_a_third() {
        let p = [Vec2::ZERO, Vec2::new(3.0, 0.0), Vec2::new(3.0, 3.0)];
        let t = tighten(&p);
        assert_eq!(t.len(), 4);
        assert!(near(t[0], Vec2::new(1.0, 0.0)) && near(t[1], Vec2::new(2.0, 0.0)));
        assert!(near(t[2], Vec2::new(3.0, 1.0)) && near(t[3], Vec2::new(3.0, 2.0)));
    }

    #[test]
    fn a_road_runs_from_the_middle_of_one_edge_to_the_middle_of_the_other() {
        let c = cell_center(3, 2);
        let mid = |d: Dir| c + dir_vector(d) * (HEX_WIDTH * 0.5);
        for (a, b) in [(Dir::West, Dir::East), (Dir::NorthEast, Dir::East)] {
            let curve = road_curve(c, a, b);
            assert_eq!(curve.len(), 4 * STEPS_PER_SPAN + 1);
            assert!(near(curve[0], mid(a)), "{:?}", curve[0]);
            assert!(near(*curve.last().unwrap(), mid(b)));
            // It leaves the edge along the line to the neighbour (the neighbour's road comes in so).
            let d = (curve[1] - curve[0]).normalize();
            assert!(d.dot(-dir_vector(a)) > 0.99, "{d:?}");
        }
        // Across the plot a road is straight and passes the middle.
        let straight = road_curve(c, Dir::West, Dir::East);
        assert!(straight.iter().all(|p| (p.y - c.y).abs() < 1e-3));
        assert!(straight.iter().any(|p| near(*p, c)));
        // A bend between two sides next to each other stays well inside the plot.
        let bend = road_curve(c, Dir::NorthEast, Dir::East);
        assert!(bend.iter().all(|p| p.distance(c) <= HEX_WIDTH * 0.5 + 1e-3));
    }

    #[test]
    fn the_sides_are_paired_as_the_original_does() {
        let mut r = 0u32;
        let mut roll = || {
            r += 1;
            r * 7
        };
        // One side: through to the opposite one.
        assert_eq!(
            pair_sides(&[Dir::East], &mut roll),
            [(Dir::East, Dir::West)]
        );
        // Two sides: joined.
        let two = pair_sides(&[Dir::East, Dir::NorthWest], &mut roll);
        assert_eq!(two.len(), 1);
        let (a, b) = two[0];
        assert!(a != b && [a, b].contains(&Dir::East) && [a, b].contains(&Dir::NorthWest));
        // Three sides: a pair and the last one with another side; every side has a road.
        let three = pair_sides(&[Dir::East, Dir::West, Dir::SouthEast], &mut roll);
        assert_eq!(three.len(), 2);
        for d in [Dir::East, Dir::West, Dir::SouthEast] {
            assert!(three.iter().any(|(a, b)| *a == d || *b == d));
        }
        assert!(three.iter().all(|(a, b)| a != b));
    }

    #[test]
    fn two_plots_meet_on_their_edge_with_the_texture_of_the_piece() {
        let links = [
            ((3, 2), link(Dir::East, RouteKind::Road)),
            ((4, 2), link(Dir::West, RouteKind::Road)),
        ];
        let d = road_decals(&links, 10, &skins(), &|_, _| false);
        assert_eq!(d.len(), 2);
        assert_eq!((d[0].cell, d[1].cell), (23, 24));
        assert_eq!(d[0].albedo.as_str(), "tx:road.dds");
        assert_eq!(d[0].priority, ROAD_PRIORITY);
        // The edge between the two plots is a point of both strips.
        let edge = (cell_center(3, 2) + cell_center(4, 2)) * 0.5;
        let middle = |m: &MeshDecal, i: usize| (m.verts[i].pos + m.verts[i + 1].pos) * 0.5;
        let ends = |m: &MeshDecal| [middle(m, 0), middle(m, m.verts.len() - 2)];
        assert!(ends(&d[0]).iter().any(|p| near(*p, edge)));
        assert!(ends(&d[1]).iter().any(|p| near(*p, edge)));
        // The strip is a tenth of the piece wide (in the texel plane: a row is 0.866 of ours), the
        // piece's rows across, its u inside the piece's range; the road has the height of its material.
        let m = &d[0];
        let wide = SWEEP_SCALE * 25.7 * LAYOUT_ROW_RATIO;
        assert!((m.verts[0].pos.distance(m.verts[1].pos) - wide).abs() < 1e-3);
        assert_eq!((m.verts[0].uv.y, m.verts[1].uv.y), (0.93, 0.99));
        assert!(m.verts.iter().all(|v| (-1e-4..=1.0001).contains(&v.uv.x)));
        assert_eq!(m.height.as_ref().unwrap().as_str(), "tx:road_h.dds");
        // The road across the plot is as many tiles as fit it, stretched to fill it.
        let repeat = tile_length(&skin("road"), HEX_WIDTH);
        assert!((repeat - HEX_WIDTH / 5.0).abs() < 1e-3);
        let quads = m.verts.len() / 4;
        assert!(quads >= 5);
        assert_eq!(m.tris.len(), quads * 2);
        let starts = m.verts.chunks(4).filter(|q| q[0].uv.x == 1.0).count();
        assert!(starts >= 2, "a new piece starts at u 1: {starts}");
    }

    #[test]
    fn a_road_between_two_kinds_is_two_decals_and_a_city_has_spokes() {
        let links = [
            ((3, 2), link(Dir::West, RouteKind::Road)),
            ((3, 2), link(Dir::East, RouteKind::Railroad)),
        ];
        let d = road_decals(&links, 10, &skins(), &|_, _| false);
        assert_eq!(d.len(), 2);
        let names: BTreeSet<&str> = d.iter().map(|m| m.albedo.as_str()).collect();
        assert_eq!(names, BTreeSet::from(["tx:road.dds", "tx:rail.dds"]));
        // A city with spokes only: one straight spoke for each link, from the middle.
        let c = cell_center(3, 2);
        let spokes = city_road_decals(&links, 10, &skins(), &|_, _| true, CityMode::SpokesOnly);
        assert_eq!(spokes.len(), 2);
        for m in &spokes {
            let first = (m.verts[0].pos + m.verts[1].pos) * 0.5;
            assert!(near(first, c));
        }
        // With the ring (the mode of every city): the ring and a spoke for each link, from the ring.
        let city = road_decals(&links, 10, &skins(), &|_, _| true);
        assert_eq!(city.len(), 3);
        for m in &city[1..] {
            let first = (m.verts[0].pos + m.verts[1].pos) * 0.5;
            assert!((first.distance(c) - CITY_RING_RADIUS * 0.5 * HEX_WIDTH).abs() < 1e-3);
        }
        // No skin yet (the model has not come): nothing.
        assert!(road_decals(&links, 10, &RoadSkins::new(), &|_, _| false).is_empty());
    }

    #[test]
    fn the_layer_gets_the_roads_and_loses_the_ones_of_a_plot_that_has_none_now() {
        let mut l = DecalLayer::new();
        let links = [
            ((1, 0), link(Dir::East, RouteKind::Road)),
            ((2, 0), link(Dir::West, RouteKind::Road)),
        ];
        let touched: BTreeSet<(i32, i32)> = [(1, 0), (2, 0)].into();
        let no_city = |_: i32, _: i32| false;
        assert_eq!(
            apply(&mut l, &links, &[], &touched, 8, &skins(), &no_city),
            2
        );
        assert_eq!(l.len(), 2);
        // The road at (2, 0) is pillaged away: the plot is in `touched`, not in the links.
        assert_eq!(
            apply(&mut l, &links[..1], &[], &touched, 8, &skins(), &no_city),
            1
        );
        assert_eq!(l.len(), 1);
    }

    /// `Road_reg` of `RoadsandRails_new.gr2`: 47.37 wide (x), 192.33 long, u along the road.
    fn road_reg() -> RoadSkin {
        RoadSkin {
            albedo: LogicalId::new("tx:roadsandrails_new_d.dds"),
            height: Some(LogicalId::new("tx:roadsandrails_new_h.dds")),
            across: [0.4191, 0.2844],
            along: [0.6773, 0.9269],
            width: 47.372,
            length: 192.33,
            along_is_u: true,
        }
    }

    #[test]
    fn four_control_points_become_six_and_the_ribbon_has_eleven_quads() {
        let c = cell_center(3, 2);
        let ctrl = control_points(c, Dir::NorthEast, Dir::East);
        let tight = tighten(&ctrl);
        // `TightenRoadCurve`: N points give 2N - 2, the ends are not kept.
        assert_eq!((ctrl.len(), tight.len()), (4, 6));
        assert!(near(tight[0], (ctrl[0] * 2.0 + ctrl[1]) / 3.0));
        assert!(near(tight[5], (ctrl[2] + ctrl[3] * 2.0) / 3.0));
        // `UpdateRoadGeometry`: 12 samples, 11 quads, 22 triangles of the strip of both sides.
        let tris = ribbon(&road_curve(c, Dir::NorthEast, Dir::East));
        assert_eq!(tris.len(), (RIBBON_SAMPLES - 1) * 2);
        let straight = ribbon(&road_curve(c, Dir::West, Dir::East));
        assert_eq!(straight.len(), 22);
        // The ribbon runs from one edge of the plot to the other.
        let xs = straight.iter().flatten().map(|p| p.x);
        let (lo, hi) = xs.fold((f32::MAX, f32::MIN), |(l, h), x| (l.min(x), h.max(x)));
        assert!(
            (lo - (c.x - HEX_WIDTH * 0.5)).abs() < 1e-2
                && (hi - (c.x + HEX_WIDTH * 0.5)).abs() < 1e-2
        );
    }

    #[test]
    fn the_ribbon_is_twelve_texels_wide_in_the_texel_plane() {
        // Along X a texel is one unit and a row 0.866: the ribbon of a road east to west is 12 texels
        // of rows, 10.4 units, wide; a road north to south 12 units.
        let east: Vec<Vec2> = (0..9).map(|i| Vec2::new(i as f32 * 8.0, 0.0)).collect();
        let zs: Vec<f32> = ribbon(&east).iter().flatten().map(|p| p.y).collect();
        let wide = zs.iter().cloned().fold(f32::MIN, f32::max)
            - zs.iter().cloned().fold(f32::MAX, f32::min);
        assert!((wide - 12.0 * LAYOUT_ROW_RATIO).abs() < 1e-3, "{wide}");
        let south: Vec<Vec2> = (0..9).map(|i| Vec2::new(0.0, i as f32 * 8.0)).collect();
        let xs: Vec<f32> = ribbon(&south).iter().flatten().map(|p| p.x).collect();
        let wide = xs.iter().cloned().fold(f32::MIN, f32::max)
            - xs.iter().cloned().fold(f32::MAX, f32::min);
        assert!((wide - 12.0).abs() < 1e-3, "{wide}");
    }

    #[test]
    fn the_strip_of_road_reg_is_four_point_seven_texels_wide_in_tiles_of_nineteen() {
        let sk = road_reg();
        // A way of 64 texels down the columns: 3 tiles (floor(64 / 19.23)), each 21.3 long.
        let way = [Vec2::ZERO, Vec2::new(0.0, 64.0 * LAYOUT_ROW_RATIO)];
        let m = strip_decal(&way, 0.0, &sk, 0).expect("a strip");
        let wide = m.verts[0].pos.distance(m.verts[1].pos);
        assert!((wide - 4.737).abs() < 1e-3, "{wide}");
        assert!((tile_length(&sk, 64.0) - 64.0 / 3.0).abs() < 1e-3);
        assert_eq!(m.verts.len() / 4, 3);
        // Every tile reads the piece's own u range from its start to its end, v across.
        for q in m.verts.chunks(4) {
            assert!((q[0].uv.x - 0.6773).abs() < 1e-4 && (q[2].uv.x - 0.9269).abs() < 1e-4);
            assert_eq!((q[0].uv.y, q[1].uv.y), (0.4191, 0.2844));
        }
        // A way shorter than a tile stretches one tile, never squeezes it.
        assert!((tile_length(&sk, 10.0) - 10.0).abs() < 1e-4);
        // The albedo and the height of the road material, the priority of Main_Road_01.
        assert_eq!(m.albedo.as_str(), "tx:roadsandrails_new_d.dds");
        assert_eq!(
            m.height.as_ref().unwrap().as_str(),
            "tx:roadsandrails_new_h.dds"
        );
        assert_eq!(m.priority, 0.7);
    }

    #[test]
    fn a_tree_on_the_road_is_cleared_and_one_beside_it_stays() {
        use crate::world::forest::batches::{CutGate, ForestGate, OpenGate, TreeCuts};
        let links = [
            ((3, 2), link(Dir::East, RouteKind::Road)),
            ((4, 2), link(Dir::West, RouteKind::Road)),
        ];
        let cuts = road_cuts(&links, 10, &|_, _| false);
        let cells: Vec<u32> = cuts.keys().copied().collect();
        assert_eq!(cells, [23, 24]);
        let cuts = TreeCuts::new(cuts.into_values().flatten());
        let gate = CutGate {
            inner: &OpenGate,
            cuts: &cuts,
        };
        let c = cell_center(3, 2);
        // On the way through the middle of the plot, and a little off it (the ribbon is 5.2 to a side).
        assert!(!gate.allows(c, 0.0));
        assert!(!gate.allows(c + Vec2::new(5.0, 4.0), 0.0));
        // Beside it, and on the plot's other side: the trees stay.
        assert!(gate.allows(c + Vec2::new(5.0, 7.0), 0.0));
        assert!(gate.allows(c + Vec2::new(0.0, -7.0), 0.0));
        assert!(gate.allows(c + Vec2::new(0.0, 25.0), 0.0));
        // The road of the next plot goes on to its middle.
        assert!(!gate.allows(cell_center(4, 2), 0.0));
    }

    #[test]
    fn the_ring_of_a_city_passes_the_middles_of_the_edges_of_its_corners_and_closes() {
        let c = cell_center(3, 2);
        let ring = ring_curve(c);
        assert!(near(ring[0], *ring.last().unwrap()), "closed");
        // The curve of the six corners and the first two again starts and ends half way between the
        // first two corners, 0.45 of the way to the edge of the plot: the middle of an edge of the ring.
        let apothem = CITY_RING_RADIUS * 0.5 * HEX_WIDTH;
        assert!((ring[0].distance(c) - apothem).abs() < 1e-3);
        // The spokes start on it.
        for d in Dir::ALL {
            let from = spoke_curve(c, d, CityMode::RingAndSpokes)[0];
            let nearest = ring
                .iter()
                .map(|p| p.distance(from))
                .fold(f32::MAX, f32::min);
            assert!(nearest < 0.5, "{d:?} {nearest}");
        }
        // Without the ring a spoke starts at the middle and ends at the middle of the edge.
        let s = spoke_curve(c, Dir::East, CityMode::SpokesOnly);
        assert!(near(s[0], c) && near(*s.last().unwrap(), c + Vec2::new(HEX_WIDTH * 0.5, 0.0)));
    }

    /// Writes a road through a forest as text (the strip of the road with its texture coordinates, the
    /// ribbon, every tree of the plots with whether the cuts leave it) to the file `CIV5_ROAD_SCENE_OUT`
    /// names, for a script that draws it with the real road texture; does nothing without it.
    #[test]
    fn a_road_through_a_forest_can_be_written_for_the_screenshot() {
        use crate::hexgrid::GridShape;
        use crate::terrain::Relief;
        use crate::world::forest::batches::{CutGate, OpenGate, TreeCell, TreeCuts};
        use crate::world::forest::scatter::{ForestTiles, TreeKind};
        use std::fmt::Write as _;

        let Some(path) = std::env::var_os("CIV5_ROAD_SCENE_OUT") else {
            return;
        };
        let shape = GridShape {
            width: 12,
            height: 8,
            wrap_x: false,
        };
        // A road from (2, 3) east, then north east and east again.
        let mut links: Vec<DrawnLink> = Vec::new();
        let mut at = (2, 3);
        for dir in [Dir::East, Dir::East, Dir::NorthEast, Dir::East, Dir::East] {
            let to = shape.neighbor(at.0, at.1, dir).expect("on the map");
            links.push((at, link(dir, RouteKind::Road)));
            links.push((to, link(dir.opposite(), RouteKind::Road)));
            at = to;
        }
        let mut plots: BTreeSet<(i32, i32)> = links.iter().map(|(p, _)| *p).collect();
        // The forest is the road's plots and the ones round them.
        for (x, y) in plots.clone() {
            for dir in Dir::ALL {
                plots.extend(shape.neighbor(x, y, dir));
            }
        }
        let decals = road_decals(
            &links,
            12,
            &RoadSkins::from([((RouteKind::Road, false), road_reg())]),
            &|_, _| false,
        );
        let cuts = TreeCuts::new(road_cuts(&links, 12, &|_, _| false).into_values().flatten());
        let tiles = ForestTiles::default();
        let mut out = String::new();
        for (x, y) in &plots {
            let c = cell_center(*x, *y);
            writeln!(out, "H {} {}", c.x, c.y).unwrap();
            let cell = TreeCell {
                kind: TreeKind::Forest,
                style: 0,
                relief: Relief::Flat,
                x: *x,
                y: *y,
            };
            let kept: BTreeSet<[u32; 2]> = cell
                .instances_gated(
                    0,
                    &tiles,
                    &CutGate {
                        inner: &OpenGate,
                        cuts: &cuts,
                    },
                )
                .iter()
                .map(|i| [i.position[0].to_bits(), i.position[2].to_bits()])
                .collect();
            for i in cell.instances_gated(0, &tiles, &OpenGate) {
                let key = [i.position[0].to_bits(), i.position[2].to_bits()];
                writeln!(
                    out,
                    "T {} {} {} {}",
                    i.position[0],
                    i.position[2],
                    i.scale,
                    u8::from(kept.contains(&key))
                )
                .unwrap();
            }
        }
        for m in &decals {
            for t in &m.tris {
                let v = |i: u16| m.verts[usize::from(i)];
                writeln!(
                    out,
                    "Q {}",
                    t.iter()
                        .map(|&i| format!(
                            "{} {} {} {}",
                            v(i).pos.x,
                            v(i).pos.y,
                            v(i).uv.x,
                            v(i).uv.y
                        ))
                        .collect::<Vec<_>>()
                        .join(" ")
                )
                .unwrap();
            }
        }
        for t in cuts_triangles(&links) {
            writeln!(
                out,
                "R {} {} {} {} {} {}",
                t[0].x, t[0].y, t[1].x, t[1].y, t[2].x, t[2].y
            )
            .unwrap();
        }
        std::fs::write(path, out).unwrap();
    }

    fn cuts_triangles(links: &[DrawnLink]) -> Vec<[Vec2; 3]> {
        road_cuts(links, 12, &|_, _| false)
            .into_values()
            .flatten()
            .collect()
    }

    #[test]
    fn a_coastal_road_is_drawn_only_where_the_plot_has_a_route() {
        // A line along the north edge of row 0, over plots 0..5: the route is on 1 and 2 only.
        let road: Vec<Vec2> = (0..6)
            .map(|x| cell_center(x, 0) + Vec2::new(0.0, -10.0))
            .collect();
        let has = |x: i32, _y: i32| x == 1 || x == 2;
        let d = coastal_decals(std::slice::from_ref(&road), 8, &skins(), &has);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].cell, 1);
        assert!(!d[0].verts.is_empty() && d[0].verts.len().is_multiple_of(4));
        // A single plot with a route in a line: too short for a strip.
        let one = |x: i32, _y: i32| x == 3;
        assert!(coastal_decals(std::slice::from_ref(&road), 8, &skins(), &one).is_empty());
        // Without the skin of a plain road nothing is drawn.
        assert!(coastal_decals(&[road], 8, &RoadSkins::new(), &has).is_empty());
    }
}
