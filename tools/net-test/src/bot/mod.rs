//! `net-bot`: a headless client of protocol v3. A raw peer on `protocol` and `wire` (NT-D3):
//! it decides what to send and when, and can break the rules a real client keeps (the knobs). Not a brain
//! and not a renderer. One bot is one thread with a non-blocking loop; time is `Instant`.

pub mod audit;
pub mod conn;
pub mod contexts;
pub mod ctl;
pub mod knobs;
pub mod ledger;
pub mod mission;
pub mod persona;
pub mod policy;
pub mod register;
pub mod session;
pub mod stats;

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use protocol::{
    ChatSend, ClientMsg, Cmd, CmdId, Command, Focus, GameCtl, KeyframeReason, LobbyCmd, Login,
    PROTOCOL_VERSION, Query, Token,
};
use serde_json::{Value, json};

use self::conn::{Conn, Poll};
use self::policy::{Done, Policy, Progress};
use self::session::{Effect, PendingReconnect, Session, note_up};

/// Unix time now, ms.
fn unix_ms_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Adds `ts` (unix ms) to an event object that has none.
fn stamp_unix_ms(ev: &mut Value, ms: u64) {
    if let Some(o) = ev.as_object_mut() {
        o.entry("ts").or_insert_with(|| json!(ms));
    }
}

#[cfg(test)]
mod ts_tests {
    use super::*;

    #[test]
    fn events_get_an_absolute_utc_ts_once() {
        let mut ev = json!({ "ev": "closed", "t_ms": 5 });
        stamp_unix_ms(&mut ev, 1_761_909_000_123);
        assert_eq!(ev["ts"], 1_761_909_000_123_u64);
        stamp_unix_ms(&mut ev, 9);
        assert_eq!(ev["ts"], 1_761_909_000_123_u64, "an existing ts is kept");
        assert!(unix_ms_now() > 1_700_000_000_000);
    }
}

/// Orders in the air at most (the gateway window, net-protocol 4.8).
pub const WINDOW: usize = 32;

#[derive(Debug, thiserror::Error)]
pub enum BotError {
    #[error("websocket: {0}")]
    Ws(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("codec: {0}")]
    Codec(String),
    #[error("not connected")]
    NotConnected,
    #[error("{0}")]
    Protocol(String),
    #[error("timeout: {0}")]
    Timeout(String),
}

#[derive(Debug, Clone)]
pub struct BotConfig {
    /// `ws://host:port/ws`.
    pub url: String,
    pub nick: String,
    pub password: String,
    /// The token is read from here at the start and written here at every login.
    pub token_file: Option<PathBuf>,
    /// Without a token, make the account over HTTP first (`register`), as a page does. `password` is then the
    /// registration code. Off (the default of a library user, whose server may be a fake that speaks WebSocket
    /// only): log in over the WebSocket with `password` only (a server with `auto_register`). `ctl` and the
    /// missions turn it on.
    pub register: bool,
    /// `timeline.jsonl`: every event, for the post-mortem.
    pub timeline: Option<PathBuf>,
    /// Come back by itself after a drop (policy `simple`: 0.5 s, doubling to 10 s).
    pub auto_reconnect: bool,
    /// Application `Ping` every so often (the gateway's ping/pong needs none to keep the socket).
    pub ping_every: Duration,
}

impl BotConfig {
    pub fn new(url: &str, nick: &str, password: &str) -> BotConfig {
        BotConfig {
            url: url.to_owned(),
            nick: nick.to_owned(),
            password: password.to_owned(),
            token_file: None,
            register: false,
            timeline: None,
            auto_reconnect: true,
            ping_every: Duration::from_secs(5),
        }
    }
}

/// The socket is gone and the bot is going to try again.
#[derive(Debug)]
struct Down {
    next_try: Instant,
    backoff: Duration,
}

pub struct Bot {
    pub cfg: BotConfig,
    pub s: Session,
    conn: Option<Conn>,
    t0: Instant,
    timeline: Option<File>,
    outbox: Vec<Value>,
    last_ping: Instant,
    frozen_until: Option<Instant>,
    down: Option<Down>,
    resend_after_attach: bool,
    next_query: u32,
    /// The last token written to `token_file`.
    saved_token: Option<Token>,
    /// The autoplay policy, when it is on: `pump` lets it act.
    pub driver: Option<Box<Policy>>,
    /// Why the policy stopped.
    pub play_done: Option<Done>,
}

