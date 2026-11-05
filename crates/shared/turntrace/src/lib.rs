//! The per-turn timeline (mp-latency-timeline spec, section 3): spans of the end of a turn on the server (the worker,
//! the gateway) in the shared envelope of the logging spec (`ts`, `kind`, `src`, `game`, `turn`, `seat`, `trace`,
//! `span`, `id`, `parent`, `t0_us`, `dur_us`, `args`), a Chrome/Perfetto export and the one-line summary of a turn.
//!
//! No I/O: the clock is passed in ([`Clock`]); the worker and the gateway use [`MonoClock`], the tests a manual one.
//! A trace is keyed by `(game, turn)` where `turn` is the turn being ended, so every span of one end of turn shares
//! the trace id `"<game>:<turn>"` without a new field on the wire.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// A source of unix microseconds. Monotone within a process.
pub trait Clock: Send + Sync {
    fn now_us(&self) -> u64;
}

/// Unix time at creation plus the monotonic time since: spans of one process never run backwards, and the worker and
/// the gateway (one machine) share the axis to within the clock's step.
#[derive(Debug)]
pub struct MonoClock {
    base_us: u64,
    start: Instant,
}

impl Default for MonoClock {
    fn default() -> Self {
        let base_us = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_micros()).unwrap_or(u64::MAX));
        MonoClock {
            base_us,
            start: Instant::now(),
        }
    }
}

impl Clock for MonoClock {
    fn now_us(&self) -> u64 {
        self.base_us + u64::try_from(self.start.elapsed().as_micros()).unwrap_or(u64::MAX)
    }
}

/// How much is recorded (`CIV5_TRACE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Level {
    /// Nothing: every call is a branch on this flag.
    Off,
    /// The catalog spans (default).
    Coarse,
    /// Coarse plus the fine spans.
    Fine,
}

impl Level {
    /// `off`, `coarse`, `fine` (anything else: `None`).
    pub fn parse(s: &str) -> Option<Level> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "0" | "none" => Some(Level::Off),
            "coarse" | "on" | "1" => Some(Level::Coarse),
            "fine" | "2" => Some(Level::Fine),
            _ => None,
        }
    }

    /// The level `CIV5_TRACE` names; unset or unknown: [`Level::Coarse`].
    pub fn from_env() -> Level {
        std::env::var("CIV5_TRACE")
            .ok()
            .and_then(|v| Level::parse(&v))
            .unwrap_or(Level::Coarse)
    }
}

/// Where a record comes from (`src`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Src {
    Gateway,
    Worker,
    Client,
    Tool,
}

impl Src {
    pub fn name(self) -> &'static str {
        match self {
            Src::Gateway => "gateway",
            Src::Worker => "worker",
            Src::Client => "client",
            Src::Tool => "tool",
        }
    }
}

/// What a record is (`kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    /// Has `t0_us` and `dur_us`.
    Span,
    /// An instant: `t0_us` only.
    Mark,
    /// A value in `args` (`value`).
    Counter,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Span => "span",
            Kind::Mark => "mark",
            Kind::Counter => "counter",
        }
    }
}

/// One record. Variant and field order is the IPC wire format of `WorkerMsg::Trace`: append only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rec {
    pub kind: Kind,
    pub src: Src,
    /// The seat; -1 for server-wide.
    pub seat: i16,
    /// The dotted stage name of the catalog (`w.ai`).
    pub span: String,
    /// The lane (a thread of the Chrome export): `core`, `pack`, `saver`, `io`, `http`, `main`, `fetch`.
    pub lane: String,
    /// Unique within `(src, seat, trace)`; 0 is "no id".
    pub id: u32,
    /// The id of the enclosing span, 0 = root.
    pub parent: u32,
    /// Start, unix microseconds on the server clock.
    pub t0_us: u64,
    pub dur_us: u64,
    /// Small numbers: sizes, a command id, a jseq, a hash (as the bits of an i64), counts.
    pub args: Vec<(String, i64)>,
}

impl Rec {
    /// The end of the span.
    pub fn end_us(&self) -> u64 {
        self.t0_us.saturating_add(self.dur_us)
    }

