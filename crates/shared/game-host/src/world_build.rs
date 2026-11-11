//! `gamecore` query results -> protocol records ("World building").
//!
//! Pure conversions: no game rules, no state. The brain filters fog; this only renames and
//! re-types fields. An unrevealed plot of `Q_PLOTS(team)` becomes `PlotView::default()` here; the core
//! adapter (`RealCore::plots`) then gives it the world fields of the truth (`PlotView::unrevealed_of`,
//! NP-D19, 2026-10-22). A revealed one keeps `-1` for "none".

use gamecore::{
    Activity, CatalogEntry as CoreEntry, City, CityBuild, CombatForecast, Command as CoreCommand,
    Direction, GameInfo, GameStatus, Notification, OrderType, PathNode as CorePathNode, Player,
    Plot, PlotBasis, PlotType as CorePlot, PurchaseWith, RangeShooter, ReachFlags as CoreReach,
    TopBar as CoreTopBar, Unit, UnitAction,
};
use gamecore_sys::ffi;
use protocol::{
    ArtRefs, Blocker, BlockerTarget, CatalogEntry, CityBuildFlags, CityBuildView, CityFlags,
    CityView, CombatPreview, Command, NotificationView, PathNode, PlayerFlags, PlayerView,
    PlotBasisView, PlotFlags, PlotType, PlotView, ReachFlags, Shooter, TopBar, TurnStatus,
    UnitActionView, UnitFlags, UnitView,
};

/// Why a record could not be converted: the core said something this reader does not know.
pub type BuildError = String;

fn none_as_minus_one(v: Option<i16>) -> i16 {
    v.unwrap_or(-1)
}

/// `Q_PLOTS` record -> `PlotView`.
pub fn plot_view(p: &Plot) -> Result<PlotView, BuildError> {
    if !p.revealed() {
        return Ok(PlotView::default());
    }
    let mut flags = PlotFlags::REVEALED;
    for (on, bit) in [
        (p.visible(), PlotFlags::VISIBLE),
        (p.has_city(), PlotFlags::HAS_CITY),
        (p.lake(), PlotFlags::IS_LAKE),
        (p.coastal(), PlotFlags::IS_COASTAL),
        (p.pillaged(), PlotFlags::PILLAGED),
        (p.natural_wonder(), PlotFlags::NATURAL_WONDER),
        (p.flags.route_pillaged(), PlotFlags::ROUTE_PILLAGED),
        (p.flags.worked(), PlotFlags::WORKED),
    ] {
        if on {
            flags |= bit;
        }
    }
    let plot_type = match p.plot_type {
        CorePlot::Mountain => PlotType::Mountain,
        CorePlot::Hills => PlotType::Hill,
        CorePlot::Land => PlotType::Land,
        CorePlot::Ocean => PlotType::Water,
        CorePlot::Other(v) => {
            return Err(format!("plot ({}, {}) has unknown plot type {v}", p.x, p.y));
        }
    };
    let rivers = Direction::ALL
        .iter()
        .filter(|&&d| p.river(d))
        .fold(0u8, |m, d| m | (1 << d.index()));
    Ok(PlotView {
        flags: PlotFlags(flags),
        plot_type,
        terrain: p.terrain,
        feature: none_as_minus_one(p.feature),
        resource: none_as_minus_one(p.resource),
        improvement: none_as_minus_one(p.improvement),
        route: none_as_minus_one(p.route),
        owner: p.owner.and_then(|o| i8::try_from(o).ok()).unwrap_or(-1),
        continent_art: u8::try_from(p.continent_art).unwrap_or(0),
        rivers,
        yields: p.yields,
        building: none_as_minus_one(p.build_type),
        resource_qty: p.resource_qty,
    })
}

/// `Q_TRADE_ROUTES` record -> `TradeRouteView`. The destination's owner is not carried: the client reads it
/// from the city at `to`.
pub fn trade_route_view(r: &gamecore::TradeRoute) -> protocol::TradeRouteView {
    protocol::TradeRouteView {
        owner: i8::try_from(r.owner).unwrap_or(-1),
        from: r.from,
        to: r.to,
        domain: r.domain,
    }
}

