//! The heavy test world (B3 perf-bench): `CIV5_FIXTURE_HEAVY=<units>[,<cities>]` fills a Huge fixture game
//! with a late-game load, deterministically (a function of the terrain seed and the majors only), so that
//! the render benchmarks have the same scene on every machine without the real core:
//! `units` land units spread over the dry plots (at most one per plot, the five land types of the test
//! catalog round-robin, owners round-robin over the majors), `cities` extra cities at least 3 plots apart
//! with borders, roads on the plots of their rings and farms/mines/pastures on a share of the rest.
//! Dev only, like `CIV5_FIXTURE_REVEAL_ALL`; the real core is not touched by it.

use protocol::{CityFlags, CityView, PlotFlags, PlotType, UnitFlags, UnitView};

use super::map::{Geo, MapData, mix};
use super::seats::{self, Major};

/// The env var; value `units[,cities]` (default cities: units / 20).
pub const HEAVY_ENV: &str = "CIV5_FIXTURE_HEAVY";

/// Parsed `CIV5_FIXTURE_HEAVY`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Heavy {
    pub units: usize,
    pub cities: usize,
}

impl Heavy {
    /// `"1500"` or `"1500,80"`; `None` when unset, zero or malformed.
    pub fn parse(text: &str) -> Option<Self> {
        let mut it = text.split(',');
        let units: usize = it.next()?.trim().parse().ok()?;
        let cities = match it.next() {
            Some(c) => c.trim().parse().ok()?,
            None => units / 20,
        };
        (units > 0).then_some(Heavy { units, cities })
    }

    pub fn from_env() -> Option<Self> {
        Self::parse(&std::env::var(HEAVY_ENV).ok()?)
    }
}

/// Land types that are built round-robin: warrior, archer, horseman, scout, worker.
const KINDS: [i16; 5] = [
    seats::U_WARRIOR,
    seats::U_ARCHER,
    seats::U_HORSEMAN,
    seats::U_SCOUT,
    seats::U_WORKER,
];

fn dry(map: &MapData, geo: &Geo, at: (i16, i16)) -> bool {
    map.plot(geo.index(at))
        .is_some_and(|p| matches!(p.plot_type, PlotType::Land | PlotType::Hill))
}

/// Adds the load to a fresh game: returns the extra units and cities; the map gets the cities' roads,
/// borders and improvements.
pub fn fill(
    heavy: Heavy,
    map: &mut MapData,
    majors: &[Major],
    geo: &Geo,
    base_city: &CityView,
) -> (Vec<UnitView>, Vec<CityView>) {
    let mut cities: Vec<CityView> = Vec::new();
    let mut units: Vec<UnitView> = Vec::new();
    if majors.is_empty() {
        return (units, cities);
    }
    // Dry plots in a fixed pseudo-random order.
    let mut order: Vec<(i16, i16)> = (0..geo.h)
        .flat_map(|y| (0..geo.w).map(move |x| (x, y)))
        .filter(|&at| dry(map, geo, at))
        .collect();
    order.sort_by_key(|&(x, y)| mix(i32::from(x), i32::from(y), 0x4845_5659));

    let mut centres: Vec<(i16, i16)> = majors.iter().map(|m| m.start).collect();
    let mut next_city = vec![2; majors.len()];
    for &at in &order {
        if cities.len() >= heavy.cities {
            break;
        }
        if centres.iter().any(|c| geo.distance(*c, at) < 3) {
            continue;
        }
        let k = cities.len() % majors.len();
        let owner = majors[k].player;
        let id = next_city[k];
        next_city[k] += 1;
        centres.push(at);
        map.place_city(at);
        map.claim(at, 2, owner);
        let mut city = base_city.clone();
        city.id = id;
        city.owner = owner;
        city.founder = owner;
        city.x = at.0;
        city.y = at.1;
        city.name = format!("Heavy {}-{}", owner, id);
        city.population = 3 + (mix(i32::from(at.0), i32::from(at.1), 7) % 10) as i16;
        city.flags = CityFlags(0);
        cities.push(city);
        for ring in geo.within(at, 1) {
            if ring != at
                && dry(map, geo, ring)
                && let Some(p) = map.plot_mut(geo.index(ring))
            {
                p.route = 0;
            }
        }
    }
    // Improvements on owned dry plots that carry nothing yet (farm, mine, pasture).
    for &(x, y) in &order {
        let i = geo.index((x, y));
        let h = mix(i32::from(x), i32::from(y), 0x494D_5052);
        if let Some(p) = map.plot_mut(i)
            && p.owner >= 0
            && p.improvement < 0
            && p.feature < 0
            && p.resource < 0
            && p.flags.0 & PlotFlags::HAS_CITY == 0
            && h.is_multiple_of(3)
        {
            p.improvement = (h / 3 % 3) as i16;
        }
    }
    // Units: one per plot, never on a city plot.
    let mut next_unit = vec![100; majors.len()];
    let mut n = 0;
    for &at in &order {
        if n >= heavy.units {
            break;
        }
        if centres.contains(&at) {
            continue;
        }
        let k = n % majors.len();
        let owner = majors[k].player;
        let id = next_unit[k];
        next_unit[k] += 1;
        let mut u = seats::spawn_unit(id, owner, KINDS[n % KINDS.len()], at);
        u.flags = UnitFlags(u.flags.0 | UnitFlags::FORTIFIED);
        units.push(u);
        n += 1;
    }
    (units, cities)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_forms() {
        assert_eq!(
            Heavy::parse("1500"),
            Some(Heavy {
                units: 1500,
                cities: 75
            })
        );
        assert_eq!(
            Heavy::parse("1500,80"),
            Some(Heavy {
                units: 1500,
                cities: 80
            })
        );
        assert_eq!(Heavy::parse("0"), None);
        assert_eq!(Heavy::parse("x"), None);
    }

    #[test]
    fn fills_a_huge_map_deterministically() {
        use super::super::map::{self, MapData};
        use std::collections::BTreeSet;
        let geo = Geo::new(map::HUGE.0, map::HUGE.1, false);
        let sites = map::start_sites(&geo, 4);
        let majors: Vec<Major> = sites
            .iter()
            .enumerate()
            .map(|(i, s)| Major {
                player: i as _,
                civ: 0,
                control: protocol::SlotControl::Ai,
                start: *s,
                research: 0,
                art_techs: 0,
                era: 0,
                gold_spent: 0,
            })
            .collect();
        let (city, _) = seats::start_position(&majors[0], &geo);
        let run = || {
            let mut m = MapData::generate(geo, 7, &sites);
            let (u, c) = fill(
                Heavy {
                    units: 1500,
                    cities: 60,
                },
                &mut m,
                &majors,
                &geo,
                &city,
            );
            (u, c)
        };
        let (u, c) = run();
        assert_eq!(u.len(), 1500);
        assert_eq!(c.len(), 60);
        let ids: BTreeSet<_> = u.iter().map(|x| (x.owner, x.id)).collect();
        assert_eq!(ids.len(), u.len());
        let places: BTreeSet<_> = u.iter().map(|x| (x.x, x.y)).collect();
        assert_eq!(places.len(), u.len());
        let (u2, _) = run();
        assert_eq!(u, u2);
    }
}
