//! The GPU instance: one drawable primitive, 96 bytes, the vertex buffer element of the UI v2 pass.
//!
//! The format is complete from the start: later primitives (gradients, shadows, images) fill fields that exist
//! already. The layout is fixed by [`ATTRIBUTES`] and checked by the tests; the vertex shader reads it as
//! 12 instance attributes.
//!
//! | Offset | Attribute | Format | Field |
//! |---|---|---|---|
//! | 0 | 0 | Float32x4 | `rect`: x, y, w, h in local pixels |
//! | 16 | 1 | Float16x4 | `radii`: tl, tr, br, bl |
//! | 24 | 2 | Float16x4 | `border`: top, right, bottom, left widths |
//! | 32 | 3 | Float32x4 | `uv`: sprite rectangle (u0, v0, u1, v1), or gradient end points |
//! | 48 | 4-7 | Unorm8x4 x4 | `c0..c3`: fill colour or gradient stop colours, tint |
//! | 64 | 8 | Unorm8x4 | `border_color` |
//! | 68 | 9 | Float16x4 | `stops`: gradient stop positions |
//! | 76 | 10 | Float16x4 | `extra`: shadow sigma, spread, ellipse ratio, repeat period |
//! | 84 | 11 | Uint32x3 | `kind:8 flags:8 atlas_layer:8 n_stops:8`, `xform:16 clip:16`, `anchor:16 ext:16` |
//!
//! Colours are straight sRGB bytes. `xform` indexes the transform table (0 = identity), `clip` the clip table
//! (0 = none), `anchor` the anchor records of the transform table (0 = screen), `ext` an external texture slot.

use bytemuck::{Pod, Zeroable};

/// Primitive kind: a box (fill, border, rounded corners).
pub const KIND_BOX: u8 = 0;
/// Primitive kind: a blurred shadow of a rounded rectangle. `rect` and `radii` are the element's border box,
/// `uv.xy` the shadow offset, `extra.x` the Gaussian's standard deviation, `extra.y` the spread, `c0` the colour.
/// The quad covers the shadow and its blur; nothing is drawn inside the element's own box.
pub const KIND_SHADOW: u8 = 1;
/// Primitive kind: an RGBA sprite or external texture.
pub const KIND_IMAGE: u8 = 2;
/// Primitive kind: a coverage mask (glyph, icon) times a colour.
pub const KIND_MASK: u8 = 3;

/// Flags bits 0-1: how the fill is computed. Solid: `c0`.
///
/// The gradient modes take `n_stops` (2 to 4) straight colours from `c0..c3` and their positions (0 to 1, in
/// order) from `stops`; coordinates are local to the box (or to its tile), `(0, 0)` at the top left corner.
pub const FILL_SOLID: u8 = 0;
/// Fill mode: linear gradient, position 0 at `uv.xy` and 1 at `uv.zw`; outside it the end colours.
pub const FILL_LINEAR: u8 = 1;
/// Fill mode: repeating linear gradient, `uv` as [`FILL_LINEAR`]; the position wraps around 1.
pub const FILL_REPEAT: u8 = 2;
/// Fill mode: conic gradient around the centre `uv.xy`. Position = `fract(turns / uv.w)`, where `turns` is the
/// angle clockwise from `uv.z` (turns from the top) as a fraction of a turn; `uv.w` is 1 for a plain gradient
/// and the span of the pattern for a repeating one.
pub const FILL_CONIC: u8 = 3;
/// Mask of the fill mode bits of `flags`.
pub const FILL_MODE_MASK: u8 = 0b11;
/// Flag: the fill is drawn (a border-only box leaves it off).
pub const FLAG_FILL: u8 = 1 << 2;
/// Flag: the corner radii are horizontal radii of ellipses whose vertical radii are `extra.z` (ry / rx) times
/// larger.
pub const FLAG_ELLIPSE: u8 = 1 << 4;
/// Flag: the border is drawn.
pub const FLAG_BORDER: u8 = 1 << 3;
/// Flag: the gradient is drawn in tiles of `extra.xy` px starting at the box's top left corner.
pub const FLAG_TILE: u8 = 1 << 5;

