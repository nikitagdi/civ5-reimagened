//! Loading screen (play-ui §3.3, AC-1.3): the stage ("Generating map..."), the host's progress
//! line when it sends one, and a bar over the stages.
//!
//! SIMPLIFIED(SIMP-018): a text window with a stage bar; original: the loading screen with the
//! leader portrait and the civilization's description.

use super::UiAction;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LoadingView {
    /// "Generating map..."
    pub stage: String,
    /// The newest progress line of the host ("game data: 120/388 files").
    pub detail: Option<String>,
    /// 1-based stage and the number of stages, for the bar.
    pub step: u32,
    pub steps: u32,
    /// Seconds in this stage.
    pub elapsed_s: f32,
}

impl LoadingView {
    /// The bar: the finished stages, plus a share of the current one that approaches but never
    /// reaches its end (stages have no measured length).
    pub fn fraction(&self) -> f32 {
        if self.steps == 0 {
            return 0.0;
        }
        let done = self.step.saturating_sub(1).min(self.steps) as f32;
        let within = 1.0 - (-self.elapsed_s.max(0.0) / 4.0).exp();
        ((done + within * 0.9) / self.steps as f32).clamp(0.0, 1.0)
    }
}

pub fn show(ctx: &egui::Context, view: &LoadingView) -> Vec<UiAction> {
    egui::Window::new("Loading")
        .title_bar(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .resizable(false)
        .movable(false)
        .fixed_size([380.0, 0.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.heading(&view.stage);
            });
            ui.add(
                egui::ProgressBar::new(view.fraction())
                    .text(format!("step {} of {}", view.step, view.steps)),
            );
            if let Some(d) = &view.detail {
                ui.label(d);
            }
            ui.weak(format!("{:.0} s", view.elapsed_s));
        });
    // The spinner and the bar move without input.
    ctx.request_repaint();
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::run_headless;

    #[test]
    fn fraction_grows_within_a_stage_and_stays_inside_it() {
        let mut v = LoadingView {
            stage: "Generating map...".into(),
            step: 2,
            steps: 4,
            ..LoadingView::default()
        };
        assert_eq!(v.fraction(), 0.25);
        v.elapsed_s = 100.0;
        assert!(v.fraction() > 0.45 && v.fraction() < 0.5);
        v.steps = 0;
        assert_eq!(v.fraction(), 0.0);
    }

    #[test]
    fn renders() {
        let v = LoadingView {
            stage: "Generating map...".into(),
            detail: Some("game data: 1/2 files".into()),
            step: 1,
            steps: 4,
            elapsed_s: 1.0,
        };
        let out = run_headless(|ctx| {
            assert!(show(ctx, &v).is_empty());
        });
        assert!(!out.is_empty());
    }
}
