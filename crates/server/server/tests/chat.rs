//! Chat of a game (9 `chat`) over real sockets with the mock worker: a line to all, a private
//! line, the rate limit, the cleaning of the text, the history of a new stream, chat without a game.

#![allow(dead_code, unused_imports)]

mod common;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{mock_server_config, temp_dir};
use protocol::{
    AutoChoices, ChatLine, ChatSend, ChatTo, ClientMsg, Cmd, Codec, Command, CommandOutcome, Conn,
    DetachReason, GameCtl, GameCtlReason, GameMsg, GameSettings, LobbyCmd, Login, Occupant,
    PROTOCOL_VERSION, Postcard, SeatControl, SeatView, ServerMsg, TimerMode, Token, TurnMode,
    TurnState,
};
use server::Shared;
use server::clock::TestClock;
use server::identity::Secret;
use server::session::limits::LimitsConfig;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};
use wire::{Encoder, Limits};

const PASSWORD: &str = "turn-password";
const ADMIN: &str = "admin-secret";

struct TestServer {
    addr: SocketAddr,
    admin: SocketAddr,
    dir: PathBuf,
    clock: Arc<TestClock>,
    _shared: Arc<Shared>,
    _rt: tokio::runtime::Runtime,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn start(name: &str, mode: &str) -> TestServer {
    let dir = temp_dir(name);
    let clock = Arc::new(TestClock::new());
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
    config.clock = clock.clone();
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
        dir,
        clock,
        _shared: shared,
        _rt: rt,
    }
}

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

/// alice (host) and bob in a game with `timer`, both attached and with a first `TurnState`.
fn game(name: &str, mode: &str, timer: TimerMode) -> (TestServer, Client, Client) {
    let srv = start(name, mode);
    let mut alice = Client::login(&srv, "alice");
    alice.until("the lobby", |m| matches!(m, ServerMsg::LobbyState(_)));
    alice.send(&ClientMsg::Lobby(LobbyCmd::CreateRoom {
        settings: settings(timer),
    }));
    let ServerMsg::LobbyState(s) = alice.until(
        "the room",
        |m| matches!(m, ServerMsg::LobbyState(s) if s.room.is_some()),
    ) else {
        unreachable!()
    };
    let room = s.room.unwrap().id;
    let mut bob = Client::login(&srv, "bob");
    bob.until(
        "the room in the list",
        |m| matches!(m, ServerMsg::LobbyState(s) if !s.rooms.is_empty()),
    );
    bob.send(&ClientMsg::Lobby(LobbyCmd::JoinRoom { room }));
    bob.until("bob in", |m| {
        matches!(m, ServerMsg::LobbyState(s) if s.room.as_ref().is_some_and(|r|
            r.slots.iter().any(|x| matches!(&x.occupant, Occupant::Human { nick, .. } if nick == "bob"))))
    });
    bob.send(&ClientMsg::Lobby(LobbyCmd::Ready(true)));
    alice.until("bob ready", |m| {
        matches!(m, ServerMsg::LobbyState(s) if s.room.as_ref().is_some_and(|r|
            r.slots.iter().any(|x| matches!(&x.occupant, Occupant::Human { ready: true, .. }))))
    });
    alice.send(&ClientMsg::Lobby(LobbyCmd::Start));
    for c in [&mut alice, &mut bob] {
        c.until("Attached", |m| matches!(m, ServerMsg::Attached(_)));
        c.until_turn("a first state", |_| true);
    }
    (srv, alice, bob)
}

fn http_get(addr: SocketAddr, path: &str) -> serde_json::Value {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        s,
        "GET {path} HTTP/1.0\r\nHost: localhost\r\nX-Admin-Token: {ADMIN}\r\n\r\n"
    )
    .unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    serde_json::from_str(raw.split("\r\n\r\n").nth(1).unwrap_or_default()).unwrap()
}

fn timer_metrics(srv: &TestServer) -> serde_json::Value {
    http_get(srv.admin, "/admin/metrics")["games"]["1"]["timer"].clone()
}

