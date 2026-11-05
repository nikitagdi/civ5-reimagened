//! The recorder of slow frames: scoped timers around the blocks of a frame, and for each frame
//! that took longer than [`SLOW_MS`] the scopes that spent its time, kept in a ring of the last
//! [`RING`] ones (`window.__civ5stats.perf.slow_frames`, the console line `[slow-frame] ...`).
//!
//! A scope is a guard: `let _s = slow::scope("city.b2_frame");` times the rest of its block. The
//! scopes nest, so a name's time includes those inside it; a frame's list is the top [`TOP`] names
//! by total time (a name that runs several times in a frame is added). When no frame is open
//! (between [`begin_frame`] and [`end_frame`], or without a clock) a scope costs a flag test.
//!
//! The clock is the app's (`performance.now()` on the web); the app gives it with [`set_clock`].
//! Native builds fall back to `Instant`.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

/// A frame longer than this is kept.
pub const SLOW_MS: f64 = 33.0;
/// Slow frames kept.
pub const RING: usize = 16;
/// Scopes kept for a frame.
pub const TOP: usize = 15;
/// Scopes shorter than this are not recorded.
const MIN_MS: f64 = 0.05;
/// Scopes recorded in one frame at most (the rest are dropped).
const MAX_ENTRIES: usize = 1024;

/// A slow frame and what it spent its time on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SlowFrame {
    /// Counts the slow frames since the start.
    pub seq: u64,
    pub ms: f64,
    pub turn: i32,
    /// `(name, ms)`, largest first.
    pub scopes: Vec<(&'static str, f64)>,
}

impl SlowFrame {
    /// `[slow-frame] 384ms turn 9: city.b2_frame 300, ...`
    pub fn line(&self) -> String {
        let scopes: Vec<String> = self
            .scopes
            .iter()
            .map(|(n, ms)| format!("{n} {ms:.0}"))
            .collect();
        format!(
            "[slow-frame] {:.0}ms turn {}: {}",
            self.ms,
            self.turn,
            scopes.join(", ")
        )
    }
}

/// Frames the trace keeps, slow or not: enough to see a stall that comes every few frames.
pub const TRACE_FRAMES: usize = 48;

/// One frame of the trace (b15-terrain): where the frame waited, kept for every frame and not only for
/// the slow ones, so a period (a long frame every third) shows in the report with its cause next to it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TraceRow {
    /// Counts the frames since the start.
    pub seq: u64,
    /// From the start of the last frame to the start of this one (0 for the first).
    pub period_ms: f64,
    /// The work of the frame, from `begin_frame` to `end_frame`.
    pub work_ms: f64,
    /// `render.acquire`: waiting for a texture of the swapchain (the GPU is behind).
    pub acquire_ms: f64,
    /// `render.submit` and `render.frame_submitted`: the queue and the reads started after it.
    pub submit_ms: f64,
    /// `render.present`.
    pub present_ms: f64,
    /// `terrain.live_frame`: the terrain's CPU share (pages, uploads, the bake).
    pub terrain_ms: f64,
}

