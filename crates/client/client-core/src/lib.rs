//! Sans-IO client logic (architecture §5.2, decision D10; net-client NC-D1).
//!
//! `ClientCore`: session, connection health, lobby model. No sockets, timers or threads: the shell
//! (`client-app::NetClient`) opens and closes the sockets `ClientCore` asks for, and feeds it time, bytes and
//! page events. Every socket has a number (NC-D15): bytes, closes and frames of a socket that is not current
//! and not a live attempt are ignored.

mod change;
mod config;
mod conn;
mod contexts;
mod lobby_model;
mod mods;
mod orders;
mod overlay;
mod session;
mod stats;
mod stream;
mod terrain_share;

use std::collections::{HashSet, VecDeque};

use protocol::{
    AccountData, AccountSave, Attached, Catalog, CatalogRef, ChatSend, ChatTo, ClientMsg, Cmd,
    CmdId, Codec, Command, CommandResult, DetachReason, GameBatch, GameCtl, GameCtlReason, GameId,
    GameMsg, JournalEntry, KeyframeReason, LobbyChoices, LobbyCmd, LobbyState, Postcard, Query,
    RejectReason, SeatView, ServerMsg, Token, TurnState, WorldView,
};
use wire::{Encoder, Limits};

pub use change::{DirtyKeys, DirtyPlots, KeyChanges, ViewChange};
pub use config::{AddrContext, AddrError, NetConfig, ServerAddr, http_base, resolve_server};
pub use conn::{ConnState, DisconnectWhy, HostEvent, ReconnectStatus, Sock, SocketCmd};
pub use contexts::{
    ContextView, Contexts, Ctx, FOCUS_NEXT, FocusPlan, OrderSink, UnitContextExt, unit_of,
};
pub use lobby_model::{HostChange, HostWatch, LobbyModel};
pub use mods::{ModNet, NetHost, OutgoingMod};
pub use protocol::{GcBase, GcBlobRef, GcJournal, GcPatchRef, GcResyncReason, GcTurn};
pub use session::{AttachInfo, NoticeKind};
pub use stats::{ClientStats, NeedKeyframeCounts};
pub use stream::{BreakWhy, ChatEntry};
pub use terrain_share::SharedTerrain;
/// The format of the catalog body, for the shell that fetches it.
pub use wire::catalog as catalog_format;

use conn::{Link, LinkAct, Who};
use orders::Orders;
use overlay::{Overlay, Touched, predict};
use session::{Phase, SessionState};
use stream::{Cx, Effect, Stream};

/// Why an order (or a lobby command, chat line, `GameCtl`) was not taken (NC-D5): without a connection it
/// is refused on the spot, nothing is queued. No game, `Replaced`, `Closed` and `Rejected` are `NoGame`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderRefused {
    NoConnection,
    NoGame,
    TooMany,
    /// The picture the order was given on is of a turn the server has left: the order does nothing (B1).
    TurnChanged,
    /// A mod message over the payload limit of the protocol (T35).
    TooBig,
}

/// What the screens show (`lobby-ui`). Computed from the session and the events; it decides nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientPhase {
    /// Closed, connecting or handshaking, including a reconnect while there is no game yet. A drop in the
    /// middle of a game keeps `Playing`; the line is `conn()` (NC-D5).
    Connecting,
    /// A refusal that needs a new nick or password.
    NeedLogin,
    Lobby,
    Room,
    /// `Start` or `JoinGame` sent, `Attached` not yet. (`LoadSave` only makes a room: the player sees it and
    /// starts it.)
    Starting,
    /// `Attached`, but no keyframe or no catalog yet.
    Loading,
    Playing,
    /// `Detached` for a reason the player must read.
    Notice(NoticeKind),
    Fatal(String),
}

/// A message of the replica stream (protocol v4), in the order the gateway sent them.
/// The core passes them on untouched: the replica (`crates/client/replica`) owns the chain, the numbers and
/// the bodies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplicaMsg {
    /// The base snapshot of a chain (join, `GcResync` answer).
    Base(GcBase),
    /// Journal batches of the human phase.
    Journal(GcJournal),
    /// A turn change: the new snapshot and perhaps a patch.
    Turn(GcTurn),
}

/// What the client tells its shell and screens.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientEvent {
    ConnChanged(ConnState),
    /// The token to store under `civ5.token.<nick>`. `Debug` prints `***`.
    TokenIssued(Token),
    LoggedIn,
    Rejected(RejectReason),
    LobbyChanged,
    LobbyError(String),
    Attached {
        game: GameId,
        mode: protocol::AttachMode,
    },
    /// The catalog of the game is not loaded: fetch `GET /catalog/{hash:016x}`.
    CatalogNeeded(CatalogRef),
    ViewChanged,
    /// The stream cannot go on as it is: the client reconnects (see the ladder, 4.3).
    StreamBroken(BreakWhy),
    OrderAnswered {
        id: CmdId,
        outcome: protocol::CommandOutcome,
    },
    /// The order did not (or maybe did not) go through, or got no answer for `order_stuck_ms`: its picture
    /// is off the screen and the line is in the journal.
    OrderRolledBack {
        id: CmdId,
        text: String,
    },
    Detached(DetachReason),
    GameCtlRejected {
        ctl: GameCtl,
        reason: GameCtlReason,
    },
    QueryResult {
        id: u32,
        answer: protocol::QueryAnswer,
    },
    Fatal(String),
    /// v12: the account's store came (after login, or the server changed the last room). Read it with
    /// `ClientCore::account`.
    AccountLoaded,
    /// The host of the game's seats or of the room changed between two snapshots (NP-D25, 4.2 item 9).
    HostChanged {
        from: Option<String>,
        to: String,
        me: bool,
    },
    /// Our own end of turn was taken back (NP-D23): `by_order` when an order to a unit or a city did it, not
    /// an explicit `CancelEndTurn`. Given once, when the end of turn stops showing as set; the screens write
    /// a note and bring back the "Next turn" button or the blocker.
    EndTurnCancelled {
        by_order: bool,
    },
    /// A message of the replica stream, in order (`Replica::on_msg`).
    Replica(ReplicaMsg),
    /// v13: the frozen mod set of the game, the first message of every fresh stream (`ModNet::on_event` attaches
    /// the client halves).
    ModSet(protocol::ModSetView),
    /// v13: a message of a mod's server half, in stream order after the set.
    ModMsg(protocol::ModS2C),
}

