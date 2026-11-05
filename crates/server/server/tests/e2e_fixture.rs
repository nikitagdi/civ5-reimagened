//! The M1 test (without kick, restart and drain): the gateway process with the real
//! `worker` child on the test world, two clients on `tungstenite`. They log in, make a room, start a game,
//! play 20 turns with orders; each client keeps its own copy of the view from the stream and checks every
//! hash. One client drops and resumes from the ring, later reloads (token only, a `Fresh`). At the end the
//! copies equal what the gateway holds, and the counters say nothing was lost.
//!
//! The second test (G5 part 2, G6 part 1): a third player takes the AI slot of the running game with
//! `JoinGame` (a `Claim` at the worker's safe point), and the turn timer (`Fixed`, 10 s) ends the turn for
//! the one of the three who does not move: `ForceEndTurn` reaches the real worker.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use protocol::{
    AttachMode, AutoChoices, BasisClaim, BasisState, ChatLine, ChatSend, ChatTo, ClientMsg, Cmd,
    Codec, Command as Order, CommandOutcome, DetachReason, GameCtl, GameMsg, GameSettings,
    LobbyCmd, Login, Occupant, PROTOCOL_VERSION, Postcard, Resume, ServerMsg, SlotOccupant,
    TimerMode, Token, TurnMode, TurnPhase, TurnState, UndoLog, ViewHash, WorldView, apply,
    check_pair,
};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};
use wire::{Encoder, Limits};

mod common;

const BIN: &str = env!("CARGO_BIN_EXE_civ5-server");
const TURNS: i32 = 20;

struct Gateway {
    child: Child,
    extra: Vec<String>,
    dir: PathBuf,
    listen: String,
    admin: String,
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// A directory per gateway: the tests of this file run in parallel.
static GATEWAYS: AtomicU32 = AtomicU32::new(0);

fn start_gateway() -> Gateway {
    start_gateway_with(&[])
}

/// A gateway with more flags (`--config`, `--test-hooks`, ...).
fn start_gateway_with(extra: &[&str]) -> Gateway {
    let dir = std::env::temp_dir().join(format!(
        "civ5-e2e-fixture-{}-{}",
        std::process::id(),
        GATEWAYS.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let listen = format!("127.0.0.1:{}", free_port());
    let admin = format!("127.0.0.1:{}", free_port());
    let extra: Vec<String> = extra.iter().map(|s| (*s).to_string()).collect();
    let child = spawn_gateway(&dir, &listen, &admin, &extra);
    let gw = Gateway {
        child,
        extra,
        dir,
        listen,
        admin,
    };
    wait_healthy(&gw);
    gw
}

fn spawn_gateway(dir: &PathBuf, listen: &str, admin: &str, extra: &[String]) -> Child {
    Command::new(BIN)
        .args(["--dev", "--core", "fixture", "--listen", listen])
        .args(["--admin-listen", admin, "--state-dir"])
        .arg(dir)
        .args(extra)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

impl Gateway {
    /// The drain with `exit=1`: the games are saved, the process ends; then the same state
    /// directory and ports get a new process.
    fn drain_and_restart(&mut self) {
        let (code, _) = admin_call(self, "POST", "/admin/drain?exit=1");
        assert_eq!(code, 200);
        let deadline = Instant::now() + Duration::from_secs(40);
        while self.child.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "the gateway did not exit");
            std::thread::sleep(Duration::from_millis(100));
        }
        self.child = spawn_gateway(&self.dir, &self.listen, &self.admin, &self.extra);
        wait_healthy(self);
    }
}

/// `/healthz` is 200 once the `Describe` pass is in.
fn wait_healthy(gw: &Gateway) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let ok = Command::new(BIN)
            .args([
                "healthcheck",
                "--dev",
                "--core",
                "fixture",
                "--listen",
                &gw.listen,
            ])
            .arg("--state-dir")
            .arg(&gw.dir)
            .output()
            .unwrap()
            .status
            .success();
        if ok {
            return;
        }
        assert!(Instant::now() < deadline, "/healthz never turned 200");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn admin_get(gw: &Gateway, path: &str) -> serde_json::Value {
    admin_call(gw, "GET", path).1
}

fn admin_call(gw: &Gateway, method: &str, path: &str) -> (u16, serde_json::Value) {
    let token = std::fs::read_to_string(gw.dir.join("admin.token")).unwrap();
    let mut s = TcpStream::connect(&gw.admin).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        s,
        "{method} {path} HTTP/1.0\r\nContent-Length: 0\r\nHost: localhost\r\nX-Admin-Token: {}\r\n\r\n",
        token.trim()
    )
    .unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    let code = raw.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = raw.split("\r\n\r\n").nth(1).unwrap();
    (
        code,
        serde_json::from_str(body).unwrap_or(serde_json::Value::Null),
    )
}

// ---------------------------------------------------------------- a client with its own copy of the view

struct Player {
    nick: String,
    ws: WebSocket<MaybeTlsStream<TcpStream>>,
    token: Option<Token>,
    player: i8,
    game: u32,
    stream: u64,
    last_seq: u64,
    view_seq: u64,
    view: Option<(WorldView, ViewHash)>,
    basis: Option<BasisState>,
    turn: Option<TurnState>,
    next_id: u32,
    answers: BTreeMap<u32, CommandOutcome>,
    /// `Attached` modes seen, in order.
    modes: Vec<AttachMode>,
    batches: u64,
    lobby: Option<protocol::LobbyState>,
    chat: Vec<ChatLine>,
    detached: Option<DetachReason>,
}

fn connect(gw: &Gateway) -> WebSocket<MaybeTlsStream<TcpStream>> {
    let (mut ws, _) = tungstenite::connect(format!("ws://{}/ws", gw.listen)).unwrap();
    if let MaybeTlsStream::Plain(s) = ws.get_mut() {
        s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    }
    ws
}

impl Player {
    fn login(gw: &Gateway, nick: &str) -> Player {
        let mut p = Player {
            nick: nick.into(),
            ws: connect(gw),
            token: None,
            player: -1,
            game: 0,
            stream: 0,
            last_seq: 0,
            view_seq: 0,
            view: None,
            basis: None,
            turn: None,
            next_id: 1,
            answers: BTreeMap::new(),
            modes: Vec::new(),
            batches: 0,
            lobby: None,
            chat: Vec::new(),
            detached: None,
        };
        p.hello(Login {
            password: "dev".into(),
            token: None,
            resume: None,
        });
        p
    }

