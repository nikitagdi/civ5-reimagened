//! The turn state, the turn timer and the host's pause (9 `turn`) over real sockets with
//! the mock worker in its `turns=1` mode and a `TestClock`: start of the timers, pause and resume, a
//! seat that is not online stops its clock, expiry sends `ForceEndTurn` for the seats of the group only,
//! hybrid groups, the war rule, `Fixed` and `Original` with a zero length, a pause by somebody who is not
//! the host, the take-back of an end of turn (NP-D23). Seats (G6 part 2): a lost connection shows after
//! 3 s, the host's kick (rights, refusals, the retry after 20 s), `Leave` as a disconnect and the way
//! back, the host that is away for 60 s passes the rights on.

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

fn fixed(seconds: u16) -> TimerMode {
    TimerMode::Fixed { seconds }
}

// ---------------------------------------------------------------- the timer

#[test]
fn the_timer_starts_with_the_turn_when_the_seats_are_online() {
    let (_srv, mut alice, mut bob) = game("t-start", "echo turns=1", fixed(10));
    for c in [&mut alice, &mut bob] {
        let t = c.until_turn("a running timer", |t| {
            t.timers.len() == 1 && t.timers[0].left_ms_when_stopped.is_none()
        });
        assert_eq!((t.turn, t.paused), (1, false));
        assert_eq!(t.timers[0].group, 0);
        assert_eq!(t.timers[0].len_ms, 10_000);
        assert_eq!(t.timers[0].deadline_ms, 10_000, "{}", c.nick);
        assert!(t.seats.iter().all(|s| s.group == 0 && s.may_act));
    }
}

#[test]
fn the_host_pauses_and_resumes_only_the_timer_and_somebody_else_is_refused() {
    let (srv, mut alice, mut bob) = game("t-pause", "echo turns=1", fixed(10));
    alice.until_turn("running", |t| {
        t.timers.len() == 1 && t.timers[0].left_ms_when_stopped.is_none()
    });
    srv.clock.advance(3_000);
    alice.ctl(GameCtl::Pause(true));
    let t = bob.until_turn("paused", |t| t.paused);
    assert_eq!(t.timers[0].left_ms_when_stopped, Some(7_000));
    alice.until_journal("paused");
    // Orders go on while it stands, and the clock stands however long it is.
    let id = alice.order(1, Command::UnitSleep { unit: 1 });
    assert_eq!(alice.answer(id), CommandOutcome::Applied);
    srv.clock.advance(100_000);
    std::thread::sleep(Duration::from_millis(400));
    // Bob is not the host.
    bob.ctl(GameCtl::Pause(false));
    let ServerMsg::GameCtlRejected { ctl, reason } = bob.until("a refusal", |m| {
        matches!(m, ServerMsg::GameCtlRejected { .. })
    }) else {
        unreachable!()
    };
    assert_eq!(
        (ctl, reason),
        (GameCtl::Pause(false), GameCtlReason::NotHost)
    );
    // The host resumes: 7 s are left from now.
    alice.ctl(GameCtl::Pause(false));
    let t = bob.until_turn("resumed", |t| !t.paused);
    assert_eq!(t.timers[0].left_ms_when_stopped, None);
    assert_eq!(t.timers[0].deadline_ms, 103_000 + 7_000);
    alice.until_journal("resumed");
    let m = timer_metrics(&srv);
    assert_eq!(m["paused_ms"], 100_000);
    assert_eq!(m["expired"], 0);
    let all = http_get(srv.admin, "/admin/metrics");
    assert_eq!(all["games"]["1"]["gamectl_rejected"]["NotHost"], 1);
}

#[test]
fn the_pause_without_a_timer_is_a_flag_and_a_zero_length_row_has_no_timer() {
    // `Original` whose row has length 0 for this turn: no timer.
    let (_srv, mut alice, mut bob) = game(
        "t-zero",
        "echo turns=1 len=0 base=0",
        TimerMode::Original { row: "FAST".into() },
    );
    assert!(alice.turn.as_ref().unwrap().timers.is_empty());
    alice.ctl(GameCtl::Pause(true));
    let t = bob.until_turn("the flag", |t| t.paused);
    assert!(t.timers.is_empty());
    alice.ctl(GameCtl::Pause(false));
    bob.until_turn("the flag off", |t| !t.paused);
}