pub struct ClientCore {
    cfg: NetConfig,
    link: Link,
    sess: SessionState,
    lobby: LobbyModel,
    /// v12: the account's store the server sent; cleared at a new login.
    account: Option<AccountData>,
    host_watch: HostWatch,
    stream: Stream,
    orders: Orders,
    /// The optimistic layer: `shown` is `base` plus its records (NC6).
    overlay: Overlay,
    catalog: Option<Catalog>,
    /// The `hash` of the catalog `catalog` was loaded for.
    catalog_hash: Option<u64>,
    /// `Detached(Replaced)` came on the current socket while a probe was logging in: probably our own
    /// probe took the session, so the verdict waits for it (NC-D15).
    held_replaced: bool,
    out: VecDeque<(Sock, Vec<u8>)>,
    /// The focus the server has (sent after the last `Attached`); `None` means its default.
    focus_sent: Option<protocol::Focus>,
    /// The unit the player had selected at the last `set_focus`, for the select counters.
    focus_selected: Option<i32>,
    cmds: VecDeque<SocketCmd>,
    events: VecDeque<ClientEvent>,
    stats: ClientStats,
    logged_unexpected: HashSet<&'static str>,
    now: u64,
}

impl ClientCore {
    pub fn new(cfg: NetConfig) -> Self {
        let sess = SessionState::new(&cfg.nick);
        ClientCore {
            link: Link::new(cfg.clone()),
            cfg,
            sess,
            lobby: LobbyModel::default(),
            account: None,
            host_watch: HostWatch::default(),
            stream: Stream::new(),
            orders: Orders::new(),
            overlay: Overlay::default(),
            catalog: None,
            catalog_hash: None,
            held_replaced: false,
            out: VecDeque::new(),
            focus_sent: None,
            focus_selected: None,
            cmds: VecDeque::new(),
            events: VecDeque::new(),
            stats: ClientStats::default(),
            logged_unexpected: HashSet::new(),
            now: 0,
        }
    }

    // ------------------------------------------------------------ input and output

    /// The shell has a server address: open the first socket.
    pub fn connect(&mut self, now_ms: u64) {
        self.now = now_ms;
        if self.sess.phase() == Phase::Rejected {
            return;
        }
        self.link.connect(now_ms);
        self.pump();
    }

    /// The socket `Open` asked for is open: `Hello` and `Login` go out at once, back to back.
    pub fn on_connected(&mut self, now_ms: u64, sock: Sock) {
        self.now = now_ms;
        match self.link.on_open(now_ms, sock) {
            Who::Current => {
                self.sess.begin_handshake();
                self.send_hello_login(sock);
            }
            Who::Attempt => self.send_hello_login(sock),
            Who::Stale => {}
        }
        self.pump();
    }

    /// A frame from the server. Errors are counted, never panics.
    pub fn on_bytes(&mut self, now_ms: u64, sock: Sock, bytes: &[u8]) {
        self.now = now_ms;
        let who = self.link.on_bytes(now_ms, sock);
        if who != Who::Stale {
            self.stats.bytes_in += bytes.len() as u64;
            match wire::decode::<ServerMsg>(bytes, &Limits::CLIENT) {
                Ok(msg) => self.handle(msg, sock, who),
                Err(err) => {
                    self.stats.decode_errors += 1;
                    tracing::warn!(%err, "undecodable frame from the server");
                    // An unreadable frame of a running stream is a broken stream (4.3 ladder).
                    if who == Who::Current && self.sess.phase() == Phase::Attached {
                        self.stream_break(BreakWhy::Undecodable);
                    }
                }
            }
        }
        self.pump();
    }

    pub fn on_disconnected(&mut self, now_ms: u64, sock: Sock, why: DisconnectWhy) {
        self.now = now_ms;
        if self.link.on_close(now_ms, sock, why) == Who::Current {
            self.sess.line_down();
        }
        self.pump();
    }

    pub fn host_event(&mut self, now_ms: u64, ev: HostEvent) {
        self.now = now_ms;
        self.link.host_event(now_ms, ev);
        self.pump();
    }

    pub fn tick(&mut self, now_ms: u64) {
        self.now = now_ms;
        self.link.tick(now_ms);
        if self.held_replaced && !self.link.attempt_logging_in() {
            self.held_replaced = false;
            self.apply_replaced();
        }
        if self.sess.phase() == Phase::Attached
            && let Some(fx) = self
                .stream
                .tick(now_ms, self.cfg.keyframe_wait_ms, &mut self.stats)
        {
            self.apply_effects(vec![fx]);
        }
        self.guard_layer(now_ms);
        self.pump();
    }

    /// A picture with no answer for `order_stuck_ms` of live line comes off, with a line for the player
    /// (4.4). The order itself is still in flight and still gets its one answer. The time does not run while
    /// the line is down or a keyframe is awaited: then the answer cannot be told from a lost one.
    fn guard_layer(&mut self, now_ms: u64) {
        if self.overlay.is_empty() {
            return;
        }
        let live = self.line_up() && !self.stream.awaiting_keyframe();
        let stuck = self.overlay.stuck(now_ms, live, self.cfg.order_stuck_ms);
        let Some(me) = self.my_player() else {
            return;
        };
        for id in stuck {
            let touched = self.overlay.remove(id, me);
            self.layer_touched(&touched);
            self.stats.layer_stuck += 1;
            let text = "No answer from the server".to_string();
            self.journal_note(text.clone(), None);
            self.events
                .push_back(ClientEvent::OrderRolledBack { id, text });
        }
        self.refresh_shown();
    }

    /// The next frame to send, and the socket it goes to (a probe socket gets its own `Hello`/`Login`).
    pub fn poll_outgoing(&mut self) -> Option<(Sock, Vec<u8>)> {
        self.out.pop_front()
    }

    /// The next thing the shell must do with a socket.
    pub fn poll_socket_cmd(&mut self) -> Option<SocketCmd> {
        self.cmds.pop_front()
    }

    pub fn poll_event(&mut self) -> Option<ClientEvent> {
        self.events.pop_front()
    }

    // ------------------------------------------------------------ login and lobby

    /// The player typed a nick and password. Starts the connection if there is none, or starts over after
    /// a refusal.
    pub fn login(&mut self, nick: &str, password: &str) {
        self.sess.set_credentials(nick, password);
        self.restart_connection();
    }

