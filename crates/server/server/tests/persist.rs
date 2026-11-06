//! The state on disk and the way back (9 `persist`) with the mock worker: a game comes back
//! after a gateway restart (seats, host, numbers, saves), sleeps when nobody is online and wakes for its
//! player or for a `JoinGame`, an administrator's stop keeps the seats, `max_games` leaves a game asleep,
//! a save that does not check out is not offered, a game with no save is let go.

#![allow(dead_code, unused_imports)]

mod common;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{mock_server_config, temp_dir};
use protocol::{
    AutoChoices, ClientMsg, Cmd, Codec, Command, CommandOutcome, Conn, DetachReason, GameCtl,
    GameCtlReason, GameMsg, GameSettings, GameStatus, LobbyCmd, Login, Occupant, PROTOCOL_VERSION,
    Postcard, SeatControl, SeatView, ServerMsg, TimerMode, Token, TurnMode, TurnState,
};
use server::clock::TestClock;
use server::game::{ActorMsg, AdminCtl};
use server::identity::Secret;
use server::session::limits::LimitsConfig;
use server::{ServerConfig, Shared};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};
use wire::{Encoder, Limits};

const PASSWORD: &str = "persist-password";
const ADMIN: &str = "admin-secret";

type Ws = WebSocket<MaybeTlsStream<TcpStream>>;

struct Client {
    ws: Ws,
    nick: String,
    token: Option<Token>,
    /// The last `TurnState` of the stream.
    turn: Option<TurnState>,
    /// The last `Seats` of the stream.
    seats: Option<Vec<SeatView>>,
    next_id: u32,
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
            turn: None,
            seats: None,
            next_id: 1,
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

    fn recv(&mut self) -> ServerMsg {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            assert!(
                Instant::now() < deadline,
                "{}: nothing came within 15 s",
                self.nick
            );
            match self.ws.read().unwrap() {
                Message::Binary(b) => {
                    let msg: ServerMsg = wire::decode(&b, &Limits::CLIENT).unwrap();
                    match &msg {
                        ServerMsg::LoggedIn(s) => self.token = s.token,
                        ServerMsg::Game(b) => {
                            for m in &b.msgs {
                                match m {
                                    GameMsg::TurnState(t) => self.turn = Some(t.clone()),
                                    GameMsg::Seats(v) => self.seats = Some(v.clone()),
                                    _ => {}
                                }
                            }
                        }
                        _ => {}
                    }
                    return msg;
                }
                Message::Close(_) => panic!("the socket closed"),
                _ => {}
            }
        }
    }

    fn until(&mut self, what: &str, mut pred: impl FnMut(&ServerMsg) -> bool) -> ServerMsg {
        for _ in 0..400 {
            let m = self.recv();
            if pred(&m) {
                return m;
            }
        }
        panic!("{}: no {what}", self.nick);
    }

    /// The first `TurnState` (of the stream from now on) that satisfies `pred`.
    fn until_turn(&mut self, what: &str, pred: impl Fn(&TurnState) -> bool) -> TurnState {
        // What the stream said last counts: the state may have come while something else was awaited.
        if let Some(t) = &self.turn
            && pred(t)
        {
            return t.clone();
        }
        for _ in 0..400 {
            let m = self.recv();
            if let ServerMsg::Game(b) = m {
                for g in b.msgs {
                    if let GameMsg::TurnState(t) = g
                        && pred(&t)
                    {
                        return t;
                    }
                }
            }
        }
        panic!(
            "{}: no TurnState with {what}; last {:?}",
            self.nick, self.turn
        );
    }

    /// The first `Seats` that satisfies `pred`; what the stream said last counts too.
    fn until_seats(&mut self, what: &str, pred: impl Fn(&[SeatView]) -> bool) -> Vec<SeatView> {
        if let Some(v) = &self.seats
            && pred(v)
        {
            return v.clone();
        }
        for _ in 0..400 {
            if let ServerMsg::Game(b) = self.recv() {
                for g in b.msgs {
                    if let GameMsg::Seats(v) = g
                        && pred(&v)
                    {
                        return v;
                    }
                }
            }
        }
        panic!("{}: no Seats with {what}; last {:?}", self.nick, self.seats);
    }

    /// The first journal line that contains `text`.
    fn until_journal(&mut self, text: &str) -> String {
        for _ in 0..400 {
            if let ServerMsg::Game(b) = self.recv() {
                for g in b.msgs {
                    if let GameMsg::Journal(entries) = g
                        && let Some(e) = entries.iter().find(|e| e.text.contains(text))
                    {
                        return e.text.clone();
                    }
                }
            }
        }
        panic!("{}: no journal line with `{text}`", self.nick);
    }

    fn order(&mut self, turn: i32, cmd: Command) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&ClientMsg::Cmd(Cmd { id, turn, cmd }));
        id
    }

    fn ctl(&mut self, ctl: GameCtl) {
        self.send(&ClientMsg::GameCtl(ctl));
    }

    /// The final answer to order `id`.
    fn answer(&mut self, id: u32) -> CommandOutcome {
        loop {
            if let ServerMsg::Game(b) = self.recv() {
                for g in b.msgs {
                    if let GameMsg::Results(rs) = g
                        && let Some(r) = rs
                            .iter()
                            .find(|r| r.id == id && r.outcome != CommandOutcome::Queued)
                    {
                        return r.outcome.clone();
                    }
                }
            }
        }
    }

    fn turn_no(&self) -> i32 {
        self.turn.as_ref().map_or(0, |t| t.turn)
    }
}