#[test]
fn a_pause_outside_a_game_is_refused() {
    let srv = start("t-nogame", "echo turns=1");
    let mut c = Client::login(&srv, "carol");
    c.until("the lobby", |m| matches!(m, ServerMsg::LobbyState(_)));
    c.ctl(GameCtl::Pause(true));
    let ServerMsg::GameCtlRejected { reason, .. } = c.until("a refusal", |m| {
        matches!(m, ServerMsg::GameCtlRejected { .. })
    }) else {
        unreachable!()
    };
    assert_eq!(reason, GameCtlReason::NoGame);
}

#[test]
fn a_seat_that_is_gone_stops_the_clock_of_its_group_and_it_runs_on_when_the_seat_returns() {
    let (srv, mut alice, bob) = game("t-lost", "echo turns=1", fixed(10));
    alice.until_turn("running", |t| {
        t.timers.len() == 1 && t.timers[0].left_ms_when_stopped.is_none()
    });
    srv.clock.advance(3_000);
    let token = bob.token;
    drop(bob);
    let t = alice.until_turn("stopped", |t| {
        t.timers
            .first()
            .is_some_and(|x| x.left_ms_when_stopped.is_some())
    });
    assert_eq!(t.timers[0].left_ms_when_stopped, Some(7_000));
    // Nothing runs out while he is away.
    srv.clock.advance(50_000);
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(timer_metrics(&srv)["expired"], 0);
    let mut bob = Client::connect(&srv, "bob", "", token);
    bob.until("Attached", |m| matches!(m, ServerMsg::Attached(_)));
    let t = alice.until_turn("running again", |t| {
        t.timers
            .first()
            .is_some_and(|x| x.left_ms_when_stopped.is_none())
    });
    assert_eq!(t.timers[0].deadline_ms, 53_000 + 7_000);
}

#[test]
fn the_timer_running_out_ends_the_turn_for_the_seats_that_have_not_finished() {
    let (mode, log) = mode_with_log("expire", "");
    let (srv, mut alice, mut bob) = game("t-expire", &mode, fixed(10));
    let id = alice.order(1, Command::EndTurn { force: false });
    assert_eq!(alice.answer(id), CommandOutcome::Applied);
    srv.clock.advance(10_001);
    // Only bob is forced (the group is "seats that may act and are not finished").
    let lines = wait_force(&log, 1);
    assert_eq!(lines, ["[1]"]);
    let line = alice.until_journal("timer ran out");
    assert!(line.contains("bob"), "{line}");
    // The turn moved on and a new timer started from now.
    let t = bob.until_turn("turn 2 with a timer", |t| {
        t.turn == 2 && t.timers.len() == 1
    });
    assert_eq!(t.timers[0].deadline_ms, 10_001 + 10_000);
    assert_eq!(force_lines(&log).len(), 1);
    let m = timer_metrics(&srv);
    assert_eq!(
        (m["started"].as_u64(), m["expired"].as_u64()),
        (Some(2), Some(1))
    );
}

#[test]
fn a_force_the_turn_does_not_obey_is_repeated_three_times_then_given_up() {
    let (mode, log) = mode_with_log("retry", "ignore_force=1");
    let (srv, mut alice, _bob) = game("t-retry", &mode, fixed(10));
    srv.clock.advance(10_000);
    wait_force(&log, 1);
    srv.clock.advance(4_000);
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(force_lines(&log).len(), 1, "not before five seconds");
    srv.clock.advance(1_000);
    wait_force(&log, 2);
    srv.clock.advance(5_000);
    wait_force(&log, 3);
    srv.clock.advance(5_000);
    let line = alice.until_journal("could not end the turn");
    assert!(line.contains("alice"), "{line}");
    srv.clock.advance(60_000);
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(force_lines(&log).len(), 3);
}

#[test]
fn hybrid_groups_run_side_by_side_and_each_runs_out_for_its_own_seats() {
    // Seat 0 moves at once, seat 1 as a sequential seat: two groups, both may act.
    let (mode, log) = mode_with_log("parallel", "seq=1");
    let (srv, mut alice, _bob) = game("t-parallel", &mode, fixed(10));
    let t = alice.until_turn("two timers", |t| {
        t.timers.len() == 2 && t.timers.iter().all(|x| x.left_ms_when_stopped.is_none())
    });
    assert_eq!(t.timers.iter().map(|x| x.group).collect::<Vec<_>>(), [0, 1]);
    assert_eq!(
        t.seats
            .iter()
            .map(|s| (s.player, s.group))
            .collect::<Vec<_>>(),
        [(0, 0), (1, 1)]
    );
    srv.clock.advance(10_000);
    let mut lines = wait_force(&log, 2);
    lines.sort();
    assert_eq!(lines, ["[0]", "[1]"]);
}

