//! What one read of the game costs, in cost units, and the frame budget counted in them.
//!
//! One cost unit is one microsecond of the browser build. Each number is the median time of one call of that
//! `GameRead` method in wasm (release), measured on a game of turn 150 on the biggest map, as the player with the
//! most cities; native code is 1.0 to 1.3 times faster, so the same numbers serve both. Medians below 5 µs are
//! counted as 5. The per-id reads (`unit_actions`, `city_builds`...) are per call, that is per unit or per city;
//! `city_builds` varies most (2070 median, 4570 max).
//!
//! The first call of some reads after a core loads is much slower than the median (the core fills its own caches):
//! `top_bar` about 11 ms, `game_info` 1.4 ms, `unit_actions` 1.6 ms, `city_builds` 4.6 ms. The table does not
//! count those spikes; a host takes them out of the frames with [`UiSource::warm_up`](crate::UiSource::warm_up).
//!
//! To measure again (after a change of the core or of a query): natively
//! `UI2_GETTERS_FIXTURE=<dir>/late-huge cargo test -p ui2-source --test getters_cost -- --ignored --nocapture`
//! (see `tests/getters_cost.rs`), in wasm `cargo xtask web-selftest --scenario ui2-getters`; take the wasm medians.
//!
//! A read answered from the source's own cache costs nothing.

use replica::CatalogTable;

/// The cost units one frame of the source may spend: 1.5 ms of reads. `u32::MAX` means no limit.
pub const DEFAULT_FRAME_BUDGET: u32 = 1500;

/// No limit: every read of a frame is made.
pub const UNLIMITED: u32 = u32::MAX;

/// One read of the game, as the budget counts it: a `GameRead` method (a catalog read per table).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadKind {
    GameInfo,
    TopBar,
    Players,
    Notifications,
    Techs,
    PlayerResources,
    PolicyBranches,
    Units,
    Cities,
    /// Per unit.
    UnitActions,
    /// Per unit.
    UnitPromotions,
    /// Per city.
    CityQueue,
    /// Per city.
    CityBuilds,
    Catalog(CatalogTable),
}

impl ReadKind {
    /// The cost of one call, in cost units (µs of the browser build).
    #[must_use]
    pub const fn cost(self) -> u32 {
        match self {
            ReadKind::GameInfo => 5,
            ReadKind::TopBar => 30,
            ReadKind::Players => 70,
            ReadKind::Notifications => 25,
            ReadKind::Techs => 375,
            ReadKind::PlayerResources => 5,
            ReadKind::PolicyBranches => 5,
            ReadKind::Units => 5,
            ReadKind::Cities => 70,
            ReadKind::UnitActions => 65,
            ReadKind::UnitPromotions => 5,
            ReadKind::CityQueue => 5,
            ReadKind::CityBuilds => 2070,
            ReadKind::Catalog(t) => catalog_cost(t),
        }
    }
}

/// One catalog table. The tables this source does not read were not measured; they count as a big table.
const fn catalog_cost(t: CatalogTable) -> u32 {
    match t {
        CatalogTable::Units => 575,
        CatalogTable::Buildings => 570,
        CatalogTable::Promotions => 540,
        CatalogTable::Technologies => 80,
        CatalogTable::Resources => 70,
        CatalogTable::Eras | CatalogTable::Projects => 10,
        CatalogTable::PolicyBranches => 5,
        _ => 600,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_production_window_read_is_over_one_frame() {
        assert!(ReadKind::CityBuilds.cost() > DEFAULT_FRAME_BUDGET);
        assert!(ReadKind::Techs.cost() < DEFAULT_FRAME_BUDGET);
        let catalog: u32 = [
            CatalogTable::Eras,
            CatalogTable::Technologies,
            CatalogTable::Resources,
            CatalogTable::PolicyBranches,
            CatalogTable::Units,
            CatalogTable::Buildings,
            CatalogTable::Projects,
            CatalogTable::Promotions,
        ]
        .into_iter()
        .map(|t| ReadKind::Catalog(t).cost())
        .sum();
        assert_eq!(catalog, 1860, "the whole catalog is more than one frame");
    }
}
