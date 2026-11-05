//! Cell under the cursor: the camera ray against the real relief
//! triangles.
//!
//! Broad phase: the part of the ray between the mountain peak height and the water level is
//! projected onto the ground and walked cell by cell, nearest first; cells are convex and do not
//! overlap in the ground plane, so the first cell with a hit holds the nearest hit. At most
//! [`MAX_CELLS`] cells are looked at. The walk runs in unwrapped world coordinates; a cell outside
//! `[0, width)` on a wrapping map is tested with the ray shifted by whole map widths, so the X seam
//! needs no extra copies of the mesh.

use glam::{Vec2, Vec3};

use crate::game_camera::Ray;
use crate::hexgrid::{GridShape, HEX_WIDTH, nearest_cell};
use crate::terrain::{MOUNTAIN_TOP, TerrainMesh, WATER_HEIGHT};

/// Most cells one pick looks at along the ray.
pub const MAX_CELLS: usize = 12;
/// Spacing of ground samples along the ray, world units. Well below the hex inradius (32), so no
/// cell the ray crosses for more than a sliver is skipped.
const STEP: f32 = 4.0;

/// A pick result: the cell and the world point where the ray met its surface (in the copy of the
/// map the ray went through, so on a wrapping map `point.x` may lie outside the map).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pick {
    pub cell: (i32, i32),
    pub point: Vec3,
}

/// Triangles of the terrain mesh grouped by cell. Built once per mesh.
#[derive(Debug, Clone)]
pub struct PickIndex {
    shape: GridShape,
    /// `starts[c]..starts[c + 1]` is the range of cell `c` in `tris`.
    starts: Vec<u32>,
    tris: Vec<[Vec3; 3]>,
}

impl PickIndex {
    /// Land, skirts and water of every cell. A triangle belongs to the cell of its first vertex.
    pub fn new(shape: GridShape, mesh: &TerrainMesh) -> Self {
        let cells = shape.width.max(0) as usize * shape.height.max(0) as usize;
        let owner = |t: &[u32]| mesh.vertices[t[0] as usize].cell as usize;
        let all = || mesh.land.chunks_exact(3).chain(mesh.water.chunks_exact(3));
        let mut starts = vec![0u32; cells + 1];
        for t in all() {
            starts[owner(t) + 1] += 1;
        }
        for c in 0..cells {
            starts[c + 1] += starts[c];
        }
        let mut next = starts.clone();
        let mut tris = vec![[Vec3::ZERO; 3]; starts[cells] as usize];
        for t in all() {
            let c = owner(t);
            tris[next[c] as usize] =
                [0, 1, 2].map(|k| Vec3::from(mesh.vertices[t[k] as usize].position));
            next[c] += 1;
        }
        Self {
            shape,
            starts,
            tris,
        }
    }

    pub fn shape(&self) -> GridShape {
        self.shape
    }

    /// Cell `(x, y)` (inside the map) hit by `ray`, with the nearest hit distance.
    fn hit_cell(&self, x: i32, y: i32, ray: &Ray) -> Option<f32> {
        let c = y as usize * self.shape.width as usize + x as usize;
        let range = self.starts[c] as usize..self.starts[c + 1] as usize;
        self.tris[range]
            .iter()
            .filter_map(|t| ray_triangle(ray, t))
            .min_by(f32::total_cmp)
    }

