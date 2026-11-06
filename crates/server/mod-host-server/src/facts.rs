//! The whole-truth map facts a server mod reads (`mod_api::WorldFacts`), kept by the game actor (T22).
//!
//! [`WorldTable`] is plain data the actor feeds from three sources: the shape of the map and the turn
//! (`WorkerReady`, `TurnInfo`), the terrain classes (built once from the fog-free `MapBasis`, patched by its diffs)
//! and the owners (`WorkerMsg::Owners`, the whole truth of the worker). It implements the trait itself: a call of a
//! mod reads these vectors and allocates nothing. Spike: `specs/modding/spikes/owners.md`.
//!
//! Freshness: the actor applies an `Owners` message before the `Results` that follow it in the same worker frame, so
//! a mod never sees a table older than the last results the actor has seen. Until the first `Full` of a worker (and
//! again after a worker restart or a lost `Delta`) every owner is `Owner::Unknown`; a mod must refuse edits then.

use mod_api::{MapShape, Owner, PlayerId, PlotClass, PlotIdx, Seat, WorldFacts};
use protocol::{MapBasis, OwnersUpdate, PlotBasisView, PlotFlags, PlotType};

/// What [`WorldTable::apply_owners`] did with an update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Applied {
    /// The table is current.
    Ok,
    /// A `Delta` that does not follow the revision the table has: the table is `Unknown` until the next `Full`; ask
    /// the worker for one.
    Gap,
    /// The update does not fit the map (wrong size, index out of range); the table is dropped (a `Delta`) or kept
    /// (a `Full`), ask the worker for a `Full`.
    Bad,
}

/// Plot classes, owners and the numbers around them. See the module doc.
#[derive(Debug, Clone)]
pub struct WorldTable {
    shape: MapShape,
    turn: i32,
    /// `y * width + x`; empty: not known.
    owners: Vec<i8>,
    /// Revision of the last update applied.
    rev: u32,
    classes: Vec<PlotClass>,
    /// Catalog id of `FEATURE_ICE`, or -1.
    ice: i16,
    /// Bit p: player p has a human seat in this game (players 0..63).
    humans: u64,
}

impl Default for WorldTable {
    fn default() -> Self {
        Self::new()
    }
}

impl WorldTable {
    /// An empty table: a map of no plots, nothing known.
    pub fn new() -> Self {
        Self {
            shape: MapShape {
                width: 0,
                height: 0,
                wrap_x: false,
                wrap_y: false,
            },
            turn: 0,
            owners: Vec::new(),
            rev: 0,
            classes: Vec::new(),
            ice: -1,
            humans: 0,
        }
    }

    /// A (re)started worker: the shape and turn it reports; owners and classes are forgotten until it sends them.
    pub fn restart(&mut self, shape: MapShape, turn: i32) {
        self.shape = shape;
        self.turn = turn;
        self.owners = Vec::new();
        self.rev = 0;
        self.classes = Vec::new();
    }

    pub fn set_turn(&mut self, turn: i32) {
        self.turn = turn;
    }

    /// The catalog id of `FEATURE_ICE` (`-1`: the game has no such feature).
    pub fn set_ice(&mut self, feature: i16) {
        self.ice = feature;
    }

    /// The players with a human seat, as a bit mask (bit `p` for player `p`).
    pub fn set_human_mask(&mut self, mask: u64) {
        self.humans = mask;
    }

    /// True when the owners are known (a `Full` was applied and no `Delta` was lost since).
    pub fn has_owners(&self) -> bool {
        !self.owners.is_empty()
    }

    /// Revision of the owners applied last (tests and logs).
    pub fn owners_rev(&self) -> u32 {
        self.rev
    }

    /// Builds the classes from the whole basis. A basis of another size than the map is ignored.
    pub fn set_basis(&mut self, basis: &MapBasis) {
        if basis.plots.len() != self.shape.plot_count() as usize {
            return;
        }
        let ice = self.ice;
        self.classes.clear();
        self.classes
            .extend(basis.plots.iter().map(|b| class_of(ice, b)));
    }

