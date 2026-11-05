//! The hex meshes of the minimap (world-overlays WOV3, `cvMinimapRenderer::BuildHexMeshes`).
//!
//! The ranges are those of the original, in its order: fog, coast, land, then one range per culture
//! owner (found in the view, in column-then-row order). The view of the main camera decides which
//! cells are made, with a cell of margin; what lies outside the map is fog. The meshes are made
//! again only when the fog, the culture or the explored region changes (`OnFOWChanged`,
//! `OnHexCultureChanged`).
//!
//! The vertices are positions in world units (original basis); the minimap camera is an offset and a
//! scale in the vertex shader.
// TODO(RW-21): `PushTessellatedHexToMesh` follows the edges of a cell with splines
// (`GetSplineForEdge`, `m_ucFog >> 2` are the edge flags) and neither body is in the dump: the hexes
// here are plain, six triangles with straight edges. The rivers and the mountain sprites are made
// from the view too (`g_RiverVisSystem::GetGeometry`, `BuildSprites` are not in the dump either).

use std::ops::Range;

use bytemuck::{Pod, Zeroable};
use glam::Vec2;

use super::camera::ViewRect;
use super::layers::{Ground, MinimapMap, cell_pos, corner};
use crate::hexgrid::{Dir, HEX_WIDTH, ROW_STEP, edge_corners};

/// A vertex of a hex: `dcl_position` (x, y, 0, 1).
pub type HexVertex = [f32; 4];

/// A vertex of a sprite: position, the texture's coordinates, the colour in gamma.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct SpriteVertex {
    pub position: [f32; 4],
    pub uv: [f32; 4],
    pub color: [f32; 4],
}

/// Side of a mountain sprite in hex widths.
// TODO(RW-21): `BuildSprites` is not in the dump; the size is chosen to read at 320 x 190.
pub const MOUNTAIN_SPRITE: f32 = 0.9;
/// Width of a river on the minimap in pixels.
// TODO(RW-21): the original draws the geometry of the river system.
pub const RIVER_PIXELS: f32 = 1.5;

/// A range of the index buffer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MeshRange {
    pub first_index: u32,
    pub num_indices: u32,
}

impl MeshRange {
    pub fn indices(&self) -> Range<u32> {
        self.first_index..self.first_index + self.num_indices
    }

    pub fn is_empty(&self) -> bool {
        self.num_indices == 0
    }
}

/// The hexes of one culture owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CultureRange {
    pub owner: u8,
    pub range: MeshRange,
}

/// Everything the layers draw from the grid.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MinimapMeshes {
    pub vertices: Vec<HexVertex>,
    pub indices: Vec<u32>,
    pub fog: MeshRange,
    pub coast: MeshRange,
    pub land: MeshRange,
    pub ice: MeshRange,
    pub rivers: MeshRange,
    pub culture: Vec<CultureRange>,
    /// Four vertices per sprite, in the order of [`quad_indices`].
    pub mountains: Vec<SpriteVertex>,
}

/// Indices of `n` quads of four vertices (two triangles each).
pub fn quad_indices(n: usize) -> Vec<u32> {
    (0..n as u32)
        .flat_map(|q| {
            let b = q * 4;
            [b, b + 1, b + 2, b, b + 2, b + 3]
        })
        .collect()
}

/// The cells `(x, y)` the meshes are made for: the view and a cell of margin, columns outer and rows
/// inner like the original's loops.
pub fn cells_in_view(view: &ViewRect) -> impl Iterator<Item = (i32, i32)> {
    let row0 = (view.min.y / ROW_STEP).floor() as i32 - 1;
    let row1 = (view.max.y / ROW_STEP).ceil() as i32 + 1;
    let col0 = (view.min.x / HEX_WIDTH - 0.5).floor() as i32 - 1;
    let col1 = (view.max.x / HEX_WIDTH).ceil() as i32 + 1;
    (col0..=col1).flat_map(move |x| (row0..=row1).map(move |y| (x, y)))
}

