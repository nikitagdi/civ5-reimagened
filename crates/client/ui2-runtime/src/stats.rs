//! Work counters of a frame, for the host's `ui2.*` counters and the tests of the zero-work rule.

/// Work counters of one frame. A frame without changes has every counter at zero except `nodes`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrameStats {
    /// Time of the whole frame in ms (0 without a clock, see `Runtime::set_clock`).
    pub cpu_ms: f32,
    /// Time of style selection in ms.
    pub style_ms: f32,
    /// Time of layout in ms.
    pub layout_ms: f32,
    /// Time of shaping and inline layout in ms.
    pub text_ms: f32,
    /// Time of binding evaluation in ms.
    pub bind_ms: f32,
    /// Time of building draw lists (and handing them to the scene) in ms.
    pub paint_ms: f32,
    /// Nodes whose style record was selected again.
    pub restyled: u32,
    /// Layout runs (one per root laid out).
    pub layout_runs: u32,
    /// Text runs shaped (runs kept from the previous shaping are not counted).
    pub shaped_runs: u32,
    /// Bindings evaluated.
    pub binds_evaluated: u32,
    /// Chunks whose draw list was rebuilt and set on the scene.
    pub chunks_rebuilt: u32,
    /// Live nodes.
    pub nodes: u32,
    /// Nodes and binding rows any pass looked at (zero on an idle frame).
    pub visited: u32,
}

/// Measures the passes of a frame with an optional clock.
pub(crate) struct Stopwatch<'a> {
    clock: Option<&'a dyn Fn() -> f64>,
    start: f64,
    last: f64,
}

impl<'a> Stopwatch<'a> {
    pub fn new(clock: Option<&'a dyn Fn() -> f64>) -> Self {
        let now = clock.map_or(0.0, |c| c());
        Self {
            clock,
            start: now,
            last: now,
        }
    }

    /// Milliseconds since the previous lap (or the start).
    pub fn lap(&mut self) -> f32 {
        let Some(c) = self.clock else {
            return 0.0;
        };
        let now = c();
        let d = now - self.last;
        self.last = now;
        d as f32
    }

    /// Milliseconds since the start.
    pub fn total(&self) -> f32 {
        (self.last - self.start) as f32
    }
}