    /// The cell under `ray` and the hit point, or `None` (ray misses the map, points up, ...).
    pub fn pick(&self, ray: &Ray) -> Option<Pick> {
        let (o, d) = (ray.origin, ray.dir);
        if d.y > -1e-6 {
            return None;
        }
        let t0 = ((MOUNTAIN_TOP - o.y) / d.y).max(0.0);
        let t1 = (WATER_HEIGHT - o.y) / d.y;
        if t1 < t0 {
            return None;
        }
        let ground_len = (t1 - t0) * Vec2::new(d.x, d.z).length();
        let n = (ground_len / STEP).ceil().max(1.0) as usize;

        let mut last = None;
        let mut seen = 0;
        for i in 0..=n {
            let t = t0 + (t1 - t0) * i as f32 / n as f32;
            let p = o + d * t;
            let (ux, uy) = nearest_cell(Vec2::new(p.x, p.z));
            if last == Some((ux, uy)) {
                continue;
            }
            last = Some((ux, uy));
            seen += 1;
            if seen > MAX_CELLS {
                break;
            }
            let Some((x, y)) = self.shape.normalize(ux, uy) else {
                continue;
            };
            // The cell's mesh lives at its home position; move the ray there.
            let shift = HEX_WIDTH * (ux - x) as f32;
            let shifted = Ray {
                origin: o - Vec3::new(shift, 0.0, 0.0),
                dir: d,
            };
            if let Some(t) = self.hit_cell(x, y, &shifted) {
                return Some(Pick {
                    cell: (x, y),
                    point: o + d * t,
                });
            }
        }
        None
    }
}

