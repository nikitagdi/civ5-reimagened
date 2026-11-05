//! `ViewChange`: what changed in the view since the last `take_change` (net-client 4.10). Section numbers
//! come from `ViewDiff::touched` (NP 4.6), so a new section needs no change here.

use std::collections::BTreeSet;

use protocol::{CityKey, SectionMask, UnitKey, ViewDiff};

/// Number of sections a keyframe replaces (0 header .. 9 trade routes). A new section adds one here.
pub const SECTION_COUNT: u8 = 10;

/// Plot indexes that changed, without repeats: `None` means all of them.
pub type DirtyPlots = Option<Vec<u32>>;

/// Keys of records that were inserted or replaced and keys that were removed, without repeats. A key is
/// in one list only.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyChanges<K> {
    pub upsert: Vec<K>,
    pub remove: Vec<K>,
}

/// `None` means all of them.
pub type DirtyKeys<K> = Option<KeyChanges<K>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewChange {
    /// Bit n: section n changed (0 header, 1 plots, 2 units, 3 cities, 4 players, 5 top bar,
    /// 6 notifications, 7 peace offers, 8 map basis, 9 trade routes).
    pub sections: SectionMask,
    pub dirty_plots: DirtyPlots,
    /// The same for the map basis: `None` means a new basis (`MapBasis`), `Some` the records of a `basis` diff.
    pub basis_plots: DirtyPlots,
    pub dirty_units: DirtyKeys<UnitKey>,
    pub dirty_cities: DirtyKeys<CityKey>,
    /// A new keyframe or a new game: everything is replaced.
    pub reset: bool,
    /// Only the optimistic layer changed (NC6).
    pub overlay: bool,
}

impl Default for ViewChange {
    /// Nothing changed.
    fn default() -> Self {
        ViewChange {
            sections: SectionMask(0),
            dirty_plots: Some(Vec::new()),
            basis_plots: Some(Vec::new()),
            dirty_units: Some(KeyChanges::default()),
            dirty_cities: Some(KeyChanges::default()),
            reset: false,
            overlay: false,
        }
    }
}

impl ViewChange {
    /// A keyframe: every section, every plot, every record. The basis is left as it was; a new basis
    /// says so itself (`basis_plots = None`).
    pub fn all() -> Self {
        ViewChange {
            sections: SectionMask((1 << SECTION_COUNT) - 1),
            dirty_plots: None,
            dirty_units: None,
            dirty_cities: None,
            reset: true,
            ..ViewChange::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == ViewChange::default()
    }
}

/// Keys with the upsert / remove rule: the last word on a key wins and a key is in one list only.
#[derive(Debug, Clone)]
enum Keys<K: Ord + Copy> {
    All,
    Some {
        upsert: BTreeSet<K>,
        remove: BTreeSet<K>,
    },
}

impl<K: Ord + Copy> Keys<K> {
    fn none() -> Self {
        Keys::Some {
            upsert: BTreeSet::new(),
            remove: BTreeSet::new(),
        }
    }

    fn add(&mut self, upsert: impl IntoIterator<Item = K>, remove: impl IntoIterator<Item = K>) {
        if let Keys::Some {
            upsert: u,
            remove: r,
        } = self
        {
            for k in upsert {
                r.remove(&k);
                u.insert(k);
            }
            for k in remove {
                u.remove(&k);
                r.insert(k);
            }
        }
    }

    fn take(&mut self) -> DirtyKeys<K> {
        match std::mem::replace(self, Keys::none()) {
            Keys::All => None,
            Keys::Some { upsert, remove } => Some(KeyChanges {
                upsert: upsert.into_iter().collect(),
                remove: remove.into_iter().collect(),
            }),
        }
    }
}

#[derive(Debug, Clone)]
enum Plots {
    All,
    Some(BTreeSet<u32>),
}

impl Plots {
    fn add(&mut self, idx: impl IntoIterator<Item = u32>) {
        if let Plots::Some(s) = self {
            s.extend(idx);
        }
    }

