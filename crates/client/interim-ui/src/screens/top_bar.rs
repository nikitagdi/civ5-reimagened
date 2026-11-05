//! Top bar (play-ui §3.3, AC-7.1): science, gold, happiness, golden age, culture, faith on the
//! left, the research, turn and year on the right, as in the original TopPanel.

//!
//! A server game adds the cluster at the right (lobby-ui 4.7, LU-D8, LU-D19): the turn timer with the
//! host's [Pause] / [Resume], and the line to the server as a dot with its round trip time.

use super::UiAction;
use super::conn::Dot;

/// The turn timer of our seat's group (4.7 "Таймер хода").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TimerView {
    /// "1:23".
    pub text: String,
    /// Under 10 s: red.
    pub red: bool,
    /// "stopped (paused by host)", "stopped (waiting for Bob)"; the time is the one at the stop.
    pub stopped: Option<String>,
    /// We are the host and the timer is on: the button.
    pub can_pause: bool,
    /// The host's pause is on: the button says "Resume".
    pub paused: bool,
    /// The line is up (LU-D19): the button is grey.
    pub orders_enabled: bool,
}

/// "m:ss" of milliseconds left, rounded up to the second.
pub fn timer_text(left_ms: u64) -> String {
    let s = left_ms.div_ceil(1000);
    format!("{}:{:02}", s / 60, s % 60)
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TopBarView {
    pub turn: i32,
    /// Negative is BC.
    pub year: i32,
    /// The human's civilization.
    pub civ: String,
    pub gold: i32,
    pub gold_per_turn: i32,
    pub science_per_turn: i32,
    /// Tech being researched and turns to it.
    pub research: Option<(String, i16)>,
    /// Research points gathered and needed for it (hint).
    pub research_progress: i32,
    pub research_cost: i32,
    /// Culture stored towards the next policy, per turn, and the cost of that policy.
    pub culture: i32,
    pub culture_per_turn: i32,
    pub next_policy_cost: i32,
    pub happiness: i16,
    pub golden_age_turns: i16,
    /// Points towards the next golden age and the threshold (hint; no threshold = 0).
    pub golden_age_progress: i32,
    pub golden_age_threshold: i32,
    pub faith: i32,
    pub faith_per_turn: i32,
    pub era: Option<String>,
    /// Frames per second, shown small at the right end.
    pub fps: f32,
    /// Keyboard help for the "?" tooltip.
    pub help: String,
    /// The turn timer; `None`: off, or a local game.
    pub timer: Option<TimerView>,
    /// The line to the server: its dot and words ("Online, 35 ms"); `None` in a local game.
    pub line: Option<(Dot, String)>,
}

/// "4000 BC" / "1 AD" (TXT_KEY_TIME_BC / TXT_KEY_TIME_AD).
pub fn year_text(year: i32) -> String {
    if year < 0 {
        format!("{} BC", -i64::from(year))
    } else {
        format!("{year} AD")
    }
}

/// Turns to the next policy: what is missing over the culture per turn, rounded up, at least 1
/// (TopPanel's culture tooltip). `None` without culture per turn.
pub fn turns_to_policy(stored: i32, per_turn: i32, cost: i32) -> Option<i32> {
    if per_turn <= 0 || cost <= 0 {
        return None;
    }
    let missing = i64::from(cost) - i64::from(stored);
    let turns = (missing + i64::from(per_turn) - 1).div_euclid(i64::from(per_turn));
    Some(turns.clamp(1, i64::from(i32::MAX)) as i32)
}

pub fn show(ctx: &egui::Context, view: &TopBarView) -> Vec<UiAction> {
    let mut actions = Vec::new();
    let width = ctx.content_rect().width();
    let fill = ctx.global_style().visuals.panel_fill;
    egui::Area::new(egui::Id::unique("top-bar"))
        .anchor(egui::Align2::LEFT_TOP, [0.0, 0.0])
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(fill)
                .inner_margin(egui::Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    ui.set_width(width - 16.0);
                    bar(ui, view, &mut actions);
                });
        });
    actions
}

