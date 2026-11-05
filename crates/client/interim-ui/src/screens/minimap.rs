//! The "Minimap" panel (lobby-ui 4.8, LU-D10). The source is the renderer's texture (the aim: the
//! pass `Minimap` has frame, fog and borders drawn) or, until the renderer's projection is real,
//! a picture the shell builds from the view.
//!
//! SIMPLIFIED(SIMP-060): the `Image` branch is the stand-in; the shell's `minimap_image.rs` builds it.
//!
//! A click or a drag over the picture gives [`UiAction::MinimapPointer`]; the panel is a window,
//! so the map under it gets no clicks (`InterimUi::pointer_over_window`).

use std::sync::Arc;

use super::UiAction;

/// The room the panel's picture gets: the size of the original's minimap (320x190).
pub const BOX: egui::Vec2 = egui::vec2(320.0, 190.0);
/// The panel's height with its frame and the gap to what stands above it (the journal).
pub const RESERVED_HEIGHT: f32 = 190.0 + 12.0 + 2.0 + 8.0;

/// A picture built from the view; `version` rises with every change of its content.
#[derive(Clone)]
pub struct MinimapImage {
    /// Width and height in pixels.
    pub size: [usize; 2],
    /// RGBA, `size[0] * size[1] * 4` bytes.
    pub rgba: Arc<Vec<u8>>,
    pub version: u64,
}

impl std::fmt::Debug for MinimapImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MinimapImage({:?}, v{})", self.size, self.version)
    }
}

