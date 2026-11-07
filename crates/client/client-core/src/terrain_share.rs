//! The cosmetic terrain share of the mods (`GameMsg::TerrainShare`, mod API 1.2): plots whose ground the mods let
//! this seat draw although it has not revealed them (an arena in another player's land).
//!
//! It is kept beside the view, never in it: the plots do not become revealed, the strategic map, the minimap, the
//! tooltips and the orders see the view as the server sent it. The renderer alone reads [`SharedTerrain`] (the
//! client app hands it over when [`SharedTerrain::rev`] moves) and draws those hexes' ground like fogged revealed
//! land. The share of each mod is kept apart: one mod's `unshare_terrain` leaves the others'.

use std::collections::BTreeMap;

use protocol::{PlotBasisView, TerrainShare};

/// The plots shared to this seat, by mod. Starts empty with every fresh stream and every game.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SharedTerrain {
    by_mod: BTreeMap<u16, BTreeMap<u32, PlotBasisView>>,
    rev: u64,
}

impl SharedTerrain {
    /// Takes one message: `reset` drops the mod's share first, then its plots are added (a plot sent again keeps the
    /// newer record).
    pub fn apply(&mut self, share: TerrainShare) {
        let before = self.by_mod.get(&share.mod_idx).cloned();
        let plots = self.by_mod.entry(share.mod_idx).or_default();
        if share.reset {
            plots.clear();
        }
        plots.extend(share.plots);
        if plots.is_empty() {
            self.by_mod.remove(&share.mod_idx);
        }
        if self.by_mod.get(&share.mod_idx) != before.as_ref() {
            self.rev += 1;
        }
    }

    /// Nothing is shared any more (a fresh stream, another game).
    pub fn clear(&mut self) {
        if !self.by_mod.is_empty() {
            self.by_mod.clear();
            self.rev += 1;
        }
    }

    /// Grows with every change: the renderer is given the share again when it moved.
    pub fn rev(&self) -> u64 {
        self.rev
    }

    pub fn is_empty(&self) -> bool {
        self.by_mod.is_empty()
    }

    /// Every shared plot once, ascending by index, with its record (the mod first in the load order wins a plot two
    /// mods share; their records are the same fog-free basis anyway).
    pub fn plots(&self) -> Vec<(u32, &PlotBasisView)> {
        let mut all: BTreeMap<u32, &PlotBasisView> = BTreeMap::new();
        for plots in self.by_mod.values() {
            for (i, b) in plots {
                all.entry(*i).or_insert(b);
            }
        }
        all.into_iter().collect()
    }

    /// Whether some mod shares the plot.
    pub fn contains(&self, index: u32) -> bool {
        self.by_mod.values().any(|p| p.contains_key(&index))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::PlotType;

    fn rec(plot_type: PlotType) -> PlotBasisView {
        PlotBasisView {
            plot_type,
            ..PlotBasisView::default()
        }
    }

    fn share(mod_idx: u16, reset: bool, plots: &[u32]) -> TerrainShare {
        TerrainShare {
            mod_idx,
            reset,
            plots: plots.iter().map(|&i| (i, rec(PlotType::Hill))).collect(),
        }
    }

    #[test]
    fn shares_of_mods_are_kept_apart_and_reset_alone() {
        let mut s = SharedTerrain::default();
        assert!(s.is_empty() && s.rev() == 0);
        s.apply(share(1, false, &[5, 3]));
        s.apply(share(2, false, &[3, 9]));
        assert_eq!(s.rev(), 2);
        let idx: Vec<u32> = s.plots().iter().map(|(i, _)| *i).collect();
        assert_eq!(idx, [3, 5, 9]);
        // The same again changes nothing.
        s.apply(share(1, false, &[5]));
        assert_eq!(s.rev(), 2);
        // Mod 1 unshares: mod 2's plots stay.
        s.apply(share(1, true, &[]));
        assert_eq!(s.rev(), 3);
        assert!(!s.contains(5) && s.contains(3) && s.contains(9));
        // A reset of a mod with nothing changes nothing.
        s.apply(share(7, true, &[]));
        assert_eq!(s.rev(), 3);
        s.clear();
        assert!(s.is_empty() && s.rev() == 4);
        s.clear();
        assert_eq!(s.rev(), 4);
    }
}
