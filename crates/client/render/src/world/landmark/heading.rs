//! The layout heading of the farms (RE-B18 item 3a F1): the direction the fields and the farm model
//! follow, a base angle turned by two octaves of simplex noise over the original's world.
//!
//! `LayoutHeading(p) = 64 (cos a, sin a)`, `a = base + (pi/8) S(p/150 + off) + (pi/16) S(p/40 + off)`,
//! `p` the point of the original's world (x east, y north, the map centre at the origin), `off` the
//! centre of the world in texels times `g_TerrainScale` and `S` Gustavson's 2D simplex noise with Ken
//! Perlin's table, times 70.
// re: LayoutHeading Fun_087bbab8, HeadingBase FUN_08847bb0 (0x0a7f1978), the offset FUN_0868a612,
// FNoise::Noise2D 0x08cd8724 (scale 70.0 @0x0923fba8, perm @0x0923fbac, grad3 @0x0923ffac)

use glam::{Vec2, Vec3};

use crate::hexgrid::HEX_WIDTH;
use crate::world::terrain::sampler::TerrainFrame;

/// Ken Perlin's permutation.
// re: 0x0923fbac (151, 160, 137, 91, 90, 15, ...)
const PERM: [u8; 256] = [
    151, 160, 137, 91, 90, 15, 131, 13, 201, 95, 96, 53, 194, 233, 7, 225, 140, 36, 103, 30, 69,
    142, 8, 99, 37, 240, 21, 10, 23, 190, 6, 148, 247, 120, 234, 75, 0, 26, 197, 62, 94, 252, 219,
    203, 117, 35, 11, 32, 57, 177, 33, 88, 237, 149, 56, 87, 174, 20, 125, 136, 171, 168, 68, 175,
    74, 165, 71, 134, 139, 48, 27, 166, 77, 146, 158, 231, 83, 111, 229, 122, 60, 211, 133, 230,
    220, 105, 92, 41, 55, 46, 245, 40, 244, 102, 143, 54, 65, 25, 63, 161, 1, 216, 80, 73, 209, 76,
    132, 187, 208, 89, 18, 169, 200, 196, 135, 130, 116, 188, 159, 86, 164, 100, 109, 198, 173,
    186, 3, 64, 52, 217, 226, 250, 124, 123, 5, 202, 38, 147, 118, 126, 255, 82, 85, 212, 207, 206,
    59, 227, 47, 16, 58, 17, 182, 189, 28, 42, 223, 183, 170, 213, 119, 248, 152, 2, 44, 154, 163,
    70, 221, 153, 101, 155, 167, 43, 172, 9, 129, 22, 39, 253, 19, 98, 108, 110, 79, 113, 224, 232,
    178, 185, 112, 104, 218, 246, 97, 228, 251, 34, 242, 193, 238, 210, 144, 12, 191, 179, 162,
    241, 81, 51, 145, 235, 249, 14, 239, 107, 49, 192, 214, 31, 181, 199, 106, 157, 184, 84, 204,
    176, 115, 121, 50, 45, 127, 4, 150, 254, 138, 236, 205, 93, 222, 114, 67, 29, 24, 72, 243, 141,
    128, 195, 78, 66, 215, 61, 156, 180,
];

/// The x and y of `grad3` (the 3D gradients of the edges of a cube).
// re: 0x0923ffac
const GRAD: [(f32, f32); 12] = [
    (1.0, 1.0),
    (-1.0, 1.0),
    (1.0, -1.0),
    (-1.0, -1.0),
    (1.0, 0.0),
    (-1.0, 0.0),
    (1.0, 0.0),
    (-1.0, 0.0),
    (0.0, 1.0),
    (0.0, -1.0),
    (0.0, 1.0),
    (0.0, -1.0),
];

// re: FNoise::Noise2D 0x08cd8724
const F2: f32 = 0.366_025_42;
const G2: f32 = 0.211_324_87;
const SCALE: f32 = 70.0;

fn perm(i: i32) -> i32 {
    i32::from(PERM[(i & 255) as usize])
}

/// Gustavson's 2D simplex noise, about -1..1.
pub fn simplex2(x: f32, y: f32) -> f32 {
    let s = (x + y) * F2;
    let i = (x + s).floor() as i32;
    let j = (y + s).floor() as i32;
    let t = (i + j) as f32 * G2;
    let x0 = x - (i as f32 - t);
    let y0 = y - (j as f32 - t);
    let (i1, j1) = if x0 > y0 { (1, 0) } else { (0, 1) };
    let corners = [
        (x0, y0, perm(i + perm(j))),
        (
            x0 - i1 as f32 + G2,
            y0 - j1 as f32 + G2,
            perm(i + i1 + perm(j + j1)),
        ),
        (
            x0 - 1.0 + 2.0 * G2,
            y0 - 1.0 + 2.0 * G2,
            perm(i + 1 + perm(j + 1)),
        ),
    ];
    let n: f32 = corners
        .iter()
        .map(|&(dx, dy, h)| {
            let t = 0.5 - dx * dx - dy * dy;
            if t < 0.0 {
                0.0
            } else {
                let (gx, gy) = GRAD[(h % 12) as usize];
                let t2 = t * t;
                t2 * t2 * (gx * dx + gy * dy)
            }
        })
        .sum();
    SCALE * n
}

