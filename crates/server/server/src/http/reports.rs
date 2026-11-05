//! `POST /client-report/v2`: the web client's full report (packet P4, live report 2026-10-31).
//!
//! The page sends the logs of its ring (the last page loads, from IndexedDB) with an optional comment, as one gzip
//! of a JSON document (plain JSON is taken too), from a logged-in player: the headers `X-Civ5-Nick` (percent-encoded) and `X-Civ5-Token`
//! (the 32 hex digits of the token the WebSocket login uses). The token is checked and never stored.
//!
//! * Size: the body is at most [`MAX_BODY`] (4 MiB; the route's body limit answers 413), and the inflated text at
//!   most [`MAX_INFLATED`] (a bomb guard). A log line over [`MAX_LINE`] bytes is cut.
//! * Limits ([`Limits`]): per nick 3 an hour and 10 a day, per address 5 an hour, 50 a day from everyone; 429 over them.
//! * Storage: `state_dir/reports/<yyyymmdd-hhmmss>-<nick>-<6 hex>/` with `meta.json` and `client.jsonl.gz` (the lines
//!   of all sessions, one gzip). At most [`KEEP_REPORTS`] reports and [`KEEP_BYTES`] bytes, none older than
//!   [`KEEP_DAYS`] days: the oldest go first.
//! * Reading: the admin API (`GET /admin/reports`, `/admin/reports/{id}`, `/admin/reports/{id}/log`,
//!   `/admin/reports/{id}/perf`; the same through `civ5-server admin GET <path>`).
//! * Performance snapshot: a report may carry `perf`, the object the performance panel's "Send report" makes
//!   (`client-app/src/perf_report.rs`: frame summary, pass table, Main by group, counters, graphics, device). It is
//!   kept apart as `perf.json` next to `meta.json` (compact JSON, at most [`MAX_PERF`] bytes; a bigger one is dropped
//!   and `meta.json` says `perf_dropped_bytes`), `meta.json` has `"perf": true`, and the list shows `perf`. Read it
//!   with `GET /admin/reports/{id}/perf`, or `docker --context <host> exec civ5 cat /var/lib/civ5/reports/<id>/perf.json`.
//!
//! * Pinning: a report pins the recording of its game (`state::retention::pin_recording`, `[recording] pin_days`,
//!   `max_pins`): the newest recording of `where.game`, and the ones `recent_recordings` names. `meta.json` gets
//!   `recording` (the directory pinned), and a `Report` record goes into the live recording of the game.
//!
//! SIMPLIFIED(SIMP-uic-50): a report names its game and turn itself (`where`), so a report from a client that was not in
//! a game, or one whose game has no recording any more, pins nothing; `meta.json` has no `recording` then. The pin is
//! made by the game's number, not by anything the server saw of the sender (a player could pin another game's
//! recording by naming it; the cost is one of the three pins).

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::Json;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

use crate::Shared;
use crate::identity::{normalize_nick, token_from_hex};
use crate::state::files::{self, FileMode};

/// The compressed body of a report.
pub const MAX_BODY: usize = 4 * 1024 * 1024;
/// The inflated JSON document (a bomb guard).
pub const MAX_INFLATED: usize = 32 * 1024 * 1024;
/// A log line over this many bytes is cut.
pub const MAX_LINE: usize = 8 * 1024;
/// The comment of the player, in characters.
pub const MAX_COMMENT: usize = 2000;
/// Sessions (page loads) in one report.
const MAX_SESSIONS: usize = 12;
/// A `client`/`where` object over this many bytes of JSON is dropped.
const MAX_SMALL_OBJECT: usize = 4096;
/// The `perf` object of a report over this many bytes of JSON is dropped (a real one is ~50 KB).
pub const MAX_PERF: usize = 512 * 1024;

/// Reports kept, their total size and age.
pub const KEEP_REPORTS: usize = 200;
pub const KEEP_BYTES: u64 = 300 * 1024 * 1024;
pub const KEEP_DAYS: u64 = 14;

const HOUR: Duration = Duration::from_secs(3600);
const DAY: Duration = Duration::from_secs(86_400);
const PER_NICK_HOUR: usize = 3;
const PER_NICK_DAY: usize = 10;
const PER_IP_HOUR: usize = 5;
const TOTAL_DAY: usize = 50;
/// Keys the limiter remembers before it forgets all (a flood of addresses cannot grow it for ever).
const MAX_KEYS: usize = 20_000;

/// Why a report is not taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limited {
    Ip,
    Nick,
    Total,
}

/// The rate limits of reports.
#[derive(Debug, Default)]
pub struct Limits {
    ip: HashMap<String, VecDeque<Instant>>,
    nick: HashMap<String, VecDeque<Instant>>,
    total: VecDeque<Instant>,
}

