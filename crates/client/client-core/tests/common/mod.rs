//! Scripted server double and a simulated network for `ClientCore` tests (net-client 9).
//!
//! `Server` speaks protocol v3 the way the gateway does for login, attach and replace; `Net` is the shell:
//! it executes the socket commands of the core on a simulated line with a one-way delay, a switch that
//! drops everything (`channel_up`) and a switch that mutes the server (`mute`). Time is a plain `u64`.

#![allow(dead_code)]

use std::collections::HashMap;

pub mod orders;
pub mod views;
pub mod world;

use client_core::{
    ClientCore, ClientEvent, ConnState, DisconnectWhy, HostEvent, NetConfig, Sock, SocketCmd,
};
use protocol::{
    AttachMode, Attached, CatalogRef, ClientMsg, Conn, DetachReason, GameBatch, GameMsg,
    GameSettings, LobbyChoices, LobbyState, PROTOCOL_VERSION, RejectReason, RoomState, SeatControl,
    SeatView, ServerMsg, Session, Token,
};
use wire::{Encoder, Limits};

pub const NICK: &str = "alice";
pub const PASSWORD: &str = "hunter2-test-password";

pub fn settings() -> GameSettings {
    GameSettings {
        name: "r".into(),
        world_size: "S".into(),
        map_script: "M".into(),
        map_options: vec![],
        speed: "s".into(),
        start_era: "e".into(),
        handicap: "h".into(),
        minor_civs: 0,
        turn_mode: protocol::TurnMode::Hybrid,
        timer: protocol::TimerMode::Off,
        game_options: vec![],
        seed: None,
        map_seed: None,
        auto_choices: Default::default(),
        diplo_auto_decline: false,
        mods: Vec::new(),
    }
}

pub fn room(id: u32, host: &str) -> RoomState {
    RoomState {
        id,
        name: "room".into(),
        host: host.into(),
        status: protocol::GameStatus::Lobby,
        settings: settings(),
        slots: vec![],
        from_save: None,
        mod_plan: protocol::ModPlan::default(),
    }
}

pub fn seat(player: i8, nick: &str, host: bool) -> SeatView {
    SeatView {
        player,
        control: SeatControl::Human { nick: nick.into() },
        conn: Conn::Online,
        host,
    }
}

pub fn catalog_ref() -> CatalogRef {
    CatalogRef {
        hash: 0xCA7A,
        len: 10,
    }
}

/// What the double does after a login.
#[derive(Debug, Clone)]
pub struct GamePlan {
    pub game: u32,
    pub player: i8,
    pub stream: u64,
    pub last_cmd_id: u32,
    /// Position of the stream: `Replay` is offered while the client's `last_seq` is not beyond it.
    pub last_seq: u64,
}

#[derive(Default)]
struct SrvSock {
    token: Option<Token>,
    nick: String,
    logged: bool,
}

pub struct Server {
    pub password: String,
    /// A client with a known token may log in without the password (GS-D2).
    pub tokens: HashMap<[u8; 16], String>,
    pub plan: Option<GamePlan>,
    pub full: bool,
    pub protocol: u32,
    pub room_host: Option<String>,
    /// The game the plan attaches to: keyframes, diffs and the log to replay.
    pub world: Option<world::World>,
    /// The server does not answer `NeedKeyframe` (a lost request).
    pub ignore_need_keyframe: bool,
    /// Reasons of the `NeedKeyframe` messages that came.
    pub need_keyframe: Vec<protocol::KeyframeReason>,
    /// Orders: numbers, answers, the ring of repeats.
    pub book: orders::OrderBook,
    /// Queries get an `Error` answer (the double has no game to ask); off: they stay unanswered.
    pub answer_queries: bool,
    socks: HashMap<Sock, SrvSock>,
    next_token: u8,
    /// Everything the clients sent, in order, with the socket.
    pub log: Vec<(Sock, ClientMsg)>,
    pub logins: Vec<(Sock, protocol::Login)>,
    /// The account's store (v12): an empty change asks for it, any other is applied.
    pub account: Vec<(String, String)>,
}

