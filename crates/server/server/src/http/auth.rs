//! Account routes: `GET /api/auth/info`, `POST /api/auth/{register,login,adopt,logout}`.
//!
//! The body is form-urlencoded or JSON (`username`, `password`, `code`, `legacy_token`, `token`), at most 4 KiB. A
//! request with an `Origin` that is not the page's own host is refused (CSRF on login; `SameSite=Lax` on the cookie is
//! the second layer). Every answer carries `Cache-Control: no-store`.
//!
//! The device token never reaches script: login, register and adopt answer with `Set-Cookie: civ5_session=...;
//! HttpOnly` ([`crate::cookie`]) and a JSON body of `{nick}` only. The WebSocket upgrade reads the cookie.
//!
//! | Route | Answer |
//! |---|---|
//! | `GET /api/auth/info` | `{registration: "open"\|"code", min_password: 8}` |
//! | `POST /api/auth/register` | 200 `{nick}` + cookie; 400 `{error, field}`; 403 origin; 409 taken or held by a game; 429 |
//! | `POST /api/auth/login` | 200 `{nick}` + cookie; 401 `{error: "Wrong nick or password."}`; 429 |
//! | `POST /api/auth/adopt` | `username, token` of an old `localStorage` login: 200 `{nick}` + cookie; 401 |
//! | `POST /api/auth/logout` | 204 + an expiring cookie (drops the device of the cookie) |

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};

use super::client_ip;
use crate::Shared;
use crate::auth::{self, PwFail};
use crate::cookie;
use crate::identity::{
    self, MAX_PASSWORD_CHARS, MIN_PASSWORD_CHARS, PwState, RegisterError, device_label,
    normalize_nick, token_from_hex,
};
use protocol::Token;

// SIMPLIFIED(SIMP-exe-856): no CSRF token. The auth POSTs and the WebSocket upgrade are guarded by `SameSite=Lax` on the
// cookie and by the `Origin` check (a present `Origin` must be the page's host); a request without `Origin` (curl, bots,
// native clients) is allowed, which a browser never sends for a cross-site POST or upgrade.
// SIMPLIFIED(SIMP-exe-857): `POST /api/auth/adopt` turns the device token that an earlier build left in `localStorage`
// into the cookie, once per browser; remove it with the migration.

/// The body of an auth request is at most this big.
pub const MAX_BODY: usize = 4096;

const WRONG: &str = "Wrong nick or password.";
const SEAT_HELD: &str =
    "This nick has a seat in a game. Open the game in the browser you played in, or ask the admin.";
const IN_USE: &str =
    "This nick is in use right now. Open the game in the browser you played in, or ask the admin.";

/// The fields of a request. No `Debug`: it holds a password.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Body {
    username: String,
    password: String,
    code: Option<String>,
    legacy_token: Option<String>,
    token: Option<String>,
}

fn reply(status: StatusCode, body: Value) -> Response {
    (status, [(header::CACHE_CONTROL, "no-store")], Json(body)).into_response()
}

/// 200 `{nick}` with the session cookie of the device.
fn signed_in(shared: &Shared, nick: &str, token: &Token) -> Response {
    let cookie = cookie::set_header(nick, token, shared.config.cookie_secure);
    (
        StatusCode::OK,
        [
            (header::CACHE_CONTROL, "no-store".to_owned()),
            (header::SET_COOKIE, cookie),
        ],
        Json(json!({ "nick": nick })),
    )
        .into_response()
}

fn bad(field: &str, text: &str) -> Response {
    reply(
        StatusCode::BAD_REQUEST,
        json!({ "error": text, "field": field }),
    )
}

/// 429 with `Retry-After` (whole seconds, at least 1).
fn limited(left: Duration) -> Response {
    let secs = left.as_secs() + u64::from(left.subsec_nanos() > 0);
    (
        StatusCode::TOO_MANY_REQUESTS,
        [
            (header::CACHE_CONTROL, "no-store".to_owned()),
            (header::RETRY_AFTER, secs.max(1).to_string()),
        ],
        Json(json!({ "error": "Too many attempts. Try again later." })),
    )
        .into_response()
}

