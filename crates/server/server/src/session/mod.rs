//! One task per WebSocket: `Hello` + `Login`, then the lobby stream. The task is
//! the only writer to its socket. It sends a WS ping every `ping_interval`, drops the session after
//! `idle_timeout` without any frame, writes what other tasks queue in its [`outbox`], and closes
//! with 1013 when that queue overflows. The game stream comes with the seat pipelines.

pub mod limits;
pub mod login;
pub mod outbox;

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use protocol::{ClientMsg, Codec, DetachReason, GameCtlReason, Postcard, RejectReason, ServerMsg};
use tokio::sync::oneshot;
use tokio::time::{Instant, Interval, MissedTickBehavior};
use wire::{Encoder, Limits, WireError};

use self::limits::{PendingGuard, SessionLimits};
use self::outbox::{Outbox, OutboxRx, OverflowWatch};
use crate::Shared;
use crate::game::{ActorMsg, ClientGameMsg, SessionLink, link_of};
use crate::lobby::{LobbyMsg, LoginInfo};
use crate::metrics::Metrics;

/// Close reason of a session that a newer login of the same account replaced.
const REPLACED: &str = "replaced by a newer session";

/// WebSocket close code 1013 "try again later": a client too slow to read, or too many sockets.
pub const CLOSE_TRY_AGAIN: u16 = 1013;

/// Outboxes of the logged-in sessions by session id: how other tasks reach a socket.
#[derive(Default)]
pub struct Sessions {
    map: Mutex<HashMap<u64, Outbox>>,
}

impl Sessions {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Outbox>> {
        self.map.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn insert(&self, id: u64, outbox: Outbox) {
        self.lock().insert(id, outbox);
    }

    fn remove(&self, id: u64) {
        self.lock().remove(&id);
    }

    /// The send queue of a logged-in session, if it is still there.
    pub fn outbox(&self, id: u64) -> Option<Outbox> {
        self.lock().get(&id).cloned()
    }

    /// Logged-in sessions with a queue.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// What arrived on the socket.
enum Incoming {
    Frame(Vec<u8>),
    /// Close with a log reason.
    End(&'static str),
}

/// The socket and what a session needs around it.
pub(crate) struct Conn {
    socket: WebSocket,
    shared: Arc<Shared>,
    id: u64,
    /// Fires when a newer login of the account replaces this session.
    kick: Option<oneshot::Receiver<()>>,
    outbox: Outbox,
    outbox_rx: OutboxRx,
    overflow: OverflowWatch,
    /// When the last frame of any kind (data, ping, pong) came from the client.
    last_rx: Instant,
    /// `Hello` + `Login` must be done by then; `None` once logged in.
    login_deadline: Option<Instant>,
    /// WS ping timer, used after login.
    ping: Interval,
    /// Close code to send instead of a bare close.
    close_code: Option<u16>,
    /// The resume point of the login, taken once when the session attaches.
    resume: Option<protocol::Resume>,
    /// Turns `true` when the gateway has drained and closes every socket (1012).
    closing: tokio::sync::watch::Receiver<bool>,
    /// When the last WS ping went out, and the round trip of the pong that answered it, microseconds.
    ping_sent: Option<Instant>,
    ws_rtt_us: Option<u64>,
}

impl Conn {
    fn new(socket: WebSocket, shared: Arc<Shared>, id: u64, kick: oneshot::Receiver<()>) -> Self {
        let (outbox, outbox_rx) = outbox::channel(
            shared.gate.config().outbox_msgs,
            shared.gate.config().outbox_bytes,
        );
        let every = shared.config.ping_interval.max(Duration::from_millis(1));
        let mut ping = tokio::time::interval_at(Instant::now() + every, every);
        ping.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let now = Instant::now();
        Self {
            socket,
            id,
            kick: Some(kick),
            outbox,
            overflow: outbox_rx.watch(),
            outbox_rx,
            last_rx: now,
            login_deadline: Some(now + shared.config.login_timeout),
            ping,
            close_code: None,
            resume: None,
            closing: shared.closing.subscribe(),
            ping_sent: None,
            ws_rtt_us: None,
            shared,
        }
    }