#[test]
fn a_sequential_seat_gets_a_fresh_timer_when_its_turn_to_act_comes() {
    // Seats 0 then 1, one after the other: groups 1 and 2.
    let (mode, log) = mode_with_log("sequence", "seq=0,1");
    let (srv, mut alice, mut bob) = game("t-sequence", &mode, fixed(10));
    let t = alice.until_turn("the first group", |t| t.timers.len() == 1);
    assert_eq!((t.timers[0].group, t.timers[0].deadline_ms), (1, 10_000));
    assert_eq!(
        t.seats
            .iter()
            .map(|s| (s.player, s.may_act))
            .collect::<Vec<_>>(),
        [(0, true), (1, false)]
    );
    // Alice ends her turn after 4 s: bob's group starts then, with a full length.
    srv.clock.advance(4_000);
    let id = alice.order(1, Command::EndTurn { force: false });
    assert_eq!(alice.answer(id), CommandOutcome::Applied);
    let t = bob.until_turn("the second group", |t| {
        t.timers.len() == 1 && t.timers[0].group == 2
    });
    assert_eq!(t.timers[0].deadline_ms, 14_000);
    srv.clock.advance(10_000);
    assert_eq!(wait_force(&log, 1), ["[1]"]);
}

// ---------------------------------------------------------------- the war rule

fn original() -> TimerMode {
    TimerMode::Original { row: "FAST".into() }
}

#[test]
fn a_war_of_humans_moves_the_timer_when_less_than_the_base_time_is_left() {
    let (srv, mut alice, mut bob) = game("t-war", "echo turns=1 len=60 base=20", original());
    let t = alice.until_turn("a timer", |t| t.timers.len() == 1);
    assert_eq!(t.timers[0].deadline_ms, 60_000);
    // 50 s are left: more than the base time, nothing moves.
    srv.clock.advance(10_000);
    let id = alice.order(1, Command::DeclareWar { team: 1 });
    assert_eq!(alice.answer(id), CommandOutcome::Applied);
    assert_eq!(timer_metrics(&srv)["extended"], 0);
    assert_eq!(alice.turn.as_ref().unwrap().timers[0].deadline_ms, 60_000);
    // 10 s are left: 10 s are added to every timer.
    srv.clock.advance(40_000);
    let id = alice.order(1, Command::DeclareWar { team: 1 });
    assert_eq!(alice.answer(id), CommandOutcome::Applied);
    let t = bob.until_turn("the moved timer", |t| {
        t.timers.first().is_some_and(|x| x.deadline_ms == 70_000)
    });
    assert_eq!(t.timers[0].len_ms, 60_000);
    assert_eq!(timer_metrics(&srv)["extended"], 1);
}

#[test]
fn a_fixed_timer_is_not_extended_by_a_war() {
    let (srv, mut alice, _bob) = game("t-fixed-war", "echo turns=1 len=60 base=20", fixed(60));
    alice.until_turn("a timer", |t| t.timers.len() == 1);
    srv.clock.advance(55_000);
    let id = alice.order(1, Command::DeclareWar { team: 1 });
    assert_eq!(alice.answer(id), CommandOutcome::Applied);
    assert_eq!(timer_metrics(&srv)["extended"], 0);
    assert_eq!(alice.turn.as_ref().unwrap().timers[0].deadline_ms, 60_000);
}

// ---------------------------------------------------------------- the end of turn taken back

