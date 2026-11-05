//! Terrain mesh: flat land, hill dome, mountain peak, flat water, skirts.
//!
//! One vertex set for the whole map, two index lists (`land`, `water`) so the water can use its own
//! pipeline later. Cells are placed at their unwrapped world position; a wrapping map is drawn
//! three times with a shift of `±64 * width`.
//!
//! No cracks: every cell's rim is the same six corners the neighbour computes, from one `f64`
//! formula rounded once, and rim heights depend only on whether the cell is water. Land next to
//! water gets a vertical skirt from the land rim down to the water level, so the two surfaces meet
//! along the skirt's bottom edge.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use glam::Vec3;

use super::{Relief, TerrainSnapshot};
use crate::hexgrid::{Dir, edge_corners};

/// Plain land height. // re: height maps of the Africa patches
pub const LAND_HEIGHT: f32 = 12.0;
/// Top of a hill, about +13 over the land. // re: Afri_*_Hill_1_1_H.dds
pub const HILL_TOP: f32 = 25.0;
/// Peak of a mountain, about +37 over the land. // re: Afri_Mountain_*_H.dds
pub const MOUNTAIN_TOP: f32 = 49.0;
/// Water surface. // re: WaterSystem::Startup
pub const WATER_HEIGHT: f32 = 10.12;

/// Upper bound for the whole map's triangles. The spec gives no number;
/// 100k is far below what one indexed draw handles.
pub const TRIANGLE_BUDGET: usize = 100_000;

/// Hill ring radius as a fraction of the corner radius; the dome profile is `12 + 13 (1 - r^2)`.
// SIMPLIFIED(SIMP-006): own dome instead of the height maps of the original patches
pub(crate) const HILL_RING: f32 = 0.5;
/// Mountain ring radii, inner first; the profile is linear from the peak to the rim.
// SIMPLIFIED(SIMP-006): own cone instead of the height maps of the original patches
const MOUNTAIN_RINGS: [f32; 2] = [0.33, 0.66];

/// Hex width in `f64`: neighbours compute their shared corners with the same formula, so the
/// results agree to the last bit before the single rounding to `f32`.
const HEX_WIDTH_64: f64 = 64.0;

/// Which texture a vertex belongs to: the base terrain, the mountain top, or a skirt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum VertexPart {
    Base = 0,
    MountainTop = 1,
    Skirt = 2,
}

/// Position (0), normal (12), cell index into `TerrainSnapshot::cells` (24), part (28); stride 32.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct TerrainVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub cell: u32,
    pub part: u32,
}

impl TerrainVertex {
    pub fn part(&self) -> VertexPart {
        match self.part {
            1 => VertexPart::MountainTop,
            2 => VertexPart::Skirt,
            _ => VertexPart::Base,
        }
    }
}

/// CPU terrain mesh. Triangles are counter-clockwise seen from outside.
#[derive(Debug, Clone, Default)]
pub struct TerrainMesh {
    pub vertices: Vec<TerrainVertex>,
    /// Land surface and skirts.
    pub land: Vec<u32>,
    /// Water surface.
    pub water: Vec<u32>,
}

impl TerrainMesh {
    pub fn triangle_count(&self) -> usize {
        (self.land.len() + self.water.len()) / 3
    }

    /// Every index points at a vertex and both lists hold whole triangles.
    pub fn indices_valid(&self) -> bool {
        let n = self.vertices.len() as u32;
        self.land.len().is_multiple_of(3)
            && self.water.len().is_multiple_of(3)
            && self.land.iter().chain(&self.water).all(|&i| i < n)
    }

    pub fn build(snapshot: &TerrainSnapshot) -> Self {
        let mut b = Builder::default();
        let (w, h) = (snapshot.width() as i32, snapshot.height() as i32);
        for y in 0..h {
            for x in 0..w {
                b.cell(snapshot, x, y);
            }
        }
        b.smooth_normals();
        for y in 0..h {
            for x in 0..w {
                b.skirts(snapshot, x, y);
            }
        }
        TerrainMesh {
            vertices: b.vertices,
            land: b.land,
            water: b.water,
        }
    }
}

