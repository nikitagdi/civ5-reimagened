//! Gateway counters and the JSON of `/admin/metrics`, `/admin/games` and
//! `/admin/games/{id}`. Names follow the spec because `net-test` reads them. Two deviations: `rtt_ms` of a
//! seat stays empty (the application `Ping` of the protocol carries no round trip the gateway could
//! read; `ws_rtt_ms` is measured), and `cmd_gateway_ms` ends when the answer reaches the pipeline of the
//! seat, not when the socket took it. `gateway_rss_kb`, `rss_kb` of a worker and `state_free_mb` are
//! read on Linux only (`null` elsewhere).

use std::sync::atomic::{AtomicU64, Ordering};

use protocol::{IPC_VERSION, PROTOCOL_VERSION, SeatControl, TurnMode, TurnPhase};
use serde_json::{Map, Value, json};

use crate::game::stats::{self, HistSum, REASON_NAMES, REJECT_NAMES, SeatStats};
use crate::game::{GameHandle, status_of};
use crate::supervisor::GameState;
use crate::{BUILD, Shared};

/// `server.sessions` counters.
#[derive(Debug, Default)]
pub struct Metrics {
    /// WebSockets open now.
    pub sessions_open: AtomicU64,
    /// Open WebSockets past `Login`.
    pub sessions_logged_in: AtomicU64,
    /// WebSockets opened since start.
    pub sessions_opened_total: AtomicU64,
    /// Sessions closed because a newer one took their token.
    pub sessions_replaced_total: AtomicU64,
    /// Logins refused (bad password, taken nick, bad nick, version).
    pub login_failed_total: AtomicU64,
    /// Logged-in sessions dropped for 20 s without a frame.
    pub sessions_lost_timeout_total: AtomicU64,
    /// Sessions closed with 1013 because their send queue overflowed.
    pub sessions_slow_consumer_total: AtomicU64,
    /// Sessions the client closed itself.
    pub sessions_closed_clean_total: AtomicU64,
}

impl Metrics {
    pub fn bump(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub fn drop_one(counter: &AtomicU64) {
        // Saturating: a gauge must never wrap below zero.
        let _ = counter.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_sub(1));
    }

    pub fn get(counter: &AtomicU64) -> u64 {
        counter.load(Ordering::Relaxed)
    }
}

fn status_name(state: GameState) -> &'static str {
    match state {
        GameState::Starting => "Starting",
        GameState::Running => "Running",
        GameState::Restarting => "Restarting",
        GameState::Faulted => "Faulted",
        GameState::Finished => "Finished",
    }
}

fn phase_name(p: Option<TurnPhase>) -> Value {
    p.map_or(Value::Null, |p| json!(format!("{p:?}")))
}

fn mode_name(m: Option<TurnMode>) -> Value {
    m.map_or(Value::Null, |m| json!(format!("{m:?}")))
}

