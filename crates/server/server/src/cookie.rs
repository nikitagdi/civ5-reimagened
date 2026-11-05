//! The session cookie (round 2): `civ5_session=<nick, percent-encoded>.<32 hex digits of the device
//! token>; Path=/; HttpOnly; SameSite=Lax; Max-Age=7776000[; Secure]`.
//!
//! The web page never sees the value (HttpOnly); the browser sends it with the `/ws` upgrade and the auth POSTs. The
//! nick is percent-encoded down to `[A-Za-z0-9_~-]`, so the last `.` always separates the token.
//!
//! SIMPLIFIED(SIMP-exe-854): the original game has no accounts; the web client's device token lives in an HttpOnly
//! cookie that script cannot read, and the WebSocket `Login` of such a session carries no token at all.

use axum::http::{HeaderMap, header};
use protocol::Token;

use crate::identity::{token_from_hex, token_to_hex};

/// The cookie's name.
pub const NAME: &str = "civ5_session";

/// 90 days, the lifetime of a device token (`identity::TOKEN_TTL_MS`).
pub const MAX_AGE_S: u64 = 90 * 24 * 3600;

fn encode_nick(nick: &str) -> String {
    let mut out = String::with_capacity(nick.len());
    for b in nick.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn decode_nick(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            out.push(u8::from_str_radix(s.get(i + 1..i + 3)?, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The cookie value for a device of `nick`.
pub fn value(nick: &str, token: &Token) -> String {
    format!("{}.{}", encode_nick(nick), token_to_hex(token))
}

/// Splits a cookie value into the nick and the device token.
pub fn parse_value(v: &str) -> Option<(String, Token)> {
    let (nick, hex) = v.rsplit_once('.')?;
    Some((decode_nick(nick)?, token_from_hex(hex)?))
}

fn attrs(secure: bool) -> String {
    format!(
        "Path=/; HttpOnly; SameSite=Lax{}",
        if secure { "; Secure" } else { "" }
    )
}

/// The `Set-Cookie` header that logs a device in.
pub fn set_header(nick: &str, token: &Token, secure: bool) -> String {
    format!(
        "{NAME}={}; {}; Max-Age={MAX_AGE_S}",
        value(nick, token),
        attrs(secure)
    )
}

/// The `Set-Cookie` header that removes the cookie.
pub fn clear_header(secure: bool) -> String {
    format!("{NAME}=; {}; Max-Age=0", attrs(secure))
}

/// The session cookie of a request: `(nick, token)`; `None` if absent or malformed.
pub fn from_headers(headers: &HeaderMap) -> Option<(String, Token)> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().strip_prefix(NAME)?.strip_prefix('='))
        .find_map(parse_value)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: Token = Token([0xab; 16]);

    #[test]
    fn the_value_round_trips_odd_nicks() {
        for nick in ["alice", "Jürgen B", "a.b.c", "x;y=z%", "名前", "a+b"] {
            let v = value(nick, &TOKEN);
            assert!(
                v.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_~-.%".contains(&b)),
                "{v}"
            );
            assert_eq!(parse_value(&v), Some((nick.to_owned(), TOKEN)), "{nick}");
        }
        assert_eq!(parse_value("nick"), None);
        assert_eq!(parse_value("nick.zz"), None);
        assert_eq!(parse_value("%zz.abababababababababababababababab"), None);
    }

    #[test]
    fn the_headers_carry_the_attributes() {
        let h = set_header("al ice", &TOKEN, true);
        assert_eq!(
            h,
            "civ5_session=al%20ice.abababababababababababababababab; Path=/; HttpOnly; SameSite=Lax; Secure; Max-Age=7776000"
        );
        assert!(!set_header("a", &TOKEN, false).contains("Secure"));
        assert_eq!(
            clear_header(true),
            "civ5_session=; Path=/; HttpOnly; SameSite=Lax; Secure; Max-Age=0"
        );
    }

    #[test]
    fn the_cookie_is_found_among_others() {
        let mut h = HeaderMap::new();
        h.append(header::COOKIE, "a=1; civ5_session2=x".parse().unwrap());
        assert!(from_headers(&h).is_none());
        h.append(
            header::COOKIE,
            "b=2; civ5_session=bob.abababababababababababababababab; c=3"
                .parse()
                .unwrap(),
        );
        assert_eq!(from_headers(&h), Some(("bob".to_owned(), TOKEN)));
    }
}