impl TerrainMesh {
    /// Diagnostic for tests: the mesh must be watertight except along the map outline. Every
    /// directed edge of every triangle needs exactly one opposite edge, found in this mesh or, on a
    /// wrapping map, in a copy shifted by `±64 * width`. Edges left over must lie between corners
    /// of the map outline (cells missing a neighbour). Positions are compared at 0.05 units.
    pub fn check_no_cracks(&self, snapshot: &TerrainSnapshot) -> Result<(), String> {
        type Key = [i64; 3];
        let key = |p: [f32; 3], dx: f32| -> Key {
            [p[0] + dx, p[1], p[2]].map(|c| (f64::from(c) * 20.0).round() as i64)
        };
        let shape = snapshot.shape();
        let shifts: Vec<f32> = if snapshot.wrap_x() {
            vec![0.0, -shape.world_width(), shape.world_width()]
        } else {
            vec![0.0]
        };
        let mut count: HashMap<(Key, Key), u32> = HashMap::new();
        for &dx in &shifts {
            for tri in self.land.chunks_exact(3).chain(self.water.chunks_exact(3)) {
                for k in 0..3 {
                    let a = self.vertices[tri[k] as usize].position;
                    let b = self.vertices[tri[(k + 1) % 3] as usize].position;
                    *count.entry((key(a, dx), key(b, dx))).or_default() += 1;
                }
            }
        }
        // Outline corners (xz only), from cells that miss a neighbour.
        let mut outline: std::collections::HashSet<[i64; 2]> = Default::default();
        for y in 0..shape.height {
            for x in 0..shape.width {
                let (cx, cz) = center_64(x, y);
                for dir in Dir::ALL {
                    if shape.neighbor(x, y, dir).is_none() {
                        let (a, b) = edge_corners(dir);
                        for c in [a, b] {
                            let (ox, oz) = corner_64(c);
                            outline.insert([
                                (f64::from((cx + ox) as f32) * 20.0).round() as i64,
                                (f64::from((cz + oz) as f32) * 20.0).round() as i64,
                            ]);
                        }
                    }
                }
            }
        }
        for tri in self.land.chunks_exact(3).chain(self.water.chunks_exact(3)) {
            for k in 0..3 {
                let a = self.vertices[tri[k] as usize].position;
                let b = self.vertices[tri[(k + 1) % 3] as usize].position;
                let (ka, kb) = (key(a, 0.0), key(b, 0.0));
                if ka == kb {
                    return Err(format!("degenerate edge at {a:?}"));
                }
                if count[&(ka, kb)] != 1 {
                    return Err(format!(
                        "edge {a:?} -> {b:?} used {} times",
                        count[&(ka, kb)]
                    ));
                }
                if count.get(&(kb, ka)) != Some(&1) {
                    let on_outline = |k: Key| outline.contains(&[k[0], k[2]]);
                    if !(on_outline(ka) && on_outline(kb)) {
                        return Err(format!("crack: edge {a:?} -> {b:?} has no partner"));
                    }
                }
            }
        }
        Ok(())
    }
}

/// World `(X, Z)` of a cell centre in `f64`.
fn center_64(x: i32, y: i32) -> (f64, f64) {
    let row_step = HEX_WIDTH_64 * 3f64.sqrt() / 2.0;
    (
        HEX_WIDTH_64 * (f64::from(x) + 0.5 * f64::from(y & 1)),
        -row_step * f64::from(y),
    )
}

/// Corner `i` offset from the centre in `f64`; the same angles as `hexgrid::corner_offset`.
fn corner_64(i: usize) -> (f64, f64) {
    let radius = HEX_WIDTH_64 / 3f64.sqrt();
    let a = (30.0 + 60.0 * i as f64).to_radians();
    (radius * a.cos(), -radius * a.sin())
}

#[derive(Default)]
struct Builder {
    vertices: Vec<TerrainVertex>,
    land: Vec<u32>,
    water: Vec<u32>,
}

impl Builder {
    fn push(&mut self, x: f64, h: f32, z: f64, cell: usize, part: VertexPart) -> u32 {
        let i = self.vertices.len() as u32;
        self.vertices.push(TerrainVertex {
            position: [x as f32, h, z as f32],
            normal: [0.0, 1.0, 0.0],
            cell: cell as u32,
            part: part as u32,
        });
        i
    }

