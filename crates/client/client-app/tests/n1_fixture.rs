//! Milestone N-1 (net-client 4.14, 11): `ClientCore` and `platform-native` enter a game, get the map basis, the
//! keyframe and the diffs, give orders and play 20 turns without a hash mismatch.
//!
//! The server here is the scripted gateway of the `client-core` tests, put on a real listening socket: a
//! WebSocket (tungstenite) for the game and a plain HTTP answer for `GET /catalog/{hash}`. The client side is
//! the real thing: `NetClient` over `WsTransport` and `StdHttp` (threads, blocking sockets). Halfway through,
//! the gateway applies an order and then cuts the socket before it answers (the lost-answer case of AC-N1.5):
//! the client reconnects with `token + resume`, repeats the order with the same `id`, and the gateway
//! applies it once.
//!
//! What this does not cover is the real `civ5-server` with the fixture worker: the seat pipeline (N2.6,
//! N2.7), the lobby rooms (N2.8) and the fixture core in a child process (N2.9-N2.14, N2.26) are not on this
//! branch. [`drive_turns`] takes any `NetClient` that is playing, so the same loop runs against the real
//! server once it exists; only the entry (login, room, `Start`) and the oracle of the server's own hash
//! change. The client's own check of every diff hash (`need_keyframe == 0`, `undo_restores == 0`) is the
//! assertion that works against both.

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use client_app::net::NetClient;
use client_core::{AddrContext, ClientEvent, ClientPhase, NetConfig};
use platform::MemoryStore;
use platform_native::{StdHttp, WsTransport};
use protocol::{
    AttachMode, Attached, BasisClaim, Catalog, CatalogRef, ClientMsg, Codec, Command, GameMsg,
    LobbyChoices, Postcard, SeatTurn, ServerMsg, Session, Token, TurnMode, TurnPhase, TurnState,
};
use tungstenite::Message;
use wire::{Encoder, Limits};

// The gateway's game: the same world and order book as the `client-core` tests, included from there.
#[path = "../../client-core/tests/common/orders.rs"]
mod orders;
#[path = "../../client-core/tests/common/views.rs"]
mod views;
#[path = "../../client-core/tests/common/world.rs"]
mod world;

use orders::OrderBook;
use world::World;

const PASSWORD: &str = "n1-test-password";
const GAME: u32 = 7;
const STREAM: u64 = 5;
const TURNS: i32 = 20;

// ---------------------------------------------------------------- the scripted gateway

struct Gateway {
    world: World,
    book: OrderBook,
    token: Token,
    catalog: Vec<u8>,
    /// The gateway applies this order and cuts the socket without answering (once).
    drop_at: Option<u32>,
    dropped: bool,
    logins: Vec<protocol::Login>,
}

type Gw = Arc<Mutex<Gateway>>;

fn frame(msg: &ServerMsg) -> Vec<u8> {
    Encoder::default().frame_msg(msg).expect("encode")
}

fn turn_state(turn: i32) -> TurnState {
    TurnState {
        turn,
        phase: TurnPhase::Humans,
        mode: TurnMode::Simultaneous,
        seats: vec![SeatTurn {
            player: 1,
            finished: false,
            may_act: true,
            group: 0,
        }],
        timers: vec![],
        paused: false,
    }
}

fn catalog_body() -> Vec<u8> {
    let mut b = b"C5CT".to_vec();
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend(Postcard.encode(&Catalog::default()).expect("encode"));
    b
}

impl Gateway {
    fn new(drop_at: Option<u32>) -> Gateway {
        Gateway {
            world: World::new(GAME, 2024, 12, 8),
            book: OrderBook::new(),
            token: Token([9; 16]),
            catalog: catalog_body(),
            drop_at,
            dropped: false,
            logins: Vec::new(),
        }
    }

    fn catalog_ref(&self) -> CatalogRef {
        CatalogRef {
            hash: protocol::xxh64(0, &self.catalog),
            len: self.catalog.len() as u32,
        }
    }