fn settings(timer: TimerMode) -> GameSettings {
    GameSettings {
        name: "turns".into(),
        world_size: "WORLDSIZE_DUEL".into(),
        map_script: "m".into(),
        map_options: vec![],
        speed: "s".into(),
        start_era: "e".into(),
        handicap: "h".into(),
        minor_civs: 0,
        turn_mode: TurnMode::Hybrid,
        timer,
        game_options: vec![],
        seed: Some(1),
        map_seed: None,
        auto_choices: AutoChoices::default(),
        diplo_auto_decline: true,
        mods: Vec::new(),
    }
}

// ---------------------------------------------------------------- servers on one state_dir

struct TestServer {
    addr: SocketAddr,
    dir: PathBuf,
    clock: Arc<TestClock>,
    shared: Arc<Shared>,
    rt: Option<tokio::runtime::Runtime>,
}

impl TestServer {
    /// The gateway stops (its workers get EOF); the state stays.
    fn stop(mut self) -> PathBuf {
        if let Some(rt) = self.rt.take() {
            rt.shutdown_timeout(Duration::from_secs(2));
        }
        self.dir.clone()
    }
}

/// A gateway on `dir` with the mock worker in `mode`; what was on the disk comes back.
fn boot(dir: &Path, mode: &str, edit: impl FnOnce(&mut ServerConfig)) -> TestServer {
    let clock = Arc::new(TestClock::new());
    let mut config = mock_server_config(
        dir,
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
    config.clock = clock.clone();
    edit(&mut config);
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
    rt.spawn(server::serve(listener, shared.clone()));
    TestServer {
        addr,
        dir: dir.to_path_buf(),
        clock,
        shared,
        rt: Some(rt),
    }
}

fn admin_game(srv: &TestServer, game: u32, ctl: AdminCtl) {
    assert!(
        srv.shared
            .games
            .get(game)
            .unwrap()
            .send(ActorMsg::Admin(ctl))
    );
}

/// Waits (real time) until a save file of the game is on disk.
fn wait_save(dir: &Path, game: u32) {
    let ring = dir.join("games").join(game.to_string()).join("ring");
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::fs::read_dir(&ring).map_or(0, Iterator::count) == 0 {
        assert!(Instant::now() < deadline, "no save of game {game}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// `host` and `guest` in a game, slot 2 an AI when `ai`; both attached with a first state.
fn make_game(srv: &TestServer, host: &str, guest: &str, ai: bool) -> (Client, Client) {
    let mut h = Client::login(srv, host);
    h.until("the lobby", |m| matches!(m, ServerMsg::LobbyState(_)));
    h.send(&ClientMsg::Lobby(LobbyCmd::CreateRoom {
        settings: settings(TimerMode::Off),
    }));
    let ServerMsg::LobbyState(s) = h.until(
        "the room",
        |m| matches!(m, ServerMsg::LobbyState(s) if s.room.is_some()),
    ) else {
        unreachable!()
    };
    let room = s.room.unwrap().id;
    if ai {
        h.send(&ClientMsg::Lobby(LobbyCmd::Slot {
            index: 2,
            occupant: protocol::SlotOccupant::Ai,
            civ: protocol::CivPick::Random,
        }));
    }
    let mut g = Client::login(srv, guest);
    g.until(
        "the room in the list",
        |m| matches!(m, ServerMsg::LobbyState(s) if !s.rooms.is_empty()),
    );
    g.send(&ClientMsg::Lobby(LobbyCmd::JoinRoom { room }));
    g.until("in the room", |m| {
        matches!(m, ServerMsg::LobbyState(s) if s.room.as_ref().is_some_and(|r|
            r.slots.iter().any(|x| matches!(&x.occupant, Occupant::Human { nick, .. } if nick == guest))))
    });
    g.send(&ClientMsg::Lobby(LobbyCmd::Ready(true)));
    h.until("ready", |m| {
        matches!(m, ServerMsg::LobbyState(s) if s.room.as_ref().is_some_and(|r|
            r.slots.iter().any(|x| matches!(&x.occupant, Occupant::Human { ready: true, .. }))
            && (!ai || r.slots[2].occupant == Occupant::Ai)))
    });
    h.send(&ClientMsg::Lobby(LobbyCmd::Start));
    for c in [&mut h, &mut g] {
        c.until("Attached", |m| matches!(m, ServerMsg::Attached(_)));
        c.until_turn("a first state", |_| true);
    }
    (h, g)
}

fn lobby_of(
    c: &mut Client,
    what: &str,
    pred: impl Fn(&protocol::LobbyState) -> bool,
) -> protocol::LobbyState {
    let ServerMsg::LobbyState(s) =
        c.until(what, |m| matches!(m, ServerMsg::LobbyState(s) if pred(s)))
    else {
        unreachable!()
    };
    s
}

fn tick() {
    std::thread::sleep(Duration::from_millis(400));
}

fn dir_for(name: &str) -> PathBuf {
    temp_dir(name)
}

// ---------------------------------------------------------------- tests

#[test]
fn a_game_comes_back_after_a_restart_with_its_seats_host_numbers_and_saves() {
    let dir = dir_for("p-restart");
    let a = boot(&dir, "echo turns=1", |_| {});
    let (alice, bob) = make_game(&a, "alice", "bob", false);
    let (ta, tb) = (alice.token, bob.token);
    admin_game(&a, 1, AdminCtl::Save);
    wait_save(&dir, 1);
    drop((alice, bob));
    let meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("games/1/meta.json")).unwrap()).unwrap();
    assert_eq!(
        (meta["status"].as_str(), meta["host"].as_str()),
        (Some("Running"), Some("alice"))
    );
    let ids: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("ids.json")).unwrap()).unwrap();
    assert_eq!(
        (ids["next_game"].as_u64(), ids["next_room"].as_u64()),
        (Some(2), Some(2))
    );
    a.stop();

    let b = boot(&dir, "echo turns=1", |_| {});
    // Alice comes back with her token: the game is up again, she sits in the same seat.
    let mut alice = Client::connect(&b, "alice", "", ta);
    let ServerMsg::Attached(att) = alice.until("Attached", |m| matches!(m, ServerMsg::Attached(_)))
    else {
        unreachable!()
    };
    assert_eq!((att.game, att.player), (1, 0));
    // Bob has not come yet: his seat is held for him.
    let v = alice.until_seats("bob held", |v| v.len() == 2);
    assert!(matches!(v[1].conn, protocol::Conn::None), "{v:?}");
    assert!(v[0].host);
    let mut bob = Client::connect(&b, "bob", "", tb);
    bob.until("Attached", |m| matches!(m, ServerMsg::Attached(_)));
    alice.until_seats("bob online", |v| {
        matches!(v[1].conn, protocol::Conn::Online)
    });
    // The nick is held: nobody else can sit in a new room as bob, and the numbers go on.
    assert_eq!(b.shared.games.next_id(), 2);
    // The save is in the list of a third player.
    let mut carol = Client::login(&b, "carol");
    let s = lobby_of(&mut carol, "the save", |s| !s.saves.is_empty());
    assert_eq!(s.saves[0].name, "turns");
    assert!(s.saves[0].id.starts_with("1-"), "{}", s.saves[0].id);
    b.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_game_without_anybody_online_sleeps_and_wakes_for_its_player() {
    let dir = dir_for("p-sleep");
    let srv = boot(&dir, "echo turns=1", |c| c.hibernate_idle_min = 1);
    let (alice, bob) = make_game(&srv, "alice", "bob", true);
    let token = alice.token;
    drop((alice, bob));
    tick();
    srv.clock.advance(59_000);
    tick();
    assert!(srv.shared.games.get(1).is_some(), "not before the minute");
    srv.clock.advance(2_000);
    let mut carol = Client::login(&srv, "carol");
    let s = lobby_of(&mut carol, "a sleeping game", |s| {
        s.games
            .first()
            .is_some_and(|g| g.status == GameStatus::Hibernated)
    });
    let g = &s.games[0];
    assert_eq!(g.seats.len(), 3);
    // Its AI slot can be taken by somebody else: the game wakes for that, too.
    assert!(g.seats[2].takeable);
    let meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("games/1/meta.json")).unwrap()).unwrap();
    assert_eq!(meta["status"], "Hibernating");
    // Alice logs in: the game wakes, she is attached again, and a new stream starts.
    let mut alice = Client::connect(&srv, "alice", "", token);
    alice.until("Attached", |m| matches!(m, ServerMsg::Attached(_)));
    alice.until_turn("a state", |_| true);
    assert!(srv.shared.games.get(1).is_some());
    srv.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_join_into_a_sleeping_game_wakes_it_and_takes_the_slot() {
    let dir = dir_for("p-sleep-join");
    let srv = boot(&dir, "echo turns=1", |c| c.hibernate_idle_min = 1);
    let (alice, bob) = make_game(&srv, "alice", "bob", true);
    drop((alice, bob));
    tick();
    srv.clock.advance(61_000);
    let mut carol = Client::login(&srv, "carol");
    lobby_of(&mut carol, "asleep", |s| {
        s.games
            .first()
            .is_some_and(|g| g.status == GameStatus::Hibernated)
    });
    carol.send(&ClientMsg::Lobby(LobbyCmd::JoinGame {
        game: 1,
        player: 2,
        basis: None,
    }));
    let ServerMsg::Attached(a) = carol.until("Attached", |m| matches!(m, ServerMsg::Attached(_)))
    else {
        unreachable!()
    };
    assert_eq!((a.game, a.player), (1, 2));
    srv.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------- the close rule (the user, 2026-11-02)

fn meta_of(dir: &Path, game: u32) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(dir.join(format!("games/{game}/meta.json"))).unwrap())
        .unwrap()
}

fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_game_whose_last_human_left_is_closed_after_the_delay_with_its_saves_and_free_nicks() {
    let dir = dir_for("p-close");
    let srv = boot(&dir, "echo turns=1", |c| c.empty_close_min = 1);
    let (alice, bob) = make_game(&srv, "alice", "bob", true);
    admin_game(&srv, 1, AdminCtl::Save);
    wait_save(&dir, 1);
    drop((alice, bob));
    tick();
    srv.clock.advance(59_000);
    tick();
    assert!(srv.shared.games.get(1).is_some(), "not before the minute");
    srv.clock.advance(2_000);
    wait_for("the game is closed", || {
        srv.shared.games.get(1).is_none() && meta_of(&dir, 1)["status"] == "Finished"
    });
    assert!(srv.shared.games.dormant(1).is_none(), "it does not sleep");
    // The saves stay and are offered by the lobby; the nicks are free: alice sits in no game and may open a room.
    let mut alice = Client::login(&srv, "alice");
    let s = lobby_of(&mut alice, "the lobby", |s| !s.saves.is_empty());
    assert!(s.games.is_empty(), "{:?}", s.games);
    assert!(s.saves[0].id.starts_with("1-"), "{}", s.saves[0].id);
    alice.send(&ClientMsg::Lobby(LobbyCmd::CreateRoom {
        settings: settings(TimerMode::Off),
    }));
    lobby_of(&mut alice, "a room of her own", |s| s.room.is_some());
    srv.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_player_back_within_the_delay_keeps_the_seat_and_the_count_starts_over() {
    let dir = dir_for("p-close-back");
    let srv = boot(&dir, "echo turns=1", |c| c.empty_close_min = 1);
    let (alice, bob) = make_game(&srv, "alice", "bob", false);
    let token = alice.token;
    drop((alice, bob));
    tick();
    srv.clock.advance(50_000);
    tick();
    let mut alice = Client::connect(&srv, "alice", "", token);
    let ServerMsg::Attached(a) = alice.until("Attached", |m| matches!(m, ServerMsg::Attached(_)))
    else {
        unreachable!()
    };
    assert_eq!((a.game, a.player), (1, 0));
    // With her in the game, any time passes.
    srv.clock.advance(300_000);
    tick();
    assert!(srv.shared.games.get(1).is_some());
    // She leaves again: a new full delay starts from here.
    drop(alice);
    tick();
    srv.clock.advance(59_000);
    tick();
    assert!(srv.shared.games.get(1).is_some(), "the count started over");
    srv.clock.advance(2_000);
    wait_for("closed at last", || srv.shared.games.get(1).is_none());
    assert_eq!(meta_of(&dir, 1)["status"], "Finished");
    srv.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_sleeping_game_without_humans_at_the_start_of_the_server_is_closed() {
    let dir = dir_for("p-close-sleeper");
    let a = boot(&dir, "echo turns=1", |_| {});
    let (alice, bob) = make_game(&a, "alice", "bob", false);
    admin_game(&a, 1, AdminCtl::Stop);
    drop((alice, bob));
    wait_for("asleep", || a.shared.games.dormant(1).is_some());
    a.stop();
    assert_eq!(meta_of(&dir, 1)["status"], "Hibernating");

    let b = boot(&dir, "echo turns=1", |c| c.empty_close_min = 1);
    assert!(
        b.shared.games.dormant(1).is_some(),
        "it sleeps after a start"
    );
    // Let the lobby see it asleep, then the delay passes.
    std::thread::sleep(Duration::from_millis(1500));
    b.clock.advance(59_000);
    std::thread::sleep(Duration::from_millis(1500));
    assert!(b.shared.games.dormant(1).is_some(), "not before the minute");
    b.clock.advance(2_000);
    wait_for("the sleeper is closed", || {
        b.shared.games.dormant(1).is_none()
    });
    wait_for("Finished on disk", || {
        meta_of(&dir, 1)["status"] == "Finished"
    });
    // Its saves are listed, its players are free.
    let mut bob = Client::login(&b, "bob");
    let s = lobby_of(&mut bob, "the lobby", |s| !s.saves.is_empty());
    assert!(s.games.is_empty());
    bob.send(&ClientMsg::Lobby(LobbyCmd::CreateRoom {
        settings: settings(TimerMode::Off),
    }));
    lobby_of(&mut bob, "a room", |s| s.room.is_some());
    b.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_close_delay_zero_keeps_a_game_for_ever() {
    let dir = dir_for("p-close-off");
    let srv = boot(&dir, "echo turns=1", |c| {
        c.empty_close_min = 0;
        c.hibernate_idle_min = 0;
    });
    let (alice, bob) = make_game(&srv, "alice", "bob", false);
    drop((alice, bob));
    tick();
    srv.clock.advance(24 * 3_600_000);
    tick();
    assert!(srv.shared.games.get(1).is_some());
    srv.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
#[test]
fn an_administrator_stop_puts_the_game_to_sleep_with_its_seats_kept() {
    let dir = dir_for("p-stop");
    let srv = boot(&dir, "echo turns=1", |_| {});
    let (mut alice, mut bob) = make_game(&srv, "alice", "bob", false);
    admin_game(&srv, 1, AdminCtl::Stop);
    for c in [&mut alice, &mut bob] {
        let ServerMsg::Detached { reason } =
            c.until("Detached", |m| matches!(m, ServerMsg::Detached { .. }))
        else {
            unreachable!()
        };
        assert!(matches!(reason, protocol::DetachReason::GameStopped { .. }));
    }
    // They see the lobby with the sleeping game and their own seats takeable; they are not free to
    // start something else.
    let s = lobby_of(&mut bob, "the sleeping game", |s| {
        s.games
            .first()
            .is_some_and(|g| g.status == GameStatus::Hibernated)
    });
    assert!(s.games[0].seats[1].takeable);
    bob.send(&ClientMsg::Lobby(LobbyCmd::CreateRoom {
        settings: settings(TimerMode::Off),
    }));
    bob.until("a refusal", |m| matches!(m, ServerMsg::LobbyError { .. }));
    // Back to the own seat: the game wakes and attaches him.
    bob.send(&ClientMsg::Lobby(LobbyCmd::JoinGame {
        game: 1,
        player: 1,
        basis: None,
    }));
    let ServerMsg::Attached(a) = bob.until("Attached", |m| matches!(m, ServerMsg::Attached(_)))
    else {
        unreachable!()
    };
    assert_eq!((a.game, a.player), (1, 1));
    srv.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The slot `index` of `s`'s room holds the human `nick`; with `ready`, he said so.
fn sits(s: &protocol::LobbyState, index: usize, nick: &str, ready: bool) -> bool {
    s.room.as_ref().is_some_and(|r| {
        matches!(&r.slots[index].occupant, Occupant::Human { nick: n, ready: x } if n == nick && (*x || !ready))
    })
}

#[test]
fn a_seated_player_takes_its_slot_in_a_room_made_from_the_save_of_its_sleeping_game() {
    let dir = dir_for("p-stop-room");
    let srv = boot(&dir, "echo turns=1", |_| {});
    let (mut alice, mut bob) = make_game(&srv, "alice", "bob", true);
    admin_game(&srv, 1, AdminCtl::Stop);
    for c in [&mut alice, &mut bob] {
        c.until("Detached", |m| matches!(m, ServerMsg::Detached { .. }));
    }
    // Carol (no seat) makes a room from the save of the stop: slots 0 and 1 are kept for alice and bob.
    let mut carol = Client::login(&srv, "carol");
    let s = lobby_of(&mut carol, "the save of the sleeping game", |s| {
        !s.saves.is_empty()
            && s.games
                .first()
                .is_some_and(|g| g.status == GameStatus::Hibernated)
    });
    carol.send(&ClientMsg::Lobby(LobbyCmd::LoadSave {
        save: s.saves[0].id.clone(),
    }));
    let s = lobby_of(&mut carol, "the room from the save", |s| {
        s.room.as_ref().is_some_and(|r| r.from_save.is_some())
    });
    let room = s.room.unwrap().id;
    // Bob comes back with his token: the room keeps his slot, so his game sleeps on.
    let token = bob.token;
    drop(bob);
    tick();
    let mut bob = Client::connect(&srv, "bob", "", token);
    lobby_of(&mut bob, "the lobby with the room", |s| !s.rooms.is_empty());
    tick();
    assert!(srv.shared.games.get(1).is_none(), "the game sleeps");
    // Alice joins the room and leaves it: her seat in the sleeping game is her place again.
    alice.send(&ClientMsg::Lobby(LobbyCmd::JoinRoom { room }));
    lobby_of(&mut alice, "alice in slot 0", |s| {
        sits(s, 0, "alice", false)
    });
    alice.send(&ClientMsg::Lobby(LobbyCmd::LeaveRoom));
    lobby_of(&mut alice, "alice out of the room", |s| s.room.is_none());
    alice.send(&ClientMsg::Lobby(LobbyCmd::CreateRoom {
        settings: settings(TimerMode::Off),
    }));
    let ServerMsg::LobbyError { text } =
        alice.until("a refusal", |m| matches!(m, ServerMsg::LobbyError { .. }))
    else {
        unreachable!()
    };
    assert!(text.contains("seat"), "{text}");
    // Both take their slots, carol starts: the game of the save is a new game, everybody where the save had them.
    for (c, i) in [(&mut alice, 0), (&mut bob, 1)] {
        c.send(&ClientMsg::Lobby(LobbyCmd::JoinRoom { room }));
        let nick = c.nick.clone();
        lobby_of(c, "in the slot", |s| sits(s, i, &nick, false));
    }
    // Every change of the room takes `Ready` off: they say it when both sit.
    lobby_of(&mut carol, "both in", |s| {
        sits(s, 0, "alice", false) && sits(s, 1, "bob", false)
    });
    for c in [&mut alice, &mut bob] {
        c.send(&ClientMsg::Lobby(LobbyCmd::Ready(true)));
    }
    lobby_of(&mut carol, "both ready", |s| {
        sits(s, 0, "alice", true) && sits(s, 1, "bob", true)
    });
    carol.send(&ClientMsg::Lobby(LobbyCmd::Start));
    for (c, player) in [(&mut alice, 0), (&mut bob, 1), (&mut carol, 2)] {
        let ServerMsg::Attached(a) = c.until("Attached", |m| matches!(m, ServerMsg::Attached(_)))
        else {
            unreachable!()
        };
        assert_eq!(a.player, player);
        assert_ne!(a.game, 1, "a new game");
    }
    srv.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_room_from_a_save_that_starts_ends_the_sleeping_game_and_frees_the_seats_left_in_it() {
    let dir = dir_for("p-stop-room-ends");
    let srv = boot(&dir, "echo turns=1", |_| {});
    let (mut alice, mut bob) = make_game(&srv, "alice", "bob", true);
    admin_game(&srv, 1, AdminCtl::Stop);
    for c in [&mut alice, &mut bob] {
        c.until("Detached", |m| matches!(m, ServerMsg::Detached { .. }));
    }
    let mut carol = Client::login(&srv, "carol");
    let s = lobby_of(&mut carol, "the save of the sleeping game", |s| {
        !s.saves.is_empty()
            && s.games
                .first()
                .is_some_and(|g| g.status == GameStatus::Hibernated)
    });
    assert!(srv.shared.games.dormant(1).is_some());
    carol.send(&ClientMsg::Lobby(LobbyCmd::LoadSave {
        save: s.saves[0].id.clone(),
    }));
    let s = lobby_of(&mut carol, "the room from the save", |s| {
        s.room.as_ref().is_some_and(|r| r.from_save.is_some())
    });
    let room = s.room.unwrap().id;
    // Only alice takes her slot; bob's seat in the sleeping game is left behind.
    alice.send(&ClientMsg::Lobby(LobbyCmd::JoinRoom { room }));
    lobby_of(&mut alice, "alice in slot 0", |s| {
        sits(s, 0, "alice", false)
    });
    alice.send(&ClientMsg::Lobby(LobbyCmd::Ready(true)));
    lobby_of(&mut carol, "alice ready", |s| sits(s, 0, "alice", true));
    // The host gives bob's open slot to the AI.
    carol.send(&ClientMsg::Lobby(LobbyCmd::Slot {
        index: 1,
        occupant: protocol::SlotOccupant::Ai,
        civ: protocol::CivPick::Random,
    }));
    lobby_of(&mut carol, "slot 1 is the AI's", |s| {
        s.room
            .as_ref()
            .is_some_and(|r| matches!(r.slots[1].occupant, Occupant::Ai))
    });
    alice.send(&ClientMsg::Lobby(LobbyCmd::Ready(true)));
    lobby_of(&mut carol, "alice ready again", |s| {
        sits(s, 0, "alice", true)
    });
    carol.send(&ClientMsg::Lobby(LobbyCmd::Start));
    let ServerMsg::Attached(a) = alice.until("Attached", |m| matches!(m, ServerMsg::Attached(_)))
    else {
        unreachable!()
    };
    assert_ne!(a.game, 1, "a new game");
    // The old game is over: no sleeper, `Finished` on disk, and bob is free again.
    assert!(srv.shared.games.dormant(1).is_none());
    let metas = server::state::meta::read_all(&dir).unwrap();
    let old = metas.iter().find(|m| m.id == 1).unwrap();
    assert_eq!(old.status, server::state::meta::MetaStatus::Finished);
    bob.send(&ClientMsg::Lobby(LobbyCmd::CreateRoom {
        settings: settings(TimerMode::Off),
    }));
    lobby_of(&mut bob, "a room for bob", |s| s.room.is_some());
    srv.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_game_that_does_not_fit_under_max_games_sleeps_and_is_refused_a_wake() {
    let dir = dir_for("p-limit");
    let a = boot(&dir, "echo turns=1", |_| {});
    let (g1a, g1b) = make_game(&a, "alice", "bob", false);
    let (g2a, g2b) = make_game(&a, "carol", "dave", false);
    admin_game(&a, 1, AdminCtl::Save);
    admin_game(&a, 2, AdminCtl::Save);
    wait_save(&dir, 1);
    wait_save(&dir, 2);
    let tokens = (g1a.token, g2a.token);
    drop((g1a, g1b, g2a, g2b));
    a.stop();

    let b = boot(&dir, "echo turns=1", |c| c.max_games = 1);
    assert!(b.shared.games.get(1).is_some(), "the first game comes back");
    assert!(b.shared.games.get(2).is_none(), "the second has no room");
    assert!(b.shared.games.dormant(2).is_some());
    // Carol logs in: the wake is refused and she sees why; the game stays asleep.
    let mut carol = Client::connect(&b, "carol", "", tokens.1);
    carol.until(
        "the refusal",
        |m| matches!(m, ServerMsg::LobbyError { text } if text.contains("too many games")),
    );
    assert!(b.shared.games.dormant(2).is_some());
    // Alice's game is up: she gets her seat.
    let mut alice = Client::connect(&b, "alice", "", tokens.0);
    alice.until("Attached", |m| matches!(m, ServerMsg::Attached(_)));
    b.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_save_that_is_not_a_c5sv_file_is_left_out_of_the_list_and_of_the_restart() {
    let dir = dir_for("p-badsave");
    let a = boot(&dir, "echo turns=1", |_| {});
    let (alice, bob) = make_game(&a, "alice", "bob", false);
    admin_game(&a, 1, AdminCtl::Save);
    wait_save(&dir, 1);
    drop((alice, bob));
    a.stop();
    // A newer file that is junk: it must not be the one the game continues from.
    let junk = dir.join("games/1/ring/00000099-t0007.c5sv");
    std::fs::write(&junk, b"this is not a save").unwrap();
    let b = boot(&dir, "echo turns=1", |_| {});
    let mut carol = Client::login(&b, "carol");
    let s = lobby_of(&mut carol, "the saves", |s| !s.saves.is_empty());
    assert!(
        s.saves.iter().all(|x| x.id != "1-00000099"),
        "{:?}",
        s.saves
    );
    assert!(b.shared.games.get(1).is_some());
    b.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_game_that_cannot_be_read_is_dropped_and_its_players_are_free() {
    let dir = dir_for("p-nosave");
    let a = boot(&dir, "echo turns=1", |_| {});
    let (alice, bob) = make_game(&a, "alice", "bob", false);
    let token = alice.token;
    drop((alice, bob));
    a.stop();
    // No save was ever written: there is nothing to continue from.
    let _ = std::fs::remove_dir_all(dir.join("games/1/ring"));
    let b = boot(&dir, "echo turns=1", |_| {});
    assert!(b.shared.games.get(1).is_none() && b.shared.games.dormant(1).is_none());
    let mut alice = Client::connect(&b, "alice", "", token);
    let s = lobby_of(&mut alice, "the lobby", |_| true);
    assert!(s.games.is_empty());
    // Free to make a room.
    alice.send(&ClientMsg::Lobby(LobbyCmd::CreateRoom {
        settings: settings(TimerMode::Off),
    }));
    lobby_of(&mut alice, "a room", |s| s.room.is_some());
    b.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------- LeaveSeat

fn can_make_a_room(c: &mut Client) {
    c.send(&ClientMsg::Lobby(LobbyCmd::CreateRoom {
        settings: settings(TimerMode::Off),
    }));
    lobby_of(c, "a room", |s| s.room.is_some());
}

fn meta_on_disk(dir: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(dir.join("games/1/meta.json")).unwrap()).unwrap()
}

#[test]
fn leave_seat_gives_the_slot_to_the_ai_in_a_running_game_and_frees_the_nick() {
    let dir = dir_for("p-leave-seat-run");
    let srv = boot(&dir, "echo turns=1", |_| {});
    let (mut alice, mut bob) = make_game(&srv, "alice", "bob", false);
    bob.send(&ClientMsg::Lobby(LobbyCmd::LeaveSeat));
    let ServerMsg::Detached { reason } =
        bob.until("Detached", |m| matches!(m, ServerMsg::Detached { .. }))
    else {
        unreachable!()
    };
    assert_eq!(reason, protocol::DetachReason::Kicked);
    // The slot is the AI's, the nick is free: bob can make a room.
    lobby_of(&mut bob, "the lobby", |s| s.room.is_none());
    let seat = srv.shared.games.get(1).unwrap().info.snapshot().seats[1]
        .control
        .clone();
    assert!(matches!(seat, protocol::SeatControl::Ai { kicked: Some(ref n) } if n == "bob"));
    can_make_a_room(&mut bob);
    // Alice is the last human now: she cannot be kicked, but she can give the seat up; the game sleeps.
    alice.send(&ClientMsg::Lobby(LobbyCmd::LeaveSeat));
    lobby_of(&mut alice, "the sleeping game", |s| {
        s.games
            .first()
            .is_some_and(|g| g.status == GameStatus::Hibernated)
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while srv.shared.games.dormant(1).is_none() {
        assert!(Instant::now() < deadline, "the game did not go to sleep");
        std::thread::sleep(Duration::from_millis(50));
    }
    let meta = srv.shared.games.dormant(1).unwrap();
    assert!(meta.humans().is_empty(), "{meta:?}");
    assert_eq!(meta.seats[0].kicked.as_deref(), Some("alice"));
    let disk = meta_on_disk(&dir);
    assert_eq!(disk["status"], "Hibernating");
    assert!(disk["seats"][0]["nick"].is_null() && disk["seats"][1]["nick"].is_null());
    can_make_a_room(&mut alice);
    srv.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn leave_seat_in_a_sleeping_game_updates_its_stored_seats() {
    let dir = dir_for("p-leave-seat-sleep");
    let srv = boot(&dir, "echo turns=1", |_| {});
    let (mut alice, mut bob) = make_game(&srv, "alice", "bob", true);
    admin_game(&srv, 1, AdminCtl::Stop);
    for c in [&mut alice, &mut bob] {
        c.until("Detached", |m| matches!(m, ServerMsg::Detached { .. }));
    }
    lobby_of(&mut bob, "the sleeping game", |s| {
        s.games
            .first()
            .is_some_and(|g| g.status == GameStatus::Hibernated)
    });
    // Without a seat there is nothing to give up.
    let mut carol = Client::login(&srv, "carol");
    lobby_of(&mut carol, "the lobby", |_| true);
    carol.send(&ClientMsg::Lobby(LobbyCmd::LeaveSeat));
    carol.until("a refusal", |m| matches!(m, ServerMsg::LobbyError { .. }));
    // Bob gives his up: stored seats and the file change, alice keeps hers.
    bob.send(&ClientMsg::Lobby(LobbyCmd::LeaveSeat));
    let s = lobby_of(&mut bob, "bob's seat is the AI's", |s| {
        s.games.first().is_some_and(|g| {
            matches!(&g.seats[1].control, protocol::SeatControl::Ai { kicked: Some(n) } if n == "bob")
        })
    });
    assert!(
        matches!(&s.games[0].seats[0].control, protocol::SeatControl::Human { nick } if nick == "alice")
    );
    let meta = srv.shared.games.dormant(1).unwrap();
    assert_eq!(meta.humans(), vec![(0, "alice".to_owned())]);
    let disk = meta_on_disk(&dir);
    assert!(disk["seats"][1]["nick"].is_null());
    assert_eq!(disk["seats"][1]["kicked"], "bob");
    // Bob can create a room now.
    can_make_a_room(&mut bob);
    srv.stop();
    let _ = std::fs::remove_dir_all(&dir);
}