#[test]
fn taking_the_end_of_turn_back_sends_the_turn_state_to_all_and_to_the_seat_with_its_results() {
    let (_srv, mut alice, mut bob) = game("t-cancel", "echo turns=1", TimerMode::Off);
    let end = alice.order(1, Command::EndTurn { force: false });
    assert_eq!(alice.answer(end), CommandOutcome::Applied);
    let t = bob.until_turn("alice finished", |t| {
        t.seats.iter().any(|s| s.player == 0 && s.finished)
    });
    assert_eq!(t.turn, 1);
    let cancel = alice.order(1, Command::CancelEndTurn);
    // The seat of the one who gave the order: the results and the turn state are one batch.
    let found = loop {
        let ServerMsg::Game(b) = alice.until("a batch", |m| matches!(m, ServerMsg::Game(_))) else {
            unreachable!()
        };
        let has_result = b.msgs.iter().any(|m| matches!(m,
            GameMsg::Results(rs) if rs.iter().any(|r| r.id == cancel && r.outcome == CommandOutcome::Applied)));
        if has_result {
            break b;
        }
    };
    let state = found.msgs.iter().find_map(|m| match m {
        GameMsg::TurnState(t) => Some(t),
        _ => None,
    });
    let state = state.expect("the turn state in the batch of the results");
    assert!(state.seats.iter().all(|s| !s.finished));
    bob.until_turn("alice not finished", |t| {
        t.seats.iter().all(|s| !s.finished)
    });
    assert_eq!(alice.turn_no(), 1);
}

// ---------------------------------------------------------------- seats: lost, kick, leave, host

fn conn_of(v: &[SeatView], player: i8) -> Conn {
    v.iter().find(|s| s.player == player).unwrap().conn
}

fn host_of(v: &[SeatView]) -> Option<i8> {
    v.iter().find(|s| s.host).map(|s| s.player)
}

/// Sleeps a little so the actor's 100 ms tick has looked at the clock.
fn tick() {
    std::thread::sleep(Duration::from_millis(400));
}

#[test]
fn a_lost_connection_shows_to_the_others_after_three_seconds_not_before() {
    let (srv, mut alice, bob) = game("s-lost", "echo turns=1", TimerMode::Off);
    alice.until_seats("both online", |v| {
        v.iter().all(|s| matches!(s.conn, Conn::Online))
    });
    drop(bob);
    // The gateway has to see the socket close before the clock moves.
    tick();
    srv.clock.advance(2_900);
    tick();
    // Nothing was sent: the seat still shows as up.
    assert!(matches!(
        conn_of(alice.seats.as_ref().unwrap(), 1),
        Conn::Online
    ));
    srv.clock.advance(200);
    let v = alice.until_seats("bob lost", |v| matches!(conn_of(v, 1), Conn::Lost { .. }));
    let Conn::Lost { since_ms } = conn_of(&v, 1) else {
        unreachable!()
    };
    assert_eq!(since_ms, 0);
}

#[test]
fn a_reload_within_three_seconds_shows_no_gap() {
    let (srv, mut alice, bob) = game("s-blink", "echo turns=1", TimerMode::Off);
    alice.until_seats("online", |v| matches!(conn_of(v, 1), Conn::Online));
    let token = bob.token;
    drop(bob);
    srv.clock.advance(1_000);
    let mut bob = Client::connect(&srv, "bob", "", token);
    bob.until("Attached", |m| matches!(m, ServerMsg::Attached(_)));
    srv.clock.advance(5_000);
    tick();
    // Whatever alice got meanwhile, bob never showed as lost.
    while let Some(m) = alice_try(&mut alice) {
        if let ServerMsg::Game(b) = m {
            for g in b.msgs {
                if let GameMsg::Seats(v) = g {
                    assert!(matches!(conn_of(&v, 1), Conn::Online), "{v:?}");
                }
            }
        }
    }
}

/// The next message if one is there within a moment.
fn alice_try(c: &mut Client) -> Option<ServerMsg> {
    if let MaybeTlsStream::Plain(s) = c.ws.get_mut() {
        s.set_read_timeout(Some(Duration::from_millis(300))).ok()?;
    }
    let r = match c.ws.read() {
        Ok(Message::Binary(b)) => {
            let msg: ServerMsg = wire::decode(&b, &Limits::CLIENT).unwrap();
            Some(msg)
        }
        _ => None,
    };
    if let MaybeTlsStream::Plain(s) = c.ws.get_mut() {
        let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
    }
    r
}