    /// Keeps a `ModHello` for the games the session attaches to. `false`: it is over the protocol's bounds.
    fn take_mod_hello(&self, api: (u16, u16, u16), built_in: Vec<protocol::BuiltMod>) -> bool {
        let hello = crate::game::modhost::ClientMods { api, built_in };
        if !hello.within_limits() {
            return false;
        }
        self.shared.mod_hellos.set(self.id, hello);
        true
    }

    /// Login is done: the idle rules and the WS ping start now.
    fn logged_in(&mut self) {
        self.login_deadline = None;
        self.last_rx = Instant::now();
        self.ping.reset();
    }

    /// Next binary frame, or why the session ends. Any frame from the client, control frames
    /// included, resets the idle timer. Queued outgoing messages are written on the way.
    async fn recv(&mut self) -> Incoming {
        loop {
            let logged_in = self.login_deadline.is_none();
            let deadline = self
                .login_deadline
                .unwrap_or(self.last_rx + self.shared.config.idle_timeout);
            let kicked = async {
                match self.kick.as_mut() {
                    Some(rx) => rx.await,
                    None => std::future::pending().await,
                }
            };
            // Biased: a kick or an overflow wins over data that is always ready to write.
            tokio::select! {
                biased;
                res = kicked => match res {
                    Ok(()) => return Incoming::End(REPLACED),
                    // Sender gone without a kick: stop listening for it.
                    Err(_) => self.kick = None,
                },
                Ok(()) = self.closing.changed() => {
                    if *self.closing.borrow() {
                        // The drain is done: "service restart".
                        self.close_code = Some(1012);
                        return Incoming::End("server restarting");
                    }
                }
                () = self.overflow.wait() => {
                    Metrics::bump(&self.shared.metrics.sessions_slow_consumer_total);
                    self.close_code = Some(CLOSE_TRY_AGAIN);
                    return Incoming::End("slow consumer");
                }
                () = tokio::time::sleep_until(deadline) => {
                    if logged_in {
                        Metrics::bump(&self.shared.metrics.sessions_lost_timeout_total);
                    }
                    return Incoming::End("timeout");
                }
                _ = self.ping.tick(), if logged_in => {
                    if let Err(reason) = self.write(Message::Ping(Vec::new().into())).await {
                        return Incoming::End(reason);
                    }
                    self.ping_sent = Some(Instant::now());
                }
                res = self.socket.recv() => match res {
                    None => return Incoming::End("closed by client"),
                    Some(Err(_)) => return Incoming::End("transport error"),
                    Some(Ok(msg)) => {
                        self.last_rx = Instant::now();
                        match msg {
                            Message::Binary(data) => return Incoming::Frame(data.to_vec()),
                            Message::Close(_) => return Incoming::End("closed by client"),
                            Message::Text(_) => {
                                return Incoming::End("text messages are not allowed");
                            }
                            Message::Pong(_) => {
                                if let Some(t) = self.ping_sent.take() {
                                    self.ws_rtt_us =
                                        Some(u64::try_from(t.elapsed().as_micros()).unwrap_or(u64::MAX));
                                }
                            }
                            Message::Ping(_) => {}
                        }
                    }
                },
                item = self.outbox_rx.recv() => {
                    // `self.outbox` keeps the queue open, so `None` does not happen.
                    if let Some(frame) = item {
                        if frame.is_empty() {
                            // The game refused this session (`Reject` is written): nothing more to say.
                            return Incoming::End("refused by the game");
                        }
                        if let Err(reason) = self.send_raw(frame).await {
                            return Incoming::End(reason);
                        }
                    }
                }
            }
        }
    }

    /// Sends one framed message.
    async fn send(&mut self, msg: &ServerMsg) -> Result<(), &'static str> {
        let frame = Encoder::default().frame_msg(msg).map_err(|err| {
            tracing::error!(%err, "failed to encode server message");
            "send error"
        })?;
        self.send_raw(frame).await
    }

