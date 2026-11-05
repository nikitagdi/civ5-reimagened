//! `TextLayout` -> `UiDrawList`.

use draw_list::{ClipRect, TextureId, UiDrawList};
use glam::Vec2;

use crate::layout::{TextLayout, TextureRef};

/// Maps a font texture to the renderer's texture id. `ui-render` / `ui-core` implement it; font textures that
/// have a `-index` pair are expanded by the renderer, not here.
pub trait FontTextures {
    fn texture_id(&self, texture: &TextureRef) -> Option<TextureId>;
}

impl TextLayout {
    /// Appends the quads to `dl`: positions are `origin` plus the layout rectangles (UI pixels), the alpha of
    /// every vertex is multiplied by `alpha`. A quad whose texture is unknown is skipped. With `clip` the
    /// quads are clipped to it (on top of the clip already on `dl`).
    pub fn emit(
        &self,
        dl: &mut UiDrawList,
        origin: Vec2,
        alpha: f32,
        clip: Option<ClipRect>,
        textures: &dyn FontTextures,
    ) {
        if self.quads.is_empty() {
            return;
        }
        if let Some(c) = clip {
            dl.push_clip(c);
        }
        let ids: Vec<Option<TextureId>> = self
            .textures
            .iter()
            .map(|t| textures.texture_id(t))
            .collect();
        let a = alpha.clamp(0.0, 1.0);
        for q in &self.quads {
            let Some(tex) = ids[q.texture as usize] else {
                continue;
            };
            let color = [
                q.color[0],
                q.color[1],
                q.color[2],
                (q.color[3] as f32 * a).round() as u8,
            ];
            dl.add_rect(
                tex,
                origin + Vec2::new(q.rect[0], q.rect[1]),
                origin + Vec2::new(q.rect[2], q.rect[3]),
                Vec2::new(q.uv[0], q.uv[1]),
                Vec2::new(q.uv[2], q.uv[3]),
                color,
            );
        }
        if clip.is_some() {
            dl.pop_clip();
        }
    }
}
