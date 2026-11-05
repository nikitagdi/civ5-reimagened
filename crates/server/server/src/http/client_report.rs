//! `POST /client-report`: the web client's crash report (`apps/web/www/bootstrap.js`, live report 2026-10-31).
//! When the wasm module traps, the page sends one short JSON text (the first error, the size of the wasm
//! memory, the turn, the game); the server logs it at warn level and keeps nothing. The body is capped
//! ([`MAX_BODY`]) and the reports are rate-limited per address and in total ([`Limiter`]).

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};

use crate::Shared;

/// A report is at most this big (bigger bodies get 413 from the body limit of the route).
pub const MAX_BODY: usize = 16 * 1024;
/// What of a report goes to the log, in characters.
const MAX_LOGGED: usize = 4000;
/// The window of the rate limit.
const WINDOW: Duration = Duration::from_secs(60);
/// Reports per address in a window.
const PER_IP: u32 = 3;
/// Reports from everyone in a window.
const TOTAL: u32 = 30;

/// Counts the reports of the current window, per address and in total.
#[derive(Debug)]
pub struct Limiter {
    start: Instant,
    total: u32,
    per_ip: HashMap<IpAddr, u32>,
}

impl Limiter {
    pub fn new(now: Instant) -> Self {
        Limiter {
            start: now,
            total: 0,
            per_ip: HashMap::new(),
        }
    }

    /// Whether a report of `ip` at `now` is taken (and counts it).
    pub fn allow(&mut self, ip: IpAddr, now: Instant) -> bool {
        if now.duration_since(self.start) >= WINDOW {
            *self = Limiter::new(now);
        }
        let n = self.per_ip.entry(ip).or_insert(0);
        if *n >= PER_IP || self.total >= TOTAL {
            return false;
        }
        *n += 1;
        self.total += 1;
        true
    }
}

static LIMITER: Mutex<Option<Limiter>> = Mutex::new(None);

/// The report as one log line: control characters (new lines among them) become spaces, the length is capped.
pub fn log_text(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body);
    let mut out: String = text
        .chars()
        .take(MAX_LOGGED)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if text.chars().count() > MAX_LOGGED {
        out.push_str(" ...");
    }
    out
}

pub async fn post(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    State(shared): State<Arc<Shared>>,
    body: Bytes,
) -> StatusCode {
    let ip = super::client_ip(peer.ip(), &headers, &shared.config.trusted_proxies);
    let now = Instant::now();
    let allowed = LIMITER
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert_with(|| Limiter::new(now))
        .allow(ip, now);
    if !allowed {
        return StatusCode::TOO_MANY_REQUESTS;
    }
    tracing::warn!(
        target: "client_report",
        %ip,
        bytes = body.len(),
        "client crash report: {}",
        log_text(&body)
    );
    StatusCode::NO_CONTENT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limit_is_per_address_and_in_total_and_resets() {
        let t0 = Instant::now();
        let mut l = Limiter::new(t0);
        let a: IpAddr = "10.0.0.1".parse().unwrap();
        for _ in 0..PER_IP {
            assert!(l.allow(a, t0));
        }
        assert!(!l.allow(a, t0));
        // Other addresses until the total is reached.
        let mut taken = PER_IP;
        for i in 0..TOTAL * 2 {
            let ip = IpAddr::from([10, 1, (i / 256) as u8, (i % 256) as u8]);
            if l.allow(ip, t0) {
                taken += 1;
            }
        }
        assert_eq!(taken, TOTAL);
        // A new window.
        assert!(l.allow(a, t0 + WINDOW));
    }

    #[test]
    fn the_logged_text_is_one_capped_line() {
        assert_eq!(log_text(b"a\nb\r\tc"), "a b  c");
        let long = vec![b'x'; MAX_LOGGED + 10];
        let t = log_text(&long);
        assert!(t.ends_with(" ..."));
        assert_eq!(t.len(), MAX_LOGGED + 4);
        assert_eq!(log_text(&[0xff, b'a']), "\u{fffd}a");
    }
}