/// A request that names an `Origin` must come from the page's own host (`Host`, or `X-Forwarded-Host` behind a proxy).
/// No `Origin` (curl, bots): allowed, a browser always sends one on a cross-site POST.
pub fn origin_ok(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    let rest = origin.split_once("://").map_or(origin, |(_, r)| r);
    let host = rest.split('/').next().unwrap_or_default();
    if host.is_empty() {
        return false;
    }
    [header::HOST.as_str(), "x-forwarded-host"]
        .into_iter()
        .flat_map(|name| headers.get_all(name).iter())
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|h| h.trim().eq_ignore_ascii_case(host))
}

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' => {
                let hex = s.get(i + 1..i + 3)?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 2;
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8(out).ok()
}

fn parse_form(body: &[u8]) -> Option<Body> {
    let text = std::str::from_utf8(body).ok()?;
    let mut b = Body::default();
    for pair in text.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let (k, v) = (percent_decode(k)?, percent_decode(v)?);
        match k.as_str() {
            "username" => b.username = v,
            "password" => b.password = v,
            "code" => b.code = Some(v),
            "legacy_token" => b.legacy_token = Some(v),
            "token" => b.token = Some(v),
            _ => {}
        }
    }
    Some(b)
}

#[allow(clippy::result_large_err)] // the Err is the answer itself, used once and sent
fn parse_body(headers: &HeaderMap, body: &[u8]) -> Result<Body, Response> {
    let json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|t| t.to_ascii_lowercase().contains("json"));
    let parsed = if json {
        serde_json::from_slice(body).ok()
    } else {
        parse_form(body)
    };
    parsed.ok_or_else(|| {
        reply(
            StatusCode::BAD_REQUEST,
            json!({ "error": "Unreadable request." }),
        )
    })
}