    /// The token of an earlier session, read from the browser store before `connect`.
    pub fn set_token(&mut self, token: Option<Token>) {
        self.sess.set_token(token);
    }

    /// "Sign out" (the shell closes the sockets after this): the session, the lobby and the game view are
    /// forgotten and the login window is up; nothing reconnects until `login`.
    pub fn sign_out(&mut self) {
        self.sess.sign_out();
        self.lobby.clear_state();
        self.host_watch.reset_room();
        self.host_watch.reset_seats();
        self.stream.reset_game();
        self.drop_orders();
        self.focus_sent = None;
        self.held_replaced = false;
        self.link.set_can_probe(false);
        self.link.finish(ConnState::Closed);
    }

    /// Button after `Full` or a fatal line: try again.
    pub fn retry(&mut self) {
        self.sess.retry();
        self.restart_connection();
    }

    /// "Use this tab" after `Replaced` (NC-D15): the only way out of that state.
    pub fn use_this_tab(&mut self) {
        self.sess.take_notice();
        self.restart_connection();
    }

    /// Gives up waiting for `Attached` after a `Start` or `JoinGame` (the shell's timeout): the lobby or the
    /// room is shown again. A game that does start later still attaches. Returns whether it was waiting.
    pub fn abandon_start(&mut self) -> bool {
        let was = self.sess.starting();
        self.sess.set_starting(false);
        was
    }

    /// The player read the message of a `Detached` (kick, game over, ...).
    pub fn dismiss_notice(&mut self) {
        self.sess.take_notice();
    }

    fn restart_connection(&mut self) {
        let now = self.now;
        if self.link.is_terminal() {
            self.link.restart(now);
        } else {
            self.link.connect(now);
        }
        self.pump();
    }

    /// A lobby command. Without a connection it is refused, nothing waits (NC-D5).
    pub fn lobby(&mut self, mut cmd: LobbyCmd) -> Result<(), OrderRefused> {
        self.refusal(false)?;
        match &mut cmd {
            // The basis the client holds goes with the join (NC-D18).
            LobbyCmd::JoinGame { basis, .. } => {
                *basis = self.sess.basis_claim();
                self.sess.set_starting(true);
            }
            // `LoadSave` makes a room from the save and nothing starts until the room's `Start`: the room
            // screen must stay up (it was hidden behind "Starting the game" for good).
            LobbyCmd::Start => self.sess.set_starting(true),
            _ => {}
        }
        self.send_current(&ClientMsg::Lobby(cmd));
        Ok(())
    }

    /// v12: a change of the account's store. Dropped when it is empty, invalid or the line is not logged in (the
    /// settings stay in the local store; they go up with the next change or the next login).
    pub fn save_account(&mut self, save: AccountSave) -> bool {
        if save.is_empty() || save.validate().is_err() || !self.logged_in() {
            return false;
        }
        // The local copy follows at once, so `account()` is what the server will hold.
        if let Some(a) = &mut self.account {
            let _ = protocol::apply_save(&mut a.prefs, &save);
        }
        self.send_current(&ClientMsg::Account(save));
        true
    }

    /// v12: the account's store, once the server has sent it.
    pub fn account(&self) -> Option<&AccountData> {
        self.account.as_ref()
    }

    /// A chat line, at most 500 characters.
    pub fn chat(&mut self, to: ChatTo, text: &str) -> Result<(), OrderRefused> {
        self.refusal(true)?;
        let text: String = text.chars().take(500).collect();
        self.send_current(&ClientMsg::Chat(ChatSend { to, text }));
        Ok(())
    }

    pub fn game_ctl(&mut self, c: GameCtl) -> Result<(), OrderRefused> {
        self.refusal(true)?;
        self.send_current(&ClientMsg::GameCtl(c));
        Ok(())
    }

    /// The UI messages of the replica (`Replica::take_outgoing`, `GCN1` bytes, at most `GC_NET_MAX`) go to the
    /// server. Refused without a connection or a game like an order; the replica keeps the bytes and tries
    /// again later (AC-G2: the player's input is not lost).
    pub fn send_gc_net(&mut self, bytes: Vec<u8>) -> Result<(), OrderRefused> {
        self.refusal(true)?;
        self.stats.gc_net_sent += 1;
        self.send_current(&ClientMsg::GcNet(bytes));
        Ok(())
    }

    /// SIMPLIFIED(SIMP-exe-900): the turn guard of UI messages is on the client (GCN1 has no turn).
    ///
    /// [`send_gc_net`](Self::send_gc_net) for messages given while the replica shows `replica_turn`. `GCN1` carries no
    /// turn, so the guard is here: when the server has already left that turn the messages do nothing
    /// (`TurnChanged`; the caller drops them, they are not kept for the next turn).
    pub fn send_gc_net_at(
        &mut self,
        bytes: Vec<u8>,
        replica_turn: i32,
    ) -> Result<(), OrderRefused> {
        self.refusal(true)?;
        if replica_turn > 0 && self.base_turn() > replica_turn {
            self.stats.gc_net_stale += 1;
            return Err(OrderRefused::TurnChanged);
        }
        self.send_gc_net(bytes)
    }

    /// The replica cannot go on (`GcResyncReason`): asks for a new base. The answer is a `ClientEvent::Replica`
    /// with a `GcBase` for this seat. Refused like [`send_gc_net`](Self::send_gc_net).
    pub fn send_gc_resync(
        &mut self,
        epoch: u32,
        have_raw_hash: Option<u64>,
        at_jseq: u64,
        reason: GcResyncReason,
    ) -> Result<(), OrderRefused> {
        self.refusal(true)?;
        self.stats.gc_resync_sent += 1;
        self.send_current(&ClientMsg::GcResync {
            epoch,
            have_raw_hash,
            at_jseq,
            reason,
        });
        Ok(())
    }

    /// v13: a message of a mod's client half to its server half (`ClientMsg::Mod`). Refused like an order when
    /// there is no connection or game, and when the payload is over `MOD_PAYLOAD_MAX`; the channel limits and the
    /// rate are the server's to enforce (D7).
    pub fn send_mod(&mut self, mod_idx: u16, ch: u8, payload: Vec<u8>) -> Result<(), OrderRefused> {
        self.refusal(true)?;
        let msg = protocol::ModC2S {
            mod_idx,
            ch,
            payload,
        };
        if !msg.within_limits() {
            return Err(OrderRefused::TooBig);
        }
        self.send_current(&ClientMsg::Mod(msg));
        Ok(())
    }

