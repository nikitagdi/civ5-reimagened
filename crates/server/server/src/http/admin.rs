//! The admin API: its own loopback listener, every request needs the `X-Admin-Token` header.
//!
//! | Request | What |
//! |---|---|
//! | `GET /admin/metrics` | the counters ([`crate::metrics`]) |
//! | `GET /admin/games`, `GET /admin/games/{id}` | the list; the details with seats, settings and saves |
//! | `POST /admin/games/{id}/save` | `SaveNow` |
//! | `POST /admin/games/{id}/stop` | save and stop; the game sleeps with its seats kept |
//! | `POST /admin/games/{id}/close` | save and close for good (also a sleeping game): `Finished`, seats and nicks free, saves stay loadable |
//! | `POST /admin/games/{id}/purge` | close the game if it is open, then delete its directory (saves, autosaves, meta) and take its saves out of the lobby list |
//! | `POST /admin/purge?nick_prefix=smoke` | the same for every game that is not running and whose humans all have a nick with the prefix (at least 3 letters): the leftovers of smoke runs |
//! | `POST /admin/games/{id}/kick?player=P` | a kick without the host |
//! | `GET /admin/games/{id}/trace?turns=126-128&format=chrome\|jsonl\|summary` | the timeline of the end of turns ([`crate::game::trace`]) |
//! | `POST /admin/games/{id}/restart` | a worker from the newest save (also from `Faulted`, or a sleeping game) |
//! | `POST /admin/drain[?exit=1]` | the drain of spec 4.12 |
//! | `POST /admin/accounts/{nick}/reset` | clears the account's password and devices; the owner registers the nick again |
//! | `GET /admin/reports[?limit=N]`, `GET /admin/reports/{id}`, `GET /admin/reports/{id}/log?grep=&level=&limit=`, `GET /admin/reports/{id}/perf` | the client reports (`POST /client-report/v2`) |
//!
//! Only with `--test-hooks` (GS-D18; without the flag these paths are 404):
//! `POST /admin/games/{id}/kill-worker?signal=KILL|STOP|CONT` and `POST /admin/games/{id}/fault` with
//! `{"seat", "kind", "value"?}` (`corrupt_hash`, `drop_batch`, `reject_next_cmd`, `close_socket`,
//! `hold_batches_ms`).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::extract::{Path, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use protocol::{GameCtlReason, GameId};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::oneshot;

use crate::Shared;
use crate::game::seat::SeatFault;
use crate::game::{ActorMsg, AdminCtl, Fault, GameHandle};
use crate::supervisor::GameState;

pub const TOKEN_HEADER: &str = "x-admin-token";

pub fn router(shared: Arc<Shared>) -> Router {
    let mut r = Router::new()
        .route("/admin/metrics", get(metrics))
        .route("/admin/games", get(games))
        .route("/admin/games/{id}", get(game))
        .route("/admin/games/{id}/save", post(save))
        .route("/admin/games/{id}/stop", post(stop))
        .route("/admin/games/{id}/close", post(close))
        .route("/admin/games/{id}/restart", post(restart))
        .route("/admin/games/{id}/kick", post(kick))
        .route("/admin/games/{id}/purge", post(purge))
        .route("/admin/purge", post(purge_by_nick))
        .route("/admin/games/{id}/trace", get(trace))
        .route("/admin/drain", post(drain))
        .route("/admin/accounts/{nick}/reset", post(reset_account))
        .route("/admin/reports", get(super::reports::admin_list))
        .route("/admin/reports/{id}", get(super::reports::admin_one))
        .route("/admin/reports/{id}/log", get(super::reports::admin_log))
        .route("/admin/reports/{id}/perf", get(super::reports::admin_perf));
    if shared.config.test_hooks {
        r = r
            .route("/admin/games/{id}/kill-worker", post(kill_worker))
            .route("/admin/games/{id}/fault", post(fault));
    }
    r.fallback(not_found)
        // Layered last, so it also covers the fallback: an unknown path without a token is 401.
        .layer(middleware::from_fn_with_state(shared.clone(), auth))
        .with_state(shared)
}

