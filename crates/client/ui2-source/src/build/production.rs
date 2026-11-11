//! The root `production`: the production window of one of the seat's cities (`ui.windows.production`), with what
//! the city builds now, its queue, every option grouped by kind with turns, lock reasons and buy prices, and the
//! details of the focused option compared across the seat's cities.

use std::sync::Arc;

use replica::{BuildFlags, City, CityBuild, CityOrder, OrderType, ReadError};
use ui2_model::r#gen::{
    CityTurns, Detail, ItemKind, Opt, OptGroup, Pending, ProdNow, Production, QueueRow, WinCity,
};
use ui2_model::{Key, KeyedList};

use crate::CountingRead;
use crate::build::{entity_key, fraction, item_key};
use crate::catalog::Catalog;
use crate::pending;

/// The kind of an item: what the order builds, a limited building being a wonder. `None` for an order this source
/// does not know.
#[must_use]
pub fn item_kind(order: OrderType, flags: BuildFlags) -> Option<ItemKind> {
    match order {
        OrderType::Train => Some(ItemKind::Unit),
        OrderType::Construct if flags.wonder() => Some(ItemKind::Wonder),
        OrderType::Construct => Some(ItemKind::Building),
        OrderType::Create => Some(ItemKind::Project),
        OrderType::Maintain => Some(ItemKind::Process),
        OrderType::Other(_) => None,
    }
}

/// The groups in the order the window shows them.
pub const GROUPS: [ItemKind; 5] = [
    ItemKind::Unit,
    ItemKind::Building,
    ItemKind::Wonder,
    ItemKind::Project,
    ItemKind::Process,
];

/// The kind of an item the city may not offer any more (a queued item): from the city's options when it is
/// there, else from the order alone.
fn kind_of(builds: &[CityBuild], order: OrderType, item: i16) -> Option<ItemKind> {
    let flags = builds
        .iter()
        .find(|b| b.order == order && b.item == item)
        .map_or(BuildFlags::default(), |b| b.flags);
    item_kind(order, flags)
}

/// What the window needs besides the city's options.
pub struct Purse {
    pub gold: i32,
    pub faith: i32,
}

/// One option row.
#[must_use]
pub fn option(
    index: usize,
    b: &CityBuild,
    catalog: &Catalog,
    purse: &Purse,
    queued: bool,
    pending_now: bool,
) -> Opt {
    let it = catalog.order_item(b.order, b.item);
    Opt {
        item: item_key(b.order, b.item),
        order: i32::try_from(index).unwrap_or(i32::MAX),
        name: it.name,
        icon: it.icon,
        turns: b.turns,
        cost: b.production_cost,
        enabled: b.flags.enabled(),
        current: b.flags.current(),
        queued,
        locked_reason: b.reason.as_deref().map(Arc::from),
        // SIMPLIFIED(SIMP-239): the short effect, the help text, the maintenance and the requirements of an item
        // come from the rules tables of the asset pipeline, which the client does not have yet: they stay empty.
        effect: None,
        buy_gold: b.gold_cost,
        buy_gold_ok: b.flags.can_purchase() && b.gold_cost.is_some_and(|c| purse.gold >= c),
        buy_faith: b.faith_cost,
        buy_faith_ok: b.faith_cost.is_some_and(|c| purse.faith >= c),
        pending: pending_now,
    }
}

/// The options grouped by kind, each group in the core's order; empty groups are left out.
#[must_use]
pub fn groups(
    builds: &[CityBuild],
    catalog: &Catalog,
    purse: &Purse,
    queued: &[Key],
    city: Key,
    pending_rows: &[Pending],
) -> KeyedList<OptGroup> {
    let mut out = KeyedList::new();
    for kind in GROUPS {
        let mut options = KeyedList::new();
        for (i, b) in builds.iter().enumerate() {
            if item_kind(b.order, b.flags) != Some(kind) {
                continue;
            }
            let key = item_key(b.order, b.item);
            let waits = pending::waits(pending_rows, &pending::option_target(city, key));
            // The core lists an item once; a repeated one keeps its first row.
            let _ = options.try_push(option(i, b, catalog, purse, queued.contains(&key), waits));
        }
        if options.is_empty() {
            continue;
        }
        let _ = out.try_push(OptGroup {
            kind,
            count: i32::try_from(options.len()).unwrap_or(i32::MAX),
            options,
        });
    }
    out
}

/// What the city builds now.
#[must_use]
pub fn current(c: &City, builds: &[CityBuild], catalog: &Catalog) -> Option<ProdNow> {
    let (order, item) = (c.production_order?, c.production_item?);
    let it = catalog.order_item(order, item);
    Some(ProdNow {
        item: item_key(order, item),
        name: it.name,
        kind: kind_of(builds, order, item)?,
        stored: c.production_stored,
        needed: c.production_needed,
        turns: c.production_turns,
        fraction: fraction(c.production_stored, c.production_needed),
    })
}

/// The queue rows from `Q_CITY_QUEUE`, head first. The list is keyed by item: a second entry of the same item
/// (two units of one type) is not shown.
#[must_use]
pub fn queue(rows: &[CityOrder], builds: &[CityBuild], catalog: &Catalog) -> KeyedList<QueueRow> {
    let mut out = KeyedList::new();
    for r in rows {
        let Some(kind) = kind_of(builds, r.order, r.item) else {
            continue;
        };
        let it = catalog.order_item(r.order, r.item);
        let _ = out.try_push(QueueRow {
            slot: r.slot,
            item: item_key(r.order, r.item),
            name: it.name,
            kind,
            turns: r.turns,
            first: r.slot == 0,
        });
    }
    out
}