    /// The value of an argument.
    pub fn arg(&self, name: &str) -> Option<i64> {
        self.args.iter().find(|(k, _)| k == name).map(|(_, v)| *v)
    }
}

/// The records of one end of a turn (`trace` = `"<game>:<turn>"`, `turn` the turn that was ended).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnTrace {
    pub game: u64,
    pub turn: i32,
    pub recs: Vec<Rec>,
}

impl TurnTrace {
    pub fn new(game: u64, turn: i32) -> Self {
        TurnTrace {
            game,
            turn,
            recs: Vec::new(),
        }
    }

    /// `"<game>:<turn>"`.
    pub fn trace_id(&self) -> String {
        format!("{}:{}", self.game, self.turn)
    }

    /// The first span or mark named `name`.
    pub fn find(&self, name: &str) -> Option<&Rec> {
        self.recs.iter().find(|r| r.span == name)
    }

    /// Every record named `name`.
    pub fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Rec> {
        self.recs.iter().filter(move |r| r.span == name)
    }

    /// The earliest start.
    pub fn first_us(&self) -> Option<u64> {
        self.recs.iter().map(|r| r.t0_us).min()
    }

    /// The latest end.
    pub fn last_us(&self) -> Option<u64> {
        self.recs.iter().map(Rec::end_us).max()
    }

    /// Adds the records of `other` (the same trace) that are not here yet (same source, seat, name and start).
    pub fn merge(&mut self, other: TurnTrace) {
        for r in other.recs {
            let dup = self.recs.iter().any(|x| {
                x.src == r.src && x.seat == r.seat && x.span == r.span && x.t0_us == r.t0_us
            });
            if !dup {
                self.recs.push(r);
            }
        }
        self.recs.sort_by_key(|r| (r.t0_us, r.id));
    }

    /// The envelope lines of the records, one JSON object each (the game log's `span` / `mark` / `counter` lines).
    /// `ts` is the unix ms at which the line is written.
    pub fn lines(&self, ts_ms: u64) -> Vec<Value> {
        let trace = self.trace_id();
        self.recs
            .iter()
            .map(|r| {
                let mut o = Map::new();
                o.insert("ts".into(), json!(ts_ms));
                o.insert("kind".into(), json!(r.kind.name()));
                o.insert("src".into(), json!(r.src.name()));
                o.insert("game".into(), json!(self.game));
                o.insert("turn".into(), json!(self.turn));
                o.insert("seat".into(), json!(r.seat));
                o.insert("trace".into(), json!(trace));
                o.insert("span".into(), json!(r.span));
                o.insert("id".into(), json!(r.id));
                o.insert("parent".into(), json!(r.parent));
                o.insert("lane".into(), json!(r.lane));
                o.insert("t0_us".into(), json!(r.t0_us));
                if r.kind == Kind::Span {
                    o.insert("dur_us".into(), json!(r.dur_us));
                }
                o.insert("args".into(), args_json(&r.args));
                Value::Object(o)
            })
            .collect()
    }
}

fn args_json(args: &[(String, i64)]) -> Value {
    let mut m = Map::new();
    for (k, v) in args {
        m.insert(k.clone(), json!(v));
    }
    Value::Object(m)
}

