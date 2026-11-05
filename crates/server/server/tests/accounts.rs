//! Accounts: register over HTTP, log in over the WebSocket with the token or the
//! password, replace on login, legacy records, the registration code, the limits, the admin reset.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use protocol::{
    ClientMsg, Codec, DetachReason, Login, PROTOCOL_VERSION, Postcard, RejectReason, ServerMsg,
    Token,
};
use serde_json::{Value, json};
use server::identity::{Secret, token_from_hex, token_to_hex};
use server::session::limits::LimitsConfig;
use server::{ServerConfig, Shared};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};
use wire::{Encoder, Limits};

const CODE: &str = "registration-code";
const PW: &str = "correct horse battery";

struct TestServer {
    addr: SocketAddr,
    admin: SocketAddr,
    shared: Arc<Shared>,
    _rt: tokio::runtime::Runtime,
}

impl TestServer {
    fn start(config: ServerConfig) -> Self {
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
        let addr = listener.local_addr().unwrap();
        let admin_addr = admin.local_addr().unwrap();
        let shared = Shared::new(config).unwrap();
        rt.spawn(server::serve(listener, shared.clone()));
        rt.spawn(server::serve_admin(admin, shared.clone()));
        Self {
            addr,
            admin: admin_addr,
            shared,
            _rt: rt,
        }
    }
}

/// A server as deployed: no auto_register, the shared password is the registration code.
fn coded() -> ServerConfig {
    ServerConfig {
        password: Some(Secret::new(CODE)),
        auto_register: false,
        admin_token: Some(Secret::new("adm")),
        limits: LimitsConfig {
            // The real 1 s delay would slow every wrong-password test.
            password_delay: Duration::from_millis(5),
            ..LimitsConfig::default()
        },
        ..ServerConfig::default()
    }
}

