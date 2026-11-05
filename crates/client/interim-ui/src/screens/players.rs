//! The players panel of a server game (lobby-ui 4.7, LU-D5, LU-D6, LU-D8, LU-D18, LU-D21), left under the
//! top bar: a row per major civilization with its colour, name (the nick of a human, "AI"), the end-of-turn
//! mark, the line, and the buttons: declare war, propose, accept or decline peace, kick (host only).
//! The note above the rows says why the server refused a host command, or who the host is now.
//!
//! The screen knows no protocol: ids are plain numbers, the dangerous clicks (war, kick) are
//! [`UiAction`]s that the shell turns into a yes / no question (LU-D7).

use super::UiAction;
use super::conn::Dot;

/// The "Turn" column: only humans have one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TurnMark {
    #[default]
    None,
    /// The end of turn is set.
    Done,
    /// May move and has not finished (also after an order took the end of turn back, LU-D20).
    Thinking,
    /// Hybrid mode: waits for its turn.
    Waiting,
}

/// The "Line" column.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum PlayerLine {
    #[default]
    Online,
    /// Lost for this long ("1:05"), from the server's clock.
    Lost(String),
    /// An AI plays the seat: a grey dot.
    Ai,
}

/// How we stand with the civilization of the row (`PlayerFlags::MET`, a live one, not us).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Diplomacy {
    /// [Declare war]
    Peace,
    /// War with a human, no offer: [Propose peace].
    WarHuman,
    /// War, our offer is out: "Peace offered".
    WarOffered,
    /// War, their offer to us: [Accept] [Decline]; the row is lit.
    WarIncoming,
    /// War with an AI: nothing to press (N-D15).
    WarAi,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlayerRow {
    pub player: i8,
    pub team: i8,
    /// RGB of the civilization's primary colour.
    pub color: [u8; 3],
    /// "Rome".
    pub civ: String,
    /// The nick of a human.
    pub nick: Option<String>,
    pub me: bool,
    /// `SeatView.host`.
    pub host: bool,
    /// The host is away: seconds left of the 60 before the role passes on.
    pub host_passes_in_s: Option<u32>,
    pub turn: TurnMark,
    pub line: PlayerLine,
    /// `None`: not met, dead or us: no buttons.
    pub diplomacy: Option<Diplomacy>,
    /// We are the host and this is another human.
    pub can_kick: bool,
}