/// The Chrome Trace Event JSON of several turns (opens in `ui.perfetto.dev` or `chrome://tracing`). `ts` is the
/// microseconds since the first record of all the traces; one process per source and seat (gateway, worker, client
/// p0 ...), one thread per lane.
pub fn chrome(traces: &[&TurnTrace]) -> Value {
    let t_base = traces
        .iter()
        .filter_map(|t| t.first_us())
        .min()
        .unwrap_or(0);
    // Stable numbering: pids by (src, seat) in order of appearance sorted, tids by lane name.
    let mut procs: BTreeMap<(Src, i16), u32> = BTreeMap::new();
    let mut lanes: BTreeMap<((Src, i16), String), u32> = BTreeMap::new();
    for t in traces {
        for r in &t.recs {
            procs.entry((r.src, r.seat)).or_insert(0);
        }
    }
    for (i, v) in procs.values_mut().enumerate() {
        *v = i as u32 + 1;
    }
    for t in traces {
        for r in &t.recs {
            let n = lanes.len() as u32 + 1;
            lanes.entry(((r.src, r.seat), r.lane.clone())).or_insert(n);
        }
    }
    let mut events: Vec<Value> = Vec::new();
    for (&(src, seat), &pid) in &procs {
        let name = if seat >= 0 {
            format!("{} p{seat}", src.name())
        } else {
            src.name().to_owned()
        };
        events.push(json!({"ph": "M", "name": "process_name", "pid": pid, "args": {"name": name}}));
    }
    for (((src, seat), lane), &tid) in &lanes {
        let pid = procs.get(&(*src, *seat)).copied().unwrap_or(0);
        events.push(json!({"ph": "M", "name": "thread_name", "pid": pid, "tid": tid, "args": {"name": lane}}));
    }
    for t in traces {
        let trace = t.trace_id();
        for r in &t.recs {
            let pid = procs.get(&(r.src, r.seat)).copied().unwrap_or(0);
            let tid = lanes
                .get(&((r.src, r.seat), r.lane.clone()))
                .copied()
                .unwrap_or(0);
            let ts = r.t0_us.saturating_sub(t_base);
            let mut args = match args_json(&r.args) {
                Value::Object(m) => m,
                _ => Map::new(),
            };
            args.insert("trace".into(), json!(trace));
            let ev = match r.kind {
                Kind::Span => {
                    json!({"ph": "X", "name": r.span, "cat": r.src.name(), "pid": pid, "tid": tid,
                    "ts": ts, "dur": r.dur_us, "args": args})
                }
                Kind::Mark => {
                    json!({"ph": "i", "s": "t", "name": r.span, "cat": r.src.name(), "pid": pid,
                    "tid": tid, "ts": ts, "args": args})
                }
                Kind::Counter => {
                    let v = r.arg("value").unwrap_or(0);
                    json!({"ph": "C", "name": r.span, "cat": r.src.name(), "pid": pid, "tid": tid, "ts": ts,
                        "args": {"value": v}})
                }
            };
            events.push(ev);
        }
    }
    json!({"traceEvents": events, "displayTimeUnit": "ms"})
}

/// The numbers of one end of a turn: where the wall time from the first END_TURN to the turn's snapshot went. They
/// add up to `total_us`; `other_us` holds what no stage claims and is shown, never hidden.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub game: u64,
    pub turn: i32,
    /// First END_TURN applied (or accepted) -> the turn's snapshot left the worker.
    pub total_us: u64,
    /// The first human's END_TURN applied -> the last one's.
    pub wait_others_us: u64,
    /// The last END_TURN applied (and its checked batch) -> the AI phase starts: orders and the replica batch.
    pub order_us: u64,
    /// The AI phase (`w.ai`), as measured around the pump calls.
    pub server_ai_us: u64,
    /// The turn change step after the AI, the turn-start save, the pack (`w.turn_change`, `w.pack`, `w.patch`).
    pub server_post_us: u64,
    /// Not claimed by any stage.
    pub other_us: u64,
    /// The pump time summed over `w.ai`'s calls (for the 5 % check against `server_ai_us`).
    pub pump_us: u64,
    pub ai_slices: u32,
    pub snapshot_bytes: u64,
    pub patch_bytes: u64,
}

impl Summary {
    /// The one line of the game log (`turn_summary`).
    pub fn line(&self, ts_ms: u64) -> Value {
        json!({
            "ts": ts_ms, "kind": "turn_summary", "src": "gateway", "game": self.game, "turn": self.turn,
            "seat": -1, "trace": format!("{}:{}", self.game, self.turn), "args": {
                "total_us": self.total_us, "wait_others_us": self.wait_others_us, "order_us": self.order_us,
                "server_ai_us": self.server_ai_us, "server_post_us": self.server_post_us,
                "other_us": self.other_us, "pump_us": self.pump_us, "ai_slices": self.ai_slices,
                "snapshot_bytes": self.snapshot_bytes, "patch_bytes": self.patch_bytes,
            }
        })
    }

