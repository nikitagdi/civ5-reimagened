//! The line to the server on the screens (lobby-ui LU-D12, LU-D19, 4.7): a coloured dot and words, and the
//! strip "No connection. Reconnecting... attempt N, next in S s".
//!
//! Dots are drawn with the painter, never glyphs: the coverage of the built-in font is not checked.

use serde::Serialize;

use super::UiAction;

/// The state of the line, as a colour.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub enum Dot {
    /// `Online`.
    #[default]
    Green,
    /// `Poor`: the answers are slow.
    Yellow,
    /// `Lost`, `Reconnecting`, `Offline`.
    Red,
    /// Nothing to say (no session yet).
    Grey,
}

impl Dot {
    pub fn color(self) -> egui::Color32 {
        match self {
            Dot::Green => egui::Color32::from_rgb(80, 200, 90),
            Dot::Yellow => egui::Color32::from_rgb(230, 190, 50),
            Dot::Red => egui::Color32::from_rgb(220, 70, 60),
            Dot::Grey => egui::Color32::from_rgb(150, 150, 150),
        }
    }
}

/// What the line looks like now (net-client `ConnState`, `reconnect_status`, `orders_enabled`).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ConnView {
    pub dot: Dot,
    /// "Online, 35 ms", "Poor connection", "No connection". Not in the DOM page's JSON: the ping changes
    /// every few seconds and would redraw the page each time (the page words the dot itself).
    #[serde(skip)]
    pub text: String,
    /// The screens that give orders are grey when `false` (LU-D19).
    pub orders_enabled: bool,
    /// "No connection. Reconnecting... attempt 2, next in 3 s"; `None` while the line is up.
    pub status: Option<String>,
}

/// A dot and a word, in a row.
pub fn dot(ui: &mut egui::Ui, dot: Dot, text: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 5.0, dot.color());
        ui.label(text);
    });
}

/// The strip at the top of the screen while the line is down. It takes no clicks.
pub fn show_strip(ctx: &egui::Context, view: &ConnView) -> Vec<UiAction> {
    let Some(status) = &view.status else {
        return Vec::new();
    };
    egui::Area::new(egui::Id::unique("conn-strip"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_TOP, [0.0, 4.0])
        .interactable(false)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .fill(egui::Color32::from_rgb(110, 40, 36))
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(status).color(egui::Color32::WHITE));
                });
        });
    // The attempt counter ticks on its own.
    ctx.request_repaint_after(std::time::Duration::from_millis(250));
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::{find_widget, run_headless};

    #[test]
    fn the_strip_shows_only_without_a_line() {
        let down = ConnView {
            dot: Dot::Red,
            text: "No connection".into(),
            orders_enabled: false,
            status: Some("No connection. Reconnecting... attempt 2, next in 3 s".into()),
        };
        assert!(
            find_widget(
                "No connection. Reconnecting... attempt 2, next in 3 s",
                |ctx| { show_strip(ctx, &down) }
            )
            .is_some()
        );
        let up = ConnView::default();
        assert!(run_headless(|ctx| drop(show_strip(ctx, &up))).is_empty());
    }

    #[test]
    fn the_colours_differ() {
        let all = [Dot::Green, Dot::Yellow, Dot::Red, Dot::Grey].map(Dot::color);
        for (i, a) in all.iter().enumerate() {
            assert!(all[i + 1..].iter().all(|b| a != b));
        }
    }
}
