//! The cosmetic terrain share (`world.share_terrain`, API 1.2): a server half lets one seat draw the ground of plots
//! that seat has not revealed (an arena in another player's land), so a first-person camera there stands on relief,
//! the same relief on every client.
//!
//! The host sends the seat the fog-free basis records of those plots (relief, terrain, continent art, relief
//! features, lake and coast flags, rivers): what the look of the ground is made of, nothing more. No owner, no
//! improvement, route, resource, unit or city goes with them, and the plots do not become revealed: the strategic
//! map, the minimap, the tooltips and the game state of the seat do not change. The client draws them like fogged
//! revealed land until the seat really reveals them.
//!
//! [`ShareQueue`] (one call's requests) and [`TerrainShares`] (what each seat was given) are the shared halves of the
//! host rules, used by the server host and by the test double alike: at most [`MAX_SHARE_PLOTS_PER_CALL`] plots per
//! call into the mod, duplicates and plots off the map dropped, a plot already shared to the seat by this mod not
//! sent again.

use std::collections::{BTreeMap, BTreeSet};

use crate::facts::PlotIdx;
use crate::ids::Seat;

/// The most plots one call into a mod may share (all its `share_terrain` calls together); the rest is dropped (and
/// logged).
pub const MAX_SHARE_PLOTS_PER_CALL: usize = 256;

/// One request of a call, in the order made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShareOp {
    /// `ServerCtx::share_terrain`.
    Share { seat: Seat, plots: Vec<PlotIdx> },
    /// `ServerCtx::unshare_terrain`.
    Unshare { seat: Seat },
}

/// The share requests one call made (host side of `share_terrain`/`unshare_terrain`). At most
/// [`MAX_SHARE_PLOTS_PER_CALL`] plots are taken over the call; what was refused is counted for one warning.
#[derive(Debug, Clone, Default)]
pub struct ShareQueue {
    ops: Vec<ShareOp>,
    plots: usize,
    over: usize,
}

impl ShareQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn share(&mut self, seat: Seat, plots: &[PlotIdx]) {
        let room = MAX_SHARE_PLOTS_PER_CALL - self.plots;
        let take = plots.len().min(room);
        self.over += plots.len() - take;
        if take > 0 {
            self.plots += take;
            self.ops.push(ShareOp::Share {
                seat,
                plots: plots[..take].to_vec(),
            });
        }
    }

    pub fn unshare(&mut self, seat: Seat) {
        self.ops.push(ShareOp::Unshare { seat });
    }

    /// The requests in order, and the warning for the log if plots were refused.
    pub fn finish(self) -> (Vec<ShareOp>, Option<String>) {
        let warn = (self.over > 0).then(|| {
            format!(
                "share_terrain: {} plots over {MAX_SHARE_PLOTS_PER_CALL} in one call were dropped",
                self.over
            )
        });
        (self.ops, warn)
    }
}

/// What the host sends one seat for one request: the new plots in the order first given (`reset`: the mod's earlier
/// share to the seat is dropped first).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareOut {
    pub seat: Seat,
    pub reset: bool,
    pub plots: Vec<PlotIdx>,
}

/// What one mod has shared to each seat. The host keeps one per mod of a game.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TerrainShares {
    by_seat: BTreeMap<Seat, BTreeSet<PlotIdx>>,
}

impl TerrainShares {
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies one call's requests on a map of `plot_count` plots: what to send, in order (nothing for a request that
    /// adds no plot, or an unshare of a seat that has none).
    pub fn apply(&mut self, ops: Vec<ShareOp>, plot_count: u32) -> Vec<ShareOut> {
        let mut out = Vec::new();
        for op in ops {
            match op {
                ShareOp::Share { seat, plots } => {
                    let have = self.by_seat.entry(seat).or_default();
                    let new: Vec<PlotIdx> = plots
                        .into_iter()
                        .filter(|&p| p < plot_count && have.insert(p))
                        .collect();
                    if have.is_empty() {
                        self.by_seat.remove(&seat);
                    }
                    if !new.is_empty() {
                        out.push(ShareOut {
                            seat,
                            reset: false,
                            plots: new,
                        });
                    }
                }
                ShareOp::Unshare { seat } => {
                    if self.by_seat.remove(&seat).is_some() {
                        out.push(ShareOut {
                            seat,
                            reset: true,
                            plots: Vec::new(),
                        });
                    }
                }
            }
        }
        out
    }