/// `Q_PLAYERS` record -> `PlayerView`.
pub fn player_view(p: &Player) -> PlayerView {
    let mut flags = 0;
    for (on, bit) in [
        (p.flags.alive(), PlayerFlags::ALIVE),
        (p.flags.human(), PlayerFlags::HUMAN),
        (p.flags.minor(), PlayerFlags::MINOR),
        (p.flags.barbarian(), PlayerFlags::BARBARIAN),
        (p.flags.met(), PlayerFlags::MET),
        (
            p.flags.at_war_with_viewer(),
            PlayerFlags::AT_WAR_WITH_VIEWER,
        ),
    ] {
        if on {
            flags |= bit;
        }
    }
    PlayerView {
        id: p.id,
        team: p.team,
        civ_type: none_as_minus_one(p.civ_type),
        leader_type: none_as_minus_one(p.leader_type),
        flags: PlayerFlags(flags),
        color_primary: p.color_primary,
        color_secondary: p.color_secondary,
        name: p.name.clone(),
        civ_name: p.civ_name.clone(),
        era: p.era.unwrap_or(-1),
        art_techs: p.art_techs,
    }
}

/// `Q_CATALOG` record -> `CatalogEntry`.
pub fn catalog_entry(e: &CoreEntry) -> CatalogEntry {
    let text = |s: &Option<String>| s.clone().unwrap_or_default();
    CatalogEntry {
        id: e.id,
        type_name: e.type_name.clone(),
        name: e.name.clone(),
        art: ArtRefs {
            art_define: text(&e.art_define),
            icon_atlas: text(&e.icon_atlas),
            icon_index: none_as_minus_one(e.icon_index),
            flag_atlas: text(&e.flag_atlas),
            flag_index: none_as_minus_one(e.flag_index),
            art_style: text(&e.art_style),
            strategic_view: text(&e.strategic_view),
        },
    }
}

/// Sets `bit` in the mask for every true flag.
fn mask(bits: &[(bool, u16)]) -> u16 {
    bits.iter()
        .filter(|(on, _)| *on)
        .fold(0, |m, (_, bit)| m | bit)
}

/// `ActivityTypes` number of an activity.
fn activity_abi(a: Activity) -> i8 {
    match a {
        Activity::Awake => 0,
        Activity::Hold => 1,
        Activity::Sleep => 2,
        Activity::Heal => 3,
        Activity::Sentry => 4,
        Activity::Intercept => 5,
        Activity::Mission => 6,
        Activity::Other(v) => v,
    }
}

/// `OrderTypes` number, `-1` for none.
fn order_abi(o: Option<OrderType>) -> i8 {
    o.map_or(-1, |o| i8::try_from(o.abi()).unwrap_or(-1))
}

/// `Q_UNITS` record -> `UnitView`.
pub fn unit_view(u: &Unit) -> UnitView {
    let f = u.flags;
    UnitView {
        id: u.id,
        owner: u.owner,
        unit_type: u.unit_type,
        x: u.x,
        y: u.y,
        hp: u.hp,
        max_hp: u.max_hp,
        moves_left: u.moves_left,
        max_moves: u.max_moves,
        strength: u.strength,
        ranged: u.ranged,
        range: u.range,
        domain: u.domain,
        activity: activity_abi(u.activity),
        level: u.level,
        experience: u.experience,
        flags: UnitFlags(mask(&[
            (f.can_move(), UnitFlags::CAN_MOVE),
            (f.needs_orders(), UnitFlags::NEEDS_ORDERS),
            (f.fortified(), UnitFlags::FORTIFIED),
            (f.sleeping(), UnitFlags::SLEEPING),
            (f.automated(), UnitFlags::AUTOMATED),
            (f.embarked(), UnitFlags::EMBARKED),
            (f.civilian(), UnitFlags::CIVILIAN),
            (f.can_promote(), UnitFlags::CAN_PROMOTE),
            (f.garrisoned(), UnitFlags::GARRISONED),
        ])),
        name: u.name.clone(),
        facing: u.facing.map_or(0, |d| d.index() as i8),
        cargo_of: u.transport_id.unwrap_or(-1),
        building: none_as_minus_one(u.build_type),
    }
}

