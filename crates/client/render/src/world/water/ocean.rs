//! The ocean and the lakes (world-water WW-1, WW-2; RW-11): the constants of
//! `FireGrafix::WaterGlobalData`, the textures of the surface, the three pipelines (one per
//! `TerrainWaterQuality`) and the temporary geometry (WW-D4): one quad over the map at the height
//! of the water, with the place in the height texture in its vertex.
//!
//! The shaders are `shaders/water/ocean.wgsl` (ports compared with the oracle). They work in the
//! basis of the original (x east, y north, z up) like the other ported families; `WaterFrame`
//! brings the matrices of our basis.

use crate::perf::PassCount;
use crate::perf::{Buf, Cluster, DeviceTrack};
use glam::{Mat4, Vec2, Vec3, Vec4};

use super::data::{LEAN_BUMPS, LEAN_BUMPS_CROSS, RIVER_BLEND, RIVER_COLOR, WaterData};
use crate::frame::mask::{MaskDepth, MaskPipelineDesc, MaskShader, class};
use crate::frame::{DEPTH_FORMAT, WORLD_FORMAT, multisample};
use crate::lighting::globals::row_major;
use crate::shaders::create_module;
use crate::texture::{UploadOptions, upload_blob};
use crate::uniform_block;
use crate::world::terrain::sampler::TerrainSampler;

/// Bytes between two copies' constants in the buffer: the block (592) up to the offset alignment.
pub const CONSTS_STRIDE: u64 = 768;

/// Copies of a wrapping map: the map and one to each side.
pub const COPIES: usize = 3;

/// Texels of the box that smooths the coast's depth ([`HeightMap::with_shelf`]): about a quarter of
/// a hex at the size the map has in the capture benchmark.
pub const SHELF_RADIUS: u32 = 6;

// re: WaterSystem::Render (RW-11), S1 capture event 1074 cb1. The sun and the sky of the water
// are the numbers of the capture until RW-4 b gives `LoadSunSky` (TODO(RW-4b)).
#[allow(clippy::excessive_precision)]
pub(crate) const SUN_DIR: [f32; 3] = [0.39889413, 0.56684953, 0.72080863];
#[allow(clippy::excessive_precision)]
const SKY_DIR: [f32; 3] = [-0.10251313, 0.90140909, 0.42065760];
const SPEC_SUN: [f32; 4] = [0.9411765, 0.8196079, 0.4, 3444.3118];
const SPEC_SKY: [f32; 4] = [0.482353, 0.678431, 0.984314, 53.81737];
const FAKE_VIEW_SUN: [f32; 4] = [-0.3275047, -0.6681096, 0.6681096, 0.7058824];
const FAKE_VIEW_SKY: [f32; 4] = [0.0091706, -0.8161702, 0.5777386, 0.0];
const GRADIENT_SCALE: f32 = 5.0;
// The distance fog of the water: the numbers of the frame (lighting::globals::fog_consts), S1a, S2 and S3.
pub(crate) use crate::lighting::globals::{
    FOG_COLOR_DEFAULT as FOG_COLOR, fog_consts as fog_consts_for_eye_z,
};
const REFLECTION_AMOUNT: f32 = 0.5;
const UNUSED_C12: [f32; 4] = [0.386162, 0.463394, 0.0965405, -0.579243];
const UNUSED_C14: [f32; 4] = [1.7, 1.2, 1.7, 1.2];
/// `fOceanHeight` of the capture: raw 13000 of 65535 times 255 times the 0.2 of the height scale.
pub const OCEAN_HEIGHT: f32 = 10.116731;
/// The scroll of the LEAN maps is `time * rate * WaterScale.x`: `ScrollPhaseA` grows by
/// `ScrollSpeedA (0.18) * 4` a second. (The 11.8 of the captures is that phase, not seconds.)
// re: WaterSystem::Update 0x0868bbae (TerrainSystem.cpp:1306-1310, 1488-1493; re-b16 O2)
pub(crate) const SCROLL_RATE: f32 = 0.18 * 4.0;
/// The projection of the overlay maps in the capture (rows of the registers): the stand-in until the
/// frame brings the projection of the mask of the fog ([`WaterFrame::overlay_proj`]).
pub(crate) const OVERLAY_ROWS: [[f32; 4]; 4] = [
    [0.00223214, 0.0, 0.0, 0.0],
    [0.0, 0.00284091, 0.0, 0.0],
    [0.0, 0.0, -0.5, 0.0],
    [-0.857143, 0.454545, 0.5, 1.0],
];

uniform_block! {
    /// `FireGrafix::WaterGlobalData`: 592 bytes, 148 floats; the name of a field is its CTAB name,
    /// the number in the comment of the WGSL struct is the index of its first float.
    pub struct WaterConsts {
        pub g_VP: [[f32; 4]; 4],
        pub OverlayProjection: [[f32; 4]; 4],
        pub unused_c12: [f32; 4],
        pub WaterScale: [f32; 4],
        pub unused_c14: [f32; 4],
        pub WaterOffset: f32,
        pub pad45: f32,
        pub pad46: f32,
        pub pad47: f32,
        pub fOceanHeight: f32,
        pub pad49: f32,
        pub pad50: f32,
        pub pad51: f32,
        pub GradientScale: f32,
        pub pad53: f32,
        pub pad54: f32,
        pub pad55: f32,
        pub SunDir: [f32; 3],
        pub pad59: f32,
        pub SkyDir: [f32; 3],
        pub pad63: f32,
        pub SpecSun: [f32; 4],
        pub SpecSky: [f32; 4],
        pub FakeViewSun: [f32; 4],
        pub FakeViewSky: [f32; 4],
        pub g_FogConsts: [f32; 4],
        pub v3CameraWorld: [f32; 3],
        pub pad87: f32,
        pub g_ReflectVP: [[f32; 4]; 4],
        pub g_ShadowVP: [[f32; 4]; 4],
        pub g_WorldToTerrainTextureSpace: [[f32; 4]; 4],
        pub g_ShadowFade: f32,
        pub pad137: f32,
        pub pad138: f32,
        pub pad139: f32,
        pub f3FogColor: [f32; 4],
        pub ReflectionAmount: f32,
        pub pad145: f32,
        pub pad146: f32,
        pub pad147: f32,
    }
}

/// What the camera shows, in our basis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WaterFrame {
    /// `clip = view_proj * point` in our basis.
    pub view_proj: Mat4,
    pub eye: Vec3,
    /// Our basis to the original's (`TerrainFrame::ours_to_orig()`).
    pub ours_to_orig: Mat4,
    /// Seconds.
    pub time: f32,
    /// The sun's view in the basis of the original (plain matrix; identity without shadows).
    pub shadow_vp: Mat4,
    /// `FadeShadows` on and the sun map in use: 1, else 0.
    pub shadow_fade: f32,
    /// The reflection camera's view, in the basis of the original.
    pub reflect_vp: Mat4,
    /// The projection of the overlay maps and the mask of the fog (`OverlayProjection`): a point
    /// of the original's basis to the clip space of the mask. `None`: the numbers of the capture.
    pub overlay_proj: Option<Mat4>,
}

impl Default for WaterFrame {
    fn default() -> Self {
        Self {
            view_proj: Mat4::IDENTITY,
            eye: Vec3::ZERO,
            ours_to_orig: crate::frame::basis_rotation(),
            time: 0.0,
            shadow_vp: Mat4::IDENTITY,
            shadow_fade: 0.0,
            reflect_vp: Mat4::IDENTITY,
            overlay_proj: None,
        }
    }
}

/// What the map gives the water.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OceanMap {
    /// `TerrainFrame::half_world()`.
    pub half_world: Vec2,
    /// `fOceanHeight`.
    pub ocean_height: f32,
    /// The map repeats along X (three copies are drawn when they are seen).
    pub wrap_x: bool,
}