fn open_registration() -> ServerConfig {
    ServerConfig {
        password: None,
        open_access: true,
        ..coded()
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("civ5-accounts-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// ------------------------------------------------------------------ HTTP

struct Resp {
    status: u16,
    /// Lowercased header block.
    head: String,
    /// The `Set-Cookie` header lines as sent.
    set_cookies: Vec<String>,
    body: String,
}

impl Resp {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("{e}: {:?}", self.body))
    }

    /// The whole `civ5_session` Set-Cookie line.
    fn session_cookie(&self) -> &str {
        self.set_cookies
            .iter()
            .find(|c| c.starts_with("civ5_session="))
            .unwrap_or_else(|| panic!("no session cookie: {:?}", self.head))
    }

    /// The device token inside the session cookie (the JSON body has none).
    fn token(&self) -> Token {
        assert!(self.json().get("token").is_none(), "{}", self.body);
        let value = self
            .session_cookie()
            .strip_prefix("civ5_session=")
            .unwrap()
            .split(';')
            .next()
            .unwrap();
        token_from_hex(value.rsplit_once('.').unwrap().1).unwrap()
    }

    /// `name=value` of the cookie, for a `Cookie` request header.
    fn cookie_pair(&self) -> String {
        self.session_cookie().split(';').next().unwrap().to_owned()
    }
}

fn http(addr: SocketAddr, method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> Resp {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    write!(s, "{method} {path} HTTP/1.0\r\nHost: {addr}\r\n").unwrap();
    for (k, v) in headers {
        write!(s, "{k}: {v}\r\n").unwrap();
    }
    write!(s, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
    // A refused body may close the socket before all of it is written.
    let _ = s.write_all(body);
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("end of headers");
    let original = String::from_utf8_lossy(&raw[..split]).into_owned();
    let head = original.to_ascii_lowercase();
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    let set_cookies = original
        .lines()
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("set-cookie")
                .then(|| v.trim().to_owned())
        })
        .collect();
    Resp {
        status,
        head,
        set_cookies,
        body: String::from_utf8_lossy(&raw[split + 4..]).into_owned(),
    }
}

fn post_json(srv: &TestServer, path: &str, body: Value) -> Resp {
    http(
        srv.addr,
        "POST",
        path,
        &[("Content-Type", "application/json")],
        body.to_string().as_bytes(),
    )
}

fn register(srv: &TestServer, nick: &str, password: &str, code: Option<&str>) -> Resp {
    let mut b = json!({ "username": nick, "password": password });
    if let Some(c) = code {
        b["code"] = c.into();
    }
    post_json(srv, "/api/auth/register", b)
}

fn login_http(srv: &TestServer, nick: &str, password: &str) -> Resp {
    post_json(
        srv,
        "/api/auth/login",
        json!({ "username": nick, "password": password }),
    )
}

fn admin(srv: &TestServer, method: &str, path: &str) -> Resp {
    http(srv.admin, method, path, &[("X-Admin-Token", "adm")], b"")
}

// ------------------------------------------------------------------ WebSocket

type Ws = WebSocket<MaybeTlsStream<TcpStream>>;

fn connect(srv: &TestServer) -> Ws {
    let (mut ws, _) = tungstenite::connect(format!("ws://{}/ws", srv.addr)).unwrap();
    set_timeout(&mut ws, Duration::from_secs(5));
    ws
}

fn set_timeout(ws: &mut Ws, d: Duration) {
    if let MaybeTlsStream::Plain(s) = ws.get_mut() {
        s.set_read_timeout(Some(d)).unwrap();
    }
}

fn send(ws: &mut Ws, msg: &ClientMsg) {
    let body = Postcard.encode(msg).unwrap();
    ws.send(Message::binary(Encoder::plain(&body))).unwrap();
}

fn recv(ws: &mut Ws) -> ServerMsg {
    match ws.read().unwrap() {
        Message::Binary(b) => wire::decode(&b, &Limits::CLIENT).unwrap(),
        other => panic!("expected a binary message, got {other:?}"),
    }
}

fn assert_closed(ws: &mut Ws) {
    loop {
        match ws.read() {
            Ok(Message::Close(_)) | Err(_) => return,
            Ok(_) => {}
        }
    }
}

fn hello_login(ws: &mut Ws, nick: &str, password: &str, token: Option<Token>) {
    send(
        ws,
        &ClientMsg::Hello {
            protocol: PROTOCOL_VERSION,
            build: "test".into(),
            nick: nick.into(),
        },
    );
    send(
        ws,
        &ClientMsg::Login(Login {
            password: password.into(),
            token,
            resume: None,
        }),
    );
}

/// Logs in and reads up to the lobby. Returns the token of the `LoggedIn`.
fn log_in(ws: &mut Ws, nick: &str, password: &str, token: Option<Token>) -> Token {
    hello_login(ws, nick, password, token);
    assert!(matches!(recv(ws), ServerMsg::Welcome { .. }));
    let token = match recv(ws) {
        ServerMsg::LoggedIn(s) => {
            assert_eq!(s.nick, nick);
            s.token.expect("the login carries the token")
        }
        other => panic!("expected LoggedIn, got {other:?}"),
    };
    assert!(matches!(recv(ws), ServerMsg::Choices(_)));
    assert!(matches!(recv(ws), ServerMsg::LobbyState(_)));
    token
}

fn log_in_rejected(ws: &mut Ws, nick: &str, password: &str, token: Option<Token>) -> RejectReason {
    hello_login(ws, nick, password, token);
    assert!(matches!(recv(ws), ServerMsg::Welcome { .. }));
    match recv(ws) {
        ServerMsg::Reject(r) => {
            assert_closed(ws);
            r
        }
        other => panic!("expected Reject, got {other:?}"),
    }
}

fn ping(ws: &mut Ws, seq: u32) {
    send(ws, &ClientMsg::Ping { seq, client_ms: 5 });
    match recv(ws) {
        ServerMsg::Pong { seq: s, .. } => assert_eq!(s, seq),
        other => panic!("expected Pong, got {other:?}"),
    }
}

fn assert_replaced(ws: &mut Ws) {
    assert_eq!(
        recv(ws),
        ServerMsg::Detached {
            reason: DetachReason::Replaced
        }
    );
    assert_closed(ws);
}

// ------------------------------------------------------------------ tests

#[test]
fn register_over_http_then_log_in_over_the_websocket_with_the_token() {
    let srv = TestServer::start(coded());
    let r = register(&srv, "Alice", PW, Some(CODE));
    assert_eq!(r.status, 200, "{}", r.body);
    assert!(r.head.contains("cache-control: no-store"), "{}", r.head);
    assert_eq!(r.json()["nick"], "Alice");
    // The token is in the HttpOnly cookie, not in the body.
    let token = r.token();
    assert_eq!(token_to_hex(&token).len(), 32);

    // The wasm client logs in with the stored token and an empty password, as it always did.
    let mut ws = connect(&srv);
    assert_eq!(log_in(&mut ws, "Alice", "", Some(token)), token);
    ping(&mut ws, 1);
    // The nick is case-insensitive and the answer carries the registered one.
    drop(ws);
    let mut ws = connect(&srv);
    hello_login(&mut ws, "ALICE", "", Some(token));
    assert!(matches!(recv(&mut ws), ServerMsg::Welcome { .. }));
    match recv(&mut ws) {
        ServerMsg::LoggedIn(s) => assert_eq!((s.nick.as_str(), s.token), ("Alice", Some(token))),
        other => panic!("{other:?}"),
    }
}

#[test]
fn websocket_password_login_checks_the_account_hash() {
    let srv = TestServer::start(coded());
    let t1 = register(&srv, "alice", PW, Some(CODE)).token();

    let mut ws = connect(&srv);
    let t2 = log_in(&mut ws, "alice", PW, None);
    // A password login is a new device with its own token.
    assert_ne!(t1, t2);
    assert_eq!(srv.shared.identity.device_count("alice"), 2);
    drop(ws);
    let mut ws = connect(&srv);
    assert_eq!(log_in(&mut ws, "alice", "", Some(t2)), t2);
}

#[test]
fn a_wrong_password_is_bad_password_and_so_is_an_unknown_nick() {
    let srv = TestServer::start(coded());
    register(&srv, "alice", PW, Some(CODE));

    let started = Instant::now();
    let mut ws = connect(&srv);
    assert_eq!(
        log_in_rejected(&mut ws, "alice", "not the password", None),
        RejectReason::BadPassword
    );
    // The registration code is no login password, and there is no auto_register here.
    let mut ws = connect(&srv);
    assert_eq!(
        log_in_rejected(&mut ws, "alice", CODE, None),
        RejectReason::BadPassword
    );
    let mut ws = connect(&srv);
    assert_eq!(
        log_in_rejected(&mut ws, "nobody", CODE, None),
        RejectReason::BadPassword
    );
    let mut ws = connect(&srv);
    assert_eq!(
        log_in_rejected(&mut ws, "nobody", PW, None),
        RejectReason::BadPassword
    );
    assert_eq!(srv.shared.identity.len(), 1, "nothing was created");
    assert!(started.elapsed() < Duration::from_secs(10));
    // A token of another account does not open this one.
    let bob = register(&srv, "bob", PW, Some(CODE)).token();
    let mut ws = connect(&srv);
    assert_eq!(
        log_in_rejected(&mut ws, "alice", "", Some(bob)),
        RejectReason::BadPassword
    );
}

#[test]
fn a_second_login_replaces_the_first_by_password_and_by_token() {
    let srv = TestServer::start(coded());
    let token = register(&srv, "bob", PW, Some(CODE)).token();

    let mut first = connect(&srv);
    log_in(&mut first, "bob", "", Some(token));
    // A password login (another browser) replaces it.
    let mut second = connect(&srv);
    let second_token = log_in(&mut second, "bob", PW, None);
    assert_replaced(&mut first);
    ping(&mut second, 1);

    // A token login replaces that one in turn.
    let mut third = connect(&srv);
    assert_eq!(
        log_in(&mut third, "bob", "", Some(second_token)),
        second_token
    );
    assert_replaced(&mut second);
    ping(&mut third, 1);
    assert_eq!(srv.shared.identity.live_count(), 1);
}

#[test]
fn a_first_client_that_never_reads_cannot_hold_up_the_new_login() {
    let srv = TestServer::start(coded());
    let token = register(&srv, "bob", PW, Some(CODE)).token();

    let mut hung = connect(&srv);
    hello_login(&mut hung, "bob", "", Some(token));
    // It reads nothing at all: not even Welcome and LoggedIn.
    let started = Instant::now();
    let mut fresh = connect(&srv);
    log_in(&mut fresh, "bob", "", Some(token));
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    ping(&mut fresh, 1);
    // The hung session is gone from the table at once, and its teardown does not touch the new one.
    assert_eq!(srv.shared.identity.live_count(), 1);
    std::thread::sleep(Duration::from_millis(300));
    ping(&mut fresh, 2);
    assert_eq!(srv.shared.identity.live_count(), 1);
    // When it finally reads, it hears why.
    assert!(matches!(recv(&mut hung), ServerMsg::Welcome { .. }));
    assert!(matches!(recv(&mut hung), ServerMsg::LoggedIn(_)));
    loop {
        match recv(&mut hung) {
            ServerMsg::Detached {
                reason: DetachReason::Replaced,
            } => break,
            ServerMsg::Choices(_) | ServerMsg::LobbyState(_) => {}
            other => panic!("{other:?}"),
        }
    }
    assert_closed(&mut hung);
}

#[test]
fn a_legacy_record_keeps_its_token_and_its_seat() {
    let dir = temp_dir("legacy");
    let old = Token([0x5a; 16]);
    std::fs::write(
        dir.join("identities.json"),
        format!(
            r#"{{"format": 1, "records": [{{"nick": "Veteran", "token": "{}", "last_seen_ms": 1}}]}}"#,
            token_to_hex(&old)
        ),
    )
    .unwrap();
    let srv = TestServer::start(ServerConfig {
        state_dir: Some(dir.clone()),
        ..coded()
    });
    assert!(dir.join("accounts.json").exists());
    assert!(dir.join("identities.json.migrated").exists());
    assert!(!dir.join("identities.json").exists());
    // The seat of a running game holds the nick.
    srv.shared.identity.reserve("veteran");

    // Its browser still has the token: it logs in as before.
    let mut ws = connect(&srv);
    assert_eq!(log_in(&mut ws, "Veteran", "", Some(old)), old);
    drop(ws);
    // Nobody can log in by password yet.
    let mut ws = connect(&srv);
    assert_eq!(
        log_in_rejected(&mut ws, "Veteran", PW, None),
        RejectReason::BadPassword
    );
    // A stranger cannot take the nick of a seat (the code is right, the token is not).
    let r = register(&srv, "veteran", PW, Some(CODE));
    assert_eq!(r.status, 409, "{}", r.body);
    assert!(
        r.json()["error"]
            .as_str()
            .unwrap()
            .contains("seat in a game")
    );
    assert_eq!(r.json()["field"], "username");
    let wrong = post_json(
        &srv,
        "/api/auth/register",
        json!({"username": "veteran", "password": PW, "code": CODE, "legacy_token": "00".repeat(16)}),
    );
    assert_eq!(wrong.status, 409);
    // The owner sets a password with the old token (the page sends it along).
    let r = post_json(
        &srv,
        "/api/auth/register",
        json!({"username": "veteran", "password": PW, "code": CODE, "legacy_token": token_to_hex(&old)}),
    );
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["nick"], "Veteran");
    let fresh = r.token();
    assert!(srv.shared.identity.is_reserved("veteran"), "the seat stays");
    // From now on the password works and the new token does; the old one was replaced.
    let mut ws = connect(&srv);
    log_in(&mut ws, "Veteran", PW, None);
    drop(ws);
    let mut ws = connect(&srv);
    assert_eq!(log_in(&mut ws, "Veteran", "", Some(fresh)), fresh);
    drop(ws);
    let mut ws = connect(&srv);
    assert_eq!(
        log_in_rejected(&mut ws, "Veteran", "", Some(old)),
        RejectReason::BadPassword
    );
    // The file survives a restart and holds no token and no password.
    let text = std::fs::read_to_string(dir.join("accounts.json")).unwrap();
    assert!(
        !text.contains(&token_to_hex(&fresh)) && !text.contains(PW),
        "{text}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_coded_server_refuses_to_register_without_the_code() {
    let srv = TestServer::start(coded());
    let info = http(srv.addr, "GET", "/api/auth/info", &[], b"");
    assert_eq!(info.status, 200);
    assert_eq!(
        info.json(),
        json!({"registration": "code", "min_password": 8})
    );
    assert!(info.head.contains("cache-control: no-store"));

    for code in [None, Some(""), Some("wrong-code")] {
        let r = register(&srv, "alice", PW, code);
        assert_eq!(r.status, 400, "{code:?}: {}", r.body);
        assert_eq!(r.json()["field"], "code");
    }
    assert!(srv.shared.identity.is_empty());
    assert_eq!(register(&srv, "alice", PW, Some(CODE)).status, 200);
}

#[test]
fn open_registration_needs_no_code() {
    let srv = TestServer::start(open_registration());
    let info = http(srv.addr, "GET", "/api/auth/info", &[], b"");
    assert_eq!(info.json()["registration"], "open");
    assert_eq!(register(&srv, "alice", PW, None).status, 200);
    // The server password in the environment is ignored there; any code passes.
    assert_eq!(register(&srv, "bob", PW, Some("whatever")).status, 200);
}

#[test]
fn registration_checks_nick_password_and_conflicts() {
    let srv = TestServer::start(coded());
    let r = register(&srv, "  ", PW, Some(CODE));
    assert_eq!(
        (r.status, r.json()["field"].clone()),
        (400, json!("username"))
    );
    let r = register(&srv, "bad\u{200b}nick", PW, Some(CODE));
    assert_eq!(r.status, 400);
    let r = register(&srv, "alice", "short", Some(CODE));
    assert_eq!(
        (r.status, r.json()["field"].clone()),
        (400, json!("password"))
    );
    let r = register(&srv, "alice", &"x".repeat(129), Some(CODE));
    assert_eq!(r.status, 400);

    assert_eq!(register(&srv, "alice", PW, Some(CODE)).status, 200);
    let r = register(&srv, "ALICE", "another password", Some(CODE));
    assert_eq!(r.status, 409, "{}", r.body);
    assert_eq!(r.json()["field"], "username");
    assert_eq!(srv.shared.identity.len(), 1);
}

#[test]
fn http_login_and_logout() {
    let srv = TestServer::start(coded());
    register(&srv, "alice", PW, Some(CODE));

    let bad = login_http(&srv, "alice", "wrong password");
    assert_eq!(bad.status, 401);
    assert_eq!(bad.json()["error"], "Wrong nick or password.");
    // An unknown nick answers the same.
    let unknown = login_http(&srv, "nobody", PW);
    assert_eq!((unknown.status, unknown.json()), (401, bad.json()));

    let ok = login_http(&srv, "ALICE", PW);
    assert_eq!(ok.status, 200, "{}", ok.body);
    assert_eq!(ok.json()["nick"], "alice");
    let token = ok.token();
    let mut ws = connect(&srv);
    assert_eq!(log_in(&mut ws, "alice", "", Some(token)), token);

    // Logout drops the device of the cookie and its connection; the token is dead.
    let out = http(
        srv.addr,
        "POST",
        "/api/auth/logout",
        &[("Cookie", ok.cookie_pair().as_str())],
        b"",
    );
    assert_eq!(out.status, 204);
    assert!(out.head.contains("cache-control: no-store"));
    assert_replaced(&mut ws);
    let mut ws = connect(&srv);
    assert_eq!(
        log_in_rejected(&mut ws, "alice", "", Some(token)),
        RejectReason::BadPassword
    );
    // The password still works.
    assert_eq!(login_http(&srv, "alice", PW).status, 200);
}

#[test]
fn form_bodies_origin_and_size_limits() {
    let srv = TestServer::start(coded());
    let form = [("Content-Type", "application/x-www-form-urlencoded")];
    let r = http(
        srv.addr,
        "POST",
        "/api/auth/register",
        &form,
        format!("username=J%C3%BCrgen&password=p%40ss+word%21&code={CODE}").as_bytes(),
    );
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.json()["nick"], "Jürgen");
    let r = http(
        srv.addr,
        "POST",
        "/api/auth/login",
        &form,
        b"username=J%C3%BCrgen&password=p%40ss+word%21",
    );
    assert_eq!(r.status, 200, "{}", r.body);

    // A page of another origin may not log in (or register): 403, nothing created.
    let evil = [
        ("Content-Type", "application/x-www-form-urlencoded"),
        ("Origin", "https://evil.example"),
    ];
    let body = format!("username=eve&password={PW}&code={CODE}");
    let r = http(
        srv.addr,
        "POST",
        "/api/auth/register",
        &evil,
        body.as_bytes(),
    );
    assert_eq!(r.status, 403);
    assert!(r.head.contains("cache-control: no-store"));
    let r = http(
        srv.addr,
        "POST",
        "/api/auth/login",
        &evil,
        b"username=a&password=b",
    );
    assert_eq!(r.status, 403);
    assert_eq!(srv.shared.identity.len(), 1);
    // The page's own origin passes.
    let own = format!("http://{}", srv.addr);
    let same = [
        ("Content-Type", "application/x-www-form-urlencoded"),
        ("Origin", own.as_str()),
    ];
    let r = http(
        srv.addr,
        "POST",
        "/api/auth/register",
        &same,
        body.as_bytes(),
    );
    assert_eq!(r.status, 200, "{}", r.body);

    // At most 4 KiB.
    let big = vec![b'a'; 5000];
    let r = http(srv.addr, "POST", "/api/auth/login", &form, &big);
    assert_eq!(r.status, 413);
    // Garbage is a 400, not a crash.
    let r = http(srv.addr, "POST", "/api/auth/login", &form, b"username=%zz");
    assert_eq!(r.status, 400);
}