    fn hello(&mut self, login: Login) {
        let nick = self.nick.clone();
        self.send(&ClientMsg::Hello {
            protocol: PROTOCOL_VERSION,
            build: "e2e".into(),
            nick,
        });
        self.send(&ClientMsg::Login(login));
    }

    /// The socket is dropped and a new one logs in with the token, naming the stream it was on (`resume`)
    /// or not (a reloaded page: nothing in memory).
    fn reconnect(&mut self, gw: &Gateway, resume: bool) {
        let resume = resume.then(|| Resume {
            stream: self.stream,
            last_seq: self.last_seq,
            basis: self.basis.as_ref().map(|b| BasisClaim {
                game: self.game,
                hash: b.hash(),
            }),
        });
        self.ws = connect(gw);
        if resume.is_none() {
            self.view = None;
            self.basis = None;
            self.turn = None;
            self.last_seq = 0;
            self.view_seq = 0;
        }
        self.hello(Login {
            password: String::new(),
            token: self.token,
            resume,
        });
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

    /// Reads one server message and takes it into the copy.
    fn pump(&mut self) -> ServerMsg {
        let deadline = Instant::now() + Duration::from_secs(30);
        let msg = loop {
            assert!(
                Instant::now() < deadline,
                "{}: nothing came for 30 s",
                self.nick
            );
            match self.ws.read().unwrap() {
                Message::Binary(b) => {
                    break wire::decode::<ServerMsg>(&b, &Limits::CLIENT).unwrap();
                }
                Message::Close(c) => panic!("{}: the socket closed: {c:?}", self.nick),
                _ => {}
            }
        };
        match &msg {
            ServerMsg::LoggedIn(s) => self.token = s.token,
            ServerMsg::LobbyState(s) => self.lobby = Some(s.clone()),
            ServerMsg::LobbyError { text } => panic!("{}: LobbyError {text}", self.nick),
            ServerMsg::Attached(a) => {
                self.modes.push(a.mode);
                self.player = a.player;
                self.game = a.game;
                self.stream = a.stream;
                self.next_id = self.next_id.max(a.last_cmd_id + 1);
                if a.mode == AttachMode::Fresh {
                    self.last_seq = 0;
                }
            }
            ServerMsg::Game(b) => {
                assert_eq!(
                    b.seq,
                    self.last_seq + 1,
                    "{}: a batch missing or repeated",
                    self.nick
                );
                self.last_seq = b.seq;
                self.batches += 1;
                for m in &b.msgs {
                    self.take(b.seq, m);
                }
            }
            ServerMsg::Detached { reason } => self.detached = Some(reason.clone()),
            _ => {}
        }
        msg
    }

    /// One game message into the copy; every hash is checked (net-protocol 4.6).
    fn take(&mut self, seq: u64, m: &GameMsg) {
        match m {
            GameMsg::MapBasis(b) => {
                self.basis = Some(BasisState::new(b.clone()).expect("the basis checks out"));
            }
            GameMsg::Keyframe(kf) => {
                let basis = self.basis.as_ref().expect("a basis before the keyframe");
                assert_eq!(
                    basis.hash(),
                    kf.basis_hash,
                    "{}: the keyframe names another basis",
                    self.nick
                );
                protocol::check(&kf.view).unwrap();
                check_pair(&kf.view, basis.map()).unwrap();
                let hash = ViewHash::of(&kf.view);
                assert_eq!(hash.value(), kf.hash, "{}: keyframe hash", self.nick);
                self.view = Some((kf.view.clone(), hash));
                self.view_seq = seq;
            }
            GameMsg::Diff(d) => {
                assert_eq!(
                    d.base, self.view_seq,
                    "{}: a diff to another batch",
                    self.nick
                );
                let (view, hash) = self.view.as_mut().expect("a view before a diff");
                let basis = self.basis.as_mut().expect("a basis before a diff");
                let undo = UndoLog::capture(view, hash, basis, &d.diff);
                apply(view, hash, basis, &d.diff).expect("the diff applies");
                if hash.value() != d.hash || d.basis_hash.is_some_and(|h| h != basis.hash()) {
                    undo.restore(view, hash, basis);
                    panic!("{}: a hash did not match after seq {seq}", self.nick);
                }
                self.view_seq = seq;
            }
            GameMsg::Results(rs) => {
                for r in rs {
                    if r.outcome != CommandOutcome::Queued {
                        self.answers.insert(r.id, r.outcome.clone());
                    }
                }
            }
            GameMsg::TurnState(t) => self.turn = Some(t.clone()),
            GameMsg::Chat(lines) => self.chat.extend(lines.iter().cloned()),
            _ => {}
        }
    }

    fn until(&mut self, what: &str, mut pred: impl FnMut(&Player, &ServerMsg) -> bool) {
        for _ in 0..2000 {
            let m = self.pump();
            if pred(self, &m) {
                return;
            }
        }
        panic!("{}: never saw {what}", self.nick);
    }

    /// The human phase of `turn` is on.
    fn wait_turn(&mut self, turn: i32) {
        loop {
            if let Some(t) = &self.turn
                && t.turn == turn
                && t.phase == TurnPhase::Humans
                && self.view.is_some()
            {
                return;
            }
            self.pump();
        }
    }

    fn order(&mut self, turn: i32, cmd: Order) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&ClientMsg::Cmd(Cmd { id, turn, cmd }));
        id
    }