    fn ring(
        &mut self,
        centre: (f64, f64),
        frac: f32,
        h: f32,
        cell: usize,
        part: VertexPart,
    ) -> [u32; 6] {
        std::array::from_fn(|i| {
            let (ox, oz) = corner_64(i);
            let f = f64::from(frac);
            self.push(centre.0 + ox * f, h, centre.1 + oz * f, cell, part)
        })
    }

    fn cell(&mut self, s: &TerrainSnapshot, x: i32, y: i32) {
        let idx = s.index(x, y).expect("cell in map");
        let relief = s.cells()[idx].drawn_relief();
        let centre = center_64(x, y);

        if relief == Relief::Water {
            // A centre vertex as well, so the water pass can give the middle of the cell its own
            // depth (`terrain::water`).
            let c = self.push(centre.0, WATER_HEIGHT, centre.1, idx, VertexPart::Base);
            let rim = self.ring(centre, 1.0, WATER_HEIGHT, idx, VertexPart::Base);
            for i in 0..6 {
                self.water.extend_from_slice(&[c, rim[i], rim[(i + 1) % 6]]);
            }
            return;
        }

        // Inner rings, innermost first: (radius fraction, height, part).
        let mut inner: Vec<(f32, f32, VertexPart)> = Vec::new();
        let (top, top_part) = match relief {
            Relief::Hill => {
                let h = LAND_HEIGHT + (HILL_TOP - LAND_HEIGHT) * (1.0 - HILL_RING * HILL_RING);
                inner.push((HILL_RING, h, VertexPart::Base));
                (HILL_TOP, VertexPart::Base)
            }
            Relief::Mountain => {
                for (k, &r) in MOUNTAIN_RINGS.iter().enumerate() {
                    let h = LAND_HEIGHT + (MOUNTAIN_TOP - LAND_HEIGHT) * (1.0 - r);
                    let part = if k == 0 {
                        VertexPart::MountainTop
                    } else {
                        VertexPart::Base
                    };
                    inner.push((r, h, part));
                }
                (MOUNTAIN_TOP, VertexPart::MountainTop)
            }
            _ => (LAND_HEIGHT, VertexPart::Base),
        };

        let c = self.push(centre.0, top, centre.1, idx, top_part);
        let mut rings: Vec<[u32; 6]> = inner
            .iter()
            .map(|&(f, h, p)| self.ring(centre, f, h, idx, p))
            .collect();
        rings.push(self.ring(centre, 1.0, LAND_HEIGHT, idx, VertexPart::Base));

        for i in 0..6 {
            let j = (i + 1) % 6;
            self.land.extend_from_slice(&[c, rings[0][i], rings[0][j]]);
        }
        for pair in rings.windows(2) {
            let (a, o) = (&pair[0], &pair[1]);
            for i in 0..6 {
                let j = (i + 1) % 6;
                self.land.extend_from_slice(&[a[i], o[i], o[j]]);
                self.land.extend_from_slice(&[a[i], o[j], a[j]]);
            }
        }
    }

    /// Averages face normals of the surface triangles over vertices at the same position, so hill
    /// rims and flat neighbours shade without a seam. Skirts are added later with their own
    /// horizontal normals.
    fn smooth_normals(&mut self) {
        let pos = |v: &TerrainVertex| Vec3::from(v.position);
        let key = |v: &TerrainVertex| v.position.map(f32::to_bits);
        let mut sums: HashMap<[u32; 3], Vec3> = HashMap::new();
        for tri in self.land.chunks_exact(3).chain(self.water.chunks_exact(3)) {
            let [a, b, c] = [0, 1, 2].map(|k| &self.vertices[tri[k] as usize]);
            let n = (pos(b) - pos(a)).cross(pos(c) - pos(a));
            for v in [a, b, c] {
                *sums.entry(key(v)).or_default() += n;
            }
        }
        for v in &mut self.vertices {
            let n = sums.get(&key(v)).copied().unwrap_or(Vec3::Y);
            v.normal = n.normalize_or(Vec3::Y).to_array();
        }
    }