/// `Q_CITIES` record -> `CityView`. "None" (no order, never) is `-1`.
pub fn city_view(c: &City) -> CityView {
    let f = c.flags;
    CityView {
        id: c.id,
        owner: c.owner,
        x: c.x,
        y: c.y,
        name: c.name.clone(),
        population: c.population,
        flags: CityFlags(mask(&[
            (f.capital(), CityFlags::CAPITAL),
            (f.puppet(), CityFlags::PUPPET),
            (f.razing(), CityFlags::RAZING),
            (f.occupied(), CityFlags::OCCUPIED),
            (f.can_range_attack(), CityFlags::CAN_RANGE_ATTACK),
        ])),
        hp: c.hp,
        max_hp: c.max_hp,
        defense: c.defense,
        production_order: order_abi(c.production_order),
        production_item: none_as_minus_one(c.production_item),
        production_turns: none_as_minus_one(c.production_turns),
        production_stored: c.production_stored,
        production_needed: c.production_needed,
        growth_turns: none_as_minus_one(c.growth_turns),
        yields: c.yields,
        buildings: Vec::new(),
        founder: c
            .founder
            .and_then(|f| i8::try_from(f).ok())
            .unwrap_or(c.owner),
    }
}

/// `Q_TOP_BAR` -> `TopBar`; no research is `-1`.
pub fn top_bar(t: &CoreTopBar) -> TopBar {
    TopBar {
        gold: t.gold,
        gold_per_turn: t.gold_per_turn,
        science_per_turn: t.science_per_turn,
        research_tech: none_as_minus_one(t.research_tech),
        research_turns: t.research_turns,
        research_progress: t.research_progress,
        research_cost: t.research_cost,
        culture: t.culture,
        culture_per_turn: t.culture_per_turn,
        next_policy_cost: t.next_policy_cost,
        happiness: t.happiness,
        golden_age_turns: t.golden_age_turns,
        faith: t.faith,
        faith_per_turn: t.faith_per_turn,
        era: t.era,
        golden_age_progress: t.golden_age_progress,
        golden_age_threshold: t.golden_age_threshold,
    }
}

/// `Q_UNIT_ACTIONS` record -> `UnitActionView`.
pub fn action_view(a: &UnitAction) -> UnitActionView {
    UnitActionView {
        action: a.action,
        enabled: a.enabled,
        reason: a.reason.clone().unwrap_or_default(),
    }
}

/// `Q_CITY_BUILDS` record -> `CityBuildView`.
pub fn build_view(b: &CityBuild) -> CityBuildView {
    let f = b.flags;
    CityBuildView {
        order_type: order_abi(Some(b.order)),
        item: b.item,
        turns: none_as_minus_one(b.turns),
        production_cost: b.production_cost,
        gold_cost: b.gold_cost.unwrap_or(-1),
        flags: CityBuildFlags(mask(&[
            (f.wonder(), CityBuildFlags::WONDER),
            (f.current(), CityBuildFlags::CURRENT),
            (f.can_purchase(), CityBuildFlags::CAN_PURCHASE),
            (f.enabled(), CityBuildFlags::ENABLED),
        ])),
        faith_cost: b.faith_cost.unwrap_or(-1),
    }
}

/// `Q_PATH` node -> `PathNode`.
pub fn path_node(n: &CorePathNode) -> PathNode {
    PathNode {
        x: n.x,
        y: n.y,
        turn: n.turn,
        flags: reach_flags(n.flags),
    }
}

