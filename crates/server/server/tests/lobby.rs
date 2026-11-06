//! The lobby over real sockets with the mock worker: rooms, slots, readiness, `Start`,
//! the `LobbyState` after a `Detached`, the admin routes, and (G5 part 2) the list of saves,
//! `LoadSave` with reserved slots, `JoinGame` into an AI slot (and the race of two), the own seat
//! again, and the 60 s rule for the members of a room.

mod common;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use common::{mock_server_config, temp_dir};
use protocol::{
    AutoChoices, CivPick, ClientMsg, Codec, DetachReason, GameMsg, GameSettings, GameStatus,
    LobbyCmd, LobbyState, Login, Occupant, PROTOCOL_VERSION, PlayerPlace, Postcard, RoomState,
    SeatControl, SeatView, ServerMsg, SlotOccupant, TimerMode, Token, TurnMode,
};
use server::clock::TestClock;
use server::identity::Secret;
use server::session::limits::LimitsConfig;
use server::{ServerConfig, Shared};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};
use wire::{Encoder, Limits};

const PASSWORD: &str = "lobby-password";
const ADMIN: &str = "admin-secret";

struct TestServer {
    addr: SocketAddr,
    admin: SocketAddr,
    shared: Arc<Shared>,
    dir: std::path::PathBuf,
    _rt: tokio::runtime::Runtime,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn start_with(name: &str, mode: &str, edit: impl FnOnce(&mut ServerConfig)) -> TestServer {
    let dir = temp_dir(name);
    let mut config = mock_server_config(
        &dir,
        mode,
        Duration::from_secs(20),
        [Duration::ZERO, Duration::ZERO, Duration::ZERO],
    );
    config.password = Some(Secret::new(PASSWORD));
    config.admin_token = Some(Secret::new(ADMIN));
    config.limits = LimitsConfig {
        password_delay: Duration::from_millis(5),
        ..LimitsConfig::default()
    };
    edit(&mut config);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let bind = || {
        rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .unwrap()
    };
    let (listener, admin) = (bind(), bind());
    let (addr, admin_addr) = (listener.local_addr().unwrap(), admin.local_addr().unwrap());
    let shared = Shared::new(config).unwrap();
    rt.block_on(shared.supervisor().unwrap().publish_describe(&shared))
        .expect("the mock Describe works");
    rt.spawn(server::serve(listener, shared.clone()));
    rt.spawn(server::serve_admin(admin, shared.clone()));
    TestServer {
        addr,
        admin: admin_addr,
        shared,
        dir,
        _rt: rt,
    }
}

fn start(name: &str) -> TestServer {
    start_with(name, "echo", |_| {})
}

type Ws = WebSocket<MaybeTlsStream<TcpStream>>;

struct Client {
    ws: Ws,
    nick: String,
    token: Option<Token>,
}

impl Client {
    fn login(srv: &TestServer, nick: &str) -> Client {
        Self::connect(srv, nick, PASSWORD, None)
    }

    fn connect(srv: &TestServer, nick: &str, password: &str, token: Option<Token>) -> Client {
        let (mut ws, _) = tungstenite::connect(format!("ws://{}/ws", srv.addr)).unwrap();
        if let MaybeTlsStream::Plain(s) = ws.get_mut() {
            s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        }
        let mut c = Client {
            ws,
            nick: nick.into(),
            token,
        };
        c.send(&ClientMsg::Hello {
            protocol: PROTOCOL_VERSION,
            build: "test".into(),
            nick: nick.into(),
        });
        c.send(&ClientMsg::Login(Login {
            password: password.into(),
            token,
            resume: None,
        }));
        c
    }

    fn send(&mut self, msg: &ClientMsg) {
        let body = Postcard.encode(msg).unwrap();
        self.ws
            .send(Message::binary(Encoder::plain(&body)))
            .unwrap();
    }

    fn lobby(&mut self, cmd: LobbyCmd) {
        self.send(&ClientMsg::Lobby(cmd));
    }

    fn recv(&mut self) -> ServerMsg {
        // Server pings keep `read` from timing out, so the wait is bounded here.
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "{}: nothing came within 15 s",
                self.nick
            );
            match self.ws.read().unwrap() {
                Message::Binary(b) => {
                    let msg: ServerMsg = wire::decode(&b, &Limits::CLIENT).unwrap();
                    if let ServerMsg::LoggedIn(session) = &msg {
                        self.token = session.token;
                    }
                    return msg;
                }
                Message::Close(_) => panic!("the socket closed"),
                _ => {}
            }
        }
    }

    /// The first `LobbyState` that satisfies `pred`; a `LobbyError` on the way is a failure.
    fn state(&mut self, what: &str, pred: impl Fn(&LobbyState) -> bool) -> LobbyState {
        for _ in 0..100 {
            match self.recv() {
                ServerMsg::LobbyState(s) if pred(&s) => return s,
                ServerMsg::LobbyError { text } => panic!(
                    "{}: unexpected LobbyError `{text}` waiting for {what}",
                    self.nick
                ),
                _ => {}
            }
        }
        panic!("{}: no LobbyState with {what}", self.nick);
    }

    fn room(&mut self, what: &str, pred: impl Fn(&RoomState) -> bool) -> RoomState {
        let s = self.state(what, |s| s.room.as_ref().is_some_and(&pred));
        s.room.unwrap()
    }

    /// The text of the next `LobbyError`.
    fn error(&mut self) -> String {
        for _ in 0..100 {
            if let ServerMsg::LobbyError { text } = self.recv() {
                return text;
            }
        }
        panic!("{}: no LobbyError", self.nick);
    }

    fn until(&mut self, what: &str, pred: impl Fn(&ServerMsg) -> bool) -> ServerMsg {
        for _ in 0..200 {
            let m = self.recv();
            if pred(&m) {
                return m;
            }
        }
        panic!("{}: no {what}", self.nick);
    }
}

fn settings(name: &str) -> GameSettings {
    GameSettings {
        name: name.into(),
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
        mods: Vec::new(),
    }
}

fn is_human(o: &Occupant, nick: &str) -> bool {
    matches!(o, Occupant::Human { nick: n, .. } if n == nick)
}

fn http_get(addr: SocketAddr, path: &str) -> (u16, String) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        s,
        "GET {path} HTTP/1.0\r\nHost: localhost\r\nX-Admin-Token: {ADMIN}\r\n\r\n"
    )
    .unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    let status = raw.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = raw.split("\r\n\r\n").nth(1).unwrap_or_default().to_owned();
    (status, body)
}

/// A room of alice (host) and bob, both in it, bob ready.
fn room_of_two(srv: &TestServer) -> (Client, Client) {
    let mut alice = Client::login(srv, "alice");
    alice.state("the lobby", |_| true);
    alice.lobby(LobbyCmd::CreateRoom {
        settings: settings("duel"),
    });
    let room = alice.room("the room", |_| true);
    let mut bob = Client::login(srv, "bob");
    bob.state("the lobby", |s| s.rooms.len() == 1);
    bob.lobby(LobbyCmd::JoinRoom { room: room.id });
    bob.room("bob in the room", |r| {
        r.slots.iter().any(|s| is_human(&s.occupant, "bob"))
    });
    bob.lobby(LobbyCmd::Ready(true));
    alice.room("bob ready", |r| {
        r.slots
            .iter()
            .any(|s| matches!(&s.occupant, Occupant::Human { nick, ready: true } if nick == "bob"))
    });
    (alice, bob)
}

