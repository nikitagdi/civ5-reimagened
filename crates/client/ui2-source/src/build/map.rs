//! The root `map`: the city banners and unit flags the HUD layer draws over the map.

use std::sync::Arc;

use replica::{City, Player, ReadError, Unit};
use ui2_model::r#gen::{BannerProd, CityBanner, Domain, Map, Plot, UnitFlag};

use crate::CountingRead;
use crate::build::players::find;
use crate::build::turn::waiting;
use crate::build::{entity_key, fraction, item_key, player_key};
use crate::catalog::Catalog;

/// `DomainTypes` of the core.
#[must_use]
pub fn domain(v: i8) -> Domain {
    match v {
        0 => Domain::Sea,
        1 => Domain::Air,
        2 => Domain::Land,
        3 => Domain::Immobile,
        4 => Domain::Hover,
        _ => Domain::Other,
    }
}

/// The owner's two colours, black when the owner is not among the players.
fn colors(players: &[Player], owner: i8) -> (u32, u32) {
    find(players, owner).map_or((0x0000_00ff, 0x0000_00ff), |p| {
        (p.color_primary, p.color_secondary)
    })
}

/// One city banner. A foreign city the seat does not see now (fog) shows only its owner, name and plot: the core
/// sends nothing else for it.
#[must_use]
pub fn banner(c: &City, seat: u8, players: &[Player], catalog: &Catalog) -> CityBanner {
    let (color, color2) = colors(players, c.owner);
    let mine = c.owner.cast_unsigned() == seat;
    let mut b = CityBanner {
        key: entity_key(c.owner, c.id),
        owner: player_key(c.owner),
        color,
        color2,
        name: Arc::from(c.name.as_str()),
        plot: Plot { x: c.x, y: c.y },
        mine,
        visible: c.flags.visible(),
        ..CityBanner::default()
    };
    if !mine && !c.flags.visible() {
        return b;
    }
    b.size = c.population;
    b.capital = c.flags.capital();
    b.puppet = c.flags.puppet();
    b.razing = c.flags.razing();
    b.occupied = c.flags.occupied();
    b.resistance = c.flags.resistance();
    b.can_shoot = c.flags.can_range_attack();
    b.strength = c.defense;
    b.hp_fraction = fraction(i32::from(c.hp), i32::from(c.max_hp));
    if mine {
        b.growth_turns = c.growth_turns;
        b.production = match (c.production_order, c.production_item) {
            (Some(order), Some(item)) => {
                let it = catalog.order_item(order, item);
                Some(BannerProd {
                    item: item_key(order, item),
                    name: it.name,
                    icon: it.icon,
                    turns: c.production_turns,
                    fraction: fraction(c.production_stored, c.production_needed),
                })
            }
            _ => None,
        };
    }
    b
}

/// One unit flag.
#[must_use]
pub fn flag(u: &Unit, seat: u8, players: &[Player], catalog: &Catalog) -> UnitFlag {
    let (color, color2) = colors(players, u.owner);
    let mine = u.owner.cast_unsigned() == seat;
    UnitFlag {
        key: entity_key(u.owner, u.id),
        owner: player_key(u.owner),
        color,
        color2,
        plot: Plot { x: u.x, y: u.y },
        icon: catalog.unit(u.unit_type).icon,
        hp_fraction: fraction(i32::from(u.hp), i32::from(u.max_hp)),
        domain: domain(u.domain),
        mine,
        waiting: mine && waiting(u),
        at_war: !mine && find(players, u.owner).is_some_and(|p| p.flags.at_war_with_viewer()),
        cargo: u.flags.cargo(),
    }
}

/// `map` from `Q_CITIES`, `Q_UNITS` and `Q_PLAYERS` (the owners' colours, war).
pub(crate) fn map(read: &mut CountingRead<'_, '_>, catalog: &Catalog) -> Result<Map, ReadError> {
    let seat = read.seat();
    let cities = read.cities()?;
    let units = read.units()?;
    let players = read.players()?;
    let mut out = Map::default();
    for c in cities.iter() {
        // Keys are (owner, id) and unique in the core's reply; a repeated one keeps its first row.
        let _ = out.cities.try_push(banner(c, seat, &players, catalog));
    }
    for u in units.iter() {
        let _ = out.units.try_push(flag(u, seat, &players, catalog));
    }
    Ok(out)
}