type Clock = Rc<dyn Fn() -> f64>;
/// What ran between two frames outside them: `(label, ms)` since the last call, taken by the page.
type Between = Box<dyn Fn() -> Vec<(&'static str, f64)>>;

/// A time between the end of a frame and the start of the next one longer than this is kept as a
/// slow frame whose first scope is `gap` (the wait of the page, not the work of a frame).
pub const GAP_MS: f64 = 33.0;

#[derive(Default)]
struct State {
    buf: Vec<(&'static str, f64)>,
    ring: VecDeque<SlowFrame>,
    seq: u64,
    start: f64,
    /// The start of the frame before (none before the first).
    prev_start: Option<f64>,
    frames: u64,
    trace: VecDeque<TraceRow>,
}

thread_local! {
    static TURN: Cell<i32> = const { Cell::new(0) };
    static CLOCK: RefCell<Option<Clock>> = const { RefCell::new(None) };
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static STATE: RefCell<State> = RefCell::new(State::default());
    /// The clock at the end of the last frame (negative before the first).
    static LAST_END: Cell<f64> = const { Cell::new(-1.0) };
    static BETWEEN: RefCell<Option<Between>> = const { RefCell::new(None) };
}

/// Gives the source of what the page ran between frames (the handlers of the network, the timers):
/// the gap rows name it.
pub fn set_between_source(source: Between) {
    BETWEEN.with(|b| *b.borrow_mut() = Some(source));
}

/// The row of a gap: `gap` the time between the frames, the labelled times the page reported for it,
/// and `gap.unlabeled` what no label covers (garbage collection, a long task of another script, the
/// browser itself).
fn gap_row(gap: f64, mut between: Vec<(&'static str, f64)>, turn: i32) -> SlowFrame {
    let labelled: f64 = between.iter().map(|(_, ms)| ms).sum();
    between.push(("gap.unlabeled", (gap - labelled).max(0.0)));
    between.sort_by(|a, b| b.1.total_cmp(&a.1));
    between.insert(0, ("gap", gap));
    between.truncate(TOP);
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        s.seq += 1;
        let frame = SlowFrame {
            seq: s.seq,
            ms: gap,
            turn,
            scopes: between,
        };
        if s.ring.len() == RING {
            s.ring.pop_front();
        }
        s.ring.push_back(frame.clone());
        frame
    })
}

/// The turn shown, for the line of a slow frame.
pub fn set_turn(turn: i32) {
    TURN.with(|t| t.set(turn));
}

/// Gives the clock (milliseconds, monotonic) the scopes read.
pub fn set_clock(clock: Clock) {
    CLOCK.with(|c| *c.borrow_mut() = Some(clock));
}

fn now() -> f64 {
    CLOCK.with(|c| c.borrow().as_ref().map_or(0.0, |f| f()))
}

/// The recorder's clock in milliseconds (0 without one), for a block that records the longest of its
/// parts rather than their sum.
pub fn now_ms() -> f64 {
    now()
}

#[cfg(not(target_arch = "wasm32"))]
fn default_clock() -> Clock {
    let start = std::time::Instant::now();
    Rc::new(move || start.elapsed().as_secs_f64() * 1000.0)
}

/// Opens the frame. Call it first thing in the frame.
pub fn begin_frame() {
    #[cfg(not(target_arch = "wasm32"))]
    CLOCK.with(|c| {
        let mut c = c.borrow_mut();
        if c.is_none() {
            *c = Some(default_clock());
        }
    });
    let has_clock = CLOCK.with(|c| c.borrow().is_some());
    ACTIVE.with(|a| a.set(has_clock));
    if has_clock {
        let t = now();
        // What the page ran since the last frame is taken every frame, so it never goes stale.
        let between = BETWEEN.with(|b| b.borrow().as_ref().map(|f| f()).unwrap_or_default());
        let last = LAST_END.with(Cell::get);
        if last >= 0.0 && t - last > GAP_MS {
            let row = gap_row(t - last, between, TURN.with(Cell::get));
            tracing::warn!("[slow-gap] {}", row.line());
        }
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            s.buf.clear();
            s.prev_start = (s.frames > 0).then_some(s.start);
            s.start = t;
        });
    }
}

/// Closes the frame; the frame is kept (and returned) if it took longer than [`SLOW_MS`], with the
/// turn of [`set_turn`].
pub fn end_frame() -> Option<SlowFrame> {
    let turn = TURN.with(Cell::get);
    if !ACTIVE.with(|a| a.replace(false)) {
        return None;
    }
    let t = now();
    LAST_END.with(|l| l.set(t));
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let ms = t - s.start;
        // The trace row of every frame: the waits of the render loop, summed from the scopes.
        let part = |names: &[&str]| -> f64 {
            s.buf
                .iter()
                .filter(|(n, _)| names.contains(n))
                .map(|(_, v)| v)
                .sum()
        };
        let (acquire_ms, submit_ms, present_ms, terrain_ms) = (
            part(&["render.acquire"]),
            part(&["render.submit", "render.frame_submitted"]),
            part(&["render.present"]),
            part(&["terrain.live_frame"]),
        );
        s.frames += 1;
        let row = TraceRow {
            seq: s.frames,
            period_ms: s.prev_start.map_or(0.0, |p| s.start - p),
            work_ms: ms,
            acquire_ms,
            submit_ms,
            present_ms,
            terrain_ms,
        };
        if s.trace.len() == TRACE_FRAMES {
            s.trace.pop_front();
        }
        s.trace.push_back(row);
        if ms <= SLOW_MS {
            s.buf.clear();
            return None;
        }
        let mut sums: Vec<(&'static str, f64)> = Vec::new();
        let buf = std::mem::take(&mut s.buf);
        for (name, v) in &buf {
            match sums.iter_mut().find(|(n, _)| n == name) {
                Some(e) => e.1 += v,
                None => sums.push((name, *v)),
            }
        }
        s.buf = buf;
        s.buf.clear();
        sums.sort_by(|a, b| b.1.total_cmp(&a.1));
        sums.truncate(TOP);
        s.seq += 1;
        let frame = SlowFrame {
            seq: s.seq,
            ms,
            turn,
            scopes: sums,
        };
        if s.ring.len() == RING {
            s.ring.pop_front();
        }
        s.ring.push_back(frame.clone());
        Some(frame)
    })
}

