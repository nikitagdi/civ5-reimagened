mod common;
use common::{Rig, n, s};
use protocol::Command;

#[test]
fn remote_window_arguments_and_actions_use_the_normal_order_path() {
    let mut r = Rig::new();
    r.one(
        "window.open",
        &[("window", s("techs")), ("a", n(7)), ("b", n(8))],
    );
    let w = &r.actions.ui().windows.techs;
    assert!(w.open);
    assert_eq!((w.a, w.b), (Some(7), Some(8)));
    r.one(
        "screen.act",
        &[
            ("screen", s("techs")),
            ("action", s("research")),
            ("a", n(12)),
            ("b", n(1)),
        ],
    );
    assert_eq!(
        r.sink.commands(),
        vec![Command::ScreenAction {
            screen: "techs".into(),
            action: "research".into(),
            args: vec![("a".into(), 12), ("b".into(), 1)]
        }]
    );
    assert_eq!(r.actions.ui().pending.len(), 1);
    r.one("window.close", &[("window", s("techs"))]);
    r.one(
        "screen.act",
        &[
            ("screen", s("techs")),
            ("action", s("research")),
            ("a", n(13)),
        ],
    );
    assert_eq!(r.sink.commands().len(), 2);
    r.one(
        "screen.act",
        &[
            ("screen", s("missing")),
            ("action", s("research")),
            ("a", n(14)),
        ],
    );
    assert_eq!(r.sink.commands().len(), 2);
}