fn token_hex(t: &Token) -> String {
    t.0.iter().map(|b| format!("{b:02x}")).collect()
}

fn token_from_hex(s: &str) -> Option<Token> {
    let s = s.trim();
    if s.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(Token(out))
}

impl Bot {
    pub fn new(cfg: BotConfig) -> Bot {
        let t0 = Instant::now();
        let mut s = Session::new(&cfg.nick, t0);
        if let Some(path) = &cfg.token_file
            && let Ok(text) = std::fs::read_to_string(path)
        {
            s.token = token_from_hex(&text);
        }
        let timeline = cfg
            .timeline
            .as_ref()
            .and_then(|p| OpenOptions::new().create(true).append(true).open(p).ok());
        let saved_token = s.token;
        Bot {
            cfg,
            s,
            conn: None,
            t0,
            timeline,
            outbox: Vec::new(),
            last_ping: t0,
            frozen_until: None,
            down: None,
            resend_after_attach: false,
            next_query: 1,
            saved_token,
            driver: None,
            play_done: None,
        }
    }

    pub fn t_ms(&self) -> u64 {
        self.t0.elapsed().as_millis() as u64
    }

    pub fn is_connected(&self) -> bool {
        self.conn.is_some()
    }

    /// The bot is down and will try again by itself.
    pub fn is_reconnecting(&self) -> bool {
        self.down.is_some()
    }

    // ------------------------------------------------------------ events

    pub fn emit(&mut self, mut ev: Value) {
        // Every event line carries the absolute UTC time (unix ms, as the server's and the page's `ts`), beside the
        // run-relative `t_ms` (SIMP-exe-1002).
        stamp_unix_ms(&mut ev, unix_ms_now());
        if let Some(f) = self.timeline.as_mut() {
            let _ = writeln!(f, "{ev}");
        }
        self.outbox.push(ev);
    }