#[test]
fn registrations_are_limited_per_address() {
    let srv = TestServer::start(coded());
    for i in 0..5 {
        let r = register(&srv, &format!("user{i}"), PW, Some(CODE));
        assert_eq!(r.status, 200, "{i}: {}", r.body);
    }
    let r = register(&srv, "user5", PW, Some(CODE));
    assert_eq!(r.status, 429, "{}", r.body);
    let retry: u64 = r
        .head
        .lines()
        .find_map(|l| l.strip_prefix("retry-after: "))
        .expect("Retry-After")
        .trim()
        .parse()
        .unwrap();
    assert!((3590..=3600).contains(&retry), "{retry}");
    assert_eq!(srv.shared.identity.len(), 5);
}

#[test]
fn wrong_codes_and_passwords_over_http_and_websocket_block_the_address_together() {
    let srv = TestServer::start(coded());
    register(&srv, "alice", PW, Some(CODE));
    // Four wrong codes, three wrong HTTP passwords, three wrong WebSocket passwords: ten failures.
    for _ in 0..4 {
        assert_eq!(register(&srv, "x", PW, Some("nope")).status, 400);
    }
    for _ in 0..3 {
        assert_eq!(login_http(&srv, "alice", "wrong").status, 401);
    }
    for _ in 0..3 {
        let mut ws = connect(&srv);
        assert_eq!(
            log_in_rejected(&mut ws, "alice", "wrong", None),
            RejectReason::BadPassword
        );
    }
    // The address is blocked: even the right password is not looked at, over HTTP ...
    let r = login_http(&srv, "alice", PW);
    assert_eq!(r.status, 429, "{}", r.body);
    assert!(r.head.contains("retry-after: "));
    assert_eq!(register(&srv, "bob", PW, Some(CODE)).status, 429);
    // ... and over the WebSocket; a token still works.
    let mut ws = connect(&srv);
    assert_eq!(
        log_in_rejected(&mut ws, "alice", PW, None),
        RejectReason::BadPassword
    );
}