/// Resident memory of a process in KiB (`/proc/<pid>/status`); `None` where there is no `/proc`.
fn rss_kb(pid: &str) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let line = text.lines().find(|l| l.starts_with("VmRSS:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

/// Free space under `dir` in MiB (`df -Pk`); `None` where `df` is not there.
pub fn state_free_mb(dir: &std::path::Path) -> Option<u64> {
    let out = std::process::Command::new("df")
        .arg("-Pk")
        .arg(dir)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let kb: u64 = text
        .lines()
        .nth(1)?
        .split_whitespace()
        .nth(3)?
        .parse()
        .ok()?;
    Some(kb / 1024)
}

fn seat_json(nick: Option<&str>, control: &str, conn: &str, st: &SeatStats) -> Value {
    let g = stats::get;
    let snap = st.snapshot();
    let tx_by_turn: Vec<Value> = st
        .tx_by_turn()
        .iter()
        .map(|t| {
            json!({
                "turn": t.turn,
                "batches": t.batches,
                "body_bytes": t.body_bytes,
                "keyframes": t.keyframes,
            })
        })
        .collect();
    json!({
        "nick": nick,
        "control": control,
        "conn": conn,
        "stream": snap.stream,
        "seq": snap.seq,
        "view_seq": snap.view_seq,
        "view_hash": snap.view_hash,
        "basis_hash": snap.basis_hash,
        "rev": snap.rev,
        "rx": { "msgs": g(&st.rx_msgs), "bytes": g(&st.rx_bytes) },
        "tx_by_turn": tx_by_turn,
        "rtt_ms": st.rtt.json(),
        "ws_rtt_ms": st.ws_rtt.json(),
        "tx": {
            "batches": g(&st.tx_batches),
            "msgs": g(&st.tx_msgs),
            "body_bytes": g(&st.tx_body_bytes),
            "raw_bytes": g(&st.tx_raw_bytes),
            "keyframe_bytes_max": g(&st.keyframe_bytes_max),
            "basis_bytes_max": g(&st.basis_bytes_max),
            "cold_batch_bytes_max": g(&st.cold_batch_bytes_max),
        },
        "resume": { "replay": g(&st.resume_replay), "fresh": g(&st.resume_fresh) },
        "ring": {
            "batches": snap.ring_batches,
            "bytes": snap.ring_bytes,
            "overflow_total": g(&st.ring_overflow_total),
        },
    })
}

/// `games.<id>.replica`: the stream of the replica.
fn replica_json(r: &crate::game::replica::ReplicaStats) -> Value {
    let get = stats::get;
    let mut resync = Map::new();
    for (i, name) in crate::game::replica::RESYNC_NAMES.iter().enumerate() {
        resync.insert((*name).to_owned(), json!(get(&r.resync[i])));
    }
    json!({
        "batches": get(&r.batches),
        "batch_bytes": get(&r.batch_bytes),
        "batches_dropped": get(&r.batches_dropped),
        "bases": get(&r.bases),
        "turns": get(&r.turns),
        "turns_with_patch": get(&r.turns_with_patch),
        "resync_bases": get(&r.resync_bases),
        "last_snapshot_bytes": get(&r.last_snapshot_bytes),
        "last_patch_bytes": get(&r.last_patch_bytes),
        "blobs": get(&r.blobs),
        "blobs_bad": get(&r.blobs_bad),
        "resync": resync,
        "resync_dropped": get(&r.resync_dropped),
        "net_in": get(&r.net_in),
        "net_bytes": get(&r.net_bytes),
        "net_dropped": get(&r.net_dropped),
        "net_rejected": get(&r.net_rejected),
    })
}

/// `games.<id>` of the metrics.
fn game_metrics(g: &GameHandle) -> Value {
    let info = &g.info;
    let snap = info.snapshot();
    let gs = &info.stats;
    let get = stats::get;
    let seat_stats = info.seat_stats.all();
    let sum = |f: fn(&SeatStats) -> &AtomicU64| -> u64 {
        seat_stats.iter().map(|(_, s)| stats::get(f(s))).sum()
    };
    let mut need = Map::new();
    for (i, name) in REASON_NAMES.iter().enumerate() {
        let n: u64 = seat_stats
            .iter()
            .map(|(_, s)| stats::get(&s.need_keyframe[i]))
            .sum();
        need.insert((*name).to_owned(), json!(n));
    }
    let mut rejected = Map::new();
    for (i, name) in REJECT_NAMES.iter().enumerate() {
        rejected.insert((*name).to_owned(), json!(get(&gs.rejected[i])));
    }
    let mut ctl_rejected = Map::new();
    for (i, name) in stats::CTL_REASON_NAMES.iter().enumerate() {
        ctl_rejected.insert((*name).to_owned(), json!(get(&gs.gamectl_rejected[i])));
    }
    let mut seats = Map::new();
    for (p, st) in &seat_stats {
        let (nick, control, conn) =
            snap.seats
                .iter()
                .find(|s| s.player == *p)
                .map_or((None, "unknown", "none"), |s| match &s.control {
                    SeatControl::Human { nick } => (Some(nick.as_str()), "Human", s.conn),
                    SeatControl::Ai { .. } => (None, "Ai", s.conn),
                    SeatControl::Closed => (None, "Closed", s.conn),
                });
        seats.insert(p.to_string(), seat_json(nick, control, conn, st));
    }
    let mut batch_build = HistSum::default();
    for (_, s) in &seat_stats {
        s.batch_build.add_to(&mut batch_build);
    }
    let pid = gs.worker_pid.load(std::sync::atomic::Ordering::Relaxed);
    let saves_last = gs.last_save().map(|s| {
        json!({
            "seq": s.seq,
            "turn": s.turn,
            "kind": format!("{:?}", s.kind),
            "bytes": s.bytes,
            "applied_upto": s.applied_upto.iter().map(|(p, c)| json!([p, c])).collect::<Vec<_>>(),
        })
    });
    json!({
        "status": status_name(snap.state),
        "turn": snap.turn,
        "phase": phase_name(snap.phase),
        "mode": mode_name(snap.mode),
        "settings": {
            "name": info.settings.name,
            "world_size": info.settings.world_size,
            "map_script": info.settings.map_script,
            "speed": info.settings.speed,
            "turn_mode": mode_name(Some(info.settings.turn_mode)),
        },
        "map": { "w": snap.map.0, "h": snap.map.1 },
        "worker": {
            "pid": (pid != 0).then_some(pid),
            "rss_kb": (pid != 0).then(|| rss_kb(&pid.to_string())).flatten(),
            "starts": get(&gs.worker_starts),
            "crashes": get(&gs.worker_crashes),
            "hang_kills": get(&gs.hang_kills),
            "last_exit": snap.last_exit,
            "boot_ms": get(&gs.boot_ms),
            "restart_ms": gs.restart_ms(),
        },
        "hist": {
            "cmd_gateway_ms": gs.cmd_gateway.json(),
            "step_ms": gs.step.json(),
            "ai_phase_ms": gs.ai_phase.json(),
            "batch_build_ms": batch_build.json(),
            "safe_point_wait_ms": gs.safe_point_wait.json(),
            "restore_ms": gs.restore.json(),
        },
        "cmds": {
            "received": get(&gs.received),
            "forwarded": get(&gs.forwarded),
            "dup_answered": get(&gs.dup_answered),
            "dup_pending_dropped": get(&gs.dup_pending_dropped),
            "expired": get(&gs.expired),
            "id_gap": get(&gs.id_gap),
            "applied": get(&gs.applied),
            "queued": get(&gs.queued),
            "lost": get(&gs.lost),
            "rejected": rejected,
        },
        "views": {
            "keyframes": sum(|s| &s.keyframes),
            "diffs": sum(|s| &s.diffs),
            "basis_sent": sum(|s| &s.basis_sent),
            "basis_skipped": sum(|s| &s.basis_skipped),
            "wantfull": sum(|s| &s.wantfull),
            "hash_fail_worker": sum(|s| &s.hash_fail_worker),
            "need_keyframe": need,
        },
        "replica": replica_json(&gs.replica),
        "queries": { "forwarded": get(&gs.queries), "dropped": get(&gs.queries_dropped) },
        "saves": { "ok": get(&gs.saves_ok), "failed": get(&gs.saves_failed), "last": saves_last },
        "fog_violations": get(&gs.fog_violations),
        "timer": {
            "started": get(&gs.timer_started),
            "expired": get(&gs.timer_expired),
            "paused_ms": get(&gs.timer_paused_ms),
            "extended": get(&gs.timer_extended),
        },
        "gamectl_rejected": ctl_rejected,
        "chat": { "lines": get(&gs.chat_lines), "dropped_rate": get(&gs.chat_dropped_rate) },
        "host": {
            "changes": get(&gs.host_changes),
            "last_change_after_lost_ms": get(&gs.host_last_change_after_lost_ms),
        },
        "seats": seats,
    })
}

/// The body of `GET /admin/metrics` (`schema: 1`).
pub fn render(shared: &Shared) -> Value {
    let m = &shared.metrics;
    let now_ms = shared.config.clock.now_ms();
    let mut by_id = Map::new();
    for g in &shared.games.list() {
        by_id.insert(g.id.to_string(), game_metrics(g));
    }
    json!({
        "schema": 1,
        "build": BUILD,
        "core_build": shared.health.core_build(),
        "protocol": PROTOCOL_VERSION,
        "ipc": IPC_VERSION,
        "uptime_s": now_ms / 1000,
        "now_ms": now_ms,
        "server": {
            "sessions": {
                "open": Metrics::get(&m.sessions_open),
                "logged_in": Metrics::get(&m.sessions_logged_in),
                "opened_total": Metrics::get(&m.sessions_opened_total),
                "lost_timeout_total": Metrics::get(&m.sessions_lost_timeout_total),
                "closed_clean_total": Metrics::get(&m.sessions_closed_clean_total),
                "replaced_total": Metrics::get(&m.sessions_replaced_total),
                "login_failed_total": Metrics::get(&m.login_failed_total),
                "slow_consumer_total": Metrics::get(&m.sessions_slow_consumer_total),
            },
            "games": {
                "running": shared.games.running(),
                "hibernating": shared.games.dormant_list().len(),
                "faulted": shared.games.faulted_total(),
                "limit": shared.config.max_games,
            },
            "mem": { "gateway_rss_kb": rss_kb("self") },
            "replica_store": { "bodies": shared.replica.len(), "bytes": shared.replica.bytes() },
            "disk": {
                "state_free_mb": shared
                    .config
                    .state_dir
                    .as_deref()
                    .and_then(state_free_mb),
            },
        },
        "games": by_id,
    })
}

fn control_json(c: &SeatControl) -> (&'static str, Option<String>, Option<String>) {
    match c {
        SeatControl::Human { nick } => ("Human", Some(nick.clone()), None),
        SeatControl::Ai { kicked } => ("Ai", None, kicked.clone()),
        SeatControl::Closed => ("Closed", None, None),
    }
}

fn seats_json(seats: &[crate::game::SeatSnap]) -> Vec<Value> {
    seats
        .iter()
        .map(|s| {
            let (control, nick, kicked) = control_json(&s.control);
            json!({
                "player": s.player,
                "civ": s.civ_name,
                "control": control,
                "nick": nick,
                "kicked": kicked,
                "conn": s.conn,
            })
        })
        .collect()
}

/// The body of `GET /admin/games`: every game with its seats, the sleeping ones too.
pub fn games_list(shared: &Shared) -> Value {
    let mut list: Vec<Value> = shared
        .games
        .list()
        .iter()
        .map(|g| {
            let snap = g.info.snapshot();
            json!({
                "id": g.id,
                "name": g.info.name,
                "host": snap.host,
                "world_size": g.info.world_size,
                "status": status_name(snap.state),
                "lobby_status": status_of(snap.state).map(|s| format!("{s:?}")),
                "turn": snap.turn,
                "phase": phase_name(snap.phase),
                "seats": seats_json(&snap.seats),
            })
        })
        .collect();
    let choices = shared.choices();
    for m in shared.games.dormant_list() {
        let seats = crate::game::restore::seat_snaps(&m, &choices);
        list.push(json!({
            "id": m.id,
            "name": m.name,
            "host": m.host,
            "world_size": m.settings.world_size,
            "status": "Hibernating",
            "lobby_status": "Hibernated",
            "turn": m.turn,
            "phase": Value::Null,
            "seats": seats_json(&seats),
        }));
    }
    list.sort_by_key(|g| g["id"].as_u64());
    json!({ "games": list })
}

/// The saves of a game on disk: file, number, turn, kind, size (newest first).
fn saves_json(shared: &Shared, id: protocol::GameId) -> Vec<Value> {
    let Some(dir) = shared
        .config
        .state_dir
        .as_ref()
        .map(|d| d.join("games").join(id.to_string()))
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (sub, kind) in [("ring", "Ring"), ("keep", "Keep")] {
        let Ok(read) = std::fs::read_dir(dir.join(sub)) else {
            continue;
        };
        for item in read.flatten() {
            let name = item.file_name().to_string_lossy().into_owned();
            let Some((seq, turn)) = game_host::saves::parse_file_name(&name) else {
                continue;
            };
            let size = item.metadata().map_or(0, |m| m.len());
            out.push(json!({
                "file": format!("{sub}/{name}"),
                "seq": seq,
                "turn": turn,
                "kind": kind,
                "bytes": size,
            }));
        }
    }
    out.sort_by_key(|v| std::cmp::Reverse(v["seq"].as_u64()));
    out
}

/// The body of `GET /admin/games/{id}`: the entry of the list, the settings and the saves.
pub fn game_detail(shared: &Shared, id: protocol::GameId) -> Option<Value> {
    let list = games_list(shared);
    let mut entry = list["games"]
        .as_array()?
        .iter()
        .find(|g| g["id"].as_u64() == Some(u64::from(id)))?
        .clone();
    let settings = match shared.games.get(id) {
        Some(g) => serde_json::to_value(&g.info.settings).ok(),
        None => shared
            .games
            .dormant(id)
            .and_then(|m| serde_json::to_value(&m.settings).ok()),
    };
    let obj = entry.as_object_mut()?;
    obj.insert("settings".into(), settings.unwrap_or(Value::Null));
    obj.insert("saves".into(), Value::Array(saves_json(shared, id)));
    if let Some(g) = shared.games.get(id) {
        obj.insert("metrics".into(), game_metrics(&g));
    }
    Some(entry)
}
