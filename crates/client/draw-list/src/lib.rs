//! UI draw list: the shared language between `ui` and `render` (rule R6).
//! Physical pixels, Y down; sRGB RGBA8, non-premultiplied.

use bytemuck::{Pod, Zeroable};
use glam::Vec2;
use std::sync::atomic::{AtomicU64, Ordering};

/// UI vertex. Shader layout: `pos` at 0, `uv` at 8, `color` (`unorm8x4`) at 16; stride 20.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct UiVertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: [u8; 4],
}

impl UiVertex {
    pub const STRIDE: u64 = std::mem::size_of::<UiVertex>() as u64;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TextureId(pub u64);

impl TextureId {
    pub const WHITE: TextureId = TextureId(0);
    /// Marks ids owned by the renderer (minimap and similar) rather than by asset streaming.
    const EXTERNAL_BIT: u64 = 1 << 63;
    /// The minimap render target; the renderer resolves it per frame.
    pub const MINIMAP: TextureId = TextureId::external(1);

    /// A renderer-owned texture handle; `n` must not use the top bit.
    pub const fn external(n: u64) -> TextureId {
        debug_assert!(
            n & Self::EXTERNAL_BIT == 0,
            "external id overflows into the marker bit"
        );
        TextureId(Self::EXTERNAL_BIT | n)
    }

    pub const fn is_external(self) -> bool {
        self.0 & Self::EXTERNAL_BIT != 0
    }
}

/// Blend mode; the slots mirror the exe's 4-entry UIQuad blend table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum UiBlend {
    /// Table slot 0: `src*a + dst*(1-a)`.
    #[default]
    Alpha,
    /// Table slot 1; meaning not recovered yet.
    Slot1,
    /// Table slot 2; the exe selects it for `MaskTexture`.
    Slot2,
    /// Table slot 3; meaning not recovered yet.
    Slot3,
}

impl UiBlend {
    /// Index into the exe's blend table.
    pub const fn slot(self) -> u8 {
        self as u8
    }
}

/// Texture sampling mode (the exe's sampler 10 default, 5 for `Sampler="P..."`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum UiSampler {
    #[default]
    Linear,
    Point,
}

/// Fragment shading variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum UiShade {
    #[default]
    Plain,
    /// `texture2` multiplies the alpha.
    Masked,
    /// `texture2` is the palette; `texture` holds the indices.
    Indexed,
}

/// Per-command state beyond texture, clip and blend.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UiMaterial {
    pub sampler: UiSampler,
    pub shade: UiShade,
    /// Mask or palette; `TextureId::WHITE` when unused.
    pub texture2: TextureId,
    /// UV of `texture2` from local quad pixels: scale xy, offset zw.
    pub tex2_xform: [f32; 4],
}

impl UiMaterial {
    pub const PLAIN: UiMaterial = UiMaterial {
        sampler: UiSampler::Linear,
        shade: UiShade::Plain,
        texture2: TextureId::WHITE,
        tex2_xform: [1.0, 1.0, 0.0, 0.0],
    };
}

impl Default for UiMaterial {
    fn default() -> Self {
        Self::PLAIN
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClipRect {
    pub min: Vec2,
    pub max: Vec2,
}

impl ClipRect {
    pub const NONE: ClipRect = ClipRect {
        min: Vec2::splat(f32::MIN),
        max: Vec2::splat(f32::MAX),
    };

    pub fn intersect(self, other: ClipRect) -> ClipRect {
        let min = self.min.max(other.min);
        let max = self.max.min(other.max).max(min);
        ClipRect { min, max }
    }

    pub fn is_empty(self) -> bool {
        self.max.x <= self.min.x || self.max.y <= self.min.y
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawCmd {
    pub texture: TextureId,
    pub clip: ClipRect,
    pub blend: UiBlend,
    pub sampler: UiSampler,
    pub shade: UiShade,
    pub texture2: TextureId,
    pub tex2_xform: [f32; 4],
    pub first_index: u32,
    pub index_count: u32,
}

/// Source of revision numbers: shared by all lists, so one value means one content everywhere.
static NEXT_REVISION: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Default)]
pub struct UiDrawList {
    vertices: Vec<UiVertex>,
    indices: Vec<u32>,
    commands: Vec<DrawCmd>,
    clip_stack: Vec<ClipRect>,
    blend_stack: Vec<UiBlend>,
    revision: u64,
}

impl UiDrawList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.vertices.clear();
        self.indices.clear();
        self.commands.clear();
        self.clip_stack.clear();
        self.blend_stack.clear();
        self.revision = NEXT_REVISION.fetch_add(1, Ordering::Relaxed);
    }