// ---------------------------------------------------------------- login

#[test]
fn login_gets_the_choices_of_the_describe_run_and_an_empty_lobby() {
    let srv = start("login");
    let mut c = Client::login(&srv, "alice");
    let ServerMsg::Choices(ch) = c.until("Choices", |m| matches!(m, ServerMsg::Choices(_))) else {
        unreachable!()
    };
    assert_eq!(ch.world_sizes[0].id, "WORLDSIZE_DUEL");
    let s = c.state("a LobbyState", |_| true);
    assert!(s.rooms.is_empty() && s.games.is_empty() && s.room.is_none());
}

// ---------------------------------------------------------------- rooms

#[test]
fn a_room_follows_the_world_size_and_the_host_sits_in_slot_zero() {
    let srv = start("room");
    let mut alice = Client::login(&srv, "alice");
    alice.state("the lobby", |_| true);
    alice.lobby(LobbyCmd::CreateRoom {
        settings: settings("duel"),
    });
    let room = alice.room("the room", |_| true);
    assert_eq!(
        (room.host.as_str(), room.status),
        ("alice", GameStatus::Lobby)
    );
    // The mock's map: 4 slots, 2 default players: slots 2 and 3 start closed.
    assert_eq!(room.slots.len(), 4);
    assert!(is_human(&room.slots[0].occupant, "alice"));
    assert_eq!(room.slots[1].occupant, Occupant::Open);
    assert_eq!(room.slots[2].occupant, Occupant::Closed);
    assert_eq!(room.slots[3].occupant, Occupant::Closed);
    // Others see it in the list.
    let mut bob = Client::login(&srv, "bob");
    let s = bob.state("the room in the list", |s| s.rooms.len() == 1);
    assert_eq!(s.rooms[0].host, "alice");
    assert_eq!((s.rooms[0].humans, s.rooms[0].slots), (1, 2));
    assert!(s.room.is_none());
}

#[test]
fn settings_are_checked_against_the_lists() {
    let srv = start("bad-settings");
    let mut a = Client::login(&srv, "alice");
    a.state("the lobby", |_| true);
    let mut bad = settings("x");
    bad.world_size = "WORLDSIZE_GIGANTIC".into();
    a.lobby(LobbyCmd::CreateRoom { settings: bad });
    assert!(a.error().contains("world size"));
    let mut bad = settings("x");
    bad.game_options = vec![("QUICK_COMBAT".into(), 1)];
    a.lobby(LobbyCmd::CreateRoom { settings: bad });
    assert!(a.error().contains("not allowed"));
    let mut bad = settings("x");
    bad.timer = TimerMode::Fixed { seconds: 5 };
    a.lobby(LobbyCmd::CreateRoom { settings: bad });
    assert!(a.error().contains("timer"));
    // The lobby is still usable.
    a.lobby(LobbyCmd::CreateRoom {
        settings: settings("ok"),
    });
    a.room("the room", |_| true);
}

#[test]
fn joining_takes_the_first_open_slot_and_a_full_room_refuses() {
    let srv = start("join");
    let (_alice, mut bob) = room_of_two(&srv);
    // The room has no open slot left (2 and 3 are closed).
    let mut carol = Client::login(&srv, "carol");
    let s = carol.state("the list", |s| s.rooms.len() == 1);
    carol.lobby(LobbyCmd::JoinRoom {
        room: s.rooms[0].id,
    });
    assert!(carol.error().contains("full"));
    // One room, one place per nick.
    bob.lobby(LobbyCmd::CreateRoom {
        settings: settings("second"),
    });
    assert!(bob.error().contains("room already"));
}

#[test]
fn ready_is_reset_by_any_change_of_the_room() {
    let srv = start("reset");
    let (mut alice, _bob) = room_of_two(&srv);
    let mut s = settings("renamed");
    s.speed = "s".into();
    alice.lobby(LobbyCmd::Settings(s));
    alice.room("ready reset", |r| {
        r.settings.name == "renamed"
            && r.slots
                .iter()
                .all(|s| !matches!(s.occupant, Occupant::Human { ready: true, .. }))
    });
}

#[test]
fn slot_rules_open_removes_a_human_ai_and_closed_refuse() {
    let srv = start("slots");
    let (mut alice, mut bob) = room_of_two(&srv);
    // Ai / Closed on a slot with a human: refused.
    alice.lobby(LobbyCmd::Slot {
        index: 1,
        occupant: SlotOccupant::Ai,
        civ: CivPick::Random,
    });
    assert!(alice.error().contains("first"));
    alice.lobby(LobbyCmd::Slot {
        index: 1,
        occupant: SlotOccupant::Closed,
        civ: CivPick::Random,
    });
    assert!(alice.error().contains("first"));
    // Not the host: refused.
    bob.lobby(LobbyCmd::Slot {
        index: 2,
        occupant: SlotOccupant::Ai,
        civ: CivPick::Random,
    });
    assert!(bob.error().contains("host"));
    // The host opens bob's slot: bob is out of the room, with a reason.
    alice.lobby(LobbyCmd::Slot {
        index: 1,
        occupant: SlotOccupant::Open,
        civ: CivPick::Random,
    });
    assert!(bob.error().contains("removed"));
    bob.state("no room", |s| s.room.is_none());
    let r = alice.room("slot 1 open", |r| r.slots[1].occupant == Occupant::Open);
    assert_eq!(r.slots.len(), 4);
    // The host sets slot 2 to an AI with a civilization.
    alice.lobby(LobbyCmd::Slot {
        index: 2,
        occupant: SlotOccupant::Ai,
        civ: CivPick::Civ("CIVILIZATION_B".into()),
    });
    let r = alice.room("an AI in slot 2", |r| r.slots[2].occupant == Occupant::Ai);
    assert_eq!(r.slots[2].civ, CivPick::Civ("CIVILIZATION_B".into()));
}

#[test]
fn a_named_civilization_is_taken_once() {
    let srv = start("civs");
    let (mut alice, mut bob) = room_of_two(&srv);
    alice.lobby(LobbyCmd::PickCiv {
        civ: CivPick::Civ("CIVILIZATION_A".into()),
    });
    alice.room("picked", |r| {
        r.slots[0].civ == CivPick::Civ("CIVILIZATION_A".into())
    });
    bob.lobby(LobbyCmd::PickCiv {
        civ: CivPick::Civ("CIVILIZATION_A".into()),
    });
    assert!(bob.error().contains("taken"));
    bob.lobby(LobbyCmd::PickCiv {
        civ: CivPick::Random,
    });
    bob.room("random is fine", |_| true);
}

