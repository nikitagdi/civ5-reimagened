//! The control API of `netproxy` (HTTP JSON on axum): `POST /profile`, `/blackhole`, `/reset`, `/half-open`,
//! `GET /stats`. A bad request is 400 with `{"error": ...}`.

use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};

use super::model::DELAY_EDGES_MS;
use super::relay::{Dir, SIDE_CLIENT, SIDE_UPSTREAM, Shared};

type Reply = (StatusCode, Json<Value>);

fn bad(msg: impl ToString) -> Reply {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": msg.to_string() })),
    )
}

pub fn router(shared: Arc<Shared>) -> Router {
    Router::new()
        .route("/profile", post(set_profile).get(get_profile))
        .route("/blackhole", post(blackhole))
        .route("/reset", post(reset))
        .route("/half-open", post(half_open))
        .route("/stats", get(stats))
        .with_state(shared)
}

async fn get_profile(State(s): State<Arc<Shared>>) -> Reply {
    let p = s.profile.read().unwrap().clone();
    (StatusCode::OK, Json(serde_json::to_value(p).unwrap()))
}

/// The fields given are laid over the current profile; `{}` changes nothing; `{"reset": true}` first goes back
/// to the default (a clean channel).
async fn set_profile(State(s): State<Arc<Shared>>, Json(body): Json<Value>) -> Reply {
    let mut body = body;
    let clean = body
        .as_object_mut()
        .and_then(|o| o.remove("reset"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let base = if clean {
        Default::default()
    } else {
        s.profile.read().unwrap().clone()
    };
    match base.patched(&body) {
        Ok(p) => {
            *s.profile.write().unwrap() = p.clone();
            (StatusCode::OK, Json(serde_json::to_value(p).unwrap()))
        }
        Err(e) => bad(e),
    }
}

#[derive(Deserialize)]
struct Ms {
    ms: u64,
}

async fn blackhole(State(s): State<Arc<Shared>>, Json(b): Json<Ms>) -> Reply {
    s.blackhole(b.ms);
    (StatusCode::OK, Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct HalfOpen {
    dir: String,
    ms: u64,
}

async fn half_open(State(s): State<Arc<Shared>>, Json(b): Json<HalfOpen>) -> Reply {
    let Some(dir) = Dir::parse(&b.dir) else {
        return bad("dir is \"up\" or \"down\"");
    };
    s.half_open(dir, b.ms);
    (StatusCode::OK, Json(json!({ "ok": true })))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ResetBody {
    conn: Option<u64>,
    /// `client`, `upstream` or `both` (the default): the socket that is closed with RST.
    side: Option<String>,
}

async fn reset(State(s): State<Arc<Shared>>, body: Option<Json<ResetBody>>) -> Reply {
    let b = body.map(|j| j.0).unwrap_or_default();
    let sides = match b.side.as_deref() {
        None | Some("both") => SIDE_CLIENT | SIDE_UPSTREAM,
        Some("client") => SIDE_CLIENT,
        Some("upstream") => SIDE_UPSTREAM,
        Some(_) => return bad("side is client, upstream or both"),
    };
    let n = s.reset(b.conn, sides);
    (StatusCode::OK, Json(json!({ "ok": true, "reset": n })))
}

async fn stats(State(s): State<Arc<Shared>>) -> Reply {
    let st = &s.stats;
    let dirs = |f: &dyn Fn(usize) -> Value| json!({ "up": f(0), "down": f(1) });
    let hist = |d: usize| -> Value {
        let mut labels: Vec<String> = DELAY_EDGES_MS.iter().map(|e| format!("<{e}")).collect();
        labels.push(format!(">={}", DELAY_EDGES_MS.last().unwrap()));
        let counts: Vec<u64> = st.delay_hist[d].iter().map(|c| c.load(Relaxed)).collect();
        json!({ "ms": labels, "count": counts })
    };
    let body = json!({
        "conns": { "total": st.conns_total.load(Relaxed), "open": st.conns_open.load(Relaxed), "ids": s.open_ids(), "resets": st.resets.load(Relaxed) },
        "bytes": dirs(&|d| json!(st.bytes[d].load(Relaxed))),
        "chunks": dirs(&|d| json!(st.chunks[d].load(Relaxed))),
        "queue": dirs(&|d| json!(st.queued[d].load(Relaxed))),
        "added_delay_ms": dirs(&|d| hist(d)),
        "added_delay_max_ms": dirs(&|d| json!(st.delay_max_ms[d].load(Relaxed))),
        "profile": serde_json::to_value(s.profile.read().unwrap().clone()).unwrap(),
    });
    (StatusCode::OK, Json(body))
}
