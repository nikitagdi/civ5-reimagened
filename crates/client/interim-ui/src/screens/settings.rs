//! The "Settings" window (lobby-ui 4.9, LU-D11): graphics quality, the statistics switch, facts about
//! the render scale and the shadow quality, the build and the line, and [Leave game] / [Sign out] / [Close]. It opens with the top bar's
//! [Settings] button; the screen keeps nothing, the shell keeps the open flag.

use super::UiAction;
use render::gfx_choice::{GfxPreset, Msaa};
use render::render_scale;
use render::shadow::ShadowQuality;
use render::terrain_detail::TerrainDetail;

/// The quality levels the window offers (`render::Preset` in the shell).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityPreset {
    Low,
    Medium,
    High,
}

impl QualityPreset {
    pub const ALL: [QualityPreset; 3] = [Self::Low, Self::Medium, Self::High];

    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "Low detail",
            Self::Medium => "Medium detail",
            Self::High => "High detail",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Self::Low => "No reflections, no object shadows, light fog, no anti-aliasing",
            Self::Medium => "Reduced shadows and fog",
            Self::High => {
                "Like the original at the developer's settings (anti-aliasing 4x instead of 8x)"
            }
        }
    }
}

const MIN_PCT: u32 = (render_scale::MIN * 100.0) as u32;
const MAX_PCT: u32 = (render_scale::MAX * 100.0) as u32;
const STEP_PCT: u32 = (render_scale::STEP * 100.0) as u32;

fn preset_hint(p: GfxPreset) -> &'static str {
    match p {
        GfxPreset::Low => {
            "60% scale, small shadows from units and buildings only, no anti-aliasing"
        }
        GfxPreset::Medium => "80% scale, medium shadows, no anti-aliasing",
        GfxPreset::High => "Full size, full shadows, anti-aliasing 4x",
    }
}

fn msaa_hint(m: Msaa) -> &'static str {
    match m {
        Msaa::Off => "Rough edges, the cheapest",
        Msaa::X4 => "Smooth edges, costs the most on a phone",
    }
}

fn shadow_hint(q: ShadowQuality) -> &'static str {
    match q {
        ShadowQuality::Off => "No shadows: everything is lit",
        ShadowQuality::Low => {
            "A small map (a quarter of the size), cast by units and buildings only"
        }
        ShadowQuality::Medium => "Half the size of the shadow map",
        ShadowQuality::High => "The size of the original",
    }
}

