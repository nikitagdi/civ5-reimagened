//! What the routes ask of the model the player sees, and how keys of the model become the numbers of the orders.
//! Nothing here reads the game: a missing root or entry makes the answer "no".

use ui2_model::generated::{Game, Map, RefuseWhy, Unit, UnitActionKind, UnitFlag};
use ui2_model::{Key, ModelStore};

/// The id of a unit or city inside its key (`(owner << 32) | id`), as the orders carry it.
pub(crate) fn entity_id(key: Key) -> i32 {
    key.0 as u32 as i32
}

/// The turn of the model: what every order is stamped with.
pub(crate) fn game_turn(model: &ModelStore) -> Option<i32> {
    model.get::<Game>().map(|g| g.turn)
}

/// A unit of the player's on the map.
pub(crate) fn own_unit(model: &ModelStore, unit: Key) -> Option<&UnitFlag> {
    model.get::<Map>()?.units.get(unit).filter(|u| u.mine)
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