    /// The plots shared to the seat, ascending.
    pub fn shared(&self, seat: Seat) -> impl Iterator<Item = PlotIdx> + '_ {
        self.by_seat.get(&seat).into_iter().flatten().copied()
    }

    /// The host forgets the seat (its client went away or got a fresh stream, which starts with no share): nothing is
    /// sent, the next share sends every plot again.
    pub fn forget_seat(&mut self, seat: Seat) {
        self.by_seat.remove(&seat);
    }

    pub fn is_empty(&self) -> bool {
        self.by_seat.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_call_takes_at_most_the_limit_over_all_its_requests() {
        let mut q = ShareQueue::new();
        let a: Vec<PlotIdx> = (0..200).collect();
        q.share(Seat(1), &a);
        q.unshare(Seat(2));
        q.share(Seat(2), &a);
        let (ops, warn) = q.finish();
        assert_eq!(ops.len(), 3);
        let taken: usize = ops
            .iter()
            .map(|o| match o {
                ShareOp::Share { plots, .. } => plots.len(),
                ShareOp::Unshare { .. } => 0,
            })
            .sum();
        assert_eq!(taken, MAX_SHARE_PLOTS_PER_CALL);
        assert!(warn.unwrap().contains("144 plots"));
        // Full: a further request adds no op.
        let mut q = ShareQueue::new();
        q.share(Seat(0), &vec![0; MAX_SHARE_PLOTS_PER_CALL]);
        q.share(Seat(0), &[1]);
        assert_eq!(q.finish().0.len(), 1);
        assert!(ShareQueue::new().finish().1.is_none());
    }

    #[test]
    fn only_new_plots_on_the_map_are_sent_and_unshare_resets() {
        let mut s = TerrainShares::new();
        let share = |seat, plots: &[PlotIdx]| ShareOp::Share {
            seat: Seat(seat),
            plots: plots.to_vec(),
        };
        let out = s.apply(vec![share(1, &[5, 3, 5, 99, 3, 7])], 10);
        assert_eq!(
            out,
            [ShareOut {
                seat: Seat(1),
                reset: false,
                plots: vec![5, 3, 7]
            }]
        );
        // Again: nothing new, nothing sent; another seat is apart.
        assert!(s.apply(vec![share(1, &[3, 7])], 10).is_empty());
        let out = s.apply(vec![share(2, &[3])], 10);
        assert_eq!(out[0].plots, [3]);
        assert_eq!(s.shared(Seat(1)).collect::<Vec<_>>(), [3, 5, 7]);
        // Unshare: a reset for that seat only; an unshare of a seat with nothing sends nothing.
        let out = s.apply(
            vec![
                ShareOp::Unshare { seat: Seat(1) },
                ShareOp::Unshare { seat: Seat(4) },
            ],
            10,
        );
        assert_eq!(
            out,
            [ShareOut {
                seat: Seat(1),
                reset: true,
                plots: vec![]
            }]
        );
        assert_eq!(s.shared(Seat(1)).count(), 0);
        assert_eq!(s.shared(Seat(2)).collect::<Vec<_>>(), [3]);
        // A forgotten seat gets everything again.
        s.forget_seat(Seat(2));
        assert_eq!(s.apply(vec![share(2, &[3])], 10)[0].plots, [3]);
        // Nothing on the map: no entry is kept.
        let mut t = TerrainShares::new();
        assert!(t.apply(vec![share(0, &[10, 11])], 10).is_empty());
        assert!(t.is_empty());
    }
}
