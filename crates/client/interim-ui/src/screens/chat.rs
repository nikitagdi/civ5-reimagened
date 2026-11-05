//! The chat tab of a server game (lobby-ui 4.7 "Chat", LU-D9, LU-D16, LU-D17): the lines of the game's
//! stream, a one-line input at the bottom and the list of recipients. It is the body of the second tab of
//! the journal window ([`super::journal`]); there is no chat in the room before the start (LU-D16).
//!
//! Lines read "m ago nick: text", the age counted from the moment the client received the line, by the
//! client's clock (LU-D17); a private one reads "(private) nick -> you".

use super::UiAction;

/// The longest line the input takes (the core cuts at the same number).
pub const MAX_CHARS: usize = 500;

/// One line of the chat, ready to show.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatRow {
    /// Milliseconds since the client received the line.
    pub age_ms: u64,
    pub from: String,
    /// A private line: who it is for ("you" or a nick).
    pub private_to: Option<String>,
    pub text: String,
}

/// "<1 min ago", "5 min ago", "2 h ago".
pub fn age_text(age_ms: u64) -> String {
    let min = age_ms / 60_000;
    match min {
        0 => "<1 min ago".to_owned(),
        1..=119 => format!("{min} min ago"),
        _ => format!("{} h ago", min / 60),
    }
}

impl ChatRow {
    /// "5 min ago Bob: hello", "<1 min ago (private) Bob -> you: psst".
    pub fn text(&self) -> String {
        let who = match &self.private_to {
            Some(to) => format!("(private) {} -> {to}", self.from),
            None => self.from.clone(),
        };
        format!("{} {who}: {}", age_text(self.age_ms), self.text)
    }
}

/// A recipient besides "All": another human of the game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatTarget {
    pub player: i8,
    pub nick: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatView {
    /// Oldest first.
    pub lines: Vec<ChatRow>,
    pub targets: Vec<ChatTarget>,
    /// Lines of others since the tab was last open (the header says "(n new)").
    pub unread: usize,
    /// The line is up (LU-D19): the input is grey.
    pub orders_enabled: bool,
}

/// What the player typed, kept between frames.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatForm {
    pub text: String,
    /// The recipient: `None` is "All".
    pub to: Option<i8>,
    /// `T` was pressed: the input takes the focus in the next frame.
    pub focus: bool,
}

/// The stable id of the input, so the focus survives the frames (and `T` can ask for it).
pub fn input_id() -> egui::Id {
    egui::Id::unique("chat-input")
}

/// The recipient's name in the list.
fn target_name(view: &ChatView, to: Option<i8>) -> String {
    to.and_then(|p| view.targets.iter().find(|t| t.player == p))
        .map_or_else(|| "All".to_owned(), |t| t.nick.clone())
}

