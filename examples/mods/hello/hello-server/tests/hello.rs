//! The server half of `hello` on the mock host, built from the real `mod.toml`.

use hello_server::{HelloServer, register};
use hello_shared::{CH_ECHO, MAX_TEXT_BYTES, Msg};
use mod_api::testing::{MockCtx, MockFacts, MockHost};
use mod_api::{
    Audience, Cap, Caps, Limiter, MOD_API_VERSION, Outgoing, Seat, ServerMod, ServerRegistry,
    Verdict,
};
use mod_manifest::{ClientPolicy, Manifest, ModId, Part, SyncClass};

const MOD_TOML: &str = include_str!("../../mod.toml");

fn manifest() -> Manifest {
    Manifest::parse(MOD_TOML).unwrap_or_else(|e| panic!("mod.toml: {e}"))
}

/// A mock host with the capabilities and channels `mod.toml` declares.
fn host() -> MockCtx {
    let m = manifest();
    MockCtx {
        caps: Caps::from_manifest(&m).unwrap(),
        host: MockHost {
            facts: MockFacts::new(8, 8),
            limiter: Limiter::from_manifest(&m),
            now_ms: 0,
            logs: Vec::new(),
        },
    }
}

fn started(h: &mut MockCtx, saved: Option<&[u8]>) -> Box<dyn ServerMod> {
    let mut reg = ServerRegistry::new();
    register(&mut reg);
    assert!(reg.problems().is_empty());
    let mut m = reg
        .create(&ModId::new("hello").unwrap())
        .expect("registered");
    h.start(m.as_mut(), saved);
    m
}

fn say(text: &str) -> Vec<u8> {
    Msg::Say { text: text.into() }.encode()
}

fn decode(o: &Outgoing) -> Msg {
    Msg::decode(&o.payload).unwrap()
}

// ----- the manifest -----

#[test]
fn mod_toml_follows_the_example_contract() {
    let m = manifest();
    assert_eq!(m.id.as_str(), "hello");
    assert_eq!(m.sync, SyncClass::Cosmetic);
    assert_eq!(m.client_policy(), ClientPolicy::Optional);
    assert!(m.has_part(Part::ServerRs) && m.has_part(Part::ClientRs));
    assert!(m.api.matches(&MOD_API_VERSION));
    assert_eq!(m.license, "MIT");
    assert_eq!(m.channels.len(), 1);
    assert_eq!((m.channels[0].id, m.channels[0].name.as_str()), (0, "echo"));
    let caps = Caps::from_manifest(&m).unwrap();
    assert!(caps.has(Cap::NetChannel));
    // The server half uses `net.channel` only; the rest belong to the client half (hello-client).
    assert_eq!(caps.iter().count(), 6);
}

#[test]
fn registers_with_one_function_and_no_problems() {
    let mut reg = ServerRegistry::new();
    register(&mut reg);
    assert!(reg.problems().is_empty());
    assert_eq!(
        reg.ids().map(ToString::to_string).collect::<Vec<_>>(),
        ["hello"]
    );
    // Registering twice is reported, not fatal.
    register(&mut reg);
    assert_eq!(reg.problems().len(), 1);
}

// ----- echo -----

#[test]
fn echo_goes_to_everybody_with_the_speaker_and_the_count() {
    let mut h = host();
    let mut m = started(&mut h, None);
    let d = h.deliver(m.as_mut(), Seat(3), CH_ECHO, &say("hi"));
    assert_eq!(d.refused, None);
    assert_eq!(d.sent.len(), 1);
    assert_eq!(d.sent[0].to, Audience::All);
    assert_eq!(d.sent[0].ch, CH_ECHO);
    assert_eq!(
        decode(&d.sent[0]),
        Msg::Echo {
            from: 3,
            count: 1,
            text: "hi".into()
        }
    );
    let d = h.deliver(m.as_mut(), Seat(5), CH_ECHO, &say("again"));
    assert_eq!(
        decode(&d.sent[0]),
        Msg::Echo {
            from: 5,
            count: 2,
            text: "again".into()
        }
    );
}

#[test]
fn malformed_and_unexpected_input_is_ignored_and_logged() {
    let mut h = host();
    let mut m = started(&mut h, None);
    for bad in [
        vec![],
        vec![9, 9],
        vec![1, 200, 0],
        Msg::State { count: 99 }.encode(),
        Msg::Echo {
            from: 0,
            count: 0,
            text: String::new(),
        }
        .encode(),
    ] {
        let d = h.deliver(m.as_mut(), Seat(1), CH_ECHO, &bad);
        assert!(d.sent.is_empty(), "{bad:?}");
    }
    assert_eq!(h.host.logs.len(), 5);
    // A channel the mod does not use is refused by the host before the mod sees it.
    let d = h.deliver(m.as_mut(), Seat(1), 7, &say("x"));
    assert_eq!(d.refused, Some(Verdict::UnknownChannel));
    // None of it counted.
    let att = h.attach(m.as_mut(), Seat(1));
    assert_eq!(decode(&att.sent[0]), Msg::State { count: 0 });
}

