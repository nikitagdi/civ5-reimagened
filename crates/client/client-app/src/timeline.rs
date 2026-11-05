//! The client's per-turn timeline (packet 2): the spans from the player's END TURN click to
//! "the turn is playable", on the client's clock, in the shared span envelope.
//!
//! A *trace* is the end of one turn, keyed by the turn being ended (`trace` = `"<game>:<turn>"`). Its spans are
//! recorded as the stages happen (`c.click`, `c.turn_named`, `c.fetch`, `c.unpack`, `c.load`, `c.lua_reset`,
//! `c.turn_events`, `c.first_frame`, `c.render_settle`, `turn.playable`; `c.check` and `c.processing` ride along).
//! Each span is also written at once as one JSON line of the envelope ([`Timeline::take_lines`]), so the log ring
//! (`apps/web/www/clientlog.js`) holds it even when the page dies in the middle of the turn. A closed trace gives a row
//! of buckets ([`Buckets`]) that add up to the total, the gap kept as `other`; the last [`ROWS`] rows are published as
//! `__civ5stats.turn`.
//!
//! The clock is the app's (ms, `performance.now()` on the web). `wall_offset_ms` (UTC unix ms minus clock ms, read again
//! every frame so a stalled clock does not drift the calendar) turns it into the envelope's `t0_us`: UTC unix
//! microseconds, the axis of the server's spans.
//!
//! SIMPLIFIED(SIMP-uic-51): the timestamps are the page's UTC clock, not corrected to the server's (the gateway clock
//! of `Pong` is monotonic, not a calendar; the skew between the machines, normally the NTP error, stays): `t0_us` is
//! UTC unix time and the line says `"synced":false`. The buckets
//! only use differences within the client, so they do not need it. The `server` bucket is the whole wait from the click
//! to the `GcTurn` (other players, the AI, the server's work and the net): the client cannot split it.

use std::collections::VecDeque;

/// How much is recorded (`?trace=off|coarse|fine`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Level {
    Off,
    #[default]
    Coarse,
    /// Coarse, and the per-handler spans of the Lua events.
    ///
    /// SIMPLIFIED(SIMP-uic-54): records the same as coarse; the per-handler spans are not made yet.
    Fine,
}

impl Level {
    pub fn parse(s: &str) -> Option<Level> {
        match s.trim().to_ascii_lowercase().as_str() {
            "off" | "0" => Some(Level::Off),
            "coarse" | "1" | "on" => Some(Level::Coarse),
            "fine" | "2" => Some(Level::Fine),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Level::Off => "off",
            Level::Coarse => "coarse",
            Level::Fine => "fine",
        }
    }
}

/// Rows (closed turns) kept for `__civ5stats.turn`.
pub const ROWS: usize = 10;
/// Spans of one trace kept (a runaway trace cannot grow for ever).
const SPANS_MAX: usize = 96;
/// A trace with no new span for this long is closed without a `turn.playable`, ms.
pub const IDLE_CLOSE_MS: f64 = 60_000.0;
/// The render settle gives up after this long, ms.
pub const SETTLE_MAX_MS: f64 = 10_000.0;
/// A frame under this is a quiet one for the settle, ms; this many in a row end it.
pub const QUIET_FRAME_MS: f64 = 33.0;
pub const QUIET_FRAMES: u32 = 3;

/// One recorded span (`dur_ms` negative: a mark).
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub name: &'static str,
    pub id: u32,
    pub t0_ms: f64,
    pub dur_ms: f64,
    pub args: Vec<(&'static str, f64)>,
}

impl Span {
    pub fn is_mark(&self) -> bool {
        self.dur_ms < 0.0
    }

    fn end_ms(&self) -> f64 {
        self.t0_ms + self.dur_ms.max(0.0)
    }
}

/// The time of a turn's end split into stages, ms. `other` is what no stage covers (it may be negative when stages
/// overlap; it is shown, never hidden).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Buckets {
    pub total: f64,
    /// Click (or the first sign of the new turn) to the `GcTurn`: other players, the AI, the server, the net.
    pub server: f64,
    /// `GcTurn` to the body in: the frame waits and the fetch.
    pub net: f64,
    /// Unpack and load into the replica core.
    pub load: f64,
    /// Lua/EUI reset and the turn's events.
    pub ui: f64,
    /// The load done to the render settled.
    pub render: f64,
    pub other: f64,
}

/// A closed trace as a row for `__civ5stats.turn`.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// The turn that ended.
    pub turn: i32,
    /// The player clicked END TURN in this trace (else another player's click or the timer ended it).
    pub clicked: bool,
    /// `turn.playable` was reached (else the trace timed out).
    pub playable: bool,
    pub patch: bool,
    pub body_bytes: f64,
    pub buckets: Buckets,
    pub spans: usize,
}

