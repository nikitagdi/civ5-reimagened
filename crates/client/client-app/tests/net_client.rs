//! `NetClient` on fake sockets and a fake HTTP client (net-client 4.1, 4.2, 4.11, 9): the address rule, the
//! token in the store, reconnecting with `resume`, probe sockets, the catalog with its checks and retries.
//! The fake gateway speaks protocol v3 for login and attach only; the game stream is `client-core`'s tests.

use std::cell::RefCell;
use std::collections::{HashSet, VecDeque};
use std::rc::Rc;

use client_app::net::{NICK_KEY, NetClient, NetHandle, stored_nick, token_key};
use client_core::{AddrContext, AddrError, ClientEvent, ClientPhase, ConnState, NetConfig};
use platform::{
    CloseReason, Connection, HostEvent, HttpClient, HttpError, HttpOp, HttpResponse, KeyValueStore,
    MemoryStore, Transport, TransportEvent,
};
use protocol::{
    AttachMode, Attached, Catalog, CatalogRef, ChatTo, ClientMsg, Codec, LobbyChoices, LobbyState,
    Login, PROTOCOL_VERSION, Postcard, RejectReason, Resume, ServerMsg, Session, Token,
};
use wire::{Encoder, Limits};

const PAGE: &str = "http://localhost:8480/";
const PASSWORD: &str = "pw-for-tests";
const TOKEN: Token = Token([7; 16]);
const TOKEN_HEX: &str = "07070707070707070707070707070707";

// ---------------------------------------------------------------- fake gateway and sockets

struct Fc {
    inbox: VecDeque<TransportEvent>,
    sent: Vec<ClientMsg>,
    closed: bool,
    /// Nothing reaches the client on this socket (a dead path).
    muted: bool,
    logged: bool,
}

#[derive(Default)]
struct Inner {
    conns: Vec<Fc>,
    urls: Vec<String>,
    /// `connect` opens the socket at once.
    auto_open: bool,
    /// The gateway attaches a logged-in client to a game.
    game: Option<CatalogRef>,
    logins: Vec<(usize, Login)>,
    known: HashSet<[u8; 16]>,
    /// `LoggedIn` carries no token (the login came from the HttpOnly session cookie of the page, protocol v9).
    cookie_login: bool,
}

type Shared = Rc<RefCell<Inner>>;

fn frame(msg: &ServerMsg) -> Vec<u8> {
    Encoder::default().frame_msg(msg).expect("encode")
}

impl Inner {
    fn push(&mut self, idx: usize, msg: &ServerMsg) {
        let c = &mut self.conns[idx];
        if !c.muted && !c.closed {
            c.inbox.push_back(TransportEvent::Message(frame(msg)));
        }
    }

    fn on_client(&mut self, idx: usize, msg: ClientMsg) {
        self.conns[idx].sent.push(msg.clone());
        match msg {
            ClientMsg::Hello { protocol, .. } => {
                let reply = if protocol == PROTOCOL_VERSION {
                    ServerMsg::Welcome {
                        session: 1,
                        build: "srv".into(),
                    }
                } else {
                    ServerMsg::Reject(RejectReason::VersionMismatch {
                        server: PROTOCOL_VERSION,
                    })
                };
                self.push(idx, &reply);
            }
            ClientMsg::Login(login) => {
                self.logins.push((idx, login.clone()));
                let known = login.token.is_some_and(|t| self.known.contains(&t.0));
                if !known && login.password != PASSWORD {
                    self.push(idx, &ServerMsg::Reject(RejectReason::BadPassword));
                    return;
                }
                self.known.insert(TOKEN.0);
                self.conns[idx].logged = true;
                self.push(
                    idx,
                    &ServerMsg::LoggedIn(Session {
                        token: (!self.cookie_login).then_some(TOKEN),
                        nick: "alice".into(),
                    }),
                );
                self.push(idx, &ServerMsg::Choices(LobbyChoices::default()));
                match self.game {
                    Some(catalog) => {
                        let replay = login.resume.is_some();
                        self.push(
                            idx,
                            &ServerMsg::Attached(Attached {
                                game: 7,
                                player: 1,
                                stream: 3,
                                mode: if replay {
                                    AttachMode::Replay
                                } else {
                                    AttachMode::Fresh
                                },
                                catalog,
                                last_cmd_id: 0,
                            }),
                        );
                    }
                    None => self.push(idx, &ServerMsg::LobbyState(LobbyState::default())),
                }
            }
            ClientMsg::Ping { seq, client_ms } => self.push(
                idx,
                &ServerMsg::Pong {
                    seq,
                    client_ms,
                    server_ms: 5_000_000,
                },
            ),
            _ => {}
        }
    }
}

