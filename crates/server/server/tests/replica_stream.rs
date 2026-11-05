//! The replica stream at the gateway (protocol v4) over real sockets with the mock worker in
//! its `replica=1` mode (made-up bodies, the real gateway): `GcBase` first, batches to every seat, `GcTurn` with a
//! patch, a `Fresh` attach that gets the base and the batches after it, the bodies over HTTP (right hash, the
//! old ones gone), `GcNet` to the worker and the refusal back as a journal line and a counter, `GcResync`
//! answered to the one seat. The real core is in `worker_real_replica.rs`.

mod common;

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{mock_server_config, temp_dir};
use protocol::{
    AiTurn, AutoChoices, ClientMsg, Cmd, Codec, Command, GameMsg, GameSettings, GcBase,
    GcResyncReason, GcTurn, LobbyCmd, Login, Occupant, PROTOCOL_VERSION, Postcard, ServerMsg,
    TimerMode, Token, TurnMode, xxh64,
};
use server::Shared;
use server::identity::Secret;
use server::session::limits::LimitsConfig;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};
use wire::{Encoder, Limits};

const PASSWORD: &str = "replica-password";
const ADMIN: &str = "admin-secret";

struct TestServer {
    addr: SocketAddr,
    admin: SocketAddr,
    dir: PathBuf,
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
    let mut config = mock_server_config(
        &dir,
        mode,
        Duration::from_secs(20),
        [Duration::ZERO, Duration::ZERO, Duration::ZERO],
    );
    config.password = Some(Secret::new(PASSWORD));
    config.admin_token = Some(Secret::new(ADMIN));
    if let Some(s) = config.supervisor.as_mut() {
        s.worker.record = true;
    }
    config.limits = LimitsConfig {
        password_delay: Duration::from_millis(5),
        ..LimitsConfig::default()
    };
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
        _shared: shared,
        _rt: rt,
    }
}

type Ws = WebSocket<MaybeTlsStream<TcpStream>>;

/// What a client heard of the replica stream, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Heard {
    Base(GcBase),
    Batch(u64, Vec<u8>),
    Turn(GcTurn),
    Ai(AiTurn),
}

struct Client {
    ws: Ws,
    nick: String,
    token: Option<Token>,
    next_id: u32,
    heard: Vec<Heard>,
    lines: Vec<String>,
}

impl Client {
    fn login(srv: &TestServer, nick: &str) -> Client {
        Self::connect(srv, nick, None)
    }

