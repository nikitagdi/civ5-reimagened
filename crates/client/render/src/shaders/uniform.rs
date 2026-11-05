//! Uniform blocks named like the constants of the original.
//!
//! A field of a block is called as the constant is in the CTAB of the shader (`g_ShadowVP`,
//! `f3FogColor`). That is needed for the comparison with the oracle (`shader-re`), for the replay
//! tests (`golden`) and for [`ConstantOverrides`]. [`uniform_block!`] makes the `#[repr(C)]`
//! struct and a table of its fields, and checks at compile time that the Rust offsets are those
//! WGSL's uniform layout gives: a `vec3<f32>` sits on a multiple of 16, a `vec4<f32>` and a matrix
//! too, and the struct ends on a multiple of 16. A block that breaks this does not compile; the
//! derive of `Pod` already refuses implicit padding, so a gap has to be written as a field.

use std::collections::HashMap;

use bytemuck::Pod;

/// A Rust type that has a uniform layout in WGSL.
pub trait WgslType {
    /// `AlignOf` in WGSL's uniform address space.
    const ALIGN: usize;
    /// `SizeOf`.
    const SIZE: usize;
    /// The type as WGSL spells it.
    const NAME: &'static str;
}

macro_rules! wgsl_type {
    ($ty:ty, $align:expr, $size:expr, $name:expr) => {
        impl WgslType for $ty {
            const ALIGN: usize = $align;
            const SIZE: usize = $size;
            const NAME: &'static str = $name;
        }
    };
}

wgsl_type!(f32, 4, 4, "f32");
wgsl_type!(i32, 4, 4, "i32");
wgsl_type!(u32, 4, 4, "u32");
wgsl_type!([f32; 2], 8, 8, "vec2<f32>");
wgsl_type!([f32; 3], 16, 12, "vec3<f32>");
wgsl_type!([f32; 4], 16, 16, "vec4<f32>");
wgsl_type!([i32; 4], 16, 16, "vec4<i32>");
wgsl_type!([u32; 4], 16, 16, "vec4<u32>");

/// Rows of vec4: `[[f32; 4]; 4]` is a `mat4x4<f32>` (columns as the array has them), other counts
/// are `array<vec4<f32>, N>` (constant arrays of the packages, stride 16).
impl<const N: usize> WgslType for [[f32; 4]; N] {
    const ALIGN: usize = 16;
    const SIZE: usize = 16 * N;
    const NAME: &'static str = if N == 4 {
        "mat4x4<f32>"
    } else {
        "array<vec4<f32>, N>"
    };
}

/// One field of a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FieldDesc {
    /// Name of the constant in the CTAB.
    pub name: &'static str,
    /// Byte offset in the Rust struct.
    pub offset: usize,
    pub size: usize,
    /// WGSL alignment of the field's type.
    pub align: usize,
    pub ty: &'static str,
}

/// What is wrong with the layout of a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutError {
    /// Field `field` is not on a multiple of its WGSL alignment.
    Misaligned { field: usize },
    /// Field `field` starts inside the one before it.
    Overlaps { field: usize },
    /// The struct does not end on a multiple of 16.
    Size { total: usize },
}

/// Struct alignment of every uniform block: WGSL rounds a uniform struct up to 16 and WebGL2
/// binds whole vec4s.
pub const BLOCK_ALIGN: usize = 16;

/// The first thing wrong with `fields` of a struct of `total` bytes, if anything. `const`, so the
/// macro checks at compile time.
pub const fn layout_error(fields: &[FieldDesc], total: usize) -> Option<LayoutError> {
    let mut end = 0;
    let mut i = 0;
    while i < fields.len() {
        let f = &fields[i];
        if !f.offset.is_multiple_of(f.align) {
            return Some(LayoutError::Misaligned { field: i });
        }
        if f.offset < end {
            return Some(LayoutError::Overlaps { field: i });
        }
        end = f.offset + f.size;
        i += 1;
    }
    if !total.is_multiple_of(BLOCK_ALIGN) || total < end {
        return Some(LayoutError::Size { total });
    }
    None
}

