//! The hex under a ray: the port of `Terrain::IntersectRay` on the
//! field. The ray walks over the texels from where it comes under the highest point the field can hold
//! to where it reaches the ground; the two triangles of each quad of texels it crosses are tested, the
//! nearest hit wins. The answer is a [`Pick`] as in B1: the hex whose centre is nearest to the point, and
//! the point. A ray that hits nothing of the field meets the water plane (the behaviour of B1; what the
//! original does there is for the hand to check).
//!
//! Cells the caller says are not pickable (statistics 1 and 2: no ground, or ground under the sea
//! floor line) are not looked at. A wrapping map is walked in the unwrapped texels and the sampler wraps
//! them, so a ray through the seam needs no copy of the map.

use glam::{Vec2, Vec3};

use super::field::CELL_SIZE;
use super::sampler::{HEIGHT_TO_UNIT, TerrainSampler};
use crate::game_camera::Ray;
use crate::hexgrid::nearest_cell;
use crate::picking::Pick;

/// The level of the water that a ray hits when it hits no ground (B1's `WATER_HEIGHT`).
pub const WATER_PLANE: f32 = 10.12;

/// The largest height a texel can hold: raw 65535.
fn max_height(sampler: &TerrainSampler) -> f32 {
    f32::from(u16::MAX) * HEIGHT_TO_UNIT * sampler.frame().terrain_scale().z
}

/// Distance along the ray to a triangle (Moeller-Trumbore, both faces), 0 or more.
fn ray_triangle(o: Vec3, d: Vec3, tri: &[Vec3; 3]) -> Option<f32> {
    const EPS: f32 = 1e-9;
    let e1 = tri[1] - tri[0];
    let e2 = tri[2] - tri[0];
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < EPS {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - tri[0];
    let u = s.dot(p) * inv;
    if !(-1e-5..=1.0 + 1e-5).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < -1e-5 || u + v > 1.0 + 1e-5 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t >= 0.0).then_some(t)
}

/// The height a ray meets at texel `(u, v)`: the ground, but not under the sea. The water is what the eye sees
/// there and where a boat or an embarked unit stands, so a click on it picks the hex it is on; with the sea
/// floor a ray through a boat went on down to the floor and picked a hex further up the screen (embarked
/// workers could not be clicked, live report 2026-10-30).
fn surface(sampler: &TerrainSampler, u: i32, v: i32) -> f32 {
    sampler.texel_height(u, v).max(sampler.sea_level())
}

/// The two triangles of the quad whose corner is the texel `(u, v)`, in our world.
fn quad(sampler: &TerrainSampler, u: i32, v: i32) -> [[Vec3; 3]; 2] {
    let frame = sampler.frame();
    let corner = |du: i32, dv: i32| {
        let (cu, cv) = (u + du, v + dv);
        let p = frame.tex_to_world(Vec2::new(cu as f32, cv as f32));
        Vec3::new(p.x, surface(sampler, cu, cv), p.y)
    };
    let (a, b, c, d) = (corner(0, 0), corner(1, 0), corner(0, 1), corner(1, 1));
    [[a, b, c], [b, d, c]]
}

/// The hex of a point of our world on the map of the sampler, wrapped; `None` off the map.
fn hex_of(sampler: &TerrainSampler, p: Vec3) -> Option<(i32, i32)> {
    let (x, y) = nearest_cell(Vec2::new(p.x, p.z));
    sampler.frame().grid_shape().normalize(x, y)
}

