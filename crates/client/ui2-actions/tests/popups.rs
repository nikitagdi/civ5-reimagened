mod common;
use common::{Rig, n, s};
use protocol::{Command, CommandOutcome, ScreenOpen};
use ui2_actions::ActionEvent;
use ui2_model::generated::HudPopup;

fn head(id: i32, decision: bool) -> HudPopup {
    HudPopup {
        id,
        screen: "techs".into(),
        a: Some(4),
        b: Some(2),
        decision,
    }
}

fn sync(r: &mut Rig, head: Option<&HudPopup>) {
    r.actions.sync_popup(head, &mut r.store, &mut r.sink, r.now);
}

#[test]
fn informational_popup_opens_closes_dismisses_and_reconnects() {
    let mut r = Rig::new();
    let first = head(7, false);
    sync(&mut r, Some(&first));
    assert!(r.ui().windows.techs.open);
    assert_eq!(r.ui().windows.techs.popup_id, Some(7));
    assert_eq!(
        (r.ui().windows.techs.a, r.ui().windows.techs.b),
        (Some(4), Some(2))
    );
    r.one("window.close", &[("window", s("techs"))]);
    sync(&mut r, Some(&first));
    assert_eq!(
        r.sink.commands(),
        [Command::ScreenAction {
            screen: "hud".into(),
            action: "popup_done".into(),
            args: vec![("a".into(), 7)],
            text: None
        }]
    );
    sync(&mut r, Some(&first));
    assert_eq!(r.sink.commands().len(), 1);
    r.actions.on_event(
        &ActionEvent::Answered {
            id: 1,
            outcome: CommandOutcome::Applied,
        },
        r.now,
    );
    sync(&mut r, Some(&first));
    assert!(!r.ui().windows.techs.open);
    sync(&mut r, None);
    assert_eq!(r.ui().windows.techs.popup_id, None);
    // Reconnect to an entry that the server still owns.
    sync(&mut r, Some(&first));
    assert!(r.ui().windows.techs.open);
}

#[test]
fn decisions_require_successful_screen_completion_and_results_open_next_window() {
    let mut r = Rig::new();
    let first = head(8, true);
    sync(&mut r, Some(&first));
    r.one("window.close", &[("window", s("techs"))]);
    r.one("window.toggle", &[("window", s("techs"))]);
    assert!(r.ui().windows.techs.open);
    sync(&mut r, Some(&first));
    assert!(r.sink.commands().is_empty());
    r.one(
        "screen.act",
        &[
            ("screen", s("techs")),
            ("action", s("research")),
            ("a", n(4)),
        ],
    );
    assert!(
        matches!(&r.sink.commands()[0], Command::ScreenAction { args, .. } if args.contains(&("popup_id".into(), 8)))
    );
    let outcome = CommandOutcome::ScreenApplied {
        screen: "techs".into(),
        open: Some(ScreenOpen {
            screen: "techs".into(),
            a: Some(9),
            b: Some(1),
        }),
        popup_done: Some(8),
        close: true,
    };
    r.actions.on_event(
        &ActionEvent::Answered {
            id: 1,
            outcome: outcome.clone(),
        },
        r.now,
    );
    sync(&mut r, Some(&first));
    assert!(r.ui().windows.techs.open);
    assert_eq!(r.ui().windows.techs.a, Some(9));
    assert_eq!(r.ui().windows.techs.popup_id, None);
    sync(&mut r, None);
    assert!(
        r.ui().windows.techs.open,
        "new action-opened window survives removal of old popup"
    );
    r.one("window.close", &[("window", s("techs"))]);
    r.actions
        .on_event(&ActionEvent::Answered { id: 1, outcome }, r.now);
    r.actions.publish(&mut r.store);
    assert!(
        !r.ui().windows.techs.open,
        "duplicate results cannot reopen the window"
    );
}

#[test]
fn rejected_dismissal_reopens_and_can_be_retried() {
    let mut r = Rig::new();
    let first = head(2, false);
    sync(&mut r, Some(&first));
    r.one("window.close", &[("window", s("techs"))]);
    sync(&mut r, Some(&first));
    r.actions.on_event(
        &ActionEvent::Answered {
            id: 1,
            outcome: CommandOutcome::Rejected {
                code: protocol::RejectCode::TurnChanged,
                reason: String::new(),
            },
        },
        r.now,
    );
    sync(&mut r, Some(&first));
    assert!(r.ui().windows.techs.open);
    r.one("window.close", &[("window", s("techs"))]);
    sync(&mut r, Some(&first));
    assert_eq!(r.sink.commands().len(), 2);
}

#[test]
fn changing_game_clears_popup_windows() {
    let mut r = Rig::new();
    sync(&mut r, Some(&head(1, true)));
    r.actions.on_event(&ActionEvent::NewGame, r.now);
    r.actions.publish(&mut r.store);
    assert!(!r.ui().windows.techs.open);
    assert_eq!(r.ui().windows.techs.popup_id, None);
}