/// A drawable primitive. See the module docs for the byte layout.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct Ui2Instance {
    /// x, y, w, h in local pixels.
    pub rect: [f32; 4],
    /// Corner radii as IEEE half floats: tl, tr, br, bl.
    pub radii: [u16; 4],
    /// Border widths as half floats: top, right, bottom, left.
    pub border: [u16; 4],
    /// Sprite rectangle or gradient points.
    pub uv: [f32; 4],
    /// Fill colour / stop colours / tint, straight sRGB RGBA.
    pub colors: [[u8; 4]; 4],
    /// Border colour.
    pub border_color: [u8; 4],
    /// Gradient stop positions as half floats.
    pub stops: [u16; 4],
    /// Shadow sigma, spread, ellipse ratio, repeat period as half floats.
    pub extra: [u16; 4],
    /// Primitive kind (`KIND_*`).
    pub kind: u8,
    /// Flags (`FILL_*`, `FLAG_*`).
    pub flags: u8,
    /// Texture array layer of the sprite or mask.
    pub atlas_layer: u8,
    /// Number of gradient stops.
    pub n_stops: u8,
    /// Element transform: local to chunk space (0 = identity).
    pub xform: u16,
    /// Clip, in chunk space (0 = none).
    pub clip: u16,
    /// Chunk transform (anchor): chunk space to screen (0 = screen).
    pub anchor: u16,
    /// External texture slot (0 = none).
    pub ext: u16,
}

/// Size of an instance in bytes.
pub const INSTANCE_SIZE: usize = 96;

const _: () = assert!(core::mem::size_of::<Ui2Instance>() == INSTANCE_SIZE);
const _: () = assert!(core::mem::align_of::<Ui2Instance>() == 4);

/// Format of an instance attribute (the `wgpu::VertexFormat` of the same name).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttrFormat {
    /// Four 32-bit floats.
    Float32x4,
    /// Four 16-bit floats.
    Float16x4,
    /// Four 8-bit normalized unsigned integers.
    Unorm8x4,
    /// Three 32-bit unsigned integers.
    Uint32x3,
}

/// One instance attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attr {
    /// `@location` in the shader.
    pub location: u32,
    /// Byte offset in the instance.
    pub offset: u32,
    /// Format.
    pub format: AttrFormat,
}

const fn attr(location: u32, offset: u32, format: AttrFormat) -> Attr {
    Attr {
        location,
        offset,
        format,
    }
}

/// The 12 instance attributes, in location order.
pub const ATTRIBUTES: [Attr; 12] = [
    attr(0, 0, AttrFormat::Float32x4),
    attr(1, 16, AttrFormat::Float16x4),
    attr(2, 24, AttrFormat::Float16x4),
    attr(3, 32, AttrFormat::Float32x4),
    attr(4, 48, AttrFormat::Unorm8x4),
    attr(5, 52, AttrFormat::Unorm8x4),
    attr(6, 56, AttrFormat::Unorm8x4),
    attr(7, 60, AttrFormat::Unorm8x4),
    attr(8, 64, AttrFormat::Unorm8x4),
    attr(9, 68, AttrFormat::Float16x4),
    attr(10, 76, AttrFormat::Float16x4),
    attr(11, 84, AttrFormat::Uint32x3),
];

impl Ui2Instance {
    /// A zeroed instance of `kind`.
    pub fn of_kind(kind: u8) -> Self {
        Self {
            kind,
            ..Self::zeroed()
        }
    }

    /// Sets the radii.
    pub fn set_radii(&mut self, r: [f32; 4]) {
        self.radii = r.map(f16_bits);
    }

    /// Sets the border widths.
    pub fn set_border(&mut self, b: [f32; 4]) {
        self.border = b.map(f16_bits);
    }

    /// The instance as bytes.
    pub fn as_bytes(&self) -> &[u8] {
        bytemuck::bytes_of(self)
    }
}

/// Converts an `f32` to IEEE 754 half precision bits: round to nearest even, overflow to infinity, subnormals
/// kept, NaN kept as NaN.
pub fn f16_bits(v: f32) -> u16 {
    let x = v.to_bits();
    let sign = ((x >> 16) & 0x8000) as u16;
    let exp = ((x >> 23) & 0xff) as i32;
    let man = x & 0x7f_ffff;
    if exp == 0xff {
        return sign | 0x7c00 | if man != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = man | 0x80_0000;
        let shift = (14 - e) as u32;
        let mut r = m >> shift;
        let rem = m & ((1u32 << shift) - 1);
        let halfway = 1u32 << (shift - 1);
        if rem > halfway || (rem == halfway && (r & 1) == 1) {
            r += 1;
        }
        return sign | r as u16;
    }
    let mut r = ((e as u32) << 10) | (man >> 13);
    let rem = man & 0x1fff;
    if rem > 0x1000 || (rem == 0x1000 && (r & 1) == 1) {
        r += 1;
    }
    sign | r as u16
}

