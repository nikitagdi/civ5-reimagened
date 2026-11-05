//! Lobby (play-ui §3.3, AC-1.2): civilization, difficulty, rivals, seed; Start. Continue when a
//! save exists.
//!
//! SIMPLIFIED(SIMP-018): one small setup window; world size, map script and every other option are
//! fixed by the host; original: the main menu and the "Set up game" screen.
//!
//! B1 only: the server session has `rooms` and `room`. This window, `LobbyForm` and `NewGameChoice` go
//! with the local game (N4.5), which is why N4.9 did not delete them.

use super::UiAction;

/// A pick-list entry (it lives in `room` now; the B1 lobby goes with the local game, N4.5).
pub use super::room::Choice;

/// What the host offers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LobbyView {
    /// Playable civilizations (barbarians and city-states already left out by the host).
    pub civs: Vec<Choice>,
    pub handicaps: Vec<Choice>,
    /// The fixed map, as text ("Duel, Lekmap Pangaea").
    pub map: String,
    /// The newest save, as text ("Rome, turn 12"), if any.
    pub save: Option<String>,
    /// Shown under the buttons (a refused start).
    pub error: Option<String>,
}

/// The choices of the player, kept between frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LobbyForm {
    /// Index into [`LobbyView::civs`]; `None` is "Random".
    pub civ: Option<usize>,
    /// Index into [`LobbyView::handicaps`].
    pub handicap: usize,
    /// 1-2 (play-ui §3.3).
    pub ai_count: u8,
    /// Empty means random.
    pub seed: String,
}

/// The difficulty preselected in the lobby (play-ui §3.3).
pub const DEFAULT_HANDICAP: &str = "HANDICAP_PRINCE";
pub const MAX_AI: u8 = 2;

impl Default for LobbyForm {
    fn default() -> Self {
        Self::new(&LobbyView::default())
    }
}

impl LobbyForm {
    /// Random civilization, Prince (or the first difficulty), one rival, random seed.
    pub fn new(view: &LobbyView) -> Self {
        Self {
            civ: None,
            handicap: view
                .handicaps
                .iter()
                .position(|h| h.key == DEFAULT_HANDICAP)
                .unwrap_or(0),
            ai_count: 1,
            seed: String::new(),
        }
    }

    /// The seed field as a number: `Ok(None)` when empty (random), `Err` when not a `u32`.
    pub fn parsed_seed(&self) -> Result<Option<u32>, String> {
        let s = self.seed.trim();
        if s.is_empty() {
            return Ok(None);
        }
        s.parse::<u32>()
            .map(Some)
            .map_err(|_| format!("the seed must be a whole number from 0 to {}", u32::MAX))
    }

    /// The choice the Start button sends.
    pub fn choice(&self, view: &LobbyView) -> Result<NewGameChoice, String> {
        Ok(NewGameChoice {
            civ: self
                .civ
                .and_then(|i| view.civs.get(i))
                .map(|c| c.key.clone()),
            handicap: view.handicaps.get(self.handicap).map(|h| h.key.clone()),
            ai_count: self.ai_count.clamp(1, MAX_AI),
            seed: self.parsed_seed()?,
        })
    }
}

/// A new game as chosen in the lobby. `None` fields keep the host's default (random civ,
/// default difficulty, random seed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewGameChoice {
    pub civ: Option<String>,
    pub handicap: Option<String>,
    pub ai_count: u8,
    pub seed: Option<u32>,
}

