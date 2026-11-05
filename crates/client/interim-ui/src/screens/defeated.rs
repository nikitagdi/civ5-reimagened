//! "You were defeated" (lobby-ui 4.7): the human is eliminated while the others play on. The window offers
//! "Watch" (keep the map, no orders) and "Leave" (back to the lobby). The map is not dimmed: watching is the
//! point.
//!
//! SIMPLIFIED(SIMP-001): a plain egui window; original: the game's defeat popup.

use super::UiAction;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DefeatedView {
    pub title: String,
    pub text: String,
}

pub fn show(ctx: &egui::Context, view: &DefeatedView) -> Vec<UiAction> {
    let mut actions = Vec::new();
    egui::Window::new(&view.title)
        .id(egui::Id::unique("defeated"))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, -60.0])
        .collapsible(false)
        .resizable(false)
        .movable(false)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            ui.label(&view.text);
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                for (text, action) in [
                    ("Watch", UiAction::DefeatWatch),
                    ("Leave", UiAction::DefeatLeave),
                ] {
                    let r = ui.add(egui::Button::new(text).min_size(egui::vec2(110.0, 28.0)));
                    if r.clicked() {
                        r.surrender_focus();
                        actions.push(action);
                    }
                }
            });
        });
    actions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::{click_at, find_widget};

    #[test]
    fn watch_and_leave() {
        let v = DefeatedView {
            title: "Defeated".into(),
            text: "You were defeated. The game goes on without you.".into(),
        };
        assert!(find_widget(&v.text, |ctx| show(ctx, &v)).is_some());
        for (label, want) in [
            ("Watch", UiAction::DefeatWatch),
            ("Leave", UiAction::DefeatLeave),
        ] {
            let at = find_widget(label, |ctx| show(ctx, &v)).unwrap();
            assert_eq!(click_at(at.center(), false, |ctx| show(ctx, &v)), [want]);
        }
    }
}
