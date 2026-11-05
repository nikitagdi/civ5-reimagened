//! Error screen (play-ui §3.3, AC-9.5): the `Fatal` text and a button that starts again.

use super::UiAction;

pub fn show(ctx: &egui::Context, message: &str) -> Vec<UiAction> {
    let mut actions = Vec::new();
    // Dim everything behind the window: the game has stopped.
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::unique("fatal-dim"))
        .order(egui::Order::Middle)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            ui.painter()
                .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(160));
            ui.allocate_rect(screen, egui::Sense::click());
        });
    egui::Window::new("The game has stopped")
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .collapsible(false)
        .resizable(false)
        .movable(false)
        .default_width(440.0)
        .show(ctx, |ui| {
            ui.label(egui::RichText::new(message).color(ui.visuals().error_fg_color));
            ui.add_space(8.0);
            ui.label("Reload the page to start again.");
            if ui.button("Reload").clicked() {
                actions.push(UiAction::Reload);
            }
        });
    actions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::{click_at, find_widget};

    #[test]
    fn reload_button() {
        let msg = "the turn is stuck: no progress";
        let at = find_widget("Reload", |ctx| show(ctx, msg)).expect("a Reload button");
        assert!(find_widget(msg, |ctx| show(ctx, msg)).is_some());
        assert_eq!(
            click_at(at.center(), false, |ctx| show(ctx, msg)),
            [UiAction::Reload]
        );
    }
}