#[test]
fn the_host_kicks_a_player_and_the_seat_goes_to_the_ai() {
    let (srv, mut alice, mut bob) = game("s-kick", "echo turns=1", fixed(10));
    // Somebody who is not the host, and refusals in the order of the checks.
    bob.ctl(GameCtl::Kick { player: 0 });
    let ServerMsg::GameCtlRejected { reason, .. } = bob.until("NotHost", |m| {
        matches!(m, ServerMsg::GameCtlRejected { .. })
    }) else {
        unreachable!()
    };
    assert_eq!(reason, GameCtlReason::NotHost);
    for (target, want) in [
        (0, GameCtlReason::SelfKick),
        (9, GameCtlReason::NoSuchPlayer),
    ] {
        alice.ctl(GameCtl::Kick { player: target });
        let ServerMsg::GameCtlRejected { reason, .. } = alice.until("a refusal", |m| {
            matches!(m, ServerMsg::GameCtlRejected { .. })
        }) else {
            unreachable!()
        };
        assert_eq!(reason, want);
    }
    alice.ctl(GameCtl::Kick { player: 1 });
    // Bob is told why, and the lobby follows.
    let ServerMsg::Detached { reason } =
        bob.until("Detached", |m| matches!(m, ServerMsg::Detached { .. }))
    else {
        unreachable!()
    };
    assert_eq!(reason, DetachReason::Kicked);
    let ServerMsg::LobbyState(s) = bob.recv() else {
        panic!("the lobby view has to follow Detached");
    };
    assert!(s.room.is_none());
    // The line comes first, then the seats.
    alice.until_journal("alice kicked bob");
    let v = alice.until_seats(
        "bob kicked",
        |v| matches!(&v[1].control, SeatControl::Ai { kicked: Some(n) } if n == "bob"),
    );
    assert_eq!(host_of(&v), Some(0));
    // The seat is the AI's: a second kick says so.
    alice.ctl(GameCtl::Kick { player: 1 });
    let ServerMsg::GameCtlRejected { reason, .. } = alice.until("NotHuman", |m| {
        matches!(m, ServerMsg::GameCtlRejected { .. })
    }) else {
        unreachable!()
    };
    assert_eq!(reason, GameCtlReason::NotHuman);
    let all = http_get(srv.admin, "/admin/metrics");
    let m = &all["games"]["1"]["gamectl_rejected"];
    assert_eq!(
        (
            m["NotHost"].as_u64(),
            m["SelfKick"].as_u64(),
            m["NoSuchPlayer"].as_u64(),
            m["NotHuman"].as_u64()
        ),
        (Some(1), Some(1), Some(1), Some(1))
    );
    // The kicked nick is free again: bob can make a room.
    bob.send(&ClientMsg::Lobby(LobbyCmd::CreateRoom {
        settings: settings(TimerMode::Off),
    }));
    bob.until(
        "his own room",
        |m| matches!(m, ServerMsg::LobbyState(s) if s.room.is_some()),
    );
}

#[test]
fn a_kick_the_worker_does_not_answer_is_asked_again_after_twenty_seconds() {
    let (srv, mut alice, mut bob) =
        game("s-kick-retry", "echo turns=1 kick=ignore", TimerMode::Off);
    alice.ctl(GameCtl::Kick { player: 1 });
    tick();
    srv.clock.advance(19_000);
    tick();
    srv.clock.advance(1_500);
    let line = alice.until_journal("not carried out");
    assert!(line.contains("bob"), "{line}");
    // The mock ignores only the first kick: the repeat goes through.
    let ServerMsg::Detached { reason } =
        bob.until("Detached", |m| matches!(m, ServerMsg::Detached { .. }))
    else {
        unreachable!()
    };
    assert_eq!(reason, DetachReason::Kicked);
}

#[test]
fn a_kick_the_core_refuses_leaves_the_seat_alone() {
    let (_srv, mut alice, _bob) =
        game("s-kick-refused", "echo turns=1 kick=refuse", TimerMode::Off);
    alice.ctl(GameCtl::Kick { player: 1 });
    let line = alice.until_journal("refused");
    assert!(line.contains("core refuses"), "{line}");
    assert!(matches!(
        alice.seats.as_ref().unwrap()[1].control,
        SeatControl::Human { .. }
    ));
}