    /// A line for people: `turn 126: total 4.2 s = wait 0.0 + order 0.1 + ai 3.5 + post 0.6 + other 0.0`.
    pub fn text(&self) -> String {
        let s = |us: u64| us as f64 / 1e6;
        format!(
            "game {} turn {}: total {:.3} s = wait_others {:.3} + order {:.3} + server_ai {:.3} + server_post {:.3} + other {:.3} (pump {:.3}, ai slices {}, snapshot {} B, patch {} B)",
            self.game,
            self.turn,
            s(self.total_us),
            s(self.wait_others_us),
            s(self.order_us),
            s(self.server_ai_us),
            s(self.server_post_us),
            s(self.other_us),
            s(self.pump_us),
            self.ai_slices,
            self.snapshot_bytes,
            self.patch_bytes
        )
    }
}

/// The union length of intervals clipped to `[lo, hi]` (overlaps counted once).
fn union_len(mut v: Vec<(u64, u64)>, lo: u64, hi: u64) -> u64 {
    v.retain(|&(a, b)| b > a);
    for iv in &mut v {
        iv.0 = iv.0.clamp(lo, hi);
        iv.1 = iv.1.clamp(lo, hi);
    }
    v.sort_unstable();
    let (mut total, mut cur_end) = (0u64, lo);
    for (a, b) in v {
        let a = a.max(cur_end);
        if b > a {
            total += b - a;
            cur_end = b;
        }
    }
    total
}

/// The summary of a worker trace. The window is the first `w.end_turn` mark (or the first `w.apply`) to the end of the
/// last `w.pack` (the snapshot leaves the worker with its pack; without a pack the end of `w.turn_change`).
pub fn summarize(t: &TurnTrace) -> Summary {
    let mut s = Summary {
        game: t.game,
        turn: t.turn,
        ..Summary::default()
    };
    let ends: Vec<&Rec> = t.all("w.end_turn").collect();
    let start = ends
        .iter()
        .map(|r| r.t0_us)
        .min()
        .or_else(|| t.find("w.apply").map(|r| r.t0_us))
        .or_else(|| t.first_us());
    let Some(start) = start else {
        return s;
    };
    let end = t
        .all("w.pack")
        .chain(t.all("w.patch"))
        .map(Rec::end_us)
        .max()
        .or_else(|| t.find("w.turn_change").map(Rec::end_us))
        .unwrap_or(start)
        .max(start);
    s.total_us = end - start;
    if let (Some(a), Some(b)) = (
        ends.iter().map(|r| r.t0_us).min(),
        ends.iter().map(|r| r.t0_us).max(),
    ) {
        s.wait_others_us = b - a;
    }
    // After the last END_TURN of the humans: the orders, the AI phase, then the turn change, the save and the pack.
    let last_end = ends.iter().map(|r| r.t0_us).max().unwrap_or(start);
    let ai: Vec<(u64, u64)> = t.all("w.ai").map(|r| (r.t0_us, r.end_us())).collect();
    let ai_start = ai.iter().map(|x| x.0).min().unwrap_or(end);
    let ai_end = ai.iter().map(|x| x.1).max().unwrap_or(last_end);
    s.server_ai_us = union_len(ai, last_end, end);
    s.pump_us = t
        .all("w.ai")
        .filter_map(|r| r.arg("pump_us"))
        .map(|v| u64::try_from(v).unwrap_or(0))
        .sum();
    s.ai_slices = t
        .all("w.ai")
        .filter_map(|r| r.arg("slices"))
        .map(|v| u32::try_from(v).unwrap_or(0))
        .sum();
    let order: Vec<(u64, u64)> = t
        .all("w.apply")
        .chain(t.all("w.check_batch"))
        .chain(t.all("w.queue"))
        .map(|r| (r.t0_us, r.end_us()))
        .collect();
    s.order_us = union_len(order, last_end, ai_start);
    // `w.pack` and `w.patch` run on the pack thread after the save: the union counts a moment once.
    let post: Vec<(u64, u64)> = t
        .all("w.turn_change")
        .chain(t.all("w.save"))
        .chain(t.all("w.snapshot"))
        .chain(t.all("w.pack_queue"))
        .chain(t.all("w.pack"))
        .chain(t.all("w.patch"))
        .map(|r| (r.t0_us, r.end_us()))
        .collect();
    s.server_post_us = union_len(post, ai_end, end);
    s.other_us = s
        .total_us
        .saturating_sub(s.wait_others_us + s.order_us + s.server_ai_us + s.server_post_us);
    s.snapshot_bytes = t
        .find("w.pack")
        .and_then(|r| r.arg("bytes"))
        .map_or(0, |v| u64::try_from(v).unwrap_or(0));
    s.patch_bytes = t
        .find("w.patch")
        .and_then(|r| r.arg("bytes"))
        .map_or(0, |v| u64::try_from(v).unwrap_or(0));
    s
}