    async fn send_raw(&mut self, bytes: Vec<u8>) -> Result<(), &'static str> {
        self.write(Message::Binary(bytes.into())).await
    }

    /// Writes one WS message, at most `send_timeout`.
    async fn write(&mut self, msg: Message) -> Result<(), &'static str> {
        let timeout = self.shared.config.send_timeout;
        match tokio::time::timeout(timeout, self.socket.send(msg)).await {
            Err(_) => Err("send timeout"),
            Ok(Err(_)) => Err("send error"),
            Ok(Ok(())) => Ok(()),
        }
    }

    /// Sends `Reject` and counts it; the caller closes with the returned reason.
    async fn reject(&mut self, reason: RejectReason, why: &'static str) -> &'static str {
        Metrics::bump(&self.shared.metrics.login_failed_total);
        match self.send(&ServerMsg::Reject(reason)).await {
            Ok(()) => why,
            Err(send_err) => send_err,
        }
    }
}

/// Unframes and decodes one client frame (`enc 0` only, at most 64 KiB, 4.13).
fn decode(frame: &[u8]) -> Result<ClientMsg, &'static str> {
    let body = wire::unframe(frame, &Limits::GATEWAY).map_err(|e| match e {
        WireError::CompressedNotAllowed => "compressed frame from client",
        _ => "bad frame",
    })?;
    Postcard
        .decode::<ClientMsg>(&body)
        .map_err(|_| "malformed message")
}

/// Closes a socket with 1013 without a session: too many unfinished connections from one IP.
async fn refuse(mut socket: WebSocket, timeout: Duration) {
    let frame = CloseFrame {
        code: CLOSE_TRY_AGAIN,
        reason: "too many connections".into(),
    };
    let _ = tokio::time::timeout(timeout, socket.send(Message::Close(Some(frame)))).await;
}

/// How long the gateway waits for the client's side of the close handshake (and reads away what the client
/// still sends) before it drops the socket.
const CLOSE_DRAIN: Duration = Duration::from_millis(1000);

/// `label` names the device (from the `User-Agent` of the upgrade request); `cookie` is its session cookie.
pub async fn run(
    socket: WebSocket,
    shared: Arc<Shared>,
    peer: IpAddr,
    label: String,
    cookie: Option<(String, protocol::Token)>,
) {
    let Some(mut pending) = shared.gate.enter_pending(peer) else {
        tracing::info!(%peer, "too many unfinished connections from this address");
        refuse(socket, shared.config.send_timeout).await;
        return;
    };
    let id = shared
        .next_session
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Metrics::bump(&shared.metrics.sessions_opened_total);
    Metrics::bump(&shared.metrics.sessions_open);
    let (kick_tx, kick_rx) = oneshot::channel();
    let mut conn = Conn::new(socket, shared.clone(), id, kick_rx);

    let mut nick = None;
    let link = SessionLink::default();
    let reason = drive(
        &mut conn,
        kick_tx,
        &mut nick,
        &mut pending,
        peer,
        &link,
        &label,
        cookie.as_ref(),
    )
    .await;

    shared.sessions.remove(id);
    shared.mod_hellos.remove(id);
    if reason == "closed by client" {
        Metrics::bump(&shared.metrics.sessions_closed_clean_total);
    }
    if let Some(nick) = &nick {
        // The seat (if any) goes to `Lost`; the lobby forgets the session.
        if let Some(l) = link_of(&link) {
            let _ = l.tx.send(ActorMsg::SessionGone {
                player: l.player,
                session: id,
            });
        }
        shared.lobby.send(LobbyMsg::Gone {
            nick: nick.clone(),
            session: id,
        });
        shared.identity.unregister(nick, id);
        Metrics::drop_one(&shared.metrics.sessions_logged_in);
    }
    Metrics::drop_one(&shared.metrics.sessions_open);
    // Free the address before the client can see the close.
    drop(pending);
    // Bound Close too: a non-reading client must not hold the task.
    let close = conn.close_code.map(|code| CloseFrame {
        code,
        reason: reason.into(),
    });
    let send_timeout = shared.config.send_timeout;
    if tokio::time::timeout(send_timeout, conn.socket.send(Message::Close(close)))
        .await
        .is_err()
    {
        tracing::warn!(session = id, "timeout sending Close");
    } else {
        // Do not drop the socket with client bytes unread (a Login sent right after Hello, an order in flight):
        // the OS answers that with a reset (10054 on Windows) and the client loses the Close/Reject before
        // reading it. Read until the client's Close or a short bound, then drop.
        let socket = &mut conn.socket;
        let _ = tokio::time::timeout(CLOSE_DRAIN, async {
            while let Some(Ok(m)) = socket.recv().await {
                if matches!(m, Message::Close(_)) {
                    break;
                }
            }
        })
        .await;
    }
    tracing::info!(
        session = id,
        %peer,
        nick = nick.as_deref().unwrap_or("-"),
        reason,
        "session closed"
    );
}