impl Server {
    pub fn new() -> Self {
        Server {
            password: PASSWORD.into(),
            tokens: HashMap::new(),
            plan: None,
            full: false,
            protocol: PROTOCOL_VERSION,
            room_host: None,
            world: None,
            ignore_need_keyframe: false,
            need_keyframe: Vec::new(),
            book: orders::OrderBook::new(),
            answer_queries: true,
            socks: HashMap::new(),
            next_token: 1,
            log: Vec::new(),
            logins: Vec::new(),
            account: Vec::new(),
        }
    }

    pub fn sent(&self, f: impl Fn(&ClientMsg) -> bool) -> usize {
        self.log.iter().filter(|(_, m)| f(m)).count()
    }

    pub fn sock_of(&self, token: Token) -> Option<Sock> {
        self.socks
            .iter()
            .filter(|(_, s)| s.logged && s.token == Some(token))
            .map(|(k, _)| *k)
            .max()
    }

    /// The server saw the socket close.
    pub fn closed(&mut self, sock: Sock) {
        self.socks.remove(&sock);
    }

    pub fn recv(&mut self, sock: Sock, msg: ClientMsg) -> Vec<(Sock, ServerMsg)> {
        self.log.push((sock, msg.clone()));
        let mut out = Vec::new();
        match msg {
            ClientMsg::Hello { protocol, nick, .. } => {
                if protocol != self.protocol {
                    out.push((
                        sock,
                        ServerMsg::Reject(RejectReason::VersionMismatch {
                            server: self.protocol,
                        }),
                    ));
                } else {
                    self.socks.entry(sock).or_default().nick = nick;
                    out.push((
                        sock,
                        ServerMsg::Welcome {
                            session: 1,
                            build: "srv".into(),
                        },
                    ));
                }
            }
            ClientMsg::Login(login) => {
                self.logins.push((sock, login.clone()));
                let nick = self.socks.entry(sock).or_default().nick.clone();
                if self.full {
                    out.push((sock, ServerMsg::Reject(RejectReason::Full)));
                    return out;
                }
                let known = login
                    .token
                    .and_then(|t| self.tokens.get(&t.0).map(|n| (t, n.clone())));
                let token = match known {
                    Some((t, n)) if n == nick => t,
                    _ => {
                        if login.password != self.password {
                            out.push((sock, ServerMsg::Reject(RejectReason::BadPassword)));
                            return out;
                        }
                        let t = Token([self.next_token; 16]);
                        self.next_token += 1;
                        self.tokens.insert(t.0, nick.clone());
                        t
                    }
                };
                // The same token elsewhere is replaced (GS: before its socket is closed).
                let old: Vec<Sock> = self
                    .socks
                    .iter()
                    .filter(|(k, s)| **k != sock && s.logged && s.token == Some(token))
                    .map(|(k, _)| *k)
                    .collect();
                for o in old {
                    out.push((
                        o,
                        ServerMsg::Detached {
                            reason: DetachReason::Replaced,
                        },
                    ));
                    if let Some(s) = self.socks.get_mut(&o) {
                        s.logged = false;
                    }
                }
                let s = self.socks.entry(sock).or_default();
                s.token = Some(token);
                s.logged = true;
                out.push((
                    sock,
                    ServerMsg::LoggedIn(Session {
                        token: Some(token),
                        nick: nick.clone(),
                    }),
                ));
                out.push((sock, ServerMsg::Choices(LobbyChoices::default())));
                match self.plan.clone() {
                    Some(p) => {
                        let have = self.world.as_ref().map_or(p.last_seq, |w| w.seq);
                        let replay = login
                            .resume
                            .is_some_and(|r| r.stream == p.stream && r.last_seq <= have);
                        out.push((
                            sock,
                            ServerMsg::Attached(Attached {
                                game: p.game,
                                player: p.player,
                                stream: p.stream,
                                mode: if replay {
                                    AttachMode::Replay
                                } else {
                                    AttachMode::Fresh
                                },
                                catalog: catalog_ref(),
                                last_cmd_id: p.last_cmd_id.max(self.book.last),
                            }),
                        ));
                        if let Some(w) = &mut self.world {
                            if replay {
                                let last = login.resume.map_or(0, |r| r.last_seq);
                                for m in w.tail(last) {
                                    out.push((sock, m));
                                }
                            } else {
                                w.reset_stream();
                                let claim = login.resume.and_then(|r| r.basis);
                                let held = claim
                                    == Some(protocol::BasisClaim {
                                        game: w.game,
                                        hash: w.basis_hash(),
                                    });
                                out.push((sock, w.keyframe(!held)));
                            }
                        }
                    }
                    None => out.push((sock, ServerMsg::LobbyState(self.lobby_state()))),
                }
            }
            ClientMsg::Account(save) => {
                if !save.is_empty() {
                    let _ = protocol::apply_save(&mut self.account, &save);
                } else {
                    out.push((
                        sock,
                        ServerMsg::Account(protocol::AccountData {
                            version: protocol::ACCOUNT_VERSION,
                            prefs: self.account.clone(),
                            last_room: None,
                        }),
                    ));
                }
            }
            ClientMsg::NeedKeyframe { reason } => {
                self.need_keyframe.push(reason);
                if !self.ignore_need_keyframe
                    && let Some(w) = &mut self.world
                {
                    out.push((sock, w.answer(reason)));
                }
            }
            ClientMsg::Cmd(c) => {
                if let Some(w) = &mut self.world {
                    for m in self.book.on_cmd(c, w) {
                        out.push((sock, m));
                    }
                }
            }
            ClientMsg::Query { id, .. } if self.answer_queries => {
                out.push((
                    sock,
                    ServerMsg::QueryResult {
                        id,
                        result: protocol::QueryAnswer::Error {
                            reason: "double".into(),
                        },
                    },
                ));
            }
            _ => {}
        }
        out
    }