/// The recorder of one process: the spans of the turns being ended, and the ring of the closed ones.
pub struct Recorder {
    clock: Arc<dyn Clock>,
    level: Level,
    src: Src,
    game: u64,
    next_id: u32,
    open: BTreeMap<i32, Vec<Rec>>,
    ring: VecDeque<TurnTrace>,
    keep: usize,
    /// A span being measured by the caller: id -> (turn, index into `open[turn]`).
    started: BTreeMap<u32, (i32, usize)>,
}

/// Handle of a span that [`Recorder::begin`] started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpanId(u32);

impl SpanId {
    /// The span of a recorder that is off.
    pub const NONE: SpanId = SpanId(0);
}

/// Spans one turn may hold; more are dropped (a runaway loop must not grow the memory).
const MAX_RECS_PER_TURN: usize = 512;

impl Recorder {
    pub fn new(clock: Arc<dyn Clock>, level: Level, src: Src, game: u64, keep: usize) -> Self {
        Recorder {
            clock,
            level,
            src,
            game,
            next_id: 1,
            open: BTreeMap::new(),
            ring: VecDeque::new(),
            keep: keep.max(1),
            started: BTreeMap::new(),
        }
    }

    pub fn level(&self) -> Level {
        self.level
    }

    pub fn set_level(&mut self, level: Level) {
        self.level = level;
    }

    /// Now, unix microseconds.
    pub fn now_us(&self) -> u64 {
        self.clock.now_us()
    }

    fn on(&self) -> bool {
        self.level != Level::Off
    }

    fn push(&mut self, turn: i32, rec: Rec) -> Option<usize> {
        let v = self.open.entry(turn).or_default();
        if v.len() >= MAX_RECS_PER_TURN {
            return None;
        }
        v.push(rec);
        Some(v.len() - 1)
    }