struct FakeTransport(Shared);
struct FakeConn(Shared, usize);

impl Transport for FakeTransport {
    fn connect(&self, url: &str) -> Box<dyn Connection> {
        let mut s = self.0.borrow_mut();
        let idx = s.conns.len();
        let mut inbox = VecDeque::new();
        if s.auto_open {
            inbox.push_back(TransportEvent::Opened);
        }
        s.conns.push(Fc {
            inbox,
            sent: Vec::new(),
            closed: false,
            muted: false,
            logged: false,
        });
        s.urls.push(url.to_owned());
        Box::new(FakeConn(self.0.clone(), idx))
    }
}

impl Connection for FakeConn {
    fn send(&mut self, bytes: Vec<u8>) {
        let msg: ClientMsg = wire::decode(&bytes, &Limits::GATEWAY).expect("an enc 0 frame");
        let mut s = self.0.borrow_mut();
        if !s.conns[self.1].closed {
            s.on_client(self.1, msg);
        }
    }

    fn poll(&mut self) -> Option<TransportEvent> {
        self.0.borrow_mut().conns[self.1].inbox.pop_front()
    }

    fn close(&mut self) {
        let mut s = self.0.borrow_mut();
        let c = &mut s.conns[self.1];
        if !c.closed {
            c.closed = true;
            c.inbox.clear();
            c.inbox
                .push_back(TransportEvent::Closed(CloseReason::Local));
        }
    }
}

#[derive(Default)]
struct HttpInner {
    script: VecDeque<Result<HttpResponse, HttpError>>,
    urls: Vec<String>,
}

#[derive(Clone, Default)]
struct FakeHttp(Rc<RefCell<HttpInner>>);

struct Done(Option<Result<HttpResponse, HttpError>>);

impl HttpOp for Done {
    fn poll(&mut self) -> Option<Result<HttpResponse, HttpError>> {
        self.0.take()
    }
}

impl HttpClient for FakeHttp {
    fn get(&self, url: &str) -> Box<dyn HttpOp> {
        let mut h = self.0.borrow_mut();
        h.urls.push(url.to_owned());
        let r = h
            .script
            .pop_front()
            .unwrap_or_else(|| Err(HttpError::Network("nothing scripted".into())));
        Box::new(Done(Some(r)))
    }
}

fn catalog_body() -> Vec<u8> {
    let mut b = b"C5CT".to_vec();
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend(Postcard.encode(&Catalog::default()).expect("encode"));
    b
}

fn ok(body: Vec<u8>) -> Result<HttpResponse, HttpError> {
    Ok(HttpResponse { status: 200, body })
}

// ---------------------------------------------------------------- harness

struct H {
    net: Shared,
    http: FakeHttp,
    store: MemoryStore,
    client: NetClient,
    now: u64,
    /// When each socket was asked for (to the 50 ms of the steps).
    opened_at: Vec<u64>,
    /// When each catalog request went out.
    requested_at: Vec<u64>,
}

fn client_on(net: &Shared, http: &FakeHttp, store: &MemoryStore) -> NetClient {
    NetClient::new(
        NetConfig::new("test-build", "alice"),
        AddrContext::Page { url: PAGE },
        None,
        Box::new(FakeTransport(net.clone())),
        Box::new(http.clone()),
        Box::new(store.clone()),
    )
    .expect("a local page")
}