/// Flags of a search node (`Q_PATH`, `Q_REACHABLE_TREE`) -> `ReachFlags`.
pub fn reach_flags(f: CoreReach) -> ReachFlags {
    ReachFlags(mask(&[
        (f.attack(), ReachFlags::ATTACK),
        (f.embark(), ReachFlags::EMBARK),
        (f.disembark(), ReachFlags::DISEMBARK),
        (f.city(), ReachFlags::CITY),
        (f.stop(), ReachFlags::STOP),
        (f.unrevealed(), ReachFlags::UNREVEALED),
        (f.capture(), ReachFlags::CAPTURE),
        (f.unreachable(), ReachFlags::UNREACHABLE),
    ]))
}

/// `Q_PLOT_BASIS` record -> `(index, PlotBasisView)`. The relief only: the flags are lake, coast and
/// natural wonder.
pub fn basis_view(b: &PlotBasis) -> Result<(u32, PlotBasisView), BuildError> {
    let plot_type = match b.plot_type {
        CorePlot::Mountain => PlotType::Mountain,
        CorePlot::Hills => PlotType::Hill,
        CorePlot::Land => PlotType::Land,
        CorePlot::Ocean => PlotType::Water,
        CorePlot::Other(v) => {
            return Err(format!("basis plot {} has unknown plot type {v}", b.index));
        }
    };
    let flags = mask(&[
        (b.flags.lake(), PlotFlags::IS_LAKE),
        (b.flags.coastal(), PlotFlags::IS_COASTAL),
        (b.flags.natural_wonder(), PlotFlags::NATURAL_WONDER),
    ]);
    Ok((
        b.index,
        PlotBasisView {
            plot_type,
            terrain: b.terrain,
            continent_art: b.continent_art,
            feature: none_as_minus_one(b.feature),
            flags: PlotFlags(flags),
            rivers: b.rivers,
            river_flow: b.river_flow,
        },
    ))
}

/// `Q_COMBAT_PREVIEW` record -> `CombatPreview`.
pub fn combat_preview(c: &CombatForecast) -> CombatPreview {
    CombatPreview {
        attacker_damage: c.damage_to_attacker,
        defender_damage: c.damage_to_target,
        attacker_strength: c.attacker_strength,
        defender_strength: c.defender_strength,
        defender_hp: c.target_hp,
        defender_max_hp: c.target_max_hp,
        flags: mask(&[
            (c.flags.kills(), CombatPreview::KILLS),
            (c.flags.captures(), CombatPreview::CAPTURES),
            (c.flags.city(), CombatPreview::CITY),
        ]),
    }
}

/// `Q_NOTIFICATIONS` record -> `NotificationView`.
pub fn notification_view(n: &Notification) -> NotificationView {
    NotificationView {
        id: n.id,
        kind: n.kind,
        at: n.plot(),
        data1: n.data1,
        data2: n.data2,
        turn: n.turn,
        dismissed: n.dismissed,
        message: n.message.clone(),
        summary: n.summary.clone(),
    }
}

/// `OrderTypes` of a protocol order number.
fn order_of(v: i8) -> Result<OrderType, BuildError> {
    match i32::from(v) {
        ffi::GC_ORDER_TRAIN => Ok(OrderType::Train),
        ffi::GC_ORDER_CONSTRUCT => Ok(OrderType::Construct),
        ffi::GC_ORDER_CREATE => Ok(OrderType::Create),
        ffi::GC_ORDER_MAINTAIN => Ok(OrderType::Maintain),
        _ => Err(format!("unknown city order type {v}")),
    }
}

