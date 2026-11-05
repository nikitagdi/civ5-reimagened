//! Test scene: hex grid and a lit icosphere. Deterministic (own hash, no `rand`), so web and native match.

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Quat, Vec3};

use crate::frame::CLEAR_GREY;

/// Layout: position (0), normal (12), color (24); stride 36.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct SceneVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub color: [f32; 3],
}

/// CPU triangle mesh. Indices are counter-clockwise seen from outside.
#[derive(Debug, Clone, Default)]
pub struct Mesh {
    pub vertices: Vec<SceneVertex>,
    pub indices: Vec<u32>,
}

impl Mesh {
    fn push(&mut self, position: Vec3, normal: Vec3, color: Vec3) -> u32 {
        let i = self.vertices.len() as u32;
        self.vertices.push(SceneVertex {
            position: position.to_array(),
            normal: normal.to_array(),
            color: color.to_array(),
        });
        i
    }
}

/// Per-frame parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scene {
    pub model_transform: Mat4,
    /// Toward the light; not unit-length.
    pub light_dir: Vec3,
    /// Clear colour of the main pass, in gamma space (the world is not sRGB).
    pub clear_color: [f64; 3],
}

impl Scene {
    /// Test scene at `time_s`.
    pub fn test(time_s: f32) -> Self {
        let rotation = Quat::from_rotation_y(time_s * 0.6) * Quat::from_rotation_x(0.4);
        let lift = 4.0 + (time_s * 1.3).sin() * 0.4;
        Self {
            model_transform: Mat4::from_scale_rotation_translation(
                Vec3::splat(1.6),
                rotation,
                Vec3::new(0.0, lift, 0.0),
            ),
            light_dir: Vec3::new(0.5, 1.0, 0.3),
            clear_color: [CLEAR_GREY; 3],
        }
    }
}

pub const HEX_RADIUS: f32 = 1.0;
/// Water tiles are flattened to this height.
const SEA_LEVEL: f32 = 0.3;

/// Center of `(col, row)` in pointy-top layout; odd rows are shifted.
pub fn hex_center(col: u32, row: u32) -> (f32, f32) {
    let w = 3f32.sqrt() * HEX_RADIUS;
    let x = col as f32 * w + if row % 2 == 1 { w * 0.5 } else { 0.0 };
    let z = row as f32 * 1.5 * HEX_RADIUS;
    (x, z)
}