fn harness(game: Option<CatalogRef>) -> H {
    let net: Shared = Rc::new(RefCell::new(Inner {
        auto_open: true,
        game,
        ..Inner::default()
    }));
    let http = FakeHttp::default();
    let store = MemoryStore::new();
    let client = client_on(&net, &http, &store);
    H {
        net,
        http,
        store,
        client,
        now: 1_000,
        opened_at: Vec::new(),
        requested_at: Vec::new(),
    }
}

impl H {
    /// Time goes on in 50 ms steps with a pump at each, as frames and the safety timer would.
    fn run(&mut self, ms: u64) {
        let end = self.now + ms;
        while self.now < end {
            self.now = (self.now + 50).min(end);
            self.client.pump(self.now);
            self.note_sockets();
        }
    }

    fn note_sockets(&mut self) {
        while self.opened_at.len() < self.sockets() {
            self.opened_at.push(self.now);
        }
        while self.requested_at.len() < self.catalog_requests() {
            self.requested_at.push(self.now);
        }
    }

    /// The player types the password: the first socket opens with it (there was no token to go on).
    fn login(&mut self) {
        self.client.login("alice", PASSWORD, self.now);
        self.note_sockets();
        self.run(200);
    }

    fn events(&mut self) -> Vec<ClientEvent> {
        std::iter::from_fn(|| self.client.poll_event()).collect()
    }

    fn sockets(&self) -> usize {
        self.net.borrow().conns.len()
    }

    fn catalog_requests(&self) -> usize {
        self.http.0.borrow().urls.len()
    }
}

// ---------------------------------------------------------------- the address

#[test]
fn the_address_rule_is_applied_when_the_client_is_made() {
    let make = |ctx: AddrContext<'_>, param: Option<&str>| {
        NetClient::new(
            NetConfig::new("b", "alice"),
            ctx,
            param,
            Box::new(FakeTransport(Rc::default())),
            Box::new(FakeHttp::default()),
            Box::new(MemoryStore::new()),
        )
    };
    // A public https page takes only its own address; a LAN `?server=` is dropped with a warning.
    let c = make(
        AddrContext::Page {
            url: "https://civ.example.org/play",
        },
        Some("ws://192.168.1.10:8480/ws"),
    )
    .unwrap();
    assert_eq!(c.url(), "wss://civ.example.org/ws");
    assert!(c.address_warning().is_some());
    // A developer's local page may name a LAN server.
    let c = make(
        AddrContext::Page { url: PAGE },
        Some("ws://192.168.1.10:8480/ws"),
    )
    .unwrap();
    assert_eq!(c.url(), "ws://192.168.1.10:8480/ws");
    assert!(c.address_warning().is_none());
    // Plain http on a public address: no login at all. A desktop needs `--server`.
    assert_eq!(
        make(
            AddrContext::Page {
                url: "http://civ.example.org/"
            },
            None
        )
        .err(),
        Some(AddrError::PlainHttp)
    );
    assert_eq!(
        make(AddrContext::Desktop, None).err(),
        Some(AddrError::NoServer)
    );
    assert!(make(AddrContext::Desktop, Some("ws://127.0.0.1:8480/ws")).is_ok());
}

// ---------------------------------------------------------------- token and nick

#[test]
fn login_stores_the_token_and_the_nick_and_a_reloaded_page_logs_in_with_the_token() {
    let mut h = harness(None);
    h.login();
    assert_eq!(h.client.core().token(), Some(TOKEN));
    assert_eq!(h.store.get(&token_key("alice")).as_deref(), Some(TOKEN_HEX));
    assert_eq!(stored_nick(&h.store).as_deref(), Some("alice"));
    assert_eq!(h.store.get(NICK_KEY).as_deref(), Some("alice"));
    let events = h.events();
    assert!(events.iter().any(|e| matches!(e, ClientEvent::LoggedIn)));
    // No secret reaches the log through the events' `Debug`.
    let dump = format!("{events:?}");
    assert!(
        !dump.contains(PASSWORD) && !dump.contains(TOKEN_HEX),
        "{dump}"
    );
    assert_eq!(h.client.phase(), ClientPhase::Lobby);

    // The page is reloaded: a new client over the same store; no password is typed.
    let net: Shared = Rc::new(RefCell::new(Inner {
        auto_open: true,
        known: [TOKEN.0].into(),
        ..Inner::default()
    }));
    let mut c = client_on(&net, &h.http, &h.store);
    c.connect(5_000);
    c.pump(5_100);
    let logins = net.borrow().logins.clone();
    assert_eq!(logins.len(), 1);
    assert_eq!(logins[0].1.token, Some(TOKEN));
    assert_eq!(logins[0].1.password, "");
    assert_eq!(c.phase(), ClientPhase::Lobby, "in without the password");
}

