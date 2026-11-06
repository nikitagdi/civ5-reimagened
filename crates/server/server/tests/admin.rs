//! The admin API (9 `admin`) over a real listener with the mock worker: the token, the names of the
//! counters, save, stop and restart, the kick of an administrator, the drain, and the test hooks (only with
//! `test_hooks`).

#![allow(dead_code, unused_imports)]

mod common;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{mock_server_config, temp_dir};
use protocol::{
    AutoChoices, ClientMsg, Cmd, Codec, Command, CommandOutcome, Conn, DetachReason, GameCtl,
    GameCtlReason, GameMsg, GameSettings, LobbyCmd, Login, Occupant, PROTOCOL_VERSION, Postcard,
    SeatControl, SeatView, ServerMsg, TimerMode, Token, TurnMode, TurnState,
};
use serde_json::Value;
use server::clock::TestClock;
use server::identity::Secret;
use server::session::limits::LimitsConfig;
use server::{ServerConfig, Shared};
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
    shared: Arc<Shared>,
    _rt: tokio::runtime::Runtime,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn start(name: &str, mode: &str) -> TestServer {
    start_with(name, mode, |_| {})
}

fn start_with(name: &str, mode: &str, edit: impl FnOnce(&mut ServerConfig)) -> TestServer {
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
        dir,
        clock,
        shared,
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
    game_with(name, mode, timer, |_| {})
}

fn game_with(
    name: &str,
    mode: &str,
    timer: TimerMode,
    edit: impl FnOnce(&mut ServerConfig),
) -> (TestServer, Client, Client) {
    let srv = start_with(name, mode, edit);
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

// ---------------------------------------------------------------- the admin API

/// One request to the admin listener: status and JSON body (`Null` for an empty or non-JSON one).
fn call(
    srv: &TestServer,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: &str,
) -> (u16, Value) {
    let mut s = TcpStream::connect(srv.admin).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(40))).unwrap();
    let auth = token.map_or(String::new(), |t| format!("X-Admin-Token: {t}\r\n"));
    write!(
        s,
        "{method} {path} HTTP/1.0\r\nHost: localhost\r\n{auth}Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    let status = raw
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap();
    let json = serde_json::from_str(raw.split("\r\n\r\n").nth(1).unwrap_or_default())
        .unwrap_or(Value::Null);
    (status, json)
}

fn get(srv: &TestServer, path: &str) -> (u16, Value) {
    call(srv, "GET", path, Some(ADMIN), "")
}

fn post(srv: &TestServer, path: &str) -> (u16, Value) {
    call(srv, "POST", path, Some(ADMIN), "")
}

fn hooks(c: &mut ServerConfig) {
    c.test_hooks = true;
}

fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !f() {
        assert!(Instant::now() < deadline, "never: {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn every_request_needs_the_token_and_unknown_paths_are_404() {
    let srv = start("a-auth", "echo turns=1");
    assert_eq!(call(&srv, "GET", "/admin/metrics", None, "").0, 401);
    assert_eq!(
        call(&srv, "GET", "/admin/metrics", Some("wrong"), "").0,
        401
    );
    assert_eq!(call(&srv, "POST", "/admin/drain", None, "").0, 401);
    assert_eq!(get(&srv, "/admin/nothing").0, 404);
    // The paths of the test hooks are not there without the flag.
    assert_eq!(post(&srv, "/admin/games/1/kill-worker?signal=KILL").0, 404);
    assert_eq!(
        call(&srv, "POST", "/admin/games/1/fault", Some(ADMIN), "{}").0,
        404
    );
    assert_eq!(post(&srv, "/admin/games/9/save").0, 404);
}

#[test]
fn the_metrics_carry_the_names_of_the_spec() {
    let (srv, mut alice, mut bob) = game("a-metrics", "echo turns=1", TimerMode::Off);
    let id = alice.order(1, Command::UnitSleep { unit: 1 });
    assert_eq!(alice.answer(id), CommandOutcome::Applied);
    let (status, m) = get(&srv, "/admin/metrics");
    assert_eq!(status, 200);
    assert_eq!(m["schema"], 1);
    for k in [
        "build",
        "core_build",
        "protocol",
        "ipc",
        "uptime_s",
        "now_ms",
    ] {
        assert!(m.get(k).is_some(), "top level {k}");
    }
    let s = &m["server"];
    for k in [
        "open",
        "logged_in",
        "opened_total",
        "lost_timeout_total",
        "closed_clean_total",
        "replaced_total",
        "login_failed_total",
        "slow_consumer_total",
    ] {
        assert!(s["sessions"].get(k).is_some(), "sessions.{k}");
    }
    for k in ["running", "hibernating", "faulted", "limit"] {
        assert!(s["games"].get(k).is_some(), "games.{k}");
    }
    assert!(s["mem"].get("gateway_rss_kb").is_some() && s["disk"].get("state_free_mb").is_some());
    let g = &m["games"]["1"];
    for k in [
        "status",
        "turn",
        "phase",
        "mode",
        "settings",
        "map",
        "worker",
        "hist",
        "cmds",
        "views",
        "saves",
        "fog_violations",
        "timer",
        "gamectl_rejected",
        "chat",
        "host",
        "seats",
    ] {
        assert!(g.get(k).is_some(), "games.1.{k}");
    }
    for k in [
        "pid",
        "rss_kb",
        "starts",
        "crashes",
        "hang_kills",
        "last_exit",
        "boot_ms",
        "restart_ms",
    ] {
        assert!(g["worker"].get(k).is_some(), "worker.{k}");
    }
    assert!(g["worker"]["pid"].as_u64().is_some_and(|p| p > 0));
    for h in [
        "cmd_gateway_ms",
        "step_ms",
        "ai_phase_ms",
        "batch_build_ms",
        "safe_point_wait_ms",
        "restore_ms",
    ] {
        for k in ["n", "min", "p50", "p95", "p99", "max", "mean"] {
            assert!(g["hist"][h].get(k).is_some(), "hist.{h}.{k}");
        }
    }
    // The order took the gateway some time, and batches were built for the seats.
    assert!(g["hist"]["cmd_gateway_ms"]["n"].as_u64().unwrap() >= 1);
    assert!(g["hist"]["batch_build_ms"]["n"].as_u64().unwrap() >= 2);
    let seat = &g["seats"]["0"];
    for k in [
        "nick",
        "control",
        "conn",
        "stream",
        "seq",
        "view_seq",
        "view_hash",
        "basis_hash",
        "rev",
        "rx",
        "tx",
        "tx_by_turn",
        "resume",
        "rtt_ms",
        "ws_rtt_ms",
        "ring",
    ] {
        assert!(seat.get(k).is_some(), "seats.0.{k}");
    }
    assert!(seat["rx"]["msgs"].as_u64().unwrap() >= 1 && seat["rx"]["bytes"].as_u64().unwrap() > 0);
    let by_turn = seat["tx_by_turn"].as_array().unwrap();
    assert!(!by_turn.is_empty() && by_turn.len() <= 400);
    assert!(
        by_turn
            .iter()
            .any(|t| t["keyframes"].as_u64().unwrap() >= 1),
        "{by_turn:?}"
    );
    let _ = &mut bob;
}

#[test]
fn save_writes_a_save_and_the_details_list_it() {
    let (srv, _alice, _bob) = game("a-save", "echo turns=1", TimerMode::Off);
    assert_eq!(post(&srv, "/admin/games/1/save").0, 200);
    wait_until("a save in the metrics", || {
        get(&srv, "/admin/metrics").1["games"]["1"]["saves"]["ok"] == 1
    });
    let (_, m) = get(&srv, "/admin/metrics");
    let last = &m["games"]["1"]["saves"]["last"];
    assert_eq!(last["seq"], 1);
    assert!(last["bytes"].as_u64().unwrap() > 0, "{last}");
    let (status, d) = get(&srv, "/admin/games/1");
    assert_eq!(status, 200);
    assert_eq!(d["settings"]["name"], "turns");
    assert_eq!(d["seats"].as_array().unwrap().len(), 2);
    let saves = d["saves"].as_array().unwrap();
    assert_eq!(saves[0]["seq"], 1);
    assert_eq!(saves[0]["kind"], "Ring");
    assert!(saves[0]["file"].as_str().unwrap().starts_with("ring/"));
    assert_eq!(get(&srv, "/admin/games/77").0, 404);
}

#[test]
fn stop_puts_the_game_to_sleep_and_restart_wakes_it() {
    let (srv, mut alice, _bob) = game("a-stop", "echo turns=1", TimerMode::Off);
    assert_eq!(post(&srv, "/admin/games/1/stop").0, 200);
    alice.until("Detached", |m| matches!(m, ServerMsg::Detached { .. }));
    wait_until("asleep", || {
        get(&srv, "/admin/games").1["games"][0]["status"] == "Hibernating"
    });
    // A sleeping game has no worker to stop or save, but it is listed and its details exist.
    assert_eq!(post(&srv, "/admin/games/1/stop").0, 404);
    let (_, d) = get(&srv, "/admin/games/1");
    assert_eq!(d["status"], "Hibernating");
    assert!(!d["saves"].as_array().unwrap().is_empty());
    assert_eq!(
        get(&srv, "/admin/metrics").1["server"]["games"]["hibernating"],
        1
    );
    // Restart: a worker from the newest save.
    assert_eq!(post(&srv, "/admin/games/1/restart").0, 200);
    wait_until("running again", || {
        get(&srv, "/admin/games").1["games"][0]["status"] == "Running"
    });
}

fn meta_status(srv: &TestServer, game: u32) -> String {
    let m: serde_json::Value = serde_json::from_slice(
        &std::fs::read(srv.dir.join(format!("games/{game}/meta.json"))).unwrap(),
    )
    .unwrap();
    m["status"].as_str().unwrap().to_owned()
}

#[test]
fn close_ends_a_running_game_keeps_its_saves_and_frees_the_nicks() {
    let (srv, mut alice, _bob) = game("a-close", "echo turns=1", TimerMode::Off);
    assert_eq!(post(&srv, "/admin/games/77/close").0, 404);
    assert_eq!(post(&srv, "/admin/games/1/close").0, 200);
    alice.until("Detached", |m| matches!(m, ServerMsg::Detached { .. }));
    wait_until("gone from the list", || {
        get(&srv, "/admin/games").1["games"]
            .as_array()
            .unwrap()
            .is_empty()
    });
    wait_until("Finished on disk", || meta_status(&srv, 1) == "Finished");
    // The save of the closing stays in the ring; nobody sleeps, nobody is seated.
    assert!(
        std::fs::read_dir(srv.dir.join("games/1/ring"))
            .unwrap()
            .count()
            > 0
    );
    assert_eq!(post(&srv, "/admin/games/1/close").0, 404);
}

#[test]
fn close_ends_a_sleeping_game_too() {
    let (srv, mut alice, _bob) = game("a-close-sleep", "echo turns=1", TimerMode::Off);
    assert_eq!(post(&srv, "/admin/games/1/stop").0, 200);
    alice.until("Detached", |m| matches!(m, ServerMsg::Detached { .. }));
    wait_until("asleep", || {
        get(&srv, "/admin/games").1["games"][0]["status"] == "Hibernating"
    });
    assert_eq!(post(&srv, "/admin/games/1/close").0, 200);
    wait_until("gone from the list", || {
        get(&srv, "/admin/games").1["games"]
            .as_array()
            .unwrap()
            .is_empty()
    });
    assert_eq!(meta_status(&srv, 1), "Finished");
    assert!(srv.shared.games.dormant(1).is_none());
    assert_eq!(post(&srv, "/admin/games/1/restart").0, 404);
}
#[test]
fn purge_closes_the_game_deletes_its_saves_and_drops_them_from_the_lobby_list() {
    let (srv, mut alice, _bob) = game("a-purge", "echo turns=1", TimerMode::Off);
    assert_eq!(post(&srv, "/admin/games/77/purge").0, 404);
    assert_eq!(post(&srv, "/admin/games/1/save").0, 200);
    wait_until("a save on disk", || {
        srv.dir.join("games/1/ring").is_dir()
            && std::fs::read_dir(srv.dir.join("games/1/ring"))
                .unwrap()
                .count()
                > 0
    });
    // The game is still running: the purge closes it first.
    assert_eq!(post(&srv, "/admin/games/1/purge").0, 200);
    assert!(!srv.dir.join("games/1").exists(), "the directory is gone");
    assert!(srv.shared.games.get(1).is_none());
    alice.until(
        "no saves in the lobby list",
        |m| matches!(m, ServerMsg::LobbyState(s) if s.saves.is_empty()),
    );
    assert_eq!(post(&srv, "/admin/games/1/purge").0, 404);
}

#[test]
fn purge_by_nick_prefix_leaves_other_players_games_alone() {
    let (srv, _alice, _bob) = game("a-purge-nick", "echo turns=1", TimerMode::Off);
    assert_eq!(post(&srv, "/admin/purge").0, 400);
    assert_eq!(post(&srv, "/admin/purge?nick_prefix=sm").0, 400);
    let (status, body) = post(&srv, "/admin/purge?nick_prefix=smoke");
    assert_eq!(status, 200);
    assert_eq!(body["purged"].as_array().unwrap().len(), 0);
    assert!(srv.dir.join("games/1/meta.json").is_file());
}

#[test]
fn kick_by_an_administrator_needs_no_host_and_names_the_refusal() {
    let (srv, mut alice, mut bob) = game("a-kick", "echo turns=1", TimerMode::Off);
    let (status, body) = post(&srv, "/admin/games/1/kick?player=9");
    assert_eq!(
        (status, body["error"].as_str()),
        (409, Some("NoSuchPlayer"))
    );
    assert_eq!(post(&srv, "/admin/games/1/kick").0, 400);
    // The host himself can be kicked this way; the rights go to bob.
    let (status, _) = post(&srv, "/admin/games/1/kick?player=0");
    assert_eq!(status, 200);
    let ServerMsg::Detached { reason } =
        alice.until("Detached", |m| matches!(m, ServerMsg::Detached { .. }))
    else {
        unreachable!()
    };
    assert_eq!(reason, protocol::DetachReason::Kicked);
    bob.until_journal("kicked by an administrator");
    let v = bob.until_seats("bob is the host", |v| {
        v.iter().any(|s| s.host && s.player == 1)
    });
    assert!(matches!(v[0].control, protocol::SeatControl::Ai { .. }));
    // Bob is the last human now, and the seat of alice is the AI's.
    let (_, body) = post(&srv, "/admin/games/1/kick?player=1");
    assert_eq!(body["error"], "LastHuman");
    let (_, body) = post(&srv, "/admin/games/1/kick?player=0");
    assert_eq!(body["error"], "NotHuman");
}

#[test]
fn drain_saves_the_games_refuses_new_sockets_and_closes_the_old_ones_with_1012() {
    let (srv, mut alice, _bob) = game("a-drain", "echo turns=1", TimerMode::Off);
    let (status, _) = post(&srv, "/admin/drain");
    assert_eq!(status, 200);
    // Saved: the worker wrote its final save, the game is no longer running, it stays `Running` on disk.
    let dir = srv.dir.join("games/1");
    assert!(std::fs::read_dir(dir.join("ring")).map_or(0, Iterator::count) >= 1);
    let meta: Value =
        serde_json::from_slice(&std::fs::read(dir.join("meta.json")).unwrap()).unwrap();
    assert_eq!(meta["status"], "Running");
    // New sockets are told to come back later.
    let refused = tungstenite::connect(format!("ws://{}/ws", srv.addr));
    match refused {
        Err(tungstenite::Error::Http(r)) => {
            assert_eq!(r.status().as_u16(), 503);
            assert_eq!(r.headers().get("retry-after").unwrap(), "5");
        }
        other => panic!("expected a 503, got {:?}", other.map(|_| ())),
    }
    // The old ones are closed with "service restart".
    let code = loop {
        match alice.ws.read() {
            Ok(Message::Close(Some(f))) => break u16::from(f.code),
            Ok(_) => {}
            Err(e) => panic!("no close frame: {e}"),
        }
    };
    assert_eq!(code, 1012);
}

#[test]
fn a_drain_with_exit_asks_the_process_to_end() {
    let srv = start("a-drain-exit", "echo turns=1");
    let shared = srv.shared.clone();
    let waiter = std::thread::spawn({
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        move || {
            rt.block_on(async {
                tokio::time::timeout(Duration::from_secs(10), shared.exit.notified())
                    .await
                    .is_ok()
            })
        }
    });
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(post(&srv, "/admin/drain?exit=1").0, 200);
    assert!(waiter.join().unwrap(), "the exit was not asked for");
}

#[test]
fn the_test_hooks_kill_the_worker_and_the_game_comes_back() {
    let (srv, _alice, _bob) = game_with("a-kill", "echo turns=1", TimerMode::Off, hooks);
    assert_eq!(post(&srv, "/admin/games/1/save").0, 200);
    wait_until("a save", || {
        get(&srv, "/admin/metrics").1["games"]["1"]["saves"]["ok"] == 1
    });
    let pid = get(&srv, "/admin/metrics").1["games"]["1"]["worker"]["pid"]
        .as_u64()
        .unwrap();
    assert_eq!(post(&srv, "/admin/games/1/kill-worker?signal=KILL").0, 200);
    wait_until("a new worker", || {
        let g = &get(&srv, "/admin/metrics").1["games"]["1"];
        g["worker"]["crashes"] == 1
            && g["status"] == "Running"
            && g["worker"]["pid"].as_u64().is_some_and(|p| p != pid)
    });
    let m = &get(&srv, "/admin/metrics").1["games"]["1"];
    assert_eq!(m["worker"]["restart_ms"].as_array().unwrap().len(), 1);
    assert_eq!(m["hist"]["restore_ms"]["n"], 1);
    assert_eq!(post(&srv, "/admin/games/1/kill-worker?signal=TERM").0, 400);
}

#[test]
fn the_test_hooks_refuse_an_order_drop_a_batch_hold_batches_and_close_a_socket() {
    let (srv, mut alice, mut bob) = game_with("a-faults", "echo turns=1", TimerMode::Off, hooks);
    let fault = |kind: &str, seat: i8, value: Option<u64>| {
        let body = match value {
            Some(v) => format!(r#"{{"seat":{seat},"kind":"{kind}","value":{v}}}"#),
            None => format!(r#"{{"seat":{seat},"kind":"{kind}"}}"#),
        };
        call(&srv, "POST", "/admin/games/1/fault", Some(ADMIN), &body).0
    };
    assert_eq!(fault("nonsense", 0, None), 400);
    // `reject_next_cmd`: the next order is refused, the one after is not.
    assert_eq!(fault("reject_next_cmd", 0, None), 200);
    std::thread::sleep(Duration::from_millis(200));
    let id = alice.order(1, Command::UnitSleep { unit: 1 });
    assert!(matches!(alice.answer(id), CommandOutcome::Rejected { .. }));
    let id = alice.order(1, Command::UnitSleep { unit: 1 });
    assert_eq!(alice.answer(id), CommandOutcome::Applied);
    // `drop_batch`: a number of the stream is used up and never arrives.
    let seq_before = {
        let (_, m) = get(&srv, "/admin/metrics");
        m["games"]["1"]["seats"]["0"]["seq"].as_u64().unwrap()
    };
    assert_eq!(fault("drop_batch", 0, None), 200);
    std::thread::sleep(Duration::from_millis(200));
    // Two orders: the first batch is dropped, the second one arrives with a number missing before it.
    for _ in 0..2 {
        alice.order(1, Command::UnitSleep { unit: 1 });
        std::thread::sleep(Duration::from_millis(100));
    }
    let first = loop {
        if let ServerMsg::Game(b) = alice.recv() {
            break b.seq;
        }
    };
    assert_eq!(first, seq_before + 2, "one number is missing");
    // `hold_batches_ms`: the pipeline of bob stands still.
    assert_eq!(fault("hold_batches_ms", 1, Some(700)), 200);
    std::thread::sleep(Duration::from_millis(100));
    let t = Instant::now();
    let id = bob.order(1, Command::UnitSleep { unit: 1 });
    let _ = id;
    bob.until("a batch after the hold", |m| {
        matches!(m, ServerMsg::Game(_))
    });
    assert!(
        t.elapsed() >= Duration::from_millis(400),
        "{:?}",
        t.elapsed()
    );
    // `close_socket`: alice is dropped from the game (a seat in `Lost`) and the socket closes.
    assert_eq!(fault("close_socket", 0, None), 200);
    let code = loop {
        match alice.ws.read() {
            Ok(Message::Close(Some(f))) => break u16::from(f.code),
            Ok(_) => {}
            Err(_) => break 0,
        }
    };
    assert!(code == 1013 || code == 0, "{code}");
}