    fn take(&mut self) -> DirtyPlots {
        match std::mem::replace(self, Plots::Some(BTreeSet::new())) {
            Plots::All => None,
            Plots::Some(s) => Some(s.into_iter().collect()),
        }
    }
}

/// Collects changes between two `take_change` calls.
#[derive(Debug, Clone)]
pub(crate) struct ChangeAcc {
    sections: u32,
    plots: Plots,
    basis: Plots,
    units: Keys<UnitKey>,
    cities: Keys<CityKey>,
    reset: bool,
    overlay: bool,
}

impl ChangeAcc {
    pub fn new() -> Self {
        ChangeAcc {
            sections: 0,
            plots: Plots::Some(BTreeSet::new()),
            basis: Plots::Some(BTreeSet::new()),
            units: Keys::none(),
            cities: Keys::none(),
            reset: false,
            overlay: false,
        }
    }

    /// A keyframe or a new game.
    pub fn all(&mut self) {
        self.sections = (1 << SECTION_COUNT) - 1;
        self.plots = Plots::All;
        self.units = Keys::All;
        self.cities = Keys::All;
        self.reset = true;
    }

    /// A new map basis arrived (`MapBasis`).
    pub fn basis_all(&mut self) {
        self.sections |= 1 << 8;
        self.basis = Plots::All;
    }

    /// An applied diff.
    pub fn diff(&mut self, d: &ViewDiff) {
        let touched = d.touched();
        if touched.0 == 0 {
            return;
        }
        self.sections |= touched.0;
        self.overlay = false;
        self.plots.add(d.plots.iter().map(|(i, _)| *i));
        self.basis.add(d.basis.iter().map(|(i, _)| *i));
        self.units.add(
            d.units.upsert.iter().map(|u| (u.owner, u.id)),
            d.units.remove.iter().copied(),
        );
        self.cities.add(
            d.cities.upsert.iter().map(|c| (c.owner, c.id)),
            d.cities.remove.iter().copied(),
        );
    }

    /// The optimistic layer changed records: the header (the blocker), the units and the cities it touches.
    /// Only the layer changed if nothing else is waiting to be taken (`overlay`, 4.10).
    pub fn layer(&mut self, units: &[UnitKey], cities: &[CityKey]) {
        let only_layer = self.sections == 0 && !self.reset;
        self.sections |= 1 << 0;
        if !units.is_empty() {
            self.sections |= 1 << 2;
            self.units.add(units.iter().copied(), []);
        }
        if !cities.is_empty() {
            self.sections |= 1 << 3;
            self.cities.add(cities.iter().copied(), []);
        }
        if only_layer {
            self.overlay = true;
        }
    }

