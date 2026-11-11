//! Builders of the model roots: plain functions from core records and session facts to generated root structs.

pub mod chat;
pub mod game;
pub mod map;
pub mod notes;
pub mod players;
pub mod production;
pub mod top;
pub mod turn;
pub mod unit;

use replica::OrderType;
use ui2_model::Key;

use crate::catalog::Catalog;
use crate::session::SessionRead;

/// What a builder may look at besides the game: the session, the frame's time, the catalog.
pub(crate) struct Cx<'a> {
    pub session: &'a dyn SessionRead,
    pub now_ms: u64,
    pub catalog: &'a Catalog,
}

/// `part / whole` rounded to 1/1000; 0 when `whole` is not positive.
#[must_use]
pub fn fraction(part: i32, whole: i32) -> f32 {
    if whole <= 0 {
        return 0.0;
    }
    let f = f64::from(part) / f64::from(whole);
    ((f * 1000.0).round() / 1000.0) as f32
}

/// `key<unit>` / `key<city>`: `(owner << 32) | id`. The core's owner is a small player id (barbarians 63).
#[must_use]
pub fn entity_key(owner: i8, id: i32) -> Key {
    Key::new(u32::from(owner.cast_unsigned()), id.cast_unsigned())
}

/// `key<player>`: the player id.
#[must_use]
pub fn player_key(id: i8) -> Key {
    Key::from_i32(i32::from(id))
}

/// `key<item>`: `(order << 16) | item`, the order being the core's `OrderTypes` number.
#[must_use]
pub fn item_key(order: OrderType, item: i16) -> Key {
    Key::from_u32((order.abi().cast_unsigned() << 16) | u32::from(item.cast_unsigned()))
}

#[cfg(test)]
mod tests {
    use super::{entity_key, fraction};
    use ui2_model::Key;

    #[test]
    fn fraction_rounds_to_thousandths() {
        assert_eq!(fraction(310, 500), 0.62);
        assert_eq!(fraction(1, 3), 0.333);
        assert_eq!(fraction(2, 3), 0.667);
        assert_eq!(fraction(5, 0), 0.0);
        assert_eq!(fraction(5, -1), 0.0);
    }

    #[test]
    fn entity_keys_put_the_owner_high() {
        assert_eq!(entity_key(2, 7), Key::new(2, 7));
        assert_eq!(entity_key(63, 1).to_js(), "63:1");
    }
}