/// The base of the heading: `atan2(0.60971075, 0.79262394)`, 37.6 degrees from east to north.
// re: FUN_08847bb0 (writes 0x0a7f1978)
pub fn heading_base() -> f32 {
    0.609_710_75f32.atan2(0.792_623_94)
}

/// The heading field of one map: the original's world point of ours and the offset of the noise.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeadingField {
    /// `orig = (X, -Z) + to_orig`.
    to_orig: Vec2,
    /// The centre of the original's world: `(padded texels >> 1) * g_TerrainScale`.
    off: Vec2,
}

impl Default for HeadingField {
    fn default() -> Self {
        Self::new(&TerrainFrame::new(0, 0, true))
    }
}

impl HeadingField {
    /// The field of the map of `frame`. The original pads the map by 4 rows (and 4 columns when it
    /// does not wrap) and centres its world on the middle of the padded texels.
    // re: WorldGlobals::GetPaddedMapSize 0x086725b4, the offset FUN_0868a612 (`m_fOffsetX/Y`)
    pub fn new(frame: &TerrainFrame) -> Self {
        let m = frame.ours_to_orig();
        let t = m.transform_point3(Vec3::ZERO);
        let scale = frame.terrain_scale();
        let pad_w = frame.width_hexes() + if frame.wrap_x() { 0 } else { 4 };
        let pad_h = frame.height_hexes() + 4;
        let texels = |n: u32| ((n * HEX_WIDTH as u32) >> 1) as f32;
        Self {
            to_orig: Vec2::new(t.x, t.y),
            off: Vec2::new(texels(pad_w) * scale.x, texels(pad_h) * scale.y),
        }
    }

    /// The original's world point (x east, y north) of our `(X, Z)`.
    pub fn orig(&self, p: Vec2) -> Vec2 {
        Vec2::new(p.x, -p.y) + self.to_orig
    }

    /// The heading at our `(X, Z)`, radians from east towards north in the original's frame (the
    /// same turn as our `R_y`).
    pub fn angle(&self, p: Vec2) -> f32 {
        let o = self.orig(p);
        let a = o / 150.0 + self.off;
        let b = o / 40.0 + self.off;
        heading_base()
            + std::f32::consts::FRAC_PI_8 * simplex2(a.x, a.y)
            + std::f32::consts::FRAC_PI_8 * 0.5 * simplex2(b.x, b.y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_a_permutation_of_the_bytes() {
        let mut seen = [false; 256];
        for &p in &PERM {
            assert!(!seen[usize::from(p)], "{p} twice");
            seen[usize::from(p)] = true;
        }
        assert_eq!(&PERM[..6], &[151, 160, 137, 91, 90, 15]);
    }

    #[test]
    fn the_noise_is_zero_on_the_lattice_and_bounded_between() {
        assert_eq!(simplex2(0.0, 0.0), 0.0);
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for i in 0..4000 {
            let v = simplex2(i as f32 * 0.173, i as f32 * 0.071 + 3.3);
            lo = lo.min(v);
            hi = hi.max(v);
        }
        assert!(lo > -1.0 && hi < 1.0, "{lo}..{hi}");
        assert!(hi - lo > 1.0, "the noise varies: {lo}..{hi}");
    }

    #[test]
    fn the_heading_turns_about_its_base_by_at_most_three_sixteenths_of_a_turn() {
        let f = HeadingField::new(&TerrainFrame::new(40, 24, true));
        let base = heading_base();
        assert!((base.to_degrees() - 37.56).abs() < 0.01);
        let mut spread = 0.0f32;
        for i in 0..400 {
            let p = Vec2::new(i as f32 * 7.3, -(i as f32) * 3.1);
            let d = f.angle(p) - base;
            assert!(d.abs() <= 3.0 * std::f32::consts::PI / 16.0 + 1e-4);
            spread = spread.max(d.abs());
        }
        assert!(spread > 0.1, "the field is not constant");
        // Smooth: two points a unit apart differ by little.
        let p = Vec2::new(300.0, -200.0);
        assert!((f.angle(p) - f.angle(p + Vec2::X)).abs() < 0.05);
    }

    #[test]
    fn the_original_point_is_centred_on_the_map() {
        let frame = TerrainFrame::new(40, 24, true);
        let f = HeadingField::new(&frame);
        let o = f.orig(Vec2::new(10.0, -20.0));
        let m = frame
            .ours_to_orig()
            .transform_point3(Vec3::new(10.0, 0.0, -20.0));
        assert!((o - Vec2::new(m.x, m.y)).length() < 1e-3);
        // The padded centre: 32 W texels across, 32 (H + 4) rows of 0.866.
        assert!((f.off.x - 32.0 * 40.0).abs() < 1e-3);
        let flat = HeadingField::new(&TerrainFrame::new(40, 24, false));
        assert!((flat.off.x - 32.0 * 44.0).abs() < 1e-3);
    }
}
