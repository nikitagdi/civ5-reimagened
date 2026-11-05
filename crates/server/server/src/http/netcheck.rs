//! `GET /netcheck/ws`: an echo socket for measuring the round trip
//! from a browser. No login. One connection per client address, at most 120 s, at most 2
//! messages per second. (The page at `/netcheck/` is a static file under `web_root`.)

use std::collections::{HashSet, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use tokio::time::Instant;

use super::client_ip;
use crate::Shared;

/// Longest echoed message; a ping carries a timestamp, not data.
const MAX_MESSAGE: usize = 1024;
/// WebSocket close code "policy violation".
const CLOSE_POLICY: u16 = 1008;

/// Per-address connection set and the limits.
#[derive(Debug)]
pub struct Netcheck {
    active: Mutex<HashSet<IpAddr>>,
    /// A connection is closed after this long.
    pub max_duration: Duration,
    /// Messages beyond this many in any second close the connection.
    pub max_per_second: usize,
}

impl Default for Netcheck {
    fn default() -> Self {
        Self::new(Duration::from_secs(120), 2)
    }
}

impl Netcheck {
    pub fn new(max_duration: Duration, max_per_second: usize) -> Self {
        Self {
            active: Mutex::default(),
            max_duration,
            max_per_second,
        }
    }

    /// Claims the address; `None` if it already has a connection.
    fn enter(self: &Arc<Self>, ip: IpAddr) -> Option<Entry> {
        let fresh = self
            .active
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(ip);
        fresh.then(|| Entry {
            owner: self.clone(),
            ip,
        })
    }
}

/// Holds an address in the set; frees it on drop.
struct Entry {
    owner: Arc<Netcheck>,
    ip: IpAddr,
}

impl Drop for Entry {
    fn drop(&mut self) {
        self.owner
            .active
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.ip);
    }
}

pub async fn ws(
    upgrade: WebSocketUpgrade,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    State(shared): State<Arc<Shared>>,
) -> Response {
    let ip = client_ip(peer.ip(), &headers, &shared.config.trusted_proxies);
    let Some(entry) = shared.netcheck.clone().enter(ip) else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    upgrade
        .max_message_size(MAX_MESSAGE)
        .on_upgrade(move |socket| echo(socket, entry))
}

async fn echo(mut socket: WebSocket, entry: Entry) {
    let limits = &entry.owner;
    let deadline = Instant::now() + limits.max_duration;
    let mut recent: VecDeque<Instant> = VecDeque::new();
    loop {
        let msg = match tokio::time::timeout_at(deadline, socket.recv()).await {
            Err(_) | Ok(None | Some(Err(_))) => break,
            Ok(Some(Ok(msg))) => msg,
        };
        let reply = match msg {
            Message::Text(_) | Message::Binary(_) => msg,
            Message::Close(_) => return,
            Message::Ping(_) | Message::Pong(_) => continue,
        };
        let now = Instant::now();
        while recent
            .front()
            .is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(1))
        {
            recent.pop_front();
        }
        if recent.len() >= limits.max_per_second {
            let _ = socket
                .send(Message::Close(Some(CloseFrame {
                    code: CLOSE_POLICY,
                    reason: "too many messages".into(),
                })))
                .await;
            return;
        }
        recent.push_back(now);
        if socket.send(reply).await.is_err() {
            return;
        }
    }
    let _ = socket.send(Message::Close(None)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_connection_per_address() {
        let n = Arc::new(Netcheck::default());
        let a: IpAddr = "10.0.0.1".parse().unwrap();
        let b: IpAddr = "10.0.0.2".parse().unwrap();
        let first = n.enter(a).unwrap();
        assert!(n.enter(a).is_none());
        assert!(n.enter(b).is_some());
        drop(first);
        assert!(n.enter(a).is_some());
    }
}