/// The spans of one ended turn.
#[derive(Debug, Clone)]
pub struct Trace {
    pub turn: i32,
    pub spans: Vec<Span>,
    next_id: u32,
    last_ms: f64,
}

impl Trace {
    fn new(turn: i32, now_ms: f64) -> Trace {
        Trace {
            turn,
            spans: Vec::new(),
            next_id: 1,
            last_ms: now_ms,
        }
    }

    fn find(&self, name: &str) -> Option<&Span> {
        self.spans.iter().find(|s| s.name == name)
    }

    fn last(&self, name: &str) -> Option<&Span> {
        self.spans.iter().rev().find(|s| s.name == name)
    }

    fn arg(&self, name: &str, key: &str) -> Option<f64> {
        self.find(name)
            .and_then(|s| s.args.iter().find(|a| a.0 == key).map(|a| a.1))
    }

    /// The buckets of the trace; `None` when it has neither a click nor a `GcTurn` (nothing to measure from).
    pub fn buckets(&self) -> Option<Buckets> {
        let click = self.last("c.click").map(|s| s.t0_ms);
        let named = self.find("c.turn_named").map(|s| s.t0_ms);
        let start = click.or(named)?;
        let end = self
            .find("turn.playable")
            .map(|s| s.t0_ms)
            .unwrap_or_else(|| self.spans.iter().map(Span::end_ms).fold(start, f64::max));
        let total = (end - start).max(0.0);
        let server = match (click, named) {
            (Some(c), Some(n)) => (n - c).max(0.0),
            _ => 0.0,
        };
        let fetch_end = self.find("c.fetch").map(Span::end_ms);
        let net = match (named, fetch_end) {
            (Some(n), Some(f)) => (f - n).max(0.0),
            _ => 0.0,
        };
        let sum = |names: &[&str]| -> f64 {
            self.spans
                .iter()
                .filter(|s| names.contains(&s.name) && !s.is_mark())
                .map(|s| s.dur_ms)
                .sum()
        };
        let load = sum(&["c.unpack", "c.load"]);
        let ui = sum(&["c.lua_reset", "c.turn_events"]);
        let render = sum(&["c.render_settle"]);
        let other = total - (server + net + load + ui + render);
        Some(Buckets {
            total,
            server,
            net,
            load,
            ui,
            render,
            other,
        })
    }

    fn row(&self, playable: bool) -> Option<Row> {
        let buckets = self.buckets()?;
        Some(Row {
            turn: self.turn,
            clicked: self.find("c.click").is_some(),
            playable,
            patch: self.arg("c.fetch", "patch").is_some_and(|p| p > 0.5),
            body_bytes: self.arg("c.fetch", "bytes").unwrap_or(0.0),
            buckets,
            spans: self.spans.len(),
        })
    }
}

/// The settle of the render after a turn's load: the view is ready and a few frames in a row were quiet.
#[derive(Debug, Clone, Copy)]
struct Settle {
    turn: i32,
    since_ms: f64,
    quiet: u32,
    first_frame_done: bool,
    pages: f64,
}

/// What the app tells the timeline about each frame (after the render).
#[derive(Debug, Clone, Copy)]
pub struct FrameInfo {
    pub now_ms: f64,
    /// The frame's own length, ms.
    pub frame_ms: f64,
    /// Every cell in view has its page (`terrain.view_ready`).
    pub view_ready: bool,
    /// Cells in view without a page and cells the field has not built: how much is left to settle.
    pub pages_pending: usize,
}

/// The recorder.
#[derive(Debug)]
pub struct Timeline {
    level: Level,
    game: i64,
    seat: i32,
    wall_offset_ms: f64,
    cur: Option<Trace>,
    rows: VecDeque<Row>,
    lines: Vec<String>,
    settle: Option<Settle>,
    /// The turn the replica's last load was for, and when the load ended (ms): the render settle starts there.
    loaded: Option<(i32, f64)>,
    /// The end of turn the seat sent is awaited by every human from this time (ms) in this turn: `c.processing`.
    processing: Option<(f64, i32)>,
    /// The turn whose `c.processing` was closed: not started again before the next turn loads.
    processing_done: Option<i32>,
}

impl Timeline {
    pub fn new(level: Level) -> Timeline {
        Timeline {
            level,
            game: -1,
            seat: -1,
            wall_offset_ms: 0.0,
            cur: None,
            rows: VecDeque::new(),
            lines: Vec::new(),
            settle: None,
            loaded: None,
            processing: None,
            processing_done: None,
        }
    }

    pub fn level(&self) -> Level {
        self.level
    }

    pub fn enabled(&self) -> bool {
        self.level != Level::Off
    }

    /// The game and seat the lines carry.
    pub fn set_context(&mut self, game: i64, seat: i32) {
        self.game = game;
        self.seat = seat;
    }