#[test]
fn a_civilization_the_room_does_not_list_is_refused() {
    let srv = start("civlist");
    let (mut alice, _bob) = room_of_two(&srv);
    // The city-state and barbarian rows are no choice; neither is a made-up name.
    for bad in [
        "CIVILIZATION_MINOR",
        "CIVILIZATION_BARBARIAN",
        "CIVILIZATION_NOPE",
    ] {
        alice.lobby(LobbyCmd::PickCiv {
            civ: CivPick::Civ(bad.into()),
        });
        assert!(alice.error().contains("unknown civilization"), "{bad}");
        alice.lobby(LobbyCmd::Slot {
            index: 2,
            occupant: SlotOccupant::Ai,
            civ: CivPick::Civ(bad.into()),
        });
        assert!(alice.error().contains("unknown civilization"), "{bad}");
    }
    alice.lobby(LobbyCmd::PickCiv {
        civ: CivPick::Civ("CIVILIZATION_C".into()),
    });
    alice.room("a listed civilization is taken", |r| {
        r.slots[0].civ == CivPick::Civ("CIVILIZATION_C".into())
    });
}

#[test]
fn a_host_who_leaves_hands_the_room_on_and_the_last_one_closes_it() {
    let srv = start("leave");
    let (mut alice, mut bob) = room_of_two(&srv);
    alice.lobby(LobbyCmd::LeaveRoom);
    alice.state("alice out", |s| s.room.is_none());
    let r = bob.room("bob hosts", |r| r.host == "bob");
    assert!(r.slots.iter().any(|s| s.occupant == Occupant::Open));
    bob.lobby(LobbyCmd::LeaveRoom);
    bob.state("the room is gone", |s| {
        s.room.is_none() && s.rooms.is_empty()
    });
    // Not in a room: refused.
    bob.lobby(LobbyCmd::LeaveRoom);
    assert!(bob.error().contains("not in a room"));
}

#[test]
fn at_most_eight_rooms() {
    let srv = start("eight");
    // One at a time: a first password login hashes (argon2), and a gateway lets 4 unfinished logins per address wait.
    let mut clients: Vec<Client> = (0..9)
        .map(|i| {
            let mut c = Client::login(&srv, &format!("p{i}"));
            c.state("the lobby", |_| true);
            c
        })
        .collect();
    for (i, c) in clients.iter_mut().enumerate().take(8) {
        c.lobby(LobbyCmd::CreateRoom {
            settings: settings(&format!("r{i}")),
        });
        c.room("the room", |_| true);
    }
    let ninth = &mut clients[8];
    ninth.lobby(LobbyCmd::CreateRoom {
        settings: settings("r9"),
    });
    assert!(ninth.error().contains("too many rooms"));
}

#[test]
fn load_save_and_join_game_check_their_arguments() {
    let srv = start("bad-args");
    let mut a = Client::login(&srv, "alice");
    a.state("the lobby", |_| true);
    // Not a save id, a save id of nothing.
    a.lobby(LobbyCmd::LoadSave { save: "x".into() });
    assert!(a.error().contains("no such save"));
    a.lobby(LobbyCmd::LoadSave {
        save: "7-00000001".into(),
    });
    assert!(a.error().contains("no such save"));
    a.lobby(LobbyCmd::JoinGame {
        game: 1,
        player: 0,
        basis: None,
    });
    assert!(a.error().contains("no such game"));
}

// ---------------------------------------------------------------- start

#[test]
fn start_checks_the_conditions_then_both_players_get_their_seats() {
    let srv = start("start");
    let mut alice = Client::login(&srv, "alice");
    alice.state("the lobby", |_| true);
    alice.lobby(LobbyCmd::CreateRoom {
        settings: settings("duel"),
    });
    let room = alice.room("the room", |_| true);
    // An open slot: refused.
    alice.lobby(LobbyCmd::Start);
    assert!(alice.error().contains("open slots"));
    let mut bob = Client::login(&srv, "bob");
    bob.state("the lobby", |s| s.rooms.len() == 1);
    bob.lobby(LobbyCmd::JoinRoom { room: room.id });
    bob.room("in", |r| {
        r.slots.iter().any(|s| is_human(&s.occupant, "bob"))
    });
    // Not the host: refused. Not ready: refused.
    bob.lobby(LobbyCmd::Start);
    assert!(bob.error().contains("host"));
    alice.lobby(LobbyCmd::Start);
    assert!(alice.error().contains("ready"));
    bob.lobby(LobbyCmd::Ready(true));
    alice.room("bob ready", |r| {
        r.slots
            .iter()
            .any(|s| matches!(&s.occupant, Occupant::Human { ready: true, .. }))
    });
    alice.lobby(LobbyCmd::Start);

    // Both are attached to their seats and get the first batch: the room is gone.
    for (c, player) in [(&mut alice, 0), (&mut bob, 1)] {
        let ServerMsg::Attached(a) = c.until("Attached", |m| matches!(m, ServerMsg::Attached(_)))
        else {
            unreachable!()
        };
        assert_eq!((a.player, a.last_cmd_id), (player, 0));
        let ServerMsg::Game(b) = c.until("a batch", |m| matches!(m, ServerMsg::Game(_))) else {
            unreachable!()
        };
        assert_eq!(b.seq, 1);
        assert!(matches!(b.msgs[0], GameMsg::MapBasis(_)));
        assert!(matches!(b.msgs[1], GameMsg::Keyframe(_)));
    }
    assert_eq!(srv.shared.games.running(), 1);

    // The admin routes: the game and its counters (names of spec 4.13).
    let (status, body) = http_get(srv.admin, "/admin/games");
    assert_eq!(status, 200);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["games"][0]["id"], 1);
    assert_eq!(v["games"][0]["status"], "Running");
    assert_eq!(v["games"][0]["seats"].as_array().unwrap().len(), 2);
    let (status, body) = http_get(srv.admin, "/admin/metrics");
    assert_eq!(status, 200);
    let m: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(m["schema"], 1);
    assert_eq!(m["server"]["sessions"]["logged_in"], 2);
    assert_eq!(m["server"]["games"]["running"], 1);
    assert_eq!(m["games"]["1"]["status"], "Running");
    assert_eq!(m["games"]["1"]["seats"]["0"]["nick"], "alice");
    assert_eq!(m["games"]["1"]["seats"]["0"]["conn"], "online");
    assert!(m["games"]["1"]["views"]["keyframes"].as_u64().unwrap() >= 2);
    assert!(
        m["games"]["1"]["seats"]["1"]["tx"]["batches"]
            .as_u64()
            .unwrap()
            >= 1
    );
    for key in [
        "received",
        "forwarded",
        "applied",
        "lost",
        "rejected",
        "dup_answered",
    ] {
        assert!(m["games"]["1"]["cmds"].get(key).is_some(), "cmds.{key}");
    }
}