fn prune(q: &mut VecDeque<Instant>, now: Instant, span: Duration) {
    while q.front().is_some_and(|t| now.duration_since(*t) >= span) {
        q.pop_front();
    }
}

fn count_within(q: &VecDeque<Instant>, now: Instant, span: Duration) -> usize {
    q.iter().filter(|t| now.duration_since(**t) < span).count()
}

impl Limits {
    /// An attempt from `ip` (counted even when the login then fails): over 5 an hour it is refused.
    pub fn attempt(&mut self, ip: &str, now: Instant) -> Result<(), Limited> {
        if self.ip.len() > MAX_KEYS {
            self.ip.clear();
        }
        let q = self.ip.entry(ip.to_owned()).or_default();
        prune(q, now, HOUR);
        if q.len() >= PER_IP_HOUR {
            return Err(Limited::Ip);
        }
        q.push_back(now);
        Ok(())
    }

    /// A report of the (logged-in) `nick` is taken: its limits, and the daily total of everyone; counts it.
    pub fn accept(&mut self, nick: &str, now: Instant) -> Result<(), Limited> {
        if self.nick.len() > MAX_KEYS {
            self.nick.clear();
        }
        prune(&mut self.total, now, DAY);
        if self.total.len() >= TOTAL_DAY {
            return Err(Limited::Total);
        }
        let key = nick.to_lowercase();
        let q = self.nick.entry(key).or_default();
        prune(q, now, DAY);
        if q.len() >= PER_NICK_DAY || count_within(q, now, HOUR) >= PER_NICK_HOUR {
            return Err(Limited::Nick);
        }
        q.push_back(now);
        self.total.push_back(now);
        Ok(())
    }
}

/// The state of the report endpoint kept in [`Shared`].
#[derive(Debug, Default)]
pub struct Reports {
    limits: Mutex<Limits>,
}

// ------------------------------------------------------------------------------------------ parsing

/// A report that passed the checks, ready to store.
#[derive(Debug)]
pub struct Parsed {
    pub comment: String,
    pub client: Value,
    pub place: Value,
    pub recent_recordings: Vec<Value>,
    /// The performance snapshot of the panel, if the report has one and it fits [`MAX_PERF`].
    pub perf: Option<Value>,
    /// The size of a `perf` that was dropped for being too big.
    pub perf_dropped: Option<usize>,
    /// `(start_ms, lines)`: oldest session first.
    pub sessions: Vec<(i64, Vec<String>)>,
}

/// Inflates a gzip body, at most `max` bytes; `None` over the cap or when it is not gzip.
pub fn gunzip_capped(body: &[u8], max: usize) -> Option<Vec<u8>> {
    let dec = flate2::read::GzDecoder::new(body);
    let mut out = Vec::new();
    // One byte more than the cap: reading it tells the cap was crossed.
    dec.take(max as u64 + 1).read_to_end(&mut out).ok()?;
    (out.len() <= max).then_some(out)
}

fn small_object(v: Option<&Value>) -> Value {
    match v {
        Some(v) if v.is_object() && v.to_string().len() <= MAX_SMALL_OBJECT => v.clone(),
        _ => Value::Object(Map::new()),
    }
}