/// splitmix64 hash mapped to `[0, 1)`.
pub fn hash01(col: u32, row: u32, seed: u64) -> f32 {
    let mut x = seed ^ ((col as u64) << 32 | row as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    (x >> 40) as f32 / (1u64 << 24) as f32
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Terrain {
    Water,
    Coast,
    Plains,
    Hills,
    Mountains,
}

/// Height in `0..~2.5`: waves plus hash noise.
pub fn tile_height(col: u32, row: u32, seed: u64) -> f32 {
    let (x, z) = hex_center(col, row);
    let waves = (x * 0.35).sin() * 0.6 + (z * 0.45 + 1.3).cos() * 0.5 + (x * 0.13 + z * 0.21).sin();
    let noise = hash01(col, row, seed) - 0.5;
    (0.9 + waves * 0.7 + noise * 0.6).clamp(0.0, 2.5)
}

pub fn terrain_for(height: f32) -> Terrain {
    match height {
        h if h < 0.45 => Terrain::Water,
        h if h < 0.7 => Terrain::Coast,
        h if h < 1.4 => Terrain::Plains,
        h if h < 1.9 => Terrain::Hills,
        _ => Terrain::Mountains,
    }
}

pub fn terrain_color(t: Terrain, jitter: f32) -> Vec3 {
    let base = match t {
        Terrain::Water => Vec3::new(0.02, 0.10, 0.35),
        Terrain::Coast => Vec3::new(0.55, 0.50, 0.25),
        Terrain::Plains => Vec3::new(0.12, 0.35, 0.06),
        Terrain::Hills => Vec3::new(0.25, 0.22, 0.08),
        Terrain::Mountains => Vec3::new(0.55, 0.55, 0.60),
    };
    base * (0.85 + jitter * 0.3)
}

/// `cols × rows` grid centered on the origin. Tiles are prisms from `y = 0`; water sits at sea level.
pub fn hex_grid(cols: u32, rows: u32, seed: u64) -> Mesh {
    let mut mesh = Mesh::default();
    let (max_x, max_z) = hex_center(cols.saturating_sub(1), rows.saturating_sub(1));
    let offset = Vec3::new(max_x * 0.5, 0.0, max_z * 0.5);
    // Pointy-top corners, slightly inside the radius so tiles show seams.
    let corners: [Vec3; 6] = std::array::from_fn(|i| {
        let a = (60.0 * i as f32 + 30.0).to_radians();
        Vec3::new(a.cos(), 0.0, a.sin()) * (HEX_RADIUS * 0.96)
    });

    for row in 0..rows {
        for col in 0..cols {
            let (cx, cz) = hex_center(col, row);
            let raw = tile_height(col, row, seed);
            let terrain = terrain_for(raw);
            let height = if terrain == Terrain::Water {
                SEA_LEVEL
            } else {
                raw
            };
            let color = terrain_color(terrain, hash01(col, row, seed ^ 0xC0FFEE));
            let center = Vec3::new(cx, height, cz) - offset;

            let c = mesh.push(center, Vec3::Y, color);
            let top: [u32; 6] =
                std::array::from_fn(|i| mesh.push(center + corners[i], Vec3::Y, color));
            for i in 0..6 {
                let j = (i + 1) % 6;
                // Clockwise from above, so the front-facing (CCW) order is c, j, i.
                mesh.indices.extend_from_slice(&[c, top[j], top[i]]);
            }

            let side_color = color * 0.7;
            for i in 0..6 {
                let j = (i + 1) % 6;
                let a = center + corners[i];
                let b = center + corners[j];
                let normal = ((corners[i] + corners[j]) * 0.5).normalize_or_zero();
                let a0 = Vec3::new(a.x, 0.0, a.z);
                let b0 = Vec3::new(b.x, 0.0, b.z);
                let v0 = mesh.push(a, normal, side_color);
                let v1 = mesh.push(b, normal, side_color);
                let v2 = mesh.push(b0, normal, side_color);
                let v3 = mesh.push(a0, normal, side_color);
                mesh.indices.extend_from_slice(&[v0, v1, v2, v0, v2, v3]);
            }
        }
    }
    mesh
}

/// Top-center of the tile closest to the grid center (the origin), in world space.
pub fn center_tile_top(cols: u32, rows: u32, seed: u64) -> Vec3 {
    let (max_x, max_z) = hex_center(cols.saturating_sub(1), rows.saturating_sub(1));
    let offset = Vec3::new(max_x * 0.5, 0.0, max_z * 0.5);
    let mut best = (f32::MAX, Vec3::ZERO);
    for row in 0..rows {
        for col in 0..cols {
            let (cx, cz) = hex_center(col, row);
            let raw = tile_height(col, row, seed);
            // Water tiles are drawn at sea level.
            let height = if terrain_for(raw) == Terrain::Water {
                SEA_LEVEL
            } else {
                raw
            };
            let top = Vec3::new(cx, height, cz) - offset;
            let d = top.x * top.x + top.z * top.z;
            if d < best.0 {
                best = (d, top);
            }
        }
    }
    best.1
}

/// Unit icosphere with `subdivisions` splits and smooth normals.
pub fn icosphere(subdivisions: u32, color: Vec3) -> Mesh {
    let t = (1.0 + 5f32.sqrt()) / 2.0;
    let mut positions: Vec<Vec3> = [
        (-1.0, t, 0.0),
        (1.0, t, 0.0),
        (-1.0, -t, 0.0),
        (1.0, -t, 0.0),
        (0.0, -1.0, t),
        (0.0, 1.0, t),
        (0.0, -1.0, -t),
        (0.0, 1.0, -t),
        (t, 0.0, -1.0),
        (t, 0.0, 1.0),
        (-t, 0.0, -1.0),
        (-t, 0.0, 1.0),
    ]
    .iter()
    .map(|&(x, y, z)| Vec3::new(x, y, z).normalize())
    .collect();
    let mut faces: Vec<[u32; 3]> = vec![
        [0, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];
    for _ in 0..subdivisions {
        let mut cache = std::collections::HashMap::new();
        let mut midpoint = |a: u32, b: u32, positions: &mut Vec<Vec3>| -> u32 {
            let key = (a.min(b), a.max(b));
            *cache.entry(key).or_insert_with(|| {
                let p = ((positions[a as usize] + positions[b as usize]) * 0.5).normalize();
                positions.push(p);
                positions.len() as u32 - 1
            })
        };
        let mut next = Vec::with_capacity(faces.len() * 4);
        for [a, b, c] in faces {
            let ab = midpoint(a, b, &mut positions);
            let bc = midpoint(b, c, &mut positions);
            let ca = midpoint(c, a, &mut positions);
            next.extend_from_slice(&[[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]]);
        }
        faces = next;
    }
    let mut mesh = Mesh::default();
    for p in positions {
        mesh.push(p, p, color);
    }
    mesh.indices = faces.into_iter().flatten().collect();
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertex_layout() {
        assert_eq!(std::mem::size_of::<SceneVertex>(), 36);
    }

    #[test]
    fn hash_is_deterministic_and_in_range() {
        for c in 0..20 {
            for r in 0..12 {
                let h = hash01(c, r, 7);
                assert_eq!(h, hash01(c, r, 7));
                assert!((0.0..1.0).contains(&h));
            }
        }
        assert_ne!(hash01(1, 2, 7), hash01(2, 1, 7));
    }

    #[test]
    fn grid_has_expected_size_and_valid_indices() {
        let m = hex_grid(20, 12, 42);
        assert_eq!(m.vertices.len(), 20 * 12 * (7 + 24));
        assert_eq!(m.indices.len(), 20 * 12 * (18 + 36));
        assert!(m.indices.iter().all(|&i| (i as usize) < m.vertices.len()));
    }

    #[test]
    fn grid_has_several_terrains() {
        let mut seen = std::collections::HashSet::new();
        for c in 0..20 {
            for r in 0..12 {
                seen.insert(format!("{:?}", terrain_for(tile_height(c, r, 42))));
            }
        }
        assert!(seen.len() >= 3, "too few distinct terrain types: {seen:?}");
    }

    #[test]
    fn center_tile_is_near_origin_and_on_the_surface() {
        let top = center_tile_top(20, 12, 42);
        assert!(top.x.hypot(top.z) <= HEX_RADIUS * 1.5, "{top:?}");
        assert!(top.y >= SEA_LEVEL - 1e-6 && top.y <= 2.5);
        // Matches a vertex of the generated grid.
        let m = hex_grid(20, 12, 42);
        assert!(
            m.vertices
                .iter()
                .any(|v| { (Vec3::from(v.position) - top).length() < 1e-5 })
        );
    }

    #[test]
    fn top_faces_point_up() {
        let m = hex_grid(1, 1, 1);
        let p = |i: usize| Vec3::from(m.vertices[m.indices[i] as usize].position);
        let n = (p(1) - p(0)).cross(p(2) - p(0));
        assert!(n.y > 0.0);
    }

    #[test]
    fn icosphere_is_unit_and_outward() {
        let m = icosphere(2, Vec3::ONE);
        assert_eq!(m.indices.len(), 20 * 16 * 3);
        for v in &m.vertices {
            assert!((Vec3::from(v.position).length() - 1.0).abs() < 1e-5);
        }
        let p = |i: usize| Vec3::from(m.vertices[m.indices[i] as usize].position);
        let n = (p(1) - p(0)).cross(p(2) - p(0));
        assert!(n.dot(p(0)) > 0.0, "faces must point outward");
    }
}