impl OceanMap {
    /// The whole world, in units of the original.
    pub fn world_size(&self) -> Vec2 {
        self.half_world * 2.0
    }

    /// The X shifts of the copies (`b1_env::map_shifts`): one on a flat map, three on a wrapping one.
    pub fn shifts(&self) -> Vec<f32> {
        if self.wrap_x {
            let w = self.world_size().x;
            vec![0.0, -w, w]
        } else {
            vec![0.0]
        }
    }
}

/// The move of the copy shifted by `shift` (our X) in the basis of the original: a point of the middle copy to where
/// that copy draws it.
pub fn copy_translation(f: &WaterFrame, shift: f32) -> Mat4 {
    Mat4::from_translation(f.ours_to_orig.transform_vector3(Vec3::new(shift, 0.0, 0.0)))
}

/// Slot of the copy shifted by `shift` in the buffer of constants.
pub fn copy_slot(shift: f32) -> usize {
    if shift == 0.0 {
        0
    } else if shift < 0.0 {
        1
    } else {
        2
    }
}

impl WaterConsts {
    /// The constants of the copy of the map shifted by `shift` along X (our basis).
    // re: WaterSystem::Render (RW-11): the layout of the block, `WaterScale = (n, n+1, n+2, n+3) / W`
    // with `n = floor(W * 0.008)` and `W` the width of the world.
    pub fn new(map: &OceanMap, f: &WaterFrame, shift: f32) -> Self {
        let size = map.world_size();
        let orig_to_ours = f.ours_to_orig.inverse();
        let view_proj =
            f.view_proj * Mat4::from_translation(Vec3::new(shift, 0.0, 0.0)) * orig_to_ours;
        let eye = f
            .ours_to_orig
            .transform_point3(f.eye - Vec3::new(shift, 0.0, 0.0));
        let w = size.x.max(1.0);
        let n = (w * 0.008).floor();
        let scale = [n / w, (n + 1.0) / w, (n + 2.0) / w, (n + 3.0) / w];
        // The world to the texture space of the terrain, as registers (row `k` of the block is
        // the column `k` of this plain matrix); the Y flip puts the north in row 0 of the texture.
        let to_tex = Mat4::from_cols(
            Vec4::new(1.0 / size.x, 0.0, 0.0, 0.0),
            Vec4::new(0.0, -1.0 / size.y, 0.0, 0.0),
            Vec4::ZERO,
            Vec4::new(0.5, -0.5, 0.0, 1.0),
        );
        let overlay = f
            .overlay_proj
            .unwrap_or_else(|| Mat4::from_cols_array_2d(&OVERLAY_ROWS));
        // The shaders take the point of the map where it is in the middle copy (`g_VP` moves it to its copy); the
        // overlay maps, the mask of the fog, the shadow map and the reflection cover what the camera sees, so they
        // take it where it is drawn: moved by the copy's shift as well. Without the move a copy of the map read
        // them a world width away (the seam of the live report 2026-10-31: no grid, no selection ring and the
        // fog of the wrong side on the water past the edge of the map, shadows of other places on it).
        let to_copy = copy_translation(f, shift);
        Self {
            g_VP: row_major(view_proj),
            // Register `k` is column `k` of the plain matrix, as `g_VP`: stored as the registers
            // themselves, the shaders read the projection transposed (no move, the mask's corner
            // for every point: the rivers were killed by their `texkill` everywhere).
            OverlayProjection: row_major(overlay * to_copy),
            unused_c12: UNUSED_C12,
            WaterScale: scale,
            unused_c14: UNUSED_C14,
            WaterOffset: f.time * SCROLL_RATE * scale[0],
            pad45: 0.0,
            pad46: 0.0,
            pad47: 0.0,
            fOceanHeight: map.ocean_height,
            pad49: 0.0,
            pad50: 0.0,
            pad51: 0.0,
            GradientScale: GRADIENT_SCALE,
            pad53: 0.0,
            pad54: 0.0,
            pad55: 0.0,
            SunDir: SUN_DIR,
            pad59: 0.0,
            SkyDir: SKY_DIR,
            pad63: 0.0,
            SpecSun: SPEC_SUN,
            SpecSky: SPEC_SKY,
            FakeViewSun: FAKE_VIEW_SUN,
            FakeViewSky: FAKE_VIEW_SKY,
            g_FogConsts: fog_consts_for_eye_z(eye.z),
            v3CameraWorld: eye.to_array(),
            pad87: 0.0,
            g_ReflectVP: row_major(f.reflect_vp * to_copy),
            g_ShadowVP: row_major(f.shadow_vp * to_copy),
            g_WorldToTerrainTextureSpace: row_major(to_tex),
            g_ShadowFade: f.shadow_fade,
            pad137: 0.0,
            pad138: 0.0,
            pad139: 0.0,
            f3FogColor: FOG_COLOR,
            ReflectionAmount: REFLECTION_AMOUNT,
            pad145: 0.0,
            pad146: 0.0,
            pad147: 0.0,
        }
    }
}

// ---- the depth ----

/// The height texture of the water (`WorldHeightMap`): one byte per texel, `raw >> 8` (the shader
/// takes the height as `x * 255 * 0.2` and compares it with `fOceanHeight`), row 0 the north, one
/// level and no mips.
// re: Terrain::RecreateGPUHeightmapAndIDMap 0x0890c454 (Terrain.cpp:346, 356-363; re-b16 O1, O1b):
// `TextureData(0x1c, 0, 1024, 1024, 1, nMips 1, ..)`, texel = `(unsigned char)(raw >> 8)`, one texel per
// field sample. With mips a far camera read levels that average the land into the sea: the shallow
// band of the depth ramp (transparent, teal-green) grew wider the further the camera was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeightMap {
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
}

/// One texel of the map's raw height (or the average of a few) in the byte of the texture: the high
/// byte, truncated.
fn height_byte(raw: f32) -> u8 {
    (raw / 256.0).floor().clamp(0.0, 255.0) as u8
}

/// The longest side of the height texture for a `TerrainWaterQuality` on a device whose textures
/// may be `device_max` texels long. High (2) is the original's: the field at full resolution, one
/// texel per height sample, whenever the device takes it (a huge map is 8192 x 5184 texels, 40.5 MiB);
/// the phone presets keep a coarser depth (field texels averaged into one) so that a large map stays
/// a few MiB: Medium (1) at most 4096, Low (0) at most 2048 texels a side.
// SIMPLIFIED(SIMP-exe-1230): one texture of the whole world, averaged down where the device or the
// preset is short, instead of the original's 1024 x 1024 tiles at full resolution everywhere.
pub fn height_max_side(quality: usize, device_max: u32) -> u32 {
    let preset = match quality {
        0 => 2048,
        1 => 4096,
        _ => u32::MAX,
    };
    preset.min(device_max).max(1)
}

impl HeightMap {
    /// The field of `sampler`, averaged down so that the long side is at most `max_side`.
    pub fn build(sampler: &TerrainSampler, max_side: u32) -> Self {
        let (fw, fh) = (
            sampler.field().width().max(1),
            sampler.field().height().max(1),
        );
        let raw = |u: i32, v: i32| f32::from(sampler.raw_height(u, v));
        HeightState::build(fw, fh, max_side, 0, &raw).map
    }