impl PartialEq for MinimapImage {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version && self.size == other.size
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum MinimapSource {
    /// No panel (no renderer or no view).
    #[default]
    None,
    /// The stand-in: a picture from the view.
    Image(MinimapImage),
    /// The renderer's texture. `generation` rises only when the texture is made again (size, device,
    /// preset); the egui id comes with [`provide_native`].
    Texture { size: (u32, u32), generation: u64 },
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MinimapView {
    pub source: MinimapSource,
    /// The camera's view on the `Image` picture: left, top, right, bottom in 0..=65535 of the map
    /// (the texture has its own frame).
    pub camera_frame: Option<[u16; 4]>,
}

/// The renderer's minimap texture as egui has it: the id and the size in pixels.
#[derive(Clone, Copy)]
struct Native {
    id: egui::TextureId,
    size: [u32; 2],
}

fn native_key() -> egui::Id {
    egui::Id::unique("minimap-native-texture")
}

/// `InterimUi::run` tells the panel which egui texture the renderer's minimap is (`None`: none yet).
pub fn provide_native(ctx: &egui::Context, texture: Option<(egui::TextureId, [u32; 2])>) {
    ctx.data_mut(|d| match texture {
        Some((id, size)) => {
            d.insert_temp(native_key(), Native { id, size });
        }
        None => d.remove::<Native>(native_key()),
    });
}

/// The picture of a source, uploaded to egui: `(texture, size, show the camera frame)`.
fn texture(
    ctx: &egui::Context,
    source: &MinimapSource,
) -> Option<(egui::TextureId, [f32; 2], bool)> {
    match source {
        MinimapSource::None => None,
        MinimapSource::Image(img) => {
            let key = egui::Id::unique("minimap-image");
            let cached: Option<(u64, [usize; 2], egui::TextureHandle)> =
                ctx.data(|d| d.get_temp(key));
            let handle = match cached {
                Some((version, size, h)) if version == img.version && size == img.size => h,
                _ => {
                    let pixels =
                        egui::ColorImage::from_rgba_unmultiplied(img.size, img.rgba.as_slice());
                    let h =
                        ctx.load_texture("minimap-image", pixels, egui::TextureOptions::NEAREST);
                    ctx.data_mut(|d| d.insert_temp(key, (img.version, img.size, h.clone())));
                    h
                }
            };
            Some((handle.id(), [img.size[0] as f32, img.size[1] as f32], true))
        }
        MinimapSource::Texture { .. } => {
            let n: Native = ctx.data(|d| d.get_temp(native_key()))?;
            Some((n.id, [n.size[0] as f32, n.size[1] as f32], false))
        }
    }
}

/// Where a picture of `size` pixels sits in `bx`: as large as fits, centred, the aspect kept.
pub fn fit(bx: egui::Rect, size: [f32; 2]) -> egui::Rect {
    if size[0] <= 0.0 || size[1] <= 0.0 {
        return bx;
    }
    let scale = (bx.width() / size[0]).min(bx.height() / size[1]);
    egui::Rect::from_center_size(bx.center(), egui::vec2(size[0] * scale, size[1] * scale))
}

/// The position `p` on `rect` as fractions of 0..=65535 from its top left corner.
pub fn fractions(rect: egui::Rect, p: egui::Pos2) -> (u16, u16) {
    let f = |v: f32, lo: f32, len: f32| {
        (((v - lo) / len.max(1.0)).clamp(0.0, 1.0) * 65535.0).round() as u16
    };
    (
        f(p.x, rect.min.x, rect.width()),
        f(p.y, rect.min.y, rect.height()),
    )
}

pub fn show(ctx: &egui::Context, view: &MinimapView) -> Vec<UiAction> {
    let mut actions = Vec::new();
    let Some((id, size, frame)) = texture(ctx, &view.source) else {
        return actions;
    };
    egui::Area::new(egui::Id::unique("minimap-panel"))
        .anchor(egui::Align2::LEFT_BOTTOM, [12.0, -12.0])
        .show(ctx, |ui| {
            egui::Frame::window(ui.style()).show(ui, |ui| {
                let (bx, resp) = ui.allocate_exact_size(BOX, egui::Sense::click_and_drag());
                let img = fit(bx, size);
                let p = ui.painter_at(bx);
                p.rect_filled(bx, 0.0, egui::Color32::BLACK);
                p.image(
                    id,
                    img,
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
                if frame && let Some([x0, y0, x1, y1]) = view.camera_frame {
                    let at = |x: u16, y: u16| {
                        egui::pos2(
                            img.min.x + img.width() * f32::from(x) / 65535.0,
                            img.min.y + img.height() * f32::from(y) / 65535.0,
                        )
                    };
                    p.rect_stroke(
                        egui::Rect::from_two_pos(at(x0, y0), at(x1, y1)),
                        0.0,
                        egui::Stroke::new(1.5, egui::Color32::WHITE),
                        egui::StrokeKind::Inside,
                    );
                }
                let released = resp.drag_stopped() || resp.clicked();
                if (released || resp.is_pointer_button_down_on())
                    && let Some(at) = resp.interact_pointer_pos().or_else(|| resp.hover_pos())
                {
                    let (u, v) = fractions(img, at);
                    actions.push(UiAction::MinimapPointer {
                        u,
                        v,
                        down: !released,
                    });
                }
            });
        });
    actions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::{click_at, run_headless};

    fn image(version: u64) -> MinimapView {
        MinimapView {
            source: MinimapSource::Image(MinimapImage {
                size: [2, 1],
                rgba: Arc::new(vec![255; 8]),
                version,
            }),
            camera_frame: Some([10_000, 10_000, 30_000, 30_000]),
        }
    }

    fn texture_view() -> MinimapView {
        MinimapView {
            source: MinimapSource::Texture {
                size: (320, 190),
                generation: 1,
            },
            camera_frame: None,
        }
    }

    /// The panel's picture rect on a 1280x720 screen (left bottom corner anchor, frame margin 6).
    fn panel_box() -> egui::Rect {
        let h = 190.0 + 12.0;
        let bottom = 720.0 - 12.0 - 1.0;
        egui::Rect::from_min_size(egui::pos2(12.0 + 7.0, bottom - h + 6.0), BOX)
    }

    fn with_native(v: MinimapView) -> impl FnMut(&egui::Context) -> Vec<UiAction> {
        move |ctx| {
            provide_native(ctx, Some((egui::TextureId::User(7), [320, 190])));
            show(ctx, &v)
        }
    }

    #[test]
    fn nothing_without_a_source() {
        let shapes = run_headless(|ctx| {
            assert!(show(ctx, &MinimapView::default()).is_empty());
        });
        assert!(shapes.is_empty());
    }

    #[test]
    fn the_image_branch_draws_the_picture_and_the_frame() {
        let v = image(1);
        let shapes = run_headless(|ctx| {
            show(ctx, &v);
        });
        assert!(
            shapes
                .iter()
                .any(|s| matches!(&s.shape, egui::Shape::Mesh(_)))
        );
        assert!(
            shapes
                .iter()
                .any(|s| matches!(&s.shape, egui::Shape::Rect(r) if r.stroke.width > 1.0))
        );
    }

    #[test]
    fn the_texture_branch_waits_for_the_texture_and_then_draws_it() {
        let v = texture_view();
        let none = run_headless(|ctx| {
            show(ctx, &v);
        });
        assert!(none.is_empty(), "no egui texture yet: no panel");
        let shapes = run_headless(|ctx| {
            provide_native(ctx, Some((egui::TextureId::User(7), [320, 190])));
            show(ctx, &v);
        });
        assert!(shapes.iter().any(|s| matches!(&s.shape,
            egui::Shape::Mesh(m) if m.texture_id == egui::TextureId::User(7))));
    }

    #[test]
    fn the_corners_of_the_picture_give_zero_and_the_top() {
        let b = panel_box();
        let actions = click_at(
            b.min + egui::vec2(0.5, 0.5),
            false,
            with_native(texture_view()),
        );
        let Some(UiAction::MinimapPointer { u, v, down }) = actions.first() else {
            panic!("{actions:?}")
        };
        assert!(*u < 400 && *v < 700, "near 0: {u} {v}");
        assert!(*down, "the press holds the button down");
        let actions = click_at(
            b.max - egui::vec2(0.5, 0.5),
            false,
            with_native(texture_view()),
        );
        let Some(UiAction::MinimapPointer { u, v, .. }) = actions.first() else {
            panic!("{actions:?}")
        };
        assert!(*u > 65_000 && *v > 64_800, "near 65535: {u} {v}");
    }

    #[test]
    fn the_release_sends_down_false() {
        let actions = click_at(panel_box().center(), false, with_native(texture_view()));
        assert!(matches!(
            actions.first(),
            Some(UiAction::MinimapPointer { down: true, .. })
        ));
        assert!(matches!(
            actions.last(),
            Some(UiAction::MinimapPointer { down: false, .. })
        ));
    }

    #[test]
    fn fractions_are_clamped_and_span_the_whole_range() {
        let r = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(100.0, 50.0));
        assert_eq!(fractions(r, r.min), (0, 0));
        assert_eq!(fractions(r, r.max), (65535, 65535));
        assert_eq!(fractions(r, egui::pos2(-50.0, 500.0)), (0, 65535));
    }

    #[test]
    fn a_wide_picture_keeps_its_aspect_in_the_box() {
        let bx = egui::Rect::from_min_size(egui::Pos2::ZERO, BOX);
        let r = fit(bx, [100.0, 20.0]);
        assert!((r.width() / r.height() - 5.0).abs() < 0.01);
        assert_eq!(r.width(), 320.0);
        assert!(bx.contains_rect(r));
    }
}
