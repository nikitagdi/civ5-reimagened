//! Per-mod limits (D7): payload ceilings, and a token bucket per (seat, channel) of one mod. The host keeps one
//! [`Limiter`] per mod per game, so the full key is seat x mod x channel.
//!
//! Time is an input: the caller passes `now_ms` (any monotonic millisecond clock), so behaviour is deterministic
//! and tests need no sleeping. All arithmetic is integer.

use std::collections::BTreeMap;

use mod_manifest::{CHANNEL_PAYLOAD_CEILING, Channel, Manifest};

use crate::caps::{Cap, Caps};
use crate::ids::{ChannelId, Seat};
use crate::server::Outgoing;

/// Outcome of checking one message against the limits and capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Verdict {
    Ok,
    /// The mod lacks the capability the operation needs (`net.channel` for sends).
    NoCapability(Cap),
    /// The channel is not declared in the manifest.
    UnknownChannel,
    /// The payload exceeds the channel's maximum for this direction.
    TooLarge {
        len: usize,
        max: u32,
    },
    /// The seat's token bucket is empty.
    RateLimited,
}

impl Verdict {
    pub fn is_ok(self) -> bool {
        self == Verdict::Ok
    }
}

/// An outgoing message the filter removed, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dropped {
    pub ch: ChannelId,
    pub reason: Verdict,
}

/// Result of [`Limiter::filter_outgoing`].
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Filtered {
    pub kept: Vec<Outgoing>,
    pub dropped: Vec<Dropped>,
}

#[derive(Debug, Clone, Copy)]
struct Spec {
    c2s_max: u32,
    s2c_max: u32,
    /// Refill in tokens per second.
    rate: u32,
    /// Capacity in tokens.
    burst: u32,
}

/// Milli-tokens: one token per `1000`. A refill of `rate` tokens/s is exactly `rate` milli-tokens per ms.
const MILLI: u64 = 1000;

#[derive(Debug, Clone, Copy)]
struct Bucket {
    milli: u64,
    last_ms: u64,
}

/// Which of the two independent buckets a charge goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    /// Charged 1 per received message by the host.
    Message,
    /// Charged by the mod through `ServerCtx::allow`, in units the mod chooses (for example edits).
    Cost,
}

/// Limits of one mod plus the token buckets of its seats.
#[derive(Debug, Clone)]
pub struct Limiter {
    specs: BTreeMap<ChannelId, Spec>,
    state_max: u64,
    buckets: BTreeMap<(Seat, ChannelId, Kind), Bucket>,
}

impl Limiter {
    /// Builds the limits from the manifest's channel list. Payload caps are clamped to the 56 KiB ceiling.
    pub fn new(channels: &[Channel], state_max: u64) -> Self {
        let specs = channels
            .iter()
            .map(|c| {
                (
                    c.id,
                    Spec {
                        c2s_max: c.c2s_max.min(CHANNEL_PAYLOAD_CEILING),
                        s2c_max: c.s2c_max.min(CHANNEL_PAYLOAD_CEILING),
                        rate: c.rate_per_s,
                        burst: c.burst,
                    },
                )
            })
            .collect();
        Self {
            specs,
            state_max,
            buckets: BTreeMap::new(),
        }
    }

    pub fn from_manifest(m: &Manifest) -> Self {
        Self::new(&m.channels, m.rust.state_max)
    }

    pub fn has_channel(&self, ch: ChannelId) -> bool {
        self.specs.contains_key(&ch)
    }

    /// The cap of `persist()` output.
    pub fn state_max(&self) -> u64 {
        self.state_max
    }

    /// Checks one client-to-server message: channel declared, size, then rate (a refused size or channel does not
    /// spend a token). On `Ok` one token has been spent.
    pub fn check_c2s(&mut self, seat: Seat, ch: ChannelId, len: usize, now_ms: u64) -> Verdict {
        let Some(spec) = self.specs.get(&ch).copied() else {
            return Verdict::UnknownChannel;
        };
        if len as u64 > u64::from(spec.c2s_max) {
            return Verdict::TooLarge {
                len,
                max: spec.c2s_max,
            };
        }
        if self.take(seat, ch, Kind::Message, 1, now_ms) {
            Verdict::Ok
        } else {
            Verdict::RateLimited
        }
    }

    /// Checks one server-to-client payload (no rate: the server is trusted not to flood itself).
    pub fn check_s2c(&self, ch: ChannelId, len: usize) -> Verdict {
        let Some(spec) = self.specs.get(&ch) else {
            return Verdict::UnknownChannel;
        };
        if len as u64 > u64::from(spec.s2c_max) {
            return Verdict::TooLarge {
                len,
                max: spec.s2c_max,
            };
        }
        Verdict::Ok
    }

    /// Spends `cost` tokens of the seat's mod-chosen bucket on `ch`; false when it does not have them.
    pub fn allow(&mut self, seat: Seat, ch: ChannelId, cost: u32, now_ms: u64) -> bool {
        self.specs.contains_key(&ch) && self.take(seat, ch, Kind::Cost, cost, now_ms)
    }

    /// Whether a `persist()` blob fits `state_max`.
    pub fn check_state(&self, len: usize) -> bool {
        len as u64 <= self.state_max
    }

    /// Forgets a seat's buckets (seat left the game).
    pub fn forget_seat(&mut self, seat: Seat) {
        self.buckets.retain(|(s, _, _), _| *s != seat);
    }

    /// Applies capability and size checks to what a mod returned. Without `net.channel` everything is dropped.
    /// Order of the kept messages is preserved (S5). The host logs `dropped`.
    pub fn filter_outgoing(&self, caps: &Caps, out: Vec<Outgoing>) -> Filtered {
        let mut f = Filtered::default();
        for o in out {
            let reason = if !caps.has(Cap::NetChannel) {
                Verdict::NoCapability(Cap::NetChannel)
            } else {
                self.check_s2c(o.ch, o.payload.len())
            };
            if reason.is_ok() {
                f.kept.push(o);
            } else {
                f.dropped.push(Dropped { ch: o.ch, reason });
            }
        }
        f
    }

    fn take(&mut self, seat: Seat, ch: ChannelId, kind: Kind, cost: u32, now_ms: u64) -> bool {
        let spec = self.specs[&ch];
        let cap = u64::from(spec.burst) * MILLI;
        let b = self.buckets.entry((seat, ch, kind)).or_insert(Bucket {
            milli: cap,
            last_ms: now_ms,
        });
        let elapsed = now_ms.saturating_sub(b.last_ms);
        b.last_ms = b.last_ms.max(now_ms);
        b.milli = b
            .milli
            .saturating_add(elapsed.saturating_mul(u64::from(spec.rate)))
            .min(cap);
        let need = u64::from(cost) * MILLI;
        if b.milli >= need {
            b.milli -= need;
            true
        } else {
            false
        }
    }
}