    /// The final answer to `id`.
    fn answer(&mut self, id: u32) -> CommandOutcome {
        loop {
            if let Some(a) = self.answers.get(&id) {
                return a.clone();
            }
            self.pump();
        }
    }

    fn my_unit(&self) -> Option<i32> {
        let (v, _) = self.view.as_ref()?;
        v.units
            .iter()
            .find(|u| u.owner == self.player)
            .map(|u| u.id)
    }

    fn view_hash(&self) -> u64 {
        self.view.as_ref().map_or(0, |(_, h)| h.value())
    }
}

fn settings(choices: &protocol::LobbyChoices, timer: TimerMode) -> GameSettings {
    let first = |l: &[protocol::LobbyChoice]| l[0].type_name.clone();
    GameSettings {
        name: "e2e".into(),
        world_size: "WORLDSIZE_DUEL".into(),
        map_script: first(&choices.map_scripts),
        map_options: vec![],
        speed: first(&choices.speeds),
        start_era: first(&choices.eras),
        handicap: first(&choices.handicaps),
        minor_civs: 0,
        turn_mode: TurnMode::Hybrid,
        timer,
        game_options: vec![],
        seed: Some(7),
        map_seed: None,
        auto_choices: AutoChoices::default(),
        diplo_auto_decline: true,
    }
}