    /// The worker answers an order it held.
    pub fn answer_order(&mut self, id: u32, outcome: protocol::CommandOutcome) -> Vec<ServerMsg> {
        let w = self.world.as_mut().expect("a game");
        self.book.answer(id, outcome, w)
    }

    pub fn lobby_state(&self) -> LobbyState {
        LobbyState {
            room: self.room_host.as_deref().map(|h| room(1, h)),
            ..Default::default()
        }
    }
}

pub fn frame(msg: &ServerMsg) -> Vec<u8> {
    Encoder::default().frame_msg(msg).expect("encode")
}

pub fn client_msg(frame: &[u8]) -> ClientMsg {
    wire::decode(frame, &Limits::GATEWAY).expect("client frame is enc 0 postcard")
}

pub fn batch(seq: u64, msgs: Vec<GameMsg>) -> ServerMsg {
    ServerMsg::Game(GameBatch { seq, msgs })
}

enum Sim {
    /// Opens at the given time if the channel is up then.
    Opening(u64),
    Open,
    /// Never opens (the SYN was lost) or closed.
    Dead,
}

enum Delivery {
    ToServer(Sock, Vec<u8>),
    ToClient(Sock, Vec<u8>),
}

pub struct Net {
    pub core: ClientCore,
    pub srv: Server,
    pub now: u64,
    /// One-way delay, ms. TCP + TLS + WS take three round trips to open.
    pub one_way: u64,
    pub channel_up: bool,
    /// The server stays silent (sockets stay up).
    pub mute: bool,
    /// While the channel is down a connect attempt fails at once (refused) instead of hanging.
    pub refuse_when_down: bool,
    /// Sockets that carry nothing any more (a TCP stuck in its retransmit back-off).
    stuck: std::collections::HashSet<Sock>,
    socks: HashMap<Sock, Sim>,
    inflight: Vec<(u64, Delivery)>,
    pub events: Vec<ClientEvent>,
    /// Every `Open` the core asked for: time, socket, probe.
    pub opens: Vec<(u64, Sock, bool)>,
    pub closes: Vec<(u64, Sock)>,
    pub states: Vec<(u64, ConnState)>,
}

