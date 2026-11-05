//! Unit panel (play-ui §3.3, AC-5.1, AC-5.6-AC-5.9), bottom centre: the selected unit's name, moves,
//! strength, ranged strength, HP and state, and its action buttons from the brain's `UnitActions`.
//! A disabled action is grey with the brain's reason as its tooltip; a click asks for the action
//! ([`UiAction::UnitAction`]). While the ranged-attack mode is on, a line says how to aim.
//!
//! SIMPLIFIED(SIMP-001): a small panel without the unit portrait, promotion icons and the
//! original's action icons; original: the UnitPanel Lua screen.

use super::UiAction;

/// One action button.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionButton {
    /// The gc-abi-v1 unit command number, given back in [`UiAction::UnitAction`].
    pub action: i16,
    pub label: String,
    /// The key, shown in the tooltip ("B").
    pub hotkey: Option<String>,
    pub enabled: bool,
    /// Why it is disabled (the brain's text).
    pub reason: String,
    /// The mode it starts is on (the ranged attack while aiming).
    pub active: bool,
}

impl ActionButton {
    fn tooltip(&self) -> String {
        let key = self
            .hotkey
            .as_ref()
            .map(|k| format!(" [{k}]"))
            .unwrap_or_default();
        if self.enabled {
            format!("{}{key}", self.label)
        } else if self.reason.is_empty() {
            format!("{}{key}: not possible now", self.label)
        } else {
            format!("{}{key}: {}", self.label, self.reason)
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct UnitPanelView {
    /// "Archer".
    pub name: String,
    /// The owner and type line ("Rome, Archer"), if it says more than the name.
    pub subtitle: Option<String>,
    /// ("Moves", "2/2"), ("Strength", "5"), ("Ranged", "7, range 2"), ("HP", "6/10").
    pub stats: Vec<(String, String)>,
    /// HP as 0..1, for the bar.
    pub health: f32,
    /// "Fortified", "Sleeping", "Automated", "Embarked", "Level 2, 30 XP", ...
    pub status: Vec<String>,
    pub actions: Vec<ActionButton>,
    /// The `UnitActions` answer has not come yet.
    pub loading: bool,
    /// The ranged-attack mode is on: how to aim and cancel.
    pub aiming: Option<String>,
}

pub fn show(ctx: &egui::Context, view: &UnitPanelView) -> Vec<UiAction> {
    let mut actions = Vec::new();
    egui::Area::new(egui::Id::unique("unit-panel"))
        .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -12.0])
        .show(ctx, |ui| {
            egui::Frame::window(ui.style()).show(ui, |ui| {
                ui.set_width(360.0);
                if let Some(text) = &view.aiming {
                    ui.colored_label(ui.visuals().warn_fg_color, text);
                }
                ui.horizontal(|ui| {
                    ui.heading(&view.name);
                    if let Some(s) = &view.subtitle {
                        ui.weak(s);
                    }
                });
                egui::Grid::new("unit-stats")
                    .num_columns(view.stats.len() * 2)
                    .spacing([8.0, 2.0])
                    .show(ui, |ui| {
                        for (k, v) in &view.stats {
                            ui.weak(k);
                            ui.strong(v);
                        }
                    });
                if view.health < 1.0 {
                    ui.add(
                        egui::ProgressBar::new(view.health.clamp(0.0, 1.0))
                            .desired_height(6.0)
                            .fill(health_color(view.health)),
                    );
                }
                if !view.status.is_empty() {
                    ui.label(view.status.join(", "));
                }
                ui.separator();
                if view.loading {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Asking what the unit can do...");
                    });
                    return;
                }
                ui.horizontal_wrapped(|ui| {
                    for b in &view.actions {
                        let button = egui::Button::new(&b.label).selected(b.active);
                        let r = ui
                            .add_enabled(b.enabled, button)
                            .on_hover_text(b.tooltip())
                            .on_disabled_hover_text(b.tooltip());
                        if r.clicked() {
                            // Keys after the click are hotkeys, not presses of this button.
                            r.surrender_focus();
                            actions.push(UiAction::UnitAction(b.action));
                        }
                    }
                    if view.actions.is_empty() {
                        ui.weak("No actions.");
                    }
                });
            });
        });
    actions
}

/// Green when healthy, through yellow, to red.
pub fn health_color(health: f32) -> egui::Color32 {
    let h = health.clamp(0.0, 1.0);
    if h > 0.66 {
        egui::Color32::from_rgb(60, 190, 60)
    } else if h > 0.33 {
        egui::Color32::from_rgb(220, 190, 40)
    } else {
        egui::Color32::from_rgb(210, 50, 40)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::{click_at, find_widget, run_headless};

    fn button(action: i16, label: &str, enabled: bool) -> ActionButton {
        ActionButton {
            action,
            label: label.into(),
            hotkey: Some("F".into()),
            enabled,
            reason: if enabled {
                String::new()
            } else {
                "Civilians cannot fortify".into()
            },
            active: false,
        }
    }

    fn view() -> UnitPanelView {
        UnitPanelView {
            name: "Archer".into(),
            subtitle: Some("Rome".into()),
            stats: vec![("Moves".into(), "2/2".into()), ("HP".into(), "6/10".into())],
            health: 0.6,
            status: vec!["Fortified".into()],
            actions: vec![
                button(2, "Ranged attack", true),
                button(4, "Fortify", false),
                button(6, "Skip turn", true),
            ],
            loading: false,
            aiming: None,
        }
    }

    #[test]
    fn shows_the_unit_and_its_buttons() {
        let v = view();
        for t in ["Archer", "Rome", "2/2", "6/10", "Fortified", "Fortify"] {
            assert!(find_widget(t, |ctx| show(ctx, &v)).is_some(), "{t}");
        }
        assert_eq!(
            v.actions[1].tooltip(),
            "Fortify [F]: Civilians cannot fortify"
        );
        let loading = UnitPanelView {
            loading: true,
            ..view()
        };
        assert!(find_widget("Asking what the unit can do...", |ctx| show(ctx, &loading)).is_some());
        assert!(find_widget("Fortify", |ctx| show(ctx, &loading)).is_none());
        assert!(!run_headless(|ctx| drop(show(ctx, &UnitPanelView::default()))).is_empty());
    }

    #[test]
    fn a_click_asks_for_the_action_and_a_disabled_button_does_nothing() {
        let v = view();
        let at = find_widget("Skip turn", |ctx| show(ctx, &v)).unwrap();
        assert_eq!(
            click_at(at.center(), false, |ctx| show(ctx, &v)),
            [UiAction::UnitAction(6)]
        );
        let at = find_widget("Fortify", |ctx| show(ctx, &v)).unwrap();
        assert!(click_at(at.center(), false, |ctx| show(ctx, &v)).is_empty());
    }

    #[test]
    fn the_aiming_line_shows_in_the_ranged_mode() {
        let v = UnitPanelView {
            aiming: Some("Ranged attack: click a red plot".into()),
            ..view()
        };
        assert!(find_widget("Ranged attack: click a red plot", |ctx| show(ctx, &v)).is_some());
    }
}
