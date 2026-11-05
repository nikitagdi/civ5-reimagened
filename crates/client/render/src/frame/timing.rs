//! Time of the frame passes. With `TIMESTAMP_QUERY` each pass the
//! renderer opens writes a start and an end timestamp; the results come back through a ring of
//! buffers two or three frames later, nothing waits. Without the feature (WebGL2, some adapters)
//! only the CPU time of recording the pass is known and `gpu_ms` is `None` ("GPU -" in the table).
//!
//! `PassStat` rows are made for every pass of table 4.1 (`PassId::ORDER`), also for those that did
//! no work in the frame, so `?stats=1` always lists all of them (AC-R8.6).

use crate::perf::{Buf, Cluster, DeviceTrack};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use super::graph::PassId;

/// One row of `FrameStats::passes`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PassStat {
    pub pass: PassId,
    /// GPU time of the pass, from a frame two or three back; `None` without timestamp queries or
    /// for a pass that is not timed on its own (`Resolve` is part of `Main`; passes a subsystem
    /// encodes itself are timed only if it uses [`crate::frame::EncoderContext::timestamps`]).
    pub gpu_ms: Option<f32>,
    /// Time spent recording the pass on the CPU, in this frame.
    pub cpu_ms: f32,
    pub draws: u32,
    pub triangles: u32,
    /// What the pass did on the command side: draw calls, triangles and instances drawn, pipeline
    /// and bind group switches (`perf::PassCount`; AC-H4).
    pub counts: crate::perf::DrawCounts,
}

impl PassStat {
    /// The number `window.__civ5stats.passes` shows: GPU time when the device gives it, else the
    /// CPU time.
    pub fn shown_ms(&self) -> f32 {
        self.gpu_ms.unwrap_or(self.cpu_ms)
    }
}

/// Name of a pass as `?stats=1` and `window.__civ5stats.passes` show it (the variant name).
pub fn pass_name(pass: PassId) -> &'static str {
    match pass {
        PassId::Prepare => "Prepare",
        PassId::Minimap => "Minimap",
        PassId::TerrainBake => "TerrainBake",
        PassId::WaterPrep => "WaterPrep",
        PassId::ShadowMap => "ShadowMap",
        PassId::ShadowBlur => "ShadowBlur",
        PassId::Reflection => "Reflection",
        PassId::ScreenOverlay => "ScreenOverlay",
        PassId::FowMask => "FowMask",
        PassId::Main => "Main",
        PassId::Resolve => "Resolve",
        PassId::Present => "Present",
        PassId::UiGame => "UiGame",
        PassId::Ui => "Ui",
        PassId::ClassMask => "ClassMask",
    }
}

/// Milliseconds from a monotonic clock; the renderer takes it from the app (`set_cpu_clock`).
pub type CpuClock = Box<dyn Fn() -> f64>;

/// The clock a renderer starts with: `Instant` natively. The browser has no `Instant`; there the
/// app gives the clock (`performance.now()`), and until it does the CPU times read 0.
pub fn default_clock() -> CpuClock {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let start = std::time::Instant::now();
        Box::new(move || start.elapsed().as_secs_f64() * 1000.0)
    }
    #[cfg(target_arch = "wasm32")]
    {
        Box::new(|| 0.0)
    }
}

/// CPU time and counts of one frame, per pass.
#[derive(Clone, Debug)]
pub struct PassTally {
    rows: Vec<PassStat>,
}

impl Default for PassTally {
    fn default() -> Self {
        Self::new()
    }
}

impl PassTally {
    pub fn new() -> Self {
        Self {
            rows: PassId::ORDER
                .iter()
                .map(|&pass| PassStat {
                    pass,
                    gpu_ms: None,
                    cpu_ms: 0.0,
                    draws: 0,
                    triangles: 0,
                    counts: Default::default(),
                })
                .collect(),
        }
    }

    /// Adds a step's recording time and draws to its pass. `ClassMask` gets a row on first use.
    pub fn add(&mut self, pass: PassId, cpu_ms: f64, draws: u32, triangles: u32) {
        let at = match self.rows.iter().position(|r| r.pass == pass) {
            Some(at) => at,
            None => {
                self.rows.push(PassStat {
                    pass,
                    gpu_ms: None,
                    cpu_ms: 0.0,
                    draws: 0,
                    triangles: 0,
                    counts: Default::default(),
                });
                self.rows.len() - 1
            }
        };
        let row = &mut self.rows[at];
        row.cpu_ms += cpu_ms.max(0.0) as f32;
        row.draws += draws;
        row.triangles += triangles;
    }

    /// Adds the command counts of a step to its pass (`ClassMask` gets a row on first use).
    pub fn add_counts(&mut self, pass: PassId, counts: crate::perf::DrawCounts) {
        self.add(pass, 0.0, 0, 0);
        if let Some(row) = self.rows.iter_mut().find(|r| r.pass == pass) {
            row.counts.add(counts);
        }
    }