/// Moeller-Trumbore, both faces; distance along the (unit) ray.
fn ray_triangle(ray: &Ray, tri: &[Vec3; 3]) -> Option<f32> {
    const EPS: f32 = 1e-7;
    let e1 = tri[1] - tri[0];
    let e2 = tri[2] - tri[0];
    let p = ray.dir.cross(e2);
    let det = e1.dot(p);
    if det.abs() < EPS {
        return None;
    }
    let inv = 1.0 / det;
    let s = ray.origin - tri[0];
    let u = s.dot(p) * inv;
    if !(-1e-5..=1.0 + 1e-5).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = ray.dir.dot(q) * inv;
    if v < -1e-5 || u + v > 1.0 + 1e-5 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t >= 0.0).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_camera::GameCamera;
    use crate::hexgrid::{CORNER_RADIUS, cell_center};
    use crate::terrain::test_util::{land, map};
    use crate::terrain::{HILL_TOP, LAND_HEIGHT, Relief, TerrainCell, TerrainSnapshot};

    const ASPECT: f32 = 16.0 / 9.0;

    fn relief(r: Relief) -> TerrainCell {
        TerrainCell {
            relief: r,
            ..land()
        }
    }

    /// 8x6, hill (2,2), mountain (5,2), water (1,4).
    fn world(wrap: bool) -> (TerrainSnapshot, PickIndex) {
        let cell = |x: i32, y: i32| match (x, y) {
            (2, 2) => relief(Relief::Hill),
            (5, 2) => relief(Relief::Mountain),
            (1, 4) => relief(Relief::Water),
            _ => land(),
        };
        let mut s = map(8, 6, cell);
        if wrap {
            s = TerrainSnapshot::new(8, 6, true, s.cells().to_vec()).unwrap();
        }
        let idx = PickIndex::new(s.shape(), &TerrainMesh::build(&s));
        (s, idx)
    }

    fn top(x: i32, y: i32, h: f32) -> Vec3 {
        let c = cell_center(x, y);
        Vec3::new(c.x, h, c.y)
    }

    /// Ray of a game camera through the world point `target`.
    fn camera_ray(cam: &GameCamera, target: Vec3) -> Ray {
        let clip = cam.view_proj(ASPECT) * target.extend(1.0);
        cam.unproject(Vec2::new(clip.x, clip.y) / clip.w, ASPECT)
    }

    /// Ray with a given elevation angle coming from `dir_xz` (unit, horizontal) towards `target`.
    fn low_ray(target: Vec3, from_xz: Vec2, elevation_deg: f32) -> Ray {
        let e = elevation_deg.to_radians();
        let back = Vec3::new(from_xz.x * e.cos(), e.sin(), from_xz.y * e.cos());
        Ray {
            origin: target + back * 800.0,
            dir: -back,
        }
    }

    /// Five camera set-ups: zoom level and focus offset.
    fn cameras(around: Vec2) -> Vec<GameCamera> {
        [
            (11.0, Vec2::ZERO),
            (14.0, Vec2::new(40.0, 60.0)),
            (17.0, Vec2::new(-80.0, -30.0)),
            (20.5, Vec2::new(10.0, -120.0)),
            (14.0, Vec2::new(-30.0, 90.0)),
        ]
        .map(|(level, off)| {
            let mut c = GameCamera::new(around + off);
            c.set_level(level);
            c
        })
        .to_vec()
    }

    #[test]
    fn centres_of_plain_hill_mountain_from_five_cameras() {
        let (_, idx) = world(false);
        for (cell, h) in [
            ((3, 3), LAND_HEIGHT),
            ((2, 2), HILL_TOP),
            ((5, 2), MOUNTAIN_TOP),
        ] {
            let target = top(cell.0, cell.1, h);
            for cam in cameras(Vec2::new(target.x, target.z)) {
                let hit = idx.pick(&camera_ray(&cam, target)).expect("hit");
                assert_eq!(hit.cell, cell, "camera focus {:?}", cam.focus());
                assert!((hit.point - target).length() < 0.5, "{:?}", hit.point);
            }
        }
    }

    #[test]
    fn hilltop_wins_over_the_neighbour_the_ground_plane_hits() {
        let (s, idx) = world(false);
        let target = top(2, 2, HILL_TOP);
        // Shallow ray from the south: the ground plane is hit 48 units behind the hill, in the next row.
        let ray = low_ray(target, Vec2::new(0.0, 1.0), 15.0);
        let naive = ray.at_height(LAND_HEIGHT).unwrap();
        assert_ne!(
            s.shape().world_to_cell(Vec2::new(naive.x, naive.z)),
            Some((2, 2))
        );
        assert_eq!(idx.pick(&ray).map(|p| p.cell), Some((2, 2)));
    }

    #[test]
    fn mountain_peak_is_not_the_cell_behind_it() {
        let (s, idx) = world(false);
        let target = top(5, 2, MOUNTAIN_TOP);
        // Shallow rays from the south: the ground-plane hit is 85+ units behind the peak.
        for elev in [15.0, 25.0] {
            let ray = low_ray(target, Vec2::new(0.0, 1.0), elev);
            let naive = ray.at_height(LAND_HEIGHT).unwrap();
            let behind = s.shape().world_to_cell(Vec2::new(naive.x, naive.z));
            assert_ne!(behind, Some((5, 2)), "plane hit lands in a cell behind");
            assert_eq!(idx.pick(&ray).map(|p| p.cell), Some((5, 2)), "elev {elev}");
        }
    }

    #[test]
    fn slope_in_front_of_the_mountain_picks_the_mountain() {
        let (_, idx) = world(false);
        // Towards the camera (+z) the profile is linear from the peak to the rim at the corner.
        for frac in [0.45, 0.7, 0.9] {
            let d = CORNER_RADIUS * frac;
            let h = LAND_HEIGHT + (MOUNTAIN_TOP - LAND_HEIGHT) * (1.0 - frac);
            let target = top(5, 2, h) + Vec3::new(0.0, 0.0, d);
            for cam in cameras(Vec2::new(target.x, target.z)) {
                assert_eq!(
                    idx.pick(&camera_ray(&cam, target)).map(|p| p.cell),
                    Some((5, 2))
                );
            }
            let low = low_ray(target, Vec2::new(0.0, 1.0), 25.0);
            assert_eq!(idx.pick(&low).map(|p| p.cell), Some((5, 2)));
        }
    }

    #[test]
    fn foot_of_the_mountain_picks_the_flat_cell_in_front() {
        let (_, idx) = world(false);
        // Flat cell south of the mountain row: its centre, seen over the mountain from the front.
        let target = top(5, 3, LAND_HEIGHT);
        for cam in cameras(Vec2::new(target.x, target.z)) {
            assert_eq!(
                idx.pick(&camera_ray(&cam, target)).map(|p| p.cell),
                Some((5, 3))
            );
        }
    }

    #[test]
    fn water_cell_and_land_behind_it() {
        let (_, idx) = world(false);
        let target = top(1, 4, WATER_HEIGHT);
        for cam in cameras(Vec2::new(target.x, target.z)) {
            let hit = idx.pick(&camera_ray(&cam, target)).expect("hit");
            assert_eq!(hit.cell, (1, 4));
            assert!((hit.point.y - WATER_HEIGHT).abs() < 0.01);
        }
    }

    #[test]
    fn off_map_and_upward_rays_give_none() {
        let (_, idx) = world(false);
        // West of the map, north of the map, and a ray that points up.
        for target in [
            Vec3::new(-300.0, 12.0, -100.0),
            Vec3::new(100.0, 12.0, -900.0),
        ] {
            let cam = GameCamera::new(Vec2::new(target.x, target.z));
            assert_eq!(idx.pick(&camera_ray(&cam, target)), None);
        }
        let up = Ray {
            origin: top(3, 3, 300.0),
            dir: Vec3::Y,
        };
        assert_eq!(idx.pick(&up), None);
        let below = Ray {
            origin: top(3, 3, 5.0),
            dir: Vec3::new(0.0, -1.0, 0.0),
        };
        assert_eq!(idx.pick(&below), None);
    }

    #[test]
    fn wrap_seam_both_ways() {
        let (s, idx) = world(true);
        let width = s.shape().world_width();
        // Hill (2,2) seen through the copies shifted by one map width either way.
        let hill = top(2, 2, HILL_TOP);
        for shift in [-width, width, 2.0 * width] {
            let target = hill + Vec3::new(shift, 0.0, 0.0);
            for cam in cameras(Vec2::new(target.x, target.z)) {
                assert_eq!(
                    idx.pick(&camera_ray(&cam, target)).map(|p| p.cell),
                    Some((2, 2))
                );
            }
        }
        // Unwrapped x = -1 is cell 7; a plain cell next to the seam.
        let target = top(-1, 3, LAND_HEIGHT);
        let cam = GameCamera::new(Vec2::new(target.x, target.z));
        assert_eq!(
            idx.pick(&camera_ray(&cam, target)).map(|p| p.cell),
            Some((7, 3))
        );
        // Without wrapping the same point is off the map.
        let (_, flat) = world(false);
        assert_eq!(flat.pick(&camera_ray(&cam, target)), None);
    }

    #[test]
    fn wrap_seam_ray_crosses_into_a_mountain() {
        // Mountain at the seam (0,2) on a wrapping map; the ray comes from the west, over cell 7.
        let cell = |x: i32, y: i32| {
            if (x, y) == (0, 2) {
                relief(Relief::Mountain)
            } else {
                land()
            }
        };
        let s = map(8, 6, cell);
        let s = TerrainSnapshot::new(8, 6, true, s.cells().to_vec()).unwrap();
        let idx = PickIndex::new(s.shape(), &TerrainMesh::build(&s));
        let peak = top(0, 2, MOUNTAIN_TOP);
        for elev in [20.0, 30.0, 45.0] {
            let ray = low_ray(peak, Vec2::new(-1.0, 0.0), elev);
            assert_eq!(idx.pick(&ray).map(|p| p.cell), Some((0, 2)), "elev {elev}");
        }
        // And from the east over cell 1 onto the mountain's west copy at x = 8.
        let far = peak + Vec3::new(s.shape().world_width(), 0.0, 0.0);
        let ray = low_ray(far, Vec2::new(1.0, 0.0), 25.0);
        assert_eq!(idx.pick(&ray).map(|p| p.cell), Some((0, 2)));
        let ray = low_ray(far, Vec2::new(-1.0, 0.0), 25.0);
        assert_eq!(idx.pick(&ray).map(|p| p.cell), Some((0, 2)));
    }

    #[test]
    fn big_map_picks_fast_enough_and_correct() {
        let s = map(128, 80, |x, y| {
            if (x + y) % 5 == 0 {
                relief(Relief::Hill)
            } else {
                land()
            }
        });
        let idx = PickIndex::new(s.shape(), &TerrainMesh::build(&s));
        let target = top(100, 40, HILL_TOP);
        let cam = GameCamera::new(Vec2::new(target.x, target.z));
        let started = std::time::Instant::now();
        for _ in 0..1000 {
            assert_eq!(
                idx.pick(&camera_ray(&cam, target)).map(|p| p.cell),
                Some((100, 40))
            );
        }
        assert!(
            started.elapsed().as_millis() < 2000,
            "{:?}",
            started.elapsed()
        );
    }
}
