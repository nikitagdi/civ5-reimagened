//! The account of a bot: before its first WebSocket login a bot has no token, and a server without
//! `auto_register` knows no password login of an unknown nick. So the bot makes the account over HTTP, as a page does:
//! `POST /api/auth/register` with the registration code (the bot's `password`, `--password-file`), and when the nick
//! exists already `POST /api/auth/login` with the account password the bot derives from its nick. Either answers with
//! the `civ5_session` cookie (`<nick>.<32 hex digits>`, HttpOnly for a page, but a bot reads the header): the digits are
//! the device token for the WebSocket `Login` (a bot is a native-style client and keeps its token).
//! On any other answer (an old server, a dev server that makes the account itself at the password login, a rate limit)
//! the bot goes on as before and logs in over the WebSocket with its `password`.
//!
//! The request goes to `host:port` of the `ws://` URL. For an https server the runner puts a local plain front in
//! between (`runner/lib/tls-front.mjs`: it rewrites `Host` and passes the bytes through TLS), so the same code serves
//! http and https hosts.
//!
//! SIMPLIFIED(SIMP-exe-859): a bot reads the device token out of the `Set-Cookie` header and logs in over the
//! WebSocket with it, as a native client does; it does not send the cookie (a cookie login gives the client no token to
//! keep across a reconnect without a jar).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use protocol::Token;
use serde_json::{Value, json};

use super::token_from_hex;

/// `ws://host:port/ws` -> `host:port`. Only plain `ws://` (the gateway has no TLS, NT-D1).
pub fn http_host(ws_url: &str) -> Option<String> {
    let rest = ws_url.strip_prefix("ws://")?;
    Some(rest.split('/').next()?.to_owned()).filter(|h| !h.is_empty())
}

/// The account password of the bot `nick` (a server wants 8 characters or more). A test account; the nick is
/// public anyway.
pub fn account_password(nick: &str) -> String {
    format!("net-bot:{nick}:account")
}

struct Reply {
    status: u16,
    /// The value of each `Set-Cookie` header (up to the first `;`), as `name=value`.
    cookies: Vec<String>,
}

fn post(host: &str, path: &str, body: &Value) -> std::io::Result<Reply> {
    let mut s = TcpStream::connect(host)?;
    s.set_read_timeout(Some(Duration::from_secs(15)))?;
    s.set_write_timeout(Some(Duration::from_secs(15)))?;
    let body = body.to_string();
    write!(
        s,
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )?;
    let mut raw = Vec::new();
    // A reset after the answer (the peer closes with bytes unread) still leaves the answer in `raw`.
    let _ = s.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw);
    let head = text.split("\r\n\r\n").next().unwrap_or_default();
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let cookies = head
        .lines()
        .skip(1)
        .filter_map(|l| {
            let (name, value) = l.split_once(':')?;
            name.trim().eq_ignore_ascii_case("set-cookie").then(|| {
                value
                    .trim()
                    .split(';')
                    .next()
                    .unwrap_or_default()
                    .to_owned()
            })
        })
        .collect();
    Ok(Reply { status, cookies })
}

/// The device token in the session cookie: `civ5_session=<percent-encoded nick>.<32 hex digits>`.
fn token_of(r: &Reply) -> Option<Token> {
    if r.status != 200 {
        return None;
    }
    r.cookies
        .iter()
        .find_map(|c| c.strip_prefix("civ5_session="))
        .and_then(|v| token_from_hex(v.rsplit_once('.')?.1))
}

