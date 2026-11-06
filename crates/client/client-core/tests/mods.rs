//! The mod layer of the client (v13, T35) on the scripted server: `ModHello` before `Login`, the set of the game
//! matched with the mods built in, the messages in both directions, and the traffic of a client with no mods.

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use client_core::{ClientEvent, ModNet, NetConfig, NetHost, OrderRefused};
use common::world::World;
use common::*;
use mod_api::{Cap, Caps, ChannelId, PlayerId, Version};
use mod_client::{AttachInfo, ClientCtx, ClientMod, ClientRegistry, CtxError, ModBuild, ModInfo};
use protocol::{
    ClientMsg, DetachReason, GameMsg, ModS2C, ModSetView, ModSync, ModView, ServerMsg, Token,
};

type Log = Rc<RefCell<Vec<String>>>;

/// A mod that writes every call it gets into a shared log, and sends one message on `attach` when asked to.
struct Recorder {
    id: &'static str,
    log: Log,
    say: Option<(ChannelId, Vec<u8>)>,
}

impl ClientMod for Recorder {
    fn attach(&mut self, cx: &mut dyn ClientCtx, info: &AttachInfo) {
        let set: Vec<String> = info
            .set
            .iter()
            .map(|m: &ModInfo| format!("{}@{}", m.id, m.mod_idx))
            .collect();
        self.log.borrow_mut().push(format!(
            "attach {} idx={} set=[{}]",
            self.id,
            info.this.mod_idx,
            set.join(",")
        ));
        if let Some((ch, bytes)) = &self.say {
            let r = cx.send(*ch, bytes);
            self.log
                .borrow_mut()
                .push(format!("send {} ok={}", self.id, r.is_ok()));
        }
    }

    fn detach(&mut self, _cx: &mut dyn ClientCtx) {
        self.log.borrow_mut().push(format!("detach {}", self.id));
    }

    fn on_msg(
        &mut self,
        _cx: &mut dyn ClientCtx,
        ch: ChannelId,
        from: Option<PlayerId>,
        payload: &[u8],
    ) {
        self.log.borrow_mut().push(format!(
            "msg {} ch={ch} from={from:?} {:?}",
            self.id,
            String::from_utf8_lossy(payload)
        ));
    }
}

fn build(caps: &[Cap]) -> ModBuild {
    ModBuild {
        version: Version::new(0, 1, 0),
        caps: Caps::of(caps),
        code_hash: [7; 16],
    }
}

/// `a` and `b` are built in; `b` sends `hi` on channel 3 when it attaches.
fn registry(log: &Log, b_says: bool) -> ClientRegistry {
    let mut r = ClientRegistry::new();
    let l = log.clone();
    r.add_built("alpha", build(&[Cap::NetChannel]), move || {
        Box::new(Recorder {
            id: "alpha",
            log: l.clone(),
            say: None,
        })
    });
    let l = log.clone();
    r.add_built("beta", build(&[Cap::NetChannel]), move || {
        Box::new(Recorder {
            id: "beta",
            log: l.clone(),
            say: b_says.then(|| (3, b"hi".to_vec())),
        })
    });
    r
}

fn view(entries: &[(&str, u16, bool)]) -> ModSetView {
    ModSetView {
        modset_hash: 0xABCD,
        mods: entries
            .iter()
            .map(|(id, idx, available)| ModView {
                id: (*id).into(),
                version: "0.1.0".into(),
                load_idx: *idx,
                sync: ModSync::Cosmetic,
                client_required: false,
                available: *available,
                pack: None,
            })
            .collect(),
    }
}

fn plan() -> GamePlan {
    GamePlan {
        game: 7,
        player: 1,
        stream: 3,
        last_cmd_id: 0,
        last_seq: 0,
    }
}

/// A net whose core carries the mods of `mods` in its handshake; the game's first batch carries `set`.
fn game(mods: &ModNet, set: Option<ModSetView>) -> (Net, Token) {
    let mut cfg = NetConfig::new("test-build", NICK);
    mods.configure(&mut cfg);
    let mut n = Net::new(cfg);
    let mut w = World::new(7, 11, 6, 5);
    w.mods = set;
    n.srv.world = Some(w);
    n.srv.plan = Some(plan());
    n.connect();
    n.login();
    let token = n.core.token().expect("token");
    n.core.set_catalog(protocol::Catalog::default());
    (n, token)
}

/// Gives the events collected so far to the mod layer, then forgets them.
fn feed(n: &mut Net, mods: &mut ModNet, host: &mut NetHost) {
    for ev in n.take_events() {
        mods.on_event(&ev, host);
    }
}