/// Cuts `s` to at most `max` bytes at a character boundary.
fn cut(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Checks the JSON document of a report and cleans it: caps on every field.
pub fn parse(doc: &[u8]) -> Result<Parsed, &'static str> {
    let v: Value = serde_json::from_slice(doc).map_err(|_| "not JSON")?;
    let obj = v.as_object().ok_or("not an object")?;
    if obj.get("v").and_then(Value::as_u64) != Some(2) {
        return Err("not version 2");
    }
    let comment: String = obj
        .get("comment")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .chars()
        .take(MAX_COMMENT)
        .collect();
    let recent_recordings = obj
        .get("recent_recordings")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .take(8)
                .filter_map(|r| {
                    Some(json!({
                        "game": r.get("game")?.as_u64()?,
                        "created_ms": r.get("created_ms").and_then(Value::as_u64).unwrap_or(0),
                    }))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut sessions = Vec::new();
    for s in obj
        .get("sessions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(MAX_SESSIONS)
    {
        let start = s.get("start").and_then(Value::as_i64).unwrap_or(0);
        let text = s.get("lines").and_then(Value::as_str).unwrap_or_default();
        let lines: Vec<String> = text
            .split('\n')
            .filter(|l| !l.trim().is_empty())
            .map(|l| clean_line(l.trim_end_matches('\r')))
            .collect();
        sessions.push((start, lines));
    }
    let (perf, perf_dropped) = match obj.get("perf") {
        Some(v) if v.is_object() => {
            let bytes = v.to_string().len();
            if bytes <= MAX_PERF {
                (Some(v.clone()), None)
            } else {
                (None, Some(bytes))
            }
        }
        _ => (None, None),
    };
    Ok(Parsed {
        comment,
        client: small_object(obj.get("client")),
        place: small_object(obj.get("where")),
        recent_recordings,
        perf,
        perf_dropped,
        sessions,
    })
}

/// One stored line: a JSON object stays as it is (at most [`MAX_LINE`] bytes); anything else, or a longer line, is
/// wrapped as `{"kind":"raw","text":"..."}` with the text cut.
fn clean_line(line: &str) -> String {
    if line.len() <= MAX_LINE
        && let Ok(Value::Object(_)) = serde_json::from_str::<Value>(line)
    {
        return line.to_owned();
    }
    json!({"kind": "raw", "cut": line.len() > MAX_LINE, "text": cut(line, MAX_LINE / 2)})
        .to_string()
}

// ------------------------------------------------------------------------------------------ storage

/// `(year, month, day)` of a day count since 1970-01-01 (the civil-from-days algorithm).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `yyyymmdd-hhmmss` of a Unix time in milliseconds (UTC).
pub fn stamp(unix_ms: u64) -> String {
    let secs = (unix_ms / 1000) as i64;
    let (y, m, d) = civil(secs.div_euclid(86_400));
    let t = secs.rem_euclid(86_400);
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        t / 3600,
        t % 3600 / 60,
        t % 60
    )
}

/// The nick as a piece of a directory name: ASCII letters and digits only, at most 16.
fn nick_part(nick: &str) -> String {
    let s: String = nick
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(16)
        .collect();
    if s.is_empty() { "player".into() } else { s }
}

/// A report id is a directory name this module made: no path piece can get through.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn gzip(bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(6));
    enc.write_all(bytes)?;
    enc.finish()
}

/// Everything about one report that is not the log, as `meta.json`.
pub struct Meta<'a> {
    pub nick: &'a str,
    pub ip: &'a str,
    pub body_bytes: usize,
    pub inflated_bytes: usize,
    pub build: &'a str,
}

/// Which recordings a report pins: the game it says it was in (the newest recording of it), and the exact
/// `g<game>-<created_ms>` of `recent_recordings`, at most four in all.
pub fn pin_targets(p: &Parsed) -> Vec<(u32, Option<u64>)> {
    let mut out: Vec<(u32, Option<u64>)> = Vec::new();
    if let Some(g) = p.place.get("game").and_then(Value::as_u64)
        && let Ok(g) = u32::try_from(g)
    {
        out.push((g, None));
    }
    for r in &p.recent_recordings {
        if let (Some(g), Some(c)) = (
            r.get("game").and_then(Value::as_u64),
            r.get("created_ms").and_then(Value::as_u64),
        ) && let Ok(g) = u32::try_from(g)
            && c != 0
        {
            out.push((g, Some(c)));
        }
    }
    out.truncate(4);
    out
}

/// Pins the recordings of a stored report and notes the first one in its `meta.json`. Returns the directories pinned.
pub fn pin_for_report(
    state_dir: &Path,
    id: &str,
    p: &Parsed,
    now_ms: u64,
    cfg: &crate::config::Recording,
) -> Vec<String> {
    let mut pinned = Vec::new();
    for (game, created) in pin_targets(p) {
        if let Some(name) =
            crate::state::retention::pin_recording(state_dir, game, created, id, now_ms, cfg)
            && !pinned.contains(&name)
        {
            pinned.push(name);
        }
    }
    if !pinned.is_empty() {
        let dir = state_dir.join("reports");
        if let Some(Value::Object(mut m)) = read_meta(&dir, id) {
            m.insert("recording".into(), json!(pinned[0]));
            m.insert("recordings".into(), json!(pinned));
            if let Ok(text) = serde_json::to_string_pretty(&Value::Object(m)) {
                let _ = files::write_atomic(
                    &dir.join(id).join("meta.json"),
                    text.as_bytes(),
                    FileMode::Private,
                );
            }
        }
    }
    pinned
}