    /// The events since the last call (`ctl` prints them).
    pub fn drain_events(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.outbox)
    }

    // ------------------------------------------------------------ connection

    /// Opens a socket and logs in. `resume`: continue the stream of the last `Attached` (with the token);
    /// without it nothing is in memory, as after a reload of the page. `reason` is set for a reconnect.
    pub fn connect(&mut self, resume: bool, reason: Option<(&str, bool)>) -> Result<(), BotError> {
        // The resume point and the claim are taken before the login forgets the stream.
        let resume_point = if resume { self.s.resume_point() } else { None };
        if let Some((why, planned)) = reason {
            // A drop that is already being mended keeps its first moment.
            self.s.pending_reconnect.get_or_insert(PendingReconnect {
                since: Instant::now(),
                reason: why.to_owned(),
                planned,
                mode: None,
            });
        }
        if let Some(old) = self.conn.take() {
            old.drop_hard();
        }
        self.s.begin_login(resume);
        if self.s.token.is_none() && self.cfg.register {
            // A server without `auto_register` knows no password login of an unknown nick: make the account first.
            if let Some(t) = register::token_for(&self.cfg.url, &self.cfg.nick, &self.cfg.password)
            {
                self.s.token = Some(t);
            }
        }
        let conn = Conn::open(&self.cfg.url)?;
        self.conn = Some(conn);
        self.down = None;
        self.last_ping = Instant::now();
        self.frozen_until = None;
        self.resend_after_attach = !self.s.knobs.no_resend && self.s.ledger.open() > 0;
        let nick = self.cfg.nick.clone();
        self.send(&ClientMsg::Hello {
            protocol: PROTOCOL_VERSION,
            build: "net-bot".into(),
            nick,
        })?;
        let password = if self.s.token.is_some() {
            String::new()
        } else {
            self.cfg.password.clone()
        };
        let login = Login {
            password,
            token: self.s.token,
            resume: resume_point,
        };
        self.send(&ClientMsg::Login(login))?;
        self.emit(
            json!({ "ev": "lobby", "what": "connect", "resume": resume, "t_ms": self.t_ms() }),
        );
        Ok(())
    }

    /// Reconnects now (an operator's `reconnect`, or the stream broke).
    pub fn reconnect(&mut self, resume: bool, reason: &str, planned: bool) -> Result<(), BotError> {
        self.connect(resume, Some((reason, planned)))
    }

    /// The link drops without a close frame; the bot does not come back by itself.
    pub fn drop_link(&mut self) {
        if let Some(c) = self.conn.take() {
            c.drop_hard();
        }
        self.down = None;
        self.emit(json!({ "ev": "closed", "reason": "dropped by the bot", "planned": true, "t_ms": self.t_ms() }));
    }

    /// A polite close.
    pub fn close(&mut self) {
        if let Some(c) = self.conn.take() {
            c.close();
        }
        self.down = None;
    }

    /// Stops reading the socket for `ms` (`freeze_ms`): the server's pings go unanswered, the socket lives.
    pub fn freeze(&mut self, ms: u64) {
        self.frozen_until = Some(Instant::now() + Duration::from_millis(ms));
    }

    // ------------------------------------------------------------ sending

    /// Sends one message; returns the moment it was handed to the socket.
    pub fn send(&mut self, msg: &ClientMsg) -> Result<Instant, BotError> {
        let conn = self.conn.as_mut().ok_or(BotError::NotConnected)?;
        let (bytes, at) = conn.send(msg)?;
        let turn = self.s.turn_number();
        note_up(&mut self.s.stats, turn, bytes);
        Ok(at)
    }

    pub fn send_raw(&mut self, bytes: Vec<u8>) -> Result<(), BotError> {
        let conn = self.conn.as_mut().ok_or(BotError::NotConnected)?;
        let n = bytes.len();
        conn.send_raw(bytes)?;
        let turn = self.s.turn_number();
        note_up(&mut self.s.stats, turn, n);
        Ok(())
    }

    /// Room for another order in the air (the limit of 32 unless the `window` knob lifted it).
    pub fn can_send(&self) -> bool {
        self.s.knobs.window || self.s.ledger.open() < WINDOW
    }

    /// Sends an order: a new `id` unless one is given. `dup` extra copies follow at once (and the
    /// `dup_cmds` knob adds its own).
    pub fn send_cmd(
        &mut self,
        command: Command,
        id: Option<CmdId>,
        dup: u32,
    ) -> Result<CmdId, BotError> {
        if self.conn.is_none() {
            return Err(BotError::NotConnected);
        }
        if id.is_none() && !self.can_send() {
            return Err(BotError::Protocol(format!(
                "{WINDOW} orders are in the air (the window knob lifts the limit)"
            )));
        }
        let id = id.unwrap_or_else(|| self.s.alloc_id());
        let cmd = Cmd {
            id,
            turn: self.s.turn_number(),
            cmd: command,
        };
        let frames = 1 + dup + self.s.knobs.dup_cmds;
        for i in 0..frames {
            let at = self.send(&ClientMsg::Cmd(cmd))?;
            self.s.ledger.sent(&cmd, at, i > 0);
        }
        Ok(id)
    }

    pub fn send_focus(&mut self, focus: Focus) -> Result<(), BotError> {
        self.send(&ClientMsg::Focus(focus)).map(|_| ())
    }

    /// A `Query`; the answer lands in `s.query_answers[id]`.
    pub fn send_query(&mut self, q: Query) -> Result<u32, BotError> {
        let id = self.next_query;
        self.next_query += 1;
        self.send(&ClientMsg::Query { id, q })?;
        self.s.stats.queries_sent += 1;
        Ok(id)
    }

    pub fn send_lobby(&mut self, cmd: LobbyCmd) -> Result<(), BotError> {
        self.send(&ClientMsg::Lobby(cmd)).map(|_| ())
    }

    pub fn send_chat(&mut self, chat: ChatSend) -> Result<(), BotError> {
        self.send(&ClientMsg::Chat(chat)).map(|_| ())
    }

    pub fn send_game_ctl(&mut self, ctl: GameCtl) -> Result<(), BotError> {
        self.send(&ClientMsg::GameCtl(ctl)).map(|_| ())
    }

    pub fn send_need_keyframe(&mut self, reason: KeyframeReason) -> Result<(), BotError> {
        self.s.awaiting_keyframe = true;
        *self
            .s
            .stats
            .need_keyframe
            .entry(format!("{reason:?}"))
            .or_insert(0) += 1;
        self.send(&ClientMsg::NeedKeyframe { reason }).map(|_| ())
    }

    // ------------------------------------------------------------ the loop

    /// One round: housekeeping, one read (at most 1 ms), then the policy if it is on. True when a message
    /// came.
    pub fn pump(&mut self) -> Result<bool, BotError> {
        let got = self.pump_io()?;
        self.drive()?;
        Ok(got)
    }

    /// The policy makes its move. A refused send (no link) is only a pause for it.
    fn drive(&mut self) -> Result<(), BotError> {
        let Some(mut policy) = self.driver.take() else {
            return Ok(());
        };
        match policy.tick(self) {
            Ok(Progress::Done(d)) => {
                let t = self.t_ms();
                self.emit(json!({ "ev": "autoplay_done", "why": d.text(), "t_ms": t }));
                self.play_done = Some(d);
            }
            Ok(_) | Err(BotError::NotConnected) => self.driver = Some(policy),
            Err(e) => {
                self.s.stats.error(format!("policy: {e}"));
                self.driver = Some(policy);
            }
        }
        Ok(())
    }

    fn pump_io(&mut self) -> Result<bool, BotError> {
        self.housekeeping()?;
        if let Some(until) = self.frozen_until {
            if Instant::now() < until {
                std::thread::sleep(Duration::from_millis(1));
                return Ok(false);
            }
            self.frozen_until = None;
        }
        let Some(conn) = self.conn.as_mut() else {
            std::thread::sleep(Duration::from_millis(1));
            return Ok(false);
        };
        match conn.poll() {
            Ok(Poll::Frame(f)) => {
                let fx = self.s.handle(&f);
                self.after_handle(fx)?;
                Ok(true)
            }
            Ok(Poll::Idle) => Ok(false),
            Ok(Poll::Closed { code, reason }) => {
                self.on_closed(code, &reason);
                Ok(false)
            }
            Err(e) => {
                // A protocol or socket error: the bot records it and treats the socket as closed.
                self.s.stats.error(format!("{e}"));
                self.on_closed(None, &e.to_string());
                Ok(false)
            }
        }
    }

    /// Pumps for `budget`.
    pub fn step(&mut self, budget: Duration) -> Result<(), BotError> {
        let end = Instant::now() + budget;
        while Instant::now() < end {
            self.pump()?;
        }
        Ok(())
    }

    /// Pumps until `pred` is true or `timeout` passes; returns whether it became true.
    pub fn wait_until(
        &mut self,
        timeout: Duration,
        mut pred: impl FnMut(&Bot) -> bool,
    ) -> Result<bool, BotError> {
        let end = Instant::now() + timeout;
        loop {
            if pred(self) {
                return Ok(true);
            }
            if Instant::now() >= end {
                return Ok(false);
            }
            self.pump()?;
        }
    }

    fn after_handle(&mut self, fx: Vec<Effect>) -> Result<(), BotError> {
        for e in fx {
            match e {
                Effect::Send(m) => {
                    self.send(&m)?;
                }
                Effect::Event(v) => self.emit(v),
                Effect::Reconnect(why) => self.reconnect(true, &why, false)?,
            }
        }
        if let Some(t) = self.s.token
            && self.saved_token != Some(t)
        {
            self.saved_token = Some(t);
            if let Some(path) = &self.cfg.token_file {
                let _ = std::fs::write(path, token_hex(&t));
            }
        }
        if self.resend_after_attach && self.s.attach.is_some() {
            self.resend_after_attach = false;
            for cmd in self.s.ledger.unanswered() {
                let at = self.send(&ClientMsg::Cmd(cmd))?;
                self.s.ledger.sent(&cmd, at, false);
            }
        }
        Ok(())
    }

    fn on_closed(&mut self, code: Option<u16>, reason: &str) {
        self.conn = None;
        let replaced = self.s.detached == Some(protocol::DetachReason::Replaced);
        let again = self.cfg.auto_reconnect && self.s.rejected.is_none() && !replaced;
        let t = self.t_ms();
        self.emit(json!({ "ev": "closed", "code": code, "reason": reason, "reconnect": again, "t_ms": t }));
        if again {
            self.s.pending_reconnect.get_or_insert(PendingReconnect {
                since: Instant::now(),
                reason: format!("closed: {reason}"),
                planned: false,
                mode: None,
            });
            self.down = Some(Down {
                next_try: Instant::now() + Duration::from_millis(500),
                backoff: Duration::from_millis(500),
            });
        }
    }

    fn housekeeping(&mut self) -> Result<(), BotError> {
        let now = Instant::now();
        if let Some(d) = &self.down
            && now >= d.next_try
        {
            let backoff = (d.backoff * 2).min(Duration::from_secs(10));
            let why = self
                .s
                .pending_reconnect
                .as_ref()
                .map_or("closed".to_owned(), |p| p.reason.clone());
            if let Err(e) = self.reconnect(true, &why, false) {
                self.s.stats.error(format!("reconnect failed: {e}"));
                self.down = Some(Down {
                    next_try: Instant::now() + backoff,
                    backoff,
                });
            }
        }
        if self.conn.is_some()
            && self.s.logged_in
            && !self.s.knobs.no_ping
            && self.frozen_until.is_none()
            && now.duration_since(self.last_ping) >= self.cfg.ping_every
        {
            self.last_ping = now;
            let ping = self.s.ping(now);
            self.send(&ping)?;
        }
        Ok(())
    }

    // ------------------------------------------------------------ result

    /// `result.json` (`schema: 1`).
    pub fn result_json(&self) -> Value {
        let s = &self.s;
        let c = s.ledger.counts();
        let st = &s.stats;
        let rtt = st.rtt_ms.summary();
        let (view_hash, view_turn) = s.view.as_ref().map_or((0, 0), |(v, h)| (h.value(), v.turn));
        json!({
            "schema": 1,
            "bot": self.cfg.nick,
            "nick": self.cfg.nick,
            "player": s.attach.as_ref().map(|a| a.player),
            "turns_played": st.turns_played,
            "cmds": {
                "sent": c.sent, "unique": c.unique, "applied": c.applied, "rejected": c.rejected,
                "lost": c.lost, "queued": c.queued, "dup_sent": c.dup_sent,
            },
            "latency_ms": st.latency_ms.summary(),
            "rtt_ms": rtt,
            "rtt_median": rtt.p50,
            "bytes": st.bytes_json(),
            "hash": { "checked": st.hash_checked, "mismatch": st.hash_mismatch },
            "need_keyframe": st.need_keyframe,
            "queries_sent": st.queries_sent,
            "contexts_used": st.contexts_used,
            "reconnects": st.reconnects,
            "fog": { "violations": s.audit.violations, "examples": s.audit.examples, "checked": s.audit.checked },
            "refused_offline": st.refused_offline,
            "host_changes": st.host_changes,
            "ledger": {
                "open": c.open, "double_outcome": c.double_outcome,
                "repeated_answers": c.repeated_answers, "unknown_answers": c.unknown_answers,
            },
            "lobby_errors": st.lobby_errors,
            // The game's own journal as the player gets it: the number of lines and the Lua errors among them.
            "journal": {
                "lines": s.journal.len(),
                "lua_errors": s.journal.iter().filter(|e| matches!(e.kind, protocol::JournalKind::LuaError)).count(),
                "lua_error_examples": s.journal.iter().filter(|e| matches!(e.kind, protocol::JournalKind::LuaError)).take(5).map(|e| e.text.clone()).collect::<Vec<_>>(),
            },
            "ignored_diffs": st.ignored_diffs,
            "modes": s.modes.iter().map(|m| format!("{m:?}")).collect::<Vec<_>>(),
            "first_view": st.first_view.map(|(turn, hash)| json!({ "turn": turn, "view_hash": hash })),
            "final": {
                "turn": s.turn_number(), "view_turn": view_turn, "view_hash": view_hash,
                "view_seq": s.view_seq, "seq": s.last_seq,
                "basis_hash": s.basis.as_ref().map(protocol::BasisState::hash),
            },
            "errors": st.errors,
        })
    }
}