    /// The replies to one client message, and whether the socket is cut after them.
    fn on_client(&mut self, msg: ClientMsg) -> (Vec<ServerMsg>, bool) {
        let mut out = Vec::new();
        match msg {
            ClientMsg::Hello { .. } => out.push(ServerMsg::Welcome {
                session: 1,
                build: "scripted".into(),
            }),
            ClientMsg::Ping { seq, client_ms } => out.push(ServerMsg::Pong {
                seq,
                client_ms,
                server_ms: 9_000_000,
            }),
            ClientMsg::Login(login) => {
                self.logins.push(login.clone());
                if login.token != Some(self.token) && login.password != PASSWORD {
                    out.push(ServerMsg::Reject(protocol::RejectReason::BadPassword));
                    return (out, false);
                }
                out.push(ServerMsg::LoggedIn(Session {
                    token: Some(self.token),
                    nick: "alice".into(),
                }));
                out.push(ServerMsg::Choices(LobbyChoices::default()));
                self.attach(&login, &mut out);
            }
            ClientMsg::Cmd(c) => {
                let is_new = c.id > self.book.last;
                if is_new {
                    // The game moves on: a turn for `EndTurn`, a small change for any other order.
                    let w = &mut self.world;
                    let mut new = w.mutated();
                    new.turn = w.view.turn + i32::from(matches!(c.cmd, Command::EndTurn { .. }));
                    out.push(w.step_to(new));
                }
                out.extend(self.book.on_cmd(c, &mut self.world));
                if is_new && matches!(c.cmd, Command::EndTurn { .. }) {
                    let ts = turn_state(self.world.view.turn);
                    out.push(self.world.push(vec![GameMsg::TurnState(ts)]));
                }
                if self.drop_at == Some(c.id) && !self.dropped {
                    // Applied and logged, but the answer never leaves.
                    self.dropped = true;
                    return (vec![], true);
                }
            }
            _ => {}
        }
        (out, false)
    }

    fn attach(&mut self, login: &protocol::Login, out: &mut Vec<ServerMsg>) {
        let replay = login
            .resume
            .is_some_and(|r| r.stream == STREAM && r.last_seq <= self.world.seq);
        out.push(ServerMsg::Attached(Attached {
            game: GAME,
            player: 1,
            stream: STREAM,
            mode: if replay {
                AttachMode::Replay
            } else {
                AttachMode::Fresh
            },
            catalog: self.catalog_ref(),
            last_cmd_id: self.book.last,
        }));
        if replay {
            let last = login.resume.map_or(0, |r| r.last_seq);
            out.extend(self.world.tail(last));
            return;
        }
        self.world.reset_stream();
        let held = login.resume.and_then(|r| r.basis)
            == Some(BasisClaim {
                game: GAME,
                hash: self.world.basis_hash(),
            });
        out.push(self.world.keyframe(!held));
        let ts = turn_state(self.world.view.turn);
        let seats = vec![protocol::SeatView {
            player: 1,
            control: protocol::SeatControl::Human {
                nick: "alice".into(),
            },
            conn: protocol::Conn::Online,
            host: true,
        }];
        out.push(
            self.world
                .push(vec![GameMsg::TurnState(ts), GameMsg::Seats(seats)]),
        );
    }
}

/// A listener that serves the WebSocket of the game and `GET /catalog/...` on the same port.
fn start_gateway(gw: Gw) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let gw = Arc::clone(&gw);
            thread::spawn(move || serve(stream, &gw));
        }
    });
    addr
}

fn serve(stream: TcpStream, gw: &Gw) {
    let mut head = [0u8; 16];
    let n = loop {
        match stream.peek(&mut head) {
            Ok(n) if n >= 13 => break n,
            Ok(0) | Err(_) => return,
            Ok(_) => thread::sleep(Duration::from_millis(1)),
        }
    };
    if head[..n].starts_with(b"GET /catalog/") {
        http_catalog(stream, gw);
    } else {
        session(stream, gw);
    }
}