fn force_lines(path: &str) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Waits (real time) until the force log has `n` lines.
fn wait_force(path: &str, n: usize) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let lines = force_lines(path);
        if lines.len() >= n {
            return lines;
        }
        assert!(
            Instant::now() < deadline,
            "the force log has {lines:?}, wanted {n} lines"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn mode_with_log(dir_name: &str, extra: &str) -> (String, String) {
    let log = std::env::temp_dir()
        .join(format!("civ5-turn-{}-{dir_name}.log", std::process::id()))
        .to_string_lossy()
        .into_owned();
    let _ = std::fs::remove_file(&log);
    (format!("echo turns=1 force_log={log} {extra}"), log)
}

// ---------------------------------------------------------------- chat

/// The first chat lines (from now on) that come in a batch.
fn until_chat(c: &mut Client, what: &str) -> Vec<ChatLine> {
    for _ in 0..400 {
        if let ServerMsg::Game(b) = c.recv() {
            for g in b.msgs {
                if let GameMsg::Chat(lines) = g {
                    return lines;
                }
            }
        }
    }
    panic!("{}: no chat ({what})", c.nick);
}

fn say(c: &mut Client, to: ChatTo, text: &str) {
    c.send(&ClientMsg::Chat(ChatSend {
        to,
        text: text.into(),
    }));
}

#[test]
fn a_line_to_all_reaches_every_seat_with_the_name_of_the_sender_and_the_gateway_time() {
    let (srv, mut alice, mut bob) = game("c-all", "echo turns=1", TimerMode::Off);
    srv.clock.advance(1234);
    say(&mut alice, ChatTo::All, "  hello\u{7} there\u{200B}  ");
    for c in [&mut alice, &mut bob] {
        let lines = until_chat(c, "to all");
        assert_eq!(lines.len(), 1);
        let l = &lines[0];
        assert_eq!(
            (
                l.from.as_str(),
                l.from_player,
                l.to,
                l.text.as_str(),
                l.server_ms
            ),
            ("alice", Some(0), ChatTo::All, "hello there", 1234)
        );
    }
    let all = http_get(srv.admin, "/admin/metrics");
    assert_eq!(all["games"]["1"]["chat"]["lines"], 1);
}

#[test]
fn a_private_line_reaches_only_the_sender_and_the_addressee() {
    let (_srv, mut alice, mut bob) = game("c-private", "echo turns=1", TimerMode::Off);
    // A third human is not needed to see it: alice writes to bob, then to nobody.
    say(&mut alice, ChatTo::Player(1), "psst");
    assert_eq!(until_chat(&mut bob, "private")[0].text, "psst");
    assert_eq!(until_chat(&mut alice, "echo")[0].to, ChatTo::Player(1));
    // An AI or a closed slot is no addressee: the sender is told in the journal.
    say(&mut alice, ChatTo::Player(7), "anybody?");
    alice.until_journal("not a human");
}

#[test]
fn too_many_lines_are_refused_with_a_journal_line_and_counted() {
    let (srv, mut alice, mut bob) = game("c-rate", "echo turns=1", TimerMode::Off);
    for i in 0..5 {
        say(&mut alice, ChatTo::All, &format!("line {i}"));
    }
    say(&mut alice, ChatTo::All, "one too many");
    alice.until_journal("too fast");
    let mut got = Vec::new();
    while got.len() < 5 {
        got.extend(
            until_chat(&mut bob, "five lines")
                .into_iter()
                .map(|l| l.text),
        );
    }
    assert_eq!(got, ["line 0", "line 1", "line 2", "line 3", "line 4"]);
    let all = http_get(srv.admin, "/admin/metrics");
    assert_eq!(all["games"]["1"]["chat"]["dropped_rate"], 1);
    // A window later the seat may speak again.
    srv.clock.advance(10_001);
    say(&mut alice, ChatTo::All, "again");
    assert_eq!(until_chat(&mut bob, "again")[0].text, "again");
}

#[test]
fn an_empty_line_is_dropped_and_a_long_one_is_cut() {
    let (_srv, mut alice, mut bob) = game("c-clean", "echo turns=1", TimerMode::Off);
    say(&mut alice, ChatTo::All, " \u{200B}\t ");
    say(&mut alice, ChatTo::All, &"я".repeat(900));
    let l = until_chat(&mut bob, "the long one");
    assert_eq!(l[0].text.chars().count(), 500);
}

#[test]
fn a_new_stream_brings_the_history_the_seat_saw_and_a_lost_seat_gets_private_lines_too() {
    let (srv, mut alice, bob) = game("c-history", "echo turns=1", TimerMode::Off);
    let token = bob.token;
    drop(bob);
    say(&mut alice, ChatTo::All, "for everybody");
    say(&mut alice, ChatTo::Player(1), "for bob while he is away");
    srv.clock.advance(5_000);
    std::thread::sleep(Duration::from_millis(300));
    // Bob comes back with the token only: a `Fresh` with the lines he saw (both).
    let mut bob = Client::connect(&srv, "bob", "", token);
    bob.until("Attached", |m| matches!(m, ServerMsg::Attached(_)));
    let lines = until_chat(&mut bob, "the history");
    let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
    assert_eq!(texts, ["for everybody", "for bob while he is away"]);
}

#[test]
fn chat_outside_a_game_is_ignored_and_the_session_goes_on() {
    let srv = start("c-nogame", "echo turns=1");
    let mut c = Client::login(&srv, "carol");
    c.until("the lobby", |m| matches!(m, ServerMsg::LobbyState(_)));
    say(&mut c, ChatTo::All, "hello?");
    c.send(&ClientMsg::Ping {
        seq: 9,
        client_ms: 1,
    });
    c.until("the pong", |m| matches!(m, ServerMsg::Pong { seq: 9, .. }));
}