/// Adds a measured block to the open frame (for a time taken some other way).
pub fn record(name: &'static str, ms: f64) {
    if ms >= MIN_MS && ACTIVE.with(Cell::get) {
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            if s.buf.len() < MAX_ENTRIES {
                s.buf.push((name, ms));
            }
        });
    }
}

/// The last [`TRACE_FRAMES`] frames, slow or not, oldest first.
pub fn trace() -> Vec<TraceRow> {
    STATE.with(|s| s.borrow().trace.iter().copied().collect())
}

/// The slow frames kept, oldest first.
pub fn ring() -> Vec<SlowFrame> {
    STATE.with(|s| s.borrow().ring.iter().cloned().collect())
}

/// Times the block it is held in.
#[must_use = "the scope times the block it is held in"]
pub struct Scope {
    name: &'static str,
    start: f64,
    on: bool,
}

/// Starts a scope; see the module text.
pub fn scope(name: &'static str) -> Scope {
    let on = ACTIVE.with(Cell::get);
    Scope {
        name,
        start: if on { now() } else { 0.0 },
        on,
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        if self.on {
            record(self.name, now() - self.start);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slow_frame_keeps_the_top_scopes_and_a_fast_one_nothing() {
        let t = Rc::new(Cell::new(0.0));
        let clock = t.clone();
        set_clock(Rc::new(move || clock.get()));
        begin_frame();
        {
            let _s = scope("a");
            t.set(t.get() + 5.0);
        }
        set_turn(1);
        assert!(end_frame().is_none(), "a fast frame is not kept");
        begin_frame();
        for _ in 0..2 {
            let _s = scope("big");
            t.set(t.get() + 30.0);
        }
        {
            let _s = scope("small");
            t.set(t.get() + 2.0);
        }
        set_turn(9);
        let f = end_frame().expect("slow");
        assert_eq!(f.turn, 9);
        assert_eq!(f.scopes[0].0, "big");
        assert!((f.scopes[0].1 - 60.0).abs() < 1e-9, "added: {:?}", f.scopes);
        assert!(f.line().starts_with("[slow-frame] 62ms turn 9: big 60"));
        assert_eq!(ring().last(), Some(&f));
        // Outside a frame a scope records nothing.
        {
            let _s = scope("outside");
            t.set(t.get() + 50.0);
        }
        begin_frame();
        t.set(t.get() + 40.0);
        let g = end_frame().unwrap();
        assert!(g.scopes.is_empty());
    }

    #[test]
    fn every_frame_is_traced_with_its_period_and_the_waits_of_the_render_loop() {
        let t = Rc::new(Cell::new(1000.0));
        let clock = t.clone();
        set_clock(Rc::new(move || clock.get()));
        // Frames of 20, 20 and 50 ms: the third one waits 40 ms at the swapchain.
        for work in [20.0, 20.0, 50.0] {
            begin_frame();
            {
                let _s = scope("render.acquire");
                t.set(t.get() + if work > 30.0 { 40.0 } else { 2.0 });
            }
            t.set(t.get() + work - if work > 30.0 { 40.0 } else { 2.0 });
            let _ = end_frame();
        }
        let rows = trace();
        let last3 = &rows[rows.len() - 3..];
        assert!((last3[1].work_ms - 20.0).abs() < 1e-9, "{last3:?}");
        assert!((last3[2].work_ms - 50.0).abs() < 1e-9);
        assert!((last3[2].acquire_ms - 40.0).abs() < 1e-9);
        assert!((last3[1].acquire_ms - 2.0).abs() < 1e-9);
        assert_eq!(last3[2].seq, last3[0].seq + 2);
        // Back to back, the period of a frame is the work of the one before.
        assert!(
            (last3[2].period_ms - last3[1].work_ms).abs() < 1e-9,
            "{last3:?}"
        );
        assert!(rows.len() <= TRACE_FRAMES);
    }
}
