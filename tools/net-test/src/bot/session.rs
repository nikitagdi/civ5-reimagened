//! The protocol brain of a bot: login, `Attached`,
//! the batches of the stream and every check on them: `seq` in a row, the order inside a batch, `base ==
//! view_seq`, the view hash after `apply`, the map basis. It owns the bot's copy of the view, its books of
//! orders (`ledger`) and its measurements (`stats`). It never touches the socket: `handle` returns the
//! things to send and the events for the timeline.

use std::collections::BTreeMap;
use std::time::Instant;

use protocol::{
    AttachMode, Attached, BasisClaim, BasisState, ChatLine, ClientMsg, CmdId, Codec,
    CommandOutcome, DetachReason, GameBatch, GameCtl, GameCtlReason, GameMsg, JournalEntry,
    KeyframeReason, LobbyChoices, LobbyState, Postcard, QueryAnswer, RejectReason, Resume,
    SeatView, ServerMsg, Token, TurnPhase, TurnState, UndoLog, ViewHash, WorldView, apply, check,
    check_pair,
};
use serde_json::{Value, json};

use super::audit::Audit;
use super::conn::Frame;
use super::contexts::Contexts;
use super::knobs::{ClaimBasis, Knobs};
use super::ledger::{Answer, Ledger, Outcome};
use super::stats::Stats;

/// What `handle` asks the bot to do.
#[derive(Debug)]
pub enum Effect {
    Send(Box<ClientMsg>),
    /// The stream cannot go on (a gap): go back with the token and the resume point.
    Reconnect(String),
    /// A line for `timeline.jsonl` (and stdout in `ctl` mode).
    Event(Value),
}

/// `Attached` as the bot keeps it.
#[derive(Debug, Clone)]
pub struct Attach {
    pub game: u32,
    pub player: i8,
    pub stream: u64,
    pub mode: AttachMode,
    /// Where the catalog of the game is (`GET /catalog/{hash}`).
    pub catalog: protocol::CatalogRef,
    pub last_cmd_id: CmdId,
}

/// A reconnect in progress: it ends when the view is current again.
#[derive(Debug, Clone)]
pub struct PendingReconnect {
    pub since: Instant,
    pub reason: String,
    pub planned: bool,
    pub mode: Option<AttachMode>,
}

pub struct Session {
    pub nick: String,
    pub token: Option<Token>,
    pub logged_in: bool,
    pub rejected: Option<RejectReason>,
    pub choices: Option<LobbyChoices>,
    pub lobby: Option<LobbyState>,
    pub attach: Option<Attach>,
    pub detached: Option<DetachReason>,
    /// `Attached` modes in the order they came.
    pub modes: Vec<AttachMode>,
    pub last_seq: u64,
    /// `seq` of the batch with the last view message.
    pub view_seq: u64,
    pub view: Option<(WorldView, ViewHash)>,
    pub basis: Option<BasisState>,
    /// Hashes of every basis the bot held, oldest first.
    pub basis_history: Vec<u64>,
    pub turn: Option<TurnState>,
    pub seats: Vec<SeatView>,
    pub contexts: Contexts,
    pub journal: Vec<JournalEntry>,
    pub chat: Vec<ChatLine>,
    pub query_answers: BTreeMap<u32, QueryAnswer>,
    pub gamectl_rejected: Vec<(GameCtl, GameCtlReason)>,
    /// The last order refused and the reason the gateway gave.
    pub last_reject: Option<(CmdId, String)>,
    /// A keyframe was asked for and has not come: diffs are not applied meanwhile.
    pub awaiting_keyframe: bool,
    pub next_id: CmdId,
    pub pending_reconnect: Option<PendingReconnect>,
    pub stats: Stats,
    pub ledger: Ledger,
    pub knobs: Knobs,
    /// Set when the bot spoiled its own view (`corrupt_view`), once.
    pub corrupted: bool,
    /// The client-side fog audit (N1.3, second source).
    pub audit: Audit,
    pub dropped_basis: bool,
    pings: BTreeMap<u32, Instant>,
    ping_seq: u32,
    /// The last `Pong.server_ms` and when it was read: the bot's estimate of the server clock.
    pub server_clock: Option<(u64, Instant)>,
    t0: Instant,
    /// This entry was `Fresh` and no `MapBasis` has come in it yet (to count `basis.skipped`).
    fresh_basis_open: bool,
}