pub fn show(ctx: &egui::Context, view: &LobbyView, form: &mut LobbyForm) -> Vec<UiAction> {
    let mut actions = Vec::new();
    egui::Window::new("New game")
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .collapsible(false)
        .resizable(false)
        .movable(false)
        .default_width(360.0)
        .show(ctx, |ui| {
            egui::Grid::new("lobby")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Civilization");
                    // A list in the window, not a combo box: the scrolled popup of a long combo
                    // box was not drawn in the browser (it took clicks while invisible).
                    egui::ScrollArea::vertical()
                        .id_salt("lobby-civ")
                        .max_height(150.0)
                        // The grid row would otherwise keep it as low as it was last frame.
                        .min_scrolled_height(150.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.set_width(220.0);
                            // The grid cell lays out left to right; the list goes down.
                            ui.vertical(|ui| {
                                ui.selectable_value(&mut form.civ, None, "Random");
                                for (i, c) in view.civs.iter().enumerate() {
                                    ui.selectable_value(&mut form.civ, Some(i), &c.name);
                                }
                            });
                        });
                    ui.end_row();

                    ui.label("Difficulty");
                    let selected = view
                        .handicaps
                        .get(form.handicap)
                        .map_or("Default", |h| h.name.as_str());
                    egui::ComboBox::from_id_salt("lobby-handicap")
                        .selected_text(selected)
                        .width(220.0)
                        .show_ui(ui, |ui| {
                            for (i, h) in view.handicaps.iter().enumerate() {
                                ui.selectable_value(&mut form.handicap, i, &h.name);
                            }
                        });
                    ui.end_row();

                    ui.label("Rivals");
                    ui.horizontal(|ui| {
                        for n in 1..=MAX_AI {
                            ui.radio_value(&mut form.ai_count, n, n.to_string());
                        }
                    });
                    ui.end_row();

                    ui.label("Seed");
                    ui.add(
                        egui::TextEdit::singleline(&mut form.seed)
                            .hint_text("random")
                            .desired_width(220.0),
                    );
                    ui.end_row();

                    ui.label("Map");
                    ui.label(&view.map);
                    ui.end_row();
                });
            ui.separator();
            let choice = form.choice(view);
            ui.horizontal(|ui| {
                let start = ui.add_enabled(choice.is_ok(), egui::Button::new("Start"));
                if start.clicked()
                    && let Ok(c) = &choice
                {
                    actions.push(UiAction::StartGame(c.clone()));
                }
                if let Some(save) = &view.save
                    && ui.button(format!("Continue: {save}")).clicked()
                {
                    actions.push(UiAction::ContinueGame);
                }
            });
            if let Err(e) = &choice {
                ui.colored_label(ui.visuals().warn_fg_color, e);
            }
            if let Some(e) = &view.error {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
        });
    actions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::{click_at, find_widget, run_headless};

    fn view() -> LobbyView {
        let c = |k: &str, n: &str| Choice {
            key: k.into(),
            name: n.into(),
        };
        LobbyView {
            civs: vec![
                c("CIVILIZATION_ROME", "Rome"),
                c("CIVILIZATION_EGYPT", "Egypt"),
            ],
            handicaps: vec![
                c("HANDICAP_SETTLER", "Settler"),
                c("HANDICAP_PRINCE", "Prince"),
            ],
            map: "Duel, Pangaea".into(),
            save: None,
            error: None,
        }
    }

    #[test]
    fn defaults_are_random_prince_one_rival() {
        let v = view();
        let f = LobbyForm::new(&v);
        assert_eq!(
            f.choice(&v),
            Ok(NewGameChoice {
                civ: None,
                handicap: Some("HANDICAP_PRINCE".into()),
                ai_count: 1,
                seed: None,
            })
        );
    }

    #[test]
    fn a_click_in_the_civilization_list_picks_it() {
        let v = view();
        let mut f = LobbyForm::new(&v);
        let at = find_widget("Egypt", |ctx| show(ctx, &v, &mut f)).expect("the list");
        let mut f = LobbyForm::new(&v);
        click_at(at.center(), false, |ctx| show(ctx, &v, &mut f));
        assert_eq!(f.civ, Some(1));
    }
    #[test]
    fn a_long_civilization_list_is_laid_out_downwards() {
        let mut v = view();
        v.civs = (0..45)
            .map(|i| Choice {
                key: format!("CIVILIZATION_{i}"),
                name: format!("Civ number {i}"),
            })
            .collect();
        let mut f = LobbyForm::new(&v);
        let a = find_widget("Civ number 0", |ctx| show(ctx, &v, &mut f)).unwrap();
        let mut f = LobbyForm::new(&v);
        let b = find_widget("Civ number 3", |ctx| show(ctx, &v, &mut f)).unwrap();
        assert!(b.min.y > a.min.y + 30.0, "{a:?} {b:?}");
    }
    #[test]
    fn seed_must_be_a_number() {
        let v = view();
        let mut f = LobbyForm::new(&v);
        f.seed = " 42 ".into();
        f.civ = Some(1);
        f.ai_count = 9;
        let c = f.choice(&v).unwrap();
        assert_eq!(
            (c.seed, c.civ.as_deref(), c.ai_count),
            (Some(42), Some("CIVILIZATION_EGYPT"), 2)
        );
        f.seed = "abc".into();
        assert!(f.choice(&v).is_err());
    }

    #[test]
    fn renders_and_start_sends_the_choice() {
        let v = view();
        let mut f = LobbyForm::new(&v);
        let out = run_headless(|ctx| {
            show(ctx, &v, &mut f);
        });
        assert!(!out.is_empty());
        let mut f = LobbyForm::new(&v);
        let start = find_widget("Start", |ctx| show(ctx, &v, &mut f)).expect("a Start button");
        let mut f = LobbyForm::new(&v);
        let actions = click_at(start.center(), false, |ctx| show(ctx, &v, &mut f));
        assert_eq!(
            actions,
            [UiAction::StartGame(LobbyForm::new(&v).choice(&v).unwrap())]
        );
    }
}
