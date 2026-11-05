//! What a step costs (`StepStats`, game-host-mp 4.17): the microseconds of each part, filled while the step runs
//! and put into the `Stats` message at the end of the call when `cfg.stats` is on.

use std::time::Instant;

use protocol::{PlayerId, StepKind, StepStats};

/// Microseconds spent in the parts of a step (`StepStats`).
#[derive(Debug, Default, Clone)]
pub(crate) struct StepTimes {
    pub apply_us: u32,
    pub pump_us: u32,
    pub views_us: u32,
    pub diff_us: u32,
    pub ctx_us: u32,
    pub save_us: u32,
    pub ai_us: u32,
    pub diff_bytes: Vec<(PlayerId, u32)>,
    pub fog_violations: u32,
    pub digest: Option<u64>,
}

impl StepTimes {
    /// The message for the gateway.
    pub(crate) fn into_stats(self, kind: StepKind) -> StepStats {
        StepStats {
            kind,
            apply_us: self.apply_us,
            pump_us: self.pump_us,
            views_us: self.views_us,
            diff_us: self.diff_us,
            ctx_us: self.ctx_us,
            save_us: self.save_us,
            ai_us: self.ai_us,
            diff_bytes: self.diff_bytes,
            fog_violations: self.fog_violations,
            digest: self.digest,
        }
    }
}

pub(crate) fn micros(t: Instant) -> u32 {
    u32::try_from(t.elapsed().as_micros()).unwrap_or(u32::MAX)
}