#[test]
fn two_clients_play_twenty_turns_on_the_test_world_and_the_hashes_agree() {
    let gw = start_gateway();
    let mut alice = Player::login(&gw, "alice");
    let mut choices = None;
    alice.until("Choices and the lobby", |_, m| {
        if let ServerMsg::Choices(c) = m {
            choices = Some(c.clone());
        }
        matches!(m, ServerMsg::LobbyState(_))
    });
    let choices = choices.expect("Choices before the lobby state");
    assert!(choices.world_sizes.iter().any(|w| w.id == "WORLDSIZE_DUEL"));

    // A room: alice hosts, slot 2 is an AI so that the AI phase runs.
    alice.lobby(LobbyCmd::CreateRoom {
        settings: settings(&choices, TimerMode::Off),
    });
    alice.until("her room", |p, _| {
        p.lobby.as_ref().is_some_and(|l| l.room.is_some())
    });
    let room_id = alice.lobby.as_ref().unwrap().room.as_ref().unwrap().id;
    alice.lobby(LobbyCmd::Slot {
        index: 2,
        occupant: SlotOccupant::Ai,
        civ: protocol::CivPick::Random,
    });
    let mut bob = Player::login(&gw, "bob");
    bob.until("the lobby", |p, _| {
        p.lobby.as_ref().is_some_and(|l| !l.rooms.is_empty())
    });
    bob.lobby(LobbyCmd::JoinRoom { room: room_id });
    bob.until("bob in the room", |p, _| {
        p.lobby
            .as_ref()
            .and_then(|l| l.room.as_ref())
            .is_some_and(|r| {
                r.slots
                    .iter()
                    .any(|s| matches!(&s.occupant, Occupant::Human { nick, .. } if nick == "bob"))
            })
    });
    bob.lobby(LobbyCmd::Ready(true));
    alice.until("bob ready and the AI in place", |p, _| {
        p.lobby
            .as_ref()
            .and_then(|l| l.room.as_ref())
            .is_some_and(|r| {
                r.slots[2].occupant == Occupant::Ai
                    && r.slots
                        .iter()
                        .any(|s| matches!(&s.occupant, Occupant::Human { ready: true, .. }))
            })
    });
    alice.lobby(LobbyCmd::Start);

    // Both are seated and see the first human turn.
    for p in [&mut alice, &mut bob] {
        p.until("Attached", |_, m| matches!(m, ServerMsg::Attached(_)));
        p.wait_turn(1);
    }
    assert_eq!((alice.player, bob.player), (0, 1));
    assert_eq!(alice.modes, [AttachMode::Fresh]);
    assert_eq!(
        alice.basis.as_ref().unwrap().hash(),
        bob.basis.as_ref().unwrap().hash()
    );
    // The two see different worlds (fog), not the same view.
    assert_ne!(alice.view_hash(), bob.view_hash());

    let mut orders = 0;
    for turn in 1..=TURNS {
        // Bob's page drops at the start of turn 8 and comes back from the ring; at turn 14 it reloads.
        if turn == 8 {
            bob.reconnect(&gw, true);
            bob.until("Attached again", |_, m| matches!(m, ServerMsg::Attached(_)));
            assert_eq!(
                bob.modes.last(),
                Some(&AttachMode::Replay),
                "a resume inside the ring"
            );
        }
        if turn == 14 {
            bob.reconnect(&gw, false);
            bob.until("Attached after a reload", |_, m| {
                matches!(m, ServerMsg::Attached(_))
            });
            assert_eq!(bob.modes.last(), Some(&AttachMode::Fresh));
            bob.wait_turn(turn);
        }
        for p in [&mut alice, &mut bob] {
            p.wait_turn(turn);
            if let Some(unit) = p.my_unit() {
                // Sleep on odd turns, wake on even ones: orders the core takes.
                let cmd = if turn % 2 == 1 {
                    Order::UnitSleep { unit }
                } else {
                    Order::UnitWake { unit }
                };
                let id = p.order(turn, cmd);
                let a = p.answer(id);
                assert!(
                    matches!(a, CommandOutcome::Applied | CommandOutcome::Rejected { .. }),
                    "{}: {a:?}",
                    p.nick
                );
                orders += 1;
            }
            let id = p.order(turn, Order::EndTurn { force: false });
            assert_eq!(
                p.answer(id),
                CommandOutcome::Applied,
                "{} ends turn {turn}",
                p.nick
            );
            orders += 1;
        }
        for p in [&mut alice, &mut bob] {
            p.wait_turn(turn + 1);
        }
    }
    assert_eq!(alice.turn.as_ref().unwrap().turn, TURNS + 1);
    assert_eq!(bob.turn.as_ref().unwrap().turn, TURNS + 1);
    assert!(orders >= 2 * TURNS);
    // Bob's reload gave him a new stream; the copies are whole.
    assert_eq!(
        bob.modes,
        [AttachMode::Fresh, AttachMode::Replay, AttachMode::Fresh]
    );

    // The copies equal what the gateway holds, and nothing was lost on the way.
    let mut m = serde_json::Value::Null;
    for _ in 0..50 {
        m = admin_get(&gw, "/admin/metrics");
        let s = &m["games"]["1"]["seats"];
        if s["0"]["view_hash"] == alice.view_hash() && s["1"]["view_hash"] == bob.view_hash() {
            break;
        }
        // Both copies were read up to the last turn state; give the gateway a moment to publish.
        std::thread::sleep(Duration::from_millis(100));
    }
    let g = &m["games"]["1"];
    assert_eq!(
        g["seats"]["0"]["view_hash"],
        alice.view_hash(),
        "alice's copy is the gateway's view"
    );
    assert_eq!(
        g["seats"]["1"]["view_hash"],
        bob.view_hash(),
        "bob's copy is the gateway's view"
    );
    assert_eq!(g["views"]["hash_fail_worker"], 0);
    assert_eq!(g["fog_violations"], 0);
    assert_eq!(g["cmds"]["lost"], 0);
    assert_eq!(g["cmds"]["expired"], 0);
    assert!(g["cmds"]["applied"].as_u64().unwrap() >= u64::from(2 * TURNS as u32));
    assert_eq!(g["worker"]["crashes"], 0);
    assert_eq!(g["seats"]["1"]["resume"]["replay"], 1);
    assert_eq!(m["server"]["games"]["running"], 1);
}