fn push_hex(vertices: &mut Vec<HexVertex>, indices: &mut Vec<u32>, center: Vec2) {
    let base = vertices.len() as u32;
    vertices.push([center.x, center.y, 0.0, 1.0]);
    for i in 0..6 {
        let p = center + corner(i);
        vertices.push([p.x, p.y, 0.0, 1.0]);
    }
    for i in 0..6u32 {
        indices.extend_from_slice(&[base, base + 1 + i, base + 1 + (i + 1) % 6]);
    }
}

/// The neighbour of `(x, y)` towards `dir`, unwrapped (`GridShape::neighbor` without the bounds).
fn neighbor(x: i32, y: i32, dir: Dir) -> (i32, i32) {
    let (dx, dy) = dir.hex_delta();
    let ny = y + dy;
    ((x - (y >> 1) + dx) + (ny >> 1), ny)
}

fn push_quad(sprites: &mut Vec<SpriteVertex>, center: Vec2, half: f32, color: [f32; 4]) {
    for (c, uv) in [
        ((-1.0, -1.0), (0.0, 1.0)),
        ((1.0, -1.0), (1.0, 1.0)),
        ((1.0, 1.0), (1.0, 0.0)),
        ((-1.0, 1.0), (0.0, 0.0)),
    ] {
        sprites.push(SpriteVertex {
            position: [center.x + c.0 * half, center.y + c.1 * half, 0.0, 1.0],
            uv: [uv.0, uv.1, 0.0, 0.0],
            color,
        });
    }
}