    /// Unix ms minus the clock's ms: how the clock's times become calendar times.
    pub fn set_wall_offset(&mut self, offset_ms: f64) {
        self.wall_offset_ms = offset_ms;
    }

    /// The open trace's turn, if any.
    pub fn open_turn(&self) -> Option<i32> {
        self.cur.as_ref().map(|t| t.turn)
    }

    /// The closed turns, oldest first.
    pub fn rows(&self) -> impl Iterator<Item = &Row> {
        self.rows.iter()
    }

    /// The envelope lines written since the last call.
    pub fn take_lines(&mut self) -> Vec<String> {
        std::mem::take(&mut self.lines)
    }

    /// The trace of `ended_turn`, opened if need be (a trace of another turn still open is closed first, unfinished).
    fn trace_for(&mut self, ended_turn: i32, now_ms: f64) -> &mut Trace {
        if self.cur.as_ref().is_some_and(|t| t.turn != ended_turn) {
            self.close(false, now_ms);
        }
        self.cur
            .get_or_insert_with(|| Trace::new(ended_turn, now_ms))
    }

    /// Records a span of the end of `ended_turn` (`dur_ms` negative: a mark). Off: nothing.
    pub fn record(
        &mut self,
        ended_turn: i32,
        name: &'static str,
        t0_ms: f64,
        dur_ms: f64,
        args: Vec<(&'static str, f64)>,
    ) {
        if !self.enabled() {
            return;
        }
        let end = t0_ms + dur_ms.max(0.0);
        let (game, seat, off) = (self.game, self.seat, self.wall_offset_ms);
        let t = self.trace_for(ended_turn, end);
        if t.spans.len() >= SPANS_MAX {
            return;
        }
        let id = t.next_id;
        t.next_id += 1;
        t.last_ms = t.last_ms.max(end);
        let span = Span {
            name,
            id,
            t0_ms,
            dur_ms,
            args,
        };
        let line = envelope(game, seat, off, ended_turn, &span);
        t.spans.push(span);
        self.lines.push(line);
    }

    /// A turn change was named by the server (`GcTurn` for `new_turn`): it ended `new_turn - 1`.
    pub fn ended_by(new_turn: i32) -> i32 {
        new_turn - 1
    }

    /// The replica's load of `new_turn` ended at `end_ms`: the render settle is awaited from here.
    pub fn loaded(&mut self, new_turn: i32, end_ms: f64) {
        if !self.enabled() {
            return;
        }
        self.loaded = Some((new_turn, end_ms));
        self.settle = None;
    }

    /// Records a UI-side span (`c.lua_reset`, `c.turn_events`) of the load `loaded` named, if its trace is still open
    /// and waiting for it (a later resync load is not a part of the turn's end). `true` when it was recorded.
    pub fn record_after_load(
        &mut self,
        name: &'static str,
        t0_ms: f64,
        dur_ms: f64,
        args: Vec<(&'static str, f64)>,
    ) -> bool {
        let Some((turn, _)) = self.loaded else {
            return false;
        };
        let waiting = self.cur.as_ref().is_some_and(|t| {
            t.turn + 1 == turn && t.find("c.render_settle").is_none() && t.find(name).is_none()
        });
        if waiting {
            self.record(turn - 1, name, t0_ms, dur_ms, args);
        }
        waiting
    }

    /// The spans the replica and the exe-api collected since the last frame, put on the timeline in time order.
    /// `turn_now`: the replica's turn; `all_complete`: every human has ended the turn (the wait for the server).
    pub fn ingest(
        &mut self,
        replica: Vec<replica::ReplicaSpan>,
        exe: Vec<exe_api::ExeSpan>,
        turn_now: i32,
        all_complete: bool,
        now_ms: f64,
    ) {
        enum Item {
            Rep(replica::ReplicaSpan),
            Exe(exe_api::ExeSpan),
        }
        if !self.enabled() {
            return;
        }
        match (all_complete, self.processing) {
            (true, None) if self.processing_done != Some(turn_now) => {
                self.processing = Some((now_ms, turn_now));
            }
            (false, Some(_)) => self.processing = None,
            _ => {}
        }
        let mut items: Vec<(f64, Item)> = replica
            .into_iter()
            .map(|s| (s.t0_ms, Item::Rep(s)))
            .chain(exe.into_iter().map(|s| (s.t0_ms, Item::Exe(s))))
            .collect();
        items.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, item) in items {
            match item {
                Item::Rep(s) if s.turn_change => {
                    let ended = Timeline::ended_by(s.turn);
                    if s.name == "c.turn_named"
                        && let Some((since, turn)) = self.processing.take()
                    {
                        self.processing_done = Some(turn);
                        self.record(
                            ended,
                            "c.processing",
                            since,
                            (s.t0_ms - since).max(0.0),
                            Vec::new(),
                        );
                    }
                    self.record(ended, s.name, s.t0_ms, s.dur_ms, s.args.clone());
                    if s.name == "c.load" {
                        self.loaded(s.turn, s.t0_ms + s.dur_ms);
                    }
                }
                // A check outside a turn change belongs to the end of the turn that is open.
                Item::Rep(s) if s.name == "c.check" => {
                    if let Some(turn) = self.open_turn() {
                        self.record(turn, s.name, s.t0_ms, s.dur_ms, s.args.clone());
                    }
                }
                Item::Rep(_) => {}
                Item::Exe(s) => match s.name {
                    "c.click" => self.record(turn_now, s.name, s.t0_ms, s.dur_ms, s.args.clone()),
                    "c.lua_reset" => {
                        self.record_after_load(s.name, s.t0_ms, s.dur_ms, s.args.clone());
                    }
                    "c.turn_events"
                        if self.record_after_load(s.name, s.t0_ms, s.dur_ms, s.args.clone()) =>
                    {
                        self.events_done(s.t0_ms + s.dur_ms);
                    }
                    _ => {}
                },
            }
        }
        self.tick(now_ms);
    }

    /// The Lua side finished its turn events for the load (`c.turn_events` ended at `end_ms`): the settle starts.
    pub fn events_done(&mut self, end_ms: f64) {
        if let Some((turn, _)) = self.loaded
            && self.settle.is_none()
            && self.cur.as_ref().is_some_and(|t| t.turn + 1 == turn)
        {
            self.settle = Some(Settle {
                turn,
                since_ms: end_ms,
                quiet: 0,
                first_frame_done: false,
                pages: 0.0,
            });
        }
    }

    /// Whether the render settle is being awaited.
    pub fn settling(&self) -> bool {
        self.settle.is_some()
    }

    /// One frame after the load: the first one is `c.first_frame`; the settle ends on quiet frames with the view ready.
    /// `top`: the largest scopes of the frame (for the first frame's line), names only.
    pub fn frame(&mut self, f: FrameInfo, top: Option<&str>) {
        let Some(mut s) = self.settle else { return };
        let ended = Timeline::ended_by(s.turn);
        if !s.first_frame_done {
            s.first_frame_done = true;
            let mut args = vec![("ms", f.frame_ms)];
            // The names of the biggest scopes ride in the line as a string (see `envelope`); here, their count.
            if let Some(t) = top {
                args.push(("top_n", t.split(',').count() as f64));
            }
            self.record_with_text(
                ended,
                "c.first_frame",
                f.now_ms - f.frame_ms,
                f.frame_ms,
                args,
                top.map(|t| ("top", t.to_owned())),
            );
        }
        if f.frame_ms < QUIET_FRAME_MS && f.view_ready {
            s.quiet += 1;
        } else {
            s.quiet = 0;
        }
        s.pages = s.pages.max(f.pages_pending as f64);
        let timed_out = f.now_ms - s.since_ms >= SETTLE_MAX_MS;
        if s.quiet >= QUIET_FRAMES || timed_out {
            let start = s.since_ms;
            self.record(
                ended,
                "c.render_settle",
                start,
                (f.now_ms - start).max(0.0),
                vec![
                    ("pages", s.pages),
                    (
                        "timeout",
                        f64::from(u8::from(timed_out && s.quiet < QUIET_FRAMES)),
                    ),
                ],
            );
            self.settle = None;
            self.record(ended, "turn.playable", f.now_ms, -1.0, Vec::new());
            self.close(true, f.now_ms);
        } else {
            self.settle = Some(s);
        }
    }

    /// A line for a span with one text argument (`top`).
    fn record_with_text(
        &mut self,
        ended_turn: i32,
        name: &'static str,
        t0_ms: f64,
        dur_ms: f64,
        args: Vec<(&'static str, f64)>,
        text: Option<(&'static str, String)>,
    ) {
        let before = self.lines.len();
        self.record(ended_turn, name, t0_ms, dur_ms, args);
        if let (Some((k, v)), true) = (text, self.lines.len() > before)
            && let Some(last) = self.lines.last_mut()
        {
            // The text goes into the line's `args` object, as a string.
            if let Some(pos) = last.rfind("\"args\":{") {
                let at = pos + "\"args\":{".len();
                let sep = if last[at..].starts_with('}') { "" } else { "," };
                last.insert_str(at, &format!("{}:{}{sep}", json_str(k), json_str(&v)));
            }
        }
    }

    /// Closes the open trace: its row goes to the list and a `turn_summary` line is written. `playable`: the turn is
    /// playable (else it timed out).
    pub fn close(&mut self, playable: bool, now_ms: f64) {
        let Some(t) = self.cur.take() else { return };
        self.settle = None;
        self.loaded = None;
        if let Some(row) = t.row(playable) {
            self.lines.push(summary_line(
                self.game,
                self.seat,
                self.wall_offset_ms,
                now_ms,
                &row,
            ));
            if self.rows.len() >= ROWS {
                self.rows.pop_front();
            }
            self.rows.push_back(row);
        }
    }

    /// Closes a trace nothing has touched for [`IDLE_CLOSE_MS`].
    pub fn tick(&mut self, now_ms: f64) {
        if self
            .cur
            .as_ref()
            .is_some_and(|t| now_ms - t.last_ms >= IDLE_CLOSE_MS)
        {
            self.close(false, now_ms);
        }
    }

    /// `__civ5stats.turn`: `[(turn, [(field, value)])]`, oldest first, in the order of the fields below.
    pub fn stat_rows(&self) -> Vec<(i32, Vec<(&'static str, f64)>)> {
        self.rows
            .iter()
            .map(|r| {
                let b = &r.buckets;
                (
                    r.turn,
                    vec![
                        ("clicked", f64::from(u8::from(r.clicked))),
                        ("playable", f64::from(u8::from(r.playable))),
                        ("patch", f64::from(u8::from(r.patch))),
                        ("body_bytes", r.body_bytes),
                        ("spans", r.spans as f64),
                        ("total_ms", b.total),
                        ("server_ms", b.server),
                        ("net_ms", b.net),
                        ("load_ms", b.load),
                        ("ui_ms", b.ui),
                        ("render_ms", b.render),
                        ("other_ms", b.other),
                    ],
                )
            })
            .collect()
    }
}

fn micros(wall_offset_ms: f64, t_ms: f64) -> i64 {
    ((wall_offset_ms + t_ms) * 1000.0).round() as i64
}

/// A JSON string literal.
pub fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn num(v: f64) -> String {
    if v.is_finite() {
        // Whole numbers without a fraction; the rest to the microsecond.
        if v.fract() == 0.0 && v.abs() < 1e15 {
            format!("{}", v as i64)
        } else {
            format!("{:.3}", v)
        }
    } else {
        "null".into()
    }
}

/// One span as a line of the shared envelope.
fn envelope(game: i64, seat: i32, wall_offset_ms: f64, ended_turn: i32, s: &Span) -> String {
    let kind = if s.is_mark() { "mark" } else { "span" };
    let args: Vec<String> = s
        .args
        .iter()
        .map(|(k, v)| format!("{}:{}", json_str(k), num(*v)))
        .collect();
    let dur = if s.is_mark() {
        String::new()
    } else {
        format!(",\"dur_us\":{}", (s.dur_ms * 1000.0).round() as i64)
    };
    format!(
        "{{\"ts\":{},\"kind\":\"{kind}\",\"src\":\"client\",\"game\":{game},\"turn\":{ended_turn},\"seat\":{seat},\
         \"trace\":\"{game}:{ended_turn}\",\"span\":{},\"id\":{},\"parent\":0,\"t0_us\":{}{dur},\"synced\":false,\"args\":{{{}}}}}",
        (wall_offset_ms + s.t0_ms + s.dur_ms.max(0.0)).round() as i64,
        json_str(s.name),
        s.id,
        micros(wall_offset_ms, s.t0_ms),
        args.join(",")
    )
}

fn summary_line(game: i64, seat: i32, wall_offset_ms: f64, now_ms: f64, r: &Row) -> String {
    let b = &r.buckets;
    format!(
        "{{\"ts\":{},\"kind\":\"turn_summary\",\"src\":\"client\",\"game\":{game},\"turn\":{},\"seat\":{seat},\
         \"trace\":\"{game}:{}\",\"args\":{{\"clicked\":{},\"playable\":{},\"patch\":{},\"body_bytes\":{},\"spans\":{},\
         \"total_ms\":{},\"server_ms\":{},\"net_ms\":{},\"load_ms\":{},\"ui_ms\":{},\"render_ms\":{},\"other_ms\":{}}}}}",
        (wall_offset_ms + now_ms).round() as i64,
        r.turn,
        r.turn,
        u8::from(r.clicked),
        u8::from(r.playable),
        u8::from(r.patch),
        num(r.body_bytes),
        r.spans,
        num(b.total),
        num(b.server),
        num(b.net),
        num(b.load),
        num(b.ui),
        num(b.render),
        num(b.other),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tl() -> Timeline {
        let mut t = Timeline::new(Level::Coarse);
        t.set_context(936, 1);
        t.set_wall_offset(1_761_909_000_000.0);
        t
    }

    /// A whole turn: click at 1000, `GcTurn` at 9000, body in at 9300, unpack 20, load 250, reset 100, events 400,
    /// then frames until the settle.
    fn play_turn(t: &mut Timeline, ended: i32) {
        t.record(ended, "c.click", 1000.0, -1.0, vec![]);
        t.record(ended, "c.order_sent", 1001.0, -1.0, vec![("cmd", 41.0)]);
        t.record(ended, "c.turn_named", 9000.0, -1.0, vec![("patch", 1.0)]);
        t.record(
            ended,
            "c.fetch",
            9005.0,
            295.0,
            vec![("bytes", 213_000.0), ("patch", 1.0)],
        );
        t.record(ended, "c.unpack", 9310.0, 20.0, vec![]);
        t.record(ended, "c.load", 9330.0, 250.0, vec![]);
        t.loaded(ended + 1, 9580.0);
        t.record(ended, "c.lua_reset", 9600.0, 100.0, vec![]);
        t.record(
            ended,
            "c.turn_events",
            9700.0,
            400.0,
            vec![("events", 30.0)],
        );
        t.events_done(10_100.0);
    }

    fn frame(t: &mut Timeline, now: f64, ms: f64, ready: bool) {
        t.frame(
            FrameInfo {
                now_ms: now,
                frame_ms: ms,
                view_ready: ready,
                pages_pending: usize::from(!ready) * 12,
            },
            None,
        );
    }

    #[test]
    fn a_turn_gives_every_stage_and_buckets_that_add_up() {
        let mut t = tl();
        play_turn(&mut t, 126);
        assert!(t.settling());
        frame(&mut t, 10_150.0, 120.0, false);
        frame(&mut t, 10_200.0, 20.0, true);
        frame(&mut t, 10_220.0, 20.0, true);
        assert_eq!(t.rows().count(), 0, "two quiet frames are not enough");
        frame(&mut t, 10_240.0, 20.0, true);
        let rows: Vec<_> = t.rows().cloned().collect();
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert!(r.clicked && r.playable && r.patch);
        assert_eq!(r.turn, 126);
        assert_eq!(r.body_bytes, 213_000.0);
        let b = r.buckets;
        assert_eq!(b.total, 10_240.0 - 1000.0);
        assert_eq!(b.server, 8000.0);
        assert_eq!(b.net, 300.0);
        assert_eq!(b.load, 270.0);
        assert_eq!(b.ui, 500.0);
        assert_eq!(b.render, 10_240.0 - 10_100.0);
        let sum = b.server + b.net + b.load + b.ui + b.render + b.other;
        assert!((sum - b.total).abs() < 1e-9, "{b:?}");
        // The gaps (frame waits between the stages) are `other`, not hidden.
        assert!(b.other > 0.0 && b.other < 500.0, "{b:?}");
    }

    #[test]
    fn a_new_wall_offset_moves_the_stamps_of_the_spans_after_it() {
        let mut t = tl();
        t.record(4, "c.click", 1000.0, -1.0, vec![]);
        // The page clock stalled 2 s (a sleep): the offset to UTC is read again.
        t.set_wall_offset(1_761_909_002_000.0);
        t.record(4, "c.turn_named", 1500.0, -1.0, vec![]);
        let v: Vec<serde_json::Value> = t
            .take_lines()
            .iter()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(v[0]["t0_us"], (1_761_909_000_000_i64 + 1000) * 1000);
        assert_eq!(v[1]["t0_us"], (1_761_909_002_000_i64 + 1500) * 1000);
        // `ts` is UTC unix ms too.
        assert_eq!(v[1]["ts"], 1_761_909_002_000_i64 + 1500);
    }

    #[test]
    fn every_span_is_a_line_of_the_shared_envelope() {
        let mut t = tl();
        play_turn(&mut t, 126);
        let lines = t.take_lines();
        assert_eq!(lines.len(), 8);
        assert!(t.take_lines().is_empty());
        let v: serde_json::Value = serde_json::from_str(&lines[3]).unwrap();
        assert_eq!(v["kind"], "span");
        assert_eq!(v["src"], "client");
        assert_eq!(v["span"], "c.fetch");
        assert_eq!(v["game"], 936);
        assert_eq!(v["turn"], 126);
        assert_eq!(v["seat"], 1);
        assert_eq!(v["trace"], "936:126");
        assert_eq!(v["parent"], 0);
        assert_eq!(v["dur_us"], 295_000);
        assert_eq!(v["t0_us"], 1_761_909_000_000_i64 * 1000 + 9_005_000);
        assert_eq!(v["args"]["bytes"], 213_000);
        assert_eq!(v["synced"], false);
        let m: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(m["kind"], "mark");
        assert!(m.get("dur_us").is_none());
        // Ids are unique inside the trace.
        let ids: Vec<u64> = lines
            .iter()
            .map(|l| {
                serde_json::from_str::<serde_json::Value>(l).unwrap()["id"]
                    .as_u64()
                    .unwrap()
            })
            .collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len());
        // The summary line at the close.
        frame(&mut t, 10_150.0, 10.0, true);
        frame(&mut t, 10_160.0, 10.0, true);
        frame(&mut t, 10_170.0, 10.0, true);
        let tail = t.take_lines();
        let names: Vec<String> = tail
            .iter()
            .map(|l| {
                let v: serde_json::Value = serde_json::from_str(l).unwrap();
                v["span"]
                    .as_str()
                    .or(v["kind"].as_str())
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(
            names,
            [
                "c.first_frame",
                "c.render_settle",
                "turn.playable",
                "turn_summary"
            ]
        );
        let s: serde_json::Value = serde_json::from_str(tail.last().unwrap()).unwrap();
        assert_eq!(s["args"]["playable"], 1);
        assert_eq!(s["args"]["total_ms"], 9170);
    }

    #[test]
    fn a_settle_that_never_settles_times_out() {
        let mut t = tl();
        play_turn(&mut t, 7);
        frame(&mut t, 10_200.0, 90.0, false);
        frame(&mut t, 10_100.0 + SETTLE_MAX_MS, 90.0, false);
        let rows: Vec<_> = t.rows().cloned().collect();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].playable);
        let spans = t.take_lines();
        let settle = spans
            .iter()
            .find(|l| l.contains("c.render_settle"))
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(settle).unwrap();
        assert_eq!(v["args"]["timeout"], 1);
        assert_eq!(v["args"]["pages"], 12);
    }

    #[test]
    fn a_trace_is_for_one_ended_turn_and_the_old_one_closes_unfinished() {
        let mut t = tl();
        t.record(5, "c.click", 100.0, -1.0, vec![]);
        t.record(5, "c.turn_named", 900.0, -1.0, vec![]);
        // The next turn's click: turn 5's trace is closed without `playable`.
        t.record(6, "c.click", 5000.0, -1.0, vec![]);
        let rows: Vec<_> = t.rows().cloned().collect();
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].turn, rows[0].playable), (5, false));
        assert_eq!(rows[0].buckets.server, 800.0);
        assert_eq!(t.open_turn(), Some(6));
        // Idle for a minute: closed by the tick.
        t.tick(5000.0 + IDLE_CLOSE_MS - 1.0);
        assert_eq!(t.open_turn(), Some(6));
        t.tick(5000.0 + IDLE_CLOSE_MS);
        assert_eq!(t.open_turn(), None);
        // A click alone has nothing to measure to: it still gives a row (total 0).
        assert_eq!(t.rows().count(), 2);
    }

    #[test]
    fn a_turn_ended_by_others_is_measured_from_the_gc_turn() {
        let mut t = tl();
        t.record(9, "c.turn_named", 2000.0, -1.0, vec![]);
        t.record(9, "c.fetch", 2010.0, 90.0, vec![("bytes", 5.0)]);
        t.record(9, "c.load", 2100.0, 200.0, vec![]);
        t.close(false, 2400.0);
        let r = t.rows().next().unwrap().clone();
        assert!(!r.clicked);
        assert_eq!(r.buckets.server, 0.0);
        assert_eq!(r.buckets.total, 300.0);
        assert_eq!(r.buckets.net, 100.0);
    }

    #[test]
    fn off_records_nothing_and_rows_are_capped() {
        let mut off = Timeline::new(Level::Off);
        off.record(1, "c.click", 1.0, -1.0, vec![]);
        off.loaded(2, 5.0);
        assert!(off.take_lines().is_empty());
        assert_eq!(off.open_turn(), None);
        let mut t = tl();
        for turn in 0..(ROWS as i32 + 4) {
            t.record(turn, "c.click", 1.0, -1.0, vec![]);
            t.record(turn, "c.turn_named", 11.0, -1.0, vec![]);
            t.close(true, 12.0);
        }
        assert_eq!(t.rows().count(), ROWS);
        assert_eq!(t.rows().next().unwrap().turn, 4);
        let stat = t.stat_rows();
        assert_eq!(stat.len(), ROWS);
        assert!(stat[0].1.iter().any(|f| f.0 == "total_ms" && f.1 == 10.0));
        assert_eq!(Level::parse("FINE"), Some(Level::Fine));
        assert_eq!(Level::parse("off"), Some(Level::Off));
        assert_eq!(Level::parse("x"), None);
    }

    fn rspan(name: &'static str, t0: f64, dur: f64, turn: i32) -> replica::ReplicaSpan {
        replica::ReplicaSpan {
            name,
            t0_ms: t0,
            dur_ms: dur,
            turn,
            turn_change: true,
            args: vec![("bytes", 100.0)],
        }
    }

    fn espan(name: &'static str, t0: f64, dur: f64) -> exe_api::ExeSpan {
        exe_api::ExeSpan {
            name,
            t0_ms: t0,
            dur_ms: dur,
            args: vec![],
        }
    }

    /// The spans as the replica and the exe-api produce them, fed in the frames they come in: every stage of
    /// the catalog is there at the end and the buckets add up.
    #[test]
    fn the_replica_and_exe_spans_make_one_turn_with_every_client_stage() {
        let mut t = tl();
        // The click (turn 40 on the replica), the wait for everyone.
        t.ingest(
            vec![],
            vec![espan("c.click", 1000.0, -1.0)],
            40,
            true,
            1010.0,
        );
        // The GcTurn, the fetch, the load: turn 41 named, in one frame each.
        t.ingest(
            vec![rspan("c.turn_named", 5000.0, -1.0, 41)],
            vec![],
            40,
            true,
            5001.0,
        );
        t.ingest(
            vec![rspan("c.fetch", 5010.0, 150.0, 41)],
            vec![],
            40,
            false,
            5200.0,
        );
        t.ingest(
            vec![
                rspan("c.unpack", 5210.0, 30.0, 41),
                rspan("c.load", 5240.0, 300.0, 41),
            ],
            vec![],
            41,
            false,
            5560.0,
        );
        // The next frame's dispatch: the reset and the events.
        t.ingest(
            vec![],
            vec![
                espan("c.lua_reset", 5600.0, 80.0),
                espan("c.turn_events", 5680.0, 220.0),
            ],
            41,
            false,
            5900.0,
        );
        assert!(t.settling());
        for (i, ms) in [150.0, 30.0, 20.0, 20.0, 20.0].iter().enumerate() {
            frame(&mut t, 6000.0 + 40.0 * i as f64, *ms, i > 0);
        }
        let lines = t.take_lines();
        let names: Vec<String> = lines
            .iter()
            .map(|l| {
                let v: serde_json::Value = serde_json::from_str(l).unwrap();
                v["span"]
                    .as_str()
                    .or(v["kind"].as_str())
                    .unwrap()
                    .to_owned()
            })
            .collect();
        for want in [
            "c.click",
            "c.processing",
            "c.turn_named",
            "c.fetch",
            "c.unpack",
            "c.load",
            "c.lua_reset",
            "c.turn_events",
            "c.first_frame",
            "c.render_settle",
            "turn.playable",
            "turn_summary",
        ] {
            assert!(
                names.iter().any(|n| n == want),
                "{want} missing in {names:?}"
            );
        }
        // All of them are of the ended turn 40.
        assert!(lines.iter().all(|l| l.contains("\"turn\":40")), "{lines:?}");
        let r = t.rows().next().unwrap();
        assert!(r.clicked && r.playable && !r.patch);
        let b = r.buckets;
        assert_eq!(
            (b.server, b.net, b.load, b.ui),
            (4000.0, 160.0, 330.0, 300.0)
        );
        let sum = b.server + b.net + b.load + b.ui + b.render + b.other;
        assert!((sum - b.total).abs() < 1e-9, "{b:?}");
    }

    #[test]
    fn a_later_load_does_not_add_to_a_finished_turn() {
        let mut t = tl();
        t.ingest(vec![], vec![espan("c.click", 100.0, -1.0)], 9, false, 101.0);
        t.ingest(
            vec![
                rspan("c.turn_named", 500.0, -1.0, 10),
                rspan("c.load", 510.0, 20.0, 10),
            ],
            vec![],
            10,
            false,
            540.0,
        );
        t.ingest(
            vec![],
            vec![
                espan("c.lua_reset", 600.0, 10.0),
                espan("c.turn_events", 610.0, 10.0),
            ],
            10,
            false,
            700.0,
        );
        for i in 0..3 {
            frame(&mut t, 800.0 + 10.0 * i as f64, 10.0, true);
        }
        assert_eq!(t.rows().count(), 1);
        // A resync load (not a turn change) and its reset arrive: no trace is opened for them.
        t.ingest(
            vec![],
            vec![
                espan("c.lua_reset", 5000.0, 10.0),
                espan("c.turn_events", 5010.0, 10.0),
            ],
            10,
            false,
            5100.0,
        );
        assert_eq!(t.open_turn(), None);
        assert_eq!(t.rows().count(), 1);
    }

    #[test]
    fn the_first_frame_line_names_its_slowest_scopes() {
        let mut t = tl();
        play_turn(&mut t, 3);
        t.frame(
            FrameInfo {
                now_ms: 10_300.0,
                frame_ms: 200.0,
                view_ready: false,
                pages_pending: 4,
            },
            Some("city.b2_frame,app.ui,m2.dispatch"),
        );
        let line = t
            .take_lines()
            .into_iter()
            .find(|l| l.contains("c.first_frame"))
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["args"]["top"], "city.b2_frame,app.ui,m2.dispatch");
        assert_eq!(v["args"]["ms"], 200);
        assert_eq!(v["args"]["top_n"], 3);
    }
}