    /// Patches the classes with the records of a basis diff.
    pub fn apply_basis(&mut self, changes: &[(u32, PlotBasisView)]) {
        let ice = self.ice;
        for (i, b) in changes {
            if let Some(slot) = self.classes.get_mut(*i as usize) {
                *slot = class_of(ice, b);
            }
        }
    }

    /// Applies a message of the worker. A `Full` replaces the table; a `Delta` must follow the revision held.
    pub fn apply_owners(&mut self, u: &OwnersUpdate) -> Applied {
        match u {
            OwnersUpdate::Full { rev, turn, owners } => {
                if owners.len() != self.shape.plot_count() as usize || owners.is_empty() {
                    return Applied::Bad;
                }
                self.owners.clear();
                self.owners.extend_from_slice(owners);
                self.rev = *rev;
                self.turn = *turn;
                Applied::Ok
            }
            OwnersUpdate::Delta {
                base_rev,
                rev,
                changes,
            } => {
                if self.owners.is_empty() || *base_rev != self.rev {
                    self.owners.clear();
                    return Applied::Gap;
                }
                if changes
                    .iter()
                    .any(|(i, _)| *i as usize >= self.owners.len())
                {
                    self.owners.clear();
                    return Applied::Bad;
                }
                for (i, o) in changes {
                    self.owners[*i as usize] = *o;
                }
                self.rev = *rev;
                Applied::Ok
            }
        }
    }

    fn is_human(&self, p: PlayerId) -> bool {
        (0..64).contains(&p) && self.humans & (1u64 << p) != 0
    }
}

fn class_of(ice: i16, b: &PlotBasisView) -> PlotClass {
    if ice >= 0 && b.feature == ice {
        return PlotClass::Ice;
    }
    match b.plot_type {
        PlotType::Land => PlotClass::Land,
        PlotType::Hill => PlotClass::Hills,
        PlotType::Mountain => PlotClass::Mountain,
        PlotType::Water if b.flags.0 & PlotFlags::IS_LAKE != 0 => PlotClass::Lake,
        PlotType::Water => PlotClass::Water,
    }
}