impl Net {
    pub fn new(cfg: NetConfig) -> Self {
        Net {
            core: ClientCore::new(cfg),
            srv: Server::new(),
            now: 1_000,
            one_way: 0,
            channel_up: true,
            mute: false,
            refuse_when_down: false,
            stuck: Default::default(),
            socks: HashMap::new(),
            inflight: Vec::new(),
            events: Vec::new(),
            opens: Vec::new(),
            closes: Vec::new(),
            states: Vec::new(),
        }
    }

    pub fn start() -> Net {
        Net::new(NetConfig::new("test-build", NICK))
    }

    /// Connects and runs until the first quiet moment.
    pub fn connect(&mut self) {
        self.core.connect(self.now);
        self.settle();
    }

    /// Logs in with the typed password and settles.
    pub fn login(&mut self) {
        self.core.login(NICK, PASSWORD);
        self.settle();
    }

    pub fn take_events(&mut self) -> Vec<ClientEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn has_event(&self, f: impl Fn(&ClientEvent) -> bool) -> bool {
        self.events.iter().any(f)
    }

    pub fn count_events(&self, f: impl Fn(&ClientEvent) -> bool) -> usize {
        self.events.iter().filter(|e| f(e)).count()
    }

    /// The server pushes a message to the socket the token's session is on.
    pub fn push(&mut self, token: Token, msg: ServerMsg) {
        let sock = self.srv.sock_of(token).expect("token has a socket");
        self.push_to(sock, msg);
    }

    pub fn push_to(&mut self, sock: Sock, msg: ServerMsg) {
        self.send_to_client(sock, frame(&msg));
    }

    /// The line goes dead: nothing gets through, and the sockets open now stay stuck even after it returns.
    pub fn outage_start(&mut self) {
        self.channel_up = false;
        let open: Vec<Sock> = self
            .socks
            .iter()
            .filter(|(_, s)| matches!(s, Sim::Open))
            .map(|(k, _)| *k)
            .collect();
        self.stuck.extend(open);
        self.inflight.clear();
    }

    pub fn outage_end(&mut self) {
        self.channel_up = true;
    }

    /// The server closes the current socket of the token's session.
    pub fn server_drops(&mut self, token: Token, code: u16) {
        let sock = self.srv.sock_of(token).expect("token has a socket");
        self.drop_sock(sock, DisconnectWhy::Remote(code));
    }

    pub fn drop_sock(&mut self, sock: Sock, why: DisconnectWhy) {
        self.socks.insert(sock, Sim::Dead);
        self.srv.closed(sock);
        self.core.on_disconnected(self.now, sock, why);
        self.settle();
    }

    /// Sockets that are open now, sorted.
    pub fn open_socks(&self) -> Vec<Sock> {
        let mut v: Vec<Sock> = self
            .socks
            .iter()
            .filter(|(_, s)| matches!(s, Sim::Open))
            .map(|(k, _)| *k)
            .collect();
        v.sort();
        v
    }

    fn send_to_client(&mut self, sock: Sock, bytes: Vec<u8>) {
        if self.mute || !self.channel_up || self.stuck.contains(&sock) {
            return;
        }
        self.inflight
            .push((self.now + self.one_way, Delivery::ToClient(sock, bytes)));
    }

    /// Executes everything pending at `now` until nothing moves.
    pub fn settle(&mut self) {
        for _ in 0..100 {
            if !self.step_io() {
                return;
            }
        }
        panic!("the simulated network does not settle");
    }

    /// Advances time in 10 ms steps, ticking the core and the network.
    pub fn advance(&mut self, ms: u64) {
        let end = self.now + ms;
        while self.now < end {
            self.now = (self.now + 10).min(end);
            self.core.tick(self.now);
            self.settle();
        }
    }