    fn take_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        id
    }

    /// Starts a span of the trace of `turn` now.
    pub fn begin(&mut self, turn: i32, span: &str, lane: &str) -> SpanId {
        if !self.on() {
            return SpanId::NONE;
        }
        let id = self.take_id();
        let t0 = self.clock.now_us();
        let rec = Rec {
            kind: Kind::Span,
            src: self.src,
            seat: -1,
            span: span.to_owned(),
            lane: lane.to_owned(),
            id,
            parent: 0,
            t0_us: t0,
            dur_us: 0,
            args: Vec::new(),
        };
        match self.push(turn, rec) {
            Some(i) => {
                self.started.insert(id, (turn, i));
                SpanId(id)
            }
            None => SpanId::NONE,
        }
    }

    /// Ends a span now with arguments.
    pub fn end(&mut self, id: SpanId, args: &[(&str, i64)]) {
        if id == SpanId::NONE {
            return;
        }
        let now = self.clock.now_us();
        let Some((turn, i)) = self.started.remove(&id.0) else {
            return;
        };
        if let Some(r) = self.open.get_mut(&turn).and_then(|v| v.get_mut(i)) {
            r.dur_us = now.saturating_sub(r.t0_us);
            r.args
                .extend(args.iter().map(|(k, v)| ((*k).to_owned(), *v)));
        }
    }

    /// A span measured by the caller: it started at `t0_us` (this clock) and lasted `dur_us`.
    pub fn span(
        &mut self,
        turn: i32,
        span: &str,
        lane: &str,
        t0_us: u64,
        dur_us: u64,
        args: &[(&str, i64)],
    ) {
        if !self.on() {
            return;
        }
        let id = self.take_id();
        let rec = Rec {
            kind: Kind::Span,
            src: self.src,
            seat: -1,
            span: span.to_owned(),
            lane: lane.to_owned(),
            id,
            parent: 0,
            t0_us,
            dur_us,
            args: args.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect(),
        };
        self.push(turn, rec);
    }

    /// An instant now.
    pub fn mark(&mut self, turn: i32, span: &str, lane: &str, args: &[(&str, i64)]) {
        if !self.on() {
            return;
        }
        let now = self.clock.now_us();
        let id = self.take_id();
        let rec = Rec {
            kind: Kind::Mark,
            src: self.src,
            seat: -1,
            span: span.to_owned(),
            lane: lane.to_owned(),
            id,
            parent: 0,
            t0_us: now,
            dur_us: 0,
            args: args.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect(),
        };
        self.push(turn, rec);
    }

    /// A span now with no duration yet known to be needed by the caller is `begin`/`end`; this is the shortcut
    /// for `f`: the span covers the call.
    pub fn timed<T>(
        &mut self,
        turn: i32,
        span: &str,
        lane: &str,
        args: &[(&str, i64)],
        f: impl FnOnce() -> T,
    ) -> T {
        let t0 = self.clock.now_us();
        let v = f();
        let dur = self.clock.now_us().saturating_sub(t0);
        self.span(turn, span, lane, t0, dur, args);
        v
    }

    /// Freezes the trace of `turn`: it leaves the open set and goes into the ring (and is returned). `None` when the
    /// turn recorded nothing.
    pub fn close_turn(&mut self, turn: i32) -> Option<TurnTrace> {
        let mut recs = self.open.remove(&turn)?;
        self.started.retain(|_, (t, _)| *t != turn);
        recs.sort_by_key(|r| (r.t0_us, r.id));
        let tr = TurnTrace {
            game: self.game,
            turn,
            recs,
        };
        self.ring.push_back(tr.clone());
        while self.ring.len() > self.keep {
            self.ring.pop_front();
        }
        Some(tr)
    }

    /// The closed traces, oldest first.
    pub fn closed(&self) -> impl Iterator<Item = &TurnTrace> {
        self.ring.iter()
    }

    /// Turns that have open records.
    pub fn open_turns(&self) -> Vec<i32> {
        self.open.keys().copied().collect()
    }
}

/// The store of a gateway game: the merged traces of the last turns (the worker's, the gateway's own).
#[derive(Debug, Default)]
pub struct TraceStore {
    traces: VecDeque<TurnTrace>,
    keep: usize,
}

impl TraceStore {
    pub fn new(keep: usize) -> Self {
        TraceStore {
            traces: VecDeque::new(),
            keep: keep.max(1),
        }
    }

    /// Adds the records of a trace to the one of the same turn (or starts it).
    pub fn add(&mut self, t: TurnTrace) {
        match self.traces.iter_mut().find(|x| x.turn == t.turn) {
            Some(x) => x.merge(t),
            None => {
                let at = self
                    .traces
                    .iter()
                    .position(|x| x.turn > t.turn)
                    .unwrap_or(self.traces.len());
                self.traces.insert(at, t);
                while self.traces.len() > self.keep {
                    self.traces.pop_front();
                }
            }
        }
    }

    /// The traces of the turns `lo..=hi`.
    pub fn range(&self, lo: i32, hi: i32) -> Vec<&TurnTrace> {
        self.traces
            .iter()
            .filter(|t| (lo..=hi).contains(&t.turn))
            .collect()
    }

    pub fn get(&self, turn: i32) -> Option<&TurnTrace> {
        self.traces.iter().find(|t| t.turn == turn)
    }

