//! Login over a real socket (NP 4.12): `Hello` + `Login`, password, token, nick
//! reservation, tab replacement, version and v2 clients.

use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use protocol::{
    ClientMsg, Codec, DetachReason, Login, PROTOCOL_VERSION, Postcard, RejectReason, ServerMsg,
    Token,
};
use server::identity::Secret;
use server::session::limits::LimitsConfig;
use server::{ServerConfig, Shared};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};
use wire::{Encoder, Limits};

const PASSWORD: &str = "correct-horse";

struct TestServer {
    addr: SocketAddr,
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
        let listener = rt
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let shared = Shared::new(config).unwrap();
        rt.spawn(server::serve(listener, shared.clone()));
        Self {
            addr,
            shared,
            _rt: rt,
        }
    }

    /// A server that knows [`PASSWORD`].
    fn with_password() -> Self {
        Self::start(config())
    }
}

fn config() -> ServerConfig {
    ServerConfig {
        password: Some(Secret::new(PASSWORD)),
        // The real 1 s delay would slow every wrong-password test.
        limits: LimitsConfig {
            password_delay: Duration::from_millis(5),
            ..LimitsConfig::default()
        },
        ..ServerConfig::default()
    }
}

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

fn hello(protocol: u32, nick: &str) -> ClientMsg {
    ClientMsg::Hello {
        protocol,
        build: "test".into(),
        nick: nick.into(),
    }
}

fn login(password: &str, token: Option<Token>) -> ClientMsg {
    ClientMsg::Login(Login {
        password: password.into(),
        token,
        resume: None,
    })
}

fn recv(ws: &mut Ws) -> ServerMsg {
    match ws.read().unwrap() {
        Message::Binary(b) => wire::decode(&b, &Limits::CLIENT).unwrap(),
        other => panic!("expected a binary message, got {other:?}"),
    }
}

/// The socket is closed (a Close frame or an error), possibly after more data.
fn assert_closed(ws: &mut Ws) {
    loop {
        match ws.read() {
            Ok(Message::Close(_)) | Err(_) => return,
            Ok(_) => {}
        }
    }
}

/// Hello + Login back to back, as a client does. Returns the `LoggedIn` token.
fn log_in(ws: &mut Ws, nick: &str, password: &str, token: Option<Token>) -> Token {
    send(ws, &hello(PROTOCOL_VERSION, nick));
    send(ws, &login(password, token));
    assert!(matches!(recv(ws), ServerMsg::Welcome { .. }));
    let token = match recv(ws) {
        ServerMsg::LoggedIn(s) => {
            assert_eq!(s.nick, nick);
            s.token.expect("the login carries the token")
        }
        other => panic!("expected LoggedIn, got {other:?}"),
    };
    // No seat: the pick lists once, then the lobby.
    assert!(matches!(recv(ws), ServerMsg::Choices(_)));
    assert!(matches!(recv(ws), ServerMsg::LobbyState(_)));
    token
}

