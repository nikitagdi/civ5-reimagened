//! HTTP routes of the gateway: `/ws`, `/healthz`, `/catalog/{hash}`, `/replica/{hash}`, `/mods/{hash}` (mod asset packs, [`mods`]), `/netcheck/ws`,
//! `/api/auth/{info,register,login,adopt,logout}` (accounts, [`auth`]), `POST /client-report` (the web client's crash report, logged), `POST /client-report/v2` (the full report with
//! the client's logs, stored under `state_dir/reports/`), the web client and the assets. The admin API is a separate listener ([`admin`]).

pub mod admin;
pub mod auth;
pub mod catalog;
pub mod client_report;
pub mod mods;
pub mod netcheck;
pub mod replica;
pub mod reports;
pub mod static_files;

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};

use axum::Router;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{ConnectInfo, DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::json;

use crate::config::Cidr;
use crate::{BUILD, Shared, session};
use protocol::PROTOCOL_VERSION;

/// A WebSocket message from a client is at most this big (net-protocol 4.13).
const MAX_WS_MESSAGE: usize = wire::GATEWAY_MAX_FRAME;

pub fn router(shared: Arc<Shared>) -> Router {
    Router::new()
        .route("/", get(static_files::index))
        .route("/ws", get(ws_upgrade))
        .route("/healthz", get(healthz))
        .route("/api/auth/info", get(auth::info))
        .route(
            "/api/auth/register",
            post(auth::register).layer(DefaultBodyLimit::max(auth::MAX_BODY)),
        )
        .route(
            "/api/auth/login",
            post(auth::login).layer(DefaultBodyLimit::max(auth::MAX_BODY)),
        )
        .route(
            "/api/auth/adopt",
            post(auth::adopt).layer(DefaultBodyLimit::max(auth::MAX_BODY)),
        )
        .route(
            "/api/auth/logout",
            post(auth::logout).layer(DefaultBodyLimit::max(auth::MAX_BODY)),
        )
        .route(
            "/client-report",
            post(client_report::post).layer(DefaultBodyLimit::max(client_report::MAX_BODY)),
        )
        .route(
            "/client-report/v2",
            post(reports::post).layer(DefaultBodyLimit::max(reports::MAX_BODY)),
        )
        .route("/catalog/{hash}", get(catalog::get))
        .route("/replica/{hash}", get(replica::get))
        .route("/mods/{hash}", get(mods::get))
        .route("/netcheck/ws", get(netcheck::ws))
        .route("/assets/manifest", get(static_files::manifest_pointer))
        .route("/assets/{hash}", get(static_files::asset))
        .fallback(get(static_files::static_file))
        .with_state(shared)
}

async fn ws_upgrade(
    upgrade: WebSocketUpgrade,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    State(shared): State<Arc<Shared>>,
) -> Response {
    if shared.draining.load(std::sync::atomic::Ordering::Acquire) {
        // The gateway is draining: the client tries again in a moment.
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::RETRY_AFTER, "5")],
            "the server is restarting",
        )
            .into_response();
    }
    // A browser always sends `Origin` on a WebSocket upgrade, and the session cookie rides along: a page of another
    // site must not open a socket as the player. No `Origin` (native clients, bots): allowed.
    if !auth::origin_ok(&headers) {
        return (StatusCode::FORBIDDEN, "cross-origin request refused").into_response();
    }
    let ip = client_ip(peer.ip(), &headers, &shared.config.trusted_proxies);
    let cookie = crate::cookie::from_headers(&headers);
    let ua = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let label = crate::identity::device_label(ua);
    upgrade
        .max_message_size(MAX_WS_MESSAGE)
        .on_upgrade(move |socket| session::run(socket, shared, ip, label, cookie))
}

/// Address of the client for limits and logs (GS-D14): the rightmost `X-Forwarded-For` entry if
/// the peer is a trusted proxy, otherwise the peer itself.
pub fn client_ip(peer: IpAddr, headers: &HeaderMap, trusted: &[Cidr]) -> IpAddr {
    if !trusted.iter().any(|c| c.contains(peer)) {
        return peer;
    }
    headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .next_back()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(peer)
}

/// Readiness for `/healthz`: 503 until the `Describe` run has produced the core build id.
#[derive(Debug, Default)]
pub struct Health {
    core_build: Mutex<Option<String>>,
}

impl Health {
    /// The `Describe` run finished (or, until it exists, startup is complete).
    pub fn set_ready(&self, core_build: impl Into<String>) {
        *self
            .core_build
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(core_build.into());
    }

    pub fn core_build(&self) -> Option<String> {
        self.core_build
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

async fn healthz(State(shared): State<Arc<Shared>>) -> Response {
    let core_build = shared.health.core_build();
    let ready = core_build.is_some();
    let body = json!({
        "status": if ready { "ok" } else { "starting" },
        "build": BUILD,
        "core_build": core_build,
        "protocol": PROTOCOL_VERSION,
        "games": shared.games.running(),
        "restoring": 0,
    });
    let status = if ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xff(v: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert("x-forwarded-for", v.parse().unwrap());
        h
    }

    #[test]
    fn forwarded_for_is_believed_only_from_trusted_proxies() {
        let trusted: Vec<Cidr> = vec![
            "127.0.0.1".parse().unwrap(),
            "172.16.0.0/12".parse().unwrap(),
        ];
        let proxy: IpAddr = "172.17.0.2".parse().unwrap();
        let stranger: IpAddr = "8.8.8.8".parse().unwrap();
        // Rightmost entry: the one the trusted proxy itself appended.
        assert_eq!(
            client_ip(proxy, &xff("1.1.1.1, 2.2.2.2"), &trusted),
            "2.2.2.2".parse::<IpAddr>().unwrap()
        );
        // A stranger cannot choose its own address.
        assert_eq!(client_ip(stranger, &xff("2.2.2.2"), &trusted), stranger);
        // Garbage or absent header: the peer.
        assert_eq!(client_ip(proxy, &xff("nonsense"), &trusted), proxy);
        assert_eq!(client_ip(proxy, &HeaderMap::new(), &trusted), proxy);
        // No trusted proxies configured (a server without a reverse proxy).
        assert_eq!(client_ip(proxy, &xff("2.2.2.2"), &[]), proxy);
    }
}