fn kind_of(m: &GameMsg) -> &'static str {
    match m {
        GameMsg::Keyframe(_) => "Keyframe",
        GameMsg::Diff(_) => "Diff",
        GameMsg::Results(_) => "Results",
        GameMsg::Journal(_) => "Journal",
        GameMsg::TurnState(_) => "TurnState",
        GameMsg::Seats(_) => "Seats",
        GameMsg::Context(_) => "Context",
        GameMsg::Chat(_) => "Chat",
        GameMsg::MapBasis(_) => "MapBasis",
        GameMsg::GcBase(_) => "GcBase",
        GameMsg::GcJournal(_) => "GcJournal",
        GameMsg::GcTurn(_) => "GcTurn",
        GameMsg::AiTurn(_) => "AiTurn",
    }
}

/// Position of a message kind in the order of a batch (net-protocol 4.4).
fn rank(m: &GameMsg) -> u8 {
    match m {
        GameMsg::MapBasis(_) => 0,
        GameMsg::Keyframe(_) | GameMsg::Diff(_) => 1,
        GameMsg::Results(_) => 2,
        GameMsg::Journal(_) => 3,
        GameMsg::Context(_) => 4,
        GameMsg::TurnState(_) => 5,
        GameMsg::Seats(_) => 6,
        GameMsg::Chat(_) => 7,
        // The replica stream (v4) comes after the view's messages; the bot keeps no replica.
        GameMsg::GcBase(_) | GameMsg::GcJournal(_) | GameMsg::GcTurn(_) => 8,
        GameMsg::AiTurn(_) => 8,
    }
}

impl Session {
    pub fn new(nick: &str, t0: Instant) -> Session {
        Session {
            nick: nick.to_owned(),
            token: None,
            logged_in: false,
            rejected: None,
            choices: None,
            lobby: None,
            attach: None,
            detached: None,
            modes: Vec::new(),
            last_seq: 0,
            view_seq: 0,
            view: None,
            basis: None,
            basis_history: Vec::new(),
            turn: None,
            seats: Vec::new(),
            contexts: Contexts::default(),
            journal: Vec::new(),
            chat: Vec::new(),
            query_answers: BTreeMap::new(),
            gamectl_rejected: Vec::new(),
            last_reject: None,
            awaiting_keyframe: false,
            next_id: 1,
            pending_reconnect: None,
            stats: Stats::default(),
            ledger: Ledger::default(),
            knobs: Knobs::default(),
            corrupted: false,
            audit: Audit::default(),
            dropped_basis: false,
            pings: BTreeMap::new(),
            ping_seq: 0,
            server_clock: None,
            t0,
            fresh_basis_open: false,
        }
    }

    fn t_ms(&self, at: Instant) -> u64 {
        at.saturating_duration_since(self.t0).as_millis() as u64
    }

    fn event(&self, at: Instant, kind: &str, body: Value) -> Effect {
        let mut o = match body {
            Value::Object(m) => m,
            other => {
                let mut m = serde_json::Map::new();
                m.insert("data".into(), other);
                m
            }
        };
        o.insert("ev".into(), json!(kind));
        o.insert("t_ms".into(), json!(self.t_ms(at)));
        Effect::Event(Value::Object(o))
    }

    /// A new socket is going to log in. With `resume`, the stream and the view stay; without it (a reloaded
    /// page) nothing is in memory.
    pub fn begin_login(&mut self, resume: bool) {
        self.logged_in = false;
        self.rejected = None;
        self.attach = None;
        self.detached = None;
        if !resume {
            self.forget_stream();
            self.basis = None;
            // A reloaded page remembers no cities it has seen.
            self.audit.reset();
        }
    }

    fn forget_stream(&mut self) {
        self.view = None;
        self.turn = None;
        self.seats.clear();
        self.last_seq = 0;
        self.view_seq = 0;
        self.contexts = Contexts::default();
        self.awaiting_keyframe = false;
    }

    /// The basis hash named in `resume.basis` and `JoinGame.basis` (the `claim_basis` knob).
    pub fn basis_claim(&self) -> Option<BasisClaim> {
        self.basis_claim_for(self.attach.as_ref().map_or(0, |a| a.game))
    }