    /// The rows with the latest GPU times filled in.
    pub fn finish(mut self, gpu: &GpuTimer) -> Vec<PassStat> {
        for row in &mut self.rows {
            row.gpu_ms = gpu.gpu_ms(row.pass);
        }
        self.rows
    }
}

/// Passes timed per frame: two queries each.
const MAX_PASSES: u32 = 24;
const QUERIES: u32 = MAX_PASSES * 2;
/// Frames in flight: a result is read two to three frames after it was written.
const RING: usize = 3;

const FREE: u8 = 0;
const PENDING: u8 = 1;
const READY: u8 = 2;
const FAILED: u8 = 3;

struct Slot {
    queries: wgpu::QuerySet,
    resolve: Buf,
    read: Buf,
    state: Arc<AtomicU8>,
    passes: Vec<PassId>,
}

/// Where in the query set a pass writes its two timestamps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimerSpan(u32);

/// GPU time of the passes, through timestamp queries and a ring of read-back buffers.
pub struct GpuTimer {
    slots: Vec<Slot>,
    period_ns: f32,
    current: Option<usize>,
    next: usize,
    latest: Vec<(PassId, f32)>,
}

impl GpuTimer {
    /// A timer that works only if `device` was made with `TIMESTAMP_QUERY`; otherwise every call
    /// does nothing and `gpu_ms` is `None`.
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        let mut slots = Vec::new();
        if device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            let bytes = u64::from(QUERIES) * 8;
            for _ in 0..RING {
                slots.push(Slot {
                    queries: device.create_query_set(&wgpu::QuerySetDescriptor {
                        label: Some("pass timestamps"),
                        ty: wgpu::QueryType::Timestamp,
                        count: QUERIES,
                    }),
                    resolve: device.buf(
                        &wgpu::BufferDescriptor {
                            label: Some("pass timestamps resolve"),
                            size: bytes,
                            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                            mapped_at_creation: false,
                        },
                        Cluster::Other,
                    ),
                    read: device.buf(
                        &wgpu::BufferDescriptor {
                            label: Some("pass timestamps read"),
                            size: bytes,
                            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                            mapped_at_creation: false,
                        },
                        Cluster::Other,
                    ),
                    state: Arc::new(AtomicU8::new(FREE)),
                    passes: Vec::new(),
                });
            }
        }
        Self {
            slots,
            period_ns: queue.get_timestamp_period(),
            current: None,
            next: 0,
            latest: Vec::new(),
        }
    }

    /// True if the device gives GPU times.
    pub fn available(&self) -> bool {
        !self.slots.is_empty()
    }

    /// Collects the results that have arrived and picks the slot of this frame. A frame for which
    /// all slots are still in flight is not timed.
    pub fn begin_frame(&mut self, device: &wgpu::Device) {
        self.current = None;
        if self.slots.is_empty() {
            return;
        }
        // Non-blocking; on the web the callbacks come from the event loop and this does nothing.
        let _ = device.poll(wgpu::PollType::Poll);
        for slot in &mut self.slots {
            match slot.state.load(Ordering::Acquire) {
                READY => {
                    {
                        let bytes = slot.passes.len() as u64 * 16;
                        let range = slot.read.get_mapped_range(0..bytes);
                        if let Ok(range) = range {
                            let stamp = |i: usize| {
                                let b: [u8; 8] = range[i * 8..i * 8 + 8].try_into().unwrap();
                                u64::from_le_bytes(b)
                            };
                            for (i, &pass) in slot.passes.iter().enumerate() {
                                let (start, end) = (stamp(2 * i), stamp(2 * i + 1));
                                if end >= start {
                                    let ms = (end - start) as f64 * f64::from(self.period_ns) / 1e6;
                                    set_latest(&mut self.latest, pass, ms as f32);
                                }
                            }
                        }
                    }
                    slot.read.unmap();
                    slot.passes.clear();
                    slot.state.store(FREE, Ordering::Release);
                }
                FAILED => {
                    slot.passes.clear();
                    slot.state.store(FREE, Ordering::Release);
                }
                _ => {}
            }
        }
        for k in 0..self.slots.len() {
            let i = (self.next + k) % self.slots.len();
            if self.slots[i].state.load(Ordering::Acquire) == FREE {
                self.slots[i].passes.clear();
                self.current = Some(i);
                self.next = (i + 1) % self.slots.len();
                break;
            }
        }
    }

    /// Takes the next pair of queries of this frame for `pass`; `None` if the frame is not timed or
    /// the query set is full. Pass `Self::writes(span)` as the pass's `timestamp_writes`.
    pub fn reserve(&mut self, pass: PassId) -> Option<TimerSpan> {
        let slot = &mut self.slots[self.current?];
        if slot.passes.len() as u32 >= MAX_PASSES {
            return None;
        }
        slot.passes.push(pass);
        Some(TimerSpan((slot.passes.len() as u32 - 1) * 2))
    }

    /// Gives back the span of the pass reserved last, if it was not used.
    pub fn release(&mut self, span: TimerSpan) {
        if let Some(i) = self.current {
            let passes = &mut self.slots[i].passes;
            if passes.len() as u32 * 2 == span.0 + 2 {
                passes.pop();
            }
        }
    }

    pub fn writes(&self, span: TimerSpan) -> wgpu::RenderPassTimestampWrites<'_> {
        let slot = &self.slots[self.current.expect("a span comes from a timed frame")];
        wgpu::RenderPassTimestampWrites {
            query_set: &slot.queries,
            beginning_of_pass_write_index: Some(span.0),
            end_of_pass_write_index: Some(span.0 + 1),
        }
    }

    /// Resolves this frame's queries into the read-back buffer; call after the last timed pass.
    pub fn end_frame(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let Some(i) = self.current else { return };
        let slot = &self.slots[i];
        let n = slot.passes.len() as u32 * 2;
        if n == 0 {
            return;
        }
        encoder.resolve_query_set(&slot.queries, 0..n, &slot.resolve, 0);
        encoder.copy_buffer_to_buffer(&slot.resolve, 0, &slot.read, 0, u64::from(n) * 8);
    }

    /// Starts the read of this frame's results; call after the frame's commands were submitted.
    pub fn after_submit(&mut self) {
        let Some(i) = self.current.take() else { return };
        let slot = &self.slots[i];
        let n = slot.passes.len() as u64 * 8 * 2;
        if n == 0 {
            return;
        }
        slot.state.store(PENDING, Ordering::Release);
        let state = Arc::clone(&slot.state);
        slot.read.map_async(wgpu::MapMode::Read, 0..n, move |r| {
            state.store(if r.is_ok() { READY } else { FAILED }, Ordering::Release);
        });
    }

    /// Drops the newest GPU time of `pass`: a pass that is no longer encoded would show the time of
    /// its last run for ever (a shadow pass switched off still read 13.6 ms with no draws).
    pub fn forget(&mut self, pass: PassId) {
        forget_latest(&mut self.latest, pass);
    }

    /// The newest GPU time of `pass`.
    pub fn gpu_ms(&self, pass: PassId) -> Option<f32> {
        self.latest
            .iter()
            .find(|(p, _)| *p == pass)
            .map(|(_, ms)| *ms)
    }
}