/// Origin and body of a POST; the address for the limits.
#[allow(clippy::result_large_err)] // the Err is the answer itself, used once and sent
fn open(
    shared: &Shared,
    peer: SocketAddr,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<(std::net::IpAddr, Body), Response> {
    if !origin_ok(headers) {
        return Err(reply(
            StatusCode::FORBIDDEN,
            json!({ "error": "Cross-origin request refused." }),
        ));
    }
    let ip = client_ip(peer.ip(), headers, &shared.config.trusted_proxies);
    Ok((ip, parse_body(headers, body)?))
}

fn user_agent(headers: &HeaderMap) -> String {
    device_label(
        headers
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default(),
    )
}

pub async fn info(State(shared): State<Arc<Shared>>) -> Response {
    reply(
        StatusCode::OK,
        json!({ "registration": auth::registration_mode(&shared), "min_password": MIN_PASSWORD_CHARS }),
    )
}

pub async fn register(
    State(shared): State<Arc<Shared>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let (ip, b) = match open(&shared, peer, &headers, &body) {
        Ok(x) => x,
        Err(r) => return r,
    };
    let now = shared.config.clock.now_ms();
    if let Some(left) = shared.gate.blocked_for(ip, now) {
        return limited(left);
    }
    let Some(nick) = normalize_nick(&b.username) else {
        return bad(
            "username",
            "Nick must be 1 to 24 characters, with no control characters.",
        );
    };
    if !identity::password_ok(&b.password) {
        return bad(
            "password",
            &format!("Password must be {MIN_PASSWORD_CHARS} to {MAX_PASSWORD_CHARS} characters."),
        );
    }
    if !auth::code_ok(&shared, b.code.as_deref().unwrap_or_default()) {
        // A wrong code counts like a wrong password: the delay and the block of the address.
        let delay = shared.gate.record_password_failure(ip, now);
        tokio::time::sleep(delay).await;
        return bad("code", "Wrong registration code.");
    }
    if matches!(shared.identity.password_state(&nick), PwState::Hash(_)) {
        return reply(
            StatusCode::CONFLICT,
            json!({ "error": "This nick is taken.", "field": "username" }),
        );
    }
    if let Err(left) = shared.gate.register_ok(ip, now) {
        tracing::info!(%ip, "registration limit");
        return limited(left);
    }
    // The old token of a legacy nick: sent by an old page, or the session cookie of that nick (an adopted one).
    let legacy = b
        .legacy_token
        .as_deref()
        .and_then(token_from_hex)
        .or_else(|| {
            cookie::from_headers(&headers)
                .filter(|(n, _)| identity::nick_key(n) == identity::nick_key(&nick))
                .map(|(_, t)| t)
        });
    let Some(hash) = shared.hasher.hash(b.password.clone()).await else {
        return reply(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Cannot register right now." }),
        );
    };
    match shared
        .identity
        .register(&nick, hash, legacy.as_ref(), &user_agent(&headers))
    {
        Ok(r) => {
            tracing::info!(nick = %r.nick, %ip, claimed = legacy.is_some(), "registered");
            signed_in(&shared, &r.nick, &r.token)
        }
        Err(RegisterError::Taken) => reply(
            StatusCode::CONFLICT,
            json!({ "error": "This nick is taken.", "field": "username" }),
        ),
        Err(RegisterError::SeatHeld) => reply(
            StatusCode::CONFLICT,
            json!({ "error": SEAT_HELD, "field": "username" }),
        ),
        Err(RegisterError::InUse) => reply(
            StatusCode::CONFLICT,
            json!({ "error": IN_USE, "field": "username" }),
        ),
        Err(RegisterError::Internal) => reply(
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({ "error": "Cannot register right now." }),
        ),
    }
}

pub async fn login(
    State(shared): State<Arc<Shared>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let (ip, b) = match open(&shared, peer, &headers, &body) {
        Ok(x) => x,
        Err(r) => return r,
    };
    let wrong = || reply(StatusCode::UNAUTHORIZED, json!({ "error": WRONG }));
    let now = shared.config.clock.now_ms();
    if let Some(left) = shared.gate.blocked_for(ip, now) {
        return limited(left);
    }
    let Some(nick) = normalize_nick(&b.username) else {
        // Same answer, same delay, as for a nick that has no account.
        let delay = shared.gate.record_password_failure(ip, now);
        tokio::time::sleep(delay).await;
        return wrong();
    };
    match auth::check_password(&shared, ip, &nick, &b.password).await {
        Ok(hash) => match shared
            .identity
            .login_http(&nick, &hash, &user_agent(&headers))
        {
            Ok(r) => signed_in(&shared, &r.nick, &r.token),
            Err(_) => wrong(),
        },
        Err(PwFail::Blocked(left)) => limited(left),
        Err(PwFail::Wrong) => wrong(),
    }
}

/// An old page's `localStorage` login (`username`, `token`) becomes the session cookie.
pub async fn adopt(
    State(shared): State<Arc<Shared>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let (ip, b) = match open(&shared, peer, &headers, &body) {
        Ok(x) => x,
        Err(r) => return r,
    };
    let now = shared.config.clock.now_ms();
    if let Some(left) = shared.gate.blocked_for(ip, now) {
        return limited(left);
    }
    let token = b.token.as_deref().and_then(token_from_hex);
    if let (Some(nick), Some(token)) = (normalize_nick(&b.username), token)
        && let Some(shown) = shared.identity.adopt(&nick, &token)
    {
        tracing::info!(nick = %shown, %ip, "old token adopted into the session cookie");
        return signed_in(&shared, &shown, &token);
    }
    let delay = shared.gate.record_password_failure(ip, now);
    tokio::time::sleep(delay).await;
    reply(
        StatusCode::UNAUTHORIZED,
        json!({ "error": "Sign in again." }),
    )
}

pub async fn logout(
    State(shared): State<Arc<Shared>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(r) = open(&shared, peer, &headers, &body) {
        return r;
    }
    // 204 whether or not there was a cookie: the answer tells nothing. The cookie goes in any case.
    if let Some((nick, token)) = cookie::from_headers(&headers) {
        shared.identity.logout(&nick, &token);
    }
    (
        StatusCode::NO_CONTENT,
        [
            (header::CACHE_CONTROL, "no-store".to_owned()),
            (
                header::SET_COOKIE,
                cookie::clear_header(shared.config.cookie_secure),
            ),
        ],
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        h
    }

    #[test]
    fn origin_must_be_the_pages_own_host() {
        let host = "civ5.example.com";
        // No Origin: curl and the bots.
        assert!(origin_ok(&headers(&[("host", host)])));
        assert!(origin_ok(&headers(&[
            ("host", host),
            ("origin", "https://civ5.example.com")
        ])));
        assert!(origin_ok(&headers(&[
            ("host", "127.0.0.1:8080"),
            ("origin", "http://127.0.0.1:8080")
        ])));
        assert!(origin_ok(&headers(&[
            ("host", "CIV5.example.com"),
            ("origin", "https://civ5.EXAMPLE.com")
        ])));
        // A proxy that rewrites Host: the forwarded one counts.
        assert!(origin_ok(&headers(&[
            ("host", "app:8080"),
            ("x-forwarded-host", "civ5.example.com"),
            ("origin", "https://civ5.example.com")
        ])));
        for bad in [
            "https://evil.example",
            "https://civ5.example.com.evil.example",
            "null",
            "",
            "http://civ5.example.com:81",
        ] {
            assert!(
                !origin_ok(&headers(&[("host", host), ("origin", bad)])),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn form_bodies_are_percent_decoded() {
        let b = parse_form(
            b"username=J%C3%BCrgen+B&password=p%40ss%26w%3Drd&code=&x=1&legacy_token=ab",
        )
        .unwrap();
        assert_eq!(b.username, "Jürgen B");
        assert_eq!(b.password, "p@ss&w=rd");
        assert_eq!(b.code.as_deref(), Some(""));
        assert_eq!(b.legacy_token.as_deref(), Some("ab"));
        assert!(b.token.is_none());
        assert!(parse_form(b"username=%zz").is_none());
        assert!(parse_form(b"username=%C3").is_none(), "cut UTF-8");
        assert!(parse_form(&[0xff, 0xfe]).is_none());
        assert_eq!(parse_form(b"").unwrap().username, "");
    }

    #[test]
    fn json_bodies_and_the_content_type_pick_the_parser() {
        let json = headers(&[("content-type", "application/json; charset=utf-8")]);
        let b = parse_body(&json, br#"{"username":"a","password":"b","code":"c"}"#)
            .ok()
            .unwrap();
        assert_eq!((b.username.as_str(), b.code.as_deref()), ("a", Some("c")));
        assert!(parse_body(&json, b"username=a").is_err());
        assert!(parse_body(&json, br#"{"username":5}"#).is_err());
        let form = headers(&[("content-type", "application/x-www-form-urlencoded")]);
        assert!(parse_body(&form, b"username=a&password=b").is_ok());
        assert!(parse_body(&HeaderMap::new(), b"username=a").is_ok());
    }

    #[test]
    fn retry_after_rounds_up_to_whole_seconds() {
        let r = limited(Duration::from_millis(1200));
        assert_eq!(r.headers()[header::RETRY_AFTER], "2");
        let r = limited(Duration::ZERO);
        assert_eq!(r.headers()[header::RETRY_AFTER], "1");
        assert_eq!(r.headers()[header::CACHE_CONTROL], "no-store");
    }
}