#[allow(clippy::too_many_arguments)]
async fn drive(
    conn: &mut Conn,
    kick_tx: oneshot::Sender<()>,
    nick_out: &mut Option<String>,
    pending: &mut PendingGuard,
    peer: IpAddr,
    link: &SessionLink,
    label: &str,
    cookie: Option<&(String, protocol::Token)>,
) -> &'static str {
    let nick = match login::handshake(conn, kick_tx, peer, label, cookie).await {
        Ok(nick) => nick,
        Err(reason) => return reason,
    };
    pending.release();
    conn.logged_in();
    conn.shared.sessions.insert(conn.id, conn.outbox.clone());
    Metrics::bump(&conn.shared.metrics.sessions_logged_in);
    *nick_out = Some(nick);

    // The lobby actor sends `Choices` and `LobbyState`, or the game sends `Attached`.
    conn.shared.ensure_actors();
    let resume = conn.resume.take();
    conn.shared.lobby.send(LobbyMsg::Login(LoginInfo {
        nick: nick_out.clone().unwrap_or_default(),
        session: conn.id,
        out: conn.outbox.clone(),
        link: link.clone(),
        resume,
    }));

    let mut limits =
        SessionLimits::new(conn.shared.gate.config(), conn.shared.config.clock.now_ms());
    loop {
        let data = match conn.recv().await {
            Incoming::Frame(data) => data,
            Incoming::End(reason) => {
                // A kicked session learns why before the socket closes.
                if reason == REPLACED {
                    Metrics::bump(&conn.shared.metrics.sessions_replaced_total);
                    let _ = conn
                        .send(&ServerMsg::Detached {
                            reason: DetachReason::Replaced,
                        })
                        .await;
                }
                return reason;
            }
        };
        let size = data.len();
        // The round trip of the WS ping goes to the seat the session plays, if it plays one.
        if let Some(us) = conn.ws_rtt_us.take()
            && let Some(l) = link_of(link)
            && let Some(g) = conn.shared.games.get(l.game)
        {
            g.info.seat_stats.seat(l.player).ws_rtt.record_us(us);
        }
        match decode(&data) {
            Ok(ClientMsg::Ping { seq, client_ms }) => {
                let pong = ServerMsg::Pong {
                    seq,
                    client_ms,
                    server_ms: conn.shared.config.clock.now_ms(),
                };
                if let Err(reason) = conn.send(&pong).await {
                    return reason;
                }
            }
            Ok(ClientMsg::Lobby(cmd)) => {
                if limits.lobby_ok(conn.shared.config.clock.now_ms()) {
                    conn.shared.lobby.send(LobbyMsg::Cmd {
                        nick: nick_out.clone().unwrap_or_default(),
                        session: conn.id,
                        cmd,
                    });
                } else {
                    let err = ServerMsg::LobbyError {
                        text: "too many lobby commands".into(),
                    };
                    if let Err(reason) = conn.send(&err).await {
                        return reason;
                    }
                }
            }
            Ok(ClientMsg::Cmd(c)) => game_msg(link, conn.id, ClientGameMsg::Cmd(c), size),
            Ok(ClientMsg::Focus(f)) => game_msg(link, conn.id, ClientGameMsg::Focus(f), size),
            Ok(ClientMsg::Query { id, q }) => {
                game_msg(link, conn.id, ClientGameMsg::Query { id, q }, size);
            }
            Ok(ClientMsg::NeedKeyframe { reason }) => {
                game_msg(link, conn.id, ClientGameMsg::NeedKeyframe(reason), size);
            }
            // Chat only exists in the stream of a game: without a seat there is nobody to tell.
            Ok(ClientMsg::Chat(c)) => game_msg(link, conn.id, ClientGameMsg::Chat(c), size),
            Ok(ClientMsg::GcNet(bytes)) => {
                game_msg(link, conn.id, ClientGameMsg::GcNet(bytes), size);
            }
            Ok(ClientMsg::GcResync {
                epoch,
                have_raw_hash,
                at_jseq,
                reason,
            }) => game_msg(
                link,
                conn.id,
                ClientGameMsg::GcResync {
                    epoch,
                    have_raw_hash,
                    at_jseq,
                    reason,
                },
                size,
            ),
            Ok(ClientMsg::GameCtl(ctl)) => {
                if link_of(link).is_some() {
                    game_msg(link, conn.id, ClientGameMsg::GameCtl(ctl), size);
                } else {
                    // Not in a game: nothing to control.
                    let msg = ServerMsg::GameCtlRejected {
                        ctl,
                        reason: GameCtlReason::NoGame,
                    };
                    if let Err(reason) = conn.send(&msg).await {
                        return reason;
                    }
                }
            }
            // v12: the account's store. An empty change asks for the store (the client does it right after the
            // login, so the burst of a login stays `LoggedIn`, `Choices`, `LobbyState` for every older script).
            // A refused change is dropped with a line in the log (the client validates with the same functions, so
            // only a broken client sends it); the rate is the lobby's.
            Ok(ClientMsg::Account(save)) => {
                if limits.lobby_ok(conn.shared.config.clock.now_ms()) {
                    let nick = nick_out.clone().unwrap_or_default();
                    if save.is_empty() {
                        let msg = ServerMsg::Account(conn.shared.prefs.get(&nick));
                        if let Err(reason) = conn.send(&msg).await {
                            return reason;
                        }
                    } else if let Err(e) = conn.shared.prefs.save(&nick, &save) {
                        tracing::warn!(%nick, error = %e, "account settings refused");
                    }
                }
            }
            // v13: what the client has built in. It is read when the session attaches to a game, so it should come
            // before `Login`; a later one is kept for the next attach or join.
            Ok(ClientMsg::ModHello { api, built_in }) => {
                if !conn.take_mod_hello(api, built_in) {
                    return "bad ModHello";
                }
            }
            // v13: a message of a mod's client half; the game checks and routes it (a game without mods drops it).
            Ok(ClientMsg::Mod(m)) => game_msg(link, conn.id, ClientGameMsg::Mod(m), size),
            Ok(ClientMsg::Hello { .. }) => return "repeated Hello",
            Ok(ClientMsg::Login(_)) => return "repeated Login",
            Ok(other) => {
                let kind = other.kind().to_owned();
                return conn
                    .reject(RejectReason::Unsupported { kind }, "unsupported message")
                    .await;
            }
            Err(reason) => return reason,
        }
    }
}

/// Hands a game message of the client to the game the session plays; nothing if it plays none.
fn game_msg(link: &SessionLink, session: u64, msg: ClientGameMsg, bytes: usize) {
    if let Some(l) = link_of(link) {
        let _ = l.tx.send(ActorMsg::Client {
            player: l.player,
            session,
            msg,
            bytes,
        });
    }
}