    /// Walls from the land rim down to the water for every edge facing a revealed water cell.
    fn skirts(&mut self, s: &TerrainSnapshot, x: i32, y: i32) {
        let idx = s.index(x, y).expect("cell in map");
        if s.cells()[idx].drawn_relief() == Relief::Water {
            return;
        }
        let shape = s.shape();
        let centre = center_64(x, y);
        for dir in Dir::ALL {
            let Some((nx, ny)) = shape.neighbor(x, y, dir) else {
                continue;
            };
            let Some(n) = s.cell(nx, ny) else { continue };
            if n.drawn_relief() != Relief::Water {
                continue;
            }
            let (ca, cb) = edge_corners(dir);
            let ang = dir.normal_angle();
            let normal = [ang.cos(), 0.0, -ang.sin()];
            let mut corner = |c: usize, h: f32| {
                let (ox, oz) = corner_64(c);
                let i = self.push(centre.0 + ox, h, centre.1 + oz, idx, VertexPart::Skirt);
                self.vertices[i as usize].normal = normal;
                i
            };
            let (a_top, a_bot) = (corner(ca, LAND_HEIGHT), corner(ca, WATER_HEIGHT));
            let (b_top, b_bot) = (corner(cb, LAND_HEIGHT), corner(cb, WATER_HEIGHT));
            self.land
                .extend_from_slice(&[a_bot, b_bot, b_top, a_bot, b_top, a_top]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::{Fog, TerrainCell};

    fn cell(relief: Relief, revealed: bool) -> TerrainCell {
        TerrainCell {
            relief,
            fog: if revealed {
                Fog::Visible
            } else {
                Fog::Unrevealed
            },
            ..TerrainCell::default()
        }
    }

    fn snapshot(
        w: u32,
        h: u32,
        wrap: bool,
        f: impl Fn(i32, i32) -> TerrainCell,
    ) -> TerrainSnapshot {
        let cells = (0..h as i32)
            .flat_map(|y| (0..w as i32).map(move |x| (x, y)))
            .map(|(x, y)| f(x, y))
            .collect();
        TerrainSnapshot::new(w, h, wrap, cells).unwrap()
    }

    /// Deterministic mix of every relief, including unrevealed cells.
    fn mixed(x: i32, y: i32) -> TerrainCell {
        let h = (x * 31 + y * 17 + x * y * 7).rem_euclid(23);
        match h {
            0 | 1 => cell(Relief::Mountain, true),
            2..=4 => cell(Relief::Hill, true),
            5..=11 => cell(Relief::Water, true),
            12 => cell(Relief::Mountain, false),
            13 => cell(Relief::Water, false),
            _ => cell(Relief::Flat, true),
        }
    }

    #[test]
    fn no_cracks_with_every_relief_mix() {
        for wrap in [false, true] {
            let s = snapshot(40, 24, wrap, mixed);
            let m = TerrainMesh::build(&s);
            m.check_no_cracks(&s)
                .unwrap_or_else(|e| panic!("wrap {wrap}: {e}"));
        }
    }

    #[test]
    fn no_cracks_on_odd_and_tiny_maps() {
        for (w, h) in [(1, 1), (2, 3), (5, 4), (7, 7)] {
            for wrap in [false, true] {
                let s = snapshot(w, h, wrap, mixed);
                TerrainMesh::build(&s)
                    .check_no_cracks(&s)
                    .unwrap_or_else(|e| panic!("{w}x{h} wrap {wrap}: {e}"));
            }
        }
    }

    #[test]
    fn crack_check_detects_a_missing_skirt() {
        let s = snapshot(5, 5, false, |x, y| {
            cell(
                if (x, y) == (2, 2) {
                    Relief::Water
                } else {
                    Relief::Flat
                },
                true,
            )
        });
        let mut m = TerrainMesh::build(&s);
        assert!(m.check_no_cracks(&s).is_ok());
        // Drop the skirt (the last two triangles of the land list).
        m.land.truncate(m.land.len() - 6);
        assert!(m.check_no_cracks(&s).is_err());
    }

    fn one(relief: Relief, revealed: bool) -> TerrainMesh {
        TerrainMesh::build(&snapshot(1, 1, false, |_, _| cell(relief, revealed)))
    }

    #[test]
    fn triangle_counts_per_relief() {
        assert_eq!(one(Relief::Flat, true).triangle_count(), 6);
        assert_eq!(one(Relief::Hill, true).triangle_count(), 18);
        assert_eq!(one(Relief::Mountain, true).triangle_count(), 30);
        assert_eq!(one(Relief::Water, true).triangle_count(), 6);
        // Unrevealed relief is not given away.
        assert_eq!(one(Relief::Mountain, false).triangle_count(), 6);
    }

    fn max_y(m: &TerrainMesh) -> f32 {
        m.vertices
            .iter()
            .map(|v| v.position[1])
            .fold(f32::MIN, f32::max)
    }

    #[test]
    fn heights_follow_the_spec() {
        assert_eq!(max_y(&one(Relief::Flat, true)), LAND_HEIGHT);
        assert_eq!(max_y(&one(Relief::Hill, true)), HILL_TOP);
        assert_eq!(max_y(&one(Relief::Mountain, true)), MOUNTAIN_TOP);
        assert_eq!(max_y(&one(Relief::Water, true)), WATER_HEIGHT);
        assert_eq!(max_y(&one(Relief::Mountain, false)), LAND_HEIGHT);
        // Rim of hills and mountains stays at the land height.
        for relief in [Relief::Hill, Relief::Mountain] {
            let m = one(relief, true);
            let min = m
                .vertices
                .iter()
                .map(|v| v.position[1])
                .fold(f32::MAX, f32::min);
            assert_eq!(min, LAND_HEIGHT);
        }
    }

    #[test]
    fn faces_point_outward() {
        let s = snapshot(40, 24, true, mixed);
        let m = TerrainMesh::build(&s);
        let mut skirts = 0;
        for tri in m.land.chunks_exact(3).chain(m.water.chunks_exact(3)) {
            let [a, b, c] = [0, 1, 2].map(|k| &m.vertices[tri[k] as usize]);
            let p = |v: &TerrainVertex| Vec3::from(v.position);
            let n = (p(b) - p(a)).cross(p(c) - p(a));
            assert!(n.length() > 1e-3, "degenerate triangle");
            if a.part() == VertexPart::Skirt {
                skirts += 1;
                assert!(n.dot(Vec3::from(a.normal)) > 0.0, "skirt faces inward");
            } else {
                assert!(n.y > 0.0, "surface faces down");
            }
        }
        assert!(skirts > 0);
    }

    #[test]
    fn skirts_only_between_land_and_water() {
        let land_only = snapshot(6, 6, false, |_, _| cell(Relief::Hill, true));
        let m = TerrainMesh::build(&land_only);
        assert!(m.vertices.iter().all(|v| v.part() != VertexPart::Skirt));
        // Land next to unrevealed land: no wall either.
        let fog = snapshot(2, 1, false, |x, _| cell(Relief::Flat, x == 0));
        let m = TerrainMesh::build(&fog);
        assert!(m.vertices.iter().all(|v| v.part() != VertexPart::Skirt));
        // One land cell between two water cells on a row: 2 skirts = 4 triangles.
        let strip = snapshot(3, 1, false, |x, _| {
            cell(if x == 1 { Relief::Flat } else { Relief::Water }, true)
        });
        let m = TerrainMesh::build(&strip);
        assert_eq!(m.triangle_count(), 6 + 6 + 6 + 4);
    }

    #[test]
    fn normals_are_unit_and_flat_land_is_up() {
        let s = snapshot(4, 4, false, mixed);
        let m = TerrainMesh::build(&s);
        for v in &m.vertices {
            assert!((Vec3::from(v.normal).length() - 1.0).abs() < 1e-4);
        }
        let flat = one(Relief::Flat, true);
        assert!(flat.vertices.iter().all(|v| v.normal == [0.0, 1.0, 0.0]));
    }

    #[test]
    fn geometry_ignores_fog_and_material() {
        let a = snapshot(3, 3, false, mixed);
        let mut cells = a.cells().to_vec();
        for c in &mut cells {
            if c.is_revealed() {
                c.fog = Fog::Hidden;
            }
            c.art = 3;
        }
        let b = TerrainSnapshot::new(3, 3, false, cells).unwrap();
        assert!(a.same_geometry(&b));
        let mut cells = a.cells().to_vec();
        cells[0].relief = Relief::Water;
        cells[0].fog = Fog::Visible;
        let c = TerrainSnapshot::new(3, 3, false, cells).unwrap();
        assert!(!a.same_geometry(&c));
    }

    #[test]
    fn vertex_layout() {
        assert_eq!(std::mem::size_of::<TerrainVertex>(), 32);
    }
}
