//! The client half of `hello` on the headless host (`MockClientCtx`), built from the real `mod.toml`.

use hello_client::{
    FIELD_COUNT, FIELD_LAST_FROM, FIELD_LAST_TEXT, HOTKEY_CODE, HOTKEY_SAY, HelloClient,
    INTENT_SAY, KV_LAST, KV_SAID, register,
};
use hello_shared::{CH_ECHO, MOD_ID, Msg};
use mod_api::{Cap, Caps, MOD_API_VERSION, Version};
use mod_client::testing::{MockClientCtx, MockUiModel};
use mod_client::{
    AttachInfo, ClientMod, ClientRegistry, ModDispatcher, ModInfo, UiDrawList, UiValue,
};
use mod_manifest::{Manifest, ModId, Part, UiFallback, UiMode};

const MOD_TOML: &str = include_str!("../../mod.toml");
const MODEL_TOML: &str = include_str!("../../ui/model.toml");
const ECHO_HTML: &str = include_str!("../../ui/echo.html");

fn manifest() -> Manifest {
    Manifest::parse(MOD_TOML).unwrap_or_else(|e| panic!("mod.toml: {e}"))
}

/// The dispatcher of the game with hello at load index 3, capabilities from `mod.toml`.
fn game(caps: Caps) -> ModDispatcher {
    let mut reg = ClientRegistry::new();
    register(&mut reg);
    assert!(reg.problems().is_empty());
    let (d, missing) = reg.instantiate(&[ModInfo {
        mod_idx: 3,
        id: ModId::new(MOD_ID).unwrap(),
        version: Version::new(0, 1, 0),
        caps,
    }]);
    assert!(missing.is_empty());
    d
}

fn manifest_caps() -> Caps {
    Caps::from_manifest(&manifest()).unwrap()
}

fn echo(from: i8, count: u32, text: &str) -> Vec<u8> {
    Msg::Echo {
        from,
        count,
        text: text.into(),
    }
    .encode()
}

fn host() -> MockClientCtx {
    let mut h = MockClientCtx::new();
    h.world.add_player(0, "Ada").add_player(1, "Bob");
    h
}

#[test]
fn mod_toml_declares_the_client_half() {
    let m = manifest();
    assert!(m.has_part(Part::ServerRs) && m.has_part(Part::ClientRs));
    let client = m.rust.client.as_ref().expect("[rust].client");
    assert_eq!(client.crate_name, "hello-client");
    assert_eq!(client.entry, "hello_client::register");
    let caps = manifest_caps();
    for c in [
        Cap::NetChannel,
        Cap::WorldRead,
        Cap::InputClaim,
        Cap::Hud,
        Cap::Kv,
        Cap::RenderPipeline,
        Cap::Ui,
    ] {
        assert!(caps.has(c), "{c}");
    }
    assert!(m.api.matches(&MOD_API_VERSION));
}

#[test]
fn registers_with_one_function_and_no_problems() {
    let mut reg = ClientRegistry::new();
    register(&mut reg);
    assert!(reg.problems().is_empty());
    assert_eq!(
        reg.ids().map(ToString::to_string).collect::<Vec<_>>(),
        ["hello"]
    );
    register(&mut reg);
    assert_eq!(reg.problems().len(), 1, "a second registration is reported");
}

#[test]
fn the_hotkey_sends_hello_and_the_echo_shows_on_the_hud() {
    let mut d = game(manifest_caps());
    let mut h = host();
    d.attach(&mut h);
    // Alt+H is registered (a bare H is the game's).
    assert_eq!(
        h.hotkeys.lookup(HOTKEY_CODE, false, false, true),
        Some((3, HOTKEY_SAY))
    );

    // The press says hello on channel 0, through the mod's index.
    assert!(d.on_hotkey(&mut h, 3, HOTKEY_SAY));
    let sent = h.sent_on(3, CH_ECHO);
    assert_eq!(sent.len(), 1);
    assert_eq!(
        Msg::decode(sent[0]).unwrap(),
        Msg::Say {
            text: "hello #1".into()
        }
    );

    // The server's echo comes back to every seat, the speaker's too.
    assert!(d.on_msg(&mut h, 3, CH_ECHO, Some(1), &echo(1, 7, "hello #1")));
    let mut list = UiDrawList::new();
    d.hud(&mut h, &mut list);
    // One merged command (same texture): a panel of 4 vertices and the glyph pixels of four lines of text.
    assert_eq!(list.commands().len(), 1);
    assert!(list.vertices().len() > 200, "{}", list.vertices().len());
    let mut m = HelloClient::default();
    // Same state, read through the lines the HUD draws.
    let mut cx = host();
    m.attach(&mut cx, &info());
    m.on_msg(&mut cx, CH_ECHO, Some(1), &echo(1, 7, "hello #1"));
    let lines = m.lines(|id| {
        if id == 1 {
            "BOB".into()
        } else {
            format!("P{id}")
        }
    });
    assert_eq!(lines[2], "BOB SAID: hello #1");
    assert_eq!(lines[3], "ECHOES SO FAR: 7");
    assert_eq!(m.last().map(|e| e.count), Some(7));
}

