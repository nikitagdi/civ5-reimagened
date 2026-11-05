//! The window that tells why the player left the game (lobby-ui 4.7 "Notices"): kicked, replaced by another
//! tab, the game stopped, the game is over. The map or the lobby stays dimmed behind it.

use super::UiAction;

/// What the button of a notice does.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NoticeButton {
    /// The player read it: on to the lobby (`ClientCore::dismiss_notice`).
    #[default]
    Dismiss,
    /// "Use this tab" after `Replaced`.
    UseThisTab,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NoticeView {
    pub text: String,
    /// The caption of the button: "OK", "Back to lobby", "Use this tab".
    pub button: String,
    pub on_button: NoticeButton,
}

pub fn show(ctx: &egui::Context, view: &NoticeView) -> Vec<UiAction> {
    let mut actions = Vec::new();
    // Nothing behind the window takes a click.
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::unique("notice-dim"))
        .order(egui::Order::Middle)
        .fixed_pos(screen.min)
        .show(ctx, |ui| {
            ui.painter()
                .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(140));
            ui.allocate_rect(screen, egui::Sense::click());
        });
    egui::Window::new("Notice")
        .order(egui::Order::Foreground)
        .title_bar(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .collapsible(false)
        .resizable(false)
        .movable(false)
        .fixed_size([420.0, 0.0])
        .show(ctx, |ui| {
            ui.label(&view.text);
            ui.add_space(8.0);
            if ui.button(&view.button).clicked() {
                actions.push(match view.on_button {
                    NoticeButton::Dismiss => UiAction::DismissNotice,
                    NoticeButton::UseThisTab => UiAction::UseThisTab,
                });
            }
        });
    actions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::{click_at, find_widget};

    #[test]
    fn the_button_dismisses_or_takes_the_tab() {
        let mut v = NoticeView {
            text: "The game was stopped: the server restarts.".into(),
            button: "OK".into(),
            on_button: NoticeButton::Dismiss,
        };
        assert!(find_widget(&v.text, |ctx| show(ctx, &v)).is_some());
        let at = find_widget("OK", |ctx| show(ctx, &v)).expect("a button");
        assert_eq!(
            click_at(at.center(), false, |ctx| show(ctx, &v)),
            [UiAction::DismissNotice]
        );
        v.button = "Use this tab".into();
        v.on_button = NoticeButton::UseThisTab;
        let at = find_widget("Use this tab", |ctx| show(ctx, &v)).expect("a button");
        assert_eq!(
            click_at(at.center(), false, |ctx| show(ctx, &v)),
            [UiAction::UseThisTab]
        );
    }
}
