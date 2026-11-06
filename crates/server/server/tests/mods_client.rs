//! The client half of mods against the real server (T35): two headless clients (`ClientCore` + `ModNet`, the
//! dispatcher of `mod-client`) play a game with the public example mod `hello` over real sockets and the mock
//! worker. Each says something through its client mod, the server half of `hello` echoes it to both seats, and each
//! client mod hears both echoes with the speaker's seat. A client that lacks the mod plays on and hears nothing of
//! it, and a client that lacks a required mod is refused with a readable sentence.

#![allow(dead_code, unused_imports)]

mod common;

use std::cell::RefCell;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use client_core::{
    ClientCore, ClientEvent, ClientPhase, ModNet, NetConfig, NetHost, Sock, SocketCmd,
};
use common::{mock_server_config, temp_dir};
use hello_shared::{CH_ECHO, Msg};
use mod_api::{Cap, Caps, Version};
use mod_client::{AttachInfo, ClientCtx, ClientMod, ClientRegistry, ModBuild};
use mod_host_server::HostRegistry;
use protocol::{
    AutoChoices, GameSettings, LobbyCmd, ModPick, Occupant, RejectReason, TimerMode, TurnMode,
};
use server::clock::TestClock;
use server::identity::Secret;
use server::session::limits::LimitsConfig;
use server::{ServerConfig, Shared};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

const PASSWORD: &str = "mods-client-password";
const MOD_TOML: &str = include_str!("../../../../examples/mods/hello/mod.toml");

// ---------------------------------------------------------------- the server

struct TestServer {
    addr: SocketAddr,
    dir: PathBuf,
    rt: Option<tokio::runtime::Runtime>,
}