fn http_catalog(stream: TcpStream, gw: &Gw) {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    // Read the request head up to the empty line.
    while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
        line.clear();
    }
    let body = gw.lock().expect("gateway").catalog.clone();
    let mut out = reader.into_inner();
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nCache-Control: immutable\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = out.write_all(head.as_bytes());
    let _ = out.write_all(&body);
}

fn session(stream: TcpStream, gw: &Gw) {
    let Ok(mut ws) = tungstenite::accept(stream) else {
        return;
    };
    loop {
        let bytes = match ws.read() {
            Ok(Message::Binary(b)) => b,
            Ok(Message::Close(_)) | Err(_) => return,
            Ok(_) => continue,
        };
        let msg: ClientMsg = wire::decode(&bytes, &Limits::GATEWAY).expect("an enc 0 frame");
        let (replies, cut) = gw.lock().expect("gateway").on_client(msg);
        for r in replies {
            if ws.send(Message::Binary(frame(&r).into())).is_err() {
                return;
            }
        }
        if cut {
            let _ = ws.close(None);
            let _ = ws.flush();
            return;
        }
    }
}

// ---------------------------------------------------------------- the client side

struct Clock(Instant);

impl Clock {
    fn now(&self) -> u64 {
        self.0.elapsed().as_millis() as u64
    }
}

/// Pumps the client until `done` or the time is out; every event is kept in `events`.
fn spin(
    client: &mut NetClient,
    clock: &Clock,
    events: &mut Vec<ClientEvent>,
    timeout: Duration,
    mut done: impl FnMut(&NetClient, &[ClientEvent]) -> bool,
) -> bool {
    let end = Instant::now() + timeout;
    loop {
        client.pump(clock.now());
        while let Some(ev) = client.poll_event() {
            events.push(ev);
        }
        if done(client, events) {
            return true;
        }
        if Instant::now() >= end {
            return false;
        }
        thread::sleep(Duration::from_millis(2));
    }
}

fn answered(events: &[ClientEvent], id: u32) -> bool {
    events
        .iter()
        .any(|e| matches!(e, ClientEvent::OrderAnswered { id: i, .. } if *i == id))
}

/// Plays `turns` turns: two orders each (a move and `EndTurn`), waits for both answers and the new turn.
/// `server_view` is the oracle of the scripted server, `(view_seq, view hash)`; the client's own check of
/// every diff hash needs none.
fn drive_turns(
    client: &mut NetClient,
    clock: &Clock,
    events: &mut Vec<ClientEvent>,
    turns: i32,
    server_view: Option<&dyn Fn() -> (u64, u64)>,
) {
    let first = client.core().turn_state().expect("a turn state").turn;
    for t in 0..turns {
        let mv = client
            .core_mut()
            .order(Command::UnitMoveTo {
                unit: 1,
                x: 2,
                y: 1,
            })
            .expect("a move is accepted");
        let end = client
            .core_mut()
            .order(Command::EndTurn { force: false })
            .expect("the end of turn is accepted");
        client.flush();
        let want_turn = first + t + 1;
        let ok = spin(client, clock, events, Duration::from_secs(20), |c, ev| {
            answered(ev, mv)
                && answered(ev, end)
                && c.core().turn_state().is_some_and(|ts| ts.turn == want_turn)
        });
        assert!(ok, "turn {t}: no answer, phase {:?}", client.phase());
        if let Some(oracle) = server_view {
            let (seq, hash) = oracle();
            assert_eq!(client.core().view_seq(), seq, "turn {t}: the same view");
            assert_eq!(client.core().base_hash(), Some(hash), "turn {t}: hash");
        }
    }
}