    pub fn turns(&self) -> Vec<i32> {
        self.traces.iter().map(|t| t.turn).collect()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    struct Manual(AtomicU64);

    impl Manual {
        fn new() -> Arc<Manual> {
            Arc::new(Manual(AtomicU64::new(1_000_000)))
        }
        fn step(&self, us: u64) {
            self.0.fetch_add(us, Ordering::SeqCst);
        }
    }

    impl Clock for Manual {
        fn now_us(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    fn rec(c: &Arc<Manual>, level: Level) -> Recorder {
        Recorder::new(c.clone(), level, Src::Worker, 936, 3)
    }

    /// A turn's worth of spans in the order the worker makes them.
    fn one_turn(c: &Arc<Manual>, r: &mut Recorder, turn: i32) {
        r.mark(turn, "w.end_turn", "core", &[("player", 0), ("cmd", 41)]);
        c.step(1000);
        let q = r.begin(turn, "w.apply", "core");
        c.step(2000);
        r.end(q, &[("cmd", 41)]);
        let b = r.begin(turn, "w.check_batch", "core");
        c.step(5000);
        r.end(b, &[("jseq", 7)]);
        let t0 = r.now_us();
        c.step(3_000_000);
        r.span(
            turn,
            "w.ai",
            "core",
            t0,
            3_000_000,
            &[("pump_us", 2_990_000), ("slices", 80)],
        );
        let tc = r.begin(turn, "w.turn_change", "core");
        c.step(300_000);
        r.end(tc, &[]);
        let p0 = r.now_us();
        c.step(200_000);
        r.span(turn, "w.pack", "pack", p0, 80_000, &[("bytes", 534_000)]);
        r.span(
            turn,
            "w.patch",
            "pack",
            p0 + 80_000,
            120_000,
            &[("bytes", 213_000)],
        );
    }

    #[test]
    fn spans_are_recorded_in_order_with_their_args() {
        let c = Manual::new();
        let mut r = rec(&c, Level::Coarse);
        one_turn(&c, &mut r, 126);
        let t = r.close_turn(126).expect("a trace");
        let names: Vec<&str> = t.recs.iter().map(|x| x.span.as_str()).collect();
        assert_eq!(
            names,
            [
                "w.end_turn",
                "w.apply",
                "w.check_batch",
                "w.ai",
                "w.turn_change",
                "w.pack",
                "w.patch"
            ]
        );
        assert_eq!(t.find("w.apply").unwrap().dur_us, 2000);
        assert_eq!(t.find("w.check_batch").unwrap().arg("jseq"), Some(7));
        assert_eq!(t.find("w.end_turn").unwrap().kind, Kind::Mark);
        assert_eq!(t.trace_id(), "936:126");
    }

    #[test]
    fn off_records_nothing() {
        let c = Manual::new();
        let mut r = rec(&c, Level::Off);
        one_turn(&c, &mut r, 5);
        assert!(r.close_turn(5).is_none());
        assert_eq!(r.begin(5, "x", "core"), SpanId::NONE);
    }

    #[test]
    fn the_ring_keeps_the_last_turns() {
        let c = Manual::new();
        let mut r = rec(&c, Level::Coarse);
        for turn in 1..=5 {
            r.mark(turn, "w.end_turn", "core", &[]);
            r.close_turn(turn);
        }
        let turns: Vec<i32> = r.closed().map(|t| t.turn).collect();
        assert_eq!(turns, [3, 4, 5]);
    }

    #[test]
    fn a_turn_holds_a_bounded_number_of_records() {
        let c = Manual::new();
        let mut r = rec(&c, Level::Coarse);
        for _ in 0..(MAX_RECS_PER_TURN + 50) {
            r.mark(1, "m", "core", &[]);
        }
        assert_eq!(r.close_turn(1).unwrap().recs.len(), MAX_RECS_PER_TURN);
    }

    #[test]
    fn the_chrome_export_is_stable() {
        let c = Manual::new();
        let mut r = rec(&c, Level::Coarse);
        r.mark(9, "w.end_turn", "core", &[("cmd", 1)]);
        c.step(500);
        let s = r.begin(9, "w.apply", "core");
        c.step(250);
        r.end(s, &[("cmd", 1)]);
        let t = r.close_turn(9).unwrap();
        let j = chrome(&[&t]);
        let ev = j["traceEvents"].as_array().unwrap();
        // Metadata first (one process, one thread), then the events with ts from the first record.
        assert_eq!(ev[0]["ph"], "M");
        assert_eq!(ev[0]["args"]["name"], "worker");
        assert_eq!(ev[1]["args"]["name"], "core");
        assert_eq!(ev[2]["ph"], "i");
        assert_eq!(ev[2]["ts"], 0);
        assert_eq!(ev[3]["ph"], "X");
        assert_eq!(ev[3]["name"], "w.apply");
        assert_eq!(ev[3]["ts"], 500);
        assert_eq!(ev[3]["dur"], 250);
        assert_eq!(ev[3]["args"]["cmd"], 1);
        assert_eq!(ev[3]["args"]["trace"], "936:9");
    }

    #[test]
    fn the_envelope_line_carries_the_shared_fields() {
        let c = Manual::new();
        let mut r = rec(&c, Level::Coarse);
        let s = r.begin(126, "w.ai", "core");
        c.step(3_496_000);
        r.end(s, &[("slices", 1)]);
        let t = r.close_turn(126).unwrap();
        let l = &t.lines(1_761_909_483_123)[0];
        for k in [
            "ts", "kind", "src", "game", "turn", "seat", "trace", "span", "id", "parent", "t0_us",
            "dur_us", "args",
        ] {
            assert!(l.get(k).is_some(), "field {k}");
        }
        assert_eq!(l["kind"], "span");
        assert_eq!(l["src"], "worker");
        assert_eq!(l["trace"], "936:126");
        assert_eq!(l["dur_us"], 3_496_000);
        assert_eq!(l["args"]["slices"], 1);
    }

    #[test]
    fn the_buckets_plus_other_add_up_to_the_total() {
        let c = Manual::new();
        let mut r = rec(&c, Level::Coarse);
        one_turn(&c, &mut r, 126);
        let t = r.close_turn(126).unwrap();
        let s = summarize(&t);
        assert_eq!(s.server_ai_us, 3_000_000);
        assert_eq!(s.pump_us, 2_990_000);
        assert_eq!(s.ai_slices, 80);
        assert_eq!(s.snapshot_bytes, 534_000);
        assert_eq!(s.patch_bytes, 213_000);
        // The parts of the window after the last END_TURN, plus the wait, plus other, are the total.
        assert_eq!(
            s.wait_others_us + s.order_us + s.server_ai_us + s.server_post_us + s.other_us,
            s.total_us
        );
        assert!(s.total_us >= 3_500_000);
        // The 3 ms before the AI are the orders; the 200 ms of the pack thread after the turn change are the post.
        assert_eq!(s.order_us, 7_000);
        assert_eq!(s.server_post_us, 500_000);
    }

    #[test]
    fn two_humans_have_a_wait_and_the_stages_still_cover_the_window() {
        let c = Manual::new();
        let mut r = rec(&c, Level::Coarse);
        r.mark(7, "w.end_turn", "core", &[("player", 0)]);
        c.step(4_000_000);
        one_turn(&c, &mut r, 7);
        let s = summarize(&r.close_turn(7).unwrap());
        assert_eq!(s.wait_others_us, 4_000_000);
        assert_eq!(
            s.wait_others_us + s.order_us + s.server_ai_us + s.server_post_us + s.other_us,
            s.total_us
        );
    }

    #[test]
    fn the_store_merges_and_keeps_the_last_turns() {
        let mut st = TraceStore::new(2);
        let mk = |turn: i32, name: &str, t0: u64| TurnTrace {
            game: 1,
            turn,
            recs: vec![Rec {
                kind: Kind::Mark,
                src: Src::Gateway,
                seat: -1,
                span: name.into(),
                lane: "io".into(),
                id: 1,
                parent: 0,
                t0_us: t0,
                dur_us: 0,
                args: vec![],
            }],
        };
        st.add(mk(1, "a", 10));
        st.add(mk(1, "b", 20));
        st.add(mk(1, "b", 20));
        assert_eq!(
            st.get(1).unwrap().recs.len(),
            2,
            "a duplicate is not added twice"
        );
        st.add(mk(2, "a", 30));
        st.add(mk(3, "a", 40));
        assert_eq!(st.turns(), [2, 3]);
        assert_eq!(st.range(3, 9).len(), 1);
    }

    #[test]
    fn levels_parse() {
        assert_eq!(Level::parse("Fine"), Some(Level::Fine));
        assert_eq!(Level::parse("off"), Some(Level::Off));
        assert_eq!(Level::parse("loud"), None);
    }
}
