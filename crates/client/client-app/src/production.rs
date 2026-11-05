//! Production of the opened city: the `builds` of its `CityContext` (net-client 4.6) as the interim city panel.
//!
//! SIMPLIFIED(SIMP-001): a small production panel (numbers, current build, buildable items by
//! section) instead of the city screen (G2); original: the CityView / ProductionPopup Lua screens.

use client_core::Ctx;
use interim_ui::screens::city_panel::{BuildLine, BuildSection, CityPanelView, Offer, Producing};
use protocol::{
    BlockerTarget, Catalog, CityBuildFlags, CityBuildView, CityContext, CityView, Command,
    WorldView,
};

/// `EndTurnBlockingTypes::ENDTURN_BLOCKING_PRODUCTION` (CvEnums.h).
pub const BLOCKING_PRODUCTION: i32 = 2;

/// `YieldTypes` of a purchase (`CityPurchase::yield_`): gold and faith.
const YIELD_GOLD: i8 = 2;
const YIELD_FAITH: i8 = 5;

/// Yield names in `CityView::yields` order.
const YIELDS: [&str; 6] = ["food", "production", "gold", "science", "culture", "faith"];

#[derive(Debug, Default)]
pub struct ProductionChooser {
    city: Option<i32>,
    /// The context of the city has come at least once since the window opened.
    loaded: bool,
    builds: Vec<CityBuildView>,
    /// A build was chosen; the next `World` decides whether a city still waits for production.
    chosen: bool,
}

impl ProductionChooser {
    /// The city window opened. The list comes with the city's context (`sync`); the focus names the city.
    pub fn open(&mut self, city: i32) {
        self.city = Some(city);
        self.loaded = false;
        self.builds.clear();
    }

    /// Takes the context of the open city as it is now; call every frame. The list of a stale context
    /// stays up until the new one comes; the server's `ENABLED` decides what can be started.
    pub fn sync(&mut self, ctx: Ctx<'_, CityContext>) {
        if self.city.is_none() {
            return;
        }
        if let Some(c) = ctx.get() {
            // Only what can be started now: the rules are the server's (`ENABLED`).
            let builds: Vec<CityBuildView> = c
                .builds
                .iter()
                .filter(|b| b.flags.contains(CityBuildFlags::ENABLED))
                .cloned()
                .collect();
            if builds != self.builds {
                self.builds = builds;
            }
            self.loaded = true;
        }
    }

    pub fn close(&mut self) {
        self.city = None;
        self.loaded = false;
        self.builds.clear();
    }

    pub fn city(&self) -> Option<i32> {
        self.city
    }

    /// The panel of the open city; `None` when no city is open or it is gone from the world.
    pub fn view(&self, world: &WorldView, catalog: Option<&Catalog>) -> Option<CityPanelView> {
        let id = self.city?;
        let city = world.cities.iter().find(|c| c.id == id)?;
        Some(CityPanelView {
            name: city.name.clone(),
            summary: summary(city),
            yields: YIELDS
                .iter()
                .zip(city.yields)
                .map(|(n, v)| ((*n).to_owned(), v))
                .collect(),
            producing: producing(city, catalog),
            builds: self
                .builds
                .iter()
                .map(|b| BuildLine {
                    section: section(b),
                    name: item_name(catalog, b.order_type, b.item),
                    turns: b.turns,
                    current: b.flags.contains(CityBuildFlags::CURRENT),
                    cost: b.production_cost,
                    // The gold offer follows the core's CAN_PURCHASE; faith has no flag of its own.
                    gold: (b.gold_cost >= 0).then(|| Offer {
                        cost: b.gold_cost,
                        enabled: b.flags.contains(CityBuildFlags::CAN_PURCHASE),
                    }),
                    faith: (b.faith_cost >= 0).then_some(Offer {
                        cost: b.faith_cost,
                        enabled: world.top_bar.faith >= b.faith_cost,
                    }),
                })
                .collect(),
            loading: !self.loaded,
            gold: world.top_bar.gold,
            faith: world.top_bar.faith,
            refused: None,
            no_connection: false,
        })
    }

    /// The order for the clicked build; closes the panel until the next `World` says otherwise
    /// ([`Self::reopen_for`]).
    pub fn choose(&mut self, index: usize) -> Option<Command> {
        let city = self.city?;
        let b = self.builds.get(index)?.clone();
        self.close();
        self.chosen = true;
        Some(Command::CitySetProduction {
            city,
            order: b.order_type,
            item: b.item,
        })
    }