fn mod_msg(mod_idx: u16, ch: u8, from: Option<i8>, payload: &[u8]) -> GameMsg {
    GameMsg::Mod(ModS2C {
        mod_idx,
        ch,
        from,
        payload: payload.to_vec(),
    })
}

fn sent_kinds(n: &Net) -> Vec<&'static str> {
    n.srv.log.iter().map(|(_, m)| m.kind()).collect()
}

// ---------------------------------------------------------------- the handshake

#[test]
fn a_client_with_mods_says_so_between_hello_and_login() {
    let log = Log::default();
    let mods = ModNet::new(registry(&log, false));
    let (n, _) = game(&mods, None);
    let kinds = sent_kinds(&n);
    assert_eq!(&kinds[..3], ["Hello", "ModHello", "Login"], "{kinds:?}");
    let Some((_, ClientMsg::ModHello { api, built_in })) = n.srv.log.get(1) else {
        panic!("no ModHello");
    };
    assert_eq!(*api, (1, 0, 0));
    let ids: Vec<_> = built_in
        .iter()
        .map(|b| (b.id.as_str(), b.version.as_str(), b.code_hash))
        .collect();
    assert_eq!(
        ids,
        [("alpha", "0.1.0", [7; 16]), ("beta", "0.1.0", [7; 16])]
    );
}

#[test]
fn a_client_without_mods_sends_exactly_what_it_did_before() {
    let mods = ModNet::new(ClientRegistry::new());
    assert!(mods.is_empty());
    let (n, _) = game(&mods, None);
    assert!(
        n.srv.log.iter().all(|(_, m)| m.kind() != "ModHello"),
        "{:?}",
        sent_kinds(&n)
    );
    assert_eq!(&sent_kinds(&n)[..2], ["Hello", "Login"]);
    // A plain core (no `configure`) behaves the same.
    let mut plain = Net::start();
    plain.connect();
    plain.login();
    assert_eq!(sent_kinds(&plain)[..2], ["Hello", "Login"]);
}

#[test]
fn every_handshake_repeats_the_hello_with_the_mods() {
    let log = Log::default();
    let mods = ModNet::new(registry(&log, false));
    let (mut n, token) = game(&mods, None);
    n.server_drops(token, 1006);
    n.advance(5_000);
    let hellos = n.srv.sent(|m| matches!(m, ClientMsg::ModHello { .. }));
    assert!(
        hellos >= 2,
        "the reconnect carries a ModHello again: {hellos}"
    );
    // Each Login on a socket follows its ModHello.
    let log = &n.srv.log;
    for (i, (sock, m)) in log.iter().enumerate() {
        if matches!(m, ClientMsg::Login(_)) {
            assert!(
                matches!(log[i - 1], (s, ClientMsg::ModHello { .. }) if s == *sock),
                "Login at {i} is not right after a ModHello"
            );
        }
    }
}

// ---------------------------------------------------------------- the set

#[test]
fn the_set_attaches_the_mods_that_are_here_in_load_order_and_skips_the_rest() {
    let log = Log::default();
    let mut mods = ModNet::new(registry(&log, false));
    // Load order: b, x (this client lacks it: the server says unavailable), a.
    let set = view(&[("beta", 0, true), ("xeno", 1, false), ("alpha", 2, true)]);
    let (mut n, _) = game(&mods, Some(set));
    let mut host = NetHost::new();
    assert!(n.has_event(|e| matches!(e, ClientEvent::ModSet(v) if v.mods.len() == 3)));
    feed(&mut n, &mut mods, &mut host);
    assert!(mods.active());
    assert_eq!(mods.set_hash(), Some(0xABCD));
    assert_eq!(
        *log.borrow(),
        [
            "attach beta idx=0 set=[beta@0,alpha@2]",
            "attach alpha idx=2 set=[beta@0,alpha@2]"
        ]
    );
}

#[test]
fn a_registered_mod_the_server_marks_unavailable_stays_out() {
    let log = Log::default();
    let mut mods = ModNet::new(registry(&log, false));
    // The server found another version of `b` in the hello.
    let set = view(&[("alpha", 0, true), ("beta", 1, false)]);
    let (mut n, _) = game(&mods, Some(set));
    let mut host = NetHost::new();
    feed(&mut n, &mut mods, &mut host);
    assert_eq!(*log.borrow(), ["attach alpha idx=0 set=[alpha@0]"]);
}