/// Converts half precision bits to `f32` (exact).
pub fn f16_to_f32(h: u16) -> f32 {
    let sign = u32::from(h & 0x8000) << 16;
    let exp = u32::from((h >> 10) & 0x1f);
    let man = u32::from(h & 0x3ff);
    let bits = match (exp, man) {
        (0, 0) => sign,
        (0, m) => {
            // Subnormal: normalize.
            let shift = m.leading_zeros() - 21;
            let m = (m << shift) & 0x3ff;
            let e = 113 - shift;
            sign | (e << 23) | (m << 13)
        }
        (0x1f, 0) => sign | 0x7f80_0000,
        (0x1f, m) => sign | 0x7f80_0000 | (m << 13),
        (e, m) => sign | ((e + 112) << 23) | (m << 13),
    };
    f32::from_bits(bits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::offset_of;

    #[test]
    fn layout_matches_the_attribute_table() {
        assert_eq!(offset_of!(Ui2Instance, rect), 0);
        assert_eq!(offset_of!(Ui2Instance, radii), 16);
        assert_eq!(offset_of!(Ui2Instance, border), 24);
        assert_eq!(offset_of!(Ui2Instance, uv), 32);
        assert_eq!(offset_of!(Ui2Instance, colors), 48);
        assert_eq!(offset_of!(Ui2Instance, border_color), 64);
        assert_eq!(offset_of!(Ui2Instance, stops), 68);
        assert_eq!(offset_of!(Ui2Instance, extra), 76);
        assert_eq!(offset_of!(Ui2Instance, kind), 84);
        assert_eq!(offset_of!(Ui2Instance, flags), 85);
        assert_eq!(offset_of!(Ui2Instance, atlas_layer), 86);
        assert_eq!(offset_of!(Ui2Instance, n_stops), 87);
        assert_eq!(offset_of!(Ui2Instance, xform), 88);
        assert_eq!(offset_of!(Ui2Instance, clip), 90);
        assert_eq!(offset_of!(Ui2Instance, anchor), 92);
        assert_eq!(offset_of!(Ui2Instance, ext), 94);
        assert_eq!(core::mem::size_of::<Ui2Instance>(), 96);
        // Attribute 11 reads the last 12 bytes as three u32 words.
        assert_eq!(ATTRIBUTES[11].offset as usize + 12, INSTANCE_SIZE);
        for (i, a) in ATTRIBUTES.iter().enumerate() {
            assert_eq!(a.location as usize, i);
        }
    }

    #[test]
    fn meta_words_read_back_as_the_documented_u32s() {
        let mut i = Ui2Instance::of_kind(KIND_MASK);
        i.flags = 0x12;
        i.atlas_layer = 0x34;
        i.n_stops = 0x56;
        i.xform = 0x1234;
        i.clip = 0x5678;
        i.anchor = 0x9abc;
        i.ext = 0xdef0;
        let b = i.as_bytes();
        let w = |n: usize| u32::from_le_bytes(b[84 + 4 * n..88 + 4 * n].try_into().unwrap());
        assert_eq!(w(0), 3 | (0x12 << 8) | (0x34 << 16) | (0x56 << 24));
        assert_eq!(w(1), 0x1234 | (0x5678 << 16));
        assert_eq!(w(2), 0x9abc | (0xdef0 << 16));
    }

    #[test]
    fn half_float_known_values() {
        for (v, bits) in [
            (0.0f32, 0x0000u16),
            (-0.0, 0x8000),
            (1.0, 0x3c00),
            (-1.0, 0xbc00),
            (0.5, 0x3800),
            (2.0, 0x4000),
            (3.0, 0x4200),
            (24.0, 0x4e00),
            (65504.0, 0x7bff),
            (1.0 / 3.0, 0x3555),
            (100_000.0, 0x7c00),
            (f32::NEG_INFINITY, 0xfc00),
            (6.103_515_6e-5, 0x0400),
            (5.960_464_5e-8, 0x0001),
            (1e-9, 0x0000),
        ] {
            assert_eq!(f16_bits(v), bits, "{v}");
        }
        assert_eq!(f16_bits(f32::NAN) & 0x7c00, 0x7c00);
        assert_ne!(f16_bits(f32::NAN) & 0x03ff, 0);
    }

    #[test]
    fn half_float_round_trips_every_half() {
        for h in 0..=u16::MAX {
            let f = f16_to_f32(h);
            if f.is_nan() {
                assert_eq!(f16_bits(f) & 0x7c00, 0x7c00);
            } else {
                assert_eq!(f16_bits(f), h, "{h:#06x}");
            }
        }
    }

    #[test]
    fn half_float_rounds_to_nearest_even() {
        // 1 + 2^-11 is halfway between 1.0 and 1 + 2^-10: ties to even (1.0).
        assert_eq!(f16_bits(1.0 + 2f32.powi(-11)), 0x3c00);
        // 1 + 3 * 2^-11 is halfway between 1 + 2^-10 and 1 + 2^-9: ties to even (the latter).
        assert_eq!(f16_bits(1.0 + 3.0 * 2f32.powi(-11)), 0x3c02);
        // Just above halfway rounds up.
        assert_eq!(f16_bits(1.0 + 2f32.powi(-11) + 2f32.powi(-20)), 0x3c01);
    }
}
