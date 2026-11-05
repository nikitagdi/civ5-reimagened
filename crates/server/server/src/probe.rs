//! Thin local HTTP client for the `healthcheck` and `admin` subcommands.
//! Plain HTTP/1.0 over loopback: no chunked bodies, one request per connection.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, bail};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::config::Config;
use crate::state::read_admin_token;

/// Upper bound for a whole probe (connect, send, read).
const TIMEOUT: Duration = Duration::from_secs(5);
/// Responses larger than this are refused (metrics are a few hundred KiB at most).
const MAX_RESPONSE: usize = 16 * 1024 * 1024;

/// A decoded response.
#[derive(Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: String,
}

/// Sends one request and returns status and body.
pub async fn request(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> anyhow::Result<Response> {
    tokio::time::timeout(TIMEOUT, request_inner(addr, method, path, headers))
        .await
        .with_context(|| format!("{method} {path} on {addr}: timed out"))?
}

async fn request_inner(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> anyhow::Result<Response> {
    let mut stream = TcpStream::connect(addr)
        .await
        .with_context(|| format!("cannot connect to {addr}"))?;
    let mut req = format!("{method} {path} HTTP/1.0\r\nHost: {addr}\r\n");
    for (k, v) in headers {
        if k.contains(['\r', '\n']) || v.contains(['\r', '\n']) {
            bail!("header {k} has a line break");
        }
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    if method == "POST" {
        req.push_str("Content-Length: 0\r\n");
    }
    req.push_str("\r\n");
    stream.write_all(req.as_bytes()).await?;
    let mut raw = Vec::new();
    stream
        .take(MAX_RESPONSE as u64 + 1)
        .read_to_end(&mut raw)
        .await?;
    if raw.len() > MAX_RESPONSE {
        bail!("response is larger than {MAX_RESPONSE} bytes");
    }
    parse_response(&raw)
}

fn parse_response(raw: &[u8]) -> anyhow::Result<Response> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .context("malformed HTTP response: no header end")?;
    let head = std::str::from_utf8(&raw[..split]).context("non-UTF-8 response header")?;
    let status_line = head.lines().next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .with_context(|| format!("malformed status line {status_line:?}"))?;
    let body = String::from_utf8_lossy(&raw[split + 4..]).into_owned();
    Ok(Response { status, body })
}

/// `healthcheck`: true if `GET /healthz` answers 200.
pub async fn healthcheck(config: &Config) -> anyhow::Result<bool> {
    let r = request(config.probe_addr(), "GET", "/healthz", &[]).await?;
    Ok(r.status == 200)
}

/// `admin <method> <path>`: calls the admin API on loopback with the token from `state_dir`.
pub async fn admin(config: &Config, method: &str, path: &str) -> anyhow::Result<Response> {
    let token = read_admin_token(&config.state_dir.join("admin.token"))?;
    request(
        config.admin_listen,
        method,
        path,
        &[("X-Admin-Token", &token)],
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    /// Serves one canned response and returns the request text it saw.
    async fn one_shot(reply: &'static str) -> (SocketAddr, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let h = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let n = s.read(&mut buf).await.unwrap();
            s.write_all(reply.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&buf[..n]).into_owned()
        });
        (addr, h)
    }

    #[tokio::test]
    async fn parses_status_and_body_and_sends_headers() {
        let (addr, seen) = one_shot("HTTP/1.0 503 Service Unavailable\r\n\r\n{\"a\":1}").await;
        let r = request(addr, "GET", "/x", &[("X-Admin-Token", "t0k")])
            .await
            .unwrap();
        assert_eq!(
            r,
            Response {
                status: 503,
                body: "{\"a\":1}".into()
            }
        );
        let req = seen.await.unwrap();
        assert!(req.starts_with("GET /x HTTP/1.0\r\n"), "{req}");
        assert!(req.contains("X-Admin-Token: t0k\r\n"), "{req}");
    }

    #[tokio::test]
    async fn garbage_response_is_an_error() {
        let (addr, _h) = one_shot("not http").await;
        assert!(request(addr, "GET", "/", &[]).await.is_err());
    }

    #[tokio::test]
    async fn refused_connection_is_an_error() {
        let addr = {
            let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
            l.local_addr().unwrap()
        };
        assert!(request(addr, "GET", "/", &[]).await.is_err());
    }

    #[tokio::test]
    async fn header_injection_is_refused() {
        let (addr, _h) = one_shot("HTTP/1.0 200 OK\r\n\r\n").await;
        assert!(
            request(addr, "GET", "/", &[("X", "a\r\nb: c")])
                .await
                .is_err()
        );
    }
}