    /// The claim for entering the game `game` (a `JoinGame` comes when the bot is attached to nothing).
    pub fn basis_claim_for(&self, game: protocol::GameId) -> Option<BasisClaim> {
        let own = self.basis.as_ref().map(BasisState::hash);
        match self.knobs.claim_basis {
            ClaimBasis::None => None,
            ClaimBasis::Own => own.map(|hash| BasisClaim { game, hash }),
            ClaimBasis::Stale => {
                let first = self.basis_history.first().copied().or(own)?;
                // After a change of the basis the first hash is stale; without one, spoil it.
                let hash = if Some(first) == own { first ^ 1 } else { first };
                Some(BasisClaim { game, hash })
            }
            ClaimBasis::WrongGame => own.map(|hash| BasisClaim {
                game: game.wrapping_add(1000),
                hash,
            }),
        }
    }

    /// The resume point for the next login, if there is a stream to continue.
    pub fn resume_point(&self) -> Option<Resume> {
        let a = self.attach.as_ref()?;
        Some(Resume {
            stream: a.stream,
            last_seq: self.last_seq,
            basis: self.basis_claim(),
        })
    }

    /// The next application `Ping`; its answer measures the RTT.
    pub fn ping(&mut self, now: Instant) -> ClientMsg {
        self.ping_seq += 1;
        self.pings.insert(self.ping_seq, now);
        // Old unanswered pings (a dead link) do not pile up.
        while self.pings.len() > 16 {
            self.pings.pop_first();
        }
        ClientMsg::Ping {
            seq: self.ping_seq,
            client_ms: self.t_ms(now),
        }
    }

    pub fn alloc_id(&mut self) -> CmdId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn turn_number(&self) -> i32 {
        self.turn.as_ref().map_or(0, |t| t.turn)
    }

    /// The seat of this bot in `turn` may give orders now (`TurnState` is the human phase and the seat may act).
    pub fn may_act(&self) -> bool {
        let (Some(t), Some(a)) = (&self.turn, &self.attach) else {
            return false;
        };
        t.phase == TurnPhase::Humans
            && t.seats
                .iter()
                .any(|s| s.player == a.player && s.may_act && !s.finished)
    }

    // ------------------------------------------------------------ messages

    pub fn handle(&mut self, f: &Frame) -> Vec<Effect> {
        let mut fx = Vec::new();
        let turn = self.turn_number();
        let entry = self.stats.per_turn.entry(turn).or_default();
        entry.0 += f.wire_bytes as u64;
        self.stats.down_body += f.wire_bytes as u64;
        match &f.msg {
            ServerMsg::Game(b) => self.on_batch(b, f, &mut fx),
            ServerMsg::Welcome { .. } => self.count_other("Welcome", &f.msg),
            ServerMsg::Pong { seq, server_ms, .. } => {
                self.count_other("Pong", &f.msg);
                if let Some(sent) = self.pings.remove(seq) {
                    let rtt = f.at.saturating_duration_since(sent);
                    self.stats.rtt_ms.push(rtt.as_secs_f64() * 1000.0);
                }
                self.server_clock = Some((*server_ms, f.at));
            }
            ServerMsg::Reject(r) => {
                self.count_other("Reject", &f.msg);
                self.rejected = Some(r.clone());
                self.stats.error(format!("rejected: {r}"));
                fx.push(self.event(
                    f.at,
                    "error",
                    json!({ "what": "Reject", "reason": r.kind() }),
                ));
            }
            ServerMsg::LoggedIn(s) => {
                self.count_other("LoggedIn", &f.msg);
                // A bot logs in with a token or a password: the answer carries the token (v9: `None` only for a cookie login).
                self.token = s.token.or(self.token);
                self.logged_in = true;
                fx.push(self.event(
                    f.at,
                    "lobby",
                    json!({ "what": "logged_in", "nick": s.nick }),
                ));
            }
            ServerMsg::Choices(c) => {
                self.count_other("Choices", &f.msg);
                self.choices = Some(c.clone());
            }
            ServerMsg::LobbyState(s) => {
                self.count_other("LobbyState", &f.msg);
                self.lobby = Some(s.clone());
                fx.push(self.event(
                    f.at,
                    "lobby",
                    json!({ "what": "state", "rooms": s.rooms.len(), "games": s.games.len(), "in_room": s.room.is_some(),
                        "slots": s.room.as_ref().map(|r| r.slots.iter().map(|x| format!("{:?}", x.occupant)).collect::<Vec<_>>()) }),
                ));
            }
            ServerMsg::LobbyError { text } => {
                self.count_other("LobbyError", &f.msg);
                self.stats.lobby_errors.push(text.clone());
                fx.push(self.event(f.at, "lobby", json!({ "what": "error", "text": text })));
            }
            ServerMsg::Attached(a) => self.on_attached(a, f, &mut fx),
            ServerMsg::Detached { reason } => {
                self.count_other("Detached", &f.msg);
                self.attach = None;
                self.detached = Some(reason.clone());
                fx.push(self.event(
                    f.at,
                    "lobby",
                    json!({ "what": "detached", "reason": format!("{reason:?}") }),
                ));
            }
            ServerMsg::QueryResult { id, result } => {
                self.count_other("QueryResult", &f.msg);
                self.query_answers.insert(*id, result.clone());
            }
            ServerMsg::GameCtlRejected { ctl, reason } => {
                self.count_other("GameCtlRejected", &f.msg);
                self.gamectl_rejected.push((*ctl, *reason));
                fx.push(self.event(
                    f.at,
                    "lobby",
                    json!({ "what": "gamectl_rejected", "ctl": format!("{ctl:?}"), "reason": format!("{reason:?}") }),
                ));
            }
            ServerMsg::Fatal { message } => {
                self.count_other("Fatal", &f.msg);
                self.stats.error(format!("Fatal: {message}"));
            }
            // The account store (the host's last room after a start): bots keep no settings.
            ServerMsg::Account { .. } => self.count_other("Account", &f.msg),
            other => {
                let kind = other.kind();
                self.count_other(kind, other);
                self.stats
                    .error(format!("unexpected server message {kind}"));
            }
        }
        fx
    }