    /// An order (4.5). Without a connection or a game it is refused on the spot: no number, no record,
    /// nothing waits (NC-D5). Otherwise it gets the next number, goes out at once without waiting for the
    /// answers to earlier ones, and stays in the list until its one final answer (`OrderAnswered`); after
    /// every `Attached` the unanswered ones go out again with the same numbers.
    pub fn order(&mut self, cmd: Command) -> Result<CmdId, OrderRefused> {
        self.order_at(cmd, None)
    }

    /// [`order`](Self::order) stamped with the turn of the replica the order was given on (`Some`, a turn above 0):
    /// when the server is already in a later turn the order is refused there as `TurnChanged` instead of landing in
    /// the next turn (B1). `None`: the turn of the server's view.
    pub fn order_at(
        &mut self,
        cmd: Command,
        replica_turn: Option<i32>,
    ) -> Result<CmdId, OrderRefused> {
        self.refusal(true)?;
        if self.orders.len() >= self.cfg.max_inflight {
            return Err(OrderRefused::TooMany);
        }
        if let Some(unit) = unit_of(&cmd)
            && !self.contexts().unit(unit).is_fresh()
        {
            // The picture of this order is not drawn (NC-D6); the order goes out as usual.
            self.stats.ctx_stale_at_order += 1;
        }
        let id = self.sess.take_cmd_id();
        let turn = replica_turn
            .filter(|t| *t > 0)
            .unwrap_or_else(|| self.base_turn());
        self.orders.add(id, cmd, turn, self.now);
        self.stats.orders_sent += 1;
        self.draw_order(id, &cmd);
        self.send_current(&ClientMsg::Cmd(Cmd { id, turn, cmd }));
        Ok(id)
    }

    /// The picture of an order, if the table of 4.4 has one for it and the context is fresh. The next
    /// `frame()` sees it in `shown()`.
    fn draw_order(&mut self, id: CmdId, cmd: &Command) {
        let (Some(me), Some(view)) = (self.my_player(), self.shown_ref()) else {
            return;
        };
        let before = self.my_finished();
        let Some(ops) = predict(
            cmd,
            view,
            self.turn_state(),
            &self.contexts(),
            me,
            &self.overlay,
        ) else {
            return;
        };
        let touched = self.overlay.add(id, ops, self.now, me);
        self.stats.layer_records += 1;
        self.layer_touched(&touched);
        self.refresh_shown();
        if before == Some(true) && self.my_finished() == Some(false) {
            // An explicit `CancelEndTurn` is not "by order".
            self.end_turn_cancelled(!matches!(cmd, Command::CancelEndTurn));
        }
    }

    /// The layer added or removed records: the change the screens take says so.
    fn layer_touched(&mut self, t: &Touched) {
        if t.changed {
            self.stream.layer_changed(&t.units, &t.cities);
        }
    }

    fn end_turn_cancelled(&mut self, by_order: bool) {
        if by_order {
            self.stats.end_turn_cancelled_by_order += 1;
        }
        self.events
            .push_back(ClientEvent::EndTurnCancelled { by_order });
    }

    /// A question to the game (`Query::Path` and the like). Returns the number the answer will carry in
    /// `QueryResult`, or 0 when nothing was sent (no connection or game: nothing is queued either).
    pub fn query(&mut self, q: Query) -> u32 {
        if self.refusal(true).is_err() {
            return 0;
        }
        let Some((game, stream)) = self.sess.attach().map(|a| (a.game, a.stream)) else {
            return 0;
        };
        let id = self.orders.new_query(game, stream);
        *self
            .stats
            .queries
            .entry(orders::query_kind(&q))
            .or_insert(0) += 1;
        self.send_current(&ClientMsg::Query { id, q });
        id
    }

    /// The focus of the controls (NC-D7, 4.6): sent to the server when it differs from what the server has,
    /// at most once per call, and only while a game is attached over an open line. A focus with nothing in it
    /// is sent as well (the server then computes no contexts). Contexts of units that left the focus are
    /// forgotten.
    pub fn set_focus(&mut self, plan: &FocusPlan) {
        if self.sess.phase() != Phase::Attached || !self.line_up() {
            return;
        }
        if plan.selected != self.focus_selected {
            self.focus_selected = plan.selected;
            if let Some(unit) = plan.selected {
                if self.contexts().unit(unit).is_fresh() {
                    self.stats.ctx_fresh_at_select += 1;
                } else {
                    self.stats.ctx_stale_at_select += 1;
                }
            }
        }
        let focus = plan.to_focus();
        if self.focus_sent.as_ref() == Some(&focus) {
            return;
        }
        self.stream.contexts_mut().retain_focus(&focus);
        self.stats.focus_sent += 1;
        self.send_current(&ClientMsg::Focus(focus.clone()));
        self.focus_sent = Some(focus);
    }