/// A device token for `nick`: registers the account with `code`, or logs in when it exists. `None`: the server did
/// not give one (the caller logs in over the WebSocket with its password as before).
pub fn token_for(ws_url: &str, nick: &str, code: &str) -> Option<Token> {
    let host = http_host(ws_url)?;
    let password = account_password(nick);
    let reg = post(
        &host,
        "/api/auth/register",
        &json!({ "username": nick, "password": password, "code": code }),
    )
    .ok()?;
    if let Some(t) = token_of(&reg) {
        return Some(t);
    }
    if reg.status != 409 {
        return None;
    }
    let login = post(
        &host,
        "/api/auth/login",
        &json!({ "username": nick, "password": password }),
    )
    .ok()?;
    token_of(&login)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// A server that answers each request with the next canned `(status, body)` and records the requests.
    fn fake(answers: Vec<(u16, &'static str)>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let host = l.local_addr().unwrap().to_string();
        let h = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for (status, body) in answers {
                let (mut s, _) = l.accept().unwrap();
                let mut buf = [0u8; 4096];
                let mut got = Vec::new();
                loop {
                    let n = s.read(&mut buf).unwrap();
                    got.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&got).into_owned();
                    if let Some((head, rest)) = text.split_once("\r\n\r\n") {
                        let len = head
                            .lines()
                            .find_map(|l| l.strip_prefix("Content-Length: "))
                            .and_then(|v| v.parse::<usize>().ok())
                            .unwrap_or(0);
                        if rest.len() >= len || n == 0 {
                            break;
                        }
                    } else if n == 0 {
                        break;
                    }
                }
                seen.push(String::from_utf8_lossy(&got).into_owned());
                write!(s, "HTTP/1.1 {status} X\r\n{body}Content-Length: 2\r\nConnection: close\r\n\r\n{{}}").unwrap();
            }
            seen
        });
        (host, h)
    }

    /// The header block of a good answer: the cookie of the device (the body has no token).
    const TOKEN: &str = "Set-Cookie: civ5_session=bot.0123456789abcdef0123456789abcdef; Path=/; HttpOnly; SameSite=Lax; Secure; Max-Age=7776000\r\n";

    #[test]
    fn the_host_comes_from_a_plain_ws_url() {
        assert_eq!(
            http_host("ws://127.0.0.1:8080/ws").as_deref(),
            Some("127.0.0.1:8080")
        );
        assert_eq!(http_host("wss://x/ws"), None);
        assert_eq!(http_host("ws:///ws"), None);
    }

    #[test]
    fn a_new_nick_registers_with_the_code() {
        let (host, h) = fake(vec![(200, TOKEN)]);
        let t = token_for(&format!("ws://{host}/ws"), "bot", "the-code").unwrap();
        assert_eq!(t.0[0], 0x01);
        let seen = h.join().unwrap();
        assert!(
            seen[0].starts_with("POST /api/auth/register HTTP/1.1"),
            "{}",
            seen[0]
        );
        assert!(
            seen[0].contains(r#""code":"the-code""#) && seen[0].contains(r#""username":"bot""#)
        );
        assert!(seen[0].contains("net-bot:bot:account"));
    }

    #[test]
    fn an_existing_nick_logs_in_with_the_account_password() {
        let (host, h) = fake(vec![(409, ""), (200, TOKEN)]);
        assert!(token_for(&format!("ws://{host}/ws"), "bot", "c").is_some());
        let seen = h.join().unwrap();
        assert!(seen[1].starts_with("POST /api/auth/login HTTP/1.1"));
        assert!(!seen[1].contains("\"code\""));
    }

    #[test]
    fn a_good_answer_without_the_cookie_gives_no_token() {
        // An old server answered with a token in the body; this one must send the cookie.
        let (host, h) = fake(vec![(200, "")]);
        assert!(token_for(&format!("ws://{host}/ws"), "bot", "c").is_none());
        h.join().unwrap();
        let r = Reply {
            status: 200,
            cookies: vec!["civ5_session=Jr%C3%BCgen.0123456789abcdef0123456789abcdef".into()],
        };
        assert_eq!(token_of(&r).unwrap().0[0], 0x01);
    }

    #[test]
    fn other_answers_leave_the_bot_to_its_websocket_login() {
        // A wrong code, a rate limit, an old server without the route: no token, no second request.
        for status in [400, 429, 404] {
            let (host, h) = fake(vec![(status, "")]);
            assert!(token_for(&format!("ws://{host}/ws"), "bot", "c").is_none());
            assert_eq!(h.join().unwrap().len(), 1);
        }
        // Nothing listens.
        assert!(token_for("ws://127.0.0.1:1/ws", "bot", "c").is_none());
        // A login that fails.
        let (host, h) = fake(vec![(409, ""), (401, "")]);
        assert!(token_for(&format!("ws://{host}/ws"), "bot", "c").is_none());
        h.join().unwrap();
    }
}