/// The body of the tab: the lines, then the recipient and the input.
pub fn show_tab(ui: &mut egui::Ui, view: &ChatView, form: &mut ChatForm) -> Vec<UiAction> {
    let mut actions = Vec::new();
    egui::ScrollArea::vertical()
        .id_salt("chat-lines")
        .max_height(110.0)
        .stick_to_bottom(true)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            if view.lines.is_empty() {
                ui.weak("Nothing yet. Press T to write.");
            }
            for line in &view.lines {
                let label = egui::Label::new(line.text()).wrap();
                if line.private_to.is_some() {
                    ui.add(label).on_hover_text("Private line");
                } else {
                    ui.add(label);
                }
            }
        });
    // A recipient who has left the game goes back to "All".
    if form
        .to
        .is_some_and(|p| view.targets.iter().all(|t| t.player != p))
    {
        form.to = None;
    }
    ui.add_enabled_ui(view.orders_enabled, |ui| {
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("chat-to")
                .width(70.0)
                .selected_text(target_name(view, form.to))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut form.to, None, "All");
                    for t in &view.targets {
                        ui.selectable_value(&mut form.to, Some(t.player), &t.nick);
                    }
                });
            let edit = egui::TextEdit::singleline(&mut form.text)
                .id(input_id())
                .char_limit(MAX_CHARS)
                .hint_text("Say something")
                .desired_width(f32::INFINITY);
            let r = ui.add(edit);
            if form.focus {
                form.focus = false;
                r.request_focus();
            }
            if r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                r.surrender_focus();
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                let text = form.text.trim().to_owned();
                if !text.is_empty() {
                    actions.push(UiAction::Chat { to: form.to, text });
                    form.text.clear();
                }
                // The next line follows without a click.
                r.request_focus();
            }
        });
    })
    .response
    .on_disabled_hover_text("No connection");
    actions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::run_headless;

    fn row(age_ms: u64, from: &str, private_to: Option<&str>, text: &str) -> ChatRow {
        ChatRow {
            age_ms,
            from: from.into(),
            private_to: private_to.map(str::to_owned),
            text: text.into(),
        }
    }

    #[test]
    fn ages() {
        assert_eq!(age_text(0), "<1 min ago");
        assert_eq!(age_text(59_999), "<1 min ago");
        assert_eq!(age_text(5 * 60_000), "5 min ago");
        assert_eq!(age_text(3 * 3_600_000), "3 h ago");
    }

    #[test]
    fn line_texts() {
        assert_eq!(
            row(5 * 60_000, "Bob", None, "hello").text(),
            "5 min ago Bob: hello"
        );
        assert_eq!(
            row(0, "Bob", Some("you"), "psst").text(),
            "<1 min ago (private) Bob -> you: psst"
        );
    }

    fn texts(view: &ChatView, form: &mut ChatForm) -> Vec<String> {
        run_headless(|ctx| {
            egui::Area::new(egui::Id::unique("chat-test")).show(ctx, |ui| {
                show_tab(ui, view, form);
            });
        })
        .iter()
        .filter_map(|s| match &s.shape {
            egui::Shape::Text(t) => Some(t.galley.job.text.clone()),
            _ => None,
        })
        .collect()
    }

    #[test]
    fn shows_lines_and_the_recipient() {
        let view = ChatView {
            lines: vec![row(0, "Bob", None, "hi")],
            targets: vec![ChatTarget {
                player: 2,
                nick: "Bob".into(),
            }],
            unread: 0,
            orders_enabled: true,
        };
        let mut form = ChatForm::default();
        let t = texts(&view, &mut form);
        assert!(t.iter().any(|s| s == "<1 min ago Bob: hi"), "{t:?}");
        assert!(t.iter().any(|s| s == "All"), "{t:?}");
        form.to = Some(2);
        let t = texts(&view, &mut form);
        assert!(t.iter().any(|s| s == "Bob"), "{t:?}");
    }

    #[test]
    fn a_recipient_who_left_goes_back_to_all() {
        let view = ChatView {
            orders_enabled: true,
            ..ChatView::default()
        };
        let mut form = ChatForm {
            to: Some(5),
            ..ChatForm::default()
        };
        texts(&view, &mut form);
        assert_eq!(form.to, None);
    }

    /// Types `text` into the focused input and presses Enter: the line the screen sends.
    fn send(view: &ChatView, form: &mut ChatForm, text: &str) -> Vec<UiAction> {
        let ctx = egui::Context::default();
        let raw = |events: Vec<egui::Event>| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 720.0),
            )),
            events,
            ..egui::RawInput::default()
        };
        let enter = |pressed| egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let mut all = Vec::new();
        form.focus = true;
        for events in [
            vec![],
            vec![],
            vec![egui::Event::Text(text.to_owned())],
            vec![enter(true), enter(false)],
            vec![],
        ] {
            let out = ctx.run_pass(raw(events), |ctx| {
                egui::Area::new(egui::Id::unique("chat-test")).show(ctx, |ui| {
                    all.extend(show_tab(ui, view, form));
                });
            });
            out.drop_without_applying_deltas();
        }
        all
    }

    #[test]
    fn enter_sends_the_line_and_clears_the_input() {
        let view = ChatView {
            orders_enabled: true,
            ..ChatView::default()
        };
        let mut form = ChatForm::default();
        let actions = send(&view, &mut form, "  hello  ");
        assert_eq!(
            actions,
            [UiAction::Chat {
                to: None,
                text: "hello".into()
            }]
        );
        assert!(form.text.is_empty());
    }

    #[test]
    fn an_empty_line_is_not_sent() {
        let view = ChatView {
            orders_enabled: true,
            ..ChatView::default()
        };
        let mut form = ChatForm::default();
        assert!(send(&view, &mut form, "   ").is_empty());
    }

    #[test]
    fn no_connection_sends_nothing() {
        let view = ChatView {
            orders_enabled: false,
            ..ChatView::default()
        };
        let mut form = ChatForm::default();
        assert!(send(&view, &mut form, "hello").is_empty());
    }
}