#[test]
fn a_refused_password_erases_the_stored_token() {
    let mut h = harness(None);
    h.store.set(&token_key("alice"), TOKEN_HEX);
    h.client = client_on(&h.net, &h.http, &h.store);
    h.client.connect(h.now);
    h.client.login("alice", "wrong", h.now);
    h.run(300);
    assert!(
        h.events()
            .iter()
            .any(|e| matches!(e, ClientEvent::Rejected(RejectReason::BadPassword)))
    );
    assert_eq!(h.store.get(&token_key("alice")), None);
    assert_eq!(h.client.phase(), ClientPhase::NeedLogin);
}

#[test]
fn a_damaged_token_is_forgotten_and_the_token_of_another_nick_is_not_used() {
    let store = MemoryStore::new();
    store.set(&token_key("alice"), "not a token");
    store.set(&token_key("bob"), TOKEN_HEX);
    let net: Shared = Rc::new(RefCell::new(Inner {
        auto_open: true,
        ..Inner::default()
    }));
    // The socket never opens, so nothing is refused while the tokens are looked at.
    net.borrow_mut().auto_open = false;
    let mut c = client_on(&net, &FakeHttp::default(), &store);
    assert_eq!(c.core().token(), None);
    assert_eq!(store.get(&token_key("alice")), None, "removed");
    // Logging in as bob takes bob's token; going back to alice has none.
    c.login("bob", "", 100);
    assert_eq!(c.core().token(), Some(Token([7; 16])));
    c.login("alice", "", 200);
    assert_eq!(c.core().token(), None);
}

// ---------------------------------------------------------------- the web page keeps no token (protocol v9)

/// The store of the web page: it keeps the nick, never a device token (`KeyValueStore::keeps_tokens`).
#[derive(Clone, Default)]
struct WebStore(MemoryStore);

impl KeyValueStore for WebStore {
    fn get(&self, key: &str) -> Option<String> {
        self.0.get(key)
    }
    fn set(&self, key: &str, value: &str) {
        self.0.set(key, value);
    }
    fn remove(&self, key: &str) {
        self.0.remove(key);
    }
    fn keeps_tokens(&self) -> bool {
        false
    }
}

fn web_client(net: &Shared, store: &WebStore) -> NetClient {
    NetClient::new(
        NetConfig::new("test-build", "alice"),
        AddrContext::Page { url: PAGE },
        None,
        Box::new(FakeTransport(net.clone())),
        Box::new(FakeHttp::default()),
        Box::new(store.clone()),
    )
    .expect("a local page")
}

#[test]
fn a_cookie_login_without_a_token_stores_nothing_but_the_nick() {
    let net: Shared = Rc::new(RefCell::new(Inner {
        auto_open: true,
        cookie_login: true,
        // The cookie is what the gateway accepts: a login with an empty password and no token.
        ..Inner::default()
    }));
    let store = WebStore::default();
    let mut c = web_client(&net, &store);
    c.login("alice", PASSWORD, 1_000);
    for t in (1_050..1_400).step_by(50) {
        c.pump(t);
    }
    assert_eq!(c.phase(), ClientPhase::Lobby);
    assert_eq!(c.core().token(), None, "the page holds no token");
    assert_eq!(store.get(&token_key("alice")), None);
    assert_eq!(store.0.len(), 1, "only the nick: {:?}", store.get(NICK_KEY));
    assert_eq!(stored_nick(&store).as_deref(), Some("alice"));
    let events: Vec<_> = std::iter::from_fn(|| c.poll_event()).collect();
    assert!(events.iter().any(|e| matches!(e, ClientEvent::LoggedIn)));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, ClientEvent::TokenIssued(_)))
    );
}