#[test]
fn a_set_with_nothing_of_ours_attaches_nothing() {
    let log = Log::default();
    let mut mods = ModNet::new(registry(&log, false));
    let (mut n, _) = game(&mods, Some(view(&[("xeno", 0, false)])));
    let mut host = NetHost::new();
    feed(&mut n, &mut mods, &mut host);
    assert!(!mods.active());
    assert!(log.borrow().is_empty());
}

#[test]
fn an_empty_registry_ignores_the_set_and_the_messages() {
    let mut mods = ModNet::new(ClientRegistry::new());
    let (mut n, token) = game(&mods, Some(view(&[("alpha", 0, false)])));
    let m = n
        .srv
        .world
        .as_mut()
        .unwrap()
        .push(vec![mod_msg(0, 0, None, b"xeno")]);
    n.push(token, m);
    n.settle();
    let mut host = NetHost::new();
    feed(&mut n, &mut mods, &mut host);
    assert!(!mods.active());
    assert!(!host.has_outgoing());
}

// ---------------------------------------------------------------- messages in

#[test]
fn a_mod_message_reaches_the_mod_its_index_names_with_channel_sender_and_bytes() {
    let log = Log::default();
    let mut mods = ModNet::new(registry(&log, false));
    let set = view(&[("beta", 0, true), ("xeno", 1, false), ("alpha", 2, true)]);
    let (mut n, token) = game(&mods, Some(set));
    let mut host = NetHost::new();
    feed(&mut n, &mut mods, &mut host);
    log.borrow_mut().clear();
    let batch = n.srv.world.as_mut().unwrap().push(vec![
        mod_msg(2, 5, Some(1), b"for a"),
        mod_msg(0, 0, None, b"for b"),
        // The mod at index 1 is not here; an index beyond the set does not exist.
        mod_msg(1, 0, None, b"for x"),
        mod_msg(9, 0, None, b"nobody"),
    ]);
    n.push(token, batch);
    n.settle();
    feed(&mut n, &mut mods, &mut host);
    assert_eq!(
        *log.borrow(),
        [
            "msg alpha ch=5 from=Some(1) \"for a\"",
            "msg beta ch=0 from=None \"for b\""
        ]
    );
}

#[test]
fn a_mod_without_the_channel_capability_gets_nothing() {
    let log = Log::default();
    let mut r = ClientRegistry::new();
    let l = log.clone();
    r.add_built("alpha", build(&[]), move || {
        Box::new(Recorder {
            id: "alpha",
            log: l.clone(),
            say: None,
        })
    });
    let mut mods = ModNet::new(r);
    let (mut n, token) = game(&mods, Some(view(&[("alpha", 0, true)])));
    let mut host = NetHost::new();
    feed(&mut n, &mut mods, &mut host);
    let batch = n
        .srv
        .world
        .as_mut()
        .unwrap()
        .push(vec![mod_msg(0, 0, None, b"xeno")]);
    n.push(token, batch);
    n.settle();
    feed(&mut n, &mut mods, &mut host);
    assert_eq!(*log.borrow(), ["attach alpha idx=0 set=[alpha@0]"]);
}

#[test]
fn messages_of_a_batch_the_stream_refused_are_not_delivered() {
    let log = Log::default();
    let mut mods = ModNet::new(registry(&log, false));
    let (mut n, token) = game(&mods, Some(view(&[("alpha", 0, true)])));
    let mut host = NetHost::new();
    feed(&mut n, &mut mods, &mut host);
    log.borrow_mut().clear();
    // A batch is skipped: the one that comes is a gap, the stream breaks, and nothing of it reaches the mods.
    let w = n.srv.world.as_mut().unwrap();
    let _lost = w.push(vec![mod_msg(0, 0, None, b"lost")]);
    let gap = w.push(vec![mod_msg(0, 0, None, b"gap")]);
    n.push(token, gap);
    n.settle();
    assert!(!n.has_event(|e| matches!(e, ClientEvent::ModMsg(_))));
    feed(&mut n, &mut mods, &mut host);
    assert!(log.borrow().iter().all(|l| !l.starts_with("msg")));
}

#[test]
fn an_oversized_message_from_the_server_is_dropped() {
    let log = Log::default();
    let mut mods = ModNet::new(registry(&log, false));
    let (mut n, token) = game(&mods, Some(view(&[("alpha", 0, true)])));
    let mut host = NetHost::new();
    feed(&mut n, &mut mods, &mut host);
    log.borrow_mut().clear();
    let big = vec![0u8; protocol::MOD_PAYLOAD_MAX + 1];
    let batch = n
        .srv
        .world
        .as_mut()
        .unwrap()
        .push(vec![mod_msg(0, 0, None, &big)]);
    n.push(token, batch);
    n.settle();
    feed(&mut n, &mut mods, &mut host);
    assert!(log.borrow().is_empty());
}