/// Writes a report under `reports_dir`; returns its id. Prunes afterwards.
pub fn store(
    reports_dir: &Path,
    p: &Parsed,
    meta: &Meta<'_>,
    now_ms: u64,
) -> anyhow::Result<String> {
    let suffix = files::random_hex(3)?;
    let id = format!("{}-{}-{suffix}", stamp(now_ms), nick_part(meta.nick));
    let dir = reports_dir.join(&id);
    std::fs::create_dir_all(&dir)?;
    let mut log = String::new();
    let mut sessions_meta = Vec::new();
    let mut total_lines = 0usize;
    for (start, lines) in &p.sessions {
        log.push_str(
            &json!({"kind": "session", "src": "client", "start": start, "lines": lines.len()})
                .to_string(),
        );
        log.push('\n');
        for l in lines {
            log.push_str(l);
            log.push('\n');
        }
        total_lines += lines.len();
        sessions_meta.push(json!({"start": start, "lines": lines.len()}));
    }
    let gz = gzip(log.as_bytes())?;
    files::write_atomic(&dir.join("client.jsonl.gz"), &gz, FileMode::Private)?;
    if let Some(perf) = &p.perf {
        files::write_atomic(
            &dir.join("perf.json"),
            perf.to_string().as_bytes(),
            FileMode::Private,
        )?;
    }
    let doc = json!({
        "id": id,
        "time_ms": now_ms,
        "nick": meta.nick,
        "ip": meta.ip,
        "comment": p.comment,
        "client": p.client,
        "where": p.place,
        "recent_recordings": p.recent_recordings,
        "perf": p.perf.is_some(),
        "perf_dropped_bytes": p.perf_dropped,
        "sessions": sessions_meta,
        "lines": total_lines,
        "build": meta.build,
        "sizes": {"body": meta.body_bytes, "inflated": meta.inflated_bytes, "stored": gz.len()},
    });
    files::write_atomic(
        &dir.join("meta.json"),
        serde_json::to_string_pretty(&doc)?.as_bytes(),
        FileMode::Private,
    )?;
    prune_dir(reports_dir, now_ms);
    Ok(id)
}

fn dir_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| e.metadata().ok())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

/// The report ids on disk, oldest first (the id starts with its time stamp).
fn ids(reports_dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(reports_dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().is_dir())
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| valid_id(n))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// Removes the reports over the caps: older than [`KEEP_DAYS`], then the oldest while there are more than
/// [`KEEP_REPORTS`] or more than [`KEEP_BYTES`] bytes.
pub fn prune_dir(reports_dir: &Path, now_ms: u64) {
    prune_with(reports_dir, now_ms, KEEP_REPORTS, KEEP_BYTES, KEEP_DAYS);
}

pub fn prune_with(reports_dir: &Path, now_ms: u64, keep: usize, bytes: u64, days: u64) {
    let all = ids(reports_dir);
    let oldest_kept = stamp(now_ms.saturating_sub(days * 86_400_000));
    let mut sizes: Vec<(String, u64)> = all
        .into_iter()
        .map(|id| {
            let b = dir_bytes(&reports_dir.join(&id));
            (id, b)
        })
        .collect();
    let mut total: u64 = sizes.iter().map(|s| s.1).sum();
    let mut count = sizes.len();
    for (id, b) in sizes.drain(..) {
        let too_old = id.as_str() < oldest_kept.as_str();
        if too_old || count > keep || total > bytes {
            if std::fs::remove_dir_all(reports_dir.join(&id)).is_ok() {
                count -= 1;
                total = total.saturating_sub(b);
            }
        } else {
            break;
        }
    }
}

/// The reports, newest first: `meta.json` with a short comment.
pub fn list(reports_dir: &Path, limit: usize) -> Vec<Value> {
    let mut out = Vec::new();
    for id in ids(reports_dir).into_iter().rev().take(limit) {
        if let Some(Value::Object(m)) = read_meta(reports_dir, &id) {
            let place = m.get("where").cloned().unwrap_or(Value::Null);
            let comment: String = m
                .get("comment")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .chars()
                .take(80)
                .collect();
            out.push(json!({
                "id": id,
                "time_ms": m.get("time_ms"),
                "nick": m.get("nick"),
                "game": place.get("game"),
                "turn": place.get("turn"),
                "lines": m.get("lines"),
                "perf": m.get("perf"),
                "bytes": m.get("sizes").and_then(|s| s.get("stored")),
                "comment": comment,
            }));
        }
    }
    out
}