    /// A message that is not a game batch: counted by kind.
    fn count_other(&mut self, kind: &str, m: &ServerMsg) {
        let raw = Postcard.encode(m).map_or(0, |b| b.len() as u64);
        let e = self.stats.down_by_kind.entry(kind.to_owned()).or_default();
        e.msgs += 1;
        e.raw_bytes += raw;
        self.stats.down_msgs += 1;
    }

    fn on_attached(&mut self, a: &Attached, f: &Frame, fx: &mut Vec<Effect>) {
        self.count_other("Attached", &f.msg);
        self.modes.push(a.mode);
        self.next_id = self.next_id.max(a.last_cmd_id + 1);
        self.attach = Some(Attach {
            game: a.game,
            player: a.player,
            stream: a.stream,
            mode: a.mode,
            catalog: a.catalog,
            last_cmd_id: a.last_cmd_id,
        });
        self.detached = None;
        if a.mode == AttachMode::Fresh {
            // The stream starts again: a keyframe is the first view message.
            self.forget_stream();
            self.fresh_basis_open = true;
        }
        let mut finished = None;
        if let Some(p) = self.pending_reconnect.as_mut() {
            p.mode = Some(a.mode);
            // A replay has nothing to wait for: the view is current as soon as the batches after
            // `last_seq` are in, which follow at once.
            if a.mode == AttachMode::Replay {
                finished = self.pending_reconnect.take();
            }
        }
        if let Some(p) = finished {
            self.finish_reconnect(p, f.at);
        }
        fx.push(self.event(
            f.at,
            "attached",
            json!({ "game": a.game, "player": a.player, "stream": a.stream, "mode": format!("{:?}", a.mode), "last_cmd_id": a.last_cmd_id }),
        ));
    }

    fn finish_reconnect(&mut self, p: PendingReconnect, at: Instant) {
        let ms = at.saturating_duration_since(p.since).as_secs_f64() * 1000.0;
        self.stats.reconnects.push(json!({
            "reason": p.reason,
            "planned": p.planned,
            "mode": p.mode.map(|m| format!("{m:?}")),
            "ms_to_view": (ms * 10.0).round() / 10.0,
        }));
    }

    /// A keyframe or a diff was applied: a pending reconnect ends.
    fn view_current(&mut self, at: Instant) {
        if let Some(p) = self.pending_reconnect.take() {
            self.finish_reconnect(p, at);
        }
    }

