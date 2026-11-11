//! `STYL`: baked styles.
//!
//! Chrome decides the cascade; the package keeps the answers. A node has a `StyleSet`: a table of
//! records indexed by the viewport cell (breakpoints) and a mask of the node's dynamic conditions
//! (`:hover`, a class toggled by a binding, ...). The runtime only looks records up: it never
//! matches a selector (decision L33).

use serde::{Deserialize, Serialize};

use crate::container::SectionTag;
use crate::error::PackageError;
use crate::ids::{StrId, StyleId};
use crate::style::{Finite, StyleRecord};

/// Most dynamic conditions one node may depend on.
pub const MAX_CONDS: usize = 4;
/// Most viewport cells a skin may have.
pub const MAX_CELLS: usize = 16;

/// A breakpoint on one viewport axis. A value below `px` is in the lower cell; the value `px`
/// itself is in the lower cell when `upper_inclusive` and in the upper cell otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bound {
    /// The breakpoint, CSS px.
    pub px: f32,
    /// Whether the breakpoint value belongs to the lower cell (`<=` instead of `<`).
    pub upper_inclusive: bool,
}

/// What a condition tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CondKind {
    /// `:hover`.
    Hover,
    /// `:active`.
    Active,
    /// `:focus`.
    Focus,
    /// `:focus-visible`.
    FocusVisible,
    /// `:disabled`.
    Disabled,
    /// A class that a binding toggles (`data-bind-class-toggle`).
    Class(StrId),
}

/// One condition of a set: on the node itself (`up = 0`) or on its `up`-th ancestor element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cond {
    /// 0 for the node, k for the k-th ancestor element.
    pub up: u8,
    /// What is tested.
    pub kind: CondKind,
}

/// The records of one node: `table[cell * 2^n + mask]` where bit `i` of `mask` says that
/// `conds[i]` holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyleSet {
    /// At most `MAX_CONDS` conditions; the order is the bit order of the mask.
    pub conds: Vec<Cond>,
    /// `cells * 2^conds.len()` records.
    pub table: Vec<StyleId>,
}

impl StyleSet {
    /// A set with no conditions: one record per cell.
    pub fn plain(table: Vec<StyleId>) -> Self {
        Self {
            conds: Vec::new(),
            table,
        }
    }

    /// The record for a viewport cell and a condition mask. The indices were checked when the
    /// package was read; out of range still never panics (it answers the first record).
    pub fn get(&self, cell: u8, mask: u8) -> StyleId {
        let idx = (usize::from(cell) << self.conds.len()) | usize::from(mask);
        self.table
            .get(idx)
            .or(self.table.first())
            .copied()
            .unwrap_or(StyleId(0))
    }
}

/// The section.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Styles {
    /// Breakpoints on the viewport width, ascending.
    pub media_w: Vec<Bound>,
    /// Breakpoints on the viewport height, ascending.
    pub media_h: Vec<Bound>,
    /// The distinct records, in order of first use.
    pub records: Vec<StyleRecord>,
    /// The sets, in order of first use.
    pub sets: Vec<StyleSet>,
}

fn axis_index(bounds: &[Bound], v: f32) -> usize {
    bounds
        .iter()
        .filter(|b| {
            if b.upper_inclusive {
                v > b.px
            } else {
                v >= b.px
            }
        })
        .count()
}

impl Styles {
    /// The number of viewport cells: `(w bounds + 1) * (h bounds + 1)`.
    pub fn cells(&self) -> usize {
        (self.media_w.len() + 1) * (self.media_h.len() + 1)
    }

    /// The cell of a logical viewport in CSS px: `wi * (h bounds + 1) + hi`.
    pub fn cell(&self, viewport_css: [f32; 2]) -> u8 {
        let wi = axis_index(&self.media_w, viewport_css[0]);
        let hi = axis_index(&self.media_h, viewport_css[1]);
        (wi * (self.media_h.len() + 1) + hi) as u8
    }

    /// A record by id.
    pub fn record(&self, id: StyleId) -> Option<&StyleRecord> {
        self.records.get(id.index())
    }

    /// Checks cell and condition limits, table lengths, indices and floats.
    pub(crate) fn validate(&self, at: SectionTag, strs: usize) -> Result<(), PackageError> {
        let bad = |what: String| Err(PackageError::invalid(at, what));
        for (axis, bounds) in [("width", &self.media_w), ("height", &self.media_h)] {
            if bounds.iter().any(|b| !b.px.is_finite()) {
                return bad(format!("a {axis} breakpoint is not finite"));
            }
            if bounds.windows(2).any(|w| w[0].px >= w[1].px) {
                return bad(format!("the {axis} breakpoints are not ascending"));
            }
        }
        let cells = self.cells();
        if cells > MAX_CELLS {
            return bad(format!("{cells} viewport cells, at most {MAX_CELLS}"));
        }
        if self.records.iter().any(|r| !r.is_finite()) {
            return bad("a style record holds a NaN or an infinity".into());
        }
        if self
            .records
            .iter()
            .any(|r| r.str_ids().any(|f| f.index() >= strs))
        {
            return bad("a font family or a picture is not in the string table".into());
        }
        if self.records.iter().any(|r| !r.transitions.finite()) {
            return bad("a transition holds a NaN or an infinity".into());
        }
        for (i, set) in self.sets.iter().enumerate() {
            if set.conds.len() > MAX_CONDS {
                return bad(format!(
                    "set {i}: {} conditions, at most {MAX_CONDS}",
                    set.conds.len()
                ));
            }
            if set.table.len() != cells << set.conds.len() {
                return bad(format!(
                    "set {i}: the table length is not cells * 2^conditions"
                ));
            }
            if set.table.iter().any(|s| s.index() >= self.records.len()) {
                return bad(format!("set {i}: a record index is out of range"));
            }
            if set
                .conds
                .iter()
                .any(|c| matches!(c.kind, CondKind::Class(s) if s.index() >= strs))
            {
                return bad(format!("set {i}: a class is not in the string table"));
            }
        }
        Ok(())
    }
}