/// The hex and the point under `ray`. `pickable(cx, cy)` says whether the cell with this index may be
/// hit (`false` for statistics 1 and 2, and for a cell not built).
// re: Terrain::IntersectRay (Terrain.cpp:1245-1350: the cells by sphere, then two triangles a quad)
pub fn pick_ray(
    sampler: &TerrainSampler,
    ray: &Ray,
    pickable: &dyn Fn(i32, i32) -> bool,
) -> Option<Pick> {
    let (o, d) = (ray.origin, ray.dir.normalize_or_zero());
    if d.y > -1e-6 {
        return None;
    }
    let frame = sampler.frame();
    let scale = frame.terrain_scale();
    let (tex_w, tex_h) = frame.texel_size();
    let top = max_height(sampler);
    let t0 = ((top - o.y) / d.y).max(0.0);
    let t1 = (0.0 - o.y) / d.y;
    if t1 >= t0 {
        // The length of the ray's shadow on the texel plane, in texels, and the step of the walk.
        let along = Vec2::new(d.x / scale.x, d.z / scale.y).length();
        let texels = (t1 - t0) * along;
        let steps = (texels * 2.0).ceil().clamp(1.0, 65_536.0) as usize;
        let horizontal = Vec2::new(d.x, d.z).length().max(1e-6);
        // How much the ray falls over one texel of the field, to leave out the quads it passes
        // high above.
        let drop_per_texel = d.y.abs() / horizontal * scale.x.max(scale.y);
        let mut last: Option<(i32, i32)> = None;
        for i in 0..=steps {
            let t = t0 + (t1 - t0) * i as f32 / steps as f32;
            let p = o + d * t;
            let tex = frame.world_to_tex(Vec2::new(p.x, p.z));
            let (u, v) = (tex.x.floor() as i32, tex.y.floor() as i32);
            if last == Some((u, v)) {
                continue;
            }
            last = Some((u, v));
            if v < 0 || v >= tex_h as i32 {
                continue;
            }
            if !frame.wrap_x() && (u < 0 || u >= tex_w as i32) {
                continue;
            }
            let wu = if frame.wrap_x() {
                u.rem_euclid(tex_w as i32)
            } else {
                u
            };
            let cell = (wu / CELL_SIZE as i32, v / CELL_SIZE as i32);
            if !pickable(cell.0, cell.1) {
                continue;
            }
            // Above all four corners and falling slower than the walk goes: no hit here.
            let top_of_quad = [(0, 0), (1, 0), (0, 1), (1, 1)]
                .iter()
                .map(|&(du, dv)| surface(sampler, u + du, v + dv))
                .fold(0.0f32, f32::max);
            if p.y - drop_per_texel * 1.5 > top_of_quad {
                continue;
            }
            let hit = quad(sampler, u, v)
                .iter()
                .filter_map(|tri| ray_triangle(o, d, tri))
                .min_by(f32::total_cmp);
            if let Some(t) = hit {
                let point = o + d * t;
                return hex_of(sampler, point).map(|cell| Pick { cell, point });
            }
        }
    }
    // No ground: the plane of the water.
    let t = (WATER_PLANE - o.y) / d.y;
    if t < 0.0 {
        return None;
    }
    let point = o + d * t;
    hex_of(sampler, point).map(|cell| Pick { cell, point })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::game_camera::GameCamera;
    use crate::hexgrid::{CORNER_RADIUS, cell_center};
    use crate::terrain::{HILL_TOP, LAND_HEIGHT, MOUNTAIN_TOP, Relief, TerrainKind};
    use crate::world::terrain::Terrain;
    use crate::world::terrain::basis::{PlotBasis, TerrainBasis};
    use crate::world::terrain::library::TerrainLibrary;

    const ASPECT: f32 = 16.0 / 9.0;

    fn plot(relief: Relief, terrain: TerrainKind) -> PlotBasis {
        PlotBasis {
            relief,
            terrain,
            river_flow: PlotBasis::NO_FLOW,
            ..PlotBasis::default()
        }
    }

    /// 8 x 6: a hill at (2, 2), a mountain at (5, 2), water at (1, 4), grass elsewhere; the same
    /// world as the pick tests of B1.
    fn world(wrap: bool) -> Terrain {
        let mut basis =
            TerrainBasis::uniform(8, 6, wrap, 1, plot(Relief::Flat, TerrainKind::Grass));
        for (x, y, r, k) in [
            (2, 2, Relief::Hill, TerrainKind::Grass),
            (5, 2, Relief::Mountain, TerrainKind::Plains),
            (1, 4, Relief::Water, TerrainKind::Coast),
        ] {
            let i = basis.index(x, y).unwrap();
            basis.plots[i] = plot(r, k);
        }
        let mut t = Terrain::default();
        t.set_library(Arc::new(TerrainLibrary::synthetic()));
        t.set_basis(basis);
        t.build_all();
        t
    }

    fn top(x: i32, y: i32, h: f32) -> Vec3 {
        let c = cell_center(x, y);
        Vec3::new(c.x, h, c.y)
    }

    /// The point of the field over the middle of a hex, a little under its surface so that a ray
    /// aimed at it meets the surface in front of it (the peak of the mountain is a cone, thinner than
    /// the dome of B1: its top is a little lower than `MOUNTAIN_TOP`).
    fn on_field(t: &Terrain, x: i32, y: i32) -> Vec3 {
        let c = cell_center(x, y);
        let h = t.map.as_ref().unwrap().sampler.world_height(c);
        Vec3::new(c.x, h - 0.3, c.y)
    }

    fn camera_ray(cam: &GameCamera, target: Vec3) -> Ray {
        let clip = cam.view_proj(ASPECT) * target.extend(1.0);
        cam.unproject(Vec2::new(clip.x, clip.y) / clip.w, ASPECT)
    }

    fn low_ray(target: Vec3, from_xz: Vec2, elevation_deg: f32) -> Ray {
        let e = elevation_deg.to_radians();
        let back = Vec3::new(from_xz.x * e.cos(), e.sin(), from_xz.y * e.cos());
        Ray {
            origin: target + back * 800.0,
            dir: -back,
        }
    }

    /// Five camera set-ups: zoom level and focus offset (those of B1's pick tests).
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

    fn pick(t: &Terrain, ray: &Ray) -> Option<Pick> {
        t.pick(ray)
    }

    #[test]
    fn the_centres_of_a_plain_a_hill_and_a_mountain_from_five_cameras() {
        let t = world(false);
        for cell in [(3, 3), (2, 2), (5, 2)] {
            let target = on_field(&t, cell.0, cell.1);
            for cam in cameras(Vec2::new(target.x, target.z)) {
                let hit = pick(&t, &camera_ray(&cam, target)).expect("hit");
                assert_eq!(hit.cell, cell, "camera focus {:?}", cam.focus());
                // The top of the dome is the texel's height, quantised and between four texels.
                assert!((hit.point - target).length() < 1.5, "{:?}", hit.point);
            }
        }
    }

    #[test]
    fn a_slope_in_front_of_the_mountain_picks_the_mountain() {
        let t = world(false);
        // Towards the camera (+z) the profile is linear from the peak to the rim at the corner.
        for frac in [0.45, 0.7, 0.9] {
            let d = CORNER_RADIUS * frac;
            let h = LAND_HEIGHT + (MOUNTAIN_TOP - LAND_HEIGHT) * (1.0 - frac);
            let target = top(5, 2, h) + Vec3::new(0.0, 0.0, d);
            for cam in cameras(Vec2::new(target.x, target.z)) {
                assert_eq!(
                    pick(&t, &camera_ray(&cam, target)).map(|p| p.cell),
                    Some((5, 2)),
                    "frac {frac}"
                );
            }
            let low = low_ray(target, Vec2::new(0.0, 1.0), 25.0);
            assert_eq!(pick(&t, &low).map(|p| p.cell), Some((5, 2)));
        }
    }

    #[test]
    fn the_top_of_a_hill_wins_over_the_ground_behind_it() {
        let t = world(false);
        let target = on_field(&t, 2, 2);
        // A shallow ray from the south: the plane of the plain is hit 48 units behind the hill.
        let ray = low_ray(target, Vec2::new(0.0, 1.0), 15.0);
        assert_eq!(pick(&t, &ray).map(|p| p.cell), Some((2, 2)));
        let target = on_field(&t, 5, 2);
        for elev in [15.0, 25.0] {
            let ray = low_ray(target, Vec2::new(0.0, 1.0), elev);
            assert_eq!(pick(&t, &ray).map(|p| p.cell), Some((5, 2)), "elev {elev}");
        }
    }

    #[test]
    fn the_foot_of_the_mountain_is_the_plain_in_front_of_it() {
        let t = world(false);
        let target = top(5, 3, LAND_HEIGHT);
        for cam in cameras(Vec2::new(target.x, target.z)) {
            assert_eq!(
                pick(&t, &camera_ray(&cam, target)).map(|p| p.cell),
                Some((5, 3))
            );
        }
    }

    #[test]
    fn the_water_is_picked_at_its_level() {
        let t = world(false);
        let target = top(1, 4, WATER_PLANE);
        for cam in cameras(Vec2::new(target.x, target.z)) {
            let hit = pick(&t, &camera_ray(&cam, target)).expect("hit");
            assert_eq!(hit.cell, (1, 4));
            // Without the coast splines the hex is a block of water in the plain: the ray may meet
            // the bank (land at 12) before the level.
            assert!(
                hit.point.y < LAND_HEIGHT + 0.2 && hit.point.y > WATER_PLANE - 0.3,
                "{:?}",
                hit.point
            );
        }
    }

    #[test]
    fn off_the_map_and_up_the_ray_hits_nothing() {
        let t = world(false);
        for target in [
            Vec3::new(-300.0, 12.0, -100.0),
            Vec3::new(100.0, 12.0, -900.0),
        ] {
            let cam = GameCamera::new(Vec2::new(target.x, target.z));
            assert_eq!(pick(&t, &camera_ray(&cam, target)), None);
        }
        let up = Ray {
            origin: top(3, 3, 300.0),
            dir: Vec3::Y,
        };
        assert_eq!(pick(&t, &up), None);
        let below = Ray {
            origin: top(3, 3, 5.0),
            dir: Vec3::NEG_Y,
        };
        assert_eq!(pick(&t, &below), None);
    }

    #[test]
    fn the_ray_goes_through_the_seam_of_a_wrapping_map() {
        let t = world(true);
        let width = t.sampler().unwrap().frame().world_width();
        let hill = top(2, 2, HILL_TOP);
        for shift in [-width, width] {
            let target = hill + Vec3::new(shift, 0.0, 0.0);
            for cam in cameras(Vec2::new(target.x, target.z)) {
                assert_eq!(
                    pick(&t, &camera_ray(&cam, target)).map(|p| p.cell),
                    Some((2, 2)),
                    "shift {shift}"
                );
            }
        }
        // Unwrapped x = -1 is hex 7 of the row.
        let target = top(-1, 3, LAND_HEIGHT);
        let cam = GameCamera::new(Vec2::new(target.x, target.z));
        assert_eq!(
            pick(&t, &camera_ray(&cam, target)).map(|p| p.cell),
            Some((7, 3))
        );
        // Without wrapping the same point is off the map.
        let flat = world(false);
        assert_eq!(pick(&flat, &camera_ray(&cam, target)), None);
    }

    #[test]
    fn a_cell_that_is_not_built_is_not_picked() {
        let mut t = Terrain::default();
        t.set_library(Arc::new(TerrainLibrary::synthetic()));
        t.set_basis(TerrainBasis::uniform(
            8,
            6,
            false,
            1,
            plot(Relief::Flat, TerrainKind::Grass),
        ));
        // Nothing is built: the ray finds no ground; the plane of the water is below the plain, so
        // it is the water the ray meets, at the hex under it.
        let target = top(3, 3, LAND_HEIGHT);
        let cam = GameCamera::new(Vec2::new(target.x, target.z));
        let hit = pick(&t, &camera_ray(&cam, target)).expect("the water plane");
        assert!((hit.point.y - WATER_PLANE).abs() < 1e-3);
    }
}
