//! What an order waiting for its answer is about: the `target` text of a `ui.pending` row.
//!
//! The actions module writes `ui.pending[]` when it sends an order; this source marks the selected unit
//! (`unit.pending`) and a production option (`Opt.pending`) from it. Both sides spell a target with the functions
//! of [`ui2_model::target`], re-exported here.

use ui2_model::r#gen::Pending;

pub use ui2_model::target::{option_target, unit_target};

/// An order with this target waits for its answer.
#[must_use]
pub fn waits(pending: &[Pending], target: &str) -> bool {
    pending.iter().any(|p| &*p.target == target)
}

#[cfg(test)]
mod tests {
    use ui2_model::Key;

    use super::*;

    /// The spelling this source looks up; the actions module's tests pin the same strings on the writing side.
    #[test]
    fn targets_are_spelled_with_the_js_keys() {
        assert_eq!(unit_target(Key::new(0, 7)), "unit/0:7");
        assert_eq!(
            option_target(Key::new(0, 1), Key::from_u32((1 << 16) | 40)),
            "city/0:1/0:65576"
        );
    }
}