    /// Like `advance`, with a coarser step: for long waits where only the clock matters.
    pub fn advance_coarse(&mut self, ms: u64, step: u64) {
        let end = self.now + ms;
        while self.now < end {
            self.now = (self.now + step).min(end);
            self.core.tick(self.now);
            self.settle();
        }
    }

    pub fn advance_until(&mut self, limit_ms: u64, mut done: impl FnMut(&Net) -> bool) -> bool {
        let end = self.now + limit_ms;
        while self.now < end {
            if done(self) {
                return true;
            }
            self.advance(10);
        }
        done(self)
    }

    pub fn host_event(&mut self, ev: HostEvent) {
        self.core.host_event(self.now, ev);
        self.settle();
    }

    /// One pass: socket commands, due deliveries, outgoing frames, events. `true` if anything happened.
    fn step_io(&mut self) -> bool {
        let mut moved = false;
        let mut refused = Vec::new();
        while let Some(cmd) = self.core.poll_socket_cmd() {
            moved = true;
            match cmd {
                SocketCmd::Open { sock, probe } => {
                    self.opens.push((self.now, sock, probe));
                    let sim = if self.channel_up {
                        // SYN, TLS, upgrade: three round trips.
                        Sim::Opening(self.now + 6 * self.one_way)
                    } else {
                        if self.refuse_when_down {
                            refused.push(sock);
                        }
                        Sim::Dead
                    };
                    self.socks.insert(sock, sim);
                }
                SocketCmd::Close { sock } => {
                    self.closes.push((self.now, sock));
                    self.socks.insert(sock, Sim::Dead);
                    self.srv.closed(sock);
                    self.core
                        .on_disconnected(self.now, sock, DisconnectWhy::Local);
                }
            }
        }
        for sock in refused {
            self.core
                .on_disconnected(self.now, sock, DisconnectWhy::Error);
        }
        let now = self.now;
        let opening: Vec<Sock> = self
            .socks
            .iter()
            .filter(|(_, s)| matches!(s, Sim::Opening(t) if *t <= now))
            .map(|(k, _)| *k)
            .collect();
        for sock in opening {
            if self.channel_up {
                self.socks.insert(sock, Sim::Open);
                self.core.on_connected(now, sock);
                moved = true;
            }
        }
        while let Some((sock, bytes)) = self.core.poll_outgoing() {
            moved = true;
            if matches!(self.socks.get(&sock), Some(Sim::Open))
                && self.channel_up
                && !self.stuck.contains(&sock)
            {
                self.inflight
                    .push((now + self.one_way, Delivery::ToServer(sock, bytes)));
            }
        }
        let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut self.inflight)
            .into_iter()
            .partition(|(t, _)| *t <= now);
        self.inflight = later;
        for (_, d) in due {
            moved = true;
            match d {
                Delivery::ToServer(sock, bytes) => {
                    let replies = self.srv.recv(sock, client_msg(&bytes));
                    for (to, msg) in replies {
                        self.send_to_client(to, frame(&msg));
                    }
                    // A Ping is answered by the transport layer of the double.
                    if let ClientMsg::Ping { seq, client_ms } = client_msg(&bytes) {
                        self.send_to_client(
                            sock,
                            frame(&ServerMsg::Pong {
                                seq,
                                client_ms,
                                server_ms: now + 5_000_000,
                            }),
                        );
                    }
                }
                Delivery::ToClient(sock, bytes) => {
                    if matches!(self.socks.get(&sock), Some(Sim::Open)) {
                        self.core.on_bytes(now, sock, &bytes);
                    }
                }
            }
        }
        while let Some(ev) = self.core.poll_event() {
            moved = true;
            if let ClientEvent::ConnChanged(s) = &ev {
                self.states.push((now, *s));
            }
            self.events.push(ev);
        }
        moved
    }
}
