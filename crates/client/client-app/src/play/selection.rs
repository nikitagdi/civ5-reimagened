//! Which unit a click picks and which unit comes next. Pure functions of the
//! world.

use protocol::{CityView, UnitFlags, UnitView, WorldView};

/// The human's units on a plot: military first, then civilians, each by id.
pub fn own_units_at(world: &WorldView, x: i16, y: i16) -> Vec<&UnitView> {
    let mut units: Vec<&UnitView> = world
        .units
        .iter()
        .filter(|u| u.owner == world.human && u.x == x && u.y == y)
        .collect();
    units.sort_by_key(|u| (u.flags.contains(UnitFlags::CIVILIAN), u.id));
    units
}

pub fn own_unit(world: &WorldView, id: i32) -> Option<&UnitView> {
    world
        .units
        .iter()
        .find(|u| u.id == id && u.owner == world.human)
}

pub fn city_at(world: &WorldView, x: i16, y: i16) -> Option<&CityView> {
    world.cities.iter().find(|c| c.x == x && c.y == y)
}

/// What a left click on a plot with own units does, given the current selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    Unit(i32),
    /// Past the last unit of a plot with an own city.
    City(i32),
}

/// Stack cycling: no selection on the plot picks the first unit; each further click the next one;
/// after the last one the city (if own and its window is not open yet), else the first again.
pub fn cycle(
    stack: &[&UnitView],
    selected: Option<i32>,
    own_city: Option<i32>,
    city_window: Option<i32>,
) -> Option<Pick> {
    let first = stack.first()?;
    let Some(i) = selected.and_then(|s| stack.iter().position(|u| u.id == s)) else {
        return Some(Pick::Unit(first.id));
    };
    if let Some(next) = stack.get(i + 1) {
        return Some(Pick::Unit(next.id));
    }
    match own_city {
        Some(c) if city_window != Some(c) => Some(Pick::City(c)),
        _ => Some(Pick::Unit(first.id)),
    }
}

/// The human's next unit with `NEEDS_ORDERS` after `after` in ascending id, wrapping around; never
/// `after` itself.
pub fn next_needing_orders(world: &WorldView, after: Option<i32>) -> Option<i32> {
    let mut ids: Vec<i32> = world
        .units
        .iter()
        .filter(|u| u.owner == world.human && u.flags.contains(UnitFlags::NEEDS_ORDERS))
        .map(|u| u.id)
        .filter(|id| Some(*id) != after)
        .collect();
    ids.sort_unstable();
    let from = after.unwrap_or(i32::MIN);
    ids.iter()
        .copied()
        .find(|id| *id > from)
        .or(ids.first().copied())
}