fn info() -> AttachInfo {
    let me = ModInfo {
        mod_idx: 3,
        id: ModId::new(MOD_ID).unwrap(),
        version: Version::new(0, 1, 0),
        caps: manifest_caps(),
    };
    AttachInfo {
        this: me.clone(),
        set: vec![me],
    }
}

#[test]
fn what_the_hud_shows_comes_back_after_a_restart() {
    let mut h = host();
    {
        let mut d = game(manifest_caps());
        d.attach(&mut h);
        d.on_hotkey(&mut h, 3, HOTKEY_SAY);
        d.on_hotkey(&mut h, 3, HOTKEY_SAY);
        d.on_msg(&mut h, 3, CH_ECHO, Some(0), &echo(0, 2, "hello #2"));
        d.detach(&mut h);
    }
    // The mod's own namespace holds what it needs (the host prefixes the keys, see the app's kv tests).
    assert_eq!(h.kv[&3].0.get(KV_SAID).map(String::as_str), Some("2"));
    assert_eq!(
        h.kv[&3].0.get(KV_LAST).map(String::as_str),
        Some("hello #2")
    );

    // The client starts again: a fresh mod, the same store; the server's `State` brings the count.
    let mut m = HelloClient::default();
    let mut cx = host();
    cx.kv = std::mem::take(&mut h.kv);
    cx.current = 3;
    {
        let caps = manifest_caps();
        let mut g = mod_client::Guarded::new(&mut cx, &caps);
        m.attach(&mut g, &info());
        m.on_msg(&mut g, CH_ECHO, None, &Msg::State { count: 2 }.encode());
    }
    assert_eq!(m.said(), 2);
    assert_eq!(m.count(), 2);
    let lines = m.lines(|id| format!("P{id}"));
    assert_eq!(lines[1], "YOU SAID HELLO 2 TIMES");
    assert_eq!(lines[2], "LAST TIME: hello #2");
    assert_eq!(lines[3], "ECHOES SO FAR: 2");
}

#[test]
fn other_channels_garbage_and_server_says_are_ignored() {
    let mut m = HelloClient::default();
    let mut cx = host();
    m.on_msg(&mut cx, 9, None, &echo(0, 1, "x"));
    assert_eq!(m.count(), 0);
    m.on_msg(&mut cx, CH_ECHO, None, b"\xff\xff");
    m.on_msg(
        &mut cx,
        CH_ECHO,
        None,
        &Msg::Say { text: "no".into() }.encode(),
    );
    assert_eq!(m.count(), 0);
    assert!(m.last().is_none());
    assert!(cx.logged("not understood") && cx.logged("sent a Say"));
}

#[test]
fn the_mod_works_with_fewer_capabilities_than_it_asks_for() {
    // No kv, no hud, no hotkeys: it attaches, ignores the press, draws nothing, and does not panic.
    let mut d = game(Caps::of(&[Cap::NetChannel]));
    let mut h = host();
    d.attach(&mut h);
    assert!(h.hotkeys.registered.is_empty());
    assert!(d.on_hotkey(&mut h, 3, HOTKEY_SAY));
    assert_eq!(h.sent_on(3, CH_ECHO).len(), 1, "saying still works");
    let mut list = UiDrawList::new();
    d.hud(&mut h, &mut list);
    assert!(list.commands().is_empty());
}

#[test]
fn the_hotkey_of_a_nick_less_player_still_shows() {
    // An echo from a player the replica does not know is shown by number.
    let mut m = HelloClient::default();
    let mut cx = host();
    m.on_msg(&mut cx, CH_ECHO, None, &echo(5, 1, "hi"));
    let lines = m.lines(|id| format!("PLAYER {id}"));
    assert_eq!(lines[2], "PLAYER 5 SAID: hi");
}

#[test]
fn the_cube_stands_on_a_plot_of_the_local_player() {
    let mut h = host();
    let mut m = HelloClient::default();
    let mut list = UiDrawList::new();
    // Nothing known yet: the default hex.
    m.hud(&mut h, &mut list);
    assert_eq!(m.home().get(), hello_client::HEX);
    // The game tells who we are and who owns what.
    h.world.set_me(1);
    h.world.set_owner(7, 2, Some(0));
    h.world.set_owner(5, 5, Some(1));
    h.world.set_owner(6, 5, Some(1));
    m.hud(&mut h, &mut list);
    assert_eq!(m.home().get(), (5, 5));
    // A change of the world is picked up.
    h.world.set_owner(2, 1, Some(1));
    m.hud(&mut h, &mut list);
    assert_eq!(m.home().get(), (2, 1));
}