fn set_latest(latest: &mut Vec<(PassId, f32)>, pass: PassId, ms: f32) {
    match latest.iter_mut().find(|(p, _)| *p == pass) {
        Some(row) => row.1 = ms,
        None => latest.push((pass, ms)),
    }
}

fn forget_latest(latest: &mut Vec<(PassId, f32)>, pass: PassId) {
    latest.retain(|(p, _)| *p != pass);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pass_that_stops_running_loses_its_last_gpu_time_and_the_others_keep_theirs() {
        let mut latest = Vec::new();
        set_latest(&mut latest, PassId::ShadowMap, 13.6);
        set_latest(&mut latest, PassId::Main, 9.0);
        forget_latest(&mut latest, PassId::ShadowMap);
        assert_eq!(latest, [(PassId::Main, 9.0)]);
        forget_latest(&mut latest, PassId::ShadowMap);
        assert_eq!(
            latest.len(),
            1,
            "forgetting what is not there changes nothing"
        );
    }

    #[test]
    fn the_tally_has_a_row_for_every_pass_of_the_table() {
        let t = PassTally::new();
        let rows = t.rows.iter().map(|r| r.pass).collect::<Vec<_>>();
        assert_eq!(rows, PassId::ORDER);
    }

    #[test]
    fn steps_add_up_per_pass() {
        let mut t = PassTally::new();
        t.add(PassId::Main, 0.5, 3, 30);
        t.add(PassId::Main, 0.25, 2, 20);
        t.add(PassId::Prepare, 0.1, 0, 0);
        t.add(PassId::ClassMask, 0.2, 1, 2);
        let main = t.rows.iter().find(|r| r.pass == PassId::Main).unwrap();
        assert_eq!((main.draws, main.triangles), (5, 50));
        assert!((main.cpu_ms - 0.75).abs() < 1e-6);
        assert_eq!(t.rows.len(), PassId::ORDER.len() + 1, "ClassMask got a row");
    }

    #[test]
    fn the_shown_time_falls_back_to_the_cpu() {
        let mut s = PassStat {
            pass: PassId::Main,
            gpu_ms: None,
            cpu_ms: 1.5,
            draws: 0,
            triangles: 0,
            counts: Default::default(),
        };
        assert_eq!(s.shown_ms(), 1.5);
        s.gpu_ms = Some(4.0);
        assert_eq!(s.shown_ms(), 4.0);
    }

    #[test]
    fn every_pass_has_a_name_that_is_its_variant() {
        for pass in PassId::ORDER {
            assert_eq!(pass_name(pass), format!("{pass:?}"));
        }
        assert_eq!(pass_name(PassId::ClassMask), "ClassMask");
    }
}
