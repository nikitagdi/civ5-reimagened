//! End-turn button (play-ui §3.3, AC-6.1, AC-6.4), bottom right: "Next turn", the blocker
//! ("Unit needs orders", "Choose production", ...) or "Waiting for rivals" while the AI moves.
//! A click does what Enter does (go to the blocker, or end the turn); Shift+click is Shift+Enter
//! (end the turn anyway).
//!
//! A server game adds the states of lobby-ui 4.7: the humans we wait for (with [Cancel end turn] under
//! the button, LU-D20), the hybrid turn of another player, the start of the turn, and the grey
//! button without a connection (LU-D19).

use super::UiAction;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum EndTurnState {
    /// Nothing blocks: the button ends the turn.
    #[default]
    Ready,
    /// The raw `EndTurnBlockingTypes` value of the blocker.
    Blocked(i32),
    /// The rivals are moving; the civilization named by the last `TurnProgress`, if known.
    Waiting(Option<String>),
    /// Our end of turn is set; these humans have not finished (names, nick or civilization).
    WaitingHumans { names: Vec<String> },
    /// Hybrid mode: another player has the turn now (`may_act = false`).
    WaitingTurn { who: String },
    /// The server prepares the turn.
    Starting,
    /// The text of the end ("Game over: Rome wins").
    GameOver(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EndTurnView {
    pub state: EndTurnState,
    /// A refused order ("Refused: ..."), the order that cancelled the end of turn, shown above the
    /// button for a few seconds.
    pub note: Option<String>,
    /// The line is down (LU-D19): the button and [Cancel end turn] are grey.
    pub no_connection: bool,
}

/// The button text of a blocker: the core's `EndTurnBlockingTypes` (CvEnums.h) in the words of
/// the original button (TXT_KEY_*_BLOCKING / ActionInfoPanel).
pub fn blocker_label(kind: i32) -> &'static str {
    match kind {
        0 => "Choose policy",
        1 => "Choose research",
        2 => "Choose production",
        3 => "Unit needs orders",
        4 => "Diplomatic vote",
        5 => "City-state quest",
        6 => "Choose free tech",
        7 => "Units stacked",
        8 => "Unit needs orders",
        9 => "Unit promotion",
        10 => "City can attack",
        11 => "Choose free policy",
        12 => "Choose free item",
        13 => "Found a pantheon",
        14 => "Found a religion",
        15 => "Enhance religion",
        16 => "Steal tech",
        17 => "Maya calendar",
        18 => "Choose great person",
        19 => "Add reformation belief",
        20 => "World Congress proposal",
        21 => "Choose archaeology",
        22 => "World Congress vote",
        23 => "Choose ideology",
        _ => "Turn blocked",
    }
}

impl EndTurnView {
    pub fn label(&self) -> String {
        match &self.state {
            EndTurnState::Ready => "Next turn".to_owned(),
            EndTurnState::Blocked(kind) => blocker_label(*kind).to_owned(),
            EndTurnState::Waiting(Some(civ)) => format!("Waiting for rivals ({civ})..."),
            EndTurnState::Waiting(None) => "Waiting for rivals...".to_owned(),
            EndTurnState::WaitingHumans { names } => match names.len() {
                0 => "Waiting for the turn to end...".to_owned(),
                1 => "Waiting for 1 player".to_owned(),
                n => format!("Waiting for {n} players"),
            },
            EndTurnState::WaitingTurn { who } => format!("Waiting for {who} (their war turn)"),
            EndTurnState::Starting => "Preparing the turn...".to_owned(),
            EndTurnState::GameOver(text) => text.clone(),
        }
    }

    fn clickable(&self) -> bool {
        !self.no_connection && matches!(self.state, EndTurnState::Ready | EndTurnState::Blocked(_))
    }

    /// The hover text over the button.
    fn hint(&self) -> String {
        match &self.state {
            _ if self.no_connection => "No connection".to_owned(),
            EndTurnState::Blocked(_) => {
                "Click or Enter: go to it. Shift+click or Shift+Enter: end the turn anyway."
                    .to_owned()
            }
            EndTurnState::WaitingHumans { names } if !names.is_empty() => {
                format!("Waiting for: {}", names.join(", "))
            }
            _ => "Click or Enter: end the turn.".to_owned(),
        }
    }

    fn spins(&self) -> bool {
        matches!(
            self.state,
            EndTurnState::Waiting(_) | EndTurnState::Starting | EndTurnState::WaitingHumans { .. }
        )
    }
}

pub fn show(ctx: &egui::Context, view: &EndTurnView) -> Vec<UiAction> {
    let mut actions = Vec::new();
    egui::Area::new(egui::Id::unique("end-turn"))
        .anchor(egui::Align2::RIGHT_BOTTOM, [-12.0, -12.0])
        .show(ctx, |ui| {
            egui::Frame::window(ui.style()).show(ui, |ui| {
                ui.vertical(|ui| {
                    if let Some(note) = &view.note {
                        ui.colored_label(ui.visuals().warn_fg_color, note);
                    }
                    let text = egui::RichText::new(view.label()).size(18.0).strong();
                    ui.horizontal(|ui| {
                        if view.spins() && !view.no_connection {
                            ui.spinner();
                        }
                        let button = egui::Button::new(text).min_size(egui::vec2(220.0, 44.0));
                        let hint = view.hint();
                        let r = ui
                            .add_enabled(view.clickable(), button)
                            .on_hover_text(&hint)
                            .on_disabled_hover_text(hint);
                        if r.clicked() {
                            // The button must not keep the focus (Enter would press it again).
                            r.surrender_focus();
                            let force = ui.input(|i| i.modifiers.shift);
                            actions.push(UiAction::EndTurn { force });
                        }
                    });
                    if matches!(view.state, EndTurnState::WaitingHumans { .. }) {
                        let cancel =
                            egui::Button::new("Cancel end turn").min_size(egui::vec2(220.0, 26.0));
                        let r = ui.add_enabled(!view.no_connection, cancel);
                        let r = r.on_disabled_hover_text("No connection");
                        if r.clicked() {
                            r.surrender_focus();
                            actions.push(UiAction::CancelEndTurn);
                        }
                    }
                });
            });
        });
    if view.spins() && !view.no_connection {
        ctx.request_repaint();
    }
    actions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::{click_at, find_widget};

    fn view(state: EndTurnState) -> EndTurnView {
        EndTurnView {
            state,
            ..EndTurnView::default()
        }
    }

    #[test]
    fn labels() {
        assert_eq!(view(EndTurnState::Ready).label(), "Next turn");
        assert_eq!(view(EndTurnState::Blocked(8)).label(), "Unit needs orders");
        assert_eq!(view(EndTurnState::Blocked(2)).label(), "Choose production");
        assert_eq!(view(EndTurnState::Blocked(99)).label(), "Turn blocked");
        assert_eq!(
            view(EndTurnState::Waiting(Some("Egypt".into()))).label(),
            "Waiting for rivals (Egypt)..."
        );
    }

    #[test]
    fn click_is_enter_and_shift_click_forces() {
        let v = view(EndTurnState::Blocked(8));
        let at = find_widget("Unit needs orders", |ctx| show(ctx, &v))
            .expect("the button")
            .center();
        assert_eq!(
            click_at(at, false, |ctx| show(ctx, &v)),
            [UiAction::EndTurn { force: false }]
        );
        assert_eq!(
            click_at(at, true, |ctx| show(ctx, &v)),
            [UiAction::EndTurn { force: true }]
        );
    }

    #[test]
    fn server_game_states() {
        let humans = view(EndTurnState::WaitingHumans {
            names: vec!["Bob".into(), "Eve".into()],
        });
        assert_eq!(humans.label(), "Waiting for 2 players");
        assert_eq!(humans.hint(), "Waiting for: Bob, Eve");
        assert!(!humans.clickable());
        let one = view(EndTurnState::WaitingHumans {
            names: vec!["Bob".into()],
        });
        assert_eq!(one.label(), "Waiting for 1 player");
        assert_eq!(
            view(EndTurnState::WaitingTurn { who: "Bob".into() }).label(),
            "Waiting for Bob (their war turn)"
        );
        assert_eq!(
            view(EndTurnState::Starting).label(),
            "Preparing the turn..."
        );
    }

    #[test]
    fn cancel_end_turn_only_while_waiting_for_humans() {
        let v = view(EndTurnState::WaitingHumans {
            names: vec!["Bob".into()],
        });
        let at = find_widget("Cancel end turn", |ctx| show(ctx, &v))
            .expect("the cancel button")
            .center();
        assert_eq!(
            click_at(at, false, |ctx| show(ctx, &v)),
            [UiAction::CancelEndTurn]
        );
        for state in [
            EndTurnState::Ready,
            EndTurnState::Blocked(2),
            EndTurnState::Waiting(None),
            EndTurnState::Starting,
        ] {
            let v = view(state);
            assert!(find_widget("Cancel end turn", |ctx| show(ctx, &v)).is_none());
        }
    }

    #[test]
    fn no_connection_greys_the_button_and_cancel() {
        let mut v = view(EndTurnState::Ready);
        v.no_connection = true;
        let at = find_widget("Next turn", |ctx| show(ctx, &v))
            .expect("the button")
            .center();
        assert!(click_at(at, false, |ctx| show(ctx, &v)).is_empty());
        assert_eq!(v.hint(), "No connection");
        let v = EndTurnView {
            state: EndTurnState::WaitingHumans {
                names: vec!["Bob".into()],
            },
            no_connection: true,
            ..EndTurnView::default()
        };
        let at = find_widget("Cancel end turn", |ctx| show(ctx, &v))
            .expect("the cancel button")
            .center();
        assert!(click_at(at, false, |ctx| show(ctx, &v)).is_empty());
    }

    #[test]
    fn waiting_is_not_clickable() {
        let v = view(EndTurnState::Waiting(None));
        let at = find_widget("Waiting for rivals...", |ctx| show(ctx, &v))
            .expect("the button")
            .center();
        assert!(click_at(at, false, |ctx| show(ctx, &v)).is_empty());
    }
}