#[test]
fn a_third_player_joins_the_running_game_in_an_ai_slot_and_the_timer_ends_the_turn_for_her() {
    let gw = start_gateway();
    let mut alice = Player::login(&gw, "alice");
    let mut choices = None;
    alice.until("Choices and the lobby", |_, m| {
        if let ServerMsg::Choices(c) = m {
            choices = Some(c.clone());
        }
        matches!(m, ServerMsg::LobbyState(_))
    });
    let choices = choices.expect("Choices before the lobby state");
    // Alice and bob, slot 2 an AI; the timer is 10 s.
    alice.lobby(LobbyCmd::CreateRoom {
        settings: settings(&choices, TimerMode::Fixed { seconds: 10 }),
    });
    alice.until("her room", |p, _| {
        p.lobby.as_ref().is_some_and(|l| l.room.is_some())
    });
    let room_id = alice.lobby.as_ref().unwrap().room.as_ref().unwrap().id;
    alice.lobby(LobbyCmd::Slot {
        index: 2,
        occupant: SlotOccupant::Ai,
        civ: protocol::CivPick::Random,
    });
    let mut bob = Player::login(&gw, "bob");
    bob.until("the lobby", |p, _| {
        p.lobby.as_ref().is_some_and(|l| !l.rooms.is_empty())
    });
    bob.lobby(LobbyCmd::JoinRoom { room: room_id });
    bob.until("bob in the room", |p, _| {
        p.lobby
            .as_ref()
            .and_then(|l| l.room.as_ref())
            .is_some_and(|r| {
                r.slots
                    .iter()
                    .any(|s| matches!(&s.occupant, Occupant::Human { nick, .. } if nick == "bob"))
            })
    });
    bob.lobby(LobbyCmd::Ready(true));
    alice.until("bob ready and the AI in place", |p, _| {
        p.lobby
            .as_ref()
            .and_then(|l| l.room.as_ref())
            .is_some_and(|r| {
                r.slots[2].occupant == Occupant::Ai
                    && r.slots
                        .iter()
                        .any(|s| matches!(&s.occupant, Occupant::Human { ready: true, .. }))
            })
    });
    alice.lobby(LobbyCmd::Start);
    for p in [&mut alice, &mut bob] {
        p.until("Attached", |_, m| matches!(m, ServerMsg::Attached(_)));
        p.wait_turn(1);
    }

    // Carol sees the AI slot as takeable and takes it.
    let mut carol = Player::login(&gw, "carol");
    carol.until("a takeable slot", |p, _| {
        p.lobby.as_ref().is_some_and(|l| {
            l.games
                .first()
                .is_some_and(|g| g.seats.iter().any(|s| s.player == 2 && s.takeable))
        })
    });
    let game = carol.lobby.as_ref().unwrap().games[0].id;
    carol.lobby(LobbyCmd::JoinGame {
        game,
        player: 2,
        basis: None,
    });
    carol.until("Attached", |_, m| matches!(m, ServerMsg::Attached(_)));
    assert_eq!(
        (carol.player, carol.modes.clone()),
        (2, vec![AttachMode::Fresh])
    );
    carol.wait_turn(1);
    // The others see her in the turn state: three human seats.
    alice.until("three seats in the turn state", |p, _| {
        p.turn.as_ref().is_some_and(|t| t.seats.len() == 3)
    });
    assert_eq!(
        alice.basis.as_ref().unwrap().hash(),
        carol.basis.as_ref().unwrap().hash(),
        "one map basis for all seats"
    );

    // Alice and bob end turn 1; carol does not move. The timer (10 s from the start of the turn) ends it
    // for her, and turn 2 begins for everybody.
    for p in [&mut alice, &mut bob] {
        let id = p.order(1, Order::EndTurn { force: false });
        assert_eq!(
            p.answer(id),
            CommandOutcome::Applied,
            "{} ends turn 1",
            p.nick
        );
    }
    let started = Instant::now();
    for p in [&mut alice, &mut bob, &mut carol] {
        p.wait_turn(2);
    }
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "the timer is 10 s from the start of the turn"
    );
    // Carol, who got a seat in the middle of the turn, plays on: her view agrees with the gateway's.
    let mut m = serde_json::Value::Null;
    for _ in 0..50 {
        m = admin_get(&gw, "/admin/metrics");
        if m["games"]["1"]["seats"]["2"]["view_hash"] == carol.view_hash() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let g = &m["games"]["1"];
    assert_eq!(g["seats"]["2"]["view_hash"], carol.view_hash());
    assert_eq!(g["seats"]["2"]["nick"], "carol");
    assert_eq!(g["timer"]["expired"], 1, "one timer round ended the turn");
    assert_eq!(g["views"]["hash_fail_worker"], 0);
    assert_eq!(g["fog_violations"], 0);
    assert_eq!(g["worker"]["crashes"], 0);
}