#[test]
fn twenty_turns_over_real_sockets_with_a_cut_and_a_repeated_order() {
    // Order 25 is the first of turn 13: applied by the gateway, answer lost with the socket.
    let gw: Gw = Arc::new(Mutex::new(Gateway::new(Some(25))));
    let addr = start_gateway(Arc::clone(&gw));
    let mut client = NetClient::new(
        NetConfig::new("n1-test", "alice"),
        AddrContext::Desktop,
        Some(&format!("ws://{addr}/ws")),
        Box::new(WsTransport),
        Box::new(StdHttp),
        Box::new(MemoryStore::new()),
    )
    .expect("a local address");
    let clock = Clock(Instant::now());
    let mut events = Vec::new();

    // In: login, attach, basis and keyframe, catalog over HTTP.
    client.login("alice", PASSWORD, clock.now());
    let entered = spin(
        &mut client,
        &clock,
        &mut events,
        Duration::from_secs(20),
        |c, _| c.phase() == ClientPhase::Playing,
    );
    assert!(entered, "never reached Playing: {:?}", client.phase());
    assert!(client.core().basis().is_some(), "the map basis came");
    assert!(
        client.core().catalog().is_some(),
        "the catalog came over HTTP"
    );
    {
        let g = gw.lock().unwrap();
        assert_eq!(client.core().base_hash(), Some(g.world.view_hash()));
        assert_eq!(client.core().basis_hash(), Some(g.world.basis_hash()));
        assert_eq!(client.core_mut().shown(), Some(&g.world.view));
        assert!(g.logins[0].resume.is_none() && g.logins[0].password == PASSWORD);
    }

    let oracle = {
        let gw = Arc::clone(&gw);
        move || {
            let g = gw.lock().unwrap();
            (g.world.view_seq, g.world.view_hash())
        }
    };
    let first_turn = client.core().turn_state().expect("turn state").turn;
    drive_turns(&mut client, &clock, &mut events, TURNS, Some(&oracle));

    // Twenty turns on, the client shows what the gateway has, and never needed to be corrected.
    let g = gw.lock().unwrap();
    assert_eq!(client.core().turn_state().unwrap().turn, first_turn + TURNS);
    assert_eq!(client.core_mut().shown(), Some(&g.world.view));
    let s = client.core().stats();
    assert_eq!(s.need_keyframe.total(), 0, "no NeedKeyframe: {s:?}");
    assert_eq!(
        (s.undo_restores, s.decode_errors, s.stream_breaks),
        (0, 0, 0)
    );
    assert_eq!(s.keyframes, 1, "one keyframe, the reconnect was a replay");
    assert_eq!((s.attached_fresh, s.attached_replay), (1, 1));
    assert!(s.diffs >= 40, "a diff for every order: {}", s.diffs);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, ClientEvent::StreamBroken(_) | ClientEvent::Fatal(_)))
    );

    // Orders: 40 given, each applied by the gateway once, each answered once; the cut one was repeated.
    let ids: HashSet<u32> = g.book.applied.iter().copied().collect();
    assert_eq!(g.book.applied.len(), 2 * TURNS as usize);
    assert_eq!(ids.len(), g.book.applied.len(), "no order applied twice");
    assert_eq!(s.orders_sent, 2 * TURNS as u64);
    assert!(
        s.orders_resent >= 1,
        "the order without an answer was repeated"
    );
    assert_eq!(s.orders_applied, 2 * TURNS as u64, "one final answer each");
    assert_eq!(client.core().orders_in_flight(), 0);
    let repeats = g.book.seen.iter().filter(|c| c.id == 25).count();
    assert_eq!(repeats, 2, "order 25 reached the gateway twice");
    // The second login named the token and the stream.
    assert_eq!(g.logins.len(), 2);
    assert_eq!(g.logins[1].token, Some(g.token));
    assert_eq!(g.logins[1].password, "");
    assert!(g.logins[1].resume.is_some_and(|r| r.stream == STREAM));
}