impl TestServer {
    fn stop(mut self) {
        if let Some(rt) = self.rt.take() {
            rt.shutdown_timeout(Duration::from_secs(2));
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A server with `hello` (the real `mod.toml` and the real server half) installed; `extra` mods are installed
/// as `gameplay` mods without a registered server half (they only need to exist for a mismatch).
fn boot(name: &str) -> TestServer {
    let dir = temp_dir(name);
    let mods_dir = dir.join("mods");
    std::fs::create_dir_all(mods_dir.join("hello")).unwrap();
    std::fs::write(mods_dir.join("hello").join("mod.toml"), MOD_TOML).unwrap();
    let mut registry = HostRegistry::new();
    registry.register(hello_server::register);
    assert!(registry.problems().is_empty());
    let mut config = mock_server_config(
        &dir,
        "echo turns=1",
        Duration::from_secs(20),
        [Duration::ZERO, Duration::ZERO, Duration::ZERO],
    );
    config.password = Some(Secret::new(PASSWORD));
    config.limits = LimitsConfig {
        password_delay: Duration::from_millis(5),
        ..LimitsConfig::default()
    };
    config.clock = Arc::new(TestClock::new());
    config.mods_dir = Some(mods_dir);
    config.mod_registry = Arc::new(registry);
    config.test_hooks = true;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let listener = rt
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let shared = Shared::new(config).unwrap();
    rt.block_on(shared.supervisor().unwrap().publish_describe(&shared))
        .expect("the mock Describe works");
    {
        let _guard = rt.enter();
        server::game::restore::restore_all(&shared).unwrap();
    }
    rt.spawn(server::serve(listener, shared));
    TestServer {
        addr,
        dir,
        rt: Some(rt),
    }
}

// ---------------------------------------------------------------- the client mod

/// What the test mod saw and what it was told to say.
#[derive(Default)]
struct Mailbox {
    /// Texts to say on the next frame.
    to_say: Vec<String>,
    /// `(speaker seat, count, text)` of every echo; the count of a `State`.
    echoes: Vec<(i8, u32, String)>,
    states: Vec<u32>,
    attached: u32,
    detached: u32,
}

/// A test-only client half of `hello` (the real one is T37): speaks `Msg` on the echo channel.
struct HelloClient(Rc<RefCell<Mailbox>>);

impl ClientMod for HelloClient {
    fn attach(&mut self, _cx: &mut dyn ClientCtx, _info: &AttachInfo) {
        self.0.borrow_mut().attached += 1;
    }

    fn detach(&mut self, _cx: &mut dyn ClientCtx) {
        self.0.borrow_mut().detached += 1;
    }

    fn on_msg(&mut self, _cx: &mut dyn ClientCtx, ch: u8, _from: Option<i8>, payload: &[u8]) {
        assert_eq!(ch, CH_ECHO);
        match Msg::decode(payload).expect("the server half speaks the codec") {
            Msg::Echo { from, count, text } => self.0.borrow_mut().echoes.push((from, count, text)),
            Msg::State { count } => self.0.borrow_mut().states.push(count),
            Msg::Say { .. } => panic!("a Say came from the server"),
        }
    }

    fn frame(&mut self, cx: &mut dyn ClientCtx, _dt: f32) -> mod_client::FrameOut {
        for text in std::mem::take(&mut self.0.borrow_mut().to_say) {
            cx.send(CH_ECHO, &Msg::Say { text }.encode()).unwrap();
        }
        mod_client::FrameOut::default()
    }
}

fn hello_registry(mail: &Rc<RefCell<Mailbox>>, version: Version) -> ClientRegistry {
    let mut r = ClientRegistry::new();
    let mail = mail.clone();
    r.add_built(
        "hello",
        ModBuild {
            version,
            caps: Caps::of(&[Cap::NetChannel]),
            code_hash: [0; 16],
        },
        move || Box::new(HelloClient(mail.clone())),
    );
    r
}

// ---------------------------------------------------------------- a headless client

type Ws = WebSocket<MaybeTlsStream<TcpStream>>;

struct Peer {
    nick: String,
    ws: Ws,
    sock: Sock,
    core: ClientCore,
    mods: ModNet,
    host: NetHost,
    start: Instant,
    events: Vec<ClientEvent>,
}

impl Peer {
    fn connect(srv: &TestServer, nick: &str, registry: ClientRegistry) -> Peer {
        let (mut ws, _) = tungstenite::connect(format!("ws://{}/ws", srv.addr)).unwrap();
        if let MaybeTlsStream::Plain(s) = ws.get_mut() {
            s.set_read_timeout(Some(Duration::from_millis(15))).unwrap();
        }
        let mods = ModNet::new(registry);
        let mut cfg = NetConfig::new("mods-client-test", nick);
        mods.configure(&mut cfg);
        let mut p = Peer {
            nick: nick.into(),
            ws,
            sock: Sock(0),
            core: ClientCore::new(cfg),
            mods,
            host: NetHost::new(),
            start: Instant::now(),
            events: Vec::new(),
        };
        let now = p.now();
        // The typed password is in the session before the socket opens, so the first `Login` carries it.
        p.core.login(nick, PASSWORD);
        p.core.connect(now);
        let Some(SocketCmd::Open { sock, .. }) = p.core.poll_socket_cmd() else {
            panic!("the core does not ask for a socket");
        };
        p.sock = sock;
        p.core.on_connected(now, sock);
        p
    }

    fn now(&self) -> u64 {
        1_000 + self.start.elapsed().as_millis() as u64
    }

    /// One pass: what the core wants to send goes out, what came in is read, the events reach the mods, the mods
    /// run a frame, and what they sent goes out.
    fn pump(&mut self) {
        let now = self.now();
        self.core.tick(now);
        match self.ws.read() {
            Ok(Message::Binary(b)) => self.core.on_bytes(now, self.sock, &b),
            Ok(Message::Close(f)) => panic!("{}: closed by the server: {f:?}", self.nick),
            Ok(_) | Err(tungstenite::Error::Io(_)) => {}
            Err(e) => panic!("{}: the socket failed: {e}", self.nick),
        }
        while let Some(ev) = self.core.poll_event() {
            self.mods.on_event(&ev, &mut self.host);
            self.events.push(ev);
        }
        if let Some(d) = self.mods.dispatcher() {
            d.frame(&mut self.host, 0.016);
        }
        self.host.flush(&mut self.core);
        while let Some((_, bytes)) = self.core.poll_outgoing() {
            self.ws.send(Message::binary(bytes)).unwrap();
        }
        let _ = self.ws.flush();
    }

    fn until(&mut self, what: &str, cond: impl Fn(&Peer) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !cond(self) {
            assert!(
                Instant::now() < deadline,
                "{}: timed out waiting for {what}; events {:?}",
                self.nick,
                self.events
            );
            self.pump();
        }
    }

    fn in_lobby(&self) -> bool {
        self.core.lobby_state().is_some()
    }

    fn has_event(&self, f: impl Fn(&ClientEvent) -> bool) -> bool {
        self.events.iter().any(f)
    }
}

fn settings(picks: &[&str]) -> GameSettings {
    GameSettings {
        name: "mods".into(),
        world_size: "WORLDSIZE_DUEL".into(),
        map_script: "m".into(),
        map_options: vec![],
        speed: "s".into(),
        start_era: "e".into(),
        handicap: "h".into(),
        minor_civs: 0,
        turn_mode: TurnMode::Hybrid,
        timer: TimerMode::Off,
        game_options: vec![],
        seed: Some(1),
        map_seed: None,
        auto_choices: AutoChoices::default(),
        diplo_auto_decline: true,
        mods: picks
            .iter()
            .map(|id| ModPick {
                id: (*id).into(),
                req: "*".into(),
            })
            .collect(),
    }
}

/// alice hosts a room with the `picks` mods, bob joins and is ready, alice starts. Returns when both are in the lobby
/// state of the room and the start is sent; the callers wait for what they want to see.
fn start_game(alice: &mut Peer, bob: &mut Peer, picks: &[&str]) {
    both_until(alice, bob, "both in the lobby", |a, b| {
        a.in_lobby() && b.in_lobby()
    });
    alice
        .core
        .lobby(LobbyCmd::CreateRoom {
            settings: settings(picks),
        })
        .unwrap();
    both_until(alice, bob, "the room in bob's list", |a, b| {
        a.core.lobby_state().is_some_and(|s| s.room.is_some())
            && b.core.lobby_state().is_some_and(|s| !s.rooms.is_empty())
    });
    let room = bob.core.lobby_state().unwrap().rooms[0].id;
    bob.core.lobby(LobbyCmd::JoinRoom { room }).unwrap();
    both_until(alice, bob, "bob in the room", |_, b| {
        b.core.lobby_state().is_some_and(|s| {
            s.room.as_ref().is_some_and(|r| {
                r.slots
                    .iter()
                    .any(|x| matches!(&x.occupant, Occupant::Human { nick, .. } if nick == "bob"))
            })
        })
    });
    bob.core.lobby(LobbyCmd::Ready(true)).unwrap();
    both_until(alice, bob, "bob ready", |a, _| {
        a.core.lobby_state().is_some_and(|s| {
            s.room.as_ref().is_some_and(|r| {
                r.slots
                    .iter()
                    .any(|x| matches!(&x.occupant, Occupant::Human { ready: true, .. }))
            })
        })
    });
    alice.core.lobby(LobbyCmd::Start).unwrap();
}

/// Pumps both peers until `cond` holds.
fn both_until(a: &mut Peer, b: &mut Peer, what: &str, cond: impl Fn(&Peer, &Peer) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !cond(a, b) {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}; alice {:?} bob {:?}",
            a.events,
            b.events
        );
        a.pump();
        b.pump();
    }
}

// ---------------------------------------------------------------- tests

#[test]
fn two_clients_echo_through_the_client_mods_and_the_server_half_of_hello() {
    let srv = boot("mc-echo");
    let alice_mail = Rc::new(RefCell::new(Mailbox::default()));
    let bob_mail = Rc::new(RefCell::new(Mailbox::default()));
    let mut alice = Peer::connect(
        &srv,
        "alice",
        hello_registry(&alice_mail, Version::new(0, 1, 0)),
    );
    let mut bob = Peer::connect(
        &srv,
        "bob",
        hello_registry(&bob_mail, Version::new(0, 1, 0)),
    );
    start_game(&mut alice, &mut bob, &["hello"]);
    both_until(&mut alice, &mut bob, "both mods attached", |a, b| {
        a.mods.active() && b.mods.active()
    });
    assert_eq!(alice_mail.borrow().attached, 1);
    assert_eq!(bob_mail.borrow().attached, 1);
    // The set came before anything else of the stream reached the mods; the server half told each its count.
    both_until(&mut alice, &mut bob, "the counts", |_, _| {
        !alice_mail.borrow().states.is_empty() && !bob_mail.borrow().states.is_empty()
    });
    assert_eq!(alice_mail.borrow().states, [0]);

    alice_mail.borrow_mut().to_say.push("hi from alice".into());
    both_until(&mut alice, &mut bob, "alice's echo", |_, _| {
        !alice_mail.borrow().echoes.is_empty() && !bob_mail.borrow().echoes.is_empty()
    });
    bob_mail.borrow_mut().to_say.push("hello back".into());
    both_until(&mut alice, &mut bob, "bob's echo", |_, _| {
        alice_mail.borrow().echoes.len() == 2 && bob_mail.borrow().echoes.len() == 2
    });
    // Both mods heard both echoes, in order, with the speaker's seat (alice 0, bob 1) and the running count.
    let expect = vec![
        (0, 1, "hi from alice".to_string()),
        (1, 2, "hello back".to_string()),
    ];
    assert_eq!(alice_mail.borrow().echoes, expect);
    assert_eq!(bob_mail.borrow().echoes, expect);
    srv.stop();
}

#[test]
fn a_client_without_the_optional_mod_plays_and_its_traffic_has_no_mod_messages() {
    let srv = boot("mc-optional");
    let alice_mail = Rc::new(RefCell::new(Mailbox::default()));
    let mut alice = Peer::connect(
        &srv,
        "alice",
        hello_registry(&alice_mail, Version::new(0, 1, 0)),
    );
    // Bob's client has no mods at all: no `ModHello`, the game attaches him, his registry is empty.
    let mut bob = Peer::connect(&srv, "bob", ClientRegistry::new());
    start_game(&mut alice, &mut bob, &["hello"]);
    both_until(&mut alice, &mut bob, "attached", |a, b| {
        a.mods.active() && b.has_event(|e| matches!(e, client_core::ClientEvent::Attached { .. }))
    });
    // Bob's core did hear the set (`available = false` for him) and ignores it.
    both_until(&mut alice, &mut bob, "bob's set", |_, b| {
        b.has_event(|e| matches!(e, ClientEvent::ModSet(v) if v.mods.iter().all(|m| !m.available)))
    });
    assert!(!bob.mods.active());
    // Alice talks; the echo reaches her alone. Bob hears no `Mod` message.
    alice_mail.borrow_mut().to_say.push("anyone?".into());
    both_until(&mut alice, &mut bob, "alice's echo", |_, _| {
        !alice_mail.borrow().echoes.is_empty()
    });
    for _ in 0..20 {
        bob.pump();
    }
    assert!(!bob.has_event(|e| matches!(e, ClientEvent::ModMsg(_))));
    srv.stop();
}

#[test]
fn a_client_with_another_version_of_an_optional_mod_plays_without_it() {
    let srv = boot("mc-mismatch");
    // `hello` is cosmetic and optional, so it cannot refuse anybody (the refusal of a required mod is tested
    // with the server host and, on the client, as text in `net_ui`). A client whose `hello` has another version is
    // marked unavailable: the set reaches it, nothing attaches.
    let alice_mail = Rc::new(RefCell::new(Mailbox::default()));
    let bob_mail = Rc::new(RefCell::new(Mailbox::default()));
    let mut alice = Peer::connect(
        &srv,
        "alice",
        hello_registry(&alice_mail, Version::new(0, 1, 0)),
    );
    let mut bob = Peer::connect(
        &srv,
        "bob",
        hello_registry(&bob_mail, Version::new(9, 9, 9)),
    );
    start_game(&mut alice, &mut bob, &["hello"]);
    both_until(&mut alice, &mut bob, "alice attached", |a, b| {
        a.mods.active() && b.has_event(|e| matches!(e, ClientEvent::ModSet(_)))
    });
    assert!(!bob.mods.active(), "another version: not attached");
    assert_eq!(bob_mail.borrow().attached, 0);
    srv.stop();
}
