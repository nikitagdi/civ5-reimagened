//! What an order waiting for its answer is about: the `target` text of a `ui.pending` row.
//!
//! The actions module writes `ui.pending[]` when it sends an order; this source marks the selected unit
//! (`unit.pending`) and a production option (`Opt.pending`) from it. Both sides must spell a target the same way;
//! these functions are that spelling.

use ui2_model::Key;
use ui2_model::r#gen::Pending;

/// The target of an order given to a unit: `unit/<owner>:<id>`.
#[must_use]
pub fn unit_target(unit: Key) -> String {
    format!("unit/{unit}")
}

/// The target of a production order (set, queue, buy) of one item in one city: `city/<owner>:<id>/<item key>`.
#[must_use]
pub fn option_target(city: Key, item: Key) -> String {
    format!("city/{city}/{item}")
}

/// An order with this target waits for its answer.
#[must_use]
pub fn waits(pending: &[Pending], target: &str) -> bool {
    pending.iter().any(|p| &*p.target == target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_are_spelled_with_the_js_keys() {
        assert_eq!(unit_target(Key::new(0, 7)), "unit/0:7");
        assert_eq!(
            option_target(Key::new(0, 1), Key::from_u32((1 << 16) | 40)),
            "city/0:1/0:65576"
        );
    }
}