fn terrain_hint(d: TerrainDetail) -> &'static str {
    match d {
        TerrainDetail::Low => "Coarse ground and small textures; the least memory",
        TerrainDetail::Medium => "A quarter of the ground triangles, textures up to 256",
        TerrainDetail::High => "The ground as the graphics quality makes it",
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SettingsView {
    /// The preset whose keys the renderer has; `None` is "Custom".
    pub quality: Option<QualityPreset>,
    /// The keys differ from every preset (for example from `?gfx=`).
    pub custom: bool,
    /// The 3D scene's size as a percent of the screen (50..=100, steps of 5).
    pub render_scale_pct: u32,
    /// The size of the shadow map in force.
    pub shadow_quality: ShadowQuality,
    /// The preset the render scale, the shadow quality and the multisampling are; `None` is "Custom".
    pub gfx_preset: Option<GfxPreset>,
    /// The multisampling the player picked, and the one the renderer was made with (the samples
    /// change at the next start).
    pub msaa: Msaa,
    pub msaa_in_force: Msaa,
    /// The terrain detail in force (not the render scale's business).
    pub terrain_detail: TerrainDetail,
    /// "Show performance statistics".
    pub stats: bool,
    /// Server address, round trip time, builds: label and value.
    pub info: Vec<(String, String)>,
    /// A server game is running: [Leave game].
    pub can_leave: bool,
    /// A server session: [Sign out].
    pub can_sign_out: bool,
}

pub fn show(ctx: &egui::Context, view: &SettingsView) -> Vec<UiAction> {
    let mut actions = Vec::new();
    // A landscape phone is ~400 points tall, less than the window: the settings scroll inside the
    // screen, the buttons at the bottom stay in view (live report 2026-11-03).
    let screen = ctx.content_rect();
    let width = (screen.width() - 32.0).clamp(240.0, 380.0);
    let body_height = (screen.height() - 120.0).max(120.0);
    egui::Window::new("Settings")
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .collapsible(false)
        .resizable(false)
        .min_width(width)
        .max_width(width)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .max_height(body_height)
                .auto_shrink([false, true])
                .show(ui, |ui| {
            // ---- preset: the render scale, the shadows and the multisampling together
            ui.strong("Preset");
            ui.horizontal(|ui| {
                for p in GfxPreset::ALL {
                    let b = egui::Button::new(p.name()).selected(view.gfx_preset == Some(p));
                    let r = ui.add(b).on_hover_text(preset_hint(p));
                    if r.clicked() {
                        r.surrender_focus();
                        actions.push(UiAction::SetGfxPreset(p));
                    }
                }
                if view.gfx_preset.is_none() {
                    ui.weak("Custom");
                }
            });
            ui.weak(match view.gfx_preset {
                Some(p) => preset_hint(p),
                None => "Custom: the three settings below are not one of the presets.",
            });
            ui.separator();
            ui.strong("Render scale");
            let mut pct = view.render_scale_pct.clamp(MIN_PCT, MAX_PCT);
            let slider = egui::Slider::new(&mut pct, MIN_PCT..=MAX_PCT)
                .step_by(f64::from(STEP_PCT))
                .suffix("%");
            if ui.add(slider).changed() {
                actions.push(UiAction::SetRenderScale(pct));
            }
            ui.weak("The 3D world is drawn at this share of the screen size; the interface stays sharp.");
            ui.strong("Shadow quality");
            ui.horizontal(|ui| {
                for q in ShadowQuality::ALL {
                    let b = egui::Button::new(q.name()).selected(view.shadow_quality == q);
                    let r = ui.add(b).on_hover_text(shadow_hint(q));
                    if r.clicked() {
                        r.surrender_focus();
                        actions.push(UiAction::SetShadowQuality(q));
                    }
                }
            });
            ui.weak(shadow_hint(view.shadow_quality));
            // ---- terrain detail (b15-terrain): its own block, part of the presets
            ui.strong("Terrain detail");
            ui.horizontal(|ui| {
                for d in TerrainDetail::ALL {
                    let b = egui::Button::new(d.name()).selected(view.terrain_detail == d);
                    let r = ui.add(b).on_hover_text(terrain_hint(d));
                    if r.clicked() {
                        r.surrender_focus();
                        actions.push(UiAction::SetTerrainDetail(d));
                    }
                }
            });
            ui.weak(terrain_hint(view.terrain_detail));
            // ---- multisampling: fixed when the renderer is made, so a change waits for the next start
            ui.strong("Anti-aliasing (MSAA)");
            ui.horizontal(|ui| {
                for m in Msaa::ALL {
                    let b = egui::Button::new(m.name()).selected(view.msaa == m);
                    let r = ui.add(b).on_hover_text(msaa_hint(m));
                    if r.clicked() {
                        r.surrender_focus();
                        actions.push(UiAction::SetMsaa(m));
                    }
                }
            });
            if view.msaa == view.msaa_in_force {
                ui.weak(msaa_hint(view.msaa));
            } else {
                ui.weak(format!(
                    "Applies the next time the game loads (now {})",
                    view.msaa_in_force.name()
                ));
            }
            ui.separator();
            // ---- the detail of the world: the keys of the original's graphics ini
            ui.strong("World detail");
            ui.horizontal(|ui| {
                for p in QualityPreset::ALL {
                    let b = egui::Button::new(p.label()).selected(view.quality == Some(p));
                    let r = ui.add(b).on_hover_text(p.hint());
                    if r.clicked() {
                        r.surrender_focus();
                        actions.push(UiAction::SetQuality(p));
                    }
                }
                if view.custom {
                    ui.weak("Custom");
                }
            });
            match (view.quality, view.custom) {
                (Some(p), _) => ui.weak(p.hint()),
                (None, true) => ui.weak("Custom: some keys differ from the presets."),
                (None, false) => ui.weak(""),
            };
            if !view.info.is_empty() {
                ui.separator();
                egui::Grid::new("settings-info")
                    .num_columns(2)
                    .spacing([12.0, 2.0])
                    .show(ui, |ui| {
                        for (k, v) in &view.info {
                            ui.weak(k);
                            ui.label(v);
                            ui.end_row();
                        }
                    });
            }
                });
            ui.separator();
            let mut stats = view.stats;
            if ui
                .checkbox(&mut stats, "Show performance statistics")
                .changed()
            {
                actions.push(UiAction::ToggleStats(stats));
            }
            ui.horizontal(|ui| {
                if view.can_leave && ui.button("Leave game").clicked() {
                    actions.push(UiAction::LeaveGame);
                }
                if view.can_sign_out && ui.button("Sign out").clicked() {
                    actions.push(UiAction::SignOut);
                }
                if ui.button("Close").clicked() {
                    actions.push(UiAction::ToggleSettings);
                }
            });
        });
    actions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::{click_at, find_nth_widget, find_widget};

    fn view() -> SettingsView {
        SettingsView {
            quality: Some(QualityPreset::High),
            render_scale_pct: 100,
            shadow_quality: ShadowQuality::High,
            gfx_preset: Some(GfxPreset::High),
            terrain_detail: TerrainDetail::High,
            msaa: Msaa::X4,
            msaa_in_force: Msaa::X4,
            can_leave: true,
            can_sign_out: true,
            info: vec![("Server".into(), "wss://example/ws".into())],
            ..SettingsView::default()
        }
    }

    /// Clicks the `n`th widget labelled `label` (labels that two rows share count in drawing order).
    fn click_nth(v: &SettingsView, label: &str, n: usize) -> Vec<UiAction> {
        let at = find_nth_widget(label, n, |ctx| show(ctx, v))
            .unwrap_or_else(|| panic!("no {label} #{n}"))
            .center();
        click_at(at, false, |ctx| show(ctx, v))
    }

    fn click(v: &SettingsView, label: &str) -> Vec<UiAction> {
        click_nth(v, label, 0)
    }

    #[test]
    fn a_world_detail_button_sets_the_quality() {
        let v = view();
        assert_eq!(
            click(&v, "Low detail"),
            [UiAction::SetQuality(QualityPreset::Low)]
        );
        assert_eq!(
            click(&v, "Medium detail"),
            [UiAction::SetQuality(QualityPreset::Medium)]
        );
    }

    #[test]
    fn a_preset_button_sets_the_scale_the_shadows_and_the_multisampling_together() {
        let v = view();
        assert_eq!(
            click(&v, "Low"),
            [UiAction::SetGfxPreset(GfxPreset::Low)],
            "the first Low is the preset"
        );
        assert_eq!(
            click(&v, "Medium"),
            [UiAction::SetGfxPreset(GfxPreset::Medium)]
        );
        assert_eq!(click(&v, "High"), [UiAction::SetGfxPreset(GfxPreset::High)]);
    }

    #[test]
    fn the_shadow_buttons_pick_the_size_of_the_map() {
        let v = view();
        assert_eq!(
            click(&v, "Off"),
            [UiAction::SetShadowQuality(ShadowQuality::Off)]
        );
        assert_eq!(
            click_nth(&v, "Low", 1),
            [UiAction::SetShadowQuality(ShadowQuality::Low)],
            "the second Low is the shadow quality"
        );
    }

    #[test]
    fn the_msaa_buttons_pick_the_samples_and_there_is_no_2x() {
        let v = view();
        assert_eq!(click_nth(&v, "Off", 1), [UiAction::SetMsaa(Msaa::Off)]);
        assert_eq!(click(&v, "4x"), [UiAction::SetMsaa(Msaa::X4)]);
        assert!(find_widget("2x", |ctx| show(ctx, &v)).is_none());
    }

    #[test]
    fn a_msaa_change_says_it_waits_for_the_next_load() {
        let mut v = view();
        let said = |v: &SettingsView| {
            find_widget("Applies the next time the game loads (now 4x)", |ctx| {
                show(ctx, v)
            })
            .is_some()
        };
        assert!(!said(&v));
        v.msaa = Msaa::Off;
        assert!(said(&v));
    }

    #[test]
    fn the_terrain_detail_buttons_set_the_detail_and_not_the_graphics_quality() {
        let mut v = view();
        v.terrain_detail = TerrainDetail::Medium;
        let title = find_widget("Terrain detail", |ctx| show(ctx, &v)).expect("the block");
        // The buttons are in the row under the title; the first is Low (the labels also name the
        // presets above, so the row is found by its place).
        let low = egui::pos2(title.left() + 12.0, title.bottom() + 14.0);
        assert_eq!(
            click_at(low, false, |ctx| show(ctx, &v)),
            [UiAction::SetTerrainDetail(TerrainDetail::Low)]
        );
        let mut out = Vec::new();
        crate::screens::run_headless(|ctx| out = show(ctx, &v));
        assert!(out.is_empty(), "quiet until clicked");
    }

    #[test]
    fn the_render_scale_slider_is_shown_and_is_quiet_until_moved() {
        let v = view();
        assert!(find_widget("Render scale", |ctx| show(ctx, &v)).is_some());
        let mut out = vec![UiAction::ToggleSettings];
        crate::screens::run_headless(|ctx| out = show(ctx, &v));
        assert!(out.is_empty());
    }

    #[test]
    fn custom_is_named_when_the_choices_are_no_preset_or_the_keys_differ() {
        let mut v = view();
        assert!(find_widget("Custom", |ctx| show(ctx, &v)).is_none());
        v.gfx_preset = None;
        assert!(find_widget("Custom", |ctx| show(ctx, &v)).is_some());
        v.gfx_preset = Some(GfxPreset::Low);
        v.quality = None;
        v.custom = true;
        assert!(find_widget("Custom", |ctx| show(ctx, &v)).is_some());
    }

    #[test]
    fn the_stats_box_leave_and_close() {
        let v = view();
        assert_eq!(
            click(&v, "Show performance statistics"),
            [UiAction::ToggleStats(true)]
        );
        assert_eq!(click(&v, "Leave game"), [UiAction::LeaveGame]);
        assert_eq!(click(&v, "Sign out"), [UiAction::SignOut]);
        assert_eq!(click(&v, "Close"), [UiAction::ToggleSettings]);
    }

    #[test]
    fn on_a_landscape_phone_the_window_fits_the_screen_and_close_stays_in_view() {
        let v = view();
        let ctx = egui::Context::default();
        let raw = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(830.0, 384.0),
            )),
            ..egui::RawInput::default()
        };
        for _ in 0..3 {
            ctx.run_pass(raw(), |ctx| drop(show(ctx, &v)))
                .drop_without_applying_deltas();
        }
        let out = ctx.run_pass(raw(), |ctx| drop(show(ctx, &v)));
        let close = out
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.job.text == "Close" => {
                    Some(t.galley.rect.translate(t.pos.to_vec2()))
                }
                _ => None,
            })
            .expect("a Close button");
        out.drop_without_applying_deltas();
        assert!(close.bottom() <= 384.0, "Close at {close:?}");
        assert!(close.top() >= 0.0, "Close at {close:?}");
    }

    #[test]
    fn leave_game_only_in_a_game() {
        let mut v = view();
        v.can_leave = false;
        v.can_sign_out = false;
        assert!(find_widget("Leave game", |ctx| show(ctx, &v)).is_none());
        assert!(find_widget("Sign out", |ctx| show(ctx, &v)).is_none());
    }
}