/// The protocol command as the core's. Only the shape is checked here (an order or a yield the
/// ABI has no number for); whether the command is allowed is the core's answer.
pub fn core_command(cmd: &Command) -> Result<CoreCommand, BuildError> {
    use CoreCommand as C;
    let x32 = |v: i16| i32::from(v);
    Ok(match *cmd {
        Command::UnitMoveTo { unit, x, y } => C::UnitMoveTo {
            unit,
            x: x32(x),
            y: x32(y),
        },
        Command::UnitRangeAttack { unit, x, y } => C::UnitRangeAttack {
            unit,
            x: x32(x),
            y: x32(y),
        },
        Command::UnitFoundCity { unit } => C::UnitFoundCity { unit },
        Command::UnitFortify { unit } => C::UnitFortify { unit },
        Command::UnitSleep { unit } => C::UnitSleep { unit },
        Command::UnitSkip { unit } => C::UnitSkip { unit },
        Command::UnitHeal { unit } => C::UnitHeal { unit },
        Command::UnitAutomateBuild { unit } => C::UnitAutomateBuild { unit },
        Command::UnitAutomateExplore { unit } => C::UnitAutomateExplore { unit },
        Command::UnitCancel { unit } => C::UnitCancel { unit },
        Command::UnitDisband { unit } => C::UnitDisband { unit },
        Command::UnitWake { unit } => C::UnitWake { unit },
        Command::CitySetProduction { city, order, item } => C::CitySetProduction {
            city,
            order: order_of(order)?,
            item: i32::from(item),
        },
        Command::CityPurchase {
            city,
            order,
            item,
            yield_,
        } => C::CityPurchase {
            city,
            order: order_of(order)?,
            item: i32::from(item),
            with: match i32::from(yield_) {
                ffi::GC_YIELD_GOLD => PurchaseWith::Gold,
                ffi::GC_YIELD_FAITH => PurchaseWith::Faith,
                _ => return Err(format!("a city cannot buy with yield {yield_}")),
            },
        },
        Command::CityRangeAttack { city, x, y } => C::CityRangeAttack {
            city,
            x: x32(x),
            y: x32(y),
        },
        Command::DeclareWar { team } => C::DeclareWar {
            team: i32::from(team),
        },
        Command::EndTurn { force: false } => C::EndTurn,
        Command::EndTurn { force: true } => C::ForceEndTurn,
        // v3 orders: the v3 worker handles them (SIMP-057 for peace); the B1 host has no such command.
        Command::CancelEndTurn | Command::ProposePeace { .. } | Command::AnswerPeace { .. } => {
            return Err("the B1 host does not support this v3 command".into());
        }
        // v15 queue and research orders: no core command maps to them yet; refused like any
        // order this host cannot execute.
        Command::CityQueuePush { .. }
        | Command::CityQueuePop { .. }
        | Command::CityQueueMove { .. }
        | Command::CityReplaceHead { .. }
        | Command::ResearchSet { .. } => {
            return Err("this host does not support the v15 queue and research orders yet".into());
        }
    })
}

/// The protocol shooter of `Query::RangeTargets` as the core's.
pub fn range_shooter(s: Shooter) -> RangeShooter {
    match s {
        Shooter::Unit(id) => RangeShooter::Unit(id),
        Shooter::City(id) => RangeShooter::City(id),
    }
}

/// Who is to move, from `Q_GAME`.
pub fn turn_status(info: &GameInfo) -> Result<TurnStatus, BuildError> {
    match info.status {
        GameStatus::WaitingHuman => Ok(TurnStatus::HumanTurn),
        GameStatus::AiRunning => Ok(TurnStatus::AiTurn {
            active: info.active_player,
        }),
        GameStatus::GameOver => Ok(TurnStatus::GameOver {
            winner: (info.winner >= 0).then_some(info.winner),
        }),
        GameStatus::Stuck => Err("the game is stuck: the core cannot continue".into()),
        GameStatus::Other(v) => Err(format!("unknown game status {v}")),
    }
}

/// The human's end-turn blocker and its target, from `Q_GAME`.
pub fn blocker(info: &GameInfo) -> Option<Blocker> {
    if info.blocker < 0 {
        return None;
    }
    let target = if info.blocker_unit >= 0 {
        BlockerTarget::Unit(info.blocker_unit)
    } else if info.blocker_city >= 0 {
        BlockerTarget::City(info.blocker_city)
    } else if let Some((x, y)) = info.blocker_plot() {
        BlockerTarget::Plot { x, y }
    } else {
        BlockerTarget::None
    };
    Some(Blocker {
        kind: i32::from(info.blocker),
        target,
    })
}

#[cfg(test)]
mod tests {
    use gamecore::{PlayerFlags as CorePlayerFlags, PlotFlags as CorePlotFlags};
    use gamecore_sys::ffi;

    use super::*;

