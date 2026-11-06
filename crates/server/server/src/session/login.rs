//! `Hello` + `Login` (NP 4.12): version, nick, device token or password. Ends in
//! `LoggedIn` or in a `Reject` and a close.
//!
//! A valid device token logs in. Otherwise the password is checked against the account's argon2 hash; a nick without an
//! account gets `BadPassword` (no account enumeration), except on a server with `auto_register` (dev and tests), where
//! the first password login creates the account. A successful login replaces the account's live sessions.
//!
//! The web page logs in with the HttpOnly session cookie of the upgrade request (`cookie`): a `Login` without a token and
//! with an empty password, for the nick of the cookie, uses the cookie's device token, and `LoggedIn` carries no token
//! (the page never holds one, protocol v9).

use std::net::IpAddr;

use protocol::{Codec, Postcard, Token};
use tokio::sync::oneshot;

use super::{Conn, Incoming, decode};
use crate::BUILD;
use crate::auth::{self, PwFail};
use crate::identity::{Grant, Proof, PwState, nick_key, normalize_nick};
use crate::metrics::Metrics;
use protocol::{ClientMsg, PROTOCOL_VERSION, RejectReason, ServerMsg, Session};

/// Runs the handshake on a fresh socket. `Ok` is the nick the session now holds; `Err` is the
/// reason to log when closing (a `Reject` has been sent where the protocol has one). `peer` is
/// the address for the password-failure limits; `label` names the device (from the `User-Agent`); `cookie` is
/// the session cookie of the upgrade request (nick and device token), if it had one.
pub(super) async fn handshake(
    conn: &mut Conn,
    kick_tx: oneshot::Sender<()>,
    peer: IpAddr,
    label: &str,
    cookie: Option<&(String, Token)>,
) -> Result<String, &'static str> {
    let first = match conn.recv().await {
        Incoming::Frame(data) => data,
        Incoming::End(reason) => return Err(reason),
    };
    let (protocol, client_build, raw_nick) = match decode(&first) {
        Ok(ClientMsg::Hello {
            protocol,
            build,
            nick,
        }) => (protocol, build, nick),
        _ => return Err(not_a_hello(conn, &first).await),
    };
    if protocol != PROTOCOL_VERSION {
        let reject = RejectReason::VersionMismatch {
            server: PROTOCOL_VERSION,
        };
        return Err(conn.reject(reject, "protocol version mismatch").await);
    }
    let open = Metrics::get(&conn.shared.metrics.sessions_open);
    if open > u64::from(conn.shared.config.max_sessions) {
        return Err(conn.reject(RejectReason::Full, "too many sessions").await);
    }
    let Some(nick) = normalize_nick(&raw_nick) else {
        return Err(conn.reject(RejectReason::BadNick, "invalid nick").await);
    };
    let welcome = ServerMsg::Welcome {
        session: conn.id,
        build: BUILD.to_owned(),
    };
    conn.send(&welcome).await?;

    // `ModHello` (v13) may come before `Login`: the login can attach the session to a running game at once, and that
    // game compares what the client has with its mods. Any number of them; the last one counts.
    let login = loop {
        let data = match conn.recv().await {
            Incoming::Frame(data) => data,
            Incoming::End(reason) => return Err(reason),
        };
        match decode(&data) {
            Ok(ClientMsg::Login(login)) => break login,
            Ok(ClientMsg::ModHello { api, built_in }) => {
                if !conn.take_mod_hello(api, built_in) {
                    return Err("bad ModHello");
                }
            }
            Ok(_) => return Err("expected Login"),
            Err(reason) => return Err(reason),
        }
    };

    conn.resume = login.resume;
    let shared = conn.shared.clone();
    let mut kick = Some(kick_tx);
    // A blocked address gets no password check at all, right or wrong; a token still works.
    let blocked = shared.gate.is_blocked(peer, shared.config.clock.now_ms());

    let mut result = Err(RejectReason::BadPassword);
    if let Some(token) = login.token.as_ref() {
        result = shared
            .identity
            .login(&nick, Proof::Token(token), label, conn.id, &mut kick);
    }
    // The cookie of the page: no token and no password in the message, and the cookie names this nick.
    let mut by_cookie = false;
    if result.is_err()
        && login.token.is_none()
        && login.password.is_empty()
        && let Some((cookie_nick, cookie_token)) = cookie
        && nick_key(cookie_nick) == nick_key(&nick)
    {
        result =
            shared
                .identity
                .login(&nick, Proof::Token(cookie_token), label, conn.id, &mut kick);
        by_cookie = result.is_ok();
    }
    if result.is_err() {
        result = password_login(
            &shared,
            peer,
            &nick,
            &login.password,
            label,
            conn.id,
            &mut kick,
            blocked,
        )
        .await;
    }
    let grant = match result {
        Ok(grant) => grant,
        Err(reason) => {
            tracing::info!(session = conn.id, %peer, nick = %nick, ?reason, blocked, "login refused");
            return Err(conn.reject(reason, "login refused").await);
        }
    };
    tracing::info!(
        session = conn.id,
        nick = %grant.nick,
        device = %format!("{:04x}", grant.device & 0xffff),
        token = %crate::identity::token_short(&grant.token),
        replaced = ?grant.replaced,
        cookie = by_cookie,
        client_build = %client_build,
        "login"
    );
    let logged_in = ServerMsg::LoggedIn(Session {
        // A cookie login hands the page no token.
        token: (!by_cookie).then_some(grant.token),
        nick: grant.nick.clone(),
    });
    // The nick is held from here on; the caller releases it when the session ends.
    if let Err(reason) = conn.send(&logged_in).await {
        shared.identity.unregister(&grant.nick, conn.id);
        return Err(reason);
    }
    Ok(grant.nick)
}

