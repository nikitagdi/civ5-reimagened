//! The shell of the client network (net-client 4.1, NC4): `NetClient` and `NetHandle` around the sans-IO
//! `ClientCore`. It executes the socket commands of the core (the current socket and the probe sockets of
//! NC-D17), stores the token, fetches the catalog over HTTP and forwards the events. Reconnecting, pauses
//! and jitter are decided by the core; this shell only opens and closes what it is told to.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use client_core::{
    AddrContext, AddrError, ClientCore, ClientEvent, ClientPhase, DisconnectWhy, NetConfig, Sock,
    SocketCmd, catalog_format, http_base, resolve_server,
};
use platform::{
    CloseReason, Connection, HttpClient, HttpError, HttpOp, HttpResponse, KeyValueStore, Transport,
    TransportEvent,
};
use protocol::{Catalog, CatalogRef, Codec, Postcard, RejectReason, Token};

/// Where the nick of the last login is kept (`civ5.nick`, net-client 4.2 item 3).
pub const NICK_KEY: &str = "civ5.nick";

/// Where the token of a nick is kept: two nicks in one browser do not share a token.
pub fn token_key(nick: &str) -> String {
    format!("civ5.token.{nick}")
}

/// The nick of the last login, if the store has one.
pub fn stored_nick(store: &dyn KeyValueStore) -> Option<String> {
    store.get(NICK_KEY).filter(|n| !n.is_empty())
}

/// Passes of the I/O loop in one `pump`; a transport that never runs dry cannot hold the frame.
const MAX_PASSES: usize = 16;
/// Pauses after the first, second ... failure of the catalog request; the fifth failure is final.
const CATALOG_RETRY_MS: [u64; 4] = [1_000, 2_000, 4_000, 8_000];
const CATALOG_ATTEMPTS: u32 = 5;
const CATALOG_FATAL: &str = "Cannot load the game catalog";

/// What the page's events mean to the core.
fn map_host_event(ev: platform::HostEvent) -> client_core::HostEvent {
    match ev {
        platform::HostEvent::Online => client_core::HostEvent::Online,
        platform::HostEvent::Offline => client_core::HostEvent::Offline,
        platform::HostEvent::Visible => client_core::HostEvent::Visible,
        platform::HostEvent::Hidden => client_core::HostEvent::Hidden,
        platform::HostEvent::PageHide => client_core::HostEvent::PageHide,
    }
}

fn map_close(reason: &CloseReason) -> DisconnectWhy {
    match reason {
        CloseReason::Local => DisconnectWhy::Local,
        CloseReason::Remote { code, .. } => DisconnectWhy::Remote(*code),
        CloseReason::Error(_) => DisconnectWhy::Error,
    }
}

/// Reads a stored token: 32 hexadecimal digits. Anything else is not a token.
fn parse_token(s: &str) -> Option<Token> {
    if s.len() != 32 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(Token(out))
}