    /// The contexts the controls read, as of the current view (4.6).
    pub fn contexts(&self) -> ContextView<'_> {
        ContextView::new(self.stream.contexts(), self.stream.view_seq())
    }

    /// Orders sent and not answered finally.
    pub fn orders_in_flight(&self) -> usize {
        self.orders.len()
    }

    /// The catalog was fetched and checked by the shell.
    pub fn set_catalog(&mut self, catalog: Catalog) {
        self.catalog_hash = self.sess.attach().map(|a| a.catalog.hash);
        self.catalog = Some(catalog);
    }

    // ------------------------------------------------------------ reading

    pub fn phase(&self) -> ClientPhase {
        if let Some(f) = self.sess.fatal() {
            return ClientPhase::Fatal(f.to_string());
        }
        if let Some(n) = self.sess.notice() {
            return ClientPhase::Notice(n.clone());
        }
        match self.sess.phase() {
            Phase::Idle | Phase::Connecting | Phase::Handshake => ClientPhase::Connecting,
            Phase::Rejected => match self.sess.rejected() {
                Some(RejectReason::VersionMismatch { .. }) => {
                    ClientPhase::Fatal("The server has been updated, reload the page".to_string())
                }
                Some(RejectReason::Full) => ClientPhase::Fatal("The server is full".to_string()),
                Some(RejectReason::Unsupported { .. }) => {
                    ClientPhase::Fatal("The client and the server are not compatible".to_string())
                }
                _ => ClientPhase::NeedLogin,
            },
            Phase::LoggedIn => {
                if self.sess.starting() {
                    ClientPhase::Starting
                } else if self.lobby.room().is_some() {
                    ClientPhase::Room
                } else {
                    ClientPhase::Lobby
                }
            }
            Phase::Attached => {
                let catalog_ok = self.catalog.is_some()
                    && self.catalog_hash == self.sess.attach().map(|a| a.catalog.hash);
                if self.stream.view().is_some() && catalog_ok {
                    ClientPhase::Playing
                } else {
                    ClientPhase::Loading
                }
            }
        }
    }

    pub fn conn(&self) -> ConnState {
        self.link.state()
    }

    /// "Reconnecting… attempt N, next in S s" for the grey order panel; `None` while `Online` or `Poor`.
    pub fn reconnect_status(&self) -> Option<ReconnectStatus> {
        self.link.reconnect_status()
    }

    /// The order panel may take orders now: a game, an open socket and a line that is not down (NC-D5).
    pub fn orders_enabled(&self) -> bool {
        self.sess.phase() == Phase::Attached && self.line_up() && self.sess.notice().is_none()
    }

    /// The nick and password were refused, or the server turned the client away.
    pub fn rejected(&self) -> Option<&RejectReason> {
        self.sess.rejected()
    }

    pub fn token(&self) -> Option<Token> {
        self.sess.token()
    }

    pub fn nick(&self) -> &str {
        self.sess.nick()
    }

    /// The game the connection plays, and the stream position (`last_cmd_id`, `last_seq`, ...).
    pub fn attach_info(&self) -> Option<&AttachInfo> {
        self.sess.attach()
    }

    /// The number the next order gets (NC3).
    pub fn next_cmd_id(&self) -> CmdId {
        self.sess.next_cmd_id()
    }

    pub fn catalog(&self) -> Option<&Catalog> {
        self.catalog.as_ref()
    }

    pub fn lobby_state(&self) -> Option<&LobbyState> {
        self.lobby.state()
    }

    pub fn choices(&self) -> Option<&LobbyChoices> {
        self.lobby.choices()
    }

    pub fn seats(&self) -> &[SeatView] {
        self.stream.seats()
    }

    /// The view the screen and the controls take (4.4): `base` plus the optimistic layer. With no record in
    /// the layer it is the base itself, exactly as the server sent it.
    pub fn shown(&self) -> Option<&WorldView> {
        self.shown_ref()
    }

    fn shown_ref(&self) -> Option<&WorldView> {
        self.overlay.shown().or_else(|| self.stream.view())
    }

    /// The game of the current attach (none outside a game): the crash report names it.
    pub fn attached_game(&self) -> Option<GameId> {
        self.sess.attach().map(|a| a.game)
    }

    /// The view as the server sent it, without the layer.
    pub fn base(&self) -> Option<&WorldView> {
        self.stream.view()
    }

    /// Records in the optimistic layer (the test port's `overlay`).
    pub fn overlay_len(&self) -> usize {
        self.overlay.len()
    }

    /// Hash of the base view (`ViewHash::value`), the one the server's batches name.
    pub fn base_hash(&self) -> Option<u64> {
        self.stream.base_hash()
    }

    /// The map basis in page memory (NC-D18); `None` until it came.
    pub fn basis(&self) -> Option<&protocol::MapBasis> {
        self.stream.basis()
    }

    pub fn basis_hash(&self) -> Option<u64> {
        self.stream.basis_hash()
    }

    /// The plots the mods let this seat draw although it has not revealed them (mod API 1.2): the renderer's ground
    /// only, never the view (no reveal, no strategic map, no minimap, no tooltip). Empty after a fresh stream.
    pub fn terrain_share(&self) -> &SharedTerrain {
        self.stream.terrain_share()
    }

    /// `seq` of the batch with the last view message.
    pub fn view_seq(&self) -> u64 {
        self.stream.view_seq()
    }

    /// Grows after every batch that was applied and every change of the layer; a frame reads this, not the
    /// messages.
    pub fn revision(&self) -> u64 {
        self.stream.revision() + self.overlay.rev()
    }

    /// What changed since the last call (4.10).
    pub fn take_change(&mut self) -> ViewChange {
        self.stream.take_change()
    }

    /// The turn state as the screens show it: our own `finished` has the layer on it (4.4), the others' are
    /// the server's.
    pub fn turn_state(&self) -> Option<&TurnState> {
        self.overlay
            .turn_state()
            .or_else(|| self.stream.turn_state())
    }

    /// The AI civilizations the server started and ended in the AI phase of the last turn it processed, in order
    /// (SIMP-exe-820, SIMP-exe-790). The log stays until the next turn's AI phase replaces it; it names its turn.
    pub fn ai_turn(&self) -> Option<&protocol::AiTurn> {
        self.stream.ai_turn()
    }

    /// Chat lines received so far (the counter keeps growing when old lines are dropped).
    pub fn chat_total(&self) -> u64 {
        self.stream.chat_total()
    }

    pub fn chat_lines(&self) -> &[ChatEntry] {
        self.stream.chat()
    }

    pub fn journal(&self) -> &[JournalEntry] {
        self.stream.journal()
    }

    /// Time left on the timer of the group of our seat; `None`: no timer. A stopped timer (pause, a
    /// disconnected seat) shows what was left when it stopped.
    pub fn timer_left_ms(&self, now_ms: u64) -> Option<u64> {
        let me = self.sess.attach()?.player;
        let ts = self.stream.turn_state()?;
        let group = ts.seats.iter().find(|s| s.player == me)?.group;
        let t = ts.timers.iter().find(|t| t.group == group)?;
        if let Some(left) = t.left_ms_when_stopped {
            return Some(u64::from(left));
        }
        Some(t.deadline_ms.saturating_sub(self.server_ms(now_ms)))
    }

    /// Gateway clock now, from the `Pong` estimate with the smallest RTT of the last 8.
    pub fn server_ms(&self, now_ms: u64) -> u64 {
        self.link.clock().server_ms(now_ms)
    }

    /// RTT of the last `Pong`.
    pub fn rtt_ms(&self) -> Option<u64> {
        self.link.clock().rtt()
    }

    pub fn stats(&self) -> &ClientStats {
        &self.stats
    }

    /// Test port only (`fault corruptView`, 4.12): shifts a value of `base` without changing its hash, so the
    /// next diff finds the hashes apart and the client asks for a keyframe.
    pub fn debug_corrupt_view(&mut self) {
        self.stream.debug_corrupt();
        self.refresh_shown();
    }

    /// Test port only (`fault badMessage`, 4.12): a frame that is no `ClientMsg`, to the current socket.
    pub fn debug_bad_message(&mut self) {
        if let Some(sock) = self.link.current() {
            let frame = Encoder::plain(&[0xff; 12]);
            self.stats.bytes_out += frame.len() as u64;
            self.out.push_back((sock, frame));
        }
    }

    pub fn config(&self) -> &NetConfig {
        &self.cfg
    }

    // ------------------------------------------------------------ internals

    /// An open socket in `Online` or `Poor`: bytes go out now.
    fn line_up(&self) -> bool {
        matches!(self.link.state(), ConnState::Online | ConnState::Poor) && self.link.current_open()
    }

    /// NC-D5: why a message of the player cannot go out, if it cannot.
    fn refusal(&mut self, needs_game: bool) -> Result<(), OrderRefused> {
        if matches!(self.link.state(), ConnState::Replaced | ConnState::Closed)
            || self.sess.phase() == Phase::Rejected
        {
            return Err(OrderRefused::NoGame);
        }
        if !self.line_up() || !matches!(self.sess.phase(), Phase::LoggedIn | Phase::Attached) {
            self.stats.refused_offline += 1;
            return Err(OrderRefused::NoConnection);
        }
        if needs_game && self.sess.phase() != Phase::Attached {
            return Err(OrderRefused::NoGame);
        }
        Ok(())
    }

    /// The turn of the base view, which an order is stamped with; the turn state when there is no view yet.
    fn base_turn(&self) -> i32 {
        self.stream
            .view()
            .map(|v| v.turn)
            .or_else(|| self.stream.turn_state().map(|t| t.turn))
            .unwrap_or(0)
    }

    /// The seat the connection plays.
    fn my_player(&self) -> Option<protocol::PlayerId> {
        self.sess.attach().map(|a| a.player)
    }

    /// Our own `finished` as the screens show it now (the layer included); `None` without a turn state.
    pub fn my_finished(&self) -> Option<bool> {
        let me = self.my_player()?;
        self.turn_state()?
            .seats
            .iter()
            .find(|s| s.player == me)
            .map(|s| s.finished)
    }

    /// `shown` and the turn state after `base` or the records changed.
    fn refresh_shown(&mut self) {
        let me = self.my_player().unwrap_or(-1);
        self.overlay
            .rebuild(self.stream.view(), self.stream.turn_state(), me);
    }

    /// Where the order's unit or city stands in the base view, for the line of the journal.
    fn order_cell(&self, cmd: &Command) -> Option<(i16, i16)> {
        let view = self.stream.view()?;
        let me = self.my_player()?;
        if let Some(unit) = unit_of(cmd) {
            return view
                .units
                .iter()
                .find(|u| u.id == unit && u.owner == me)
                .map(|u| (u.x, u.y));
        }
        let city = match *cmd {
            Command::CitySetProduction { city, .. }
            | Command::CityPurchase { city, .. }
            | Command::CityRangeAttack { city, .. } => city,
            _ => return None,
        };
        view.cities
            .iter()
            .find(|c| c.id == city && c.owner == me)
            .map(|c| (c.x, c.y))
    }

    /// A line for the player in the journal, kind `System`, with the plot of the order if it is known.
    fn journal_note(&mut self, text: String, at: Option<(i16, i16)>) {
        let turn = self
            .stream
            .view()
            .map(|v| v.turn)
            .or_else(|| self.stream.turn_state().map(|t| t.turn))
            .unwrap_or(0);
        self.stream.push_journal(protocol::JournalEntry {
            turn,
            kind: protocol::JournalKind::System,
            text,
            at,
            simplification: None,
        });
    }

    /// `Hello`, then `ModHello` (v13: before `Login`, which may attach to a running game at once) when the client
    /// has mods built in, then `Login`. A client with no mods sends no `ModHello`: to the server that is the same
    /// as an empty list, and the traffic stays what it was.
    fn send_hello_login(&mut self, sock: Sock) {
        let [hello, login] = self.sess.hello_login(&self.cfg.build);
        self.send(sock, &hello);
        if !self.cfg.built_in.is_empty() {
            let mods = ClientMsg::ModHello {
                api: self.cfg.mod_api,
                built_in: self.cfg.built_in.clone(),
            };
            self.send(sock, &mods);
        }
        self.send(sock, &login);
    }

    fn send_current(&mut self, msg: &ClientMsg) {
        if let Some(sock) = self.link.current() {
            self.send(sock, msg);
        }
    }

    fn send(&mut self, sock: Sock, msg: &ClientMsg) {
        match Postcard.encode(msg) {
            Ok(raw) => {
                let frame = Encoder::plain(&raw);
                self.stats.bytes_out += frame.len() as u64;
                self.out.push_back((sock, frame));
            }
            // Only the kind: a `Login` must not reach the log through its fields.
            Err(err) => tracing::error!(%err, kind = msg.kind(), "failed to encode a message"),
        }
    }

    /// Moves what `Link` decided into the queues of the shell.
    fn pump(&mut self) {
        for act in self.link.drain() {
            match act {
                LinkAct::Socket(cmd) => self.cmds.push_back(cmd),
                LinkAct::Ping {
                    sock,
                    seq,
                    client_ms,
                } => self.send(sock, &ClientMsg::Ping { seq, client_ms }),
                LinkAct::State(s) => self.events.push_back(ClientEvent::ConnChanged(s)),
            }
        }
    }

    fn unexpected(&mut self, kind: &'static str, state: &'static str) {
        if self.logged_unexpected.insert(kind) {
            tracing::warn!(kind, state, "message not expected in this state, ignored");
        }
    }

    fn handle(&mut self, msg: ServerMsg, sock: Sock, who: Who) {
        let phase = self.sess.phase();
        let current = who == Who::Current;
        match msg {
            ServerMsg::Welcome { .. } if current && phase == Phase::Handshake => {
                self.link.welcomed(self.now);
            }
            ServerMsg::Reject(reason) => self.on_reject(reason, sock, who),
            ServerMsg::LoggedIn(session) => {
                if who == Who::Attempt {
                    // The first probe to log in becomes the current socket; the old one closes.
                    self.link.promote(self.now, sock);
                    self.held_replaced = false;
                } else if !current || phase != Phase::Handshake && phase != Phase::Connecting {
                    return self.unexpected("LoggedIn", "not logging in");
                }
                self.sess.logged_in(session.token, session.nick);
                self.link.logged_in();
                self.link.set_can_probe(true);
                self.host_watch.reset_room();
                self.lobby.clear_state();
                self.account = None;
                // v12: ask for the account's store (an empty change); the answer is `ServerMsg::Account`.
                self.send_current(&ClientMsg::Account(AccountSave::default()));
                if let Some(token) = session.token {
                    self.events.push_back(ClientEvent::TokenIssued(token));
                }
                self.events.push_back(ClientEvent::LoggedIn);
            }
            ServerMsg::Account(a) if current && self.logged_in() => {
                self.account = Some(a);
                self.events.push_back(ClientEvent::AccountLoaded);
            }
            ServerMsg::Choices(c) if current && self.logged_in() => {
                self.lobby.set_choices(c);
                self.events.push_back(ClientEvent::LobbyChanged);
            }
            ServerMsg::LobbyState(s) if current && self.logged_in() => {
                let change = self.host_watch.room(s.room.as_ref(), self.sess.nick());
                self.lobby.set_state(s);
                if let Some(c) = change {
                    self.host_changed(c);
                }
                self.events.push_back(ClientEvent::LobbyChanged);
            }
            ServerMsg::Attached(a) if current && self.logged_in() => self.on_attached(a),
            ServerMsg::Detached { reason } => self.on_detached(reason, who),
            ServerMsg::LobbyError { text } if current => {
                self.sess.set_starting(false);
                self.events.push_back(ClientEvent::LobbyError(text));
            }
            ServerMsg::GameCtlRejected { ctl, reason } if current => {
                // Not in the stream, changes no state.
                self.events
                    .push_back(ClientEvent::GameCtlRejected { ctl, reason });
            }
            ServerMsg::Pong {
                client_ms,
                server_ms,
                ..
            } if current && self.link.current_welcomed() => {
                self.link.pong(self.now, client_ms, server_ms);
                stats::push_ring(
                    &mut self.stats.rtt_ring,
                    stats::RTT_RING,
                    self.now.saturating_sub(client_ms),
                );
            }
            ServerMsg::Game(batch) if current && phase == Phase::Attached => {
                self.on_game_batch(batch);
            }
            ServerMsg::QueryResult { id, result } if current && phase == Phase::Attached => {
                let own = self.sess.attach().map(|a| (a.game, a.stream));
                if own.is_some_and(|(game, stream)| self.orders.take_query(id, game, stream)) {
                    self.stats.queries_answered += 1;
                    self.events
                        .push_back(ClientEvent::QueryResult { id, answer: result });
                } else {
                    self.stats.queries_dropped += 1;
                }
            }
            other => self.unexpected(other.kind(), "not accepted here"),
        }
    }

    fn logged_in(&self) -> bool {
        matches!(self.sess.phase(), Phase::LoggedIn | Phase::Attached)
    }

    fn on_reject(&mut self, reason: RejectReason, sock: Sock, who: Who) {
        if who == Who::Attempt && reason == RejectReason::Full {
            // A full server turns a probe away; the probes go on at their pace.
            self.cmds.push_back(SocketCmd::Close { sock });
            self.link.on_close(self.now, sock, DisconnectWhy::Error);
            return;
        }
        self.sess.rejected_with(reason.clone());
        self.link.finish(ConnState::Closed);
        self.events.push_back(ClientEvent::Rejected(reason));
    }

    fn on_attached(&mut self, a: Attached) {
        let catalog = a.catalog;
        let game = a.game;
        let mode = a.mode;
        let outcome = self.sess.attached(a);
        self.orders.clear_queries();
        // A new attach: the server's focus is its default again.
        self.focus_sent = None;
        if outcome.new_game {
            self.stream.reset_game();
            // Another game: the orders in flight belonged to the old one (4.2).
            self.drop_orders();
        } else if outcome.fresh {
            self.stream.reset_fresh();
        }
        if outcome.fresh {
            self.host_watch.reset_seats();
            self.stats.attached_fresh += 1;
        } else {
            self.stats.attached_replay += 1;
            // A keyframe was awaited when the line dropped: the request may have died with the socket.
            if let Some(fx) = self.stream.resend_if_waiting(self.now, &mut self.stats) {
                self.apply_effects(vec![fx]);
            }
        }
        self.sess.set_basis_claim(self.stream.claim());
        self.link.attached(self.now);
        self.events.push_back(ClientEvent::Attached { game, mode });
        self.resend_orders();
        if self.catalog.is_none() || self.catalog_hash != Some(catalog.hash) {
            self.events.push_back(ClientEvent::CatalogNeeded(catalog));
        }
    }

    fn on_detached(&mut self, reason: DetachReason, who: Who) {
        if reason == DetachReason::Replaced {
            match who {
                // The old socket after we replaced it was already ignored as `Stale`.
                Who::Current if self.link.attempt_logging_in() => self.held_replaced = true,
                Who::Current | Who::Attempt => self.apply_replaced(),
                Who::Stale => {}
            }
            return;
        }
        if who != Who::Current {
            return;
        }
        self.sess.detached(&reason);
        self.stream.clear_view();
        self.drop_orders();
        self.events.push_back(ClientEvent::Detached(reason));
    }

    /// After every `Attached`, in either mode, the orders without a final answer go out again in the order of
    /// their numbers, with the same `id` and `turn`. This is not a queue: the player gave them while the
    /// line was up, and the gateway answers each `id` once (4.5).
    fn resend_orders(&mut self) {
        for cmd in self.orders.unanswered() {
            self.stats.orders_resent += 1;
            self.send_current(&ClientMsg::Cmd(cmd));
        }
    }

    /// The game is over for this connection (`Detached`, `Replaced`, another game): orders and queries in
    /// flight have no one to answer them.
    fn drop_orders(&mut self) {
        self.stats.orders_dropped += self.orders.clear() as u64;
        self.overlay.clear();
    }

    /// Answers of a batch, after its view (4.5). Final answers are events; a failed order is also a line
    /// in the journal and an `OrderRolledBack`, and its picture comes off the layer at the same moment (4.4)
    /// unless a keyframe is awaited: then the record stays until the keyframe, or the unit would jump back
    /// before the view that explains it. Returns whether an order to a unit or a city was applied.
    fn settle_orders(&mut self, results: Vec<CommandResult>) -> bool {
        let awaiting = self.stream.awaiting_keyframe();
        let me = self.my_player();
        let mut applied_order = false;
        for r in results {
            let Some(s) = self.orders.answer(r, &mut self.stats) else {
                continue;
            };
            let unit_or_city = unit_of(&s.cmd).is_some()
                || matches!(
                    s.cmd,
                    Command::CitySetProduction { .. }
                        | Command::CityPurchase { .. }
                        | Command::CityRangeAttack { .. }
                );
            stats::push_ring(
                &mut self.stats.order_ms_ring,
                stats::ORDER_RING,
                self.now.saturating_sub(s.sent_ms),
            );
            if s.outcome == protocol::CommandOutcome::Applied && unit_or_city {
                applied_order = true;
            }
            if let Some(me) = me {
                if awaiting {
                    self.overlay.mark_answered(s.id);
                } else {
                    let touched = self.overlay.remove(s.id, me);
                    self.layer_touched(&touched);
                }
            }
            self.events.push_back(ClientEvent::OrderAnswered {
                id: s.id,
                outcome: s.outcome,
            });
            if let Some(text) = s.text {
                self.stats.layer_rollbacks += 1;
                let at = self.order_cell(&s.cmd);
                self.journal_note(text.clone(), at);
                self.events
                    .push_back(ClientEvent::OrderRolledBack { id: s.id, text });
            }
        }
        applied_order
    }

    /// `Replaced` on the current socket is final: no reconnecting until the player says so (NC-D15).
    fn apply_replaced(&mut self) {
        self.sess.detached(&DetachReason::Replaced);
        self.stream.clear_view();
        self.drop_orders();
        self.link.finish(ConnState::Replaced);
        self.events
            .push_back(ClientEvent::Detached(DetachReason::Replaced));
    }

    /// A batch of the game stream (4.3): duplicates are dropped, a skipped `seq` breaks the stream, the rest
    /// is applied whole and in order.
    fn on_game_batch(&mut self, batch: GameBatch) {
        let last = self.sess.attach().map_or(0, |a| a.last_seq);
        if batch.seq <= last {
            self.stats.dup_batches += 1;
            return;
        }
        if batch.seq > last + 1 {
            self.stream_break(BreakWhy::Gap);
            return;
        }
        let Some(game) = self.sess.attach().map(|a| a.game) else {
            return;
        };
        let seq = batch.seq;
        // The answers to orders are the stream's last word in a batch: they are applied after the view.
        let results: Vec<CommandResult> = batch
            .msgs
            .iter()
            .filter_map(|m| match m {
                GameMsg::Results(r) => Some(r.as_slice()),
                _ => None,
            })
            .flatten()
            .cloned()
            .collect();
        let finished_before = self.my_finished();
        let mut cx = Cx {
            now: self.now,
            game,
            nick: self.sess.nick(),
            host: &mut self.host_watch,
            stats: &mut self.stats,
        };
        let fx = self.stream.on_batch(batch, &mut cx);
        let broken = fx.iter().any(|e| matches!(e, Effect::Break(_)));
        if !broken {
            self.sess.note_seq(seq);
        }
        self.apply_effects(fx);
        let replica = self.stream.take_replica();
        let mods = self.stream.take_mods();
        if !broken {
            self.events.extend(mods);
            self.stats.replica_msgs += replica.len() as u64;
            self.events
                .extend(replica.into_iter().map(ClientEvent::Replica));
        }
        if !broken && self.stream.view().is_some() {
            self.events.push_back(ClientEvent::ViewChanged);
        }
        // A broken batch is not taken: the reconnect brings it again, answers included.
        let applied_order = !broken && self.settle_orders(results);
        if !self.stream.awaiting_keyframe()
            && let Some(me) = self.my_player()
        {
            // The keyframe is in: the records whose answers were held go with it.
            let touched = self.overlay.remove_answered(me);
            self.layer_touched(&touched);
        }
        self.refresh_shown();
        // Without a picture (an attack) the end of turn is taken back by the batch that applies the order:
        // the view and the turn state with `finished = false` arrive together (NP-D23).
        if !broken
            && applied_order
            && finished_before == Some(true)
            && self.my_finished() == Some(false)
        {
            self.end_turn_cancelled(true);
        }
    }

    fn apply_effects(&mut self, effects: Vec<Effect>) {
        for e in effects {
            match e {
                Effect::NeedKeyframe(reason) => self.send_need_keyframe(reason),
                Effect::Break(why) => self.stream_break(why),
                Effect::Basis(claim) => self.sess.set_basis_claim(claim),
                Effect::HostChanged(c) => self.host_changed(c),
            }
        }
    }

    fn send_need_keyframe(&mut self, reason: KeyframeReason) {
        self.send_current(&ClientMsg::NeedKeyframe { reason });
    }

    /// The ladder of stream failures (4.3): the first reconnects with `resume` (the gateway sends the
    /// tail), the second without it (`Fresh`), the third within a minute is `Fatal`.
    fn stream_break(&mut self, why: BreakWhy) {
        self.stats.stream_breaks += 1;
        let mut n = self.stream.register_break(self.now);
        if why == BreakWhy::KeyframeTimeout {
            // No keyframe twice: `resume` has been tried by the very request, start over.
            n = n.max(2);
        }
        self.events.push_back(ClientEvent::StreamBroken(why));
        match n {
            1 => self.link.break_current(self.now),
            2 => {
                self.sess.forget_resume();
                self.link.break_current(self.now);
            }
            _ => {
                let text = "The client and the server are not compatible".to_string();
                self.sess.set_fatal(text.clone());
                self.link.finish(ConnState::Closed);
                self.events.push_back(ClientEvent::Fatal(text));
            }
        }
    }

    fn host_changed(&mut self, c: HostChange) {
        self.stats.host_changes += 1;
        self.events.push_back(ClientEvent::HostChanged {
            from: c.from,
            to: c.to,
            me: c.me,
        });
    }
}