#[test]
fn a_game_that_fails_sends_the_players_back_to_a_lobby_state() {
    // No save to restart from: the first crash faults the game.
    let srv = start_with("faulted", "crash-after n=1", |_| {});
    let (mut alice, mut bob) = room_of_two(&srv);
    alice.lobby(LobbyCmd::Start);
    alice.until("Attached", |m| matches!(m, ServerMsg::Attached(_)));
    bob.until("Attached", |m| matches!(m, ServerMsg::Attached(_)));
    alice.send(&ClientMsg::Cmd(protocol::Cmd {
        id: 1,
        turn: 1,
        cmd: protocol::Command::CancelEndTurn,
    }));
    // `Detached`, then the lobby again, with no room and no game.
    let ServerMsg::Detached { reason } =
        alice.until("Detached", |m| matches!(m, ServerMsg::Detached { .. }))
    else {
        unreachable!()
    };
    assert!(matches!(reason, DetachReason::GameStopped { .. }));
    let s = alice.state("the lobby again", |s| s.games.is_empty());
    assert!(s.room.is_none());
    // The nick is free: a new room works.
    alice.lobby(LobbyCmd::CreateRoom {
        settings: settings("again"),
    });
    alice.room("a new room", |_| true);
    let (_, body) = http_get(srv.admin, "/admin/metrics");
    let m: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(m["server"]["games"]["faulted"], 1);
}

#[test]
fn games_at_once_are_limited() {
    let srv = start_with("maxgames", "echo", |c| c.max_games = 1);
    let (mut alice, mut bob) = room_of_two(&srv);
    alice.lobby(LobbyCmd::Start);
    alice.until("Attached", |m| matches!(m, ServerMsg::Attached(_)));
    bob.until("Attached", |m| matches!(m, ServerMsg::Attached(_)));
    let mut carol = Client::login(&srv, "carol");
    carol.state("the lobby", |_| true);
    carol.lobby(LobbyCmd::CreateRoom {
        settings: settings("c"),
    });
    let room = carol.room("room", |_| true);
    let mut dave = Client::login(&srv, "dave");
    dave.state("lobby", |s| s.rooms.len() == 1);
    dave.lobby(LobbyCmd::JoinRoom { room: room.id });
    dave.lobby(LobbyCmd::Ready(true));
    carol.room("dave ready", |r| {
        r.slots
            .iter()
            .any(|s| matches!(&s.occupant, Occupant::Human { ready: true, .. }))
    });
    carol.lobby(LobbyCmd::Start);
    assert!(carol.error().contains("too many games"));
}

// ---------------------------------------------------------------- JoinGame, saves, lost members

/// What the admin API will do (N3.36): the same message to the game actor.
/// A game that ends at once (the mock says `GameOver` from turn 2): both players end turn 1, the game saves
/// itself (save 1) and stays open for five quiet minutes; `finish` moves the clock past them, the players
/// are let go and the stop writes save 2. A game that is stopped by an administrator sleeps with its seats
/// instead (N3.35), so the tests of the saves use this way to free the players.
fn ended_game(name: &str) -> (TestServer, Arc<TestClock>, Client, Client) {
    let clock = Arc::new(TestClock::new());
    let c = clock.clone();
    let srv = start_with(name, "echo turns=1 over=2", move |cfg| cfg.clock = c);
    let (mut alice, mut bob) = game_with_ai(&srv);
    for p in [&mut alice, &mut bob] {
        p.send(&ClientMsg::Cmd(protocol::Cmd {
            id: 1,
            turn: 1,
            cmd: protocol::Command::EndTurn { force: false },
        }));
    }
    (srv, clock, alice, bob)
}

/// A game of alice (host), bob and an AI in slot 2; both are attached.
fn game_with_ai(srv: &TestServer) -> (Client, Client) {
    let mut alice = Client::login(srv, "alice");
    alice.state("the lobby", |_| true);
    alice.lobby(LobbyCmd::CreateRoom {
        settings: settings("duel"),
    });
    let room = alice.room("the room", |_| true);
    alice.lobby(LobbyCmd::Slot {
        index: 2,
        occupant: SlotOccupant::Ai,
        civ: CivPick::Civ("CIVILIZATION_C".into()),
    });
    let mut bob = Client::login(srv, "bob");
    bob.state("the lobby", |s| s.rooms.len() == 1);
    bob.lobby(LobbyCmd::JoinRoom { room: room.id });
    bob.room("bob in the room", |r| {
        r.slots.iter().any(|s| is_human(&s.occupant, "bob"))
    });
    bob.lobby(LobbyCmd::Ready(true));
    alice.room("ready and the AI", |r| {
        r.slots[2].occupant == Occupant::Ai
            && r.slots
                .iter()
                .any(|s| matches!(&s.occupant, Occupant::Human { ready: true, .. }))
    });
    alice.lobby(LobbyCmd::Start);
    alice.until("Attached", |m| matches!(m, ServerMsg::Attached(_)));
    bob.until("Attached", |m| matches!(m, ServerMsg::Attached(_)));
    (alice, bob)
}

/// The `Seats` of a batch, if it has them.
fn seats_of(m: &ServerMsg) -> Option<Vec<SeatView>> {
    let ServerMsg::Game(b) = m else { return None };
    b.msgs.iter().find_map(|g| match g {
        GameMsg::Seats(v) => Some(v.clone()),
        _ => None,
    })
}

fn is_seat_of(v: &[SeatView], player: i8, nick: &str) -> bool {
    v.iter().any(|s| {
        s.player == player && matches!(&s.control, SeatControl::Human { nick: n } if n == nick)
    })
}

#[test]
fn join_game_takes_an_ai_slot_of_a_running_game() {
    let srv = start("joingame");
    let (mut alice, _bob) = game_with_ai(&srv);
    let mut carol = Client::login(&srv, "carol");
    // The list shows the AI slot as takeable for her, the human seats not.
    let s = carol.state("the game with a takeable slot", |s| {
        s.games
            .first()
            .is_some_and(|g| g.status == GameStatus::Running && g.seats.iter().any(|x| x.takeable))
    });
    let g = &s.games[0];
    let takeable: Vec<i8> = g
        .seats
        .iter()
        .filter(|x| x.takeable)
        .map(|x| x.player)
        .collect();
    assert_eq!(takeable, [2]);
    carol.lobby(LobbyCmd::JoinGame {
        game: g.id,
        player: 2,
        basis: None,
    });
    let ServerMsg::Attached(a) = carol.until("Attached", |m| matches!(m, ServerMsg::Attached(_)))
    else {
        unreachable!()
    };
    assert_eq!(
        (a.game, a.player, a.mode),
        (g.id, 2, protocol::AttachMode::Fresh)
    );
    let ServerMsg::Game(b) = carol.until("a batch", |m| matches!(m, ServerMsg::Game(_))) else {
        unreachable!()
    };
    assert_eq!(b.seq, 1);
    assert!(matches!(b.msgs[0], GameMsg::MapBasis(_)));
    assert!(matches!(b.msgs[1], GameMsg::Keyframe(_)));
    // The others see the new human in their seats.
    let seats = alice.until("Seats with carol", |m| {
        seats_of(m).is_some_and(|v| is_seat_of(&v, 2, "carol"))
    });
    assert!(seats_of(&seats).is_some());
    // And the lobby shows the slot as taken, takeable for nobody.
    let mut dave = Client::login(&srv, "dave");
    let s = dave.state("the slot taken", |s| {
        s.games.first().is_some_and(|g| {
            g.seats.iter().any(|x| {
                x.player == 2
                    && matches!(&x.control, SeatControl::Human { nick } if nick == "carol")
            })
        })
    });
    assert!(s.games[0].seats.iter().all(|x| !x.takeable));
    // Dave tries anyway: the slot is not an AI's any more.
    dave.lobby(LobbyCmd::JoinGame {
        game: s.games[0].id,
        player: 2,
        basis: None,
    });
    assert!(dave.error().contains("not an AI"));
    // Carol has a seat now: she cannot take another, nor a room.
    carol.lobby(LobbyCmd::JoinGame {
        game: s.games[0].id,
        player: 3,
        basis: None,
    });
    assert!(carol.error().contains("have a seat"));
}