    fn need_keyframe(
        &mut self,
        reason: KeyframeReason,
        at: Instant,
        why: &str,
        fx: &mut Vec<Effect>,
    ) {
        *self
            .stats
            .need_keyframe
            .entry(format!("{reason:?}"))
            .or_insert(0) += 1;
        fx.push(self.event(
            at,
            "error",
            json!({ "what": "need_keyframe", "reason": format!("{reason:?}"), "why": why }),
        ));
        if !self.awaiting_keyframe {
            self.awaiting_keyframe = true;
            fx.push(Effect::Send(Box::new(ClientMsg::NeedKeyframe { reason })));
        }
    }

    fn on_batch(&mut self, b: &GameBatch, f: &Frame, fx: &mut Vec<Effect>) {
        if self.attach.is_none() {
            self.stats
                .error(format!("a batch (seq {}) with no seat attached", b.seq));
            return;
        }
        if b.seq != self.last_seq + 1 {
            self.stats.error(format!(
                "seq {} after {}: a batch missing or repeated",
                b.seq, self.last_seq
            ));
            fx.push(Effect::Reconnect("seq gap".into()));
            return;
        }
        self.last_seq = b.seq;
        self.check_order(b);
        let fog_before = self.audit.violations;

        let has_basis = b.msgs.iter().any(|m| matches!(m, GameMsg::MapBasis(_)));
        let has_key = b.msgs.iter().any(|m| matches!(m, GameMsg::Keyframe(_)));
        if has_basis && has_key {
            self.stats.cold_n += 1;
            self.stats.cold_max_bytes = self.stats.cold_max_bytes.max(f.wire_bytes as u64);
        }
        if has_key && !has_basis && self.fresh_basis_open && self.basis.is_some() {
            self.stats.basis_skipped += 1;
        }
        if has_key {
            self.fresh_basis_open = false;
        }
        let mut kinds = Vec::new();
        for m in &b.msgs {
            let kind = kind_of(m);
            kinds.push(kind);
            let raw = Postcard.encode(m).map_or(0, |v| v.len() as u64);
            let e = self.stats.down_by_kind.entry(kind.to_owned()).or_default();
            e.msgs += 1;
            e.raw_bytes += raw;
            self.stats.down_msgs += 1;
            self.on_msg(b.seq, m, raw, f, fx);
        }
        if self.audit.violations > fog_before {
            let last = self.audit.examples.last().cloned();
            fx.push(self.event(
                f.at,
                "error",
                json!({ "what": "fog_violation", "total": self.audit.violations, "last": last }),
            ));
        }
        self.after_batch(f.at, fx);
        fx.push(self.event(
            f.at,
            "batch",
            json!({ "seq": b.seq, "bytes": f.wire_bytes, "kinds": kinds, "turn": self.turn_number() }),
        ));
    }

    /// Order inside a batch: `MapBasis`, one view message, `Results`, `Journal`, `Context`, `TurnState`,
    /// `Seats`, `Chat`; `MapBasis` only with a `Keyframe`.
    fn check_order(&mut self, b: &GameBatch) {
        let mut last = 0;
        let mut views = 0;
        for m in &b.msgs {
            let r = rank(m);
            if r < last {
                self.stats.error(format!(
                    "seq {}: {} is out of order in the batch",
                    b.seq,
                    kind_of(m)
                ));
            }
            last = last.max(r);
            if r == 1 {
                views += 1;
            }
        }
        if views > 1 {
            self.stats
                .error(format!("seq {}: {views} view messages in one batch", b.seq));
        }
        let basis = b.msgs.iter().any(|m| matches!(m, GameMsg::MapBasis(_)));
        let key = b.msgs.iter().any(|m| matches!(m, GameMsg::Keyframe(_)));
        if basis && !key {
            self.stats
                .error(format!("seq {}: MapBasis without a Keyframe", b.seq));
        }
    }

