//! A yes / no question in the middle of the screen: "Declare war?" before an order that attacks a
//! civilization at peace (AC-5.5), "Disband?" before a unit is deleted. The answer is
//! [`UiAction::Confirm`]; Esc is handled by the game (it drops the question). The questions of a
//! server game (lobby-ui LU-D7): declare war on a player, remove a player, leave to the lobby;
//! `danger` paints the "yes" red.
//!
//! SIMPLIFIED(SIMP-001): a plain egui window; original: the DeclareWarPopup / generic popup Lua
//! screens.

use super::UiAction;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfirmView {
    pub title: String,
    pub text: String,
    pub yes: String,
    pub no: String,
    /// The "yes" does harm to someone (war, kick): its button is red.
    pub danger: bool,
}

pub fn show(ctx: &egui::Context, view: &ConfirmView) -> Vec<UiAction> {
    let mut actions = Vec::new();
    egui::Window::new(&view.title)
        .id(egui::Id::unique("confirm"))
        .anchor(egui::Align2::CENTER_CENTER, [0.0, -60.0])
        .collapsible(false)
        .resizable(false)
        .movable(false)
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            ui.label(&view.text);
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                for (text, yes) in [(&view.yes, true), (&view.no, false)] {
                    let mut button = egui::Button::new(text).min_size(egui::vec2(110.0, 28.0));
                    if yes && view.danger {
                        button = button.fill(egui::Color32::from_rgb(140, 45, 40));
                    }
                    let r = ui.add(button);
                    if r.clicked() {
                        r.surrender_focus();
                        actions.push(UiAction::Confirm(yes));
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
    fn yes_and_no() {
        let v = ConfirmView {
            title: "Declare war?".into(),
            text: "Attacking Egypt means war.".into(),
            yes: "Declare war".into(),
            no: "Cancel".into(),
            danger: false,
        };
        assert!(find_widget("Attacking Egypt means war.", |ctx| show(ctx, &v)).is_some());
        for (label, yes) in [("Declare war", true), ("Cancel", false)] {
            let at = find_widget(label, |ctx| show(ctx, &v)).unwrap();
            assert_eq!(
                click_at(at.center(), false, |ctx| show(ctx, &v)),
                [UiAction::Confirm(yes)]
            );
        }
    }
}
