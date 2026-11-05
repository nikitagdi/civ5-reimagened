//! Frame time ring (AC-H1): the last ~10 s of frames, fps, 1% low, CPU against GPU time and the
//! verdict "CPU / GPU / vsync". Pure logic, no wgpu: the renderer pushes one sample per frame.

use std::collections::VecDeque;

/// How much history the graph and the percentiles cover.
pub const WINDOW_MS: f64 = 10_000.0;
/// A stage that takes this part of the frame period is the one that holds the frame.
const BOUND: f32 = 0.85;

/// One frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameSample {
    /// End of the frame on the renderer's clock, ms.
    pub at_ms: f64,
    /// Time since the previous frame, ms.
    pub dt_ms: f32,
    /// CPU time of the frame (app update and encoding), ms.
    pub cpu_ms: f32,
    /// GPU time of the frame (sum of the timed passes, two or three frames old), ms.
    pub gpu_ms: Option<f32>,
}

/// What holds the frame rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Cpu,
    Gpu,
    /// Both stages are well under the frame period: the display paces the frames.
    Vsync,
    /// No GPU times and the CPU is not the limit.
    Unknown,
}

impl Verdict {
    pub fn name(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Gpu => "gpu",
            Self::Vsync => "vsync",
            Self::Unknown => "unknown",
        }
    }
}

/// The verdict for a frame period `dt` with typical CPU and GPU times, ms.
pub fn verdict(dt: f32, cpu: f32, gpu: Option<f32>) -> Verdict {
    if dt <= 0.0 {
        return Verdict::Unknown;
    }
    let (cpu_frac, gpu_frac) = (cpu / dt, gpu.map(|g| g / dt));
    match gpu_frac {
        Some(g) if g >= BOUND && g >= cpu_frac => Verdict::Gpu,
        _ if cpu_frac >= BOUND => Verdict::Cpu,
        Some(g) if g >= BOUND => Verdict::Gpu,
        Some(_) => Verdict::Vsync,
        None => Verdict::Unknown,
    }
}

/// Numbers over the window.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameSummary {
    pub frames: u32,
    /// Frames per second over the window.
    pub fps: f32,
    pub dt_p50_ms: f32,
    pub dt_p99_ms: f32,
    /// FPS of the slowest 1% of frames (1000 / p99).
    pub low1_fps: f32,
    pub cpu_ms: f32,
    pub gpu_ms: Option<f32>,
    pub verdict: Option<Verdict>,
}

#[derive(Default)]
pub struct FrameRing {
    samples: VecDeque<FrameSample>,
}

impl FrameRing {
    pub fn push(&mut self, s: FrameSample) {
        self.samples.push_back(s);
        while let Some(first) = self.samples.front() {
            if s.at_ms - first.at_ms > WINDOW_MS {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Frame periods oldest first, for the graph.
    pub fn dt_series(&self) -> impl Iterator<Item = f32> + '_ {
        self.samples.iter().map(|s| s.dt_ms)
    }

    pub fn summary(&self) -> FrameSummary {
        if self.samples.is_empty() {
            return FrameSummary::default();
        }
        let n = self.samples.len();
        let mut dt: Vec<f32> = self.samples.iter().map(|s| s.dt_ms).collect();
        dt.sort_by(f32::total_cmp);
        let mut cpu: Vec<f32> = self.samples.iter().map(|s| s.cpu_ms).collect();
        cpu.sort_by(f32::total_cmp);
        let mut gpu: Vec<f32> = self.samples.iter().filter_map(|s| s.gpu_ms).collect();
        gpu.sort_by(f32::total_cmp);
        let span = (self.samples[n - 1].at_ms - self.samples[0].at_ms).max(1.0);
        let p = |v: &[f32], q: f32| v[(((v.len() - 1) as f32) * q).round() as usize];
        let dt50 = p(&dt, 0.5);
        let dt99 = p(&dt, 0.99);
        // 1% low: the fps of the mean of the slowest 1% of the frames (at least one).
        let k = n.div_ceil(100);
        let worst = dt[n - k..].iter().sum::<f32>() / k as f32;
        let cpu50 = p(&cpu, 0.5);
        let gpu50 = (!gpu.is_empty()).then(|| p(&gpu, 0.5));
        FrameSummary {
            frames: n as u32,
            fps: if n > 1 {
                ((n - 1) as f64 * 1000.0 / span) as f32
            } else {
                0.0
            },
            dt_p50_ms: dt50,
            dt_p99_ms: dt99,
            low1_fps: if worst > 0.0 { 1000.0 / worst } else { 0.0 },
            cpu_ms: cpu50,
            gpu_ms: gpu50,
            verdict: Some(verdict(dt50, cpu50, gpu50)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(ring: &mut FrameRing, n: usize, dt: f32, cpu: f32, gpu: Option<f32>) {
        let start = ring.samples.back().map_or(0.0, |s| s.at_ms);
        for i in 1..=n {
            ring.push(FrameSample {
                at_ms: start + f64::from(dt) * i as f64,
                dt_ms: dt,
                cpu_ms: cpu,
                gpu_ms: gpu,
            });
        }
    }

    #[test]
    fn steady_frames_give_fps_and_equal_low() {
        let mut r = FrameRing::default();
        feed(&mut r, 200, 10.0, 2.0, Some(3.0));
        let s = r.summary();
        assert!((s.fps - 100.0).abs() < 1.0, "{}", s.fps);
        assert!((s.low1_fps - 100.0).abs() < 0.1);
        assert_eq!(s.verdict, Some(Verdict::Vsync));
    }

    #[test]
    fn one_percent_low_sees_the_spikes() {
        let mut r = FrameRing::default();
        for _ in 0..9 {
            feed(&mut r, 99, 10.0, 2.0, None);
            feed(&mut r, 1, 50.0, 2.0, None);
        }
        let s = r.summary();
        assert!(s.low1_fps < 25.0, "{}", s.low1_fps);
        assert!(s.fps > 70.0);
    }

    #[test]
    fn window_keeps_ten_seconds() {
        let mut r = FrameRing::default();
        feed(&mut r, 3000, 10.0, 1.0, None); // 30 s
        let span = r.samples.back().unwrap().at_ms - r.samples.front().unwrap().at_ms;
        assert!(span <= WINDOW_MS && span > WINDOW_MS - 20.0);
        assert!(r.len() <= 1001);
    }

    #[test]
    fn verdicts() {
        assert_eq!(verdict(10.0, 9.5, Some(3.0)), Verdict::Cpu);
        assert_eq!(verdict(10.0, 3.0, Some(9.5)), Verdict::Gpu);
        assert_eq!(verdict(16.7, 3.0, Some(4.0)), Verdict::Vsync);
        assert_eq!(verdict(16.7, 3.0, None), Verdict::Unknown);
        assert_eq!(verdict(10.0, 9.5, None), Verdict::Cpu);
    }
}