    /// The shelf of the coast: the heights smoothed by a box of `radius` texels on each side, so the
    /// depth of the water grows from the shore over a fraction of a hex and the shallows are a wide
    /// soft band, like the capture's, and not a step from the sand to the open water.
    // SIMPLIFIED(SIMP-021): the original's coast cells slope on their own; ours are as steep as the
    // terrain cells give them, the band is the blur of that edge.
    pub fn with_shelf(self, radius: u32) -> Self {
        if radius == 0 {
            return self;
        }
        let (w, h) = (self.width as usize, self.height as usize);
        let r = radius as usize;
        let mut across = vec![0u8; self.bytes.len()];
        for l in 0..h {
            blur_line(&self.bytes, &mut across, w, l, w, 1, r);
        }
        let mut bytes = vec![0u8; self.bytes.len()];
        for l in 0..w {
            blur_line(&across, &mut bytes, h, l, 1, w, r);
        }
        Self {
            width: self.width,
            height: self.height,
            bytes,
        }
    }

    /// A map of one flat byte, for tests and for a world without heights.
    pub fn flat(width: u32, height: u32, value: u8) -> Self {
        Self {
            width,
            height,
            bytes: vec![value; (width * height) as usize],
        }
    }

    /// Bytes of the texture (one level).
    pub fn gpu_bytes(&self) -> usize {
        self.bytes.len()
    }
}

/// The heights of the water made so that a cell of the field that changed is made again alone: the
/// averaged field (`base`) and its blur across (`across`) are kept when there is a blur; without one
/// (the coast styles, the live case) the map is the averaged field itself and nothing else is kept.
/// A cell of the field is the texels of its hex; the bytes are those of
/// `HeightMap::build(..).with_shelf(radius)`.
pub struct HeightState {
    pub map: HeightMap,
    base: Vec<u8>,
    across: Vec<u8>,
    /// Field texels to a texel of the map, and the size of the field.
    f: u32,
    fw: u32,
    fh: u32,
    radius: u32,
}

/// A rectangle of texels `[x0, x1) x [y0, y1)` of the map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TexelRect {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

impl TexelRect {
    fn union(self, o: Self) -> Self {
        Self {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }
}

impl HeightState {
    /// The map of a field of `fw` x `fh` texels read through `raw(u, v)`, in the byte of the texture.
    pub fn build(
        fw: u32,
        fh: u32,
        max_side: u32,
        radius: u32,
        raw: &dyn Fn(i32, i32) -> f32,
    ) -> Self {
        let mut build = HeightBuild::new(fw, fh, max_side, radius);
        while !build.step(usize::MAX, raw) {}
        build.finish()
    }

    /// The size of the field the map is made from.
    pub fn field_size(&self) -> (u32, u32) {
        (self.fw, self.fh)
    }

    /// Field texels averaged into one texel of the map (1: full resolution).
    pub fn texels_per_texel(&self) -> u32 {
        self.f
    }

    /// The average of the field texels under the map texel `(col, row)`.
    fn texel(&self, col: u32, row: u32, raw: &dyn Fn(i32, i32) -> f32) -> u8 {
        field_texel((self.f, self.map.height, self.fw, self.fh), col, row, raw)
    }

    /// Makes the texels under the field rectangle `[u0, u1) x [v0, v1)` again from `raw`, and what
    /// the blur makes of them. Returns the changed rectangle of the map.
    pub fn update(
        &mut self,
        (u0, v0, u1, v1): (u32, u32, u32, u32),
        raw: &dyn Fn(i32, i32) -> f32,
    ) -> TexelRect {
        let (w, h) = (self.map.width, self.map.height);
        let f = self.f;
        let c0 = (u0 / f).min(w - 1);
        let c1 = ((u1.max(u0 + 1) - 1) / f).min(w - 1);
        // Field rows from the south: the map row of field row `v` is `h - 1 - v / f`.
        let r0 = (h - 1).saturating_sub(((v1.max(v0 + 1) - 1) / f).min(h - 1));
        let r1 = (h - 1).saturating_sub((v0 / f).min(h - 1));
        let r = self.radius;
        if r == 0 {
            for row in r0..=r1 {
                for col in c0..=c1 {
                    self.map.bytes[(row * w + col) as usize] = self.texel(col, row, raw);
                }
            }
            return TexelRect {
                x0: c0,
                y0: r0,
                x1: c1 + 1,
                y1: r1 + 1,
            };
        }
        for row in r0..=r1 {
            for col in c0..=c1 {
                self.base[(row * w + col) as usize] = self.texel(col, row, raw);
            }
        }
        let (wu, hu) = (w as usize, h as usize);
        let window =
            |data: &[u8], len: usize, at: usize, line: usize, stride: usize, step: usize| {
                let rr = r as usize;
                let n = 2 * rr + 1;
                let mut sum = 0usize;
                for k in 0..n {
                    let i = (at + k).saturating_sub(rr).min(len - 1);
                    sum += usize::from(data[line * stride + i * step]);
                }
                ((sum + n / 2) / n) as u8
            };
        // Across: the rows of the cell, the columns the blur reaches.
        let xa = c0.saturating_sub(r) as usize;
        let xb = ((c1 + r).min(w - 1)) as usize;
        for row in r0 as usize..=r1 as usize {
            for x in xa..=xb {
                self.across[row * wu + x] = window(&self.base, wu, x, row, wu, 1);
            }
        }
        // Down: the rows the blur reaches.
        let ya = r0.saturating_sub(r) as usize;
        let yb = ((r1 + r).min(h - 1)) as usize;
        for y in ya..=yb {
            for x in xa..=xb {
                self.map.bytes[y * wu + x] = window(&self.across, hu, y, x, 1, wu);
            }
        }
        TexelRect {
            x0: xa as u32,
            y0: ya as u32,
            x1: xb as u32 + 1,
            y1: yb as u32 + 1,
        }
    }

    /// The union of two changed rectangles.
    pub fn merge(into: &mut Option<TexelRect>, more: TexelRect) {
        *into = Some(into.map_or(more, |a| a.union(more)));
    }
}

/// The average of the field texels under the map texel `(col, row)`, for a map made of a field of
/// `fw` x `fh` texels at `f` field texels to a map texel and `h` rows.
fn field_texel(
    (f, h, fw, fh): (u32, u32, u32, u32),
    col: u32,
    row: u32,
    raw: &dyn Fn(i32, i32) -> f32,
) -> u8 {
    // The texture runs from the north; the field from the south.
    let v0 = (h - 1 - row) * f;
    let u0 = col * f;
    let mut sum = 0.0f32;
    let mut n = 0.0f32;
    for dv in 0..f {
        for du in 0..f {
            let (u, v) = (u0 + du, v0 + dv);
            if u < fw && v < fh {
                sum += raw(u as i32, v as i32);
                n += 1.0;
            }
        }
    }
    height_byte(sum / n.max(1.0))
}

/// A box blur of `radius` texels on each side with the edge repeated, of the line `l` of `len`
/// texels, written into `out`; texel `i` of line `l` is at `l * stride + i * step`.
fn blur_line(
    input: &[u8],
    out: &mut [u8],
    len: usize,
    l: usize,
    stride: usize,
    step: usize,
    r: usize,
) {
    let at = |i: usize| usize::from(input[l * stride + i * step]);
    let mut sum: usize = (0..=r.min(len - 1)).map(at).sum::<usize>() + at(0) * r;
    for i in 0..len {
        let n = 2 * r + 1;
        out[l * stride + i * step] = ((sum + n / 2) / n) as u8;
        let add = (i + r + 1).min(len - 1);
        let drop = i.saturating_sub(r);
        sum = sum + at(add) - at(drop);
    }
}

/// A [`HeightState`] made a few units of work at a time, so that the first build of a whole map is
/// not one frame: the field averaged down (`base`), then the blur across and down when there is one,
/// each in the order and with the arithmetic of the whole build, so the bytes are the same.
pub struct HeightBuild {
    f: u32,
    fw: u32,
    fh: u32,
    radius: u32,
    w: u32,
    h: u32,
    base: Vec<u8>,
    across: Vec<u8>,
    down: Vec<u8>,
    stage: BuildStage,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BuildStage {
    /// Next row of the averaged field.
    Base(u32),
    /// Next line of the blur across, then of the blur down.
    Across(usize),
    Down(usize),
    Done,
}

impl HeightBuild {
    /// A build for a field of `fw` x `fh` texels (see [`HeightState::build`]); nothing is read yet.
    pub fn new(fw: u32, fh: u32, max_side: u32, radius: u32) -> Self {
        let (fw, fh) = (fw.max(1), fh.max(1));
        let f = fw.max(fh).div_ceil(max_side.max(1)).max(1);
        let (w, h) = (fw.div_ceil(f), fh.div_ceil(f));
        Self {
            f,
            fw,
            fh,
            radius,
            w,
            h,
            base: vec![0u8; (w * h) as usize],
            across: Vec::new(),
            down: Vec::new(),
            stage: BuildStage::Base(0),
        }
    }