pub fn read_meta(reports_dir: &Path, id: &str) -> Option<Value> {
    if !valid_id(id) {
        return None;
    }
    let text = std::fs::read_to_string(reports_dir.join(id).join("meta.json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// The `perf.json` of a report: the performance snapshot it carried.
pub fn read_perf(reports_dir: &Path, id: &str) -> Option<Value> {
    if !valid_id(id) {
        return None;
    }
    let text = std::fs::read_to_string(reports_dir.join(id).join("perf.json")).ok()?;
    serde_json::from_str(&text).ok()
}

fn level_rank(name: &str) -> Option<u8> {
    Some(match name {
        "error" => 1,
        "warn" => 2,
        "info" => 3,
        "debug" => 4,
        "trace" => 5,
        _ => return None,
    })
}

/// The log of a report as text: the lines with `grep` in them (any case) and at `level` or more important (spans and
/// lines with no level always), the last `limit`. Control characters are escaped (the log is data).
pub fn read_log(
    reports_dir: &Path,
    id: &str,
    grep: Option<&str>,
    level: Option<&str>,
    limit: usize,
) -> Option<String> {
    if !valid_id(id) {
        return None;
    }
    let gz = std::fs::read(reports_dir.join(id).join("client.jsonl.gz")).ok()?;
    let text = String::from_utf8_lossy(&gunzip_capped(&gz, MAX_INFLATED)?).into_owned();
    let want = level.and_then(level_rank);
    let grep = grep.map(str::to_lowercase);
    let mut kept: VecDeque<&str> = VecDeque::new();
    for line in text.lines() {
        if let Some(g) = &grep
            && !line.to_lowercase().contains(g)
        {
            continue;
        }
        if let Some(w) = want
            && let Some(rank) = line_level(line)
            && rank > w
        {
            continue;
        }
        if kept.len() == limit.max(1) {
            kept.pop_front();
        }
        kept.push_back(line);
    }
    let mut out = String::new();
    for l in kept {
        for c in l.chars() {
            if c.is_control() {
                out.extend(c.escape_default());
            } else {
                out.push(c);
            }
        }
        out.push('\n');
    }
    Some(out)
}

/// The level of a stored line (`"lvl":"warn"` or a number), if it has one.
fn line_level(line: &str) -> Option<u8> {
    let v: Value = serde_json::from_str(line).ok()?;
    match v.get("lvl")? {
        Value::String(s) => level_rank(s),
        Value::Number(n) => n.as_u64().map(|n| n.min(5) as u8),
        _ => None,
    }
}

// ------------------------------------------------------------------------------------------ the route

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn header<'a>(h: &'a HeaderMap, name: &str) -> &'a str {
    h.get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
}

fn answer(status: StatusCode, text: &str) -> Response {
    (status, Json(json!({ "error": text }))).into_response()
}

pub async fn post(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    State(shared): State<Arc<Shared>>,
    body: Bytes,
) -> Response {
    let Some(state_dir) = shared.config.state_dir.clone() else {
        return answer(
            StatusCode::SERVICE_UNAVAILABLE,
            "this server keeps no reports",
        );
    };
    let ip = super::client_ip(peer.ip(), &headers, &shared.config.trusted_proxies).to_string();
    let now = Instant::now();
    if shared
        .reports
        .limits
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .attempt(&ip, now)
        .is_err()
    {
        return answer(
            StatusCode::TOO_MANY_REQUESTS,
            "too many reports from this address",
        );
    }
    // Logged-in players only: the nick and the token of the WebSocket login.
    // The token comes in a header (native tools, an old page) or in the session cookie of the page.
    let nick = percent_decode(header(&headers, "x-civ5-nick")).and_then(|n| normalize_nick(&n));
    let token = token_from_hex(header(&headers, "x-civ5-token")).or_else(|| {
        let (cookie_nick, token) = crate::cookie::from_headers(&headers)?;
        let nick = nick.as_deref()?;
        (crate::identity::nick_key(&cookie_nick) == crate::identity::nick_key(nick))
            .then_some(token)
    });
    let (Some(nick), Some(token)) = (nick, token) else {
        return answer(StatusCode::UNAUTHORIZED, "reports need a logged-in player");
    };
    if !shared.identity.verify(&nick, &token) {
        return answer(StatusCode::UNAUTHORIZED, "reports need a logged-in player");
    }
    if shared
        .reports
        .limits
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .accept(&nick, now)
        .is_err()
    {
        return answer(
            StatusCode::TOO_MANY_REQUESTS,
            "too many reports from this player",
        );
    }
    let body_len = body.len();
    // A gzip body is known by its first two bytes (no header needed: a proxy may drop `Content-Encoding`).
    let gzip_body = body.starts_with(&[0x1f, 0x8b]);
    let wall = shared.config.wall_clock.unix_ms();
    let rec_cfg = shared.config.recording.clone();
    let nick_for_log = nick.clone();
    let result =
        tokio::task::spawn_blocking(move || -> Result<StoredReport, (StatusCode, String)> {
            let doc = if gzip_body {
                gunzip_capped(&body, MAX_INFLATED).ok_or((
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "the body does not inflate within the cap".into(),
                ))?
            } else {
                body.to_vec()
            };
            let parsed = parse(&doc).map_err(|e| (StatusCode::BAD_REQUEST, e.to_owned()))?;
            let meta = Meta {
                nick: &nick,
                ip: &ip,
                body_bytes: body_len,
                inflated_bytes: doc.len(),
                build: crate::BUILD,
            };
            let id = store(&state_dir.join("reports"), &parsed, &meta, wall)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))?;
            let pinned = pin_for_report(&state_dir, &id, &parsed, wall, &rec_cfg);
            Ok(StoredReport {
                id,
                pinned,
                game: parsed.place.get("game").and_then(Value::as_u64),
                turn: parsed.place.get("turn").and_then(Value::as_i64),
                player: parsed.place.get("player").and_then(Value::as_i64),
            })
        })
        .await;
    match result {
        Ok(Ok(stored)) => {
            let id = stored.id.clone();
            // The live recording of the game hears of the report (the tool jumps to its turn).
            if let Some(game) = stored.game.and_then(|g| u32::try_from(g).ok())
                && let Some(h) = shared.games.get(game)
            {
                h.info
                    .rec
                    .put(recording::Body::Report(recording::format::ReportRec {
                        id: id.clone(),
                        player: stored.player.and_then(|p| i8::try_from(p).ok()),
                        nick: nick_for_log.clone(),
                        turn: stored.turn.and_then(|t| i32::try_from(t).ok()),
                    }));
            }
            tracing::info!(target: "client_report", %id, bytes = body_len, pinned = ?stored.pinned, "client report v2 stored");
            (StatusCode::CREATED, Json(json!({ "id": id }))).into_response()
        }
        Ok(Err((status, why))) => {
            if status == StatusCode::INTERNAL_SERVER_ERROR {
                tracing::error!(target: "client_report", "cannot store a report: {why}");
                answer(status, "cannot store the report")
            } else {
                answer(status, &why)
            }
        }
        Err(_) => answer(StatusCode::INTERNAL_SERVER_ERROR, "cannot store the report"),
    }
}