// ---------------------------------------------------------------- shared steps of the later tests

/// alice (host) and bob in a room with an AI in slot 2, started and at the first human turn.
fn two_in_a_game(gw: &Gateway, timer: TimerMode) -> (Player, Player) {
    let mut alice = Player::login(gw, "alice");
    let mut choices = None;
    alice.until("Choices and the lobby", |_, m| {
        if let ServerMsg::Choices(c) = m {
            choices = Some(c.clone());
        }
        matches!(m, ServerMsg::LobbyState(_))
    });
    let choices = choices.expect("Choices before the lobby state");
    alice.lobby(LobbyCmd::CreateRoom {
        settings: settings(&choices, timer),
    });
    alice.until("her room", |p, _| {
        p.lobby.as_ref().is_some_and(|l| l.room.is_some())
    });
    let room_id = alice.lobby.as_ref().unwrap().room.as_ref().unwrap().id;
    alice.lobby(LobbyCmd::Slot {
        index: 2,
        occupant: SlotOccupant::Ai,
        civ: protocol::CivPick::Random,
    });
    let mut bob = Player::login(gw, "bob");
    bob.until("the lobby", |p, _| {
        p.lobby.as_ref().is_some_and(|l| !l.rooms.is_empty())
    });
    bob.lobby(LobbyCmd::JoinRoom { room: room_id });
    bob.until("bob in the room", |p, _| {
        p.lobby
            .as_ref()
            .and_then(|l| l.room.as_ref())
            .is_some_and(|r| {
                r.slots
                    .iter()
                    .any(|s| matches!(&s.occupant, Occupant::Human { nick, .. } if nick == "bob"))
            })
    });
    bob.lobby(LobbyCmd::Ready(true));
    alice.until("bob ready and the AI in place", |p, _| {
        p.lobby
            .as_ref()
            .and_then(|l| l.room.as_ref())
            .is_some_and(|r| {
                r.slots[2].occupant == Occupant::Ai
                    && r.slots
                        .iter()
                        .any(|s| matches!(&s.occupant, Occupant::Human { ready: true, .. }))
            })
    });
    alice.lobby(LobbyCmd::Start);
    for p in [&mut alice, &mut bob] {
        p.until("Attached", |_, m| matches!(m, ServerMsg::Attached(_)));
        p.wait_turn(1);
    }
    (alice, bob)
}

