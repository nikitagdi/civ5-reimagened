//! The one record that the HUD, `window.__civ5stats` and the desktop log line are made from
//! (AC-H7): they cannot disagree because they all read this.

use super::frame::FrameSummary;

/// One row of the pass table.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PassRow {
    pub name: &'static str,
    pub gpu_ms: Option<f32>,
    pub cpu_ms: f32,
    pub draw_calls: u32,
    pub triangles: u64,
    pub instances: u64,
    pub pipeline_sets: u32,
    pub bind_group_sets: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PerfSnapshot {
    pub frame: FrameSummary,
    pub passes: Vec<PassRow>,
    /// What `Main` drew, by group, the largest in triangles first (see [`super::groups`]).
    pub main_groups: Vec<super::groups::GroupRow>,
    /// The groups `Main` is told to skip (the debug switches of the panel; 0 is the normal state).
    pub groups_off: u32,
    /// Frame periods of the window, ms, oldest first, at most [`GRAPH_POINTS`] values.
    pub graph: Vec<f32>,
    /// Every named number: `gpu_mem.*`, `ram.*`, `frame.*`, the counters of the subsystems and of
    /// other layers (`terrain.*`, `net.*`, `assets.*`, `storage.*`), by name.
    pub counters: Vec<(String, f64)>,
    /// The frames over 33 ms and where their time went, the last 16 (see [`super::slow`]).
    pub slow_frames: Vec<super::slow::SlowFrame>,
    /// The last frames, slow or not, with the waits of the render loop (see [`super::slow::TraceRow`]).
    pub frame_trace: Vec<super::slow::TraceRow>,
}

/// A long frame that comes back at the same distance: `every` frames apart, `excess_ms` longer than
/// a normal one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StallCycle {
    pub every: usize,
    pub excess_ms: f32,
}

/// Whether the long frames of `dt` (frame periods, ms, oldest first) repeat with a period of 2 to 8
/// frames: a frame is long past 1.5 times the median, and most gaps between long frames must be the
/// same. `None` for a series with no such rhythm (a steady one, or spikes at random).
pub fn stall_cycle(dt: &[f32]) -> Option<StallCycle> {
    if dt.len() < 12 {
        return None;
    }
    let mut sorted = dt.to_vec();
    sorted.sort_by(f32::total_cmp);
    let median = sorted[sorted.len() / 2];
    if median <= 0.0 {
        return None;
    }
    let limit = median * 1.5;
    let long: Vec<usize> = (0..dt.len()).filter(|&i| dt[i] > limit).collect();
    if long.len() < 4 {
        return None;
    }
    let gaps: Vec<usize> = long.windows(2).map(|w| w[1] - w[0]).collect();
    let mut best = (0usize, 0usize);
    for every in 2..=8 {
        let n = gaps.iter().filter(|&&g| g == every).count();
        if n > best.1 {
            best = (every, n);
        }
    }
    // Three quarters of the gaps are the same.
    if best.1 * 4 < gaps.len() * 3 {
        return None;
    }
    let excess = long.iter().map(|&i| dt[i] - median).sum::<f32>() / long.len() as f32;
    Some(StallCycle {
        every: best.0,
        excess_ms: excess,
    })
}

/// The graph is cut down to this many points (the window has up to ~1500 frames at 144 fps).
pub const GRAPH_POINTS: usize = 300;

/// Takes at most `max` values, the worst (largest) of each bucket, so spikes survive.
pub fn decimate(series: impl Iterator<Item = f32>, max: usize) -> Vec<f32> {
    let all: Vec<f32> = series.collect();
    if all.len() <= max || max == 0 {
        return all;
    }
    let bucket = all.len().div_ceil(max);
    all.chunks(bucket)
        .map(|c| c.iter().copied().fold(0.0, f32::max))
        .collect()
}

impl PerfSnapshot {
    /// `(group, [(rest of the name, value)])` by the text before the first dot, groups in order of
    /// first appearance. Names without a dot go to the group `""`.
    pub fn groups(&self) -> Vec<(String, Vec<(&str, f64)>)> {
        let mut out: Vec<(String, Vec<(&str, f64)>)> = Vec::new();
        for (name, v) in &self.counters {
            let (g, rest) = name.split_once('.').unwrap_or(("", name));
            match out.iter_mut().find(|(k, _)| k == g) {
                Some((_, rows)) => rows.push((rest, *v)),
                None => out.push((g.to_owned(), vec![(rest, *v)])),
            }
        }
        out
    }

    /// The value of a counter by its full name.
    pub fn counter(&self, name: &str) -> Option<f64> {
        self.counters
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| *v)
    }

    /// One log line for the desktop: `perf fps=… p50=…ms low1=…fps cpu=…ms gpu=…ms verdict=… …`.
    pub fn log_line(&self) -> String {
        let f = &self.frame;
        let gpu = f.gpu_ms.map_or("-".to_owned(), |g| format!("{g:.2}"));
        let verdict = f.verdict.map_or("-", |v| v.name());
        let mb = |n: &str| self.counter(n).map_or(0.0, |b| b / (1024.0 * 1024.0));
        format!(
            "perf fps={:.1} p50={:.2}ms low1={:.1}fps cpu={:.2}ms gpu={}ms verdict={} \
             gpu_mem={:.1}MB ram_live={:.1}MB wasm={:.1}MB draws={} tris={}",
            f.fps,
            f.dt_p50_ms,
            f.low1_fps,
            f.cpu_ms,
            gpu,
            verdict,
            mb("gpu_mem.total"),
            mb("ram.heap_live"),
            mb("ram.wasm_linear"),
            self.passes.iter().map(|p| p.draw_calls).sum::<u32>(),
            self.passes.iter().map(|p| p.triangles).sum::<u64>(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decimate_keeps_spikes() {
        let mut s = vec![1.0_f32; 1000];
        s[501] = 40.0;
        let d = decimate(s.into_iter(), 100);
        assert!(d.len() <= 100);
        assert!(d.contains(&40.0));
    }

    #[test]
    fn a_long_frame_every_third_is_a_cycle_and_a_steady_or_random_series_is_not() {
        let phone: Vec<f32> = (0..60)
            .map(|i| if i % 3 == 2 { 50.0 } else { 20.0 })
            .collect();
        let c = stall_cycle(&phone).expect("a cycle");
        assert_eq!(c.every, 3);
        assert!((c.excess_ms - 30.0).abs() < 1e-3);
        assert_eq!(stall_cycle(&[16.7; 60]), None);
        // Spikes that come at uneven distances.
        let mut random = vec![16.0_f32; 80];
        for i in [5, 9, 20, 22, 41, 60, 71] {
            random[i] = 60.0;
        }
        assert_eq!(stall_cycle(&random), None);
        assert_eq!(stall_cycle(&[20.0, 50.0]), None, "too short to tell");
    }

    #[test]
    fn groups_split_on_first_dot() {
        let snap = PerfSnapshot {
            counters: vec![
                ("terrain.holes".into(), 2.0),
                ("net.in_bps".into(), 10.0),
                ("terrain.rebakes".into(), 3.0),
                ("plain".into(), 1.0),
            ],
            ..Default::default()
        };
        let g = snap.groups();
        assert_eq!(g[0].0, "terrain");
        assert_eq!(g[0].1, vec![("holes", 2.0), ("rebakes", 3.0)]);
        assert_eq!(g[1].0, "net");
        assert_eq!(g[2].0, "");
    }

    #[test]
    fn log_line_has_the_numbers() {
        let snap = PerfSnapshot::default();
        let l = snap.log_line();
        assert!(l.starts_with("perf fps=0.0"), "{l}");
    }
}