    fn on_msg(&mut self, seq: u64, m: &GameMsg, raw: u64, f: &Frame, fx: &mut Vec<Effect>) {
        match m {
            GameMsg::MapBasis(mb) => {
                self.stats.basis_n += 1;
                self.stats.basis_max_bytes = self.stats.basis_max_bytes.max(raw);
                if mb.plots.len() != usize::from(mb.width) * usize::from(mb.height) {
                    self.stats.error(format!(
                        "MapBasis has {} plots for a {}x{} map",
                        mb.plots.len(),
                        mb.width,
                        mb.height
                    ));
                }
                match BasisState::new(mb.clone()) {
                    Ok(state) => {
                        self.basis_history.push(state.hash());
                        self.basis = Some(state);
                    }
                    Err(e) => {
                        self.basis = None;
                        self.stats
                            .error(format!("MapBasis does not check out: {e:?}"));
                        self.need_keyframe(KeyframeReason::BasisMismatch, f.at, "bad MapBasis", fx);
                    }
                }
            }
            GameMsg::Keyframe(kf) => {
                self.stats.keyframes_n += 1;
                self.stats.keyframe_max_bytes = self.stats.keyframe_max_bytes.max(raw);
                let Some(basis) = self.basis.as_ref() else {
                    self.need_keyframe(
                        KeyframeReason::BasisMismatch,
                        f.at,
                        "keyframe without a basis",
                        fx,
                    );
                    return;
                };
                if basis.hash() != kf.basis_hash {
                    self.need_keyframe(
                        KeyframeReason::BasisMismatch,
                        f.at,
                        "keyframe names another basis",
                        fx,
                    );
                    return;
                }
                if let Err(e) = check(&kf.view) {
                    self.stats
                        .error(format!("keyframe does not check out: {e:?}"));
                }
                if let Err(e) = check_pair(&kf.view, basis.map()) {
                    self.stats
                        .error(format!("keyframe does not fit the basis: {e:?}"));
                }
                let hash = ViewHash::of(&kf.view);
                self.stats.hash_checked += 1;
                if hash.value() != kf.hash {
                    self.stats.hash_mismatch += 1;
                    self.need_keyframe(KeyframeReason::HashMismatch, f.at, "keyframe hash", fx);
                    return;
                }
                if self.stats.first_view.is_none() {
                    self.stats.first_view = Some((kf.view.turn, hash.value()));
                }
                self.view = Some((kf.view.clone(), hash));
                self.view_seq = seq;
                self.awaiting_keyframe = false;
                if let Some(basis) = self.basis.as_ref() {
                    self.audit.check_keyframe(&kf.view, basis, kf.view.turn);
                }
                self.view_current(f.at);
            }
            GameMsg::Diff(d) => {
                if self.awaiting_keyframe {
                    self.stats.ignored_diffs += 1;
                    return;
                }
                if self.view.is_none() || d.base != self.view_seq {
                    let why = format!("diff base {} against view_seq {}", d.base, self.view_seq);
                    self.need_keyframe(KeyframeReason::BaseMismatch, f.at, &why, fx);
                    return;
                }
                let (Some((view, hash)), Some(basis)) = (self.view.as_mut(), self.basis.as_mut())
                else {
                    self.need_keyframe(
                        KeyframeReason::BasisMismatch,
                        f.at,
                        "diff without a basis",
                        fx,
                    );
                    return;
                };
                let undo = UndoLog::capture(view, hash, basis, &d.diff);
                self.stats.hash_checked += 1;
                if let Err(e) = apply(view, hash, basis, &d.diff) {
                    undo.restore(view, hash, basis);
                    self.need_keyframe(KeyframeReason::ApplyError, f.at, &format!("{e:?}"), fx);
                    return;
                }
                let basis_wrong = d.basis_hash.is_some_and(|h| h != basis.hash());
                if hash.value() != d.hash || basis_wrong {
                    undo.restore(view, hash, basis);
                    self.stats.hash_mismatch += 1;
                    self.need_keyframe(KeyframeReason::HashMismatch, f.at, "diff hash", fx);
                    return;
                }
                if d.basis_hash.is_some() {
                    self.basis_history.push(basis.hash());
                }
                self.audit.check_diff(view, basis, &d.diff, view.turn);
                self.view_seq = seq;
                self.view_current(f.at);
            }
            GameMsg::Results(rs) => {
                for r in rs {
                    match self.ledger.answer(r.id, &r.outcome, f.at) {
                        Answer::Final(lat, outcome) => {
                            self.stats.latency_ms.push(lat.as_secs_f64() * 1000.0);
                            let mut ev = json!({ "id": r.id, "outcome": outcome_text(&r.outcome), "ms": (lat.as_secs_f64() * 10000.0).round() / 10.0 });
                            // The text of the refusal (the core names the blocker of a refused end of turn).
                            if let CommandOutcome::Rejected { reason, .. } = &r.outcome {
                                ev["reason"] = json!(reason);
                                self.last_reject = Some((r.id, reason.clone()));
                            }
                            fx.push(self.event(f.at, "result", ev));
                            if outcome == Outcome::Lost {
                                self.stats.error(format!("order {} was lost", r.id));
                            }
                        }
                        Answer::Queued => {
                            fx.push(self.event(
                                f.at,
                                "result",
                                json!({ "id": r.id, "outcome": "Queued" }),
                            ));
                        }
                        Answer::Repeat => {}
                        Answer::Double => self.stats.error(format!(
                            "order {} got a second, different final outcome",
                            r.id
                        )),
                        Answer::Unknown => self
                            .stats
                            .error(format!("an answer to order {} which was never sent", r.id)),
                    }
                }
            }
            GameMsg::Journal(entries) => {
                if let Some((view, _)) = self.view.as_ref() {
                    self.audit.check_journal(view, entries, view.turn);
                }
                self.journal.extend(entries.iter().cloned());
            }
            GameMsg::TurnState(t) => {
                let changed = self
                    .turn
                    .as_ref()
                    .is_none_or(|o| o.turn != t.turn || o.phase != t.phase);
                if let Some(old) = &self.turn
                    && t.turn > old.turn
                {
                    self.stats.turns_played += 1;
                }
                self.turn = Some(t.clone());
                if changed {
                    fx.push(self.event(
                        f.at,
                        "turn",
                        json!({ "turn": t.turn, "phase": format!("{:?}", t.phase), "may_act": self.may_act() }),
                    ));
                }
            }
            GameMsg::Seats(v) => {
                self.on_seats(v, f.at);
                self.seats = v.clone();
            }
            GameMsg::Context(c) => {
                self.contexts.apply(c, self.view_seq);
                if let Some((view, _)) = self.view.as_ref() {
                    self.audit.check_context(view, c, view.turn);
                }
            }
            GameMsg::Chat(lines) => self.chat.extend(lines.iter().cloned()),
            // The replica stream is not the bots' business.
            GameMsg::GcBase(_) | GameMsg::GcJournal(_) | GameMsg::GcTurn(_) => {}
            // Nor is the AI progress.
            GameMsg::AiTurn(_) => {}
        }
    }