async fn auth(State(shared): State<Arc<Shared>>, req: Request, next: Next) -> Response {
    let Some(expected) = &shared.config.admin_token else {
        return (StatusCode::SERVICE_UNAVAILABLE, "admin API has no token").into_response();
    };
    let given = req
        .headers()
        .get(TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if !expected.matches(given) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    next.run(req).await
}

async fn not_found() -> StatusCode {
    StatusCode::NOT_FOUND
}

fn err(status: StatusCode, text: &str) -> Response {
    (status, Json(json!({ "error": text }))).into_response()
}

fn ok() -> Response {
    Json(json!({ "ok": true })).into_response()
}

async fn metrics(State(shared): State<Arc<Shared>>) -> Json<Value> {
    Json(crate::metrics::render(&shared))
}

async fn games(State(shared): State<Arc<Shared>>) -> Json<Value> {
    Json(crate::metrics::games_list(&shared))
}

async fn game(State(shared): State<Arc<Shared>>, Path(id): Path<GameId>) -> Response {
    match crate::metrics::game_detail(&shared, id) {
        Some(v) => Json(v).into_response(),
        None => err(StatusCode::NOT_FOUND, "no such game"),
    }
}

/// A game with a worker (or a supervisor): `Err` is the answer to send.
#[allow(clippy::result_large_err)] // the Err is the answer itself, used once and sent
fn live(shared: &Shared, id: GameId) -> Result<GameHandle, Response> {
    shared
        .games
        .get(id)
        .ok_or_else(|| err(StatusCode::NOT_FOUND, "no such game (it may be asleep)"))
}

#[derive(Deserialize)]
struct TraceQuery {
    turns: Option<String>,
    format: Option<String>,
}

/// `turns=126`, `turns=126-128`, `turns=126-` (to the last) or none (all held).
fn turn_range(s: Option<&str>) -> Option<(i32, i32)> {
    let Some(s) = s.filter(|s| !s.trim().is_empty()) else {
        return Some((i32::MIN, i32::MAX));
    };
    match s.split_once('-') {
        None => s.trim().parse().ok().map(|t| (t, t)),
        Some((a, b)) => {
            let lo = if a.trim().is_empty() {
                i32::MIN
            } else {
                a.trim().parse().ok()?
            };
            let hi = if b.trim().is_empty() {
                i32::MAX
            } else {
                b.trim().parse().ok()?
            };
            Some((lo, hi))
        }
    }
}

/// The timeline of the end of turns: Chrome/Perfetto JSON (default), the journal lines, or one summary line a turn.
async fn trace(
    State(shared): State<Arc<Shared>>,
    Path(id): Path<GameId>,
    Query(q): Query<TraceQuery>,
) -> Response {
    let h = match live(&shared, id) {
        Ok(h) => h,
        Err(r) => return r,
    };
    let Some((lo, hi)) = turn_range(q.turns.as_deref()) else {
        return err(StatusCode::BAD_REQUEST, "turns= is a turn, a-b, a- or -b");
    };
    let t = &h.info.trace;
    match q.format.as_deref().unwrap_or("chrome") {
        "chrome" => Json(t.chrome(lo, hi)).into_response(),
        "jsonl" => (
            [(axum::http::header::CONTENT_TYPE, "application/x-ndjson")],
            t.jsonl(lo, hi),
        )
            .into_response(),
        "summary" => (
            [(
                axum::http::header::CONTENT_TYPE,
                "text/plain; charset=utf-8",
            )],
            t.summary_text(lo, hi),
        )
            .into_response(),
        "info" => Json(t.info()).into_response(),
        _ => err(
            StatusCode::BAD_REQUEST,
            "format= is chrome, jsonl, summary or info",
        ),
    }
}

async fn save(State(shared): State<Arc<Shared>>, Path(id): Path<GameId>) -> Response {
    let h = match live(&shared, id) {
        Ok(h) => h,
        Err(r) => return r,
    };
    if h.info.snapshot().state != GameState::Running {
        return err(StatusCode::CONFLICT, "the game has no running worker");
    }
    h.send(ActorMsg::Admin(AdminCtl::Save));
    ok()
}

async fn stop(State(shared): State<Arc<Shared>>, Path(id): Path<GameId>) -> Response {
    let h = match live(&shared, id) {
        Ok(h) => h,
        Err(r) => return r,
    };
    if h.info.snapshot().state != GameState::Running {
        return err(StatusCode::CONFLICT, "the game has no running worker");
    }
    h.send(ActorMsg::Admin(AdminCtl::Stop));
    ok()
}

/// Closes a game for good (the user's rule, 2026-11-02): a running one is saved and stopped, a sleeping one is ended
/// by the lobby; the seats and nicks are free, the saves stay. The answer does not wait for the worker.
async fn close(State(shared): State<Arc<Shared>>, Path(id): Path<GameId>) -> Response {
    if let Some(h) = shared.games.get(id) {
        h.send(ActorMsg::Admin(AdminCtl::Close));
        return ok();
    }
    if shared.games.dormant(id).is_some() {
        shared
            .lobby
            .send(crate::lobby::LobbyMsg::CloseSleeping { game: id });
        return ok();
    }
    err(StatusCode::NOT_FOUND, "no such game")
}

async fn restart(State(shared): State<Arc<Shared>>, Path(id): Path<GameId>) -> Response {
    if let Some(h) = shared.games.get(id) {
        h.send(ActorMsg::Admin(AdminCtl::Restart));
        return ok();
    }
    // A sleeping game wakes.
    if shared.games.dormant(id).is_some() {
        return match crate::game::restore::wake(&shared, id) {
            Ok(_) => ok(),
            Err(why) => err(StatusCode::CONFLICT, &why),
        };
    }
    err(StatusCode::NOT_FOUND, "no such game")
}

/// How long a purge waits for a game to end after the close asked for it.
const PURGE_WAIT: Duration = Duration::from_secs(20);

/// Closes the game if it is open, waits until it is gone, then deletes `games/<id>/` and takes the saves out of the
/// lobby's list. `Err` is the answer to send. The retention pass would delete a finished game after its age; a smoke
/// run does not wait for that.
#[allow(clippy::result_large_err)] // the Err is the answer itself, used once and sent
async fn purge_game(shared: &Arc<Shared>, id: GameId) -> Result<(), Response> {
    let Some(dir) = shared
        .config
        .state_dir
        .as_ref()
        .map(|d| d.join("games").join(id.to_string()))
    else {
        return Err(err(StatusCode::CONFLICT, "the gateway keeps no state"));
    };
    if let Some(h) = shared.games.get(id) {
        h.send(ActorMsg::Admin(AdminCtl::Close));
    } else if shared.games.dormant(id).is_some() {
        shared
            .lobby
            .send(crate::lobby::LobbyMsg::CloseSleeping { game: id });
    } else if !dir.is_dir() {
        return Err(err(StatusCode::NOT_FOUND, "no such game"));
    }
    let deadline = tokio::time::Instant::now() + PURGE_WAIT;
    while shared.games.get(id).is_some() || shared.games.dormant(id).is_some() {
        if tokio::time::Instant::now() >= deadline {
            return Err(err(StatusCode::GATEWAY_TIMEOUT, "the game did not end"));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    if let Err(e) = tokio::fs::remove_dir_all(&dir).await
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(game = id, error = %e, "admin: cannot delete the game directory");
        return Err(err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "cannot delete the game directory",
        ));
    }
    shared
        .lobby
        .send(crate::lobby::LobbyMsg::PurgeGame { game: id });
    tracing::info!(game = id, "admin: the game and its saves are deleted");
    Ok(())
}

/// Deletes a game with its saves (see [`purge_game`]).
async fn purge(State(shared): State<Arc<Shared>>, Path(id): Path<GameId>) -> Response {
    match purge_game(&shared, id).await {
        Ok(()) => Json(json!({ "ok": true, "game": id })).into_response(),
        Err(r) => r,
    }
}

/// Deletes every game that is not running whose humans all have a nick starting with `nick_prefix` (the games the
/// smoke runs of `cargo xtask deploy` left: `smoke123456`). A game with a worker is left alone.
async fn purge_by_nick(
    State(shared): State<Arc<Shared>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let prefix = q.get("nick_prefix").map(|p| p.to_lowercase());
    let Some(prefix) = prefix.filter(|p| p.chars().count() >= 3) else {
        return err(
            StatusCode::BAD_REQUEST,
            "nick_prefix=<at least 3 letters> is needed",
        );
    };
    let Some(state_dir) = shared.config.state_dir.clone() else {
        return err(StatusCode::CONFLICT, "the gateway keeps no state");
    };
    let metas = match crate::state::meta::read_all(&state_dir) {
        Ok(m) => m,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e:#}")),
    };
    let (mut purged, mut skipped) = (Vec::new(), Vec::new());
    for m in metas {
        let humans = m.humans();
        if humans.is_empty()
            || !humans
                .iter()
                .all(|(_, nick)| nick.to_lowercase().starts_with(&prefix))
        {
            continue;
        }
        if shared.games.get(m.id).is_some() {
            skipped.push(m.id);
        } else if purge_game(&shared, m.id).await.is_ok() {
            purged.push(m.id);
        } else {
            skipped.push(m.id);
        }
    }
    Json(json!({ "ok": true, "purged": purged, "skipped": skipped })).into_response()
}

fn reason_name(r: GameCtlReason) -> &'static str {
    crate::game::stats::CTL_REASON_NAMES[crate::game::stats::ctl_reason_index(r)]
}

