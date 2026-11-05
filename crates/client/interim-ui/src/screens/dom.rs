//! The actions the web page's own screens (login, lobby list, room) send back as JSON (`window.__civ5ui`).
//!
//! The page posts one JSON text per click; `client-app` parses it into a [`UiAction`] and feeds it to the
//! normal path. Only the actions of those three screens exist here, so a page (or a console) cannot send a
//! game order this way. Serde's usual shape: a unit variant is its name (`"CreateRoom"`), the others a
//! one-key object (`{"JoinRoom": 3}`, `{"Login": {"nick": "a", "password": ""}}`).

use serde::Deserialize;

use super::UiAction;
use super::room::{RoomEdit, SlotKind};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub enum DomAction {
    /// Sign in with the token the page stored for `nick` (the password is empty then).
    Login {
        nick: String,
        password: String,
    },
    Retry,
    Reload,
    SignOut,
    UseThisTab,
    CreateRoom,
    QuickStart,
    JoinRoom(u32),
    LeaveRoom,
    LoadSave(String),
    GiveUpSeat,
    ConfirmGiveUp(bool),
    TakeSeat {
        game: u32,
        player: i8,
    },
    RoomEdit(RoomEdit),
    SlotSet {
        index: u8,
        kind: SlotKind,
        civ: Option<String>,
    },
    PickCiv(Option<String>),
    TakeSlot(u8),
    Ready(bool),
    Start,
}

impl From<DomAction> for UiAction {
    fn from(a: DomAction) -> Self {
        match a {
            DomAction::Login { nick, password } => UiAction::Login { nick, password },
            DomAction::Retry => UiAction::Retry,
            DomAction::Reload => UiAction::Reload,
            DomAction::SignOut => UiAction::SignOut,
            DomAction::UseThisTab => UiAction::UseThisTab,
            DomAction::CreateRoom => UiAction::CreateRoom,
            DomAction::QuickStart => UiAction::QuickStart,
            DomAction::JoinRoom(id) => UiAction::JoinRoom(id),
            DomAction::LeaveRoom => UiAction::LeaveRoom,
            DomAction::LoadSave(id) => UiAction::LoadSave(id),
            DomAction::GiveUpSeat => UiAction::GiveUpSeat,
            DomAction::ConfirmGiveUp(yes) => UiAction::ConfirmGiveUp(yes),
            DomAction::TakeSeat { game, player } => UiAction::TakeSeat { game, player },
            DomAction::RoomEdit(e) => UiAction::RoomEdit(e),
            DomAction::SlotSet { index, kind, civ } => UiAction::SlotSet { index, kind, civ },
            DomAction::PickCiv(civ) => UiAction::PickCiv(civ),
            DomAction::TakeSlot(i) => UiAction::TakeSlot(i),
            DomAction::Ready(v) => UiAction::Ready(v),
            DomAction::Start => UiAction::Start,
        }
    }
}

/// Reads one action the page posted. `None`: not one of the page's actions (it is dropped).
pub fn parse_action(json: &str) -> Option<UiAction> {
    serde_json::from_str::<DomAction>(json).ok().map(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::room::{TimerChoice, TurnModeChoice};

    #[test]
    fn the_pages_clicks_become_ui_actions() {
        assert_eq!(parse_action(r#""CreateRoom""#), Some(UiAction::CreateRoom));
        assert_eq!(
            parse_action(r#"{"JoinRoom":3}"#),
            Some(UiAction::JoinRoom(3))
        );
        assert_eq!(
            parse_action(r#"{"Login":{"nick":"a","password":""}}"#),
            Some(UiAction::Login {
                nick: "a".into(),
                password: String::new()
            })
        );
        assert_eq!(
            parse_action(r#"{"RoomEdit":{"Victory":{"key":"VICTORY_TIME","on":false}}}"#),
            Some(UiAction::RoomEdit(RoomEdit::Victory {
                key: "VICTORY_TIME".into(),
                on: false
            }))
        );
        assert_eq!(
            parse_action(r#"{"RoomEdit":{"MaxTurns":300}}"#),
            Some(UiAction::RoomEdit(RoomEdit::MaxTurns(300)))
        );
        assert_eq!(
            parse_action(r#"{"RoomEdit":{"MapOption":{"id":"2","value":3}}}"#),
            Some(UiAction::RoomEdit(RoomEdit::MapOption {
                id: "2".into(),
                value: 3
            }))
        );
        assert_eq!(
            parse_action(r#"{"RoomEdit":{"Timer":{"Fixed":90}}}"#),
            Some(UiAction::RoomEdit(RoomEdit::Timer(TimerChoice::Fixed(90))))
        );
        assert_eq!(
            parse_action(r#"{"RoomEdit":{"TurnMode":"Simultaneous"}}"#),
            Some(UiAction::RoomEdit(RoomEdit::TurnMode(
                TurnModeChoice::Simultaneous
            )))
        );
        assert_eq!(
            parse_action(r#"{"SlotSet":{"index":1,"kind":"Ai","civ":null}}"#),
            Some(UiAction::SlotSet {
                index: 1,
                kind: SlotKind::Ai,
                civ: None
            })
        );
    }

    #[test]
    fn a_game_order_cannot_come_this_way() {
        assert_eq!(parse_action(r#"{"EndTurn":{"force":true}}"#), None);
        assert_eq!(parse_action(r#"{"Kick":{"player":1}}"#), None);
        assert_eq!(parse_action("not json"), None);
    }
}