    /// The `CityPurchase` for the build at `index`, paid with faith or gold; the window stays up
    /// (the next `World` refreshes it). The core refuses what it does not accept.
    pub fn buy(&self, index: usize, faith: bool) -> Option<Command> {
        let city = self.city?;
        let b = self.builds.get(index)?;
        Some(Command::CityPurchase {
            city,
            order: b.order_type,
            item: b.item,
            yield_: if faith { YIELD_FAITH } else { YIELD_GOLD },
        })
    }

    /// The first human-turn `World` after a choice: the city to open again when production still
    /// blocks the turn (another city waits for its build), else `None`.
    pub fn reopen_for(&mut self, world: &WorldView, layered: bool) -> Option<i32> {
        if !self.chosen || world.status != protocol::TurnStatus::HumanTurn {
            return None;
        }
        self.chosen = false;
        match crate::play::blocker_of(world, layered) {
            Some(b) if b.kind == BLOCKING_PRODUCTION => match b.target {
                BlockerTarget::City(id) => Some(id),
                _ => None,
            },
            _ => None,
        }
    }
}

fn section(b: &CityBuildView) -> BuildSection {
    match b.order_type {
        _ if b.flags.contains(CityBuildFlags::WONDER) => BuildSection::Wonders,
        0 => BuildSection::Units,
        1 => BuildSection::Buildings,
        _ => BuildSection::Other,
    }
}

/// The catalog name of an order item (`OrderTypes`: 0 train, 1 construct, 2 create; processes
/// have no catalog table).
fn item_name(catalog: Option<&Catalog>, order: i8, item: i16) -> String {
    let table = catalog.and_then(|c| match order {
        0 => Some(&c.units),
        1 => Some(&c.buildings),
        2 => Some(&c.projects),
        _ => None,
    });
    table
        .and_then(|t| t.iter().find(|e| e.id == item))
        .map_or_else(|| format!("order {order} item {item}"), |e| e.name.clone())
}

fn summary(c: &CityView) -> String {
    let growth = if c.growth_turns > 0 {
        format!(", grows in {} turns", c.growth_turns)
    } else {
        ", not growing".to_owned()
    };
    format!(
        "Population {}{growth}, HP {}/{}",
        c.population, c.hp, c.max_hp
    )
}

