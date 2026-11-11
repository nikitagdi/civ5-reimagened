//! What the routes ask of the model the player sees, and how keys of the model become the numbers of the orders.
//! Nothing here reads the game: a missing root or entry makes the answer "no".

use ui2_model::generated::{
    CityBanner, Game, Map, Opt, Production, RefuseWhy, Research, Unit, UnitActionKind, UnitFlag,
};
use ui2_model::{Key, ModelStore};

/// The id of a unit or city inside its key (`(owner << 32) | id`), as the orders carry it.
pub(crate) fn entity_id(key: Key) -> i32 {
    key.0 as u32 as i32
}

/// The turn of the model: what every order is stamped with.
pub(crate) fn game_turn(model: &ModelStore) -> Option<i32> {
    model.get::<Game>().map(|g| g.turn)
}

/// The id of a technology inside its key.
pub(crate) fn tech_id(key: Key) -> Option<i16> {
    i16::try_from(key.0).ok()
}

/// An item of the production window as an order carries it: `key<item>` is `(order << 16) | item`, the order
/// type of the core and the id of the item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ItemRef {
    pub order: i8,
    pub item: i16,
}

impl ItemRef {
    pub(crate) fn from_key(key: Key) -> Option<ItemRef> {
        if key.0 > u64::from(u32::MAX) {
            return None;
        }
        Some(ItemRef {
            order: i8::try_from(key.0 >> 16).ok()?,
            item: i16::try_from(key.0 & 0xffff).ok()?,
        })
    }
}

/// A unit of the player's on the map.
pub(crate) fn own_unit(model: &ModelStore, unit: Key) -> Option<&UnitFlag> {
    model.get::<Map>()?.units.get(unit).filter(|u| u.mine)
}

/// A city of the player's.
pub(crate) fn own_city(model: &ModelStore, city: Key) -> Option<&CityBanner> {
    model.get::<Map>()?.cities.get(city).filter(|c| c.mine)
}

/// The model of the production window, if it is the window of this city.
pub(crate) fn production_of(model: &ModelStore, city: Key) -> Option<&Production> {
    model
        .get::<Option<Production>>()?
        .as_ref()
        .filter(|p| p.city == city)
}

/// An option of the production window, in whichever group it is.
pub(crate) fn option_of(p: &Production, item: Key) -> Option<&Opt> {
    p.groups.iter().find_map(|g| g.options.get(item))
}

/// The research model.
pub(crate) fn research(model: &ModelStore) -> Option<&Research> {
    model.get::<Research>()
}

/// A unit on the map, whoever's it is.
pub(crate) fn any_unit(model: &ModelStore, unit: Key) -> Option<&UnitFlag> {
    model.get::<Map>()?.units.get(unit)
}

/// May the action be ordered for the unit, as far as the model says. The model describes the selected unit
/// only; for another unit nothing is known and the server decides.
pub(crate) fn unit_action(
    model: &ModelStore,
    unit: Key,
    kind: UnitActionKind,
) -> Result<(), RefuseWhy> {
    let Some(Some(selected)) = model.get::<Option<Unit>>() else {
        return Ok(());
    };
    if selected.key != unit {
        return Ok(());
    }
    let Some(row) = selected.actions.get(Key::from_enum(kind as u16)) else {
        return Err(RefuseWhy::ActionUnavailable);
    };
    if !row.supported {
        return Err(RefuseWhy::Unsupported);
    }
    if !row.enabled {
        return Err(RefuseWhy::ActionUnavailable);
    }
    Ok(())
}
