//! The page's own screens (web): sign-in, lobby list and room are DOM, not egui.
//!
//! SIMPLIFIED(SIMP-uic-60): a DOM page instead of the original FrontEnd Lua screens.
//!
//! Each frame the views of those three screens are taken out of [`Screens`] (egui does not draw them), turned
//! into JSON and handed to the page through the platform's [`UiBridge`] when they changed. The actions the page
//! posted come back as JSON and go through the normal [`UiAction`] path (only the actions of these screens
//! parse, see `interim_ui::screens::dom`). `?egui=1` leaves the bridge out and the egui screens stay.

use interim_ui::screens::dom::parse_action;
use interim_ui::screens::login::LoginView;
use interim_ui::screens::room::RoomView;
use interim_ui::screens::rooms::LobbyListView;
use interim_ui::screens::{Screens, UiAction};
use platform::UiBridge;

/// What [`DomUi::present`] took out of the screens; [`Hidden::restore`] puts it back (the test port reads the
/// screens after the frame).
#[derive(Default)]
pub struct Hidden {
    login: Option<LoginView>,
    lobby_list: Option<LobbyListView>,
    room: Option<RoomView>,
}

impl Hidden {
    pub fn restore(self, views: &mut Screens) {
        views.login = self.login;
        views.lobby_list = self.lobby_list;
        views.room = self.room;
    }
}

pub struct DomUi {
    bridge: Box<dyn UiBridge>,
    /// The screen and the JSON the page was told last.
    shown: Option<(&'static str, String)>,
}

fn json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|e| {
        tracing::error!("a screen of the page cannot be written: {e}");
        String::new()
    })
}

impl DomUi {
    pub fn new(bridge: Box<dyn UiBridge>) -> Self {
        Self {
            bridge,
            shown: None,
        }
    }

    /// Takes the page's screens out of `views` and tells the page when what it shows changed. With none of
    /// them up the page is told `"none"`: it hides itself and the canvas has the screen.
    pub fn present(&mut self, views: &mut Screens) -> Hidden {
        let hidden = Hidden {
            login: views.login.take(),
            lobby_list: views.lobby_list.take(),
            room: views.room.take(),
        };
        let (screen, text) = if let Some(v) = &hidden.login {
            ("login", json(v))
        } else if let Some(v) = &hidden.lobby_list {
            ("lobby", json(v))
        } else if let Some(v) = &hidden.room {
            ("room", json(v))
        } else {
            ("none", String::new())
        };
        if self
            .shown
            .as_ref()
            .is_none_or(|(s, t)| *s != screen || *t != text)
        {
            self.bridge.render(screen, &text);
            self.shown = Some((screen, text));
        }
        hidden
    }

    /// Hands the page the game art of its screens (`lobby_art`), once: the screen `"art"`, which only stores it
    /// and redraws (it does not change what the page shows).
    pub fn send_art(&mut self, json: &str) {
        self.bridge.render("art", json);
    }

    /// What the page posted since the last frame; text that is not one of the page's actions is dropped.
    pub fn actions(&self) -> Vec<UiAction> {
        self.bridge
            .take_actions()
            .iter()
            .filter_map(|a| {
                let parsed = parse_action(a);
                if parsed.is_none() {
                    tracing::warn!("the page posted an action that is not one of its own");
                }
                parsed
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Default)]
    struct Fake {
        rendered: RefCell<Vec<(String, String)>>,
        posted: RefCell<Vec<String>>,
    }

    struct Handle(Rc<Fake>);

    impl UiBridge for Handle {
        fn render(&self, screen: &str, json: &str) {
            self.0
                .rendered
                .borrow_mut()
                .push((screen.to_owned(), json.to_owned()));
        }

        fn take_actions(&self) -> Vec<String> {
            std::mem::take(&mut *self.0.posted.borrow_mut())
        }
    }

    fn dom() -> (DomUi, Rc<Fake>) {
        let fake = Rc::new(Fake::default());
        (DomUi::new(Box::new(Handle(Rc::clone(&fake)))), fake)
    }

    #[test]
    fn the_page_is_told_only_when_its_screen_changes() {
        let (mut ui, fake) = dom();
        let mut views = Screens {
            lobby_list: Some(LobbyListView {
                nick: "alice".into(),
                ..LobbyListView::default()
            }),
            ..Screens::default()
        };
        let hidden = ui.present(&mut views);
        assert!(views.lobby_list.is_none(), "egui does not draw the lobby");
        hidden.restore(&mut views);
        assert!(views.lobby_list.is_some(), "the test port still sees it");
        // The same lobby again: nothing is sent.
        ui.present(&mut views).restore(&mut views);
        assert_eq!(fake.rendered.borrow().len(), 1);
        assert_eq!(fake.rendered.borrow()[0].0, "lobby");
        assert!(fake.rendered.borrow()[0].1.contains("\"nick\":\"alice\""));
        // The room replaces it; then nothing for the canvas.
        views = Screens {
            room: Some(RoomView::default()),
            ..Screens::default()
        };
        ui.present(&mut views);
        views = Screens::default();
        ui.present(&mut views);
        let seen: Vec<String> = fake.rendered.borrow().iter().map(|r| r.0.clone()).collect();
        assert_eq!(seen, ["lobby", "room", "none"]);
    }

    #[test]
    fn the_login_screen_carries_the_error_text() {
        let (mut ui, fake) = dom();
        let mut views = Screens {
            login: Some(LoginView {
                nick: "bob".into(),
                error: Some("Wrong nick or password.".into()),
                ..LoginView::default()
            }),
            ..Screens::default()
        };
        ui.present(&mut views);
        let rendered = fake.rendered.borrow();
        assert_eq!(rendered[0].0, "login");
        assert!(rendered[0].1.contains("Wrong nick or password."));
    }

    #[test]
    fn posted_actions_come_back_and_foreign_ones_are_dropped() {
        let (ui, fake) = dom();
        fake.posted.borrow_mut().extend([
            r#"{"Login":{"nick":"a","password":""}}"#.to_owned(),
            r#"{"EndTurn":{"force":false}}"#.to_owned(),
            r#""Start""#.to_owned(),
        ]);
        assert_eq!(
            ui.actions(),
            [
                UiAction::Login {
                    nick: "a".into(),
                    password: String::new()
                },
                UiAction::Start
            ]
        );
        assert!(ui.actions().is_empty());
    }
}