#[test]
fn an_admin_reset_lets_the_owner_register_again() {
    let srv = TestServer::start(coded());
    let token = register(&srv, "alice", PW, Some(CODE)).token();
    srv.shared.identity.reserve("alice");
    let mut ws = connect(&srv);
    log_in(&mut ws, "alice", "", Some(token));

    // The admin API needs its token.
    let no = http(srv.admin, "POST", "/admin/accounts/alice/reset", &[], b"");
    assert_eq!(no.status, 401);
    assert_eq!(
        admin(&srv, "POST", "/admin/accounts/nobody/reset").status,
        404
    );
    let r = admin(&srv, "POST", "/admin/accounts/Alice/reset");
    assert_eq!(r.status, 200, "{}", r.body);
    // The live session is kicked, the password and the token are dead.
    assert_replaced(&mut ws);
    assert_eq!(login_http(&srv, "alice", PW).status, 401);
    let mut ws = connect(&srv);
    assert_eq!(
        log_in_rejected(&mut ws, "alice", "", Some(token)),
        RejectReason::BadPassword
    );
    // The seat is still reserved for the nick, and the owner registers it with a new password.
    assert!(srv.shared.identity.is_reserved("alice"));
    let again = register(&srv, "alice", "a brand new password", Some(CODE));
    assert_eq!(again.status, 200, "{}", again.body);
    assert_eq!(
        login_http(&srv, "alice", "a brand new password").status,
        200
    );
}