/// The password side of a login. The answers of a wrong password are held back by [`auth::check_password`]; the
/// connection stays counted as unfinished meanwhile.
#[allow(clippy::too_many_arguments)]
async fn password_login(
    shared: &crate::Shared,
    peer: IpAddr,
    nick: &str,
    password: &str,
    label: &str,
    session: u64,
    kick: &mut Option<oneshot::Sender<()>>,
    blocked: bool,
) -> Result<Grant, RejectReason> {
    if blocked {
        return Err(RejectReason::BadPassword);
    }
    // No password and no valid token or cookie: nothing to check (a password is 8 characters or more), so the refusal
    // costs no hash and counts no failure against the address (a page load without a cookie ends here). A dev or test
    // server (`auto_register`) goes on: its registration gate may take an empty code.
    if password.is_empty() && !shared.config.auto_register {
        return Err(RejectReason::BadPassword);
    }
    // SIMPLIFIED(SIMP-exe-843): dev and test servers create the account at the first password login, for bots and tests.
    if shared.config.auto_register
        && shared.identity.password_state(nick) == PwState::Unknown
        && auth::code_ok(shared, password)
    {
        let hash = shared
            .hasher
            .hash(password.to_owned())
            .await
            .ok_or(RejectReason::Full)?;
        // A refusal here means somebody registered the nick meanwhile: the normal check below decides.
        if let Ok(reg) = shared.identity.register(nick, hash, None, label) {
            return shared
                .identity
                .login(nick, Proof::Token(&reg.token), label, session, kick);
        }
    }
    match auth::check_password(shared, peer, nick, password).await {
        Ok(hash) => {
            shared
                .identity
                .login(nick, Proof::Password { hash: &hash }, label, session, kick)
        }
        Err(PwFail::Blocked(_) | PwFail::Wrong) => Err(RejectReason::BadPassword),
    }
}

/// The first message was not a v3 `Hello`. A v2 client (bare postcard, no frame byte) gets a
/// bare postcard v2 `Reject(VersionMismatch)` it can read (NP-D10); anything else is closed.
async fn not_a_hello(conn: &mut Conn, first: &[u8]) -> &'static str {
    if let Ok(protocol::ClientMsg::Hello { .. }) = Postcard.decode::<protocol::ClientMsg>(first) {
        let reject = protocol::ServerMsg::Reject(protocol::RejectReason::VersionMismatch {
            server: PROTOCOL_VERSION,
        });
        Metrics::bump(&conn.shared.metrics.login_failed_total);
        if let Ok(bytes) = Postcard.encode(&reject)
            && let Err(reason) = conn.send_raw(bytes).await
        {
            return reason;
        }
        return "protocol v2 client";
    }
    "expected Hello"
}