    fn connect(srv: &TestServer, nick: &str, token: Option<Token>) -> Client {
        let (mut ws, _) = tungstenite::connect(format!("ws://{}/ws", srv.addr)).unwrap();
        if let MaybeTlsStream::Plain(s) = ws.get_mut() {
            s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        }
        let mut c = Client {
            ws,
            nick: nick.into(),
            token,
            next_id: 1,
            heard: Vec::new(),
            lines: Vec::new(),
        };
        c.send(&ClientMsg::Hello {
            protocol: PROTOCOL_VERSION,
            build: "test".into(),
            nick: nick.into(),
        });
        c.send(&ClientMsg::Login(Login {
            password: PASSWORD.into(),
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
                "{}: nothing came in 15 s",
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
                                    GameMsg::GcBase(x) => self.heard.push(Heard::Base(*x)),
                                    GameMsg::GcTurn(x) => self.heard.push(Heard::Turn(*x)),
                                    GameMsg::AiTurn(p) => self.heard.push(Heard::Ai(p.clone())),
                                    GameMsg::GcJournal(j) => {
                                        for (i, bytes) in j.batches.iter().enumerate() {
                                            self.heard.push(Heard::Batch(
                                                j.first_jseq + i as u64,
                                                bytes.clone(),
                                            ));
                                        }
                                    }
                                    GameMsg::Journal(es) => {
                                        self.lines.extend(es.iter().map(|e| e.text.clone()));
                                    }
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

    /// Reads until `pred` holds for what was heard of the replica stream so far.
    fn hear(&mut self, what: &str, pred: impl Fn(&[Heard]) -> bool) {
        for _ in 0..400 {
            if pred(&self.heard) {
                return;
            }
            self.recv();
        }
        panic!("{}: no {what}; heard {:?}", self.nick, self.heard);
    }

    fn order(&mut self, cmd: Command) {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&ClientMsg::Cmd(Cmd { id, turn: 1, cmd }));
    }

    fn bases(&self) -> Vec<GcBase> {
        self.heard
            .iter()
            .filter_map(|h| match h {
                Heard::Base(b) => Some(*b),
                _ => None,
            })
            .collect()
    }

    fn batches(&self) -> Vec<(u64, Vec<u8>)> {
        self.heard
            .iter()
            .filter_map(|h| match h {
                Heard::Batch(j, b) => Some((*j, b.clone())),
                _ => None,
            })
            .collect()
    }
}

fn settings() -> GameSettings {
    GameSettings {
        name: "replica".into(),
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
    }
}

/// alice (host) and bob in a game, both attached, with the base in hand.
fn game(name: &str, mode: &str) -> (TestServer, Client, Client) {
    game_with(name, mode, true)
}

fn game_with(name: &str, mode: &str, base: bool) -> (TestServer, Client, Client) {
    let srv = start(name, mode);
    let mut alice = Client::login(&srv, "alice");
    alice.until("the lobby", |m| matches!(m, ServerMsg::LobbyState(_)));
    alice.send(&ClientMsg::Lobby(LobbyCmd::CreateRoom {
        settings: settings(),
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
        if base {
            c.hear("the base", |h| {
                h.iter().any(|x| matches!(x, Heard::Base(_)))
            });
        }
    }
    (srv, alice, bob)
}

/// `GET path` on the game listener: status and body.
fn http_get(addr: SocketAddr, path: &str, admin: bool) -> (u16, Vec<u8>) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    let token = if admin {
        format!("X-Admin-Token: {ADMIN}\r\n")
    } else {
        String::new()
    };
    write!(s, "GET {path} HTTP/1.0\r\nHost: localhost\r\n{token}\r\n").unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).unwrap();
    let at = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
    let head = String::from_utf8_lossy(&raw[..at]).into_owned();
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, raw[at..].to_vec())
}

fn blob(srv: &TestServer, hash: u64) -> (u16, Vec<u8>) {
    http_get(srv.addr, &format!("/replica/{hash:016x}"), false)
}

fn replica_metrics(srv: &TestServer) -> serde_json::Value {
    let (code, body) = http_get(srv.admin, "/admin/metrics", true);
    assert_eq!(code, 200);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    v["games"]["1"]["replica"].clone()
}

const MODE: &str = "echo replica=1 patch=1";

#[test]
fn every_seat_gets_the_base_then_the_batches_and_the_body_is_served() {
    let (srv, mut alice, mut bob) = game("r-base", MODE);
    for c in [&alice, &bob] {
        let bases = c.bases();
        assert_eq!(bases.len(), 1, "{}", c.nick);
        assert_eq!((bases[0].epoch, bases[0].turn, bases[0].jseq), (77, 1, 0));
    }
    let base = alice.bases()[0];
    // The body over HTTP: the hash in the URL is the one of the body, and it never changes.
    let (code, body) = blob(&srv, base.snapshot.hash);
    assert_eq!(code, 200);
    assert_eq!(xxh64(0, &body), base.snapshot.hash);
    assert_eq!(body.len() as u32, base.snapshot.size);
    assert_eq!(blob(&srv, base.snapshot.hash ^ 1).0, 404);
    assert_eq!(http_get(srv.addr, "/replica/zz", false).0, 404);
    // Orders that are not an end of turn: one batch each, to both seats, in order.
    alice.order(Command::CancelEndTurn);
    alice.order(Command::CancelEndTurn);
    for c in [&mut alice, &mut bob] {
        c.hear("two batches", |h| {
            h.iter().filter(|x| matches!(x, Heard::Batch(..))).count() >= 2
        });
        assert_eq!(
            c.batches(),
            vec![(1, b"J1".to_vec()), (2, b"J2".to_vec())],
            "{}",
            c.nick
        );
    }
}

#[test]
fn a_turn_goes_to_everybody_with_its_patch_and_old_bodies_go() {
    let (srv, mut alice, mut bob) = game("r-turn", MODE);
    let first = alice.bases()[0].snapshot.hash;
    alice.order(Command::EndTurn { force: false });
    for c in [&mut alice, &mut bob] {
        c.hear("a turn", |h| h.iter().any(|x| matches!(x, Heard::Turn(_))));
        let Some(Heard::Turn(t)) = c.heard.iter().find(|x| matches!(x, Heard::Turn(_))) else {
            unreachable!()
        };
        assert_eq!((t.epoch, t.turn, t.jseq), (77, 2, 0));
        let p = t.patch.expect("variant A: a patch comes with the turn");
        // Both bodies are there; the patch names the snapshot it starts from.
        let (code, snap) = blob(&srv, t.snapshot.hash);
        assert_eq!((code, xxh64(0, &snap)), (200, t.snapshot.hash));
        let (code, patch) = blob(&srv, p.blob.hash);
        assert_eq!((code, xxh64(0, &patch)), (200, p.blob.hash));
        assert_eq!(p.from_raw_hash, 1000 + 1);
    }
    let m = replica_metrics(&srv);
    assert_eq!(
        (m["bases"].as_u64(), m["turns"].as_u64()),
        (Some(1), Some(1))
    );
    assert_eq!(m["turns_with_patch"].as_u64(), Some(1));
    // Seven more turns: the bodies of the first base are not kept for ever.
    for _ in 0..7 {
        alice.order(Command::EndTurn { force: false });
    }
    alice.hear("eight turns", |h| {
        h.iter().filter(|x| matches!(x, Heard::Turn(_))).count() >= 8
    });
    assert_eq!(blob(&srv, first).0, 404, "the first base is gone");
    let last = alice
        .heard
        .iter()
        .rev()
        .find_map(|h| match h {
            Heard::Turn(t) => Some(*t),
            _ => None,
        })
        .unwrap();
    assert_eq!(blob(&srv, last.snapshot.hash).0, 200);
    assert_eq!(blob(&srv, last.patch.unwrap().blob.hash).0, 200);
}

/// The AI turn events of the worker reach every seat, and the timeline of the end of the turn (the worker's trace and
/// the gateway's own spans) is served by the admin API as Chrome JSON, journal lines and a summary.
#[test]
fn the_ai_turn_events_reach_the_seats_and_the_timeline_is_served() {
    let (srv, mut alice, mut bob) = game("r-timeline", MODE);
    alice.order(Command::EndTurn { force: false });
    for c in [&mut alice, &mut bob] {
        c.hear("the AI turn events and the turn", |h| {
            h.iter().any(|x| matches!(x, Heard::Ai(_)))
                && h.iter().any(|x| matches!(x, Heard::Turn(_)))
        });
        let p = c
            .heard
            .iter()
            .find_map(|x| match x {
                Heard::Ai(p) => Some(p.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(p.turn, 1);
        assert_eq!(
            p.events
                .iter()
                .map(|e| (e.player, e.ended))
                .collect::<Vec<_>>(),
            [(3, false), (3, true)]
        );
    }
    // The body of the new turn was fetched by nobody yet; fetch it: the gateway times that (`g.blob_serve`).
    let t = alice
        .heard
        .iter()
        .find_map(|h| match h {
            Heard::Turn(t) => Some(*t),
            _ => None,
        })
        .unwrap();
    assert_eq!(blob(&srv, t.snapshot.hash).0, 200);
    // Chrome JSON: the worker's and the gateway's spans of turn 1 on one axis.
    let (code, body) = http_get(srv.admin, "/admin/games/1/trace?turns=1", true);
    assert_eq!(code, 200);
    let j: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let names: Vec<&str> = j["traceEvents"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["ph"] == "X" || e["ph"] == "i")
        .filter_map(|e| e["name"].as_str())
        .collect();
    for want in [
        "w.end_turn",
        "w.ai",
        "w.turn_change",
        "w.pack",
        "g.turn_fwd",
        "g.blob_serve",
    ] {
        assert!(names.contains(&want), "{want} in {names:?}");
    }
    let procs: Vec<&str> = j["traceEvents"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["name"] == "process_name")
        .filter_map(|e| e["args"]["name"].as_str())
        .collect();
    assert_eq!(procs, ["gateway", "worker"]);
    // The summary: one line, the AI is the 3 s the worker measured.
    let (code, body) = http_get(
        srv.admin,
        "/admin/games/1/trace?turns=1&format=summary",
        true,
    );
    assert_eq!(code, 200);
    let text = String::from_utf8(body).unwrap();
    assert!(
        text.contains("turn 1") && text.contains("server_ai 3.000"),
        "{text}"
    );
    // The journal lines carry the shared envelope.
    let (code, body) = http_get(srv.admin, "/admin/games/1/trace?turns=1&format=jsonl", true);
    assert_eq!(code, 200);
    let line: serde_json::Value = serde_json::from_str(
        String::from_utf8(body)
            .unwrap()
            .lines()
            .find(|l| l.contains("\"w.ai\""))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        (line["kind"].as_str(), line["src"].as_str()),
        (Some("span"), Some("worker"))
    );
    assert_eq!(line["trace"], "1:1");
    // A turn range that is not one is refused; a game that is not there is 404.
    assert_eq!(
        http_get(srv.admin, "/admin/games/1/trace?turns=x", true).0,
        400
    );
    assert_eq!(http_get(srv.admin, "/admin/games/9/trace", true).0, 404);
    assert_eq!(http_get(srv.admin, "/admin/games/1/trace", false).0, 401);
}

#[test]
fn a_fresh_attach_gets_the_base_of_the_turn_and_the_batches_after_it() {
    let (srv, mut alice, bob) = game("r-fresh", MODE);
    let token = bob.token;
    drop(bob);
    alice.order(Command::CancelEndTurn);
    alice.order(Command::EndTurn { force: false });
    alice.order(Command::CancelEndTurn);
    alice.order(Command::CancelEndTurn);
    alice.hear("the last batch", |h| {
        h.iter().any(|x| matches!(x, Heard::Batch(3, _)))
    });
    // Bob comes back with his token only: a `Fresh` stream, not a replay.
    let mut bob = Client::connect(&srv, "bob", token);
    bob.until("Attached", |m| matches!(m, ServerMsg::Attached(_)));
    bob.hear("the batches", |h| {
        h.iter().filter(|x| matches!(x, Heard::Batch(..))).count() >= 2
    });
    let bases = bob.bases();
    assert_eq!(bases.len(), 1);
    // The base of the chain is the snapshot of the turn that started with the `EndTurn`: `jseq` 1, turn 2.
    assert_eq!((bases[0].turn, bases[0].jseq), (2, 1));
    assert_eq!(
        bob.batches(),
        vec![(2, b"J2".to_vec()), (3, b"J3".to_vec())],
        "the batches after the base, none before it"
    );
}

#[test]
fn ui_messages_reach_the_worker_and_come_back_as_batches_or_as_a_refusal() {
    let (srv, mut alice, mut bob) = game("r-net", MODE);
    alice.send(&ClientMsg::GcNet(b"research".to_vec()));
    for c in [&mut alice, &mut bob] {
        c.hear("the echo", |h| {
            h.iter()
                .any(|x| matches!(x, Heard::Batch(1, b) if b == b"research"))
        });
    }
    alice.send(&ClientMsg::GcNet(b"reject: the turn handshake".to_vec()));
    for _ in 0..50 {
        if alice.lines.iter().any(|l| l.contains("refused")) {
            break;
        }
        alice.recv();
    }
    assert!(
        alice.lines.iter().any(|l| l.contains("refused")),
        "{:?}",
        alice.lines
    );
    assert!(
        !bob.lines.iter().any(|l| l.contains("refused")),
        "only the seat that sent it is told"
    );
    // An empty message: dropped at the gateway, counted (a frame over the limit never gets this far).
    alice.send(&ClientMsg::GcNet(Vec::new()));
    let m = (0..50)
        .find_map(|_| {
            let m = replica_metrics(&srv);
            (m["net_dropped"].as_u64() == Some(1)).then_some(m)
        })
        .expect("the oversized message is counted");
    assert_eq!(m["net_in"].as_u64(), Some(2));
    assert_eq!(m["net_rejected"].as_u64(), Some(1));
}

/// The recording of a game: every order as received, the UI messages, the human and the AI journal,
/// the keyframe of every turn start in its own segment, the worker's marks, the results.
#[test]
fn a_game_is_recorded_orders_net_journal_ai_journal_and_a_segment_per_turn() {
    use recording::Recording;
    use recording::format::{Body, KfKind, Verdict};
    let (srv, mut alice, mut bob) = game("r-rec", MODE);
    alice.send(&ClientMsg::GcNet(b"research".to_vec()));
    alice.order(Command::CancelEndTurn);
    alice.order(Command::EndTurn { force: false });
    alice.hear("turn 2", |h| h.iter().any(|x| matches!(x, Heard::Turn(_))));
    bob.order(Command::EndTurn { force: false });
    bob.hear("turn 3", |h| {
        h.iter().filter(|x| matches!(x, Heard::Turn(_))).count() >= 2
    });
    let dir = std::fs::read_dir(srv.dir.join("recordings"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("g1-"))
        })
        .expect("the game has a recording");
    // The journal records wait for the two-second flush; a keyframe flushes at once.
    let mut all = Vec::new();
    for _ in 0..100 {
        all = Recording::open(&dir)
            .and_then(|r| r.read_all())
            .map(|segs| segs.into_iter().flat_map(|s| s.entries).collect())
            .unwrap_or_default();
        let have = |f: &dyn Fn(&Body) -> bool| all.iter().any(|e: &recording::Entry| f(&e.body));
        let kf_count = Recording::open(&dir).map_or(0, |r| r.manifest.keyframes.len());
        if kf_count == 3
            && have(&|b| matches!(b, Body::Net(_)))
            && have(&|b| matches!(b, Body::Journal(j) if j.phase == protocol::RecPhase::Ai))
            && have(&|b| matches!(b, Body::Journal(j) if j.phase == protocol::RecPhase::Human))
            // The orders can come with a later flush than the turn change.
            && all.iter().filter(|e| matches!(e.body, Body::Order(_))).count() >= 3
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let r = Recording::open(&dir).unwrap();
    let kfs: Vec<_> = r
        .manifest
        .keyframes
        .iter()
        .map(|k| (k.turn, k.kind))
        .collect();
    // The base of turn 1 is whole; turns 2 and 3 keep the patch (`MODE` has patch=1).
    assert_eq!(
        kfs,
        [(1, KfKind::Full), (2, KfKind::Patch), (3, KfKind::Patch)]
    );
    for k in &r.manifest.keyframes {
        let body = r
            .keyframe_body(k.file.as_deref().unwrap(), k.hash)
            .expect("the body is on disk with the hash of the record");
        assert!(!body.is_empty());
    }
    let orders: Vec<_> = all
        .iter()
        .filter_map(|e| match &e.body {
            Body::Order(o) => Some((o.player, o.cmd.id, o.verdict.clone())),
            _ => None,
        })
        .collect();
    assert!(orders.len() >= 3, "{orders:?}");
    assert!(
        orders.iter().all(|o| o.2 == Verdict::Forwarded),
        "{orders:?}"
    );
    assert!(
        all.iter()
            .any(|e| matches!(&e.body, Body::Net(n) if n.bytes == b"research" && n.taken))
    );
    assert!(
        all.iter()
            .any(|e| matches!(&e.body, Body::Result(x) if x.outcome == "Applied"))
    );
    assert!(all.iter().any(|e| matches!(&e.body,
        Body::Journal(j) if j.phase == protocol::RecPhase::Ai && j.bytes.starts_with(b"ai-journal"))));
    assert!(all.iter().any(|e| matches!(&e.body,
        Body::Worker(w) if matches!(w.ev, recording::format::WorkerEv::CallBegin { .. }))));
    assert!(
        all.iter()
            .any(|e| matches!(&e.body, Body::Trace(t) if !t.bytes.is_empty()))
    );
    assert!(
        all.iter()
            .any(|e| matches!(&e.body, Body::Seat(s) if s.event == "attach"))
    );
    // The worker's core build is in the header record.
    assert!(
        all.iter()
            .any(|e| matches!(&e.body, Body::Header(h) if !h.core_build.is_empty()))
    );
}

#[test]
fn a_resync_is_answered_to_the_one_seat_and_not_twice_in_a_row() {
    let (srv, mut alice, mut bob) = game("r-resync", MODE);
    alice.order(Command::CancelEndTurn);
    alice.hear("a batch", |h| {
        h.iter().any(|x| matches!(x, Heard::Batch(1, _)))
    });
    bob.hear("a batch", |h| {
        h.iter().any(|x| matches!(x, Heard::Batch(1, _)))
    });
    alice.send(&ClientMsg::GcResync {
        epoch: 77,
        have_raw_hash: Some(1001),
        at_jseq: 1,
        reason: GcResyncReason::Diverged,
    });
    alice.hear("a second base", |h| {
        h.iter().filter(|x| matches!(x, Heard::Base(_))).count() >= 2
    });
    let bases = alice.bases();
    assert_eq!(bases[1].jseq, 1, "the base is the state after the batch");
    let (code, body) = blob(&srv, bases[1].snapshot.hash);
    assert_eq!((code, xxh64(0, &body)), (200, bases[1].snapshot.hash));
    // A second ask at once is not answered; bob is untouched.
    alice.send(&ClientMsg::GcResync {
        epoch: 77,
        have_raw_hash: None,
        at_jseq: 1,
        reason: GcResyncReason::Gap,
    });
    alice.order(Command::CancelEndTurn);
    alice.hear("the next batch", |h| {
        h.iter().any(|x| matches!(x, Heard::Batch(2, _)))
    });
    bob.hear("the next batch", |h| {
        h.iter().any(|x| matches!(x, Heard::Batch(2, _)))
    });
    assert_eq!(alice.bases().len(), 2);
    assert_eq!(bob.bases().len(), 1, "a resync is for the seat that asked");
    let m = replica_metrics(&srv);
    assert_eq!(m["resync_bases"].as_u64(), Some(1));
    assert_eq!(m["resync"]["diverged"].as_u64(), Some(1));
    assert_eq!(m["resync"]["gap"].as_u64(), Some(1));
    assert_eq!(m["resync_dropped"].as_u64(), Some(1));
}

#[test]
fn a_game_without_the_replica_stream_sends_none_of_it() {
    // The test world and the mock without `replica=1`: no Gc message, the old streams as before.
    let (_srv, mut alice, _bob) = game_with("r-none", "echo", false);
    alice.order(Command::CancelEndTurn);
    alice.until("an answer", |m| {
        matches!(m, ServerMsg::Game(b) if b.msgs.iter().any(|g| matches!(g, GameMsg::Results(_))))
    });
    assert!(alice.heard.is_empty());
    // A UI message where there is no replica stream is refused by the worker, with a line to the seat.
    alice.send(&ClientMsg::GcNet(vec![1, 2, 3]));
    for _ in 0..50 {
        if alice.lines.iter().any(|l| l.contains("refused")) {
            break;
        }
        alice.recv();
    }
    assert!(alice.lines.iter().any(|l| l.contains("refused")));
}