async fn kick(
    State(shared): State<Arc<Shared>>,
    Path(id): Path<GameId>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let h = match live(&shared, id) {
        Ok(h) => h,
        Err(r) => return r,
    };
    let Some(player) = q.get("player").and_then(|p| p.parse::<i8>().ok()) else {
        return err(StatusCode::BAD_REQUEST, "player=<number> is needed");
    };
    let (tx, rx) = oneshot::channel();
    if !h.send(ActorMsg::AdminKick { player, reply: tx }) {
        return err(StatusCode::CONFLICT, "the game has ended");
    }
    match tokio::time::timeout(Duration::from_secs(2), rx).await {
        Ok(Ok(Ok(()))) => ok(),
        Ok(Ok(Err(reason))) => err(StatusCode::CONFLICT, reason_name(reason)),
        _ => err(StatusCode::GATEWAY_TIMEOUT, "the game did not answer"),
    }
}

/// Clears the hash and the devices of an account (a lost password); its live sessions are kicked.
async fn reset_account(State(shared): State<Arc<Shared>>, Path(nick): Path<String>) -> Response {
    if shared.identity.reset(&nick) {
        tracing::info!(nick = %nick, "admin: account reset");
        ok()
    } else {
        err(StatusCode::NOT_FOUND, "no such account")
    }
}

