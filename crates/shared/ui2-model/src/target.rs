//! What an order waiting for its answer is about: the `target` text of a `ui.pending` row.
//!
//! The actions module writes `ui.pending[]` when it sends an order; the source marks the selected unit
//! (`unit.pending`) and a production option (`Opt.pending`) by looking a target up in those rows. Both sides spell
//! a target with these functions, so the two can never disagree. Keys are written in their JS form
//! (`<owner>:<id>`, see [`Key::to_js`]).

use crate::Key;

/// The target of the turn's own orders (`turn.end`, `turn.cancel`).
pub const TURN_TARGET: &str = "turn";

/// The target of an order given to a unit: `unit/<owner>:<id>`.
#[must_use]
pub fn unit_target(unit: Key) -> String {
    format!("unit/{unit}")
}

/// The target of a production order (set, queue, replace, buy) of one item in one city:
/// `city/<owner>:<id>/<item key>`.
#[must_use]
pub fn option_target(city: Key, item: Key) -> String {
    format!("city/{city}/{item}")
}

/// The target of an order about a city's queue as a whole (remove or move an entry): `city/<owner>:<id>`.
#[must_use]
pub fn city_target(city: Key) -> String {
    format!("city/{city}")
}

/// The target of a research order: `tech/<tech key>`.
#[must_use]
pub fn tech_target(tech: Key) -> String {
    format!("tech/{tech}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_are_spelled_with_the_js_keys() {
        assert_eq!(unit_target(Key::new(0, 7)), "unit/0:7");
        assert_eq!(unit_target(Key::new(3, 12)), "unit/3:12");
        assert_eq!(
            option_target(Key::new(0, 1), Key::from_u32((1 << 16) | 40)),
            "city/0:1/0:65576"
        );
        assert_eq!(city_target(Key::new(0, 20)), "city/0:20");
        assert_eq!(tech_target(Key::from_u32(6)), "tech/0:6");
        assert_eq!(TURN_TARGET, "turn");
    }
}