/// Makes the meshes of `map` for `view`. `pixels_per_unit` is the scale of the target, for the
/// width of the rivers.
pub fn build_meshes(map: &MinimapMap, view: &ViewRect, pixels_per_unit: f32) -> MinimapMeshes {
    let cells: Vec<(i32, i32)> = cells_in_view(view).collect();
    let mut m = MinimapMeshes::default();
    let range_from = |m: &MinimapMeshes| m.indices.len() as u32;

    let pass = |m: &mut MinimapMeshes, want: &dyn Fn(&super::layers::MinimapCell) -> bool| {
        let first = range_from(m);
        for &(x, y) in &cells {
            if want(&map.cell(x, y)) {
                push_hex(&mut m.vertices, &mut m.indices, cell_pos(x, y));
            }
        }
        MeshRange {
            first_index: first,
            num_indices: m.indices.len() as u32 - first,
        }
    };
    m.fog = pass(&mut m, &|c| c.unexplored);
    m.coast = pass(&mut m, &|c| c.ground == Ground::Coast);
    m.land = pass(&mut m, &|c| c.ground == Ground::Land);

    // Culture: the owners found in the view, in the order they are met.
    let mut owners: Vec<u8> = Vec::new();
    for &(x, y) in &cells {
        if let Some(o) = map.cell(x, y).owner
            && !owners.contains(&o)
        {
            owners.push(o);
        }
    }
    for owner in owners {
        let range = pass(&mut m, &|c| c.owner == Some(owner));
        m.culture.push(CultureRange { owner, range });
    }
    m.ice = pass(&mut m, &|c| c.ice);

    // Rivers: a band along each edge that has one, once per edge.
    let first = m.indices.len() as u32;
    let width = (RIVER_PIXELS / pixels_per_unit.max(1e-6)).max(HEX_WIDTH * 0.1);
    for &(x, y) in &cells {
        let cell = map.cell(x, y);
        if cell.rivers == 0 {
            continue;
        }
        let centre = cell_pos(x, y);
        for dir in Dir::ALL {
            if cell.rivers & (1 << dir.index()) == 0 {
                continue;
            }
            let (nx, ny) = neighbor(x, y, dir);
            let other = map.cell(nx, ny);
            let theirs = other.rivers & (1 << dir.opposite().index()) != 0;
            let n = cell_pos(nx, ny);
            // Both sides of an edge may carry the bit: the lower cell draws it.
            if theirs && (n.x, n.y) < (centre.x, centre.y) {
                continue;
            }
            let (a, b) = edge_corners(dir);
            let (p, q) = (centre + corner(a), centre + corner(b));
            let along = (q - p).normalize_or_zero();
            let side = Vec2::new(-along.y, along.x) * (width * 0.5);
            let base = m.vertices.len() as u32;
            for v in [p - side, q - side, q + side, p + side] {
                m.vertices.push([v.x, v.y, 0.0, 1.0]);
            }
            m.indices
                .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
    m.rivers = MeshRange {
        first_index: first,
        num_indices: m.indices.len() as u32 - first,
    };

    // Mountains: a sprite on each.
    for &(x, y) in &cells {
        if map.cell(x, y).mountain {
            push_quad(
                &mut m.mountains,
                cell_pos(x, y),
                HEX_WIDTH * MOUNTAIN_SPRITE * 0.5,
                [1.0, 1.0, 1.0, 1.0],
            );
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::test_util::{land, map as snapshot};
    use crate::terrain::{FeatureKind, Fog, Relief, TerrainCell, TerrainKind};
    use crate::world::overlay::OwnersInput;

    fn cell(kind: TerrainKind) -> TerrainCell {
        TerrainCell {
            kind,
            fog: Fog::Visible,
            ..land()
        }
    }

    fn whole(w: u32, h: u32) -> ViewRect {
        ViewRect {
            min: Vec2::new(-HEX_WIDTH, -ROW_STEP),
            max: Vec2::new(HEX_WIDTH * (w as f32 + 1.0), ROW_STEP * (h as f32 + 1.0)),
        }
    }

    /// Hexes of a range.
    fn hexes(r: MeshRange) -> u32 {
        r.num_indices / 18
    }

    #[test]
    fn each_layer_has_the_hexes_of_its_cells() {
        let snap = snapshot(4, 3, |x, y| match (x, y) {
            (0, 0) | (1, 0) => cell(TerrainKind::Coast),
            (2, 0) => cell(TerrainKind::Ocean),
            (3, 2) => TerrainCell::default(),
            _ => cell(TerrainKind::Grass),
        });
        let map = MinimapMap::from_snapshot(&snap);
        let m = build_meshes(&map, &whole(4, 3), 1.0);
        assert_eq!(hexes(m.coast), 2);
        // Land: the 12 cells, less 2 coast, 1 ocean; an unexplored cell is land by its byte (and
        // so is what lies off the map), the fog is drawn over it.
        assert!(hexes(m.land) >= 12 - 3);
        assert!(hexes(m.land) <= hexes(m.fog) + 9);
        assert!(
            hexes(m.fog) >= 1,
            "the unexplored cell and what lies off the map"
        );
        assert_eq!(
            m.ice,
            MeshRange {
                first_index: m.ice.first_index,
                num_indices: 0
            }
        );
        // The ranges follow each other: fog, coast, land.
        assert_eq!(m.coast.first_index, m.fog.first_index + m.fog.num_indices);
        assert_eq!(
            m.land.first_index,
            m.coast.first_index + m.coast.num_indices
        );
        assert!(m.indices.iter().all(|&i| (i as usize) < m.vertices.len()));
    }

    #[test]
    fn what_is_off_the_map_inside_the_view_is_fog() {
        let snap = snapshot(2, 2, |_, _| cell(TerrainKind::Grass));
        let map = MinimapMap::from_snapshot(&snap);
        let m = build_meshes(&map, &whole(2, 2), 1.0);
        assert!(hexes(m.fog) > 0, "the margin is fog");
        let only_the_map = ViewRect {
            min: Vec2::new(0.0, 0.0),
            max: Vec2::new(10.0, 10.0),
        };
        // A tiny view still makes its cell and a cell of margin round it.
        let small = build_meshes(&map, &only_the_map, 1.0);
        assert!(hexes(small.land) >= 1);
    }

    #[test]
    fn culture_has_a_range_per_owner() {
        let snap = snapshot(4, 1, |_, _| cell(TerrainKind::Grass));
        let mut map = MinimapMap::from_snapshot(&snap);
        map.set_owners(&OwnersInput::new(4, 1, vec![2, 2, 0, -1], vec![[1, 1, 1]; 3]).unwrap());
        let m = build_meshes(&map, &whole(4, 1), 1.0);
        let owners: Vec<(u8, u32)> = m
            .culture
            .iter()
            .map(|c| (c.owner, hexes(c.range)))
            .collect();
        // Met column by column: owner 2 first.
        assert_eq!(owners, vec![(2, 2), (0, 1)]);
    }

    #[test]
    fn ice_rivers_and_mountains_come_from_the_view() {
        let snap = snapshot(3, 3, |x, y| match (x, y) {
            (0, 0) => TerrainCell {
                feat: FeatureKind::Ice,
                ..cell(TerrainKind::Snow)
            },
            (1, 1) => TerrainCell {
                relief: Relief::Mountain,
                ..cell(TerrainKind::Grass)
            },
            (2, 2) => TerrainCell {
                rivers: 1 << Dir::East.index(),
                ..cell(TerrainKind::Grass)
            },
            _ => cell(TerrainKind::Grass),
        });
        let map = MinimapMap::from_snapshot(&snap);
        let m = build_meshes(&map, &whole(3, 3), 1.0);
        assert_eq!(hexes(m.ice), 1);
        assert_eq!(m.mountains.len(), 4);
        // One river edge: one quad of two triangles.
        assert_eq!(m.rivers.num_indices, 6);
    }

    #[test]
    fn an_edge_with_a_river_on_both_sides_is_drawn_once() {
        // (0, 0) and (1, 0) share the east edge of (0, 0).
        let snap = snapshot(2, 1, |x, _| TerrainCell {
            rivers: if x == 0 {
                1 << Dir::East.index()
            } else {
                1 << Dir::West.index()
            },
            ..cell(TerrainKind::Grass)
        });
        let m = build_meshes(&MinimapMap::from_snapshot(&snap), &whole(2, 1), 1.0);
        assert_eq!(m.rivers.num_indices, 6);
    }

    #[test]
    fn a_river_is_at_least_a_pixel_and_a_half_wide() {
        let snap = snapshot(2, 1, |x, _| TerrainCell {
            rivers: if x == 0 { 1 << Dir::East.index() } else { 0 },
            ..cell(TerrainKind::Grass)
        });
        let map = MinimapMap::from_snapshot(&snap);
        let wide = |ppu: f32| {
            let m = build_meshes(&map, &whole(2, 1), ppu);
            let r = m.rivers.indices();
            let vs: Vec<Vec2> = r
                .step_by(6)
                .flat_map(|i| (0..4).map(move |k| (i, k)))
                .map(|(i, k)| {
                    let v = m.vertices[m.indices[i as usize + [0, 1, 2, 2][k]] as usize];
                    Vec2::new(v[0], v[1])
                })
                .collect();
            (vs[0] - vs[3]).length()
        };
        assert!(wide(0.01) > wide(10.0), "a small target needs a wider band");
        assert!(wide(0.01) >= 1.5 / 0.01 - 1e-2);
    }

    #[test]
    fn quads_have_the_indices_of_two_triangles() {
        assert_eq!(quad_indices(2), vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7]);
    }

    #[test]
    fn the_cells_of_a_view_cover_it() {
        let v = ViewRect {
            min: Vec2::new(100.0, 100.0),
            max: Vec2::new(400.0, 300.0),
        };
        let cells: Vec<_> = cells_in_view(&v).collect();
        // Every point of the view is within a cell of the list.
        for px in [100.0, 250.0, 400.0] {
            for py in [100.0, 200.0, 300.0] {
                let near = cells
                    .iter()
                    .any(|&(x, y)| (cell_pos(x, y) - Vec2::new(px, py)).length() <= 40.0);
                assert!(near, "({px}, {py})");
            }
        }
    }
}