    fn on_seats(&mut self, new: &[SeatView], at: Instant) {
        let old_host = self.seats.iter().find(|s| s.host);
        let new_host = new.iter().find(|s| s.host);
        if let (Some(o), Some(n)) = (old_host, new_host)
            && o.player != n.player
        {
            let lost_for = match o.conn {
                protocol::Conn::Lost { since_ms } => self
                    .server_clock
                    .map(|(ms, when)| ms + at.saturating_duration_since(when).as_millis() as u64)
                    .map(|now| now.saturating_sub(since_ms)),
                _ => None,
            };
            let rec = json!({ "from": o.player, "to": n.player, "at_ms": self.t_ms(at), "lost_for_ms": lost_for });
            self.stats.host_changes.push(rec);
        }
    }

    /// What the bot does to itself after a whole batch (the violations of a raw peer).
    fn after_batch(&mut self, at: Instant, fx: &mut Vec<Effect>) {
        let turn = self.turn_number();
        if let Some(n) = self.knobs.corrupt_view_at_turn
            && turn >= n
            && !self.corrupted
            && let Some((view, hash)) = self.view.as_mut()
            && let Some(plot) = view.map.plots.first_mut()
        {
            // The copy is wrong and its hash is recomputed: the next diff disagrees with the server. A plot
            // record is the lasting kind of damage: a diff that does not name plot 0 does not repair it (the
            // top bar and the header are replaced whole whenever they change).
            plot.yields[5] = plot.yields[5].wrapping_add(1);
            *hash = ViewHash::of(view);
            self.corrupted = true;
            fx.push(self.event(at, "error", json!({ "what": "corrupt_view", "turn": turn })));
        }
        if let Some(n) = self.knobs.drop_basis_at_turn
            && turn >= n
            && !self.dropped_basis
            && self.basis.is_some()
        {
            self.basis = None;
            self.dropped_basis = true;
            // The answer is `MapBasis` and `Keyframe` in one batch (the diffs are not applied until then).
            self.awaiting_keyframe = true;
            fx.push(Effect::Send(Box::new(ClientMsg::NeedKeyframe {
                reason: KeyframeReason::BasisMismatch,
            })));
            *self
                .stats
                .need_keyframe
                .entry("BasisMismatch".into())
                .or_insert(0) += 1;
            fx.push(self.event(
                at,
                "error",
                json!({ "what": "need_keyframe", "reason": "BasisMismatch", "why": "drop_basis" }),
            ));
        }
    }

