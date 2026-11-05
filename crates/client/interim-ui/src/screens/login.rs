//! Sign-in (lobby-ui 4.2, AC-N4.1): the server address, "Nick", "Password", [Connect]; the text of the
//! last refusal under them.
//!
//! The password lives only in the form, and the form drops it when the player sends it.

use serde::{Deserialize, Serialize};

use super::UiAction;

/// The button that goes with a refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorButton {
    Reload,
    Retry,
}

impl ErrorButton {
    pub fn label(self) -> &'static str {
        match self {
            ErrorButton::Reload => "Reload",
            ErrorButton::Retry => "Retry",
        }
    }

    pub fn action(self) -> UiAction {
        match self {
            ErrorButton::Reload => UiAction::Reload,
            ErrorButton::Retry => UiAction::Retry,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LoginView {
    /// The nick the form starts with (`civ5.nick` or `?nick=`).
    pub nick: String,
    /// The server address, read only.
    pub server: String,
    /// "Wrong nick or password.", ...
    pub error: Option<String>,
    pub button: Option<ErrorButton>,
}

/// The text fields, kept between frames.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoginForm {
    pub nick: String,
    pub password: String,
    /// The nick was taken from the view once; the player's own edits stay after that.
    filled: bool,
}

/// Longest nick (the server checks the same).
pub const MAX_NICK_CHARS: usize = 24;

/// The nick as it goes out: trimmed, 1 to 24 characters, no control characters.
pub fn clean_nick(raw: &str) -> Option<String> {
    let nick = raw.trim();
    let n = nick.chars().count();
    let ok = (1..=MAX_NICK_CHARS).contains(&n) && !nick.chars().any(char::is_control);
    ok.then(|| nick.to_owned())
}

impl LoginForm {
    /// The action of the Connect button, `None` while the nick is not valid. The password is dropped from the
    /// form.
    pub fn submit(&mut self) -> Option<UiAction> {
        let nick = clean_nick(&self.nick)?;
        let password = std::mem::take(&mut self.password);
        Some(UiAction::Login { nick, password })
    }
}

pub fn show(ctx: &egui::Context, view: &LoginView, form: &mut LoginForm) -> Vec<UiAction> {
    let mut actions = Vec::new();
    if !form.filled {
        form.filled = true;
        form.nick.clone_from(&view.nick);
    }
    egui::Window::new("Sign in")
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .collapsible(false)
        .resizable(false)
        .movable(false)
        .default_width(360.0)
        .show(ctx, |ui| {
            egui::Grid::new("login")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Server");
                    ui.weak(&view.server);
                    ui.end_row();

                    ui.label("Nick");
                    let nick = ui.add(
                        egui::TextEdit::singleline(&mut form.nick)
                            .id_salt("login-nick")
                            .char_limit(MAX_NICK_CHARS * 2)
                            .desired_width(220.0),
                    );
                    ui.end_row();

                    ui.label("Password");
                    let password = ui.add(
                        egui::TextEdit::singleline(&mut form.password)
                            .id_salt("login-password")
                            .password(true)
                            .desired_width(220.0),
                    );
                    ui.end_row();

                    // Enter in either field connects.
                    let enter = (nick.lost_focus() || password.lost_focus())
                        && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    ui.label("");
                    let valid = clean_nick(&form.nick).is_some();
                    let connect = ui.add_enabled(valid, egui::Button::new("Connect"));
                    if (connect.clicked() || (enter && valid))
                        && let Some(a) = form.submit()
                    {
                        actions.push(a);
                    }
                    ui.end_row();
                });
            if clean_nick(&form.nick).is_none() && !form.nick.is_empty() {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "Use 1-24 characters, no control characters.",
                );
            }
            if let Some(e) = &view.error {
                ui.add_space(4.0);
                ui.colored_label(ui.visuals().error_fg_color, e);
                if let Some(b) = view.button
                    && ui.button(b.label()).clicked()
                {
                    actions.push(b.action());
                }
            }
        });
    actions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::{click_at, find_widget, run_headless};

    fn view() -> LoginView {
        LoginView {
            nick: "alice".into(),
            server: "ws://127.0.0.1:8080/ws".into(),
            error: None,
            button: None,
        }
    }

    #[test]
    fn the_nick_is_checked_like_the_server_does() {
        assert_eq!(clean_nick("  bob "), Some("bob".into()));
        assert_eq!(clean_nick(""), None);
        assert_eq!(clean_nick("   "), None);
        assert_eq!(clean_nick("a\tb"), None);
        assert_eq!(clean_nick(&"x".repeat(24)), Some("x".repeat(24)));
        assert_eq!(clean_nick(&"x".repeat(25)), None);
    }

    #[test]
    fn submit_sends_the_login_and_forgets_the_password() {
        let mut f = LoginForm {
            nick: " alice ".into(),
            password: "secret".into(),
            filled: true,
        };
        assert_eq!(
            f.submit(),
            Some(UiAction::Login {
                nick: "alice".into(),
                password: "secret".into()
            })
        );
        assert!(f.password.is_empty());
        f.nick.clear();
        assert_eq!(f.submit(), None);
    }

    #[test]
    fn renders_with_the_nick_filled_and_the_connect_button_clicks() {
        let v = view();
        let out = run_headless(|ctx| {
            show(ctx, &v, &mut LoginForm::default());
        });
        assert!(!out.is_empty());
        let at = find_widget("Connect", |ctx| show(ctx, &v, &mut LoginForm::default()))
            .expect("a Connect button");
        let actions = click_at(at.center(), false, |ctx| {
            show(ctx, &v, &mut LoginForm::default())
        });
        assert_eq!(
            actions,
            [UiAction::Login {
                nick: "alice".into(),
                password: String::new()
            }]
        );
    }

    #[test]
    fn a_refusal_shows_its_text_and_button() {
        let mut v = view();
        v.error = Some("The server was updated. Reload the page.".into());
        v.button = Some(ErrorButton::Reload);
        assert!(
            find_widget("The server was updated. Reload the page.", |ctx| {
                show(ctx, &v, &mut LoginForm::default())
            })
            .is_some()
        );
        let at = find_widget("Reload", |ctx| show(ctx, &v, &mut LoginForm::default())).unwrap();
        assert_eq!(
            click_at(at.center(), false, |ctx| {
                show(ctx, &v, &mut LoginForm::default())
            }),
            [UiAction::Reload]
        );
    }
}