impl PlayerRow {
    /// "Rome (Bob)", "Rome (AI)".
    pub fn name(&self) -> String {
        match &self.nick {
            Some(nick) => format!("{} ({nick})", self.civ),
            None => format!("{} (AI)", self.civ),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlayersView {
    pub rows: Vec<PlayerRow>,
    /// Unfolded; folded it is the one line "Players (n)".
    pub open: bool,
    /// The refusal of a host command or the new host, for a few seconds (LU-D18, LU-D21).
    pub note: Option<String>,
    /// The line is down (LU-D19): the buttons are grey.
    pub orders_enabled: bool,
}

const HOST_HINT: &str = "Host: can remove players and pause the timer. If the host is away for 60 s, the role passes to the next player.";
const NO_CONNECTION: &str = "No connection";
const LIT: egui::Color32 = egui::Color32::from_rgb(90, 60, 20);
const DANGER: egui::Color32 = egui::Color32::from_rgb(150, 45, 40);

fn turn_text(m: TurnMark) -> &'static str {
    match m {
        TurnMark::None => "",
        TurnMark::Done => "Done",
        TurnMark::Thinking => "Thinking",
        TurnMark::Waiting => "Waiting",
    }
}

fn dot_of(line: &PlayerLine) -> Dot {
    match line {
        PlayerLine::Online => Dot::Green,
        PlayerLine::Lost(_) => Dot::Red,
        PlayerLine::Ai => Dot::Grey,
    }
}

/// A button that is grey with the reason when the line is down.
fn button(
    ui: &mut egui::Ui,
    enabled: bool,
    text: &str,
    fill: Option<egui::Color32>,
    hint: &str,
) -> bool {
    let mut b = egui::Button::new(text);
    if let Some(fill) = fill {
        b = b.fill(fill);
    }
    let hint = if enabled { hint } else { NO_CONNECTION };
    let r = ui
        .add_enabled(enabled, b)
        .on_hover_text(hint)
        .on_disabled_hover_text(hint);
    if r.clicked() {
        r.surrender_focus();
        true
    } else {
        false
    }
}

pub fn show(ctx: &egui::Context, view: &PlayersView) -> Vec<UiAction> {
    let mut actions = Vec::new();
    egui::Area::new(egui::Id::unique("players"))
        .anchor(egui::Align2::LEFT_TOP, [12.0, 42.0])
        .show(ctx, |ui| {
            egui::Frame::window(ui.style()).show(ui, |ui| {
                if let Some(note) = &view.note {
                    ui.colored_label(ui.visuals().warn_fg_color, note);
                }
                let arrow = if view.open { "v" } else { ">" };
                let r = ui.button(format!("{arrow} Players ({})", view.rows.len()));
                if r.clicked() {
                    r.surrender_focus();
                    actions.push(UiAction::TogglePlayers);
                }
                if view.open {
                    for row in &view.rows {
                        show_row(ui, view, row, &mut actions);
                    }
                }
            });
        });
    actions
}

fn show_row(ui: &mut egui::Ui, view: &PlayersView, row: &PlayerRow, actions: &mut Vec<UiAction>) {
    let on = view.orders_enabled;
    let lit = row.diplomacy == Some(Diplomacy::WarIncoming);
    let fill = if lit { LIT } else { egui::Color32::TRANSPARENT };
    egui::Frame::NONE
        .fill(fill)
        .inner_margin(egui::Margin::symmetric(2, 1))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                let [r, g, b] = row.color;
                ui.painter()
                    .rect_filled(rect, 2.0, egui::Color32::from_rgb(r, g, b));
                let name = if row.me {
                    egui::RichText::new(row.name()).strong()
                } else {
                    egui::RichText::new(row.name())
                };
                ui.add_sized([170.0, 18.0], egui::Label::new(name).truncate());
                if row.host {
                    ui.label(egui::RichText::new("host").small().italics())
                        .on_hover_text(HOST_HINT);
                }
                ui.add_sized(
                    [62.0, 18.0],
                    egui::Label::new(egui::RichText::new(turn_text(row.turn))),
                );
                // The line: a dot, and the time since a human was lost.
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter()
                    .circle_filled(rect.center(), 5.0, dot_of(&row.line).color());
                if let PlayerLine::Lost(since) = &row.line {
                    ui.colored_label(Dot::Red.color(), format!("no connection {since}"));
                }
                if let Some(s) = row.host_passes_in_s {
                    ui.weak(format!("host role passes in {s} s"));
                }
                diplomacy_buttons(ui, on, row, actions);
                if row.can_kick {
                    let lost = matches!(row.line, PlayerLine::Lost(_));
                    let fill = lost.then_some(DANGER);
                    if button(
                        ui,
                        on,
                        "Kick",
                        fill,
                        "Remove the player; the AI takes the civilization",
                    ) {
                        actions.push(UiAction::Kick { player: row.player });
                    }
                }
            });
        });
}