#[cfg(feature = "write")]
impl crate::remap::Remapped for StyleRecord {
    fn remap(&mut self, r: &crate::remap::Remap) {
        for f in self.str_ids_mut() {
            r.str(f);
        }
    }
}

#[cfg(feature = "write")]
impl crate::remap::Remapped for StyleSet {
    fn remap(&mut self, r: &crate::remap::Remap) {
        for c in &mut self.conds {
            if let CondKind::Class(s) = &mut c.kind {
                r.str(s);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn styles(w: &[(f32, bool)], h: &[(f32, bool)]) -> Styles {
        let b = |v: &[(f32, bool)]| {
            v.iter()
                .map(|&(px, upper_inclusive)| Bound {
                    px,
                    upper_inclusive,
                })
                .collect()
        };
        Styles {
            media_w: b(w),
            media_h: b(h),
            ..Styles::default()
        }
    }

    #[test]
    fn cells_follow_the_breakpoints() {
        let s = styles(&[(1280.0, false), (1600.0, false)], &[(800.0, true)]);
        assert_eq!(s.cells(), 6);
        assert_eq!(s.cell([1000.0, 700.0]), 0);
        assert_eq!(s.cell([1000.0, 900.0]), 1);
        assert_eq!(s.cell([1280.0, 700.0]), 2); // `>=` bound: the value is in the upper cell
        assert_eq!(s.cell([1599.9, 800.0]), 2); // `<=` bound on height: 800 is in the lower cell
        assert_eq!(s.cell([1600.0, 800.1]), 5);
        assert_eq!(styles(&[], &[]).cell([1.0, 2.0]), 0);
    }

    #[test]
    fn get_indexes_cell_then_mask() {
        let set = StyleSet {
            conds: vec![
                Cond {
                    up: 0,
                    kind: CondKind::Hover,
                },
                Cond {
                    up: 1,
                    kind: CondKind::Active,
                },
            ],
            table: (0..8).map(StyleId).collect(),
        };
        assert_eq!(set.get(0, 0), StyleId(0));
        assert_eq!(set.get(0, 3), StyleId(3));
        assert_eq!(set.get(1, 2), StyleId(6));
        assert_eq!(set.get(200, 200), StyleId(0)); // out of range does not panic
    }

    #[test]
    fn limits_are_checked() {
        let at = SectionTag::STYL;
        let mut s = styles(
            &[(1.0, false), (2.0, false), (3.0, false)],
            &[(1.0, false), (2.0, false)],
        );
        assert_eq!(s.cells(), 12);
        assert!(s.validate(at, 0).is_ok());
        s.media_w.push(Bound {
            px: 4.0,
            upper_inclusive: false,
        });
        assert_eq!(s.cells(), 15);
        assert!(s.validate(at, 0).is_ok());
        s.media_w.push(Bound {
            px: 5.0,
            upper_inclusive: false,
        });
        assert_eq!(s.cells(), 18);
        assert!(s.validate(at, 0).is_err());
        let s = styles(&[(2.0, false), (1.0, false)], &[]);
        assert!(s.validate(at, 0).is_err());
        let s = styles(&[(f32::NAN, false)], &[]);
        assert!(s.validate(at, 0).is_err());
    }

    #[test]
    fn set_tables_must_match_cells_and_conditions() {
        let at = SectionTag::STYL;
        let mut s = styles(&[(100.0, false)], &[]);
        s.records.push(StyleRecord::initial());
        let hover = Cond {
            up: 0,
            kind: CondKind::Hover,
        };
        s.sets.push(StyleSet {
            conds: vec![hover],
            table: vec![StyleId(0); 4],
        });
        assert!(s.validate(at, 0).is_ok());
        s.sets[0].table.pop();
        assert!(s.validate(at, 0).is_err());
        s.sets[0].table = vec![StyleId(0); 2 << 5];
        s.sets[0].conds = vec![hover; 5];
        assert!(s.validate(at, 0).is_err()); // five conditions
        s.sets[0].conds = vec![hover];
        s.sets[0].table = vec![StyleId(0), StyleId(0), StyleId(0), StyleId(1)];
        assert!(s.validate(at, 0).is_err()); // record 1 does not exist
    }
}