#[test]
fn leave_is_a_disconnect_the_seat_stays_and_the_player_returns_with_join_game() {
    let (srv, mut alice, mut bob) = game("s-leave", "echo turns=1", fixed(10));
    alice.until_turn("running", |t| {
        t.timers.len() == 1 && t.timers[0].left_ms_when_stopped.is_none()
    });
    srv.clock.advance(2_000);
    bob.ctl(GameCtl::Leave);
    let ServerMsg::Detached { reason } =
        bob.until("Detached", |m| matches!(m, ServerMsg::Detached { .. }))
    else {
        unreachable!()
    };
    assert_eq!(reason, DetachReason::Left);
    // The lobby view follows, with the seat takeable for bob.
    let ServerMsg::LobbyState(s) = bob.recv() else {
        panic!("the lobby view has to follow Detached");
    };
    let seat = &s.games[0].seats[1];
    assert!(seat.takeable && matches!(&seat.control, SeatControl::Human { nick } if nick == "bob"));
    // The clock of the group stands for him, and nothing runs out meanwhile.
    let t = alice.until_turn("stopped", |t| {
        t.timers
            .first()
            .is_some_and(|x| x.left_ms_when_stopped.is_some())
    });
    assert_eq!(t.timers[0].left_ms_when_stopped, Some(8_000));
    srv.clock.advance(100_000);
    tick();
    assert_eq!(timer_metrics(&srv)["expired"], 0);
    alice.until_seats("bob lost", |v| matches!(conn_of(v, 1), Conn::Lost { .. }));
    // Another game or room is not for him while the seat is his.
    bob.send(&ClientMsg::Lobby(LobbyCmd::CreateRoom {
        settings: settings(TimerMode::Off),
    }));
    bob.until("a refusal", |m| matches!(m, ServerMsg::LobbyError { .. }));
    // Back on the same seat, no claim.
    bob.send(&ClientMsg::Lobby(LobbyCmd::JoinGame {
        game: 1,
        player: 1,
        basis: None,
    }));
    let ServerMsg::Attached(a) = bob.until("Attached", |m| matches!(m, ServerMsg::Attached(_)))
    else {
        unreachable!()
    };
    assert_eq!(a.player, 1);
    alice.until_seats("bob online", |v| matches!(conn_of(v, 1), Conn::Online));
}

#[test]
fn a_host_who_is_away_for_sixty_seconds_hands_over_and_comes_back_as_a_player() {
    let (srv, alice, mut bob) = game("s-host", "echo turns=1", TimerMode::Off);
    bob.until_seats("both online", |v| {
        v.iter().all(|s| matches!(s.conn, Conn::Online))
    });
    let token = alice.token;
    drop(alice);
    tick();
    srv.clock.advance(59_000);
    tick();
    assert_eq!(host_of(bob.seats.as_ref().unwrap()), Some(0));
    srv.clock.advance(1_500);
    bob.until_journal("bob is the host now");
    let v = bob.until_seats("bob is the host", |v| host_of(v) == Some(1));
    assert!(matches!(conn_of(&v, 0), Conn::Lost { .. }));
    let list = http_get(srv.admin, "/admin/games");
    assert_eq!(list["games"][0]["host"], "bob");
    let all = http_get(srv.admin, "/admin/metrics");
    let m = &all["games"]["1"]["host"];
    assert_eq!(m["changes"], 1);
    assert!(m["last_change_after_lost_ms"].as_u64().unwrap() >= 60_000);
    // Alice returns: a player, not the host; her pause is refused.
    let mut alice = Client::connect(&srv, "alice", "", token);
    alice.until("Attached", |m| matches!(m, ServerMsg::Attached(_)));
    let v = alice.until_seats("alice online", |v| matches!(conn_of(v, 0), Conn::Online));
    assert_eq!(host_of(&v), Some(1));
    alice.ctl(GameCtl::Pause(true));
    let ServerMsg::GameCtlRejected { reason, .. } = alice.until("NotHost", |m| {
        matches!(m, ServerMsg::GameCtlRejected { .. })
    }) else {
        unreachable!()
    };
    assert_eq!(reason, GameCtlReason::NotHost);
    // The new host can pause.
    bob.ctl(GameCtl::Pause(true));
    alice.until_turn("paused", |t| t.paused);
}

#[test]
fn the_rights_stay_with_a_lost_host_while_nobody_else_is_online() {
    let (srv, alice, bob) = game("s-host-alone", "echo turns=1", TimerMode::Off);
    let token_b = bob.token;
    drop(alice);
    drop(bob);
    tick();
    srv.clock.advance(120_000);
    tick();
    let mut bob = Client::connect(&srv, "bob", "", token_b);
    bob.until("Attached", |m| matches!(m, ServerMsg::Attached(_)));
    // Alice has been away for two minutes: the rights pass as soon as bob is here.
    let v = bob.until_seats("bob is the host", |v| host_of(v) == Some(1));
    assert!(matches!(conn_of(&v, 0), Conn::Lost { .. }));
}