#[test]
fn auto_register_makes_the_account_at_the_first_password_login() {
    // The in-process test config (and --dev): the registration code is the password of the first login.
    let srv = TestServer::start(ServerConfig {
        auto_register: true,
        ..coded()
    });
    let mut ws = connect(&srv);
    let token = log_in(&mut ws, "bot1", CODE, None);
    drop(ws);
    assert_eq!(srv.shared.identity.len(), 1);
    // The password it made the account with logs in again; the token too.
    let mut ws = connect(&srv);
    assert_eq!(log_in(&mut ws, "bot1", "", Some(token)), token);
    drop(ws);
    let mut ws = connect(&srv);
    log_in(&mut ws, "bot1", CODE, None);
    // A wrong code does not create anything.
    let mut ws = connect(&srv);
    assert_eq!(
        log_in_rejected(&mut ws, "bot2", "not the code", None),
        RejectReason::BadPassword
    );
    assert_eq!(srv.shared.identity.len(), 1);
}

#[test]
fn accounts_and_devices_survive_a_restart_and_keep_only_hashes() {
    let dir = temp_dir("restart");
    let (token, pw_token) = {
        let srv = TestServer::start(ServerConfig {
            state_dir: Some(dir.clone()),
            ..coded()
        });
        let token = register(&srv, "alice", PW, Some(CODE)).token();
        let pw_token = login_http(&srv, "alice", PW).token();
        (token, pw_token)
    };
    let srv = TestServer::start(ServerConfig {
        state_dir: Some(dir.clone()),
        ..coded()
    });
    let mut ws = connect(&srv);
    assert_eq!(log_in(&mut ws, "alice", "", Some(token)), token);
    drop(ws);
    let mut ws = connect(&srv);
    assert_eq!(log_in(&mut ws, "alice", "", Some(pw_token)), pw_token);
    let text = std::fs::read_to_string(dir.join("accounts.json")).unwrap();
    assert!(text.contains("$argon2id$v=19$m=19456,t=2,p=1$"), "{text}");
    assert!(text.contains("\"format\": 2"));
    assert!(
        !text.contains(PW) && !text.contains(&token_to_hex(&token)),
        "{text}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------ the session cookie (round 2)

/// A WebSocket whose upgrade request carries `headers` (a `Cookie`, an `Origin`).
fn connect_with(srv: &TestServer, headers: &[(&str, &str)]) -> Result<Ws, tungstenite::Error> {
    use tungstenite::client::IntoClientRequest;
    let mut req = format!("ws://{}/ws", srv.addr)
        .into_client_request()
        .unwrap();
    for (k, v) in headers {
        req.headers_mut().insert(
            tungstenite::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
            v.parse().unwrap(),
        );
    }
    let (mut ws, _) = tungstenite::connect(req)?;
    set_timeout(&mut ws, Duration::from_secs(5));
    Ok(ws)
}

/// A cookie login: no token and no password in the message. Returns the `LoggedIn` session.
fn log_in_by_cookie(ws: &mut Ws, nick: &str) -> protocol::Session {
    hello_login(ws, nick, "", None);
    assert!(matches!(recv(ws), ServerMsg::Welcome { .. }));
    let session = match recv(ws) {
        ServerMsg::LoggedIn(s) => s,
        other => panic!("expected LoggedIn, got {other:?}"),
    };
    assert!(matches!(recv(ws), ServerMsg::Choices(_)));
    assert!(matches!(recv(ws), ServerMsg::LobbyState(_)));
    session
}

#[test]
fn login_and_register_set_the_http_only_session_cookie() {
    let srv = TestServer::start(coded());
    let r = register(&srv, "Alice", PW, Some(CODE));
    assert_eq!(r.status, 200, "{}", r.body);
    let c = r.session_cookie();
    let token_hex = token_to_hex(&r.token());
    assert!(
        c.starts_with(&format!("civ5_session=Alice.{token_hex};")),
        "{c}"
    );
    for attr in [
        "Path=/",
        "HttpOnly",
        "SameSite=Lax",
        "Secure",
        "Max-Age=7776000",
    ] {
        assert!(c.split("; ").any(|a| a == attr), "{attr} missing in {c}");
    }
    // The body names the nick and nothing else.
    assert_eq!(r.json(), json!({"nick": "Alice"}));

    let l = login_http(&srv, "alice", PW);
    assert_eq!(l.status, 200, "{}", l.body);
    assert!(l.session_cookie().starts_with("civ5_session=Alice."));
    assert!(l.session_cookie().contains("; HttpOnly"));
    assert_eq!(l.json(), json!({"nick": "Alice"}));
    assert_ne!(l.token(), r.token(), "a login is a device of its own");
    // A failed login sets none.
    assert!(
        login_http(&srv, "alice", "nope nope nope")
            .set_cookies
            .is_empty()
    );
}

#[test]
fn a_plain_http_profile_can_drop_secure() {
    let srv = TestServer::start(ServerConfig {
        cookie_secure: false,
        ..coded()
    });
    let r = register(&srv, "alice", PW, Some(CODE));
    let c = r.session_cookie();
    assert!(c.contains("HttpOnly") && c.contains("SameSite=Lax"), "{c}");
    assert!(!c.contains("Secure"), "{c}");
    let out = http(srv.addr, "POST", "/api/auth/logout", &[], b"");
    assert!(!out.session_cookie().contains("Secure"));
}

#[test]
fn the_websocket_logs_in_from_the_cookie_and_gets_no_token() {
    let srv = TestServer::start(coded());
    let r = register(&srv, "Alice", PW, Some(CODE));
    let cookie = r.cookie_pair();
    let mut ws = connect_with(&srv, &[("Cookie", cookie.as_str())]).unwrap();
    // A message with no token and no password; the nick may differ in case.
    let session = log_in_by_cookie(&mut ws, "alice");
    assert_eq!(session.nick, "Alice");
    assert_eq!(session.token, None);
    ping(&mut ws, 1);

    // The cookie of another nick opens nothing for this one.
    let mut ws2 = connect_with(&srv, &[("Cookie", cookie.as_str())]).unwrap();
    hello_login(&mut ws2, "Bob", "", None);
    assert!(matches!(recv(&mut ws2), ServerMsg::Welcome { .. }));
    assert_eq!(recv(&mut ws2), ServerMsg::Reject(RejectReason::BadPassword));
    // No cookie, no token, no password: refused, and nothing counts against the address.
    for _ in 0..12 {
        let mut ws3 = connect(&srv);
        assert_eq!(
            log_in_rejected(&mut ws3, "Alice", "", None),
            RejectReason::BadPassword
        );
    }
    let mut ws4 = connect(&srv);
    assert_ne!(log_in(&mut ws4, "Alice", PW, None), r.token());

    // A token login over the WebSocket still hands the token to a native client.
    let mut ws5 = connect(&srv);
    let token = r.token();
    assert_eq!(log_in(&mut ws5, "Alice", "", Some(token)), token);
}

#[test]
fn logout_clears_the_cookie_and_the_device() {
    let srv = TestServer::start(coded());
    let r = register(&srv, "alice", PW, Some(CODE));
    let cookie = r.cookie_pair();
    let mut ws = connect_with(&srv, &[("Cookie", cookie.as_str())]).unwrap();
    log_in_by_cookie(&mut ws, "alice");

    let out = http(
        srv.addr,
        "POST",
        "/api/auth/logout",
        &[("Cookie", cookie.as_str())],
        b"",
    );
    assert_eq!(out.status, 204);
    let line = out.session_cookie();
    assert!(line.starts_with("civ5_session=;"), "{line}");
    assert!(
        line.contains("Max-Age=0") && line.contains("HttpOnly"),
        "{line}"
    );
    assert_replaced(&mut ws);
    assert_eq!(srv.shared.identity.device_count("alice"), 0);

    // The old cookie is dead.
    let mut ws = connect_with(&srv, &[("Cookie", cookie.as_str())]).unwrap();
    hello_login(&mut ws, "alice", "", None);
    assert!(matches!(recv(&mut ws), ServerMsg::Welcome { .. }));
    assert_eq!(recv(&mut ws), ServerMsg::Reject(RejectReason::BadPassword));
    // Logging out again, or without a cookie, is as quiet.
    assert_eq!(
        http(srv.addr, "POST", "/api/auth/logout", &[], b"").status,
        204
    );
}

#[test]
fn adopt_turns_an_old_token_into_the_cookie() {
    let srv = TestServer::start(coded());
    let token = register(&srv, "Alice", PW, Some(CODE)).token();
    let form = [("Content-Type", "application/x-www-form-urlencoded")];
    let body = format!("username=alice&token={}", token_to_hex(&token));
    let ok = http(srv.addr, "POST", "/api/auth/adopt", &form, body.as_bytes());
    assert_eq!(ok.status, 200, "{}", ok.body);
    assert_eq!(ok.json(), json!({"nick": "Alice"}));
    assert!(ok.session_cookie().contains("; HttpOnly"));
    assert_eq!(ok.token(), token, "the same device keeps its token");
    let mut ws = connect_with(&srv, &[("Cookie", ok.cookie_pair().as_str())]).unwrap();
    assert_eq!(log_in_by_cookie(&mut ws, "Alice").token, None);

    // A wrong token, a token of another nick, a bad hex string: 401 and no cookie.
    let bob = register(&srv, "bob", PW, Some(CODE)).token();
    for body in [
        format!("username=alice&token={}", "00".repeat(16)),
        format!("username=alice&token={}", token_to_hex(&bob)),
        "username=alice&token=zz".to_owned(),
        "username=nobody&token=".to_owned() + &token_to_hex(&token),
    ] {
        let r = http(srv.addr, "POST", "/api/auth/adopt", &form, body.as_bytes());
        assert_eq!(r.status, 401, "{body}: {}", r.body);
        assert!(r.set_cookies.is_empty(), "{body}");
    }
    // Another origin may not adopt.
    let r = http(
        srv.addr,
        "POST",
        "/api/auth/adopt",
        &[
            ("Content-Type", "application/x-www-form-urlencoded"),
            ("Origin", "https://evil.example"),
        ],
        body.as_bytes(),
    );
    assert_eq!(r.status, 403);
    assert!(r.set_cookies.is_empty());
}

#[test]
fn a_foreign_origin_is_refused_on_the_upgrade_and_on_the_posts() {
    let srv = TestServer::start(coded());
    let r = register(&srv, "alice", PW, Some(CODE));
    let cookie = r.cookie_pair();
    let evil = [
        ("Cookie", cookie.as_str()),
        ("Origin", "https://evil.example"),
    ];
    match connect_with(&srv, &evil) {
        Err(tungstenite::Error::Http(resp)) => assert_eq!(resp.status().as_u16(), 403),
        other => panic!("expected a 403, got {:?}", other.map(|_| ())),
    }
    // The page's own origin passes, and so does no origin at all.
    let own = format!("http://{}", srv.addr);
    let mut ws = connect_with(
        &srv,
        &[("Cookie", cookie.as_str()), ("Origin", own.as_str())],
    )
    .unwrap();
    log_in_by_cookie(&mut ws, "alice");
    // A POST of another origin: 403, and no cookie is set.
    for path in ["/api/auth/login", "/api/auth/register", "/api/auth/logout"] {
        let r = http(
            srv.addr,
            "POST",
            path,
            &[
                ("Content-Type", "application/json"),
                ("Origin", "https://evil.example"),
                ("Cookie", cookie.as_str()),
            ],
            json!({"username": "alice", "password": PW, "code": CODE})
                .to_string()
                .as_bytes(),
        );
        assert_eq!(r.status, 403, "{path}");
        assert!(r.set_cookies.is_empty(), "{path}");
    }
    // The logout of the foreign origin did not drop the device.
    assert_eq!(srv.shared.identity.device_count("alice"), 1);
}

#[test]
fn the_cookie_of_a_legacy_nick_is_the_proof_to_claim_it() {
    let dir = temp_dir("legacy-cookie");
    let old = Token([0x5b; 16]);
    std::fs::write(
        dir.join("identities.json"),
        format!(
            r#"{{"format": 1, "records": [{{"nick": "Veteran", "token": "{}", "last_seen_ms": 1}}]}}"#,
            token_to_hex(&old)
        ),
    )
    .unwrap();
    let srv = TestServer::start(ServerConfig {
        state_dir: Some(dir.clone()),
        ..coded()
    });
    // Seat held: the old token is the only way. The page sends it as the adopted cookie.
    srv.shared.identity.reserve("veteran");
    let form = [("Content-Type", "application/x-www-form-urlencoded")];
    let adopted = http(
        srv.addr,
        "POST",
        "/api/auth/adopt",
        &form,
        format!("username=Veteran&token={}", token_to_hex(&old)).as_bytes(),
    );
    assert_eq!(adopted.status, 200, "{}", adopted.body);
    let cookie = adopted.cookie_pair();
    let body = json!({"username": "veteran", "password": PW, "code": CODE}).to_string();
    let no_cookie = http(
        srv.addr,
        "POST",
        "/api/auth/register",
        &[("Content-Type", "application/json")],
        body.as_bytes(),
    );
    assert_eq!(no_cookie.status, 409, "{}", no_cookie.body);
    let claimed = http(
        srv.addr,
        "POST",
        "/api/auth/register",
        &[
            ("Content-Type", "application/json"),
            ("Cookie", cookie.as_str()),
        ],
        body.as_bytes(),
    );
    assert_eq!(claimed.status, 200, "{}", claimed.body);
    let _ = std::fs::remove_dir_all(&dir);
}