#[test]
fn a_web_store_neither_reads_nor_writes_a_token_even_when_the_server_sends_one() {
    let net: Shared = Rc::new(RefCell::new(Inner {
        auto_open: true,
        known: [TOKEN.0].into(),
        ..Inner::default()
    }));
    let store = WebStore::default();
    // An old token that the migration has not removed yet: the wasm does not use it.
    store.set(&token_key("alice"), TOKEN_HEX);
    let mut c = web_client(&net, &store);
    assert_eq!(c.core().token(), None);
    store.remove(&token_key("alice"));
    c.login("alice", PASSWORD, 1_000);
    for t in (1_050..1_400).step_by(50) {
        c.pump(t);
    }
    assert_eq!(net.borrow().logins[0].1.token, None);
    // The server answered with a token (a password login): the core has it, the store is not written.
    assert_eq!(c.core().token(), Some(TOKEN));
    assert_eq!(store.0.len(), 1, "the nick only");
    assert_eq!(store.get(&token_key("alice")), None);
}

// ---------------------------------------------------------------- reconnecting

#[test]
fn a_dropped_socket_comes_back_with_token_and_resume() {
    let mut h = harness(Some(CatalogRef { hash: 1, len: 1 }));
    h.login();
    assert_eq!(h.client.core().conn(), ConnState::Online);
    let first_logins = h.net.borrow().logins.len();
    assert!(h.net.borrow().logins[0].1.resume.is_none());

    // The connection drops (the server restarts, 1012) in the middle of play: the line was alive a moment
    // ago, so the core asks for a fresh socket at once (NC-D17) and logs in on it with token and resume.
    h.net.borrow_mut().conns[0]
        .inbox
        .push_back(TransportEvent::Closed(CloseReason::Remote {
            code: 1012,
            reason: String::new(),
        }));
    h.run(300);
    assert_eq!(h.sockets(), 2, "one new socket");
    let n = h.net.borrow();
    assert_eq!(n.logins.len(), first_logins + 1);
    let (idx, login) = n.logins.last().unwrap();
    assert_eq!(*idx, 1);
    assert_eq!(login.token, Some(TOKEN));
    assert_eq!(
        login.resume,
        Some(Resume {
            stream: 3,
            last_seq: 0,
            basis: None
        })
    );
    drop(n);
    assert_eq!(h.client.core().conn(), ConnState::Online);
    assert!(h.events().iter().any(|e| matches!(
        e,
        ClientEvent::Attached {
            mode: AttachMode::Replay,
            ..
        }
    )));
}

#[test]
fn silence_opens_probe_sockets_and_the_first_to_log_in_replaces_the_old_one() {
    let mut h = harness(Some(CatalogRef { hash: 1, len: 1 }));
    h.login();
    assert_eq!(h.client.core().conn(), ConnState::Online);
    // The path dies: the old socket neither answers nor closes.
    h.net.borrow_mut().conns[0].muted = true;
    h.run(3_500);
    assert_eq!(h.sockets(), 1, "no probe before 4 s of silence");
    h.run(2_500);
    assert!(h.sockets() >= 2, "probe sockets started");
    let n = h.net.borrow();
    assert!(n.conns[0].closed, "the old socket was closed by the shell");
    let current = n.conns.iter().rposition(|c| c.logged).unwrap();
    assert!(current >= 1, "a probe logged in");
    // At most two probes were alive at a time.
    let alive = n.conns.iter().filter(|c| !c.closed).count();
    assert!(alive <= 2, "{alive} sockets alive");
    drop(n);
    assert_eq!(h.client.core().conn(), ConnState::Online);
}