// ------------------------------------------------------------------------------------------ the admin API

/// What the blocking part of `post` hands back.
struct StoredReport {
    id: String,
    pinned: Vec<String>,
    game: Option<u64>,
    turn: Option<i64>,
    player: Option<i64>,
}

fn reports_dir(shared: &Shared) -> Option<PathBuf> {
    shared.config.state_dir.as_ref().map(|d| d.join("reports"))
}

pub async fn admin_list(
    State(shared): State<Arc<Shared>>,
    axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
) -> Response {
    let Some(dir) = reports_dir(&shared) else {
        return Json(json!({ "reports": [] })).into_response();
    };
    let limit = q
        .get("limit")
        .and_then(|l| l.parse().ok())
        .unwrap_or(50usize)
        .min(500);
    let reports = tokio::task::spawn_blocking(move || list(&dir, limit))
        .await
        .unwrap_or_default();
    Json(json!({ "reports": reports })).into_response()
}

pub async fn admin_one(
    State(shared): State<Arc<Shared>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    let meta = reports_dir(&shared).and_then(|d| read_meta(&d, &id));
    match meta {
        Some(m) => Json(m).into_response(),
        None => answer(StatusCode::NOT_FOUND, "no such report"),
    }
}

pub async fn admin_perf(
    State(shared): State<Arc<Shared>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response {
    let perf = reports_dir(&shared).and_then(|d| read_perf(&d, &id));
    match perf {
        Some(p) => Json(p).into_response(),
        None => answer(
            StatusCode::NOT_FOUND,
            "no such report, or it has no perf section",
        ),
    }
}

pub async fn admin_log(
    State(shared): State<Arc<Shared>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>,
) -> Response {
    let Some(dir) = reports_dir(&shared) else {
        return answer(StatusCode::NOT_FOUND, "no such report");
    };
    let limit = q
        .get("limit")
        .and_then(|l| l.parse().ok())
        .unwrap_or(2000usize)
        .min(200_000);
    let (grep, level) = (q.get("grep").cloned(), q.get("level").cloned());
    let text = tokio::task::spawn_blocking(move || {
        read_log(&dir, &id, grep.as_deref(), level.as_deref(), limit)
    })
    .await
    .ok()
    .flatten();
    match text {
        Some(t) => (
            StatusCode::OK,
            [(
                axum::http::header::CONTENT_TYPE,
                "text/plain; charset=utf-8",
            )],
            t,
        )
            .into_response(),
        None => answer(StatusCode::NOT_FOUND, "no such report"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("civ5-reports-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn doc(comment: &str, lines: &str) -> Vec<u8> {
        json!({
            "v": 2, "comment": comment,
            "client": {"build": "b1"}, "where": {"game": 7, "turn": 126},
            "recent_recordings": [{"game": 7, "created_ms": 5}],
            "sessions": [{"start": 1000, "lines": lines}],
        })
        .to_string()
        .into_bytes()
    }

    fn meta(nick: &str) -> Meta<'_> {
        Meta {
            nick,
            ip: "10.0.0.1",
            body_bytes: 10,
            inflated_bytes: 20,
            build: "t",
        }
    }

    #[test]
    fn the_limits_are_per_address_nick_and_in_total() {
        let t0 = Instant::now();
        let mut l = Limits::default();
        for _ in 0..PER_IP_HOUR {
            assert!(l.attempt("1.1.1.1", t0).is_ok());
        }
        assert_eq!(l.attempt("1.1.1.1", t0), Err(Limited::Ip));
        assert!(l.attempt("1.1.1.1", t0 + HOUR).is_ok(), "a new hour");
        for _ in 0..PER_NICK_HOUR {
            assert!(l.accept("Ann", t0).is_ok());
        }
        assert_eq!(
            l.accept("ann", t0),
            Err(Limited::Nick),
            "case does not matter"
        );
        // The daily limit of a nick: the hours pass, the day does not.
        let mut taken = PER_NICK_HOUR;
        for h in 1..6u64 {
            for _ in 0..PER_NICK_HOUR {
                if l.accept("Ann", t0 + HOUR * h as u32).is_ok() {
                    taken += 1;
                }
            }
        }
        assert_eq!(taken, PER_NICK_DAY);
        // From everyone: 50 a day.
        let mut l = Limits::default();
        let mut taken = 0;
        for i in 0..TOTAL_DAY * 2 {
            if l.accept(&format!("p{i}"), t0).is_ok() {
                taken += 1;
            }
        }
        assert_eq!(taken, TOTAL_DAY);
        assert!(l.accept("late", t0 + DAY).is_ok());
    }

    #[test]
    fn a_report_pins_the_recording_of_its_game_and_says_so_in_its_meta() {
        let dir = scratch("pin");
        let rec = dir.join("recordings").join("g7-5");
        std::fs::create_dir_all(&rec).unwrap();
        std::fs::write(rec.join("seg-0000.c5rc"), b"x").unwrap();
        let p = parse(&doc("it broke", "{\"kind\":\"x\"}\n")).unwrap();
        assert_eq!(pin_targets(&p), vec![(7, None), (7, Some(5))]);
        let t = 1_700_000_000_000;
        let id = store(&dir.join("reports"), &p, &meta("Ann"), t).unwrap();
        let cfg = crate::config::Recording::default();
        let pinned = pin_for_report(&dir, &id, &p, t, &cfg);
        assert_eq!(pinned, vec!["g7-5".to_owned()]);
        let m = read_meta(&dir.join("reports"), &id).unwrap();
        assert_eq!(m["recording"], "g7-5");
        let pin = recording::reader::read_pin(&rec).unwrap();
        assert_eq!(pin.report, id);
        assert_eq!(pin.until_ms, t + 14 * 86_400_000);
        // A report from a client that was in no game pins nothing.
        let mut none = parse(&doc("x", "{\"kind\":\"x\"}\n")).unwrap();
        none.place = json!({});
        none.recent_recordings.clear();
        let id2 = store(&dir.join("reports"), &none, &meta("Bob"), t + 1000).unwrap();
        assert!(pin_for_report(&dir, &id2, &none, t, &cfg).is_empty());
        assert!(
            read_meta(&dir.join("reports"), &id2)
                .unwrap()
                .get("recording")
                .is_none()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_stamp_is_utc_calendar_time() {
        assert_eq!(stamp(0), "19700101-000000");
        assert_eq!(stamp(1_761_909_483_123), "20251031-111803");
        assert_eq!(stamp(951_782_400_000 + 86_399_000), "20000229-235959");
    }

    #[test]
    fn a_bomb_is_stopped_at_the_cap_and_a_normal_body_inflates() {
        let big = vec![b'a'; 100_000];
        let gz = gzip(&big).unwrap();
        assert!(gz.len() < 1000);
        assert_eq!(gunzip_capped(&gz, 100_000).unwrap().len(), 100_000);
        assert!(gunzip_capped(&gz, 99_999).is_none());
        assert!(gunzip_capped(b"not gzip", 100).is_none());
    }

    #[test]
    fn parse_caps_and_cleans_the_document() {
        let long = "x".repeat(MAX_LINE * 2);
        let lines = format!("{{\"lvl\":\"warn\",\"msg\":\"a\"}}\n\nnot json\n{long}\n");
        let p = parse(&doc(&"c".repeat(MAX_COMMENT + 50), &lines)).unwrap();
        assert_eq!(p.comment.chars().count(), MAX_COMMENT);
        assert_eq!(p.sessions.len(), 1);
        let l = &p.sessions[0].1;
        assert_eq!(l.len(), 3, "the empty line is gone: {l:?}");
        assert_eq!(l[0], "{\"lvl\":\"warn\",\"msg\":\"a\"}");
        assert!(l[1].contains("\"raw\""), "{}", l[1]);
        assert!(l[2].len() <= MAX_LINE, "{}", l[2].len());
        assert!(l[2].contains("\"cut\":true"));
        assert_eq!(parse(b"[]").unwrap_err(), "not an object");
        assert_eq!(parse(br#"{"v":1}"#).unwrap_err(), "not version 2");
        assert_eq!(parse(b"{").unwrap_err(), "not JSON");
    }

    #[test]
    fn a_report_is_stored_listed_read_and_pruned_oldest_first() {
        let dir = scratch("store");
        let lines = "{\"lvl\":\"error\",\"msg\":\"boom\"}\n{\"lvl\":\"debug\",\"msg\":\"detail\"}\n{\"kind\":\"span\",\"span\":\"c.load\"}";
        let p = parse(&doc("it froze", lines)).unwrap();
        let t = 1_761_909_483_000;
        let a = store(&dir, &p, &meta("Ann"), t).unwrap();
        assert!(a.starts_with("20251031-111803-Ann-"), "{a}");
        assert!(dir.join(&a).join("client.jsonl.gz").exists());
        let m = read_meta(&dir, &a).unwrap();
        assert_eq!(m["comment"], "it froze");
        assert_eq!(m["where"]["turn"], 126);
        let rows = list(&dir, 10);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["game"], 7);
        let all = read_log(&dir, &a, None, None, 100).unwrap();
        assert!(all.contains("\"session\"") && all.contains("boom") && all.contains("detail"));
        let errs = read_log(&dir, &a, None, Some("error"), 100).unwrap();
        assert!(errs.contains("boom") && !errs.contains("detail"));
        assert!(errs.contains("c.load"), "spans have no level and stay");
        let g = read_log(&dir, &a, Some("DETAIL"), None, 100).unwrap();
        assert_eq!(g.lines().count(), 1);
        let last = read_log(&dir, &a, None, None, 1).unwrap();
        assert_eq!(last.lines().count(), 1);
        // Ids that are not ours never touch the disk.
        for bad in ["..", "../x", "a/b", "", "a b"] {
            assert!(read_meta(&dir, bad).is_none(), "{bad}");
            assert!(read_log(&dir, bad, None, None, 1).is_none(), "{bad}");
        }
        // Pruning: by count the oldest goes; by age too.
        let b = store(&dir, &p, &meta("Bob"), t + 1000).unwrap();
        let c = store(&dir, &p, &meta("Cy"), t + 2000).unwrap();
        prune_with(&dir, t + 2000, 2, u64::MAX, 14);
        assert_eq!(ids(&dir), {
            let mut v = vec![b.clone(), c.clone()];
            v.sort();
            v
        });
        prune_with(&dir, t + 2000, 200, 1, 14);
        assert!(
            ids(&dir).is_empty(),
            "over the byte cap everything goes, oldest first"
        );
        let old = store(&dir, &p, &meta("Old"), t).unwrap();
        prune_with(&dir, t + 15 * 86_400_000, 200, u64::MAX, 14);
        assert!(!dir.join(old).exists(), "older than 14 days");
    }

    #[test]
    fn the_perf_section_of_a_report_is_kept_apart_and_read_back() {
        let dir = scratch("perf");
        let mut d: Value = serde_json::from_slice(&doc("slow", "{\"kind\":\"x\"}\n")).unwrap();
        d["perf"] = json!({"v": 1, "passes": [{"name": "Main", "gpu_ms": 19.3}]});
        let p = parse(d.to_string().as_bytes()).unwrap();
        assert_eq!(p.perf.as_ref().unwrap()["passes"][0]["name"], "Main");
        let id = store(&dir, &p, &meta("Ann"), 1_761_909_483_000).unwrap();
        assert!(dir.join(&id).join("perf.json").exists());
        assert_eq!(read_meta(&dir, &id).unwrap()["perf"], true);
        assert_eq!(list(&dir, 5)[0]["perf"], true);
        assert_eq!(read_perf(&dir, &id).unwrap()["passes"][0]["gpu_ms"], 19.3);
        // A report without one: no file, `perf` false, nothing to read.
        let plain = parse(&doc("x", "{\"kind\":\"x\"}\n")).unwrap();
        let id2 = store(&dir, &plain, &meta("Bob"), 1_761_909_484_000).unwrap();
        assert!(!dir.join(&id2).join("perf.json").exists());
        assert_eq!(read_meta(&dir, &id2).unwrap()["perf"], false);
        assert!(read_perf(&dir, &id2).is_none());
        // Not an object, or too big: dropped, the report is still taken.
        d["perf"] = json!("text");
        assert!(parse(d.to_string().as_bytes()).unwrap().perf.is_none());
        d["perf"] = json!({"blob": "x".repeat(MAX_PERF)});
        let big = parse(d.to_string().as_bytes()).unwrap();
        assert!(big.perf.is_none() && big.perf_dropped.unwrap() > MAX_PERF);
        for bad in ["..", "a/b", ""] {
            assert!(read_perf(&dir, bad).is_none(), "{bad}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nicks_and_percent_encoding() {
        assert_eq!(nick_part("Иван"), "player");
        assert_eq!(nick_part("a/../b c"), "abc");
        assert_eq!(percent_decode("Ann%20B").as_deref(), Some("Ann B"));
        assert_eq!(percent_decode("%D0%9D").as_deref(), Some("Н"));
        assert!(percent_decode("%zz").is_none());
        assert!(percent_decode("%").is_none());
    }
}