/// The queue when the core has no `Q_CITY_QUEUE`: only the head, from the city record.
#[must_use]
pub fn head_only(now: Option<&ProdNow>) -> KeyedList<QueueRow> {
    let mut out = KeyedList::new();
    if let Some(n) = now {
        let _ = out.try_push(QueueRow {
            slot: 0,
            item: n.item,
            name: n.name.clone(),
            kind: n.kind,
            turns: n.turns,
            first: true,
        });
    }
    out
}

/// The turns one item takes in each of the seat's cities that can build it now, with the share of the slowest.
#[must_use]
pub fn compare(per_city: &[(Key, Arc<str>, Option<i16>)]) -> KeyedList<CityTurns> {
    let max = per_city
        .iter()
        .filter_map(|(_, _, t)| *t)
        .max()
        .unwrap_or(0);
    let mut out = KeyedList::new();
    for (city, name, turns) in per_city {
        let _ = out.try_push(CityTurns {
            city: *city,
            name: name.clone(),
            turns: *turns,
            fraction: turns.map_or(0.0, |t| fraction(i32::from(t), i32::from(max))),
        });
    }
    out
}

/// The production window's root. A closed window, no city, or a city that is not the seat's: `None`, with no
/// read. Reads `Q_CITIES` and `Q_TOP_BAR` (kept), `Q_CITY_BUILDS` and `Q_CITY_QUEUE` of the city, and only with a
/// focused option `Q_CITY_BUILDS` of every other city of the seat.
pub(crate) fn production(
    read: &mut CountingRead<'_, '_>,
    catalog: &Catalog,
    window: &WinCity,
    focus: Option<Key>,
    pending_rows: &[Pending],
) -> Result<Option<Production>, ReadError> {
    let Some(key) = window.city.filter(|_| window.open) else {
        return Ok(None);
    };
    let seat = read.seat();
    let cities = read.cities()?;
    let Some(city) = cities
        .iter()
        .find(|c| c.owner.cast_unsigned() == seat && entity_key(c.owner, c.id) == key)
    else {
        return Ok(None);
    };
    let top = read.top_bar()?;
    let purse = Purse {
        gold: top.gold,
        faith: top.faith,
    };
    let builds = read.city_builds(city.id)?;
    let now = current(city, &builds, catalog);
    let (queue_known, queue) = match read.city_queue(city.id) {
        Ok(rows) => (true, queue(&rows, &builds, catalog)),
        Err(ReadError::Unsupported) => (false, head_only(now.as_ref())),
        Err(e) => return Err(e),
    };
    let queued: Vec<Key> = queue.iter().map(|r| r.item).collect();
    let groups = groups(&builds, catalog, &purse, &queued, key, pending_rows);

    let detail = match focus.and_then(|f| {
        builds
            .iter()
            .find(|b| item_key(b.order, b.item) == f)
            .cloned()
    }) {
        Some(b) => {
            let mut per_city = Vec::new();
            let mut mine: Vec<&City> = cities
                .iter()
                .filter(|c| c.owner.cast_unsigned() == seat)
                .collect();
            mine.sort_by_key(|c| c.id);
            for c in mine {
                let other = read.city_builds(c.id)?;
                if let Some(o) = other
                    .iter()
                    .find(|o| o.order == b.order && o.item == b.item && o.flags.enabled())
                {
                    per_city.push((
                        entity_key(c.owner, c.id),
                        Arc::from(c.name.as_str()),
                        o.turns,
                    ));
                }
            }
            let it = catalog.order_item(b.order, b.item);
            Some(Detail {
                item: item_key(b.order, b.item),
                name: it.name,
                kind: item_kind(b.order, b.flags).unwrap_or_default(),
                turns: b.turns,
                cost: b.production_cost,
                // See the simplification on `Opt.effect`: no help, maintenance or requirements yet.
                help: None,
                maintenance: None,
                requires: KeyedList::new(),
                compare: compare(&per_city),
                short_gold: b.gold_cost.map(|c| c - purse.gold).filter(|s| *s > 0),
            })
        }
        None => None,
    };

    Ok(Some(Production {
        city: key,
        name: Arc::from(city.name.as_str()),
        capital: city.flags.capital(),
        size: city.population,
        rate: i32::from(city.yields[1]),
        overflow: city.overflow,
        gold: purse.gold,
        faith: purse.faith,
        current: now,
        queue_known,
        queue,
        groups,
        detail,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compare_shares_the_slowest_city() {
        let rows = vec![
            (Key::new(0, 1), Arc::from("Moscow"), Some(7)),
            (Key::new(0, 2), Arc::from("Novgorod"), Some(14)),
            (Key::new(0, 3), Arc::from("Kazan"), None),
        ];
        let c = compare(&rows);
        let f: Vec<f32> = c.iter().map(|r| r.fraction).collect();
        assert_eq!(f, [0.5, 1.0, 0.0]);
    }

    #[test]
    fn kinds_of_items() {
        assert_eq!(
            item_kind(OrderType::Construct, BuildFlags(0x0001)),
            Some(ItemKind::Wonder)
        );
        assert_eq!(
            item_kind(OrderType::Construct, BuildFlags(0)),
            Some(ItemKind::Building)
        );
        assert_eq!(
            item_kind(OrderType::Maintain, BuildFlags(0)),
            Some(ItemKind::Process)
        );
        assert_eq!(item_kind(OrderType::Other(3), BuildFlags(0)), None);
    }
}