    /// The summary of `ctl` `state`.
    pub fn summary(&self) -> Value {
        let (hash, units, cities) = self.view.as_ref().map_or((0, 0, 0), |(v, h)| {
            (h.value(), v.units.len(), v.cities.len())
        });
        json!({
            "nick": self.nick,
            "logged_in": self.logged_in,
            "attached": self.attach.as_ref().map(|a| json!({ "game": a.game, "player": a.player, "stream": a.stream, "mode": format!("{:?}", a.mode) })),
            "turn": self.turn_number(),
            "phase": self.turn.as_ref().map(|t| format!("{:?}", t.phase)),
            "may_act": self.may_act(),
            "turn_state": self.turn,
            "seats": self.seats,
            "view": { "hash": hash, "units": units, "cities": cities, "seq": self.view_seq },
            "mine": self.view.as_ref().map(|(v, _)| v.units.iter().filter(|u| u.owner == v.human).map(|u| u.id).collect::<Vec<_>>()),
            "seq": self.last_seq,
            "basis_hash": self.basis.as_ref().map(BasisState::hash),
            "open_orders": self.ledger.open(),
            "awaiting_keyframe": self.awaiting_keyframe,
            "in_room": self.lobby.as_ref().is_some_and(|l| l.room.is_some()),
            "room": self.lobby.as_ref().and_then(|l| l.room.as_ref()).map(|r| r.id),
            // The slots of the room: who sits there (`Human:<nick>`, `Ai`, `Open`, `Closed`) and for whom it is kept.
            "room_slots": self.lobby.as_ref().and_then(|l| l.room.as_ref()).map(|r| r.slots.iter().map(|s| json!({
                "index": s.index,
                "occupant": match &s.occupant {
                    protocol::Occupant::Human { nick, .. } => format!("Human:{nick}"),
                    other => format!("{other:?}"),
                },
                "reserved_for": s.reserved_for,
            })).collect::<Vec<_>>()),
            // The map sizes of the server's pick lists (`Describe`), in the server's order.
            "world_sizes": self.choices.as_ref().map(|c| c.world_sizes.iter().map(|w| json!({
                "id": w.id, "default_players": w.default_players, "max_players": w.max_players,
            })).collect::<Vec<_>>()),
            // The chat lines received, the peace offers of the view, the newest journal lines (as sent, with
            // kind and turn) and the games of the lobby list with their host.
            "chat": self.chat,
            "peace_offers": self.view.as_ref().map(|(v, _)| &v.peace_offers),
            "journal": &self.journal[self.journal.len().saturating_sub(JOURNAL_IN_SUMMARY)..],
            "games": self.lobby.as_ref().map(|l| l.games.iter().map(|g| json!({
                "id": g.id, "name": g.name, "host": g.host, "turn": g.turn, "status": format!("{:?}", g.status),
            })).collect::<Vec<_>>()),
        })
    }
}

/// How many of the newest journal lines `summary` reports.
const JOURNAL_IN_SUMMARY: usize = 200;

pub fn outcome_text(o: &CommandOutcome) -> String {
    match o {
        CommandOutcome::Rejected { code, .. } => format!("Rejected({code:?})"),
        other => format!("{other:?}"),
    }
}

/// `ClientMsg::Ping` etc. are measured on the way out by the bot.
pub fn note_up(stats: &mut Stats, turn: i32, bytes: usize) {
    stats.up_body += bytes as u64;
    stats.up_msgs += 1;
    stats.per_turn.entry(turn).or_default().1 += bytes as u64;
}