#[test]
fn two_join_game_for_one_slot_the_first_wins() {
    let srv = start("race");
    let (_alice, _bob) = game_with_ai(&srv);
    let mut carol = Client::login(&srv, "carol");
    let mut dave = Client::login(&srv, "dave");
    let s = carol.state("takeable", |s| {
        s.games
            .first()
            .is_some_and(|g| g.seats.iter().any(|x| x.takeable))
    });
    dave.state("takeable", |s| {
        s.games
            .first()
            .is_some_and(|g| g.seats.iter().any(|x| x.takeable))
    });
    let game = s.games[0].id;
    for c in [&mut carol, &mut dave] {
        c.lobby(LobbyCmd::JoinGame {
            game,
            player: 2,
            basis: None,
        });
    }
    // One gets `Attached`, the other a `LobbyError`.
    let mut outcome = Vec::new();
    for c in [&mut carol, &mut dave] {
        let m = c.until("Attached or LobbyError", |m| {
            matches!(m, ServerMsg::Attached(_) | ServerMsg::LobbyError { .. })
        });
        outcome.push(matches!(m, ServerMsg::Attached(_)));
    }
    outcome.sort_unstable();
    assert_eq!(outcome, [false, true]);
    assert_eq!(
        srv.shared
            .games
            .get(game)
            .unwrap()
            .info
            .snapshot()
            .seats
            .iter()
            .filter(|s| matches!(s.control, SeatControl::Human { .. }))
            .count(),
        3
    );
}

#[test]
fn a_refused_claim_tells_the_player_and_frees_the_slot() {
    let srv = start_with("refused", "echo claim=refuse", |_| {});
    let (_alice, _bob) = game_with_ai(&srv);
    let mut carol = Client::login(&srv, "carol");
    let s = carol.state("takeable", |s| {
        s.games
            .first()
            .is_some_and(|g| g.seats.iter().any(|x| x.takeable))
    });
    carol.lobby(LobbyCmd::JoinGame {
        game: s.games[0].id,
        player: 2,
        basis: None,
    });
    assert!(carol.error().contains("core refuses"));
    // She is still free and the slot is takeable again.
    carol.state("takeable again", |s| {
        s.games
            .first()
            .is_some_and(|g| g.seats.iter().any(|x| x.takeable))
    });
    carol.lobby(LobbyCmd::JoinGame {
        game: s.games[0].id,
        player: 2,
        basis: None,
    });
    assert!(carol.error().contains("core refuses"));
}

#[test]
fn a_claim_the_worker_never_answers_fails_after_twenty_seconds() {
    let clock = Arc::new(TestClock::new());
    let c = clock.clone();
    let srv = start_with("claim-timeout", "echo claim=ignore", move |cfg| {
        cfg.clock = c
    });
    let (_alice, _bob) = game_with_ai(&srv);
    let mut carol = Client::login(&srv, "carol");
    let s = carol.state("takeable", |s| {
        s.games
            .first()
            .is_some_and(|g| g.seats.iter().any(|x| x.takeable))
    });
    carol.lobby(LobbyCmd::JoinGame {
        game: s.games[0].id,
        player: 2,
        basis: None,
    });
    // While it waits, a second request is turned away.
    let mut dave = Client::login(&srv, "dave");
    dave.state("the lobby", |_| true);
    dave.lobby(LobbyCmd::JoinGame {
        game: s.games[0].id,
        player: 2,
        basis: None,
    });
    assert!(dave.error().contains("taking that slot") || dave.error().contains("not takeable"));
    clock.advance(19_000);
    std::thread::sleep(Duration::from_millis(400));
    clock.advance(2_000);
    assert!(carol.error().contains("did not answer"));
}