fn producing(c: &CityView, catalog: Option<&Catalog>) -> Option<Producing> {
    (c.production_item >= 0).then(|| Producing {
        name: item_name(catalog, c.production_order, c.production_item),
        stored: c.production_stored,
        needed: c.production_needed,
        turns: c.production_turns,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{Blocker, CatalogEntry, TurnStatus};

    fn build(item: i16, flags: u16) -> CityBuildView {
        CityBuildView {
            order_type: 0,
            item,
            turns: 4,
            production_cost: 40,
            gold_cost: -1,
            flags: CityBuildFlags(flags),
            faith_cost: -1,
        }
    }

    fn city_ctx(city: i32, builds: Vec<CityBuildView>) -> CityContext {
        CityContext {
            city,
            view_seq: 3,
            builds,
            ..CityContext::default()
        }
    }

    fn world() -> WorldView {
        crate::test_world::world()
    }

    #[test]
    fn lists_enabled_builds_by_section_and_orders_the_clicked_one() {
        let w = world();
        let city = w.cities.iter().find(|c| c.owner == w.human).unwrap().id;
        let mut p = ProductionChooser::default();
        p.open(city);
        assert!(p.view(&w, None).unwrap().loading);
        p.sync(Ctx::Missing);
        assert!(p.view(&w, None).unwrap().loading, "no context yet");
        let mut wonder = build(7, CityBuildFlags::ENABLED | CityBuildFlags::WONDER);
        wonder.order_type = 1;
        let context = city_ctx(
            city,
            vec![
                build(1, CityBuildFlags::ENABLED),
                build(2, 0),
                build(5, CityBuildFlags::ENABLED | CityBuildFlags::CURRENT),
                wonder,
            ],
        );
        p.sync(Ctx::Fresh(&context));
        let catalog = Catalog {
            units: vec![CatalogEntry {
                id: 1,
                type_name: "UNIT_WARRIOR".into(),
                name: "Warrior".into(),
                ..CatalogEntry::default()
            }],
            ..Catalog::default()
        };
        let v = p.view(&w, Some(&catalog)).unwrap();
        assert!(!v.loading);
        let lines: Vec<_> = v.builds.iter().map(|b| (b.section, b.text())).collect();
        assert_eq!(
            lines,
            [
                (BuildSection::Units, "Warrior: 4 turns".to_owned()),
                (
                    BuildSection::Units,
                    "order 0 item 5: 4 turns (current)".to_owned()
                ),
                (BuildSection::Wonders, "order 1 item 7: 4 turns".to_owned()),
            ]
        );
        assert_eq!(
            p.choose(0),
            Some(Command::CitySetProduction {
                city,
                order: 0,
                item: 1
            })
        );
        assert!(p.view(&w, None).is_none(), "closed after the choice");
    }

    #[test]
    fn reopens_after_a_choice_only_while_production_blocks() {
        let mut w = world();
        let mut p = ProductionChooser::default();
        p.open(1);
        p.builds = vec![build(1, CityBuildFlags::ENABLED)];
        assert!(p.choose(0).is_some());
        w.blocker = Some(Blocker {
            kind: BLOCKING_PRODUCTION,
            target: BlockerTarget::City(2),
        });
        w.status = TurnStatus::AiTurn { active: 1 };
        assert_eq!(p.reopen_for(&w, false), None, "not before the human's turn");
        w.status = TurnStatus::HumanTurn;
        assert_eq!(p.reopen_for(&w, false), Some(2));
        assert_eq!(p.reopen_for(&w, false), None, "once per choice");

        p.open(1);
        p.builds = vec![build(1, CityBuildFlags::ENABLED)];
        assert!(p.choose(0).is_some());
        w.blocker = Some(Blocker {
            kind: 8,
            target: BlockerTarget::Unit(4),
        });
        assert_eq!(
            p.reopen_for(&w, false),
            None,
            "a unit blocks: the panel stays closed"
        );
    }

    #[test]
    fn purchase_offers_follow_the_core_and_buying_keeps_the_window() {
        let mut w = world();
        w.top_bar.gold = 100;
        w.top_bar.faith = 10;
        let city = w.cities.iter().find(|c| c.owner == w.human).unwrap().id;
        let mut p = ProductionChooser::default();
        p.open(city);
        let mut cheap = build(1, CityBuildFlags::ENABLED | CityBuildFlags::CAN_PURCHASE);
        cheap.gold_cost = 60;
        cheap.faith_cost = 8;
        let mut dear = build(2, CityBuildFlags::ENABLED);
        dear.order_type = 1;
        dear.gold_cost = 300;
        dear.faith_cost = 80;
        let free = build(3, CityBuildFlags::ENABLED);
        let context = city_ctx(city, vec![cheap, dear, free]);
        p.sync(Ctx::Fresh(&context));
        let v = p.view(&w, None).unwrap();
        let offers: Vec<_> = v.builds.iter().map(|b| (b.gold, b.faith)).collect();
        let o = |cost, enabled| Some(Offer { cost, enabled });
        assert_eq!(
            offers,
            [
                (o(60, true), o(8, true)),
                (o(300, false), o(80, false)),
                (None, None)
            ]
        );
        assert_eq!((v.gold, v.faith), (100, 10));

        assert_eq!(
            p.buy(0, false),
            Some(Command::CityPurchase {
                city,
                order: 0,
                item: 1,
                yield_: 2
            })
        );
        assert_eq!(
            p.buy(1, true),
            Some(Command::CityPurchase {
                city,
                order: 1,
                item: 2,
                yield_: 5
            })
        );
        assert_eq!(p.buy(9, false), None);
        assert_eq!(p.city(), Some(city), "buying does not close the window");

        // A new view: the context is stale and the list stays up until the new one comes.
        p.sync(Ctx::Stale(&context));
        let v = p.view(&w, None).unwrap();
        assert!(!v.loading && v.builds.len() == 3);
        p.sync(Ctx::Missing);
        assert_eq!(p.view(&w, None).unwrap().builds.len(), 3);
        // A new window starts empty and loading.
        p.open(city);
        assert!(p.view(&w, None).unwrap().loading);
    }
}