async fn drain(
    State(shared): State<Arc<Shared>>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let exit = q.get("exit").is_some_and(|v| v == "1" || v == "true");
    // The answer comes when the drain is done: the caller sees that the games are saved.
    shared.drain(exit).await;
    ok()
}

// ---------------------------------------------------------------- test hooks

/// A real signal to the worker of the game: a crash (`KILL`) or a hang (`STOP`, then `CONT`).
async fn kill_worker(
    State(shared): State<Arc<Shared>>,
    Path(id): Path<GameId>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    let h = match live(&shared, id) {
        Ok(h) => h,
        Err(r) => return r,
    };
    let pid = h
        .info
        .stats
        .worker_pid
        .load(std::sync::atomic::Ordering::Relaxed);
    if pid == 0 {
        return err(StatusCode::CONFLICT, "the game has no worker process");
    }
    let signal = q.get("signal").map_or("KILL", String::as_str);
    if !matches!(signal, "KILL" | "STOP" | "CONT") {
        return err(StatusCode::BAD_REQUEST, "signal is KILL, STOP or CONT");
    }
    match send_signal(pid, signal) {
        Ok(()) => ok(),
        Err(why) => err(StatusCode::NOT_IMPLEMENTED, &why),
    }
}

#[cfg(unix)]
fn send_signal(pid: u32, signal: &str) -> Result<(), String> {
    let status = std::process::Command::new("kill")
        .args(["-s", signal, &pid.to_string()])
        .status()
        .map_err(|e| format!("cannot run kill: {e}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("kill -s {signal} {pid} failed"))
}

#[cfg(not(unix))]
fn send_signal(pid: u32, signal: &str) -> Result<(), String> {
    if signal != "KILL" {
        return Err(format!("signal {signal} needs a Unix system"));
    }
    let status = std::process::Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .status()
        .map_err(|e| format!("cannot run taskkill: {e}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("taskkill {pid} failed"))
}

#[derive(Debug, Deserialize)]
struct FaultBody {
    seat: i8,
    kind: String,
    value: Option<u64>,
}

async fn fault(
    State(shared): State<Arc<Shared>>,
    Path(id): Path<GameId>,
    Json(b): Json<FaultBody>,
) -> Response {
    let h = match live(&shared, id) {
        Ok(h) => h,
        Err(r) => return r,
    };
    let fault = match b.kind.as_str() {
        "corrupt_hash" => Fault::Seat(SeatFault::CorruptHash),
        "drop_batch" => Fault::Seat(SeatFault::DropBatch),
        "close_socket" => Fault::Seat(SeatFault::CloseSocket),
        "hold_batches_ms" => Fault::Seat(SeatFault::Hold(b.value.unwrap_or(1000))),
        "reject_next_cmd" => Fault::RejectNextCmd,
        _ => return err(StatusCode::BAD_REQUEST, "unknown fault kind"),
    };
    h.send(ActorMsg::Fault {
        player: b.seat,
        fault,
    });
    ok()
}