fn format_token(t: &Token) -> String {
    use std::fmt::Write;
    t.0.iter().fold(String::with_capacity(32), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// The events of the session and the orders go to the log ring (`net.*`, `cmd.*`): the token never does.
fn log_event(ev: &ClientEvent) {
    use protocol::CommandOutcome as O;
    match ev {
        ClientEvent::LoggedIn => tracing::info!("net.login: logged in"),
        ClientEvent::Rejected(why) => tracing::warn!(?why, "net.login: the login was refused"),
        ClientEvent::Attached { game, mode } => {
            tracing::info!(game, ?mode, "net.attach: attached to the game")
        }
        ClientEvent::Detached(why) => tracing::info!(?why, "net.attach: detached from the game"),
        ClientEvent::ConnChanged(state) => tracing::info!(?state, "net.conn: the line changed"),
        ClientEvent::StreamBroken(why) => {
            tracing::warn!(?why, "net.stream: the stream is broken, reconnecting")
        }
        ClientEvent::OrderAnswered { id, outcome } => match outcome {
            O::Applied | O::Queued => {
                tracing::debug!(id, ?outcome, "cmd.result: the order was answered")
            }
            O::Rejected { .. } | O::Lost => {
                tracing::info!(id, ?outcome, "cmd.result: the order was not applied")
            }
        },
        ClientEvent::OrderRolledBack { id, text } => {
            tracing::warn!(id, %text, "cmd.result: the order was rolled back")
        }
        ClientEvent::Fatal(text) => tracing::error!(%text, "net.fatal: the client cannot go on"),
        _ => {}
    }
}

fn load_token(store: &dyn KeyValueStore, nick: &str) -> Option<Token> {
    if !store.keeps_tokens() {
        return None;
    }
    let key = token_key(nick);
    let raw = store.get(&key)?;
    let token = parse_token(&raw);
    if token.is_none() {
        // Damaged by hand or by another version: forget it, the player logs in with the password.
        store.remove(&key);
    }
    token
}

/// The catalog being fetched: `GET {http}/catalog/{hash:016x}` with the retries of 4.11.
struct CatalogJob {
    want: CatalogRef,
    failures: u32,
    op: Option<Box<dyn HttpOp>>,
    /// Start the next request at this time.
    retry_at: Option<u64>,
}

/// The body of a catalog response, checked: status, hash, prefix, content.
fn check_catalog(
    want: &CatalogRef,
    res: Result<HttpResponse, HttpError>,
) -> Result<Catalog, String> {
    let resp = res.map_err(|e| e.to_string())?;
    if !resp.is_success() {
        return Err(format!("HTTP status {}", resp.status));
    }
    let body = resp.body;
    if body.len() as u64 != u64::from(want.len) {
        return Err(format!(
            "the body has {} bytes, the server said {}",
            body.len(),
            want.len
        ));
    }
    if protocol::xxh64(0, &body) != want.hash {
        return Err("the body does not match its hash".to_string());
    }
    let rest = catalog_format::strip_catalog_header(&body).map_err(|e| e.to_string())?;
    Postcard
        .decode::<Catalog>(rest)
        .map_err(|e| format!("the catalog cannot be read: {e}"))
}

/// `ClientCore` with its sockets (net-client 4.1). It owns the transport, the HTTP client and the store; the
/// application gives it the time and the page events and reads the core through [`NetHandle::with`].
pub struct NetClient {
    core: ClientCore,
    transport: Box<dyn Transport>,
    http: Box<dyn HttpClient>,
    store: Box<dyn KeyValueStore>,
    ws_url: String,
    http_base: Option<String>,
    warning: Option<String>,
    conns: HashMap<Sock, Box<dyn Connection>>,
    catalog: Option<CatalogJob>,
    /// The shell's own fatal error (the catalog); `phase()` shows it.
    fatal: Option<String>,
    events: VecDeque<ClientEvent>,
    /// Messages of the replica stream, in order (M2): taken by the replica's owner.
    replica_msgs: Vec<client_core::ReplicaMsg>,
    /// Bodies being fetched: GET {http}/replica/{hash:016x}.
    blob_jobs: Vec<(u64, Box<dyn HttpOp>)>,
    now: u64,
    /// The sockets are not read before this time (test port).
    silent_until: u64,
}

impl NetClient {
    /// The address is checked here (NC-D16): the password goes only to the page's own address or to the one
    /// a developer chose. The token of `cfg.nick` is read from the store. Nothing connects before
    /// [`NetClient::connect`].
    ///
    /// # Errors
    /// The address is refused (plain `http` on a public page, no server for a desktop, ...).
    pub fn new(
        cfg: NetConfig,
        addr: AddrContext<'_>,
        server_param: Option<&str>,
        transport: Box<dyn Transport>,
        http: Box<dyn HttpClient>,
        store: Box<dyn KeyValueStore>,
    ) -> Result<Self, AddrError> {
        let server = resolve_server(addr, server_param)?;
        if let Some(w) = &server.warning {
            tracing::warn!("{w}");
        }
        let mut core = ClientCore::new(cfg.clone());
        core.set_token(load_token(&*store, &cfg.nick));
        Ok(NetClient {
            core,
            transport,
            http,
            store,
            http_base: http_base(&server.url),
            ws_url: server.url,
            warning: server.warning,
            conns: HashMap::new(),
            catalog: None,
            fatal: None,
            events: VecDeque::new(),
            replica_msgs: Vec::new(),
            blob_jobs: Vec::new(),
            now: 0,
            silent_until: 0,
        })
    }

    /// Why `?server=` was ignored, if it was (for the log of the page).
    pub fn address_warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }

    pub fn url(&self) -> &str {
        &self.ws_url
    }

    pub fn core(&self) -> &ClientCore {
        &self.core
    }

    /// The core, for the player's actions. Call [`NetClient::flush`] after, or let [`NetHandle::with`] do it.
    pub fn core_mut(&mut self) -> &mut ClientCore {
        &mut self.core
    }

    /// What the screens show: the core's phase, or the shell's fatal error.
    pub fn phase(&self) -> ClientPhase {
        match &self.fatal {
            Some(text) => ClientPhase::Fatal(text.clone()),
            None => self.core.phase(),
        }
    }

    /// Sockets the shell holds now (the current one and live probes).
    pub fn socket_count(&self) -> usize {
        self.conns.len()
    }

    /// The first socket. Hello and Login go out as soon as it opens.
    pub fn connect(&mut self, now: u64) {
        self.now = now;
        self.core.connect(now);
        self.pump(now);
    }

    /// The player typed a nick and password. The token of that nick (not of the earlier one) is taken from
    /// the store.
    pub fn login(&mut self, nick: &str, password: &str, now: u64) {
        self.now = now;
        self.core.set_token(load_token(&*self.store, nick));
        self.core.login(nick, password);
        self.pump(now);
    }

    /// "Sign out" (lobby-ui 4.3): forgets the token of the nick and drops the sockets. The core reconnects with
    /// no token and no password, so the gateway asks for the password again (a server without one lets the
    /// nick straight back in).
    pub fn sign_out(&mut self, now: u64) {
        self.now = now;
        self.store.remove(&token_key(self.core.nick()));
        self.core.sign_out();
        let socks: Vec<Sock> = self.conns.keys().copied().collect();
        for sock in socks {
            if let Some(mut conn) = self.conns.remove(&sock) {
                conn.close();
                self.core.on_disconnected(now, sock, DisconnectWhy::Local);
            }
        }
        self.pump(now);
    }

    /// The page told something about itself.
    pub fn host_event(&mut self, ev: platform::HostEvent, now: u64) {
        self.now = now;
        self.core.host_event(now, map_host_event(ev));
        self.pump(now);
    }

    /// The next event for the screens; the shell has seen it first (token stored, catalog started).
    pub fn poll_event(&mut self) -> Option<ClientEvent> {
        self.events.pop_front()
    }

    /// One turn of the network: time, what the sockets received, the sockets the core asks for, what the
    /// core wants to send, the events, the catalog. Called every frame, by the safety timer and after page
    /// events (NC-D9); cheap when nothing happens.
    pub fn pump(&mut self, now: u64) {
        use render::perf::slow::scope;
        self.now = now;
        let s = scope("net.core_tick");
        self.core.tick(now);
        drop(s);
        let s = scope("net.io");
        self.io(now);
        drop(s);
        let s = scope("net.collect_events");
        self.collect_events();
        drop(s);
        let s = scope("net.catalog_step");
        self.catalog_step(now);
        drop(s);
        // A catalog that failed for good, or an event that started a job, may have changed what is due.
        let _s = scope("net.io");
        self.io(now);
    }

    /// Sends what the core queued and executes its socket commands, without reading the sockets: after an
    /// order or a chat line.
    pub fn flush(&mut self) {
        let now = self.now;
        self.run_commands(now);
        self.send_outgoing();
    }

    fn io(&mut self, now: u64) {
        for _ in 0..MAX_PASSES {
            let mut moved = self.drain_transport(now);
            moved |= self.run_commands(now);
            moved |= self.send_outgoing();
            if !moved {
                return;
            }
        }
        tracing::warn!("the network loop did not settle in {MAX_PASSES} passes");
    }

    /// Test port (`fault dropSocket`): the open sockets are closed from our side as by an error; the core
    /// reconnects by itself.
    pub fn drop_sockets(&mut self, now: u64) {
        self.now = now;
        let socks: Vec<Sock> = self.conns.keys().copied().collect();
        for sock in socks {
            if let Some(mut conn) = self.conns.remove(&sock) {
                conn.close();
                self.core.on_disconnected(now, sock, DisconnectWhy::Error);
            }
        }
        self.pump(now);
    }

    /// Test port (`fault silence {ms}`): the sockets are not read for `ms` (a probe of the liveness checks;
    /// what arrives meanwhile waits in the transport).
    pub fn silence(&mut self, now: u64, ms: u64) {
        self.silent_until = now + ms;
    }

    /// A frame the core did not make, straight to the current socket (`fault badMessage`).
    pub fn bad_message(&mut self, now: u64) {
        self.now = now;
        self.core.debug_bad_message();
        self.flush();
    }

    fn drain_transport(&mut self, now: u64) -> bool {
        if now < self.silent_until {
            return false;
        }
        let mut socks: Vec<Sock> = self.conns.keys().copied().collect();
        socks.sort_by_key(|s| s.0);
        let mut moved = false;
        for sock in socks {
            while let Some(ev) = self.conns.get_mut(&sock).and_then(|c| c.poll()) {
                moved = true;
                match ev {
                    TransportEvent::Opened => {
                        tracing::info!(sock = sock.0, "net.connect: the socket opened");
                        self.core.on_connected(now, sock);
                    }
                    TransportEvent::Message(bytes) => {
                        let _s = render::perf::slow::scope("net.on_bytes");
                        self.core.on_bytes(now, sock, &bytes);
                    }
                    TransportEvent::Closed(reason) => {
                        match &reason {
                            CloseReason::Local => {
                                tracing::info!(
                                    sock = sock.0,
                                    "net.close: the socket was closed here"
                                )
                            }
                            CloseReason::Remote { code, reason } => {
                                tracing::info!(sock = sock.0, code, %reason, "net.close: the server closed the socket");
                            }
                            CloseReason::Error(e) => {
                                tracing::info!(sock = sock.0, "net.close: the socket failed: {e}");
                            }
                        }
                        self.conns.remove(&sock);
                        self.core.on_disconnected(now, sock, map_close(&reason));
                        break;
                    }
                }
            }
        }
        moved
    }

    fn run_commands(&mut self, now: u64) -> bool {
        let mut moved = false;
        while let Some(cmd) = self.core.poll_socket_cmd() {
            moved = true;
            match cmd {
                SocketCmd::Open { sock, probe } => {
                    tracing::info!(sock = sock.0, probe, "net.connect: opening a socket");
                    let conn = self.transport.connect(&self.ws_url);
                    self.conns.insert(sock, conn);
                }
                SocketCmd::Close { sock } => {
                    // A socket that has not opened is closed the same way; its late events are gone.
                    if let Some(mut conn) = self.conns.remove(&sock) {
                        conn.close();
                        self.core.on_disconnected(now, sock, DisconnectWhy::Local);
                    }
                }
            }
        }
        moved
    }

    fn send_outgoing(&mut self) -> bool {
        let mut moved = false;
        while let Some((sock, bytes)) = self.core.poll_outgoing() {
            moved = true;
            // A frame for a socket that is gone is dropped: it belonged to that socket's login.
            if let Some(conn) = self.conns.get_mut(&sock) {
                conn.send(bytes);
            }
        }
        moved
    }

    fn collect_events(&mut self) {
        while let Some(ev) = self.core.poll_event() {
            log_event(&ev);
            match &ev {
                // The web page keeps no token (its session is a cookie); a native client stores it.
                ClientEvent::TokenIssued(t) if self.store.keeps_tokens() => {
                    self.store
                        .set(&token_key(self.core.nick()), &format_token(t));
                }
                ClientEvent::LoggedIn => self.store.set(NICK_KEY, self.core.nick()),
                ClientEvent::Rejected(
                    RejectReason::BadPassword | RejectReason::BadNick | RejectReason::NickTaken,
                ) => self.store.remove(&token_key(self.core.nick())),
                ClientEvent::CatalogNeeded(want) => self.start_catalog(*want),
                ClientEvent::Replica(_) => {
                    if let ClientEvent::Replica(m) = ev {
                        self.replica_msgs.push(m);
                    }
                    continue;
                }
                _ => {}
            }
            self.events.push_back(ev);
        }
    }

    // ------------------------------------------------------------ replica stream (M2)

    /// The replica messages that came since the last call, in order.
    pub fn take_replica_msgs(&mut self) -> Vec<client_core::ReplicaMsg> {
        std::mem::take(&mut self.replica_msgs)
    }

    /// Starts GET {http}/replica/{hash:016x} (once per hash at a time).
    pub fn fetch_replica_blob(&mut self, hash: u64) {
        if self.blob_jobs.iter().any(|(h, _)| *h == hash) {
            return;
        }
        let Some(base) = &self.http_base else {
            return;
        };
        let url = format!("{base}/replica/{hash:016x}");
        tracing::debug!(%url, "fetching a replica body");
        self.blob_jobs.push((hash, self.http.get(&url)));
    }

    /// The bodies that finished: Some(bytes), or None when the request failed (status or network).
    pub fn poll_replica_blobs(&mut self) -> Vec<(u64, Option<Vec<u8>>)> {
        let mut done = Vec::new();
        self.blob_jobs.retain_mut(|(hash, op)| match op.poll() {
            None => true,
            Some(res) => {
                let body = match res {
                    Ok(r) if r.is_success() => Some(r.body),
                    Ok(r) => {
                        tracing::warn!(status = r.status, "replica body request failed");
                        None
                    }
                    Err(e) => {
                        tracing::warn!("replica body request failed: {e}");
                        None
                    }
                };
                done.push((*hash, body));
                false
            }
        });
        done
    }

    // ------------------------------------------------------------ catalog

    fn start_catalog(&mut self, want: CatalogRef) {
        if self.catalog.as_ref().is_some_and(|j| j.want == want) {
            return;
        }
        if self.http_base.is_none() {
            self.catalog_fatal();
            return;
        }
        self.catalog = Some(CatalogJob {
            want,
            failures: 0,
            op: None,
            retry_at: Some(self.now),
        });
    }

    fn catalog_step(&mut self, now: u64) {
        let Some(mut job) = self.catalog.take() else {
            return;
        };
        if job.op.is_none() {
            if job.retry_at.is_none_or(|t| now < t) {
                self.catalog = Some(job);
                return;
            }
            let Some(base) = &self.http_base else {
                return self.catalog_fatal();
            };
            let url = format!("{base}/catalog/{:016x}", job.want.hash);
            tracing::debug!(%url, attempt = job.failures + 1, "fetching the catalog");
            job.op = Some(self.http.get(&url));
            job.retry_at = None;
        }
        let answer = job.op.as_mut().and_then(|op| op.poll());
        let Some(res) = answer else {
            self.catalog = Some(job);
            return;
        };
        job.op = None;
        match check_catalog(&job.want, res) {
            Ok(catalog) => {
                // Only for the game the core is attached to: a late answer for an earlier one is dropped.
                let current = self.core.attach_info().map(|a| a.catalog.hash);
                if current == Some(job.want.hash) {
                    self.core.set_catalog(catalog);
                }
            }
            Err(why) => {
                job.failures += 1;
                tracing::warn!(failures = job.failures, "catalog request failed: {why}");
                if job.failures >= CATALOG_ATTEMPTS {
                    return self.catalog_fatal();
                }
                let wait = CATALOG_RETRY_MS[(job.failures - 1) as usize];
                job.retry_at = Some(now + wait);
                self.catalog = Some(job);
            }
        }
    }

    fn catalog_fatal(&mut self) {
        self.catalog = None;
        self.fatal = Some(CATALOG_FATAL.to_string());
        self.events
            .push_back(ClientEvent::Fatal(CATALOG_FATAL.to_string()));
    }
}

/// `Rc<RefCell<NetClient>>` with short methods (net-client 4.1). The app creates it, keeps a copy for the
/// safety timer and the page events, and gives another to `Services`. A borrow lives only inside a method:
/// the page is single-threaded and the timer, the events and the frame never run inside one another.
#[derive(Clone)]
pub struct NetHandle(Rc<RefCell<NetClient>>);

impl NetHandle {
    pub fn new(client: NetClient) -> Self {
        NetHandle(Rc::new(RefCell::new(client)))
    }

    /// Frame, timer or event: one turn of the network.
    pub fn pump(&self, now_ms: u64) {
        self.0.borrow_mut().pump(now_ms);
    }

    /// The safety timer (NC-D9): one turn of the network if no frame did it for `idle_ms`. A handle that is in
    /// use (the frame is running) is left alone.
    pub fn pump_if_idle(&self, now_ms: u64, idle_ms: u64) {
        if let Ok(mut c) = self.0.try_borrow_mut()
            && now_ms.saturating_sub(c.now) >= idle_ms
        {
            c.pump(now_ms);
        }
    }

    /// A page event, then a turn of the network (4.8 driver 3).
    pub fn host_event(&self, ev: platform::HostEvent, now_ms: u64) {
        let mut c = self.0.borrow_mut();
        c.host_event(ev, now_ms);
        c.pump(now_ms);
    }

    pub fn connect(&self, now_ms: u64) {
        self.0.borrow_mut().connect(now_ms);
    }

    /// The core, for the player's actions (`order`, `lobby`, `chat`, ...) and for reading. What it queued
    /// for the network leaves at once, not at the next frame.
    pub fn with<R>(&self, f: impl FnOnce(&mut ClientCore) -> R) -> R {
        let mut c = self.0.borrow_mut();
        let r = f(&mut c.core);
        c.flush();
        r
    }

    /// Reads the core without touching it: nothing is sent, nothing is polled. `None` if the handle is in use
    /// (a frame or a page event is running).
    pub fn peek<R>(&self, f: impl FnOnce(&ClientCore) -> R) -> Option<R> {
        self.0.try_borrow().ok().map(|c| f(&c.core))
    }

    /// The whole shell, for what the core does not know (`phase()` with the shell's fatal, `login`, events).
    pub fn with_client<R>(&self, f: impl FnOnce(&mut NetClient) -> R) -> R {
        f(&mut self.0.borrow_mut())
    }

    pub fn poll_event(&self) -> Option<ClientEvent> {
        self.0.borrow_mut().poll_event()
    }
}