fn bar(ui: &mut egui::Ui, view: &TopBarView, actions: &mut Vec<UiAction>) {
    {
        ui.horizontal(|ui| {
            let science = egui::Color32::from_rgb(110, 170, 255);
            let gold = egui::Color32::from_rgb(255, 210, 80);
            let happy = egui::Color32::from_rgb(120, 220, 120);
            let unhappy = egui::Color32::from_rgb(240, 110, 90);
            let culture = egui::Color32::from_rgb(210, 120, 255);
            let faith = egui::Color32::from_rgb(230, 230, 230);
            ui.colored_label(science, format!("Science {:+}", view.science_per_turn))
                .on_hover_text("Science per turn");
            match &view.research {
                Some((tech, turns)) if *turns >= 0 => {
                    ui.colored_label(science, format!("{tech} ({turns} turns)"))
                }
                Some((tech, _)) => ui.colored_label(science, tech),
                None => ui.weak("no research"),
            }
            .on_hover_text(if view.research_cost > 0 {
                format!(
                    "Technology being researched: {}/{}",
                    view.research_progress, view.research_cost
                )
            } else {
                "Technology being researched".to_owned()
            });
            ui.separator();
            ui.colored_label(
                gold,
                format!("Gold {} ({:+})", view.gold, view.gold_per_turn),
            )
            .on_hover_text("Treasury and gold per turn");
            ui.separator();
            let (color, text) = if view.happiness < 0 {
                (unhappy, format!("Unhappiness {}", view.happiness))
            } else {
                (happy, format!("Happiness {}", view.happiness))
            };
            ui.colored_label(color, text)
                .on_hover_text("Happiness of the empire; negative is unhappiness");
            if view.golden_age_turns > 0 {
                ui.colored_label(gold, format!("Golden age: {} turns", view.golden_age_turns));
            } else if view.golden_age_threshold > 0 {
                ui.colored_label(
                    gold,
                    format!(
                        "Golden age {}/{}",
                        view.golden_age_progress, view.golden_age_threshold
                    ),
                )
                .on_hover_text("Golden age points towards the next golden age");
            }
            ui.separator();
            let policy =
                turns_to_policy(view.culture, view.culture_per_turn, view.next_policy_cost)
                    .map(|t| format!(", policy in {t}"))
                    .unwrap_or_default();
            ui.colored_label(
                culture,
                format!(
                    "Culture {}/{} ({:+}{policy})",
                    view.culture, view.next_policy_cost, view.culture_per_turn
                ),
            )
            .on_hover_text("Culture towards the next policy, per turn, turns to it");
            ui.separator();
            ui.colored_label(
                faith,
                format!("Faith {} ({:+})", view.faith, view.faith_per_turn),
            )
            .on_hover_text("Faith and faith per turn");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // The left side takes about 560 points; the extras go when the window is narrow.
                let width = ui.ctx().content_rect().width();
                // The rightmost: the settings window (lobby-ui 4.9).
                let r = ui.button("Settings");
                if r.clicked() {
                    r.surrender_focus();
                    actions.push(UiAction::ToggleSettings);
                }
                if width >= 900.0 {
                    ui.weak(format!("{:.0} fps", view.fps));
                }
                ui.label("?").on_hover_text(&view.help);
                if let Some((dot, text)) = &view.line {
                    ui.separator();
                    // Right to left: the words first, then the dot at their left.
                    ui.weak(text);
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 5.0, dot.color());
                }
                if let Some(t) = &view.timer {
                    ui.separator();
                    timer(ui, t, actions);
                }
                ui.separator();
                ui.strong(format!("Turn {}: {}", view.turn, year_text(view.year)));
                if let Some(era) = view.era.as_ref().filter(|_| width >= 1000.0) {
                    ui.label(era);
                }
                if width >= 1150.0 {
                    ui.label(&view.civ);
                }
            });
        });
    }
}