/// Both end the turn; the next one is on.
fn end_turn_together(alice: &mut Player, bob: &mut Player, turn: i32) {
    for p in [&mut *alice, &mut *bob] {
        let id = p.order(turn, Order::EndTurn { force: false });
        assert_eq!(p.answer(id), CommandOutcome::Applied);
    }
    for p in [alice, bob] {
        p.wait_turn(turn + 1);
    }
}

#[test]
fn a_leaking_test_world_shows_in_fog_violations_of_the_metrics() {
    // The net-test runner writes this config: the worker audits the fog, the test world leaks it.
    let dir = std::env::temp_dir().join(format!("civ5-e2e-fog-cfg-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = dir.join("server.toml");
    std::fs::write(
        &cfg,
        "test_hooks = true\n[worker]\naudit_fog = true\n[worker.fixture_test]\nleak_fog = true\n",
    )
    .unwrap();
    let gw = start_gateway_with(&["--config", cfg.to_str().unwrap()]);
    let (mut alice, mut bob) = two_in_a_game(&gw, TimerMode::Off);
    end_turn_together(&mut alice, &mut bob, 1);
    let mut seen = 0;
    for _ in 0..50 {
        seen = admin_get(&gw, "/admin/metrics")["games"]["1"]["fog_violations"]
            .as_u64()
            .unwrap();
        if seen > 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        seen > 0,
        "the auditor found leaks that the metrics do not show"
    );
}

// ---------------------------------------------------------------- chat, kick, restart, admin routes

fn say(p: &mut Player, to: ChatTo, text: &str) {
    p.send(&ClientMsg::Chat(ChatSend {
        to,
        text: text.into(),
    }));
}

fn wait_json(gw: &Gateway, what: &str, path: &str, ok: impl Fn(&serde_json::Value) -> bool) {
    for _ in 0..150 {
        if ok(&admin_get(gw, path)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("{what}: never true on {path}");
}

#[test]
fn chat_a_kick_the_admin_save_and_stop_and_a_gateway_restart_keep_the_games() {
    let mut gw = start_gateway();
    let (mut alice, mut bob) = two_in_a_game(&gw, TimerMode::Off);

    // Chat: to all, and private from bob.
    say(&mut alice, ChatTo::All, "hello everybody");
    for p in [&mut alice, &mut bob] {
        p.until("the line to all", |p, _| {
            p.chat.iter().any(|l| l.text == "hello everybody")
        });
    }
    let line = bob
        .chat
        .iter()
        .find(|l| l.text == "hello everybody")
        .unwrap();
    assert_eq!((line.from.as_str(), line.from_player), ("alice", Some(0)));
    say(&mut bob, ChatTo::Player(0), "psst");
    alice.until("the private line", |p, _| {
        p.chat.iter().any(|l| l.text == "psst")
    });

    // The admin saves: a file shows up in the list of saves of the game.
    let (code, _) = admin_call(&gw, "POST", "/admin/games/1/save");
    assert_eq!(code, 200);
    wait_json(&gw, "a save on disk", "/admin/games/1", |g| {
        g["saves"].as_array().is_some_and(|s| !s.is_empty())
    });

    // The host kicks bob: he is detached, the seat goes to the AI, the game goes on for alice.
    alice.send(&ClientMsg::GameCtl(GameCtl::Kick { player: 1 }));
    bob.until("detached", |p, _| p.detached.is_some());
    assert_eq!(bob.detached, Some(DetachReason::Kicked));
    wait_json(&gw, "the seat of bob is the AI", "/admin/games/1", |g| {
        g["seats"][1]["control"] == "Ai" && g["seats"][1]["kicked"] == "bob"
    });
    let id = alice.order(1, Order::EndTurn { force: false });
    assert_eq!(alice.answer(id), CommandOutcome::Applied);
    alice.wait_turn(2);
    let (code, _) = admin_call(&gw, "POST", "/admin/games/1/kick?player=0");
    assert_eq!(code, 409, "the last human cannot be kicked");

    // Restart of the gateway: the game, its id and its seats come back from the disk.
    let before = admin_get(&gw, "/admin/games");
    let token = alice.token;
    drop((alice, bob));
    gw.drain_and_restart();
    let after = admin_get(&gw, "/admin/games");
    let ids = |v: &serde_json::Value| -> Vec<u64> {
        v["games"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| g["id"].as_u64().unwrap())
            .collect()
    };
    assert_eq!(ids(&before), vec![1]);
    assert_eq!(ids(&after), vec![1]);
    assert_eq!(after["games"][0]["name"], before["games"][0]["name"]);
    let mut carol = Player::login(&gw, "carol");
    carol.until("the game in the lobby", |p, _| {
        p.lobby
            .as_ref()
            .is_some_and(|l| l.games.iter().any(|g| g.id == 1))
    });
    // Alice comes back with her token and sits in her old seat.
    let mut alice = Player::login(&gw, "alice");
    alice.ws = connect(&gw);
    alice.token = token;
    alice.hello(Login {
        password: String::new(),
        token,
        resume: None,
    });
    alice.until("Attached", |_, m| matches!(m, ServerMsg::Attached(_)));
    assert_eq!((alice.game, alice.player), (1, 0));

    // The admin stops the game: it sleeps with its seats, and the room list still has it.
    let (code, _) = admin_call(&gw, "POST", "/admin/games/1/stop");
    assert_eq!(code, 200);
    wait_json(&gw, "the game asleep", "/admin/games", |g| {
        g["games"][0]["status"] == "Hibernating"
    });
    let (code, _) = admin_call(&gw, "POST", "/admin/games/1/restart");
    assert_eq!(code, 200);
    wait_json(&gw, "the game awake", "/admin/games", |g| {
        g["games"][0]["status"] == "Running"
    });
}