    fn plot(flags: u16, plot_type: CorePlot) -> Plot {
        Plot {
            x: 3,
            y: 4,
            flags: CorePlotFlags(flags),
            plot_type,
            terrain: 2,
            feature: Some(1),
            resource: None,
            resource_qty: 0,
            improvement: Some(5),
            route: None,
            owner: Some(1),
            continent_art: 2,
            yields: [1, 2, 3, 0, 0, 0],
            build_type: None,
            build_pct: 0,
        }
    }

    #[test]
    fn unrevealed_plot_is_the_default() {
        let p = plot(0, CorePlot::Land);
        assert_eq!(plot_view(&p).unwrap(), PlotView::default());
    }

    #[test]
    fn revealed_plot_keeps_fields_and_maps_rivers_by_direction() {
        let rivers =
            (1u16 << (ffi::GC_PLOT_F_RIVER_SHIFT + 2)) | (1u16 << (ffi::GC_PLOT_F_RIVER_SHIFT + 5));
        let flags = ffi::GC_PLOT_F_REVEALED | ffi::GC_PLOT_F_VISIBLE | ffi::GC_PLOT_F_COASTAL;
        let v = plot_view(&plot(flags | rivers, CorePlot::Hills)).unwrap();
        assert_eq!(
            v.flags.0,
            PlotFlags::REVEALED | PlotFlags::VISIBLE | PlotFlags::IS_COASTAL
        );
        assert_eq!(v.plot_type, PlotType::Hill);
        assert_eq!(
            (v.feature, v.resource, v.improvement, v.route),
            (1, -1, 5, -1)
        );
        assert_eq!((v.owner, v.continent_art, v.terrain), (1, 2, 2));
        assert_eq!(v.rivers, (1 << 2) | (1 << 5));
        assert_eq!(v.yields, [1, 2, 3, 0, 0, 0]);
    }

    #[test]
    fn unknown_plot_type_is_an_error() {
        let e = plot_view(&plot(ffi::GC_PLOT_F_REVEALED, CorePlot::Other(9))).unwrap_err();
        assert!(e.contains("unknown plot type 9"), "{e}");
    }

    #[test]
    fn player_flags_and_unknown_civ() {
        let p = Player {
            id: 2,
            team: 2,
            civ_type: None,
            leader_type: None,
            flags: CorePlayerFlags(ffi::GC_PLAYER_F_ALIVE | ffi::GC_PLAYER_F_MET),
            color_primary: 0xAABBCCDD,
            color_secondary: 1,
            name: "Unknown".into(),
            civ_name: "Unknown".into(),
            era: None,
            nick: None,
            slot: -1,
            art_techs: 0,
        };
        let v = player_view(&p);
        assert_eq!((v.civ_type, v.leader_type), (-1, -1));
        assert_eq!(v.flags.0, PlayerFlags::ALIVE | PlayerFlags::MET);
        assert_eq!(v.color_primary, 0xAABBCCDD);
    }

    #[test]
    fn unit_and_city_records_keep_fields_and_flags() {
        let u = Unit {
            id: 8193,
            owner: 0,
            unit_type: 4,
            x: 10,
            y: 11,
            hp: 100,
            max_hp: 100,
            moves_left: 60,
            max_moves: 120,
            strength: 8,
            ranged: 0,
            range: 0,
            domain: 2,
            activity: Activity::Sleep,
            level: 1,
            experience: 0,
            flags: gamecore::UnitFlags(ffi::GC_UNIT_F_CAN_MOVE | ffi::GC_UNIT_F_CIVILIAN),
            transport_id: None,
            build_type: None,
            facing: None,
            name: "Settler".into(),
        };
        let v = unit_view(&u);
        assert_eq!(
            (v.id, v.x, v.y, v.moves_left, v.activity),
            (8193, 10, 11, 60, 2)
        );
        assert_eq!(v.flags.0, UnitFlags::CAN_MOVE | UnitFlags::CIVILIAN);
        assert_eq!(v.name, "Settler");

        let c = City {
            id: 1,
            owner: 0,
            production_order: None,
            x: 3,
            y: 4,
            population: 1,
            flags: gamecore::CityFlags(ffi::GC_CITY_F_CAPITAL | ffi::GC_CITY_F_VISIBLE),
            hp: 200,
            max_hp: 200,
            defense: 9,
            production_item: None,
            production_turns: None,
            growth_turns: Some(5),
            production_stored: 0,
            production_needed: 0,
            yields: [2, 2, 1, 1, 1, 0],
            founder: None,
            name: "Rome".into(),
        };
        let v = city_view(&c);
        assert_eq!(v.flags.0, CityFlags::CAPITAL);
        assert_eq!(
            (
                v.production_order,
                v.production_item,
                v.production_turns,
                v.growth_turns
            ),
            (-1, -1, -1, 5)
        );
        let c = City {
            production_order: Some(OrderType::Train),
            production_item: Some(7),
            ..c
        };
        assert_eq!(
            (
                city_view(&c).production_order,
                city_view(&c).production_item
            ),
            (0, 7)
        );
    }