#[test]
fn a_save_is_listed_and_a_room_grows_from_it_with_reserved_slots() {
    let (srv, clock, mut alice, mut bob) = ended_game("loadsave");
    // The end of the game wrote a save; the list shows it.
    let mut carol = Client::login(&srv, "carol");
    let s = carol.state("a save in the list", |s| !s.saves.is_empty());
    let save = &s.saves[0];
    assert_eq!(save.id, "1-00000001");
    assert_eq!(save.name, "duel");
    assert_eq!((save.game, save.host.as_str()), (1, "alice"));
    assert_eq!(
        save.seats
            .iter()
            .map(|(p, _, n)| (*p, n.clone()))
            .collect::<Vec<_>>(),
        [
            (0, Some("alice".to_owned())),
            (1, Some("bob".to_owned())),
            (2, None)
        ]
    );
    // Five quiet minutes later the game closes; alice and bob are free again.
    clock.advance(5 * 60 * 1000 + 1);
    alice.until("Detached", |m| matches!(m, ServerMsg::Detached { .. }));
    bob.until("Detached", |m| matches!(m, ServerMsg::Detached { .. }));
    alice.state("alice in the lobby", |s| {
        s.room.is_none() && s.games.is_empty()
    });
    bob.state("bob in the lobby", |s| s.room.is_none());
    // The stop wrote a newer save; per game the index keeps the newest of `ring/` only.
    let s = carol.state("the save of the stop", |s| {
        s.saves.first().is_some_and(|x| x.id == "1-00000002")
    });
    assert_eq!(s.saves.len(), 1);
    let save = &s.saves[0];

    // Carol loads it: the room has the save's settings; the humans' slots are reserved for them,
    // carol (nobody's slot is hers) sits in the AI slot.
    carol.lobby(LobbyCmd::LoadSave {
        save: save.id.clone(),
    });
    let room = carol.room("the room from the save", |r| r.from_save.is_some());
    assert_eq!(room.from_save.as_deref(), Some("1-00000002"));
    assert_eq!(room.settings.name, "duel");
    assert_eq!(room.slots.len(), 4);
    assert_eq!(room.slots[0].occupant, Occupant::Open);
    assert_eq!(room.slots[0].reserved_for.as_deref(), Some("alice"));
    assert_eq!(room.slots[1].reserved_for.as_deref(), Some("bob"));
    assert!(is_human(&room.slots[2].occupant, "carol"));
    assert_eq!(room.slots[3].occupant, Occupant::Closed);
    // The save's settings and civilizations are the save's; its closed slot stays closed.
    carol.lobby(LobbyCmd::Settings(settings("other")));
    assert!(carol.error().contains("saved game"));
    carol.lobby(LobbyCmd::PickCiv {
        civ: CivPick::Random,
    });
    assert!(carol.error().contains("fixed"));
    carol.lobby(LobbyCmd::Slot {
        index: 3,
        occupant: SlotOccupant::Open,
        civ: CivPick::Random,
    });
    assert!(carol.error().contains("no player"));
    carol.lobby(LobbyCmd::Slot {
        index: 0,
        occupant: SlotOccupant::Closed,
        civ: CivPick::Random,
    });
    assert!(carol.error().contains("cannot be closed"));
    // A stranger cannot take a reserved slot; the owners can, and only theirs.
    let mut dave = Client::login(&srv, "dave");
    let rooms = dave.state("the room", |s| s.rooms.len() == 1);
    dave.lobby(LobbyCmd::JoinRoom {
        room: rooms.rooms[0].id,
    });
    assert!(dave.error().contains("full"));
    bob.lobby(LobbyCmd::JoinRoom {
        room: rooms.rooms[0].id,
    });
    bob.room("bob in slot 1", |r| is_human(&r.slots[1].occupant, "bob"));
    alice.lobby(LobbyCmd::JoinRoom {
        room: rooms.rooms[0].id,
    });
    alice.room("alice in slot 0", |r| {
        is_human(&r.slots[0].occupant, "alice")
    });
    bob.lobby(LobbyCmd::Ready(true));
    alice.lobby(LobbyCmd::Ready(true));
    carol.room("all ready", |r| {
        r.slots
            .iter()
            .filter(|s| matches!(&s.occupant, Occupant::Human { ready: true, .. }))
            .count()
            == 2
    });
    carol.lobby(LobbyCmd::Start);
    // The game from the save: everybody is seated where the save had them (carol where the AI sat).
    for (c, player) in [(&mut alice, 0), (&mut bob, 1), (&mut carol, 2)] {
        let ServerMsg::Attached(a) = c.until("Attached", |m| matches!(m, ServerMsg::Attached(_)))
        else {
            unreachable!()
        };
        assert_eq!(a.player, player, "{}", c.nick);
        assert_ne!(a.game, 1, "a new game number");
    }
}

#[test]
fn the_host_can_drop_a_reservation() {
    let (srv, clock, mut alice, mut bob) = ended_game("unreserve");
    let mut carol = Client::login(&srv, "carol");
    carol.state("a save", |s| !s.saves.is_empty());
    clock.advance(5 * 60 * 1000 + 1);
    alice.until("Detached", |m| matches!(m, ServerMsg::Detached { .. }));
    bob.until("Detached", |m| matches!(m, ServerMsg::Detached { .. }));
    let s = carol.state("the save of the stop", |s| {
        s.saves.first().is_some_and(|x| x.id == "1-00000002")
    });
    carol.lobby(LobbyCmd::LoadSave {
        save: s.saves[0].id.clone(),
    });
    carol.room("the room", |r| r.from_save.is_some());
    // Bob's slot goes to the AI: his reservation is gone, he cannot come.
    carol.lobby(LobbyCmd::Slot {
        index: 1,
        occupant: SlotOccupant::Ai,
        civ: CivPick::Random,
    });
    let r = carol.room("slot 1 is AI", |r| r.slots[1].occupant == Occupant::Ai);
    assert_eq!(r.slots[1].reserved_for, None);
    bob.state("lobby", |_| true);
    let id = bob.state("the room", |s| s.rooms.len() == 1).rooms[0].id;
    bob.lobby(LobbyCmd::JoinRoom { room: id });
    assert!(bob.error().contains("full"));
}

#[test]
fn own_seat_can_be_taken_again_with_join_game() {
    let srv = start("own-seat");
    let (mut alice, _bob) = game_with_ai(&srv);
    // `JoinGame` on the own seat attaches the connection again (a stream of its own).
    alice.lobby(LobbyCmd::JoinGame {
        game: 1,
        player: 0,
        basis: None,
    });
    let ServerMsg::Attached(a) =
        alice.until("a new Attached", |m| matches!(m, ServerMsg::Attached(_)))
    else {
        unreachable!()
    };
    assert_eq!((a.player, a.mode), (0, protocol::AttachMode::Fresh));
    // Somebody else's seat is not hers to take.
    alice.lobby(LobbyCmd::JoinGame {
        game: 1,
        player: 1,
        basis: None,
    });
    assert!(alice.error().contains("have a seat"));
}

/// Waits for the lobby actor's sweep (every 250 ms) of the clock.
fn sweep() {
    std::thread::sleep(Duration::from_millis(700));
}

#[test]
fn a_room_member_without_a_session_leaves_after_sixty_seconds() {
    let clock = Arc::new(TestClock::new());
    let c = clock.clone();
    let srv = start_with("room-lost", "echo", move |cfg| cfg.clock = c);
    let (mut alice, bob) = room_of_two(&srv);
    drop(bob);
    // The session has to end (real time) before the clock moves.
    sweep();
    clock.advance(59_000);
    sweep();
    // Still there: any change of the room shows it.
    alice.lobby(LobbyCmd::Ready(true));
    alice.room("bob is still in", |r| {
        r.slots.iter().any(|s| is_human(&s.occupant, "bob"))
    });
    clock.advance(2_000);
    sweep();
    alice.lobby(LobbyCmd::Ready(false));
    let r = alice.room("bob is gone", |r| {
        !r.slots.iter().any(|s| is_human(&s.occupant, "bob"))
    });
    assert_eq!(r.slots[1].occupant, Occupant::Open);
    assert_eq!(r.host, "alice");
}

#[test]
fn a_member_who_comes_back_in_time_stays_in_the_room() {
    let clock = Arc::new(TestClock::new());
    let c = clock.clone();
    let srv = start_with("room-back", "echo", move |cfg| cfg.clock = c);
    let (mut alice, bob) = room_of_two(&srv);
    let token = bob.token;
    drop(bob);
    sweep();
    clock.advance(30_000);
    sweep();
    let mut bob = Client::connect(&srv, "bob", "", token);
    bob.room("bob is back in the room", |r| {
        r.slots.iter().any(|s| is_human(&s.occupant, "bob"))
    });
    clock.advance(40_000);
    sweep();
    alice.lobby(LobbyCmd::Ready(true));
    alice.room("bob is still in", |r| {
        r.slots.iter().any(|s| is_human(&s.occupant, "bob"))
    });
}

