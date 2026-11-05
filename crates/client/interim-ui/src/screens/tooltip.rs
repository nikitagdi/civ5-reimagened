//! The tooltip over the hovered plot (AC-7.4): the lines are built by the game (terrain, feature,
//! resource, improvement, owner, yields, units), the screen only draws them next to the pointer.
//! Not over a unit flag or a city banner: those have their own tooltip.
//!
//! SIMPLIFIED(SIMP-001): a plain egui frame with text lines; original: the plot help text of the
//! `InfoTooltipInclude` / `PlotHelpManager` Lua with icons and colours.

use super::UiAction;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TooltipView {
    /// The first line is the title.
    pub lines: Vec<String>,
}

/// Where the tooltip sits relative to the pointer (points).
const OFFSET: egui::Vec2 = egui::vec2(16.0, 18.0);

pub fn show(ctx: &egui::Context, view: &TooltipView) -> Vec<UiAction> {
    let Some(pointer) = ctx.input(|i| i.pointer.hover_pos()) else {
        return Vec::new();
    };
    if view.lines.is_empty() || crate::labels::hit(ctx, pointer) {
        return Vec::new();
    }
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::unique("plot-tooltip"))
        .order(egui::Order::Tooltip)
        .interactable(false)
        .fixed_pos(pointer + OFFSET)
        .constrain_to(screen)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                for (i, line) in view.lines.iter().enumerate() {
                    if i == 0 {
                        ui.strong(line);
                    } else {
                        ui.label(line);
                    }
                }
            });
        });
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::find_widget;

    #[test]
    fn draws_every_line_near_the_pointer_only_with_a_pointer() {
        let view = TooltipView {
            lines: vec!["Grassland".into(), "Owner: Egypt".into()],
        };
        // No pointer, no tooltip.
        assert!(find_widget("Owner: Egypt", |ctx| show(ctx, &view)).is_none());
        let ctx = egui::Context::default();
        let at = egui::pos2(300.0, 200.0);
        let raw = |events| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 720.0),
            )),
            events,
            ..egui::RawInput::default()
        };
        let mut found = Vec::new();
        for i in 0..4 {
            let events = if i == 0 {
                vec![egui::Event::PointerMoved(at)]
            } else {
                vec![]
            };
            let out = ctx.run_pass(raw(events), |ctx| drop(show(ctx, &view)));
            found = out
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) => {
                        Some((t.galley.job.text.clone(), t.pos, t.galley.rect.size()))
                    }
                    _ => None,
                })
                .collect();
            out.drop_without_applying_deltas();
        }
        let texts: Vec<&str> = found.iter().map(|(t, _, _)| t.as_str()).collect();
        assert_eq!(texts, ["Grassland", "Owner: Egypt"]);
        // Right of and below the pointer, the title above the second line.
        assert!(found[0].1.x > at.x && found[0].1.y > at.y);
        assert!(found[1].1.y > found[0].1.y);
    }
}