fn play_to_the_end(name: &str) -> (TestServer, Client, Client) {
    let (srv, mut alice, mut bob) = game(name, "echo turns=1 over=2", TimerMode::Off);
    for c in [&mut alice, &mut bob] {
        let id = c.order(1, Command::EndTurn { force: false });
        assert_eq!(c.answer(id), CommandOutcome::Applied);
    }
    alice.until_journal("game is over");
    (srv, alice, bob)
}

fn game_status(srv: &TestServer) -> String {
    http_get(srv.admin, "/admin/games")["games"]
        .as_array()
        .and_then(|g| g.first())
        .map_or_else(
            || "gone".to_owned(),
            |g| g["lobby_status"].as_str().unwrap_or("?").to_owned(),
        )
}

#[test]
fn the_host_returning_to_the_main_menu_after_the_end_closes_the_game_for_everybody() {
    let (srv, mut alice, mut bob) = play_to_the_end("s-over-host");
    // Bob (not the host) leaving does not close it while the host is still looking.
    bob.ctl(GameCtl::Leave);
    bob.until("Detached", |m| matches!(m, ServerMsg::Detached { .. }));
    srv.clock.advance(1_000);
    tick();
    assert_eq!(game_status(&srv), "Ended");
    // The host presses "return to main menu": the game is closed, the other seat is let go too.
    alice.ctl(GameCtl::Leave);
    alice.until("Detached", |m| matches!(m, ServerMsg::Detached { .. }));
    for _ in 0..100 {
        if game_status(&srv) == "gone" {
            break;
        }
        tick();
    }
    assert_eq!(game_status(&srv), "gone");
}

#[test]
fn a_finished_game_whose_players_all_dropped_closes_soon_and_is_not_left_running() {
    let (srv, alice, bob) = play_to_the_end("s-over-empty");
    drop(alice);
    drop(bob);
    tick();
    srv.clock.advance(20_000);
    for _ in 0..100 {
        if game_status(&srv) == "gone" {
            break;
        }
        tick();
    }
    assert_eq!(game_status(&srv), "gone");
}

#[test]
fn a_finished_game_stays_open_while_a_player_is_still_online() {
    let (srv, alice, bob) = play_to_the_end("s-over-online");
    drop(bob);
    tick();
    srv.clock.advance(60_000);
    tick();
    assert_eq!(game_status(&srv), "Ended");
    drop(alice);
}

#[test]
fn a_finished_game_is_saved_listed_as_ended_and_closes_after_five_quiet_minutes() {
    let (srv, mut alice, mut bob) = game("s-over", "echo turns=1 over=2", TimerMode::Off);
    for c in [&mut alice, &mut bob] {
        let id = c.order(1, Command::EndTurn { force: false });
        assert_eq!(c.answer(id), CommandOutcome::Applied);
    }
    alice.until_journal("game is over");
    let m = http_get(srv.admin, "/admin/metrics");
    // The worker was asked for a save at the end.
    for _ in 0..40 {
        if http_get(srv.admin, "/admin/metrics")["games"]["1"]["saves"]["ok"] == 1 {
            break;
        }
        tick();
    }
    assert_eq!(
        http_get(srv.admin, "/admin/games")["games"][0]["lobby_status"],
        "Ended"
    );
    let _ = m;
    // Still connected: the final view can be looked at. Five quiet minutes later the seats are let go.
    srv.clock.advance(5 * 60 * 1000 + 1);
    let ServerMsg::Detached { reason } =
        bob.until("Detached", |m| matches!(m, ServerMsg::Detached { .. }))
    else {
        unreachable!()
    };
    assert_eq!(reason, DetachReason::GameOver);
    let ServerMsg::LobbyState(s) = bob.recv() else {
        panic!("the lobby view has to follow Detached");
    };
    assert!(s.room.is_none());
    // The end is for everybody (DS-0016): the other human is let go with the same reason.
    let ServerMsg::Detached { reason } =
        alice.until("Detached", |m| matches!(m, ServerMsg::Detached { .. }))
    else {
        unreachable!()
    };
    assert_eq!(reason, DetachReason::GameOver);
}