#[test]
fn a_host_without_a_session_hands_the_room_to_the_next_human_after_sixty_seconds() {
    let clock = Arc::new(TestClock::new());
    let c = clock.clone();
    let srv = start_with("host-lost", "echo", move |cfg| cfg.clock = c);
    let (alice, mut bob) = room_of_two(&srv);
    drop(alice);
    sweep();
    clock.advance(61_000);
    sweep();
    let r = bob.room("bob hosts", |r| r.host == "bob");
    assert!(!r.slots.iter().any(|s| is_human(&s.occupant, "alice")));
    assert_eq!(r.slots[0].occupant, Occupant::Open);
}

// ---------------------------------------------------------------- the online list

fn place_of(s: &LobbyState, nick: &str) -> Option<PlayerPlace> {
    s.online
        .iter()
        .find(|p| p.nick == nick)
        .map(|p| p.place.clone())
}

#[test]
fn the_online_list_shows_lobby_room_and_drops_a_disconnected_nick() {
    let srv = start("online-room");
    let mut carol = Client::login(&srv, "Carol");
    carol.state("carol alone", |s| s.online.len() == 1);
    let (mut alice, bob) = room_of_two(&srv);
    // Sorted by nick without regard to case; the room members carry host and ready.
    let s = carol.state("all three, bob ready", |s| {
        matches!(
            place_of(s, "bob"),
            Some(PlayerPlace::Room { ready: true, .. })
        )
    });
    let nicks: Vec<_> = s.online.iter().map(|p| p.nick.as_str()).collect();
    assert_eq!(nicks, ["alice", "bob", "Carol"]);
    assert!(matches!(
        place_of(&s, "alice"),
        Some(PlayerPlace::Room {
            host: true,
            ready: false,
            ref name,
            ..
        }) if name == "duel"
    ));
    assert!(matches!(
        place_of(&s, "bob"),
        Some(PlayerPlace::Room { host: false, .. })
    ));
    assert_eq!(place_of(&s, "Carol"), Some(PlayerPlace::Lobby));
    // A dropped connection is gone from the list for the others.
    drop(bob);
    let s = carol.state("bob gone", |s| place_of(s, "bob").is_none());
    assert_eq!(s.online.len(), 2);
    alice.state("bob gone for alice too", |s| place_of(s, "bob").is_none());
}

#[test]
fn the_online_list_shows_the_game_of_a_player_in_a_running_game() {
    let srv = start("online-game");
    let mut carol = Client::login(&srv, "carol");
    carol.state("the lobby", |_| true);
    let (_alice, _bob) = game_with_ai(&srv);
    let s = carol.state("alice and bob in a game", |s| {
        matches!(place_of(s, "alice"), Some(PlayerPlace::Game { .. }))
            && matches!(place_of(s, "bob"), Some(PlayerPlace::Game { .. }))
    });
    assert!(matches!(
        place_of(&s, "alice"),
        Some(PlayerPlace::Game { game: 1, ref name, .. }) if name == "duel"
    ));
    assert_eq!(place_of(&s, "carol"), Some(PlayerPlace::Lobby));
}

// ---------------------------------------------------------------- the account store (v12)

/// The client's way to load the store: an empty change.
fn ask_account(c: &mut Client) -> protocol::AccountData {
    c.send(&ClientMsg::Account(protocol::AccountSave::default()));
    match c.until("Account", |m| matches!(m, ServerMsg::Account(_))) {
        ServerMsg::Account(a) => a,
        _ => unreachable!(),
    }
}

/// A round trip on the connection: everything sent before it has been taken by the session.
fn sync(c: &mut Client) {
    c.send(&ClientMsg::Ping {
        seq: 77,
        client_ms: 1,
    });
    c.until("Pong", |m| matches!(m, ServerMsg::Pong { seq: 77, .. }));
}

#[test]
fn the_account_store_comes_at_login_is_saved_and_survives_a_new_login() {
    let srv = start("account");
    let mut alice = Client::login(&srv, "alice");
    let first = ask_account(&mut alice);
    assert!(first.prefs.is_empty() && first.last_room.is_none());
    assert_eq!(first.version, protocol::ACCOUNT_VERSION);
    alice.send(&ClientMsg::Account(protocol::AccountSave {
        set: vec![
            ("GridOn".into(), "0".into()),
            ("Volume.0".into(), "0.5".into()),
        ],
        remove: vec![],
    }));
    // A broken change (a key with a space) is dropped whole and does not close the session.
    alice.send(&ClientMsg::Account(protocol::AccountSave {
        set: vec![
            ("YieldOn".into(), "1".into()),
            ("bad key".into(), "1".into()),
        ],
        remove: vec![],
    }));
    sync(&mut alice);
    alice.send(&ClientMsg::Account(protocol::AccountSave {
        set: vec![("ResourceOn".into(), "1".into())],
        remove: vec!["GridOn".into()],
    }));
    sync(&mut alice);

    // A login of the same nick (any case) on another socket gets the store.
    let mut again = Client::login(&srv, "alice");
    let a = ask_account(&mut again);
    assert_eq!(
        a.prefs,
        [
            ("ResourceOn".to_string(), "1".to_string()),
            ("Volume.0".to_string(), "0.5".to_string()),
        ]
    );
    // Another nick has its own.
    let mut bob = Client::login(&srv, "bob");
    assert!(ask_account(&mut bob).prefs.is_empty());
}

#[test]
fn the_settings_of_a_started_room_are_the_hosts_last_room() {
    let srv = start("account-room");
    let (mut alice, mut bob) = room_of_two(&srv);
    assert!(
        srv.shared.prefs.get("alice").last_room.is_none(),
        "nothing before the start"
    );
    alice.lobby(LobbyCmd::Start);
    // The host is told (before or after `Attached`), the guest is not; the seed of that one game is not kept.
    let mut account = None;
    let mut attached = false;
    for _ in 0..200 {
        match alice.recv() {
            ServerMsg::Account(a) => account = Some(a),
            ServerMsg::Attached(_) => attached = true,
            _ => {}
        }
        if account.is_some() && attached {
            break;
        }
    }
    let a = account.expect("the host gets the store with the last room");
    let room = a.last_room.expect("a last room");
    assert_eq!(room.name, "duel");
    assert_eq!(room.seed, None);
    assert_eq!(srv.shared.prefs.get("ALICE").last_room, Some(room));
    assert!(srv.shared.prefs.get("bob").last_room.is_none());
    bob.until("Attached", |m| matches!(m, ServerMsg::Attached(_)));
}

/// Installs mods under a scratch directory: (id, extra manifest lines). All are 1.0.0, API ^1, with an assets part.
fn install_mods(name: &str, mods: &[(&str, &str, &str)]) -> std::path::PathBuf {
    let root = temp_dir(name);
    for (id, sync, extra) in mods {
        std::fs::create_dir_all(root.join(id)).unwrap();
        std::fs::write(
            root.join(id).join("mod.toml"),
            format!(
                "[mod]\nid=\"{id}\"\nname=\"{id} title\"\nversion=\"1.0.0\"\napi=\"^1\"\nlicense=\"MIT\"\nsync=\"{sync}\"\nparts=[\"assets\"]\n{extra}\n"
            ),
        )
        .unwrap();
    }
    root
}