fn diplomacy_buttons(ui: &mut egui::Ui, on: bool, row: &PlayerRow, actions: &mut Vec<UiAction>) {
    let Some(d) = row.diplomacy else {
        return;
    };
    match d {
        Diplomacy::Peace => {
            if button(
                ui,
                on,
                "Declare war",
                None,
                "Declare war on this civilization",
            ) {
                actions.push(UiAction::DeclareWar { team: row.team });
            }
        }
        Diplomacy::WarHuman => {
            if button(ui, on, "Propose peace", None, "Offer white peace") {
                actions.push(UiAction::ProposePeace { to: row.player });
            }
        }
        Diplomacy::WarOffered => {
            ui.weak("Peace offered");
        }
        Diplomacy::WarIncoming => {
            if button(ui, on, "Accept", None, "Accept the offer of peace") {
                actions.push(UiAction::AnswerPeace {
                    from: row.player,
                    accept: true,
                });
            }
            if button(ui, on, "Decline", None, "Decline the offer of peace") {
                actions.push(UiAction::AnswerPeace {
                    from: row.player,
                    accept: false,
                });
            }
        }
        Diplomacy::WarAi => {
            ui.weak("At war")
                .on_hover_text("AI civilizations do not accept peace yet");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::{click_at, find_widget};

    fn row(player: i8, diplomacy: Option<Diplomacy>) -> PlayerRow {
        PlayerRow {
            player,
            team: player,
            color: [200, 30, 30],
            civ: "Egypt".into(),
            nick: Some("Bob".into()),
            diplomacy,
            ..PlayerRow::default()
        }
    }

    fn view(rows: Vec<PlayerRow>) -> PlayersView {
        PlayersView {
            rows,
            open: true,
            note: None,
            orders_enabled: true,
        }
    }

    fn press(v: &PlayersView, label: &str) -> Vec<UiAction> {
        let at = find_widget(label, |ctx| show(ctx, v))
            .unwrap_or_else(|| panic!("{label}"))
            .center();
        click_at(at, false, |ctx| show(ctx, v))
    }

    #[test]
    fn names() {
        let mut r = row(1, None);
        assert_eq!(r.name(), "Egypt (Bob)");
        r.nick = None;
        assert_eq!(r.name(), "Egypt (AI)");
    }

    #[test]
    fn the_header_folds_and_counts() {
        let mut v = view(vec![row(1, None), row(2, None)]);
        v.open = false;
        assert_eq!(press(&v, "> Players (2)"), [UiAction::TogglePlayers]);
        assert!(find_widget("Egypt (Bob)", |ctx| show(ctx, &v)).is_none());
    }

    #[test]
    fn peace_row_declares_war() {
        let v = view(vec![row(1, Some(Diplomacy::Peace))]);
        assert_eq!(press(&v, "Declare war"), [UiAction::DeclareWar { team: 1 }]);
    }

    #[test]
    fn war_with_a_human_proposes_peace() {
        let v = view(vec![row(2, Some(Diplomacy::WarHuman))]);
        assert_eq!(
            press(&v, "Propose peace"),
            [UiAction::ProposePeace { to: 2 }]
        );
        assert!(find_widget("Declare war", |ctx| show(ctx, &v)).is_none());
    }

    #[test]
    fn outgoing_offer_has_no_buttons() {
        let v = view(vec![row(2, Some(Diplomacy::WarOffered))]);
        assert!(find_widget("Peace offered", |ctx| show(ctx, &v)).is_some());
        assert!(find_widget("Propose peace", |ctx| show(ctx, &v)).is_none());
    }

    #[test]
    fn incoming_offer_accepts_or_declines() {
        let v = view(vec![row(2, Some(Diplomacy::WarIncoming))]);
        assert_eq!(
            press(&v, "Accept"),
            [UiAction::AnswerPeace {
                from: 2,
                accept: true
            }]
        );
        assert_eq!(
            press(&v, "Decline"),
            [UiAction::AnswerPeace {
                from: 2,
                accept: false
            }]
        );
    }

    #[test]
    fn war_with_an_ai_has_no_peace_button() {
        let mut r = row(3, Some(Diplomacy::WarAi));
        r.nick = None;
        r.line = PlayerLine::Ai;
        let v = view(vec![r]);
        assert!(find_widget("At war", |ctx| show(ctx, &v)).is_some());
        for label in ["Propose peace", "Declare war", "Accept"] {
            assert!(find_widget(label, |ctx| show(ctx, &v)).is_none(), "{label}");
        }
    }

    #[test]
    fn kick_is_the_hosts_and_for_others_only() {
        let mut r = row(1, None);
        r.can_kick = true;
        let v = view(vec![r]);
        assert_eq!(press(&v, "Kick"), [UiAction::Kick { player: 1 }]);
        let v = view(vec![row(1, None)]);
        assert!(find_widget("Kick", |ctx| show(ctx, &v)).is_none());
    }

    #[test]
    fn lost_human_shows_the_time_and_the_host_countdown() {
        let mut r = row(1, None);
        r.host = true;
        r.line = PlayerLine::Lost("1:05".into());
        r.host_passes_in_s = Some(42);
        let v = view(vec![r]);
        assert!(find_widget("no connection 1:05", |ctx| show(ctx, &v)).is_some());
        assert!(find_widget("host role passes in 42 s", |ctx| show(ctx, &v)).is_some());
        assert!(find_widget("host", |ctx| show(ctx, &v)).is_some());
    }

    #[test]
    fn the_returned_host_has_no_host_buttons() {
        // The former host is an ordinary player: no label, no Kick (the data says `host = false`).
        let v = view(vec![row(1, Some(Diplomacy::Peace))]);
        assert!(find_widget("host", |ctx| show(ctx, &v)).is_none());
        assert!(find_widget("Kick", |ctx| show(ctx, &v)).is_none());
    }

    #[test]
    fn marks_are_words() {
        let mut r = row(1, None);
        r.turn = TurnMark::Done;
        let mut t = row(2, None);
        t.turn = TurnMark::Thinking;
        let mut w = row(3, None);
        w.turn = TurnMark::Waiting;
        let v = view(vec![r, t, w]);
        for want in ["Done", "Thinking", "Waiting"] {
            assert!(find_widget(want, |ctx| show(ctx, &v)).is_some(), "{want}");
        }
    }

    #[test]
    fn no_connection_greys_every_button() {
        let mut host = row(1, Some(Diplomacy::Peace));
        host.can_kick = true;
        let mut v = view(vec![
            host,
            row(2, Some(Diplomacy::WarHuman)),
            row(3, Some(Diplomacy::WarIncoming)),
        ]);
        v.orders_enabled = false;
        for label in ["Declare war", "Kick", "Propose peace", "Accept", "Decline"] {
            assert!(press(&v, label).is_empty(), "{label}");
        }
        // The header still folds: reading is free without a line.
        v.open = false;
        assert_eq!(press(&v, "> Players (3)"), [UiAction::TogglePlayers]);
    }
}