// ----- limits come from the manifest -----

#[test]
fn host_limits_cut_oversize_and_bursts() {
    let mut h = host();
    let mut m = started(&mut h, None);
    // 512 bytes is the channel's c2s_max; a full-length text is 3 + 256 bytes and fits.
    let full = say(&"a".repeat(MAX_TEXT_BYTES));
    assert!(
        h.deliver(m.as_mut(), Seat(1), CH_ECHO, &full)
            .refused
            .is_none()
    );
    let big = vec![1u8; 513];
    assert!(matches!(
        h.deliver(m.as_mut(), Seat(2), CH_ECHO, &big).refused,
        Some(Verdict::TooLarge { len: 513, max: 512 })
    ));
    // burst = 10, rate 5/s: seat 4 can send 10 at once, the 11th is cut, 200 ms later one more fits.
    let mut ok = 0;
    for _ in 0..12 {
        if h.deliver(m.as_mut(), Seat(4), CH_ECHO, &say("x"))
            .refused
            .is_none()
        {
            ok += 1;
        }
    }
    assert_eq!(ok, 10);
    h.advance_ms(200);
    assert!(
        h.deliver(m.as_mut(), Seat(4), CH_ECHO, &say("x"))
            .refused
            .is_none()
    );
    // Another seat has its own bucket.
    assert!(
        h.deliver(m.as_mut(), Seat(5), CH_ECHO, &say("x"))
            .refused
            .is_none()
    );
}

// ----- replay and persistence -----

#[test]
fn a_late_seat_is_told_the_count() {
    let mut h = host();
    let mut m = started(&mut h, None);
    for _ in 0..3 {
        h.deliver(m.as_mut(), Seat(1), CH_ECHO, &say("x"));
    }
    let d = h.attach(m.as_mut(), Seat(6));
    assert_eq!(d.sent.len(), 1);
    assert_eq!(d.sent[0].to, Audience::Sender);
    assert_eq!(decode(&d.sent[0]), Msg::State { count: 3 });
}

#[test]
fn the_counter_survives_a_restart() {
    let mut h = host();
    let mut m = started(&mut h, None);
    for _ in 0..4 {
        h.deliver(m.as_mut(), Seat(1), CH_ECHO, &say("x"));
    }
    let saved = m.persist();
    assert!(
        Limiter::from_manifest(&manifest()).check_state(saved.len()),
        "fits state_max"
    );

    let mut h2 = host();
    let mut again = started(&mut h2, Some(&saved));
    let d = h2.deliver(again.as_mut(), Seat(1), CH_ECHO, &say("y"));
    assert!(matches!(decode(&d.sent[0]), Msg::Echo { count: 5, .. }));
    assert_eq!(h2.attach(again.as_mut(), Seat(2)).sent.len(), 1);
}

#[test]
fn unreadable_saved_state_starts_fresh_with_a_warning() {
    for bad in [&[][..], &[1, 2][..], &[9, 0, 0, 0, 0][..]] {
        let mut h = host();
        let mut m = started(&mut h, Some(bad));
        assert!(!h.host.logs.is_empty());
        let d = h.deliver(m.as_mut(), Seat(1), CH_ECHO, &say("x"));
        assert!(matches!(decode(&d.sent[0]), Msg::Echo { count: 1, .. }));
    }
}

#[test]
fn dirty_flag_follows_changes_and_saves() {
    let mut h = host();
    let mut m = started(&mut h, Some(&[1, 5, 0, 0, 0]));
    assert!(!m.is_dirty(), "a freshly loaded state is clean");
    h.deliver(m.as_mut(), Seat(1), CH_ECHO, &say("x"));
    assert!(m.is_dirty());
    m.mark_saved();
    assert!(!m.is_dirty());
    // A message that changes nothing does not dirty it.
    h.deliver(m.as_mut(), Seat(1), CH_ECHO, &[]);
    assert!(!m.is_dirty());
}

#[test]
fn the_count_saturates_instead_of_wrapping() {
    let mut h = host();
    let mut m = started(&mut h, Some(&[1, 255, 255, 255, 255]));
    let d = h.deliver(m.as_mut(), Seat(1), CH_ECHO, &say("x"));
    assert!(matches!(
        decode(&d.sent[0]),
        Msg::Echo {
            count: u32::MAX,
            ..
        }
    ));
}

#[test]
fn the_struct_is_usable_directly() {
    let h = HelloServer::default();
    assert_eq!(h.count(), 0);
}