#[test]
fn probe_sockets_that_do_not_open_are_closed_after_a_second_and_never_more_than_two_live() {
    let mut h = harness(Some(CatalogRef { hash: 1, len: 1 }));
    h.login();
    h.net.borrow_mut().conns[0].muted = true;
    // Every new socket hangs in its connect.
    h.net.borrow_mut().auto_open = false;
    h.run(8_000);
    let n = h.net.borrow();
    assert!(n.conns.len() >= 5, "{} sockets tried", n.conns.len());
    assert!(
        n.conns.iter().skip(1).filter(|c| c.closed).count() >= 3,
        "the probes that did not open were closed by the shell"
    );
    let alive_probes = n.conns.iter().skip(1).filter(|c| !c.closed).count();
    assert!(alive_probes <= 2, "{alive_probes} probes alive");
    // Each probe lived about `probe_attempt_ms` (1 s) before it was closed.
    drop(n);
    let spawn_gaps: Vec<u64> = h.opened_at.windows(2).map(|w| w[1] - w[0]).collect();
    assert!(
        spawn_gaps.iter().skip(1).all(|g| *g >= 450),
        "{spawn_gaps:?}"
    );
}

#[test]
fn page_hide_closes_a_socket_that_has_not_opened() {
    let mut h = harness(None);
    h.net.borrow_mut().auto_open = false;
    h.client.connect(h.now);
    h.run(100);
    assert_eq!(h.sockets(), 1);
    assert!(!h.net.borrow().conns[0].closed);
    h.client.host_event(HostEvent::PageHide, h.now);
    h.run(100);
    assert!(h.net.borrow().conns[0].closed);
    assert_eq!(h.client.socket_count(), 0);
}

#[test]
fn page_events_reach_the_core() {
    let mut h = harness(Some(CatalogRef { hash: 1, len: 1 }));
    h.login();
    h.client.host_event(HostEvent::Offline, h.now);
    assert_eq!(h.client.core().conn(), ConnState::Offline);
    assert!(!h.client.core().orders_enabled());
    h.client.host_event(HostEvent::Online, h.now);
    h.run(2_000);
    assert_eq!(h.client.core().conn(), ConnState::Online);
}

// ---------------------------------------------------------------- the handle

#[test]
fn what_the_core_queues_through_the_handle_leaves_at_once() {
    let mut h = harness(Some(CatalogRef { hash: 1, len: 1 }));
    h.login();
    let sent_before = h.net.borrow().conns[0].sent.len();
    let handle = NetHandle::new(h.client);
    // No pump between the call and the look: `with` flushes.
    let r = handle.with(|core| core.chat(ChatTo::All, "hello"));
    assert_eq!(r, Ok(()));
    let n = h.net.borrow();
    let chats: Vec<_> = n.conns[0].sent[sent_before..]
        .iter()
        .filter(|m| matches!(m, ClientMsg::Chat(_)))
        .collect();
    assert_eq!(chats.len(), 1);
}

// ---------------------------------------------------------------- the catalog

fn attached_with_catalog(body: &[u8]) -> H {
    let cref = CatalogRef {
        hash: protocol::xxh64(0, body),
        len: body.len() as u32,
    };
    let h = harness(Some(cref));
    h.http.0.borrow_mut().script.push_back(ok(body.to_vec()));
    h
}

#[test]
fn the_catalog_is_fetched_by_its_hash_and_given_to_the_core() {
    let mut h = attached_with_catalog(&catalog_body());
    h.login();
    assert_eq!(h.http.0.borrow().urls.len(), 1);
    let want = format!(
        "http://localhost:8480/catalog/{:016x}",
        protocol::xxh64(0, &catalog_body())
    );
    assert_eq!(h.http.0.borrow().urls[0], want);
    assert!(h.client.core().catalog().is_some());
    assert!(
        h.events()
            .iter()
            .any(|e| matches!(e, ClientEvent::CatalogNeeded(_)))
    );
    assert_eq!(h.client.phase(), ClientPhase::Loading, "no keyframe yet");
}