    /// The size of the field the heights are made from.
    pub fn field_size(&self) -> (u32, u32) {
        (self.fw, self.fh)
    }

    /// Work of the build still to do, in units (a read of the field, a texel blurred): the budget of
    /// [`Self::step`] is in the same unit. 0 once done.
    pub fn remaining(&self) -> usize {
        let (w, h) = (self.w as usize, self.h as usize);
        let per_row = w * (self.f * self.f) as usize;
        let blur = if self.radius == 0 { 0 } else { 2 * w * h };
        match self.stage {
            BuildStage::Base(row) => (h - row as usize) * per_row + blur,
            BuildStage::Across(l) => (h - l) * w + w * h,
            BuildStage::Down(l) => (w - l) * h,
            BuildStage::Done => 0,
        }
    }

    /// Does up to `units` of work (at least one piece) reading the field through `raw`; true once
    /// the build is done.
    pub fn step(&mut self, units: usize, raw: &dyn Fn(i32, i32) -> f32) -> bool {
        let (w, h) = (self.w as usize, self.h as usize);
        let r = self.radius as usize;
        let mut left = units;
        loop {
            match self.stage {
                BuildStage::Base(row) => {
                    let geometry = (self.f, self.h, self.fw, self.fh);
                    for col in 0..self.w {
                        self.base[(row * self.w + col) as usize] =
                            field_texel(geometry, col, row, raw);
                    }
                    left = left.saturating_sub(w * (self.f * self.f) as usize);
                    if row + 1 < self.h {
                        self.stage = BuildStage::Base(row + 1);
                    } else if r == 0 {
                        self.stage = BuildStage::Done;
                    } else {
                        self.across = vec![0u8; self.base.len()];
                        self.down = vec![0u8; self.base.len()];
                        self.stage = BuildStage::Across(0);
                    }
                }
                BuildStage::Across(l) => {
                    blur_line(&self.base, &mut self.across, w, l, w, 1, r);
                    left = left.saturating_sub(w);
                    self.stage = if l + 1 < h {
                        BuildStage::Across(l + 1)
                    } else {
                        BuildStage::Down(0)
                    };
                }
                BuildStage::Down(l) => {
                    blur_line(&self.across, &mut self.down, h, l, 1, w, r);
                    left = left.saturating_sub(h);
                    self.stage = if l + 1 < w {
                        BuildStage::Down(l + 1)
                    } else {
                        BuildStage::Done
                    };
                }
                BuildStage::Done => return true,
            }
            if left == 0 {
                return self.stage == BuildStage::Done;
            }
        }
    }

    /// The state once the build is done.
    pub fn finish(self) -> HeightState {
        assert!(
            self.stage == BuildStage::Done,
            "the height build is not done"
        );
        // Without a blur the map is the averaged field: one copy, not three (a huge map at full
        // resolution is 40 MiB of bytes).
        let (bytes, base) = if self.radius == 0 {
            (self.base, Vec::new())
        } else {
            (self.down, self.base)
        };
        HeightState {
            map: HeightMap {
                width: self.w,
                height: self.h,
                bytes,
            },
            base,
            across: self.across,
            f: self.f,
            fw: self.fw,
            fh: self.fh,
            radius: self.radius,
        }
    }
}

// ---- the geometry ----

/// The vertex: `(x, y)` in the world of the original, `(u, v)` in the height texture.
pub type OceanVertex = [f32; 4];

/// Two triangles over the map (WW-D4): `u = (x + half.x) / W`, `v = 1 - (y + half.y) / H`.
// re: RW-11 (WaterSystem builds quads per chunk with this vertex); WW-D4 is the temporary shape.
pub fn quad(half: Vec2) -> [OceanVertex; 6] {
    let size = half * 2.0;
    let v = |x: f32, y: f32| -> OceanVertex {
        [x, y, (x + half.x) / size.x, 1.0 - (y + half.y) / size.y]
    };
    let (a, b, c, d) = (
        v(-half.x, -half.y),
        v(half.x, -half.y),
        v(half.x, half.y),
        v(-half.x, half.y),
    );
    [a, b, c, a, c, d]
}

// ---- the bindings ----

/// What the surface reads besides its own textures; all of them are views the owner keeps.
#[derive(Clone, Default)]
pub struct OceanInputs {
    /// `WaterBlendMap`: the currents, `Rgba8Unorm`.
    pub currents: Option<wgpu::TextureView>,
    /// `ReflectionMap`: the reflection target, or none for black.
    pub reflection: Option<wgpu::TextureView>,
    /// `WaterFogRamp`: the ramp of the distance fog.
    pub fog_ramp: Option<wgpu::TextureView>,
    /// `TransFOWColor`: the 3D lookup of the fog of war (the colour key of the terrain).
    pub lut: Option<wgpu::TextureView>,
    /// `ColorKey` of the rivers (`Rivers_Generic_ColorKey`, S1a ev 1104 t12); none: `lut` stands in.
    pub river_key: Option<wgpu::TextureView>,
    /// `FOWMask`: none is "everything visible".
    pub fow: Option<wgpu::TextureView>,
    /// `OverlayMap`: none is "transparent".
    pub overlay: Option<wgpu::TextureView>,
    /// `DynamicShadowTexture`: none is "no shadow".
    pub shadow: Option<wgpu::TextureView>,
}

pub(crate) fn texture_entry(
    binding: u32,
    dim: wgpu::TextureViewDimension,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: dim,
            multisampled: false,
        },
        count: None,
    }
}

pub(crate) fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

/// Bindings 1..=22: the textures in pairs with their samplers; `true` marks the clamped ones
/// (`WaterColor`, `WaterFogRamp`, `TransFOWColor`: addressing of the table of facts).
const TEXTURES: [(&str, bool); 11] = [
    ("LEAN1_1", false),
    ("LEAN1_2", false),
    ("ReflectionMap", false),
    ("WaterBlendMap", false),
    ("WaterColor", true),
    ("WorldHeightMap", false),
    ("WaterFogRamp", true),
    ("OverlayMap", false),
    ("FOWMask", false),
    ("TransFOWColor", true),
    ("DynamicShadowTexture", false),
];