// ---------------------------------------------------------------- messages out

#[test]
fn send_becomes_a_mod_message_with_the_load_index_of_the_game() {
    let log = Log::default();
    let mut mods = ModNet::new(registry(&log, true));
    let set = view(&[("alpha", 0, true), ("xeno", 1, false), ("beta", 2, true)]);
    let (mut n, _) = game(&mods, Some(set));
    let mut host = NetHost::new();
    feed(&mut n, &mut mods, &mut host);
    assert!(log.borrow().contains(&"send beta ok=true".to_string()));
    assert!(host.has_outgoing());
    host.flush(&mut n.core);
    n.settle();
    let sent: Vec<_> = n
        .srv
        .log
        .iter()
        .filter_map(|(_, m)| match m {
            ClientMsg::Mod(m) => Some((m.mod_idx, m.ch, m.payload.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(sent, [(2, 3, b"hi".to_vec())]);
    assert!(!host.has_outgoing());
}

#[test]
fn send_without_the_capability_is_denied_and_a_huge_payload_is_refused() {
    struct Sender;
    impl ClientMod for Sender {
        fn attach(&mut self, cx: &mut dyn ClientCtx, _: &AttachInfo) {
            let big = vec![0u8; 56 * 1024 + 1];
            let r = cx.send(0, &big);
            assert!(matches!(
                r,
                Err(CtxError::TooLarge { .. }) | Err(CtxError::Denied(_))
            ));
        }
    }
    for caps in [&[][..], &[Cap::NetChannel][..]] {
        let mut r = ClientRegistry::new();
        r.add_built("sender", build(caps), || Box::new(Sender));
        let mut mods = ModNet::new(r);
        let (mut n, _) = game(&mods, Some(view(&[("sender", 0, true)])));
        let mut host = NetHost::new();
        feed(&mut n, &mut mods, &mut host);
        assert!(!host.has_outgoing(), "caps {caps:?}");
    }
}

#[test]
fn the_core_refuses_what_it_cannot_send() {
    let mut n = Net::start();
    // No line, no game.
    assert!(n.core.send_mod(0, 0, vec![1]).is_err());
    n.srv.world = Some(World::new(7, 11, 6, 5));
    n.srv.plan = Some(plan());
    n.connect();
    n.login();
    assert_eq!(n.core.send_mod(0, 0, vec![1]), Ok(()));
    let big = vec![0u8; protocol::MOD_PAYLOAD_MAX + 1];
    assert_eq!(n.core.send_mod(0, 0, big), Err(OrderRefused::TooBig));
    let exact = vec![0u8; protocol::MOD_PAYLOAD_MAX];
    assert_eq!(n.core.send_mod(0, 0, exact), Ok(()));
}

// ---------------------------------------------------------------- lifetime

#[test]
fn a_new_set_detaches_the_old_mods_first_and_a_detach_event_ends_them() {
    let log = Log::default();
    let mut mods = ModNet::new(registry(&log, false));
    let (mut n, token) = game(&mods, Some(view(&[("alpha", 0, true), ("beta", 1, true)])));
    let mut host = NetHost::new();
    feed(&mut n, &mut mods, &mut host);
    log.borrow_mut().clear();
    // The same set again (a fresh stream after a gap): detach in reverse, then attach.
    let again = view(&[("alpha", 0, true), ("beta", 1, true)]);
    let batch = n
        .srv
        .world
        .as_mut()
        .unwrap()
        .push(vec![GameMsg::Mods(again)]);
    n.push(token, batch);
    n.settle();
    feed(&mut n, &mut mods, &mut host);
    assert_eq!(
        *log.borrow(),
        [
            "detach beta",
            "detach alpha",
            "attach alpha idx=0 set=[alpha@0,beta@1]",
            "attach beta idx=1 set=[alpha@0,beta@1]"
        ]
    );
    log.borrow_mut().clear();
    n.push(
        token,
        ServerMsg::Detached {
            reason: DetachReason::Left,
        },
    );
    n.settle();
    feed(&mut n, &mut mods, &mut host);
    assert_eq!(*log.borrow(), ["detach beta", "detach alpha"]);
    assert!(!mods.active());
    // A message after the detach finds nobody.
    mods.on_event(
        &ClientEvent::ModMsg(ModS2C {
            mod_idx: 0,
            ch: 0,
            from: None,
            payload: vec![],
        }),
        &mut host,
    );
    assert_eq!(log.borrow().len(), 2);
}