    /// Grows on every `clear` and every accepted `add_*`; equal values mean equal content, also
    /// between different lists (a new list starts at 0, which is the empty content).
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn vertices(&self) -> &[UiVertex] {
        &self.vertices
    }

    pub fn indices(&self) -> &[u32] {
        &self.indices
    }

    pub fn commands(&self) -> &[DrawCmd] {
        &self.commands
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    pub fn current_clip(&self) -> ClipRect {
        self.clip_stack.last().copied().unwrap_or(ClipRect::NONE)
    }

    pub fn push_clip(&mut self, clip: ClipRect) {
        let clip = self.current_clip().intersect(clip);
        self.clip_stack.push(clip);
    }

    pub fn pop_clip(&mut self) {
        self.clip_stack.pop();
    }

    pub fn current_blend(&self) -> UiBlend {
        self.blend_stack.last().copied().unwrap_or_default()
    }

    /// The parent sets the mode for its children until the matching `pop_blend`.
    pub fn push_blend(&mut self, blend: UiBlend) {
        self.blend_stack.push(blend);
    }

    pub fn pop_blend(&mut self) {
        self.blend_stack.pop();
    }

    /// Plain material; see [`Self::add_triangles_with`].
    pub fn add_triangles(&mut self, texture: TextureId, vertices: &[UiVertex], indices: &[u32]) {
        self.add_triangles_with(texture, &UiMaterial::PLAIN, vertices, indices);
    }

    /// Consecutive calls with the same texture, clip, blend and material merge; bad indices hit `debug_assert`.
    pub fn add_triangles_with(
        &mut self,
        texture: TextureId,
        material: &UiMaterial,
        vertices: &[UiVertex],
        indices: &[u32],
    ) {
        debug_assert!(
            indices.len().is_multiple_of(3),
            "index count is not a multiple of 3"
        );
        debug_assert!(
            indices.iter().all(|&i| (i as usize) < vertices.len()),
            "index out of vertex range"
        );
        let clip = self.current_clip();
        if clip.is_empty() || indices.is_empty() {
            return;
        }
        let blend = self.current_blend();
        self.revision = NEXT_REVISION.fetch_add(1, Ordering::Relaxed);
        let base = self.vertices.len() as u32;
        let first_index = self.indices.len() as u32;
        self.vertices.extend_from_slice(vertices);
        self.indices.extend(indices.iter().map(|&i| base + i));
        let count = indices.len() as u32;

        match self.commands.last_mut() {
            Some(last)
                if last.texture == texture
                    && last.clip == clip
                    && last.blend == blend
                    && last.sampler == material.sampler
                    && last.shade == material.shade
                    && last.texture2 == material.texture2
                    && last.tex2_xform == material.tex2_xform
                    && last.first_index + last.index_count == first_index =>
            {
                last.index_count += count;
            }
            _ => self.commands.push(DrawCmd {
                texture,
                clip,
                blend,
                sampler: material.sampler,
                shade: material.shade,
                texture2: material.texture2,
                tex2_xform: material.tex2_xform,
                first_index,
                index_count: count,
            }),
        }
    }

    pub fn add_rect(
        &mut self,
        texture: TextureId,
        min: Vec2,
        max: Vec2,
        uv_min: Vec2,
        uv_max: Vec2,
        color: [u8; 4],
    ) {
        let v = |p: Vec2, uv: Vec2| UiVertex {
            pos: p.to_array(),
            uv: uv.to_array(),
            color,
        };
        let vertices = [
            v(min, uv_min),
            v(Vec2::new(max.x, min.y), Vec2::new(uv_max.x, uv_min.y)),
            v(max, uv_max),
            v(Vec2::new(min.x, max.y), Vec2::new(uv_min.x, uv_max.y)),
        ];
        self.add_triangles(texture, &vertices, &[0, 1, 2, 0, 2, 3]);
    }

    pub fn add_filled_rect(&mut self, min: Vec2, max: Vec2, color: [u8; 4]) {
        self.add_rect(TextureId::WHITE, min, max, Vec2::ZERO, Vec2::ONE, color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertex_layout() {
        assert_eq!(UiVertex::STRIDE, 20);
        let v = UiVertex {
            pos: [1.0, 2.0],
            uv: [0.0, 1.0],
            color: [255, 0, 0, 255],
        };
        assert_eq!(bytemuck::bytes_of(&v).len(), 20);
    }

    #[test]
    fn merges_same_state() {
        let mut dl = UiDrawList::new();
        dl.add_filled_rect(Vec2::ZERO, Vec2::splat(10.0), [255; 4]);
        dl.add_filled_rect(Vec2::splat(10.0), Vec2::splat(20.0), [0, 0, 0, 255]);
        assert_eq!(dl.commands().len(), 1);
        assert_eq!(dl.commands()[0].index_count, 12);
        assert_eq!(dl.vertices().len(), 8);
        assert_eq!(&dl.indices()[6..], &[4, 5, 6, 4, 6, 7]);
    }

    #[test]
    fn splits_on_texture_and_clip() {
        let mut dl = UiDrawList::new();
        dl.add_filled_rect(Vec2::ZERO, Vec2::ONE, [255; 4]);
        dl.add_rect(
            TextureId(7),
            Vec2::ZERO,
            Vec2::ONE,
            Vec2::ZERO,
            Vec2::ONE,
            [255; 4],
        );
        dl.push_clip(ClipRect {
            min: Vec2::ZERO,
            max: Vec2::splat(5.0),
        });
        dl.add_rect(
            TextureId(7),
            Vec2::ZERO,
            Vec2::ONE,
            Vec2::ZERO,
            Vec2::ONE,
            [255; 4],
        );
        dl.pop_clip();
        let cmds = dl.commands();
        assert_eq!(cmds.len(), 3);
        assert_eq!(cmds[1].clip, ClipRect::NONE);
        assert_eq!(cmds[2].clip.max, Vec2::splat(5.0));
        assert_eq!(cmds[2].first_index, 12);
    }

    #[test]
    fn nested_clip_intersects_and_empty_clip_skips() {
        let mut dl = UiDrawList::new();
        dl.push_clip(ClipRect {
            min: Vec2::ZERO,
            max: Vec2::splat(10.0),
        });
        dl.push_clip(ClipRect {
            min: Vec2::splat(20.0),
            max: Vec2::splat(30.0),
        });
        assert!(dl.current_clip().is_empty());
        dl.add_filled_rect(Vec2::ZERO, Vec2::ONE, [255; 4]);
        assert!(dl.is_empty());
        dl.pop_clip();
        dl.pop_clip();
        dl.pop_clip();
        assert_eq!(dl.current_clip(), ClipRect::NONE);
    }

    fn quad(dl: &mut UiDrawList, material: &UiMaterial) {
        let v = |x: f32, y: f32| UiVertex {
            pos: [x, y],
            uv: [0.0; 2],
            color: [255; 4],
        };
        let vertices = [v(0.0, 0.0), v(1.0, 0.0), v(1.0, 1.0), v(0.0, 1.0)];
        dl.add_triangles_with(TextureId(3), material, &vertices, &[0, 1, 2, 0, 2, 3]);
    }

    #[test]
    fn defaults_match_plain_alpha_linear() {
        let mut dl = UiDrawList::new();
        dl.add_filled_rect(Vec2::ZERO, Vec2::ONE, [255; 4]);
        let c = dl.commands()[0];
        assert_eq!(c.blend, UiBlend::Alpha);
        assert_eq!(c.sampler, UiSampler::Linear);
        assert_eq!(c.shade, UiShade::Plain);
        assert_eq!(c.texture2, TextureId::WHITE);
        assert_eq!(UiVertex::STRIDE, 20);
    }

    #[test]
    fn splits_on_each_new_field() {
        let masked = UiMaterial {
            shade: UiShade::Masked,
            texture2: TextureId(9),
            ..UiMaterial::PLAIN
        };
        let variants = [
            UiMaterial {
                sampler: UiSampler::Point,
                ..UiMaterial::PLAIN
            },
            masked,
            UiMaterial {
                texture2: TextureId(10),
                ..masked
            },
            UiMaterial {
                tex2_xform: [2.0, 2.0, 0.0, 0.0],
                ..masked
            },
            UiMaterial {
                shade: UiShade::Indexed,
                ..masked
            },
        ];
        let mut dl = UiDrawList::new();
        quad(&mut dl, &UiMaterial::PLAIN);
        for (i, m) in variants.iter().enumerate() {
            quad(&mut dl, m);
            assert_eq!(dl.commands().len(), i + 2);
        }
        quad(&mut dl, &variants[4]);
        assert_eq!(dl.commands().len(), 6);
        assert_eq!(dl.commands()[5].index_count, 12);
        assert_eq!(dl.commands()[5].tex2_xform, masked.tex2_xform);
    }

    #[test]
    fn blend_stack_applies_and_splits() {
        let mut dl = UiDrawList::new();
        quad(&mut dl, &UiMaterial::PLAIN);
        dl.push_blend(UiBlend::Slot2);
        dl.push_blend(UiBlend::Slot3);
        assert_eq!(dl.current_blend(), UiBlend::Slot3);
        quad(&mut dl, &UiMaterial::PLAIN);
        dl.pop_blend();
        assert_eq!(dl.current_blend(), UiBlend::Slot2);
        quad(&mut dl, &UiMaterial::PLAIN);
        dl.pop_blend();
        dl.pop_blend();
        assert_eq!(dl.current_blend(), UiBlend::Alpha);
        quad(&mut dl, &UiMaterial::PLAIN);
        let blends: Vec<_> = dl.commands().iter().map(|c| c.blend).collect();
        assert_eq!(
            blends,
            [
                UiBlend::Alpha,
                UiBlend::Slot3,
                UiBlend::Slot2,
                UiBlend::Alpha
            ]
        );
        assert_eq!(UiBlend::Slot2.slot(), 2);
        dl.push_blend(UiBlend::Slot1);
        dl.clear();
        assert_eq!(dl.current_blend(), UiBlend::Alpha);
    }

    #[test]
    fn revision_grows_on_content_change_only() {
        let mut dl = UiDrawList::new();
        let r0 = dl.revision();
        dl.push_clip(ClipRect::NONE);
        dl.push_blend(UiBlend::Slot1);
        dl.pop_blend();
        assert_eq!(dl.revision(), r0);
        dl.add_filled_rect(Vec2::ZERO, Vec2::ONE, [255; 4]);
        let r1 = dl.revision();
        assert!(r1 > r0);
        dl.add_triangles(TextureId::WHITE, &[], &[]);
        assert_eq!(dl.revision(), r1);
        dl.clear();
        assert!(dl.revision() > r1);
    }

    #[test]
    fn revisions_do_not_repeat_between_lists() {
        let build = |x: f32| {
            let mut dl = UiDrawList::new();
            dl.add_filled_rect(Vec2::splat(x), Vec2::splat(x + 5.0), [255; 4]);
            dl
        };
        let (a, b) = (build(0.0), build(9.0));
        assert_ne!(a.revision(), b.revision());
        assert_eq!(a.clone().revision(), a.revision());
        assert_eq!(UiDrawList::new().revision(), 0);
    }

    #[test]
    fn external_ids() {
        assert!(TextureId::MINIMAP.is_external());
        assert!(TextureId::external(7).is_external());
        assert!(!TextureId::WHITE.is_external());
        assert!(!TextureId((1 << 63) - 1).is_external());
        assert_ne!(TextureId::external(1), TextureId::external(2));
        assert_eq!(TextureId::MINIMAP.0, (1 << 63) | 1);
    }
}