/// Hello + Login that must end in a `Reject`.
fn log_in_rejected(ws: &mut Ws, nick: &str, password: &str, token: Option<Token>) -> RejectReason {
    send(ws, &hello(PROTOCOL_VERSION, nick));
    send(ws, &login(password, token));
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
    send(ws, &ClientMsg::Ping { seq, client_ms: 77 });
    match recv(ws) {
        ServerMsg::Pong {
            seq: s, client_ms, ..
        } => assert_eq!((s, client_ms), (seq, 77)),
        other => panic!("expected Pong, got {other:?}"),
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("civ5-session-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn password_login_then_ping() {
    let srv = TestServer::with_password();
    let mut ws = connect(&srv);
    send(&mut ws, &hello(PROTOCOL_VERSION, "alice"));
    send(&mut ws, &login(PASSWORD, None));
    match recv(&mut ws) {
        ServerMsg::Welcome { session, build } => {
            assert_eq!(session, 1);
            assert_eq!(build, env!("CARGO_PKG_VERSION"));
        }
        other => panic!("expected Welcome, got {other:?}"),
    }
    match recv(&mut ws) {
        ServerMsg::LoggedIn(s) => assert_eq!(s.nick, "alice"),
        other => panic!("expected LoggedIn, got {other:?}"),
    }
    assert!(matches!(recv(&mut ws), ServerMsg::Choices(_)));
    assert!(matches!(recv(&mut ws), ServerMsg::LobbyState(_)));
    ping(&mut ws, 1);
    ping(&mut ws, 2);
}

#[test]
fn wrong_password_is_rejected_and_closed() {
    let srv = TestServer::with_password();
    let mut ws = connect(&srv);
    let r = log_in_rejected(&mut ws, "alice", "wrong-password", None);
    assert_eq!(r, RejectReason::BadPassword);
    assert!(srv.shared.identity.is_empty());
}

#[test]
fn server_without_a_password_refuses_password_logins() {
    let srv = TestServer::start(ServerConfig::default());
    let mut ws = connect(&srv);
    let r = log_in_rejected(&mut ws, "alice", "", None);
    assert_eq!(r, RejectReason::BadPassword);
}

#[test]
fn open_access_logs_in_by_nick_alone() {
    let srv = TestServer::start(ServerConfig {
        open_access: true,
        ..ServerConfig::default()
    });
    let mut ws = connect(&srv);
    let token = log_in(&mut ws, "guest", "", None);
    ping(&mut ws, 1);
    // The same nick logs in again (the first login made its account) and replaces the live session; the token
    // of the first login still works for its device.
    let mut other = connect(&srv);
    let second = log_in(&mut other, "guest", "", None);
    assert_ne!(second, token);
    assert_eq!(
        recv(&mut ws),
        ServerMsg::Detached {
            reason: DetachReason::Replaced
        }
    );
    drop(other);
    let mut again = connect(&srv);
    assert_eq!(log_in(&mut again, "guest", "", Some(token)), token);
}

#[test]
fn token_replaces_the_password_after_a_reconnect() {
    let srv = TestServer::with_password();
    let mut first = connect(&srv);
    let token = log_in(&mut first, "alice", PASSWORD, None);
    drop(first);

    // A page reload: only the token, no password.
    let mut second = connect(&srv);
    let again = log_in(&mut second, "alice", "", Some(token));
    assert_eq!(again, token);
    ping(&mut second, 1);
}

#[test]
fn token_of_another_nick_does_not_count() {
    let srv = TestServer::with_password();
    let mut a = connect(&srv);
    let token = log_in(&mut a, "alice", PASSWORD, None);
    let mut b = connect(&srv);
    let r = log_in_rejected(&mut b, "bob", "", Some(token));
    assert_eq!(r, RejectReason::BadPassword);
    // With the right password the foreign token is ignored and bob gets his own.
    let mut b = connect(&srv);
    let own = log_in(&mut b, "bob", PASSWORD, Some(token));
    assert_ne!(own, token);
}

#[test]
fn live_nick_is_replaced_by_a_password_login_of_the_owner() {
    let srv = TestServer::with_password();
    let mut a = connect(&srv);
    log_in(&mut a, "alice", PASSWORD, None);
    // The account's password (the one it was made with) logs in again: the old session goes.
    let mut b = connect(&srv);
    log_in(&mut b, "alice", PASSWORD, None);
    assert_eq!(
        recv(&mut a),
        ServerMsg::Detached {
            reason: DetachReason::Replaced
        }
    );
    assert_closed(&mut a);
    ping(&mut b, 1);
    // Another password does not.
    let mut c = connect(&srv);
    assert_eq!(
        log_in_rejected(&mut c, "alice", "some-other-password", None),
        RejectReason::BadPassword
    );
    ping(&mut b, 2);
}

#[test]
fn a_reserved_nick_keeps_its_account_and_logs_in_by_password_or_token() {
    let srv = TestServer::with_password();
    let mut a = connect(&srv);
    let token = log_in(&mut a, "alice", PASSWORD, None);
    drop(a);
    srv.shared.identity.reserve("alice");

    // Another password is no way in; the owner's password and token are.
    let mut b = connect(&srv);
    assert_eq!(
        log_in_rejected(&mut b, "alice", "some-other-password", None),
        RejectReason::BadPassword
    );
    let mut c = connect(&srv);
    assert_eq!(log_in(&mut c, "alice", "", Some(token)), token);
    drop(c);
    let mut d = connect(&srv);
    log_in(&mut d, "alice", PASSWORD, None);
}

#[test]
fn second_tab_replaces_the_first() {
    let srv = TestServer::with_password();
    let mut old = connect(&srv);
    let token = log_in(&mut old, "bob", PASSWORD, None);

    let mut new = connect(&srv);
    assert_eq!(log_in(&mut new, "bob", "", Some(token)), token);

    // The old socket hears why, then closes.
    assert_eq!(
        recv(&mut old),
        ServerMsg::Detached {
            reason: DetachReason::Replaced
        }
    );
    assert_closed(&mut old);
    ping(&mut new, 1);
    assert_eq!(srv.shared.identity.live_count(), 1);
}

#[test]
fn version_mismatch_is_rejected_framed() {
    let srv = TestServer::with_password();
    let mut ws = connect(&srv);
    send(&mut ws, &hello(PROTOCOL_VERSION + 1, "alice"));
    assert_eq!(
        recv(&mut ws),
        ServerMsg::Reject(RejectReason::VersionMismatch {
            server: PROTOCOL_VERSION
        })
    );
    assert_closed(&mut ws);
}

/// A client one version behind (a v14 build against a v15 server: it cannot read or send the v15
/// orders) is refused the same way, before any session exists.
#[test]
fn older_client_is_rejected_framed() {
    let srv = TestServer::with_password();
    let mut ws = connect(&srv);
    send(&mut ws, &hello(PROTOCOL_VERSION - 1, "alice"));
    assert_eq!(
        recv(&mut ws),
        ServerMsg::Reject(RejectReason::VersionMismatch {
            server: PROTOCOL_VERSION
        })
    );
    assert_closed(&mut ws);
    assert_eq!(srv.shared.identity.live_count(), 0);
}

#[test]
fn v2_client_gets_a_bare_postcard_v2_reject() {
    let srv = TestServer::with_password();
    let mut ws = connect(&srv);
    // What a v2 client sends: postcard without a frame byte.
    let v2_hello = protocol::ClientMsg::Hello {
        protocol: 2,
        build: "old".into(),
        nick: "alice".into(),
    };
    ws.send(Message::binary(Postcard.encode(&v2_hello).unwrap()))
        .unwrap();
    match ws.read().unwrap() {
        Message::Binary(b) => {
            // Bare postcard, readable by the v2 decoder.
            let msg: protocol::ServerMsg = Postcard.decode(&b).unwrap();
            assert_eq!(
                msg,
                protocol::ServerMsg::Reject(protocol::RejectReason::VersionMismatch {
                    server: PROTOCOL_VERSION
                })
            );
        }
        other => panic!("expected a binary message, got {other:?}"),
    }
    assert_closed(&mut ws);
}

#[test]
fn bad_nicks_are_rejected() {
    let srv = TestServer::with_password();
    for nick in ["", "   ", "a\nb", "bob\u{200B}", &"x".repeat(25)] {
        let mut ws = connect(&srv);
        send(&mut ws, &hello(PROTOCOL_VERSION, nick));
        assert_eq!(
            recv(&mut ws),
            ServerMsg::Reject(RejectReason::BadNick),
            "nick {nick:?}"
        );
        assert_closed(&mut ws);
    }
}

#[test]
fn first_message_must_be_hello() {
    let srv = TestServer::with_password();
    let mut ws = connect(&srv);
    send(
        &mut ws,
        &ClientMsg::Ping {
            seq: 0,
            client_ms: 0,
        },
    );
    assert_closed(&mut ws);

    let mut ws = connect(&srv);
    send(&mut ws, &login(PASSWORD, None));
    assert_closed(&mut ws);
}

#[test]
fn login_must_follow_hello() {
    let srv = TestServer::with_password();
    let mut ws = connect(&srv);
    send(&mut ws, &hello(PROTOCOL_VERSION, "alice"));
    assert!(matches!(recv(&mut ws), ServerMsg::Welcome { .. }));
    send(
        &mut ws,
        &ClientMsg::Ping {
            seq: 0,
            client_ms: 0,
        },
    );
    assert_closed(&mut ws);
}

#[test]
fn no_login_within_the_window_closes_the_socket() {
    let srv = TestServer::start(ServerConfig {
        login_timeout: Duration::from_millis(200),
        ..config()
    });
    let mut ws = connect(&srv);
    assert_closed(&mut ws);

    // Hello alone is not enough either.
    let mut ws = connect(&srv);
    send(&mut ws, &hello(PROTOCOL_VERSION, "alice"));
    assert!(matches!(recv(&mut ws), ServerMsg::Welcome { .. }));
    assert_closed(&mut ws);
}

#[test]
fn text_compressed_and_oversized_frames_close_the_connection() {
    let srv = TestServer::with_password();

    let mut ws = connect(&srv);
    ws.send(Message::text("hello")).unwrap();
    assert_closed(&mut ws);

    // enc 1: the gateway takes only plain frames from clients.
    let mut ws = connect(&srv);
    let body = Postcard.encode(&hello(PROTOCOL_VERSION, "alice")).unwrap();
    let mut packed = vec![1u8];
    packed.extend_from_slice(&body);
    ws.send(Message::binary(packed)).unwrap();
    assert_closed(&mut ws);

    // Over 64 KiB.
    let mut ws = connect(&srv);
    let mut big = vec![0u8; 70 * 1024];
    big[1] = 1;
    // The server may close while the client is still writing; either way the socket ends.
    let _ = ws.send(Message::binary(big));
    assert_closed(&mut ws);
}

#[test]
fn unsupported_messages_get_a_reject_after_login() {
    let srv = TestServer::with_password();
    let mut ws = connect(&srv);
    log_in(&mut ws, "alice", PASSWORD, None);
    send(&mut ws, &ClientMsg::Continue);
    assert_eq!(
        recv(&mut ws),
        ServerMsg::Reject(RejectReason::Unsupported {
            kind: "Continue".into()
        })
    );
    assert_closed(&mut ws);
}

#[test]
fn repeated_hello_closes_the_session() {
    let srv = TestServer::with_password();
    let mut ws = connect(&srv);
    log_in(&mut ws, "alice", PASSWORD, None);
    send(&mut ws, &hello(PROTOCOL_VERSION, "alice"));
    assert_closed(&mut ws);
}

#[test]
fn too_many_sessions_get_full() {
    let srv = TestServer::start(ServerConfig {
        max_sessions: 1,
        ..config()
    });
    let mut a = connect(&srv);
    log_in(&mut a, "alice", PASSWORD, None);
    let mut b = connect(&srv);
    send(&mut b, &hello(PROTOCOL_VERSION, "bob"));
    assert_eq!(recv(&mut b), ServerMsg::Reject(RejectReason::Full));
    assert_closed(&mut b);
    ping(&mut a, 1);
}

#[test]
fn tokens_survive_a_gateway_restart() {
    let dir = temp_dir("restart");
    let token = {
        let srv = TestServer::start(ServerConfig {
            state_dir: Some(dir.clone()),
            ..config()
        });
        let mut ws = connect(&srv);
        log_in(&mut ws, "alice", PASSWORD, None)
    };
    // The first runtime is gone (and with it the live session). A new gateway, same state_dir.
    let srv = TestServer::start(ServerConfig {
        state_dir: Some(dir.clone()),
        ..config()
    });
    let mut ws = connect(&srv);
    assert_eq!(log_in(&mut ws, "alice", "", Some(token)), token);
    let file = std::fs::read_to_string(dir.join("accounts.json")).unwrap();
    assert!(!file.contains(PASSWORD), "the password is never stored");
    let hex: String = token.0.iter().map(|b| format!("{b:02x}")).collect();
    assert!(!file.contains(&hex), "the token is stored as a hash only");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn counters_follow_sessions() {
    use std::sync::atomic::Ordering::Relaxed;
    let srv = TestServer::with_password();
    let mut a = connect(&srv);
    let token = log_in(&mut a, "alice", PASSWORD, None);
    let mut bad = connect(&srv);
    log_in_rejected(&mut bad, "alice", "nope-nope", None);
    let mut b = connect(&srv);
    log_in(&mut b, "alice", "", Some(token));
    assert_eq!(
        recv(&mut a),
        ServerMsg::Detached {
            reason: DetachReason::Replaced
        }
    );
    let m = &srv.shared.metrics;
    assert_eq!(m.sessions_opened_total.load(Relaxed), 3);
    assert_eq!(m.login_failed_total.load(Relaxed), 1);
    assert_eq!(m.sessions_replaced_total.load(Relaxed), 1);
}

// ---- N2.3: WS ping, idle timeout, send queue, limits ------------------------------------

fn is_timeout(e: &tungstenite::Error) -> bool {
    matches!(e, tungstenite::Error::Io(io)
        if matches!(io.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut))
}

/// Next application message, skipping WS control frames.
fn recv_app(ws: &mut Ws) -> ServerMsg {
    loop {
        match ws.read().unwrap() {
            Message::Ping(_) | Message::Pong(_) => {}
            Message::Binary(b) => return wire::decode(&b, &Limits::CLIENT).unwrap(),
            other => panic!("expected a binary message, got {other:?}"),
        }
    }
}

fn wait_until(what: &str, cond: impl Fn() -> bool) {
    for _ in 0..200 {
        if cond() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out waiting for: {what}");
}

fn count(c: &std::sync::atomic::AtomicU64) -> u64 {
    c.load(std::sync::atomic::Ordering::Relaxed)
}

/// Like `log_in` but returns the session id from `Welcome` too.
fn log_in_session(ws: &mut Ws, nick: &str) -> u64 {
    send(ws, &hello(PROTOCOL_VERSION, nick));
    send(ws, &login(PASSWORD, None));
    let ServerMsg::Welcome { session, .. } = recv(ws) else {
        panic!("expected Welcome");
    };
    assert!(matches!(recv(ws), ServerMsg::LoggedIn(_)));
    assert!(matches!(recv(ws), ServerMsg::Choices(_)));
    assert!(matches!(recv(ws), ServerMsg::LobbyState(_)));
    session
}

fn timing(ping_ms: u64, idle_ms: u64) -> ServerConfig {
    ServerConfig {
        ping_interval: Duration::from_millis(ping_ms),
        idle_timeout: Duration::from_millis(idle_ms),
        ..config()
    }
}

#[test]
fn server_sends_a_ws_ping_every_interval() {
    let srv = TestServer::start(timing(100, 10_000));
    let mut ws = connect(&srv);
    log_in(&mut ws, "alice", PASSWORD, None);
    let started = std::time::Instant::now();
    for _ in 0..3 {
        match ws.read().unwrap() {
            Message::Ping(_) => {}
            other => panic!("expected a WS ping, got {other:?}"),
        }
    }
    // Three pings at 100 ms: not instant, not seconds.
    let took = started.elapsed();
    assert!(took >= Duration::from_millis(200), "{took:?}");
    assert!(took < Duration::from_secs(3), "{took:?}");
}

#[test]
fn silent_client_is_lost_after_the_idle_timeout() {
    let srv = TestServer::start(timing(50, 400));
    let mut ws = connect(&srv);
    log_in(&mut ws, "alice", PASSWORD, None);
    assert_eq!(count(&srv.shared.metrics.sessions_logged_in), 1);
    // The client reads nothing, so it answers no ping: no frame reaches the server.
    wait_until("session lost", || {
        count(&srv.shared.metrics.sessions_open) == 0
    });
    assert_eq!(count(&srv.shared.metrics.sessions_lost_timeout_total), 1);
    assert_eq!(srv.shared.identity.live_count(), 0);
    assert_closed(&mut ws);
}

#[test]
fn client_that_answers_pings_stays() {
    let srv = TestServer::start(timing(50, 400));
    let mut ws = connect(&srv);
    log_in(&mut ws, "alice", PASSWORD, None);
    set_timeout(&mut ws, Duration::from_millis(50));
    // Three idle timeouts of reading: tungstenite answers each ping with a pong.
    let until = std::time::Instant::now() + Duration::from_millis(1300);
    while std::time::Instant::now() < until {
        match ws.read() {
            Ok(Message::Ping(_) | Message::Pong(_)) => {}
            Err(e) if is_timeout(&e) => {}
            other => panic!("the session ended: {other:?}"),
        }
    }
    set_timeout(&mut ws, Duration::from_secs(5));
    send(
        &mut ws,
        &ClientMsg::Ping {
            seq: 1,
            client_ms: 77,
        },
    );
    assert!(matches!(recv_app(&mut ws), ServerMsg::Pong { seq: 1, .. }));
    assert_eq!(count(&srv.shared.metrics.sessions_lost_timeout_total), 0);
}

#[test]
fn any_client_frame_resets_the_idle_timer() {
    let srv = TestServer::start(timing(10_000, 400));
    let mut ws = connect(&srv);
    log_in(&mut ws, "alice", PASSWORD, None);
    // Application pings every 150 ms keep the session for over three idle timeouts.
    for seq in 0..9 {
        std::thread::sleep(Duration::from_millis(150));
        ping(&mut ws, seq);
    }
    assert_eq!(count(&srv.shared.metrics.sessions_lost_timeout_total), 0);
}

#[test]
fn queued_messages_reach_the_client() {
    let srv = TestServer::with_password();
    let mut ws = connect(&srv);
    let id = log_in_session(&mut ws, "alice");
    let outbox = srv.shared.sessions.outbox(id).expect("logged in");
    let msg = ServerMsg::LobbyError {
        text: "from another task".into(),
    };
    outbox
        .push(wire::Encoder::default().frame_msg(&msg).unwrap())
        .unwrap();
    assert_eq!(recv(&mut ws), msg);
    drop(ws);
    wait_until("session removed", || {
        srv.shared.sessions.outbox(id).is_none()
    });
}

#[test]
fn slow_reader_is_closed_with_1013() {
    use tungstenite::protocol::frame::coding::CloseCode;
    let srv = TestServer::with_password();
    let mut ws = connect(&srv);
    let id = log_in_session(&mut ws, "alice");
    let outbox = srv.shared.sessions.outbox(id).expect("logged in");

    // The client does not read. The socket buffers fill, then the 4 MiB queue does.
    let mut overflowed = false;
    for _ in 0..400 {
        if outbox.push(vec![0u8; 1 << 20]).is_err() {
            overflowed = true;
            break;
        }
    }
    assert!(overflowed, "400 MiB never filled the queue");

    set_timeout(&mut ws, Duration::from_secs(15));
    let code = loop {
        match ws.read() {
            Ok(Message::Close(frame)) => break frame.map(|f| f.code),
            Ok(_) => {}
            Err(e) => panic!("no close frame: {e}"),
        }
    };
    assert_eq!(code, Some(CloseCode::Again));
    wait_until("session gone", || {
        count(&srv.shared.metrics.sessions_open) == 0
    });
    assert_eq!(count(&srv.shared.metrics.sessions_slow_consumer_total), 1);
}

#[test]
fn choices_are_the_ones_the_gateway_holds() {
    let srv = TestServer::with_password();
    srv.shared.set_choices(protocol::LobbyChoices {
        hash: 42,
        ..Default::default()
    });
    let mut ws = connect(&srv);
    send(&mut ws, &hello(PROTOCOL_VERSION, "alice"));
    send(&mut ws, &login(PASSWORD, None));
    assert!(matches!(recv(&mut ws), ServerMsg::Welcome { .. }));
    assert!(matches!(recv(&mut ws), ServerMsg::LoggedIn(_)));
    match recv(&mut ws) {
        ServerMsg::Choices(c) => assert_eq!(c.hash, 42),
        other => panic!("expected Choices, got {other:?}"),
    }
    // The only one online is alice herself, in the lobby.
    assert_eq!(
        recv(&mut ws),
        ServerMsg::LobbyState(protocol::LobbyState {
            online: vec![protocol::OnlinePlayer {
                nick: "alice".into(),
                place: protocol::PlayerPlace::Lobby,
            }],
            ..protocol::LobbyState::default()
        })
    );
}

#[test]
fn lobby_commands_are_rate_limited() {
    let clock = Arc::new(server::clock::TestClock::new());
    let srv = TestServer::start(ServerConfig {
        clock: clock.clone(),
        ..config()
    });
    let mut ws = connect(&srv);
    log_in(&mut ws, "alice", PASSWORD, None);
    let lobby = ClientMsg::Lobby(protocol::LobbyCmd::LeaveRoom);
    let answer = |ws: &mut Ws| match recv(ws) {
        ServerMsg::LobbyError { text } => text,
        other => panic!("expected LobbyError, got {other:?}"),
    };
    // Ten a second: the eleventh in the same instant is over the limit.
    for _ in 0..10 {
        send(&mut ws, &lobby);
        assert!(!answer(&mut ws).contains("too many"));
    }
    send(&mut ws, &lobby);
    assert_eq!(answer(&mut ws), "too many lobby commands");
    // The session lives, and a second later the bucket has filled again.
    clock.advance(1000);
    send(&mut ws, &lobby);
    assert!(!answer(&mut ws).contains("too many"));
}

#[test]
fn wrong_password_answer_is_held_back() {
    let srv = TestServer::start(ServerConfig {
        limits: LimitsConfig {
            password_delay: Duration::from_millis(400),
            ..LimitsConfig::default()
        },
        ..config()
    });
    let mut ws = connect(&srv);
    let started = std::time::Instant::now();
    let r = log_in_rejected(&mut ws, "alice", "wrong-password", None);
    assert_eq!(r, RejectReason::BadPassword);
    assert!(started.elapsed() >= Duration::from_millis(400));
    // A right password is not delayed.
    let mut ws = connect(&srv);
    let started = std::time::Instant::now();
    log_in(&mut ws, "alice", PASSWORD, None);
    assert!(started.elapsed() < Duration::from_millis(400));
}

fn blocking_config(clock: &Arc<server::clock::TestClock>) -> ServerConfig {
    ServerConfig {
        clock: clock.clone(),
        limits: LimitsConfig {
            password_delay: Duration::from_millis(1),
            password_block_fails: 3,
            ..LimitsConfig::default()
        },
        ..config()
    }
}

#[test]
fn repeated_wrong_passwords_block_the_address() {
    let clock = Arc::new(server::clock::TestClock::new());
    let srv = TestServer::start(blocking_config(&clock));
    // The token of an earlier login is not a password guess.
    let mut first = connect(&srv);
    let token = log_in(&mut first, "alice", PASSWORD, None);
    drop(first);

    for _ in 0..3 {
        let mut ws = connect(&srv);
        let r = log_in_rejected(&mut ws, "bob", "wrong-password", None);
        assert_eq!(r, RejectReason::BadPassword);
    }
    // Blocked: even the right password is refused...
    let mut ws = connect(&srv);
    assert_eq!(
        log_in_rejected(&mut ws, "bob", PASSWORD, None),
        RejectReason::BadPassword
    );
    // ...but a token still opens the door.
    let mut ws = connect(&srv);
    assert_eq!(log_in(&mut ws, "alice", "", Some(token)), token);
    // Ten minutes later the block is over.
    clock.advance(10 * 60 * 1000);
    let mut ws = connect(&srv);
    log_in(&mut ws, "bob", PASSWORD, None);
}

fn connect_from(srv: &TestServer, forwarded_for: &str) -> Ws {
    use tungstenite::client::IntoClientRequest;
    let mut req = format!("ws://{}/ws", srv.addr)
        .into_client_request()
        .unwrap();
    req.headers_mut()
        .insert("x-forwarded-for", forwarded_for.parse().unwrap());
    let (mut ws, _) = tungstenite::connect(req).unwrap();
    set_timeout(&mut ws, Duration::from_secs(5));
    ws
}

#[test]
fn forwarded_for_names_the_client_only_behind_a_trusted_proxy() {
    let clock = Arc::new(server::clock::TestClock::new());
    let block_after_two = |trusted: Vec<server::config::Cidr>| ServerConfig {
        trusted_proxies: trusted,
        limits: LimitsConfig {
            password_block_fails: 2,
            ..blocking_config(&clock).limits
        },
        ..blocking_config(&clock)
    };

    // Behind a trusted proxy each forwarded address has its own count.
    let srv = TestServer::start(block_after_two(vec!["127.0.0.1".parse().unwrap()]));
    for _ in 0..2 {
        let mut ws = connect_from(&srv, "9.9.9.9");
        log_in_rejected(&mut ws, "bob", "wrong-password", None);
    }
    let mut ws = connect_from(&srv, "9.9.9.9");
    assert_eq!(
        log_in_rejected(&mut ws, "bob", PASSWORD, None),
        RejectReason::BadPassword
    );
    let mut ws = connect_from(&srv, "9.9.9.8");
    log_in(&mut ws, "bob", PASSWORD, None);

    // Without a trusted proxy the header is ignored: one address, one count.
    let srv = TestServer::start(block_after_two(Vec::new()));
    for i in 0..2 {
        let mut ws = connect_from(&srv, &format!("8.8.8.{i}"));
        log_in_rejected(&mut ws, "bob", "wrong-password", None);
    }
    let mut ws = connect_from(&srv, "8.8.8.200");
    assert_eq!(
        log_in_rejected(&mut ws, "bob", PASSWORD, None),
        RejectReason::BadPassword
    );
}

#[test]
fn unfinished_connections_per_address_are_capped() {
    use tungstenite::protocol::frame::coding::CloseCode;
    let srv = TestServer::with_password();
    let idle: Vec<Ws> = (0..4).map(|_| connect(&srv)).collect();
    wait_until("four sessions open", || {
        count(&srv.shared.metrics.sessions_open) == 4
    });

    let mut fifth = connect(&srv);
    match fifth.read().unwrap() {
        Message::Close(Some(frame)) => assert_eq!(frame.code, CloseCode::Again),
        other => panic!("expected Close(1013), got {other:?}"),
    }
    // The refused socket was never a session.
    assert_eq!(count(&srv.shared.metrics.sessions_opened_total), 4);

    // A login ends the "unfinished" state: that socket no longer counts.
    let mut idle = idle;
    let mut first = idle.remove(0);
    log_in(&mut first, "alice", PASSWORD, None);
    let mut again = connect(&srv);
    log_in(&mut again, "bob", PASSWORD, None);
    drop(idle);
}
