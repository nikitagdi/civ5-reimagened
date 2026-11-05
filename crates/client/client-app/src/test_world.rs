//! The world of the client tests: the first turn of the fixture's Duel map as the human sees it,
//! built from the opening data of `game_host::fixture` (`build_map_with`, `build_units`,
//! `build_cities`, `build_players`, `catalog`). A development tool; there is no host behind it.
//!
//! Human units 1 Settler (7,12), 3 Warrior (6,12) in Roma (city 1), 4 Archer (8,11), 5 Horseman
//! (10,10); AI Warrior 9 at (13,9) at war; Thebes (city 3) at (30,10).

use game_host::fixture::{
    AI, HUMAN, VISIBLE_UNTIL, build_cities, build_map_with, build_players, build_units,
};
use protocol::{Blocker, BlockerTarget, Catalog, TopBar, TurnStatus, UnitFlags, WorldView};

/// Turn 1, the human to move, on the plain map.
pub(crate) fn world() -> WorldView {
    world_on(false)
}

/// The same world on the map that wraps along X.
pub(crate) fn world_wrapped() -> WorldView {
    world_on(true)
}

/// The catalog the world's type numbers refer to.
pub(crate) fn catalog() -> Catalog {
    game_host::fixture::catalog()
}

/// The world and its catalog, as the client gets them when the game starts.
pub(crate) fn world_and_catalog() -> (WorldView, Catalog) {
    (world(), catalog())
}

fn world_on(wrap_x: bool) -> WorldView {
    let units = build_units();
    let blocker = units
        .iter()
        .find(|u| u.owner == HUMAN && u.flags.contains(UnitFlags::NEEDS_ORDERS))
        .map(|u| Blocker {
            kind: 1,
            target: BlockerTarget::Unit(u.id),
        });
    let turn = 1;
    WorldView {
        turn,
        year: -4000,
        human: HUMAN,
        human_team: 0,
        status: TurnStatus::HumanTurn,
        blocker,
        map: build_map_with(wrap_x),
        // The foreign units in the fog are not sent.
        units: units
            .into_iter()
            .filter(|u| u.owner == HUMAN || u.x < VISIBLE_UNTIL)
            .collect(),
        cities: build_cities(),
        players: build_players(),
        top_bar: TopBar {
            gold: 100 + turn * 3,
            gold_per_turn: 3,
            science_per_turn: 2,
            research_tech: 0,
            research_turns: 8,
            research_progress: 5,
            research_cost: 20,
            culture: turn,
            culture_per_turn: 1,
            next_policy_cost: 25,
            happiness: 5,
            ..TopBar::default()
        },
        notifications: Vec::new(),
        peace_offers: Vec::new(),
        trade_routes: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_opening_world_has_the_fixture_cast() {
        let w = world();
        assert_eq!((w.turn, w.status), (1, TurnStatus::HumanTurn));
        assert_eq!(w.map.plots.len(), 40 * 24);
        assert_eq!(w.cities.len(), 3);
        assert!(w.units.len() >= 9);
        assert!(w.units.iter().any(|u| u.owner == AI));
        assert!(!catalog().units.is_empty());
        assert!(world_wrapped().map.wrap_x && !w.map.wrap_x);
    }
}