/// A struct made by [`uniform_block!`].
pub trait UniformBlock: Pod {
    const FIELDS: &'static [FieldDesc];

    /// The field called `name` (the CTAB name).
    fn field(name: &str) -> Option<&'static FieldDesc> {
        Self::FIELDS.iter().find(|f| f.name == name)
    }
}

/// Makes a `#[repr(C)]` uniform struct with CTAB-named fields, its [`UniformBlock`] table, and a
/// compile-time check of the WGSL layout.
///
/// ```ignore
/// uniform_block! {
///     /// `pShadowGlobals`
///     pub struct ShadowGlobals {
///         pub g_ShadowVP: [[f32; 4]; 4],
///         pub f3Tint: [f32; 3],
///         pub g_ShadowFade: f32,
///     }
/// }
/// ```
#[macro_export]
macro_rules! uniform_block {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident {
            $( $(#[$fmeta:meta])* $fvis:vis $field:ident : $ty:ty ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[repr(C)]
        #[allow(non_snake_case)]
        #[derive(Clone, Copy, Debug, PartialEq, ::bytemuck::Pod, ::bytemuck::Zeroable)]
        $vis struct $name {
            $( $(#[$fmeta])* $fvis $field: $ty ),+
        }

        impl $crate::shaders::UniformBlock for $name {
            const FIELDS: &'static [$crate::shaders::FieldDesc] = &[
                $( $crate::shaders::FieldDesc {
                    name: stringify!($field),
                    offset: ::core::mem::offset_of!($name, $field),
                    size: <$ty as $crate::shaders::WgslType>::SIZE,
                    align: <$ty as $crate::shaders::WgslType>::ALIGN,
                    ty: <$ty as $crate::shaders::WgslType>::NAME,
                } ),+
            ];
        }

        const _: () = assert!(
            $crate::shaders::layout_error(
                <$name as $crate::shaders::UniformBlock>::FIELDS,
                ::core::mem::size_of::<$name>(),
            )
            .is_none(),
            concat!(
                "uniform block ",
                stringify!($name),
                ": a field is off the WGSL alignment, overlaps, or the size is not a multiple of 16 \
                 (see shaders::layout_error)"
            )
        );
    };
}

/// Values that replace constants of the blocks when they are uploaded (spec 4.6 "Подмена
/// констант"): the time and the cloud scrolls of a capture, for `golden`, or a hand-set value in a
/// debugging session. Floats only: `values` fill the field's bytes from its start.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConstantOverrides {
    values: HashMap<String, Vec<f32>>,
}

impl ConstantOverrides {
    pub fn new(values: HashMap<String, Vec<f32>>) -> Self {
        Self { values }
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn get(&self, name: &str) -> Option<&[f32]> {
        self.values.get(name).map(Vec::as_slice)
    }

    /// Writes the overridden constants into `block`; returns how many fields were changed. A
    /// field takes as many values as it has room for; extra values are ignored, a shorter list
    /// leaves the rest of the field as it was.
    pub fn apply<T: UniformBlock>(&self, block: &mut T) -> usize {
        if self.values.is_empty() {
            return 0;
        }
        let bytes = bytemuck::bytes_of_mut(block);
        let mut changed = 0;
        for field in T::FIELDS {
            let Some(values) = self.values.get(field.name) else {
                continue;
            };
            let room = (field.size / 4).min(values.len());
            if room == 0 {
                continue;
            }
            let dst = &mut bytes[field.offset..field.offset + room * 4];
            dst.copy_from_slice(bytemuck::cast_slice(&values[..room]));
            changed += 1;
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    uniform_block! {
        /// A block like `pShadowGlobals`: a matrix, a vec3 followed by a scalar (they share a
        /// vec4), a vec4, two vec2.
        pub struct ShadowGlobals {
            pub g_ShadowVP: [[f32; 4]; 4],
            pub f3FogColor: [f32; 3],
            pub g_ShadowFade: f32,
            pub g_FogConsts: [f32; 4],
            pub pad: [f32; 2],
            pub v2Scroll: [f32; 2],
        }
    }

    #[test]
    fn the_table_has_the_ctab_names_and_wgsl_offsets() {
        let names: Vec<_> = ShadowGlobals::FIELDS.iter().map(|f| f.name).collect();
        assert_eq!(
            names,
            [
                "g_ShadowVP",
                "f3FogColor",
                "g_ShadowFade",
                "g_FogConsts",
                "pad",
                "v2Scroll"
            ]
        );
        let at = |n| ShadowGlobals::field(n).unwrap();
        assert_eq!((at("g_ShadowVP").offset, at("g_ShadowVP").size), (0, 64));
        assert_eq!(at("f3FogColor").offset, 64);
        // A scalar may follow a vec3 inside its 16 bytes.
        assert_eq!(at("g_ShadowFade").offset, 76);
        assert_eq!(at("g_FogConsts").offset, 80);
        assert_eq!(at("v2Scroll").offset, 104);
        assert_eq!(at("g_ShadowVP").ty, "mat4x4<f32>");
        assert_eq!(std::mem::size_of::<ShadowGlobals>(), 112);
        assert_eq!(layout_error(ShadowGlobals::FIELDS, 112), None);
    }

    fn field(name: &'static str, offset: usize, size: usize, align: usize) -> FieldDesc {
        FieldDesc {
            name,
            offset,
            size,
            align,
            ty: "",
        }
    }

    /// What `uniform_block!` turns into a compile error is found by the same function.
    #[test]
    fn wrong_alignment_overlap_and_size_are_found() {
        // A vec3 on offset 4 (after a lone f32): WGSL puts it on 16.
        let bad = [field("a", 0, 4, 4), field("v", 4, 12, 16)];
        assert_eq!(
            layout_error(&bad, 16),
            Some(LayoutError::Misaligned { field: 1 })
        );
        // A vec4 that starts before the end of the previous field.
        let overlap = [field("m", 0, 64, 16), field("v", 48, 16, 16)];
        assert_eq!(
            layout_error(&overlap, 64),
            Some(LayoutError::Overlaps { field: 1 })
        );
        // Ends on 12, not a multiple of 16.
        let short = [
            field("a", 0, 4, 4),
            field("b", 4, 4, 4),
            field("c", 8, 4, 4),
        ];
        assert_eq!(
            layout_error(&short, 12),
            Some(LayoutError::Size { total: 12 })
        );
        // A good one.
        let good = [field("v", 0, 12, 16), field("s", 12, 4, 4)];
        assert_eq!(layout_error(&good, 16), None);
        assert_eq!(layout_error(&[], 0), None);
    }

    #[test]
    fn overrides_replace_fields_by_ctab_name() {
        let mut block: ShadowGlobals = bytemuck::Zeroable::zeroed();
        block.g_ShadowFade = 1.0;
        let overrides = ConstantOverrides::new(HashMap::from([
            ("g_ShadowFade".to_owned(), vec![0.25]),
            ("f3FogColor".to_owned(), vec![0.1, 0.2, 0.3, 0.4, 0.5]),
            ("v2Scroll".to_owned(), vec![7.0]),
            ("NotAConstant".to_owned(), vec![1.0]),
        ]));
        assert_eq!(overrides.apply(&mut block), 3);
        assert_eq!(block.g_ShadowFade, 0.25);
        // The vec3 takes three of the five values.
        assert_eq!(block.f3FogColor, [0.1, 0.2, 0.3]);
        // One value into a vec2: the second stays as it was.
        assert_eq!(block.v2Scroll, [7.0, 0.0]);
        assert_eq!(overrides.get("g_ShadowFade"), Some(&[0.25][..]));
        assert_eq!(ConstantOverrides::default().apply(&mut block), 0);
    }
}