impl WorldFacts for WorldTable {
    fn map(&self) -> MapShape {
        self.shape
    }
    fn turn(&self) -> i32 {
        self.turn
    }
    fn plot_at(&self, x: i16, y: i16) -> Option<PlotIdx> {
        self.shape.index(x, y)
    }
    fn owner(&self, p: PlotIdx) -> Owner {
        if self.owners.is_empty() {
            return Owner::Unknown;
        }
        match self.owners.get(p as usize) {
            Some(&o) if o >= 0 && self.is_human(o) => Owner::Seat(Seat(o)),
            Some(&o) if o >= 0 => Owner::Npc(o),
            _ => Owner::None,
        }
    }
    fn class(&self, p: PlotIdx) -> PlotClass {
        self.classes
            .get(p as usize)
            .copied()
            .unwrap_or(PlotClass::Water)
    }
    fn seat_player(&self, seat: Seat) -> Option<PlayerId> {
        self.is_human(seat.player()).then_some(seat.player())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape() -> MapShape {
        MapShape {
            width: 4,
            height: 3,
            wrap_x: true,
            wrap_y: false,
        }
    }

    fn table() -> WorldTable {
        let mut t = WorldTable::new();
        t.restart(shape(), 5);
        t.set_human_mask(0b11);
        t
    }

    fn full(rev: u32, owners: &[i8]) -> OwnersUpdate {
        OwnersUpdate::Full {
            rev,
            turn: 7,
            owners: owners.to_vec(),
        }
    }

    #[test]
    fn unknown_until_a_full_then_the_truth() {
        let mut t = table();
        assert_eq!(t.owner(0), Owner::Unknown);
        assert_eq!(t.turn(), 5);
        let mut o = vec![-1i8; 12];
        o[1] = 0;
        o[2] = 5;
        assert_eq!(t.apply_owners(&full(1, &o)), Applied::Ok);
        assert_eq!(t.turn(), 7, "a Full carries the turn");
        assert_eq!(t.owner(0), Owner::None);
        assert_eq!(t.owner(1), Owner::Seat(Seat(0)));
        assert_eq!(t.owner(2), Owner::Npc(5));
        assert_eq!(t.owner(99), Owner::None, "off the map");
        assert_eq!(t.seat_player(Seat(1)), Some(1));
        assert_eq!(t.seat_player(Seat(2)), None);
        assert_eq!(t.plot_at(-1, 0), Some(3), "wraps in x");
        assert_eq!(t.plot_at(0, 3), None);
        assert_eq!(t.plot_xy(5), (1, 1));
    }

    #[test]
    fn a_delta_needs_the_revision_it_follows() {
        let mut t = table();
        t.apply_owners(&full(4, &[-1; 12]));
        let d = |base_rev, rev, i, o| OwnersUpdate::Delta {
            base_rev,
            rev,
            changes: vec![(i, o)],
        };
        assert_eq!(t.apply_owners(&d(4, 5, 3, 1)), Applied::Ok);
        assert_eq!(t.owner(3), Owner::Seat(Seat(1)));
        assert_eq!(t.owners_rev(), 5);
        // A lost revision: unknown until the next Full.
        assert_eq!(t.apply_owners(&d(7, 8, 3, 0)), Applied::Gap);
        assert_eq!(t.owner(3), Owner::Unknown);
        assert_eq!(t.apply_owners(&d(8, 9, 3, 0)), Applied::Gap);
        assert_eq!(t.apply_owners(&full(10, &[-1; 12])), Applied::Ok);
        assert_eq!(t.owner(3), Owner::None);
    }

    #[test]
    fn a_table_of_the_wrong_size_is_refused() {
        let mut t = table();
        assert_eq!(t.apply_owners(&full(1, &[-1; 11])), Applied::Bad);
        assert!(!t.has_owners());
        t.apply_owners(&full(1, &[-1; 12]));
        let bad = OwnersUpdate::Delta {
            base_rev: 1,
            rev: 2,
            changes: vec![(12, 0)],
        };
        assert_eq!(t.apply_owners(&bad), Applied::Bad);
        assert_eq!(t.owner(0), Owner::Unknown, "a bad delta drops the table");
    }

    fn rec(plot_type: PlotType, lake: bool, feature: i16) -> PlotBasisView {
        PlotBasisView {
            plot_type,
            feature,
            flags: PlotFlags(if lake { PlotFlags::IS_LAKE } else { 0 }),
            ..PlotBasisView::default()
        }
    }

    #[test]
    fn classes_come_from_the_basis_and_follow_its_diffs() {
        let mut t = table();
        t.set_ice(9);
        let mut plots = vec![rec(PlotType::Land, false, -1); 12];
        plots[1] = rec(PlotType::Hill, false, -1);
        plots[2] = rec(PlotType::Mountain, false, -1);
        plots[3] = rec(PlotType::Water, false, -1);
        plots[4] = rec(PlotType::Water, true, -1);
        plots[5] = rec(PlotType::Water, false, 9);
        plots[6] = rec(PlotType::Land, false, 3);
        let basis = MapBasis {
            width: 4,
            height: 3,
            plots,
            ..MapBasis::default()
        };
        t.set_basis(&basis);
        let c: Vec<PlotClass> = (0..7).map(|i| t.class(i)).collect();
        use PlotClass::*;
        assert_eq!(c, [Land, Hills, Mountain, Water, Lake, Ice, Land]);
        assert_eq!(t.class(40), Water, "off the map");
        t.apply_basis(&[(0, rec(PlotType::Land, false, 9))]);
        assert_eq!(t.class(0), Ice);
        // A basis of another size changes nothing.
        t.set_basis(&MapBasis::default());
        assert_eq!(t.class(0), Ice);
    }
}