    #[test]
    fn commands_convert_and_bad_shapes_are_refused() {
        assert_eq!(
            core_command(&Command::UnitMoveTo {
                unit: 3,
                x: 5,
                y: 6
            }),
            Ok(CoreCommand::UnitMoveTo {
                unit: 3,
                x: 5,
                y: 6
            })
        );
        assert_eq!(
            core_command(&Command::EndTurn { force: false }),
            Ok(CoreCommand::EndTurn)
        );
        assert_eq!(
            core_command(&Command::EndTurn { force: true }),
            Ok(CoreCommand::ForceEndTurn)
        );
        assert_eq!(
            core_command(&Command::CitySetProduction {
                city: 1,
                order: 0,
                item: 2
            }),
            Ok(CoreCommand::CitySetProduction {
                city: 1,
                order: OrderType::Train,
                item: 2
            })
        );
        assert_eq!(
            core_command(&Command::CityPurchase {
                city: 1,
                order: 1,
                item: 2,
                yield_: 5
            }),
            Ok(CoreCommand::CityPurchase {
                city: 1,
                order: OrderType::Construct,
                item: 2,
                with: PurchaseWith::Faith
            })
        );
        assert!(
            core_command(&Command::CityPurchase {
                city: 1,
                order: 1,
                item: 2,
                yield_: 3
            })
            .is_err()
        );
        assert!(
            core_command(&Command::CitySetProduction {
                city: 1,
                order: 9,
                item: 2
            })
            .is_err()
        );
        assert_eq!(
            core_command(&Command::DeclareWar { team: 2 }),
            Ok(CoreCommand::DeclareWar { team: 2 })
        );
        assert_eq!(
            core_command(&Command::CityRangeAttack {
                city: 8192,
                x: 3,
                y: 4
            }),
            Ok(CoreCommand::CityRangeAttack {
                city: 8192,
                x: 3,
                y: 4
            })
        );
    }

    /// The v15 queue and research orders have no core command yet: refused, never mapped to another.
    #[test]
    fn v15_orders_are_not_core_commands_yet() {
        for cmd in [
            Command::CityQueuePush {
                city: 1,
                order: 0,
                item: 2,
                front: false,
            },
            Command::CityQueuePop { city: 1, slot: 0 },
            Command::CityQueueMove {
                city: 1,
                from: 0,
                to: 1,
            },
            Command::CityReplaceHead {
                city: 1,
                order: 0,
                item: 2,
            },
            Command::ResearchSet {
                tech: 3,
                queue: false,
            },
        ] {
            let e = core_command(&cmd).unwrap_err();
            assert!(e.contains("does not support"), "{cmd:?}: {e}");
        }
    }

    #[test]
    fn range_shooter_conversion() {
        assert_eq!(range_shooter(Shooter::Unit(7)), RangeShooter::Unit(7));
        assert_eq!(range_shooter(Shooter::City(8192)), RangeShooter::City(8192));
        assert_eq!(range_shooter(Shooter::City(8192)).abi(), Some(-8192));
    }
}