#[test]
fn a_failed_catalog_request_is_repeated_after_1_2_and_4_seconds() {
    let good = catalog_body();
    let mut h = attached_with_catalog(&good);
    // Network error, HTTP 404, a body that is not the catalog; then the right one.
    {
        let mut s = h.http.0.borrow_mut();
        s.script.clear();
        s.script.push_back(Err(HttpError::Network("reset".into())));
        s.script.push_back(Ok(HttpResponse {
            status: 404,
            body: vec![],
        }));
        let mut bad = good.clone();
        *bad.last_mut().unwrap() ^= 0x55;
        s.script.push_back(ok(bad));
        s.script.push_back(ok(good.clone()));
    }
    h.login();
    assert_eq!(h.catalog_requests(), 1, "the first request at once");
    assert!(h.client.core().catalog().is_none());
    h.run(10_000);
    assert_eq!(h.catalog_requests(), 4);
    assert!(
        h.client.core().catalog().is_some(),
        "the fourth try was right"
    );
    // Pauses between the requests: 1 s, 2 s, 4 s (to the 50 ms of the steps).
    let gaps: Vec<u64> = h.requested_at.windows(2).map(|w| w[1] - w[0]).collect();
    assert_eq!(gaps.len(), 3, "{gaps:?}");
    for (gap, want) in gaps.iter().zip([1_000, 2_000, 4_000]) {
        assert!(*gap >= want && *gap <= want + 100, "{gaps:?}");
    }
    assert!(
        !h.events()
            .iter()
            .any(|e| matches!(e, ClientEvent::Fatal(_)))
    );
}

#[test]
fn five_failures_of_the_catalog_are_fatal() {
    let body = catalog_body();
    let mut h = attached_with_catalog(&body);
    h.http.0.borrow_mut().script.clear();
    h.login();
    h.run(30_000);
    assert_eq!(h.catalog_requests(), 5, "five tries, no sixth");
    let fatal: Vec<_> = h
        .events()
        .into_iter()
        .filter_map(|e| match e {
            ClientEvent::Fatal(t) => Some(t),
            _ => None,
        })
        .collect();
    assert_eq!(fatal, ["Cannot load the game catalog"]);
    assert_eq!(
        h.client.phase(),
        ClientPhase::Fatal("Cannot load the game catalog".into())
    );
    assert!(h.client.core().catalog().is_none());
}

#[test]
fn a_catalog_without_the_prefix_is_not_accepted_even_with_a_right_hash() {
    let mut raw = b"XXXX".to_vec();
    raw.extend_from_slice(&1u16.to_le_bytes());
    raw.extend(Postcard.encode(&Catalog::default()).unwrap());
    let mut h = attached_with_catalog(&raw);
    h.login();
    assert!(h.client.core().catalog().is_none());
    assert_eq!(h.catalog_requests(), 1);
}

#[test]
fn a_catalog_of_another_version_is_not_accepted_even_with_a_right_hash() {
    // The version is little-endian: 1 written big-endian reads 256.
    let mut raw = b"C5CT".to_vec();
    raw.extend_from_slice(&1u16.to_be_bytes());
    raw.extend(Postcard.encode(&Catalog::default()).unwrap());
    let mut h = attached_with_catalog(&raw);
    h.login();
    assert!(h.client.core().catalog().is_none());
    assert_eq!(h.catalog_requests(), 1);
    // The body of the server's own version is read (the helper writes it as `wire` does).
    let mut ok_body = client_core::catalog_format::catalog_header().to_vec();
    ok_body.extend(Postcard.encode(&Catalog::default()).unwrap());
    let mut h = attached_with_catalog(&ok_body);
    h.login();
    assert!(h.client.core().catalog().is_some());
}

#[test]
fn a_catalog_longer_than_announced_is_not_accepted_even_with_a_right_hash() {
    let body = catalog_body();
    let cref = CatalogRef {
        hash: protocol::xxh64(0, &body),
        len: body.len() as u32 - 1,
    };
    let mut h = harness(Some(cref));
    h.http.0.borrow_mut().script.push_back(ok(body));
    h.login();
    assert!(h.client.core().catalog().is_none());
    assert_eq!(h.catalog_requests(), 1);
}