pub(crate) fn one_texel(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    format: wgpu::TextureFormat,
    texel: [u8; 4],
    dim: wgpu::TextureDimension,
) -> wgpu::TextureView {
    let texture = device.tex_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: dim,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &texel,
        Cluster::Other,
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// The surface of the water with everything it owns on the GPU.
pub struct Ocean {
    device: wgpu::Device,
    queue: wgpu::Queue,
    samples: u32,
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    module: wgpu::ShaderModule,
    pipelines: [Option<wgpu::RenderPipeline>; 3],
    wrap: wgpu::Sampler,
    clamp: wgpu::Sampler,
    consts: Buf,
    vertices: Buf,
    vertices_for: Option<Vec2>,
    // Textures of the pack.
    lean: Option<wgpu::TextureView>,
    lean_cross: Option<wgpu::TextureView>,
    color: Option<wgpu::TextureView>,
    /// The rivers' `WaterColor` (sRGB view, as the capture) and `WaterBlendMap`, when the pack has them.
    river_color: Option<wgpu::TextureView>,
    river_blend: Option<wgpu::TextureView>,
    data_revision: Option<u64>,
    // The depth, and the texture it is in (written in parts as the field builds).
    height: Option<wgpu::TextureView>,
    height_tex: Option<crate::perf::Tex>,
    // Placeholders for what other subsystems give.
    black: wgpu::TextureView,
    clear: wgpu::TextureView,
    visible: wgpu::TextureView,
    group: Option<wgpu::BindGroup>,
    /// The class mask's pipeline (`golden` only), made on the first mask draw.
    mask_pipeline: std::cell::OnceCell<wgpu::RenderPipeline>,
}

impl Ocean {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, samples: u32) -> Self {
        let module = create_module(
            device,
            "water ocean",
            crate::wgsl_module!("common/sm3.wgsl", "water/ocean.wgsl"),
        );
        let mut entries = vec![wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: true,
                min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<WaterConsts>() as u64),
            },
            count: None,
        }];
        for (i, (name, _)) in TEXTURES.iter().enumerate() {
            let dim = if *name == "TransFOWColor" {
                wgpu::TextureViewDimension::D3
            } else {
                wgpu::TextureViewDimension::D2
            };
            entries.push(texture_entry(1 + 2 * i as u32, dim));
            entries.push(sampler_entry(2 + 2 * i as u32));
        }
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("water ocean"),
            entries: &entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("water ocean"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let sampler = |label, mode| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(label),
                address_mode_u: mode,
                address_mode_v: mode,
                address_mode_w: mode,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                ..Default::default()
            })
        };
        let rgba = wgpu::TextureFormat::Rgba8Unorm;
        let d2 = wgpu::TextureDimension::D2;
        Self {
            device: device.clone(),
            queue: queue.clone(),
            samples,
            layout,
            pipeline_layout,
            module,
            pipelines: [None, None, None],
            wrap: sampler("water wrap", wgpu::AddressMode::Repeat),
            clamp: sampler("water clamp", wgpu::AddressMode::ClampToEdge),
            consts: device.buf(
                &wgpu::BufferDescriptor {
                    label: Some("water consts"),
                    size: CONSTS_STRIDE * COPIES as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                },
                Cluster::Other,
            ),
            vertices: device.buf(
                &wgpu::BufferDescriptor {
                    label: Some("water quad"),
                    size: std::mem::size_of::<[OceanVertex; 6]>() as u64,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                },
                Cluster::Other,
            ),
            vertices_for: None,
            lean: None,
            lean_cross: None,
            color: None,
            river_color: None,
            river_blend: None,
            data_revision: None,
            height: None,
            height_tex: None,
            black: one_texel(device, queue, "water black", rgba, [0, 0, 0, 255], d2),
            clear: one_texel(device, queue, "water clear", rgba, [0, 0, 0, 0], d2),
            // `FOWMask.x` is the visibility, `.z` the "seen now": both on.
            visible: one_texel(device, queue, "water visible", rgba, [255, 0, 255, 255], d2),
            group: None,
            mask_pipeline: std::cell::OnceCell::new(),
        }
    }

    /// Uploads the textures of the pack that changed. Returns whether the ocean can be drawn.
    pub fn set_data(&mut self, data: &WaterData) -> bool {
        if self.data_revision == Some(data.revision()) {
            return self.has_textures();
        }
        self.data_revision = Some(data.revision());
        let up = |blob: &asset_format::TextureBlob, label: &str| match upload_blob(
            &self.device,
            &self.queue,
            blob,
            label,
            UploadOptions::WORLD,
        ) {
            Ok(u) => Some(u.texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(u.view_dimension),
                ..Default::default()
            })),
            Err(e) => {
                tracing::warn!(label, error = %e, "water: texture not uploaded");
                None
            }
        };
        self.lean = data.texture(LEAN_BUMPS).and_then(|b| up(b, "water lean"));
        self.lean_cross = data
            .texture(LEAN_BUMPS_CROSS)
            .and_then(|b| up(b, "water lean cross"));
        // The original samples both colour maps through an sRGB view: the ocean's `WaterColor`
        // (S3 ev 9937, t6: R8G8B8A8_SRGB, 256 x 4) and the river colour map (S1a ev 1104, t6). Read
        // raw, the ramp of the sea comes out about twice as light.
        let up_srgb = |blob: &asset_format::TextureBlob, label: &str| {
            let mut b = blob.clone();
            b.srgb = true;
            match upload_blob(
                &self.device,
                &self.queue,
                &b,
                label,
                UploadOptions {
                    linear: false,
                    ..UploadOptions::WORLD
                },
            ) {
                Ok(u) => Some(
                    u.texture
                        .create_view(&wgpu::TextureViewDescriptor::default()),
                ),
                Err(e) => {
                    tracing::warn!(label, error = %e, "water: texture not uploaded");
                    None
                }
            }
        };
        self.color = data.water_color().and_then(|b| up_srgb(b, "water color"));
        self.river_blend = data.texture(RIVER_BLEND).and_then(|b| up(b, "river blend"));
        self.river_color = data
            .texture(RIVER_COLOR)
            .and_then(|b| up_srgb(b, "river color"));
        self.group = None;
        self.has_textures()
    }

    /// The LEAN maps and the colour ramp the rivers share with the surface (`LEAN1_1`, `LEAN1_2`,
    /// `WaterColor`), once they are uploaded.
    pub(crate) fn shared_views(
        &self,
    ) -> Option<(wgpu::TextureView, wgpu::TextureView, wgpu::TextureView)> {
        Some((
            self.lean.clone()?,
            self.lean_cross.clone()?,
            self.color.clone()?,
        ))
    }

    /// The rivers' own colour map and blend map (`None` where the pack lacks them: the rivers keep their
    /// stand-ins).
    pub(crate) fn river_views(&self) -> (Option<wgpu::TextureView>, Option<wgpu::TextureView>) {
        (self.river_color.clone(), self.river_blend.clone())
    }

    fn has_textures(&self) -> bool {
        self.lean.is_some() && self.lean_cross.is_some() && self.color.is_some()
    }

    /// Uploads the depth: one level, as the original's (re-b16 O1b).
    pub fn set_height(&mut self, map: &HeightMap) {
        let texture = self.device.tex_with_data(
            &self.queue,
            &wgpu::TextureDescriptor {
                label: Some("water height"),
                size: wgpu::Extent3d {
                    width: map.width,
                    height: map.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &map.bytes,
            Cluster::Other,
        );
        self.height = Some(texture.create_view(&wgpu::TextureViewDescriptor::default()));
        self.height_tex = Some(texture);
        self.group = None;
    }

    /// Writes the changed rectangle of the depth into its texture. False if there is no texture of
    /// the size of `map` to write into (the whole map is to be uploaded).
    pub fn update_height(&mut self, map: &HeightMap, rect: TexelRect) -> bool {
        let Some(tex) = &self.height_tex else {
            return false;
        };
        if tex.width() != map.width || tex.height() != map.height {
            return false;
        }
        let (w, h) = (rect.x1 - rect.x0, rect.y1 - rect.y0);
        if w > 0 && h > 0 {
            let mut bytes = Vec::with_capacity((w * h) as usize);
            for y in rect.y0..rect.y1 {
                let row = (y * map.width + rect.x0) as usize;
                bytes.extend_from_slice(&map.bytes[row..row + w as usize]);
            }
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: rect.x0,
                        y: rect.y0,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w),
                    rows_per_image: Some(h),
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
        }
        true
    }

    pub fn height_view(&self) -> Option<&wgpu::TextureView> {
        self.height.as_ref()
    }

    pub fn has_height(&self) -> bool {
        self.height.is_some()
    }

    /// Writes the constants of each copy and the quad of the map.
    pub fn prepare(&mut self, map: &OceanMap, frame: &WaterFrame) {
        for shift in map.shifts() {
            let slot = copy_slot(shift).min(COPIES - 1);
            let consts = WaterConsts::new(map, frame, shift);
            self.queue.write_buffer(
                &self.consts,
                slot as u64 * CONSTS_STRIDE,
                bytemuck::bytes_of(&consts),
            );
        }
        if self.vertices_for != Some(map.half_world) {
            self.queue.write_buffer(
                &self.vertices,
                0,
                bytemuck::cast_slice(&quad(map.half_world)),
            );
            self.vertices_for = Some(map.half_world);
        }
    }

    /// The pipeline of a quality (`TerrainWaterQuality`: 0 low, 1 medium, 2 high).
    fn pipeline(&mut self, quality: usize) -> &wgpu::RenderPipeline {
        let q = quality.min(2);
        if self.pipelines[q].is_none() {
            let fragment = ["psoceanlow", "psoceanmedium", "psoceanhigh"][q];
            let attributes = wgpu::vertex_attr_array![0 => Float32x4];
            let pipeline = self
                .device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(fragment),
                    layout: Some(&self.pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: &self.module,
                        entry_point: Some("vswaterocean"),
                        compilation_options: Default::default(),
                        buffers: &[Some(wgpu::VertexBufferLayout {
                            array_stride: std::mem::size_of::<OceanVertex>() as u64,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &attributes,
                        })],
                    },
                    primitive: wgpu::PrimitiveState {
                        topology: wgpu::PrimitiveTopology::TriangleList,
                        cull_mode: None,
                        ..Default::default()
                    },
                    // No depth write, `LessEqual`: the water lies over the seabed it was cut from.
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: DEPTH_FORMAT,
                        depth_write_enabled: Some(false),
                        depth_compare: Some(wgpu::CompareFunction::LessEqual),
                        stencil: Default::default(),
                        bias: Default::default(),
                    }),
                    multisample: multisample(self.samples),
                    fragment: Some(wgpu::FragmentState {
                        module: &self.module,
                        entry_point: Some(fragment),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: WORLD_FORMAT,
                            // re: RW-11: colour SrcAlpha / InvSrcAlpha, alpha One / One.
                            blend: Some(wgpu::BlendState {
                                color: wgpu::BlendComponent {
                                    src_factor: wgpu::BlendFactor::SrcAlpha,
                                    dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                                    operation: wgpu::BlendOperation::Add,
                                },
                                alpha: wgpu::BlendComponent {
                                    src_factor: wgpu::BlendFactor::One,
                                    dst_factor: wgpu::BlendFactor::One,
                                    operation: wgpu::BlendOperation::Add,
                                },
                            }),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                });
            self.pipelines[q] = Some(pipeline);
        }
        self.pipelines[q].as_ref().expect("made above")
    }

    /// Makes the pipeline of `quality` ready, so that `draw` can run from `&self`.
    pub fn ensure_pipeline(&mut self, quality: usize) {
        let _ = self.pipeline(quality);
    }

    /// The group of the frame, made again where an input changed. Returns whether the surface can
    /// be drawn: the textures of the pack, the depth, the ramp of the fog and the lookup of the fog
    /// of war and the currents are in.
    pub fn bind(&mut self, inputs: &OceanInputs) -> bool {
        let (Some(lean), Some(cross), Some(color), Some(height)) =
            (&self.lean, &self.lean_cross, &self.color, &self.height)
        else {
            return false;
        };
        let (Some(fog_ramp), Some(lut)) = (&inputs.fog_ramp, &inputs.lut) else {
            return false;
        };
        // Quality 0 has no map of the currents: black.
        let currents = inputs.currents.as_ref().unwrap_or(&self.black);
        let views: [&wgpu::TextureView; 11] = [
            lean,
            cross,
            inputs.reflection.as_ref().unwrap_or(&self.black),
            currents,
            color,
            height,
            fog_ramp,
            inputs.overlay.as_ref().unwrap_or(&self.clear),
            inputs.fow.as_ref().unwrap_or(&self.visible),
            lut,
            inputs.shadow.as_ref().unwrap_or(&self.black),
        ];
        let mut entries = vec![wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &self.consts,
                offset: 0,
                size: wgpu::BufferSize::new(std::mem::size_of::<WaterConsts>() as u64),
            }),
        }];
        for (i, view) in views.iter().enumerate() {
            entries.push(wgpu::BindGroupEntry {
                binding: 1 + 2 * i as u32,
                resource: wgpu::BindingResource::TextureView(view),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: 2 + 2 * i as u32,
                resource: wgpu::BindingResource::Sampler(if TEXTURES[i].1 {
                    &self.clamp
                } else {
                    &self.wrap
                }),
            });
        }
        self.group = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("water ocean"),
            layout: &self.layout,
            entries: &entries,
        }));
        true
    }

    pub fn is_bound(&self) -> bool {
        self.group.is_some()
    }

    /// Draws the copy shifted by `shift`; returns the number of indices drawn (the vertices) for
    /// the statistics.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, quality: usize, shift: f32) -> u32 {
        let (Some(group), Some(pipeline)) = (&self.group, &self.pipelines[quality.min(2)]) else {
            return 0;
        };
        pass.set_pipeline_c(pipeline);
        pass.set_bind_group_c(
            0,
            group,
            &[(copy_slot(shift) as u64 * CONSTS_STRIDE) as u32],
        );
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.draw_c(0..6, 0..1);
        6
    }

    /// The same quad into the class mask as [`class::WATER`], with the vertex stage of the
    /// surface (the same for the three qualities) and the group of the frame.
    pub fn draw_mask(&self, pass: &mut wgpu::RenderPass<'_>, shift: f32) {
        let Some(group) = &self.group else { return };
        let pipeline = self.mask_pipeline.get_or_init(|| {
            let attributes = wgpu::vertex_attr_array![0 => Float32x4];
            MaskShader::new(&self.device).pipeline_with(
                &self.device,
                &MaskPipelineDesc {
                    label: "water ocean mask",
                    vertex: &self.module,
                    vertex_entry: "vswaterocean",
                    vertex_buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<OceanVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attributes,
                    })],
                    bind_group_layouts: &[Some(&self.layout)],
                    cull: None,
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    class: class::WATER,
                },
                MaskDepth::OVER_BED,
            )
        });
        pass.set_pipeline_c(pipeline);
        pass.set_bind_group_c(
            0,
            group,
            &[(copy_slot(shift) as u64 * CONSTS_STRIDE) as u32],
        );
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.draw_c(0..6, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shaders::UniformBlock;

    fn map() -> OceanMap {
        OceanMap {
            half_world: Vec2::new(1088.0, 680.0),
            ocean_height: OCEAN_HEIGHT,
            wrap_x: true,
        }
    }

    /// A field of pseudo-random heights where the texels of the rectangles `changed` differ.
    fn field(changed: Vec<(i32, i32, i32, i32)>) -> impl Fn(i32, i32) -> f32 {
        move |u, v| {
            let mut x = (u as u32).wrapping_mul(0x9e37_79b1) ^ (v as u32).wrapping_mul(0x85eb_ca6b);
            for (u0, v0, u1, v1) in &changed {
                if (*u0..*u1).contains(&u) && (*v0..*v1).contains(&v) {
                    x ^= 0x5bd1_e995;
                }
            }
            x ^= x >> 15;
            x = x.wrapping_mul(0x2c1b_3c6d);
            x ^= x >> 12;
            (x % 65535) as f32
        }
    }

    #[test]
    fn a_cell_made_again_alone_gives_the_bytes_of_a_whole_new_build() {
        // f = 4 (cells of 64 texels are whole texels of the map), f = 3 (they are not) and f = 1 (the
        // full resolution), with the blur of the stand-in and without it.
        for (fw, fh, side, r) in [
            (256u32, 192u32, 64u32, 6u32),
            (200, 130, 70, 6),
            (256, 192, 4096, 0),
            (200, 130, 70, 0),
        ] {
            let mut state = HeightState::build(fw, fh, side, r, &field(vec![]));
            let (a, b) = ((0, 0, 64, 64), (64, 64, 128, 128));
            for (c, now) in [(a, vec![a]), (b, vec![a, b])] {
                let rect = state.update(
                    (c.0 as u32, c.1 as u32, c.2 as u32, c.3 as u32),
                    &field(now),
                );
                assert!(rect.x1 > rect.x0 && rect.y1 > rect.y0);
            }
            let fresh = HeightState::build(fw, fh, side, r, &field(vec![a, b]));
            assert_eq!(state.map.bytes, fresh.map.bytes, "{fw}x{fh} side {side}");
            // And the build is the old way: the field averaged down, then the shelf.
            assert_eq!(fresh.map.width, fw.div_ceil(fresh.f));
        }
    }

    /// A build made a few units at a time gives the bytes of the whole build, which are those of the
    /// field averaged down, blurred across and down with the edge repeated.
    #[test]
    fn a_build_in_pieces_gives_the_bytes_of_the_whole_build() {
        for (fw, fh, side, r) in [
            (256u32, 192u32, 64u32, 6u32),
            (200, 130, 70, 6),
            (90, 40, 90, 0),
        ] {
            let raw = field(vec![(10, 10, 60, 50)]);
            let whole = HeightState::build(fw, fh, side, r, &raw);
            // The plain way.
            let f = fw.max(fh).div_ceil(side).max(1);
            let (w, h) = (fw.div_ceil(f) as usize, fh.div_ceil(f) as usize);
            let mut base = vec![0u8; w * h];
            for row in 0..h {
                for col in 0..w {
                    base[row * w + col] =
                        field_texel((f, h as u32, fw, fh), col as u32, row as u32, &raw);
                }
            }
            let blur = |src: &[u8], len: usize, count: usize, stride: usize, step: usize| {
                let mut out = vec![0u8; src.len()];
                let n = 2 * r as usize + 1;
                for l in 0..count {
                    for i in 0..len {
                        let sum: usize = (0..n)
                            .map(|k| {
                                let j = (i + k).saturating_sub(r as usize).min(len - 1);
                                usize::from(src[l * stride + j * step])
                            })
                            .sum();
                        out[l * stride + i * step] = ((sum + n / 2) / n) as u8;
                    }
                }
                out
            };
            let level0 = if r == 0 {
                base
            } else {
                let across = blur(&base, w, h, w, 1);
                blur(&across, h, w, 1, w)
            };
            assert_eq!(whole.map.bytes, level0, "{fw}x{fh}");
            for units in [1usize, 700, 5000] {
                let mut build = HeightBuild::new(fw, fh, side, r);
                let mut steps = 0;
                while !build.step(units, &raw) {
                    steps += 1;
                    assert!(build.remaining() > 0);
                    assert!(steps < 1_000_000);
                }
                assert_eq!(build.remaining(), 0);
                let state = build.finish();
                assert_eq!(state.map.bytes, whole.map.bytes, "{fw}x{fh} {units}");
                assert_eq!(state.across, whole.across);
            }
        }
    }

    /// re-b16 O2: `WaterOffset` is `ScrollPhaseA * WaterScale.x` and the phase grows 0.72 a second,
    /// not 1 (the ripples ran 1.39 times too fast).
    #[test]
    fn the_ripples_scroll_0_72_of_the_scale_a_second() {
        let frame = WaterFrame {
            time: 10.0,
            ..Default::default()
        };
        let c = WaterConsts::new(&map(), &frame, 0.0);
        assert!((c.WaterOffset - 7.2 * c.WaterScale[0]).abs() < 1e-6);
    }

    #[test]
    fn the_block_is_the_592_bytes_of_the_original() {
        assert_eq!(std::mem::size_of::<WaterConsts>(), 592);
        let at = |n: &str| WaterConsts::field(n).unwrap().offset / 4;
        // The index of the first float of each constant (RW-11).
        for (name, index) in [
            ("g_VP", 0),
            ("OverlayProjection", 16),
            ("WaterScale", 36),
            ("WaterOffset", 44),
            ("fOceanHeight", 48),
            ("GradientScale", 52),
            ("SunDir", 56),
            ("SkyDir", 60),
            ("SpecSun", 64),
            ("SpecSky", 68),
            ("FakeViewSun", 72),
            ("FakeViewSky", 76),
            ("g_FogConsts", 80),
            ("v3CameraWorld", 84),
            ("g_ReflectVP", 88),
            ("g_ShadowVP", 104),
            ("g_WorldToTerrainTextureSpace", 120),
            ("g_ShadowFade", 136),
            ("f3FogColor", 140),
            ("ReflectionAmount", 144),
        ] {
            assert_eq!(at(name), index, "{name}");
        }
        assert!(CONSTS_STRIDE as usize >= 592 && CONSTS_STRIDE.is_multiple_of(256));
    }

    #[test]
    fn the_scale_of_the_lean_layers_follows_the_width_of_the_world() {
        // The capture: W = 2175 gives (17, 18, 19, 20) / W.
        let m = OceanMap {
            half_world: Vec2::new(1087.5, 600.0),
            ocean_height: OCEAN_HEIGHT,
            wrap_x: false,
        };
        let c = WaterConsts::new(&m, &WaterFrame::default(), 0.0);
        let want = [0.0078161, 0.0082759, 0.0087356, 0.0091954];
        for (g, w) in c.WaterScale.iter().zip(want) {
            assert!((g - w).abs() < 1e-6, "{g} {w}");
        }
    }

    /// `cache/golden/facts.md`, S1a ev 1074 and S2 ev 1006 (`PSOceanHigh` cb1+480): the world is
    /// 2175 x 1883.55 units, `g_WorldToTerrainTextureSpace` scales by 1 / 2175 and -1 / 1883.55 and
    /// moves by (0.5, -0.5); `WaterScale` is built from the same width (the number `W` of the
    /// formula), so the two agree on one world.
    #[test]
    fn the_world_of_the_capture_gives_its_texture_space_and_its_scale() {
        let m = OceanMap {
            half_world: Vec2::new(2175.0 / 2.0, 1883.55 / 2.0),
            ocean_height: OCEAN_HEIGHT,
            wrap_x: false,
        };
        let c = WaterConsts::new(&m, &WaterFrame::default(), 0.0);
        // `row_major`: element `j` of column `i` is component `i` of register `j`.
        let t = c.g_WorldToTerrainTextureSpace;
        let reg = |k: usize| [t[0][k], t[1][k], t[2][k], t[3][k]];
        assert!((reg(0)[0] - 0.0004597701).abs() < 1e-9, "{:?}", reg(0));
        assert!((reg(1)[1] + 0.0005309124).abs() < 1e-9, "{:?}", reg(1));
        assert_eq!((reg(3)[0], reg(3)[1]), (0.5, -0.5));
        assert_eq!((reg(0)[1], reg(1)[0]), (0.0, 0.0));
        // The width of the texture space is the width the scale of the layers is made from.
        assert!((c.WaterScale[0] * 2175.0 - 17.0).abs() < 1e-3);
        assert!((reg(0)[0] * 2175.0 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn the_projection_of_the_overlay_comes_from_the_frame_or_the_capture() {
        // Register `k` as `row_major_reg` of the shaders reads it: component `i` is `[i][k]`.
        let capture = WaterConsts::new(&map(), &WaterFrame::default(), 0.0).OverlayProjection;
        let reg = |m: [[f32; 4]; 4], k: usize| [m[0][k], m[1][k], m[2][k], m[3][k]];
        assert_eq!(reg(capture, 3), [-0.857143, 0.454545, 0.5, 1.0]);
        assert_eq!(reg(capture, 0), [0.00223214, 0.0, 0.0, 0.0]);
        let p = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0));
        let frame = WaterFrame {
            overlay_proj: Some(p),
            ..Default::default()
        };
        let c = WaterConsts::new(&map(), &frame, 0.0);
        assert_eq!(c.OverlayProjection, row_major(p));
        // The move is register 3, as the vertex shader adds it.
        assert_eq!(reg(c.OverlayProjection, 3), [1.0, 2.0, 3.0, 1.0]);
    }

    #[test]
    fn a_copy_moves_the_view_and_the_eye_together() {
        let frame = WaterFrame {
            eye: Vec3::new(10.0, 50.0, 20.0),
            ..Default::default()
        };
        let a = WaterConsts::new(&map(), &frame, 0.0);
        let b = WaterConsts::new(&map(), &frame, 2176.0);
        // The eye is seen from the copy: 2176 to the west of the map's point.
        assert!((a.v3CameraWorld[0] - b.v3CameraWorld[0] - 2176.0).abs() < 1e-3);
        assert_eq!(a.v3CameraWorld[1], b.v3CameraWorld[1]);
        assert_ne!(a.g_VP, b.g_VP);
        assert_eq!(copy_slot(0.0), 0);
        assert_eq!(copy_slot(-5.0), 1);
        assert_eq!(copy_slot(5.0), 2);
    }

    /// A copy of the map reads the overlay maps (grid, selection ring, mask of the fog), the shadow map and the
    /// reflection where it draws the point, not a world width away (the seam of the live report 2026-10-31).
    #[test]
    fn a_copy_reads_the_maps_of_the_screen_where_it_draws_the_point() {
        let overlay = Mat4::from_scale(Vec3::new(0.001, 0.002, 1.0))
            * Mat4::from_translation(Vec3::new(-300.0, 40.0, 0.0));
        let shadow = Mat4::from_cols_array(&[
            0.003, 0.0, 0.0, 0.0, 0.0, 0.002, 0.0001, 0.0, 0.0, 0.001, 0.0002, 0.0, -0.4, 0.1, 0.5,
            1.0,
        ]);
        let frame = WaterFrame {
            overlay_proj: Some(overlay),
            shadow_vp: shadow,
            reflect_vp: shadow * 2.0,
            ..Default::default()
        };
        // Register `k` (`row_major_reg`) is column `k` of the plain matrix: rebuild it and apply it as the shader does.
        let apply = |m: [[f32; 4]; 4], p: Vec3| {
            let plain = Mat4::from_cols_array_2d(&m).transpose();
            plain * p.extend(1.0)
        };
        let w = 4224.0;
        let p = Vec3::new(-1900.0, -420.0, OCEAN_HEIGHT);
        // Where the copy shifted by +W draws `p`, in the original's basis.
        let drawn = frame.ours_to_orig.transform_point3(
            frame.ours_to_orig.inverse().transform_point3(p) + Vec3::new(w, 0.0, 0.0),
        );
        let c = WaterConsts::new(&map(), &frame, w);
        for (got, plain) in [
            (c.OverlayProjection, overlay),
            (c.g_ShadowVP, shadow),
            (c.g_ReflectVP, shadow * 2.0),
        ] {
            let want = plain * drawn.extend(1.0);
            assert!(
                (apply(got, p) - want).length() < 1e-3,
                "{} {want}",
                apply(got, p)
            );
        }
        // The middle copy is unchanged.
        let m = WaterConsts::new(&map(), &frame, 0.0);
        assert_eq!(m.OverlayProjection, row_major(overlay));
        assert_eq!(m.g_ShadowVP, row_major(shadow));
    }

    #[test]
    fn the_quad_covers_the_map_with_the_height_texture_north_up() {
        let q = quad(Vec2::new(10.0, 5.0));
        // South-west corner: u 0, v 1. North-east corner: u 1, v 0.
        assert_eq!(q[0], [-10.0, -5.0, 0.0, 1.0]);
        assert_eq!(q[2], [10.0, 5.0, 1.0, 0.0]);
        assert_eq!(q[3], q[0]);
        assert_eq!(q[4], q[2]);
        assert_eq!(q[5], [-10.0, 5.0, 0.0, 0.0]);
    }

    #[test]
    fn the_height_map_is_averaged_flipped_and_has_one_level() {
        use crate::world::terrain::sampler::{TerrainFrame, TerrainSampler};
        let mut s = TerrainSampler::new(TerrainFrame::new(2, 1, false));
        // 128 x 64 texels: the south row is 0, the north row is 65535.
        for v in 0..64 {
            for u in 0..128 {
                s.field_mut()
                    .set_height(u, v, if v >= 32 { 65535 } else { 0 });
            }
        }
        let h = HeightMap::build(&s, 64);
        assert_eq!((h.width, h.height), (64, 32));
        assert_eq!(h.bytes.len(), 64 * 32, "one level, no mips (re-b16 O1b)");
        // Row 0 of the texture is the north: the top half is 255, the bottom 0.
        assert_eq!(h.bytes[0], 255);
        assert_eq!(h.bytes[(31 * 64) as usize], 0);
    }

    /// re-b16 O1: the byte of a texel is the high byte of the raw height, truncated (the seabed of a
    /// coast hex, 9766, is 38; the sea level 13000 is 50, not the rounded 51).
    #[test]
    fn the_byte_of_the_depth_is_the_high_byte_of_the_raw_height() {
        assert_eq!(height_byte(13000.0), 50);
        assert_eq!(height_byte(9766.0), 38);
        assert_eq!(height_byte(255.0), 0);
        assert_eq!(height_byte(65535.0), 255);
    }

    /// re-b16 O1: on High the depth is the field at full resolution, one texel per height sample, so
    /// the land of the shore is not averaged into the first texels of the water; the phone presets
    /// keep a coarser depth, and no preset asks for more than the device takes.
    #[test]
    fn the_depth_is_the_field_at_full_resolution_on_high() {
        // A standard map: 80 x 52 hexes, 5120 x 3392 texels with the north rows of the world.
        let (fw, fh) = (80 * 64u32, 52 * 64 + 64u32);
        let f_of = |side: u32| fw.max(fh).div_ceil(side);
        assert_eq!(f_of(height_max_side(2, 16384)), 1);
        assert_eq!(f_of(height_max_side(1, 16384)), 2);
        assert_eq!(f_of(height_max_side(0, 16384)), 3);
        assert_eq!(height_max_side(2, 4096), 4096);
        // A step from the land (15340) to the coast's floor (9766) one texel wide stays one texel wide.
        let raw = |u: i32, _v: i32| if u < 10 { 15340.0 } else { 9766.0 };
        let full = HeightState::build(40, 8, height_max_side(2, 16384), 0, &raw);
        assert_eq!(full.texels_per_texel(), 1);
        assert_eq!(&full.map.bytes[8..12], &[59, 59, 38, 38]);
        // Averaged three to one, the land reaches into the first texel of the water.
        let coarse = HeightState::build(40, 8, 14, 0, &raw);
        assert_eq!(coarse.texels_per_texel(), 3);
        assert_eq!(
            coarse.map.bytes[3],
            ((15340.0 + 2.0 * 9766.0) / 3.0 / 256.0) as u8
        );
    }
}