/// The timer and the host's button (the cluster is laid out right to left).
fn timer(ui: &mut egui::Ui, t: &TimerView, actions: &mut Vec<UiAction>) {
    if t.can_pause {
        let label = if t.paused { "Resume" } else { "Pause" };
        let hint = if t.orders_enabled {
            "Host: stop or start the turn timer. Orders still go through."
        } else {
            "No connection"
        };
        let r = ui
            .add_enabled(t.orders_enabled, egui::Button::new(label))
            .on_hover_text(hint)
            .on_disabled_hover_text(hint);
        if r.clicked() {
            r.surrender_focus();
            actions.push(UiAction::Pause(!t.paused));
        }
    }
    if let Some(why) = &t.stopped {
        ui.weak(why);
    }
    let mut text = egui::RichText::new(&t.text).strong();
    if t.red && t.stopped.is_none() {
        text = text.color(egui::Color32::from_rgb(240, 90, 80));
    }
    ui.label(text).on_hover_text("Time left in this turn");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::{click_at, find_widget, run_headless};

    fn timer_view(can_pause: bool, paused: bool, orders: bool) -> TopBarView {
        TopBarView {
            timer: Some(TimerView {
                text: "1:05".into(),
                can_pause,
                paused,
                orders_enabled: orders,
                ..TimerView::default()
            }),
            ..TopBarView::default()
        }
    }

    #[test]
    fn timer_texts() {
        assert_eq!(timer_text(65_000), "1:05");
        assert_eq!(timer_text(64_001), "1:05");
        assert_eq!(timer_text(0), "0:00");
        assert_eq!(timer_text(9_500), "0:10");
    }

    #[test]
    fn the_timer_is_shown_with_the_hosts_pause() {
        let v = timer_view(true, false, true);
        assert!(find_widget("1:05", |ctx| show(ctx, &v)).is_some());
        let at = find_widget("Pause", |ctx| show(ctx, &v))
            .expect("pause")
            .center();
        assert_eq!(
            click_at(at, false, |ctx| show(ctx, &v)),
            [UiAction::Pause(true)]
        );
        let v = timer_view(true, true, true);
        let at = find_widget("Resume", |ctx| show(ctx, &v))
            .expect("resume")
            .center();
        assert_eq!(
            click_at(at, false, |ctx| show(ctx, &v)),
            [UiAction::Pause(false)]
        );
    }

    #[test]
    fn only_the_host_has_the_button_and_not_without_a_line() {
        let v = timer_view(false, false, true);
        assert!(find_widget("Pause", |ctx| show(ctx, &v)).is_none());
        assert!(find_widget("1:05", |ctx| show(ctx, &v)).is_some());
        let v = timer_view(true, false, false);
        let at = find_widget("Pause", |ctx| show(ctx, &v))
            .expect("pause")
            .center();
        assert!(click_at(at, false, |ctx| show(ctx, &v)).is_empty());
    }

    #[test]
    fn a_stopped_timer_says_why() {
        let mut v = timer_view(false, true, true);
        v.timer.as_mut().unwrap().stopped = Some("stopped (paused by host)".into());
        assert!(find_widget("stopped (paused by host)", |ctx| show(ctx, &v)).is_some());
    }

    #[test]
    fn the_line_is_a_dot_and_words() {
        let v = TopBarView {
            line: Some((Dot::Green, "Online, 35 ms".into())),
            ..TopBarView::default()
        };
        assert!(find_widget("Online, 35 ms", |ctx| show(ctx, &v)).is_some());
    }

    #[test]
    fn the_settings_button_opens_the_window() {
        let v = TopBarView::default();
        let at = find_widget("Settings", |ctx| show(ctx, &v))
            .expect("settings")
            .center();
        assert_eq!(
            click_at(at, false, |ctx| show(ctx, &v)),
            [UiAction::ToggleSettings]
        );
    }

    #[test]
    fn years_and_policy_turns() {
        assert_eq!(year_text(-4000), "4000 BC");
        assert_eq!(year_text(1), "1 AD");
        assert_eq!(turns_to_policy(10, 3, 25), Some(5));
        assert_eq!(turns_to_policy(30, 3, 25), Some(1), "at least one");
        assert_eq!(turns_to_policy(0, 0, 25), None);
    }

    #[test]
    fn renders_the_numbers() {
        let v = TopBarView {
            turn: 3,
            year: -3900,
            civ: "Rome".into(),
            gold: 12,
            gold_per_turn: 3,
            research: Some(("Pottery".into(), 7)),
            ..TopBarView::default()
        };
        let out = run_headless(|ctx| {
            show(ctx, &v);
        });
        let texts: Vec<String> = out
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some(t.galley.job.text.clone()),
                _ => None,
            })
            .collect();
        for want in ["Gold 12 (+3)", "Turn 3: 3900 BC", "Pottery (7 turns)"] {
            assert!(texts.iter().any(|t| t == want), "{want} in {texts:?}");
        }
    }

    #[test]
    fn shows_every_value_of_the_core() {
        let v = TopBarView {
            turn: 40,
            year: 120,
            civ: "Rome".into(),
            gold: 77,
            gold_per_turn: -2,
            science_per_turn: 5,
            culture: 10,
            culture_per_turn: 4,
            next_policy_cost: 30,
            happiness: -3,
            golden_age_progress: 20,
            golden_age_threshold: 200,
            faith: 14,
            faith_per_turn: 1,
            era: Some("Classical era".into()),
            research: Some(("Writing".into(), 3)),
            ..TopBarView::default()
        };
        let out = run_headless(|ctx| {
            show(ctx, &v);
        });
        let texts: Vec<String> = out
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some(t.galley.job.text.clone()),
                _ => None,
            })
            .collect();
        for want in [
            "Science +5",
            "Gold 77 (-2)",
            "Unhappiness -3",
            "Golden age 20/200",
            "Culture 10/30 (+4, policy in 5)",
            "Faith 14 (+1)",
            "Classical era",
            "Turn 40: 120 AD",
        ] {
            assert!(texts.iter().any(|t| t == want), "{want} in {texts:?}");
        }
    }
}
