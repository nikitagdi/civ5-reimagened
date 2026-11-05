//! Journal (play-ui §3.3, AC-7.2, AC-7.3, AC-6.2, AC-6.3), bottom left (above the minimap when
//! there is one, lobby-ui 4.8): the entries of this and
//! past turns with their turn, scrolled to the newest. Auto-choices and auto-declines are marked
//! "(auto)" with their SIMP number; the tooltip says what it means.
//!
//! In a server game (lobby-ui 4.7, LU-D9) it is one window with two tabs, "Journal" and "Chat",
//! 320 wide: the journal has the entries of B1 and the new kinds (`System` grey, `Session` blue,
//! `Diplomacy` orange); the chat is [`super::chat`]. Folded, it is the line of the newest entry.

use super::UiAction;
use super::chat::{self, ChatForm, ChatView};

/// The two tabs of the window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LogTab {
    #[default]
    Journal,
    Chat,
}

/// What kind of line it is, for its colour.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LineKind {
    #[default]
    Normal,
    /// Restart, pause, save, a refused order.
    System,
    /// Seat events: taken over, removed, the new host.
    Session,
    /// Peace offers and their answers.
    Diplomacy,
}

impl LineKind {
    fn color(self) -> Option<egui::Color32> {
        match self {
            LineKind::Normal => None,
            LineKind::System => Some(egui::Color32::from_rgb(150, 150, 150)),
            LineKind::Session => Some(egui::Color32::from_rgb(110, 170, 255)),
            LineKind::Diplomacy => Some(egui::Color32::from_rgb(240, 160, 70)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalLine {
    pub turn: i32,
    pub text: String,
    /// The SIMP number of an auto-choice or auto-decline.
    pub simplification: Option<u16>,
    /// The plot of the event: a click on the line moves the camera there (AC-7.3).
    pub at: Option<(i16, i16)>,
    pub kind: LineKind,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JournalView {
    /// Oldest first.
    pub lines: Vec<JournalLine>,
    pub open: bool,
    pub tab: LogTab,
    /// The chat of a server game; `None` (a local game) leaves the window with the journal only.
    pub chat: Option<ChatView>,
    /// The minimap panel stands under the window (bottom left): the window sits above it.
    pub above_minimap: bool,
}

impl JournalLine {
    /// "T3: Research: Pottery (auto, SIMP-002)".
    pub fn text(&self) -> String {
        match self.simplification {
            Some(n) => format!("T{}: {} (auto, SIMP-{n:03})", self.turn, self.text),
            None => format!("T{}: {}", self.turn, self.text),
        }
    }
}

/// The tooltip of a SIMP-tagged line.
fn simp_hint(n: u16) -> String {
    match n {
        2 => "SIMP-002: the AI made this choice for you (no screen for it yet)".to_owned(),
        3 => "SIMP-003: the AI's offer was declined for you (no diplomacy screen yet)".to_owned(),
        n => format!("SIMP-{n:03}: simplified in this build"),
    }
}

pub fn show(ctx: &egui::Context, view: &JournalView, form: &mut ChatForm) -> Vec<UiAction> {
    let mut actions = Vec::new();
    // The tab shown: the chat only where there is one.
    let tab = match (&view.chat, view.tab) {
        (Some(_), LogTab::Chat) => LogTab::Chat,
        _ => LogTab::Journal,
    };
    // The minimap has the bottom left corner (as in the original); the journal stands on it.
    let lift = if view.above_minimap {
        super::minimap::RESERVED_HEIGHT
    } else {
        0.0
    };
    egui::Area::new(egui::Id::unique("journal"))
        .anchor(egui::Align2::LEFT_BOTTOM, [12.0, -12.0 - lift])
        .show(ctx, |ui| {
            egui::Frame::window(ui.style()).show(ui, |ui| {
                ui.set_width(320.0);
                ui.horizontal(|ui| {
                    let arrow = if view.open { "v" } else { ">" };
                    let r = ui.button(format!("{arrow} Journal ({})", view.lines.len()));
                    if r.clicked() {
                        r.surrender_focus();
                        actions.push(if view.open && tab != LogTab::Journal {
                            UiAction::LogTab(LogTab::Journal)
                        } else {
                            UiAction::ToggleJournal
                        });
                    }
                    if let Some(c) = &view.chat {
                        let on_chat = view.open && tab == LogTab::Chat;
                        let new = c.unread > 0 && !on_chat;
                        let mut text = format!("Chat ({})", c.lines.len());
                        if new {
                            text.push_str(&format!(" ({} new)", c.unread));
                        }
                        let mut b = egui::Button::new(text).selected(on_chat);
                        if new {
                            b = b.fill(egui::Color32::from_rgb(90, 70, 20));
                        }
                        let r = ui.add(b);
                        if r.clicked() {
                            r.surrender_focus();
                            actions.push(if on_chat {
                                UiAction::ToggleJournal
                            } else {
                                UiAction::LogTab(LogTab::Chat)
                            });
                        }
                    }
                });
                if !view.open {
                    // Folded: the newest line of the tab.
                    let last = match (tab, &view.chat) {
                        (LogTab::Chat, Some(c)) => c.lines.last().map(chat::ChatRow::text),
                        _ => view.lines.last().map(JournalLine::text),
                    };
                    if let Some(last) = last {
                        ui.add(egui::Label::new(last).truncate());
                    }
                    return;
                }
                match (tab, &view.chat) {
                    (LogTab::Chat, Some(c)) => actions.extend(chat::show_tab(ui, c, form)),
                    _ => journal_tab(ui, view, &mut actions),
                }
            });
        });
    actions
}

fn journal_tab(ui: &mut egui::Ui, view: &JournalView, actions: &mut Vec<UiAction>) {
    egui::ScrollArea::vertical()
        .id_salt("journal-lines")
        .max_height(140.0)
        .stick_to_bottom(true)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            if view.lines.is_empty() {
                ui.weak("Nothing yet.");
            }
            for line in &view.lines {
                let mut text = egui::RichText::new(line.text());
                if let Some(c) = line.kind.color() {
                    text = text.color(c);
                }
                let label = egui::Label::new(text).wrap();
                // A line with a place can be clicked.
                let mut r = match line.at {
                    Some(_) => ui
                        .add(label.sense(egui::Sense::click()))
                        .on_hover_cursor(egui::CursorIcon::PointingHand),
                    None => ui.add(label),
                };
                if let Some(n) = line.simplification {
                    r = r.on_hover_text(simp_hint(n));
                }
                if let Some((x, y)) = line.at
                    && r.clicked()
                {
                    actions.push(UiAction::JournalGoto { x, y });
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::{click_at, find_widget};

    fn view(open: bool) -> JournalView {
        JournalView {
            lines: vec![
                JournalLine {
                    turn: 1,
                    text: "Muscat is ready for a new construction project.".into(),
                    simplification: None,
                    at: Some((7, 12)),
                    kind: LineKind::Normal,
                },
                JournalLine {
                    turn: 2,
                    text: "Research: Pottery".into(),
                    simplification: Some(2),
                    at: None,
                    kind: LineKind::Normal,
                },
            ],
            open,
            ..JournalView::default()
        }
    }

    #[test]
    fn lines_carry_turn_and_simp() {
        let v = view(true);
        assert_eq!(v.lines[1].text(), "T2: Research: Pottery (auto, SIMP-002)");
        assert!(
            find_widget("T2: Research: Pottery (auto, SIMP-002)", |ctx| show(
                ctx,
                &v,
                &mut ChatForm::default()
            ))
            .is_some()
        );
        assert!(
            find_widget(
                "T1: Muscat is ready for a new construction project.",
                |ctx| { show(ctx, &v, &mut ChatForm::default()) }
            )
            .is_some()
        );
    }

    #[test]
    fn a_click_on_a_line_with_a_place_goes_there() {
        let v = view(true);
        let at = find_widget(
            "T1: Muscat is ready for a new construction project.",
            |ctx| show(ctx, &v, &mut ChatForm::default()),
        )
        .expect("the line");
        assert_eq!(
            click_at(at.center(), false, |ctx| show(
                ctx,
                &v,
                &mut ChatForm::default()
            )),
            [UiAction::JournalGoto { x: 7, y: 12 }]
        );
        // A line without a place (and a SIMP tooltip) does nothing.
        let at = find_widget("T2: Research: Pottery (auto, SIMP-002)", |ctx| {
            show(ctx, &v, &mut ChatForm::default())
        })
        .expect("the line");
        assert!(
            click_at(at.center(), false, |ctx| show(
                ctx,
                &v,
                &mut ChatForm::default()
            ))
            .is_empty()
        );
    }

    #[test]
    fn the_header_toggles() {
        let v = view(false);
        let at = find_widget("> Journal (2)", |ctx| {
            show(ctx, &v, &mut ChatForm::default())
        })
        .expect("the header");
        assert_eq!(
            click_at(at.center(), false, |ctx| show(
                ctx,
                &v,
                &mut ChatForm::default()
            )),
            [UiAction::ToggleJournal]
        );
    }

    fn with_chat(open: bool, tab: LogTab, unread: usize) -> JournalView {
        JournalView {
            open,
            tab,
            chat: Some(ChatView {
                lines: vec![chat::ChatRow {
                    age_ms: 0,
                    from: "Bob".into(),
                    private_to: None,
                    text: "hi".into(),
                }],
                unread,
                orders_enabled: true,
                ..ChatView::default()
            }),
            ..view(open)
        }
    }

    fn find(v: &JournalView, text: &str) -> Option<egui::Rect> {
        find_widget(text, |ctx| show(ctx, v, &mut ChatForm::default()))
    }

    #[test]
    fn the_chat_tab_selects_and_shows_unread() {
        let v = with_chat(true, LogTab::Journal, 2);
        let at = find(&v, "Chat (1) (2 new)").expect("the chat tab says how many are new");
        assert_eq!(
            click_at(at.center(), false, |ctx| show(
                ctx,
                &v,
                &mut ChatForm::default()
            )),
            [UiAction::LogTab(LogTab::Chat)]
        );
        // Open on the chat: no "new" on its header, the lines are shown.
        let v = with_chat(true, LogTab::Chat, 2);
        assert!(find(&v, "Chat (1)").is_some());
        assert!(find(&v, "<1 min ago Bob: hi").is_some());
    }

    #[test]
    fn folded_shows_the_newest_line_of_the_tab() {
        let v = with_chat(false, LogTab::Chat, 0);
        assert!(find(&v, "<1 min ago Bob: hi").is_some());
    }

    #[test]
    fn no_chat_tab_in_a_local_game() {
        assert!(find(&view(true), "Chat (0)").is_none());
    }

    #[test]
    fn kinds_have_colours() {
        assert!(LineKind::Normal.color().is_none());
        let colours: Vec<_> = [LineKind::System, LineKind::Session, LineKind::Diplomacy]
            .iter()
            .map(|k| k.color().expect("coloured"))
            .collect();
        assert_ne!(colours[0], colours[1]);
        assert_ne!(colours[1], colours[2]);
    }
}