fn pick(id: &str, req: &str) -> protocol::ModPick {
    protocol::ModPick {
        id: id.into(),
        req: req.into(),
    }
}

fn plan_ids(p: &protocol::ModPlan) -> Vec<(String, bool)> {
    match p {
        protocol::ModPlan::Ok(v) => v
            .iter()
            .map(|m| (m.id.clone(), m.client_required))
            .collect(),
        protocol::ModPlan::Err(e) => panic!("expected a plan, got problems: {e:?}"),
    }
}

fn plan_problems(p: &protocol::ModPlan) -> String {
    match p {
        protocol::ModPlan::Err(e) => e
            .iter()
            .map(|p| p.text.clone())
            .collect::<Vec<_>>()
            .join("\n"),
        protocol::ModPlan::Ok(v) => panic!("expected problems, got a plan: {v:?}"),
    }
}

/// T43: the installed mods come with the login, the host's picks are resolved into `RoomState.mod_plan` (dependencies
/// added, the optional-for-players mods flagged, the pack hash included) and every change of picks refreshes it.
#[test]
fn the_room_plan_follows_the_hosts_picks_and_adds_dependencies() {
    let mods = install_mods(
        "lobby-mods-plan",
        &[
            ("base", "gameplay", ""),
            (
                "extra",
                "cosmetic",
                "client=\"optional\"\n[deps]\nbase=\"^1\"",
            ),
        ],
    );
    // A pack next to `extra`: its hash reaches the plan.
    let pack = mod_assets::Pack::build(
        "extra".parse().unwrap(),
        vec![],
        vec![mod_assets::NewFile {
            id: "data/x".into(),
            kind: mod_assets::AssetKind::Raw,
            bytes: b"x".to_vec(),
        }],
    )
    .unwrap();
    std::fs::write(mods.join("extra/extra.pack"), pack.to_bytes()).unwrap();
    let dir = mods.clone();
    let srv = start_with("lobby-mods-plan-srv", "echo", move |c| {
        c.mods_dir = Some(dir);
    });
    let mut a = Client::login(&srv, "alice");
    let first = a.until("the choices", |m| matches!(m, ServerMsg::Choices(_)));
    let ServerMsg::Choices(choices) = first else {
        unreachable!()
    };
    let listed: Vec<_> = choices.mods.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(listed, ["base", "extra"]);
    assert!(!choices.mods[1].client_required);
    a.state("the lobby", |_| true);

    // A room without picks: the empty plan.
    a.lobby(LobbyCmd::CreateRoom {
        settings: settings("plain"),
    });
    let room = a.room("the room", |_| true);
    assert_eq!(room.mod_plan, protocol::ModPlan::default());

    // `extra` alone pulls `base` in, in load order; only `extra` is optional for players.
    let mut s = room.settings.clone();
    s.mods = vec![pick("extra", "^1")];
    a.lobby(LobbyCmd::Settings(s.clone()));
    let room = a.room("the plan", |r| r.mod_plan != protocol::ModPlan::default());
    assert_eq!(
        plan_ids(&room.mod_plan),
        [("base".to_string(), true), ("extra".to_string(), false)]
    );
    let protocol::ModPlan::Ok(views) = &room.mod_plan else {
        unreachable!()
    };
    assert_eq!(views[0].version, "1.0.0");
    assert_eq!(views[0].pack, None);
    assert_eq!(
        views[1].pack.as_deref(),
        Some(pack.hash().to_string().as_str())
    );
    assert_eq!(room.settings.mods, s.mods, "the picks stay the host's");

    // A guest sees the same plan.
    let mut b = Client::login(&srv, "bob");
    b.state("the lobby", |s| s.rooms.len() == 1);
    b.lobby(LobbyCmd::JoinRoom { room: room.id });
    let seen = b.room("in", |r| {
        r.slots.iter().any(|s| is_human(&s.occupant, "bob"))
    });
    assert_eq!(seen.mod_plan, room.mod_plan);

    // Dropping the picks gives the empty plan back.
    s.mods = vec![];
    a.lobby(LobbyCmd::Settings(s));
    let room = a.room("empty again", |r| {
        r.mod_plan == protocol::ModPlan::default()
    });
    assert!(room.settings.mods.is_empty());
    let _ = std::fs::remove_dir_all(mods);
}

/// T43: a plan with problems is shown with the dependency chain and the start is refused until the picks are fixed.
#[test]
fn a_room_with_unresolvable_picks_shows_the_chain_and_cannot_start() {
    let mods = install_mods(
        "lobby-mods-err",
        &[
            ("base", "gameplay", ""),
            ("needy", "gameplay", "[deps]\nbase=\"^2\""),
        ],
    );
    let dir = mods.clone();
    let srv = start_with("lobby-mods-err-srv", "echo", move |c| {
        c.mods_dir = Some(dir);
    });
    let mut a = Client::login(&srv, "alice");
    a.state("the lobby", |_| true);
    // Picks are accepted into the room even when they cannot be resolved: the host sees why.
    let mut s = settings("broken");
    s.mods = vec![pick("ghost", "*")];
    a.lobby(LobbyCmd::CreateRoom { settings: s });
    let room = a.room("the room", |_| true);
    assert!(plan_problems(&room.mod_plan).contains("ghost"));
    a.lobby(LobbyCmd::Start);
    let e = a.error();
    assert!(
        e.contains("mods cannot be loaded") && e.contains("ghost"),
        "{e}"
    );

    // The chain names who wanted what.
    let mut s = room.settings.clone();
    s.mods = vec![pick("needy", "*")];
    a.lobby(LobbyCmd::Settings(s.clone()));
    let room = a.room("the chain", |r| {
        matches!(&r.mod_plan, protocol::ModPlan::Err(e) if e.iter().any(|p| p.text.contains("needy")))
    });
    let text = plan_problems(&room.mod_plan);
    assert!(text.contains("base"), "{text}");
    a.lobby(LobbyCmd::Start);
    assert!(a.error().contains("mods cannot be loaded"));

    // A bad pick (not a version requirement) is a problem as well, and too many picks are refused outright.
    s.mods = vec![pick("base", "not a version")];
    a.lobby(LobbyCmd::Settings(s.clone()));
    a.room("bad req", |r| {
        matches!(r.mod_plan, protocol::ModPlan::Err(_))
    });
    s.mods = (0..=protocol::MAX_MODS)
        .map(|i| pick(&format!("m{i}"), "*"))
        .collect();
    a.lobby(LobbyCmd::Settings(s.clone()));
    assert!(a.error().contains("at most"));

    // Fixed picks: the plan is a plan again and the start gets past the mods (it then fails on the open slot).
    s.mods = vec![pick("base", "^1")];
    a.lobby(LobbyCmd::Settings(s));
    let room = a.room("fixed", |r| matches!(r.mod_plan, protocol::ModPlan::Ok(_)));
    assert_eq!(plan_ids(&room.mod_plan), [("base".to_string(), true)]);
    a.lobby(LobbyCmd::Start);
    assert!(a.error().contains("open slots"));
    let _ = std::fs::remove_dir_all(mods);
}