#[test]
fn mod_toml_declares_the_panel_in_hud_right() {
    let m = manifest();
    assert!(m.has_part(Part::Ui));
    let ui = m.ui.as_ref().expect("[ui]");
    assert_eq!(ui.model.as_deref(), Some("ui/model.toml"));
    assert_eq!(ui.scripts, ["ui/hello.js"]);
    assert_eq!(ui.panels.len(), 1);
    let p = &ui.panels[0];
    assert_eq!(
        (p.id.as_str(), p.source.as_str(), p.point.as_str()),
        ("echo", "ui/echo.html", "hud.right")
    );
    assert_eq!((p.mode, p.fallback), (UiMode::Append, UiFallback::Mods));
    // The panel reads the fields the Rust half writes and sends the intent it answers.
    for field in [FIELD_COUNT, FIELD_LAST_FROM, FIELD_LAST_TEXT] {
        assert!(
            MODEL_TOML
                .lines()
                .any(|l| l.split('=').next().map(str::trim) == Some(field)),
            "ui/model.toml declares {field}"
        );
        assert!(
            ECHO_HTML.contains(&format!("mods.hello.{field}")),
            "{field}"
        );
    }
    assert!(ECHO_HTML.contains(&format!("intent mods.hello.{INTENT_SAY}()")));
}

/// A host whose UI model knows the fields of `ui/model.toml`.
fn ui_host() -> MockClientCtx {
    let mut h = host();
    h.ui.insert(
        3,
        MockUiModel::declared(&[FIELD_COUNT, FIELD_LAST_FROM, FIELD_LAST_TEXT]),
    );
    h
}

#[test]
fn the_echo_fills_the_panel_and_its_button_says_hello() {
    let mut d = game(manifest_caps());
    let mut h = ui_host();
    d.attach(&mut h);
    // The server's count and an echo: "Echoes 7 / last: Bob: hello #1".
    assert!(d.on_msg(&mut h, 3, CH_ECHO, None, &Msg::State { count: 6 }.encode()));
    assert_eq!(h.ui_of(3).get(FIELD_COUNT), Some(&UiValue::Int(6)));
    assert!(d.on_msg(&mut h, 3, CH_ECHO, Some(1), &echo(1, 7, "hello #1")));
    let m = h.ui_of(3);
    assert_eq!(m.get(FIELD_COUNT), Some(&UiValue::Int(7)));
    assert_eq!(m.get(FIELD_LAST_FROM), Some(&UiValue::Str("Bob".into())));
    assert_eq!(
        m.get(FIELD_LAST_TEXT),
        Some(&UiValue::Str("hello #1".into()))
    );
    assert!(!h.logged("not updated"), "every write fits the model");

    // The button: the intent `mods.hello.say` reaches the mod as `say` and sends `Say` like the hotkey.
    assert!(d.on_ui_intent(&mut h, 3, INTENT_SAY, &[]));
    let sent = h.sent_on(3, CH_ECHO);
    assert_eq!(sent.len(), 1);
    assert_eq!(
        Msg::decode(sent[0]).unwrap(),
        Msg::Say {
            text: "hello #1".into()
        }
    );
    // Another intent name does nothing.
    assert!(d.on_ui_intent(&mut h, 3, "other", &[]));
    assert_eq!(h.sent_on(3, CH_ECHO).len(), 1);

    // A speaker the replica names only later: the panel shows the nick once it is known.
    assert!(d.on_msg(&mut h, 3, CH_ECHO, Some(5), &echo(5, 8, "hi")));
    assert_eq!(
        h.ui_of(3).get(FIELD_LAST_FROM),
        Some(&UiValue::Str("PLAYER 5".into()))
    );
    h.world.add_player(5, "Eve");
    let _ = d.frame(&mut h, 0.016);
    assert_eq!(
        h.ui_of(3).get(FIELD_LAST_FROM),
        Some(&UiValue::Str("Eve".into()))
    );
    let writes = h.ui_of(3).ops.len();
    let _ = d.frame(&mut h, 0.016);
    assert_eq!(h.ui_of(3).ops.len(), writes, "nothing again while it stays");

    // The classic HUD still draws (`hud()` is kept).
    let mut list = UiDrawList::new();
    d.hud(&mut h, &mut list);
    assert_eq!(list.commands().len(), 1);
}

#[test]
fn without_the_ui_capability_the_panel_is_left_alone() {
    let caps = Caps::of(&[Cap::NetChannel, Cap::WorldRead, Cap::Hud]);
    let mut d = game(caps);
    let mut h = ui_host();
    d.attach(&mut h);
    assert!(d.on_msg(&mut h, 3, CH_ECHO, Some(1), &echo(1, 2, "hi")));
    assert!(h.ui_of(3).ops.is_empty());
    assert!(!h.logged("not updated"), "a missing UI v2 is not a warning");
    // The dispatcher does not hand UI intents to a mod without `ui`.
    assert!(!d.on_ui_intent(&mut h, 3, INTENT_SAY, &[]));
    assert!(h.sent_on(3, CH_ECHO).is_empty());
}