    pub fn take(&mut self) -> ViewChange {
        let c = ViewChange {
            sections: SectionMask(self.sections),
            dirty_plots: self.plots.take(),
            basis_plots: self.basis.take(),
            dirty_units: self.units.take(),
            dirty_cities: self.cities.take(),
            reset: self.reset,
            overlay: self.overlay,
        };
        self.sections = 0;
        self.reset = false;
        self.overlay = false;
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{CityView, PlotView, SectionDiff, UnitView};

    fn unit(owner: i8, id: i32) -> UnitView {
        UnitView {
            owner,
            id,
            ..Default::default()
        }
    }

    fn city(owner: i8, id: i32) -> CityView {
        CityView {
            owner,
            id,
            ..Default::default()
        }
    }

    #[test]
    fn nothing_changed_is_the_default_and_take_resets() {
        let mut a = ChangeAcc::new();
        assert!(a.take().is_empty());
        a.all();
        let c = a.take();
        assert!(c.reset && c.dirty_plots.is_none() && c.dirty_units.is_none());
        assert_eq!(c.sections, SectionMask(0x3FF));
        assert_eq!(
            c.basis_plots,
            Some(vec![]),
            "a keyframe alone does not replace the basis"
        );
        assert!(a.take().is_empty());
    }

    #[test]
    fn diffs_collect_sections_plots_and_keys_without_repeats() {
        let mut a = ChangeAcc::new();
        let d = ViewDiff {
            plots: vec![(5, PlotView::default()), (9, PlotView::default())],
            units: SectionDiff {
                upsert: vec![unit(0, 1), unit(0, 2)],
                remove: vec![],
            },
            cities: SectionDiff {
                upsert: vec![],
                remove: vec![(1, 7)],
            },
            ..Default::default()
        };
        a.diff(&d);
        let d2 = ViewDiff {
            plots: vec![(5, PlotView::default()), (2, PlotView::default())],
            units: SectionDiff {
                upsert: vec![unit(0, 3)],
                remove: vec![(0, 2)],
            },
            cities: SectionDiff {
                upsert: vec![city(1, 7)],
                remove: vec![],
            },
            ..Default::default()
        };
        a.diff(&d2);
        let c = a.take();
        assert_eq!(c.dirty_plots, Some(vec![2, 5, 9]));
        // Upserted then removed: removed. Removed then upserted again: upserted.
        assert_eq!(
            c.dirty_units,
            Some(KeyChanges {
                upsert: vec![(0, 1), (0, 3)],
                remove: vec![(0, 2)]
            })
        );
        assert_eq!(
            c.dirty_cities,
            Some(KeyChanges {
                upsert: vec![(1, 7)],
                remove: vec![]
            })
        );
        assert_eq!(c.sections, SectionMask(1 << 1 | 1 << 2 | 1 << 3));
        assert!(!c.reset && !c.overlay);
    }

    #[test]
    fn a_keyframe_swallows_later_diffs_until_taken() {
        let mut a = ChangeAcc::new();
        a.all();
        let d = ViewDiff {
            plots: vec![(1, PlotView::default())],
            units: SectionDiff {
                upsert: vec![unit(0, 1)],
                remove: vec![],
            },
            ..Default::default()
        };
        a.diff(&d);
        let c = a.take();
        assert!(c.reset && c.dirty_plots.is_none() && c.dirty_units.is_none());
    }

    #[test]
    fn the_layer_marks_header_units_and_cities_and_says_it_alone_changed() {
        let mut a = ChangeAcc::new();
        a.layer(&[(1, 4)], &[(1, 2)]);
        let c = a.take();
        assert_eq!(c.sections, SectionMask(1 | 1 << 2 | 1 << 3));
        assert!(c.overlay && !c.reset);
        assert_eq!(c.dirty_units.unwrap().upsert, [(1, 4)]);
        assert_eq!(c.dirty_cities.unwrap().upsert, [(1, 2)]);
        assert_eq!(
            c.dirty_plots,
            Some(vec![]),
            "the layer never touches plots or the basis"
        );
        assert_eq!(c.basis_plots, Some(vec![]));
        // A header-only record (the end of the turn).
        a.layer(&[], &[]);
        let c = a.take();
        assert_eq!(c.sections, SectionMask(1));
        assert!(c.overlay);
        // With a diff waiting, the change is not the layer's alone; the keys still join.
        let d = ViewDiff {
            plots: vec![(1, PlotView::default())],
            ..Default::default()
        };
        a.diff(&d);
        a.layer(&[(1, 4)], &[]);
        let c = a.take();
        assert!(!c.overlay);
        assert_eq!(c.dirty_plots, Some(vec![1]));
        assert_eq!(c.dirty_units.unwrap().upsert, [(1, 4)]);
        // After a keyframe everything is dirty and stays so.
        a.all();
        a.layer(&[(1, 4)], &[]);
        let c = a.take();
        assert!(c.reset && !c.overlay && c.dirty_units.is_none());
    }

    #[test]
    fn the_basis_has_its_own_list() {
        let mut a = ChangeAcc::new();
        let d = ViewDiff {
            basis: vec![(3, Default::default()), (1, Default::default())],
            ..Default::default()
        };
        a.diff(&d);
        let c = a.take();
        assert_eq!(c.basis_plots, Some(vec![1, 3]));
        assert_eq!(c.dirty_plots, Some(vec![]));
        assert!(c.sections.contains(8) && !c.sections.contains(1));
        a.basis_all();
        let c = a.take();
        assert_eq!(c.basis_plots, None);
        assert!(!c.reset, "a new basis is not a reset of the view");
    }
}
