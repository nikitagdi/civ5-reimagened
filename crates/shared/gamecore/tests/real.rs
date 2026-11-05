//! Integration with the real Lekmod gamecore (ABI v1). Every test returns early on the stub
//! (`gamecore::IMPLEMENTATION != "lekmod"`); run on Linux/WSL with `GAMECORE_LIB_DIR_x86_64` set
//! (`cargo xtask test --gamecore-lib <dir>`, the archives come from `cargo xtask gamecore`).
//!
//! The game data is the `export-gamedata` folder (`assets-out/gamedata-b1`): found by walking up from
//! the crate, or `CIV5_GAMEDATA=<dir>`.
//!
//! The real core is a process singleton (one `gc_create` per process lifetime), so everything that
//! needs an instance is one test.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gamecore::{
    AutoChoiceKind, CatalogTable, Command, CommandError, Game, GameError, GameEvent, GameStatus,
    HostServices, OrderType, PumpResult, PurchaseWith, RangeShooter, Setup, Unit,
};
use gamecore_sys::{GcError, ffi};

struct DirHost {
    dir: PathBuf,
    events: Arc<Mutex<Vec<GameEvent>>>,
}

impl HostServices for DirHost {
    fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        if path
            .split(|c| c == '/' || c == char::from(92u8))
            .any(|p| p == "..")
        {
            return None;
        }
        std::fs::read(self.dir.join(path)).ok()
    }

    fn game_event(&self, ev: GameEvent) {
        self.events.lock().unwrap().push(ev);
    }
}

fn gamedata_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CIV5_GAMEDATA") {
        return PathBuf::from(d);
    }
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    here.ancestors()
        .map(|a| a.join("assets-out/gamedata-b1"))
        .find(|d| d.join("vfs-index.json").is_file())
        .expect("game data not found: set CIV5_GAMEDATA=<export-gamedata dir>")
}

/// Pumps until the human is up; the AI civs run in slices.
fn pump_to_human(game: &mut Game) -> PumpResult {
    for _ in 0..2000 {
        let r = game.pump(64).unwrap();
        if r.status == gamecore_sys::ffi::GC_PUMP_WAITING_HUMAN {
            return r;
        }
        assert_eq!(r.status, gamecore_sys::ffi::GC_PUMP_AI_RUNNING);
    }
    panic!("the human was not up after 2000 pump calls");
}

#[test]
fn real_core_v1_game_and_queries() {
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return;
    }
    assert_eq!(gamecore::abi_version(), gamecore_sys::ffi::GC_ABI_VERSION);

    let events = Arc::new(Mutex::new(Vec::new()));
    let mut game = Game::new(DirHost {
        dir: gamedata_dir(),
        events: events.clone(),
    })
    .unwrap();
    assert_eq!(
        Game::new(DirHost {
            dir: gamedata_dir(),
            events: Arc::default(),
        })
        .err(),
        Some(GameError::AlreadyExists)
    );

    // Before a game: not started.
    assert_eq!(game.pump(1).err(), Some(GameError::NotStarted));
    assert_eq!(game.state_digest(-1).err(), Some(GameError::NotStarted));
    assert_eq!(game.game_info().err(), Some(GameError::NotStarted));
    // A bad setup is an argument error and does not use up the one-game slot.
    assert!(matches!(
        game.new_game_json(r#"{"no_such_key":1}"#),
        Err(GameError::Core(GcError::InvalidArg(_)))
    ));

    // A real game: the typed Setup is accepted key by key.
    let setup = Setup::default();
    game.new_game(&setup).unwrap();
    let up = pump_to_human(&mut game);
    assert!(up.turn >= 0 && up.active_player >= 0);

    // Q_GAME
    let info = game.game_info().unwrap();
    assert_eq!(info.status, GameStatus::WaitingHuman);
    assert_eq!(info.turn, up.turn);
    assert!(info.map_width > 0 && info.map_height > 0);
    assert!(info.human_player >= 0);
    let total = info.plot_count();
    assert!(total > 0);

    // Q_PLOTS: W*H plots, rows by y then x; team 0 sees only part of the map, the whole truth all of it.
    let team = i32::from(info.human_team);
    let seen = game.plots(team).unwrap();
    assert_eq!(seen.len(), total, "plot count = W*H");
    let w = usize::try_from(info.map_width).unwrap();
    for (i, p) in seen.iter().enumerate() {
        assert_eq!(
            (usize::try_from(p.x).unwrap(), usize::try_from(p.y).unwrap()),
            (i % w, i / w),
            "plot {i} is at its row/column"
        );
        if !p.revealed() {
            assert!(!p.visible() && !p.has_city() && p.owner.is_none() && p.route.is_none());
        }
    }
    let revealed = seen.iter().filter(|p| p.revealed()).count();
    let visible = seen.iter().filter(|p| p.visible()).count();
    eprintln!(
        "map {}x{}: {total} plots, {revealed} revealed, {visible} visible; {} players",
        info.map_width,
        info.map_height,
        game.players(team).unwrap().len()
    );
    assert!(revealed > 0, "the start area is revealed");
    assert!(revealed < total, "fog: {revealed} of {total} revealed");
    assert!(visible <= revealed);
    let truth = game.plots(-1).unwrap();
    assert_eq!(truth.len(), total);
    assert!(
        truth.iter().all(|p| p.revealed() && p.visible()),
        "viewer -1 sees everything"
    );
    assert!(
        truth.iter().any(|p| p.rivers().iter().any(|&r| r)),
        "the map has rivers"
    );
    assert!(
        truth
            .iter()
            .any(|p| p.plot_type != gamecore::PlotType::Ocean),
        "the map has land"
    );
    assert!(matches!(
        game.plots(99),
        Err(GameError::Core(GcError::InvalidArg(_)))
    ));

    // Q_PLAYERS
    let players = game.players(team).unwrap();
    assert!(players.len() >= 2, "the human and one AI at least");
    let me = players
        .iter()
        .find(|p| i32::from(p.id) == i32::from(info.human_player))
        .expect("the human is listed");
    assert!(me.flags.alive() && me.flags.human());
    assert!(!me.name.is_empty() && !me.civ_name.is_empty());
    assert!(me.civ_type.is_some());

    // Q_CATALOG: every table answers.
    for table in CatalogTable::ALL {
        let c = game.catalog(table).unwrap();
        assert!(!c.is_empty(), "{table:?} is empty");
        assert!(c.iter().all(|e| !e.type_name.is_empty()), "{table:?}");
    }
    let art = game.catalog(CatalogTable::ArtTechs).unwrap();
    assert!(art.len() <= 64 && art.iter().all(|e| !e.type_name.is_empty()));
    let timers = game.catalog_timers().unwrap();
    assert!(!timers.is_empty() && timers.iter().all(|t| t.type_name.starts_with("TURNTIMER_")));
    // The lobby lists carry localized names, not text keys.
    let sizes = game.catalog_world_sizes().unwrap();
    let speeds = game.catalog(CatalogTable::GameSpeeds).unwrap();
    assert!(
        sizes
            .iter()
            .map(|w| &w.name)
            .chain(speeds.iter().map(|s| &s.name))
            .chain(timers.iter().map(|t| &t.name))
            .all(|n| !n.is_empty() && !n.starts_with("TXT_KEY_"))
    );
    assert!(game.build_id().unwrap().starts_with("lekmod-portable/"));
    let units = game.catalog(CatalogTable::Units).unwrap();
    assert!(
        units
            .iter()
            .any(|u| u.type_name == "UNIT_WARRIOR" && u.name == "Warrior")
    );

    // The digest is stable between two calls without a pump.
    let d = game.state_digest(-1).unwrap();
    assert_eq!(game.state_digest(-1).unwrap(), d);

    play_the_first_turn(&mut game, &events);
    drop(game);
}

/// Units, actions, path, found a city, production, top bar, end turn, notifications (G1 queries).
fn play_the_first_turn(game: &mut Game, events: &Arc<Mutex<Vec<GameEvent>>>) {
    let info = game.game_info().unwrap();
    let me = u8::try_from(info.human_player).unwrap();
    let team = i32::from(info.human_team);
    let unit_types: HashMap<i16, String> = game
        .catalog(CatalogTable::Units)
        .unwrap()
        .into_iter()
        .map(|e| (e.id, e.type_name))
        .collect();

    // Q_UNITS: the human starts with a settler and a warrior.
    let units = game.units(team).unwrap();
    let mine: Vec<&Unit> = units
        .iter()
        .filter(|u| u.owner == info.human_player)
        .collect();
    let find = |name: &str| -> Unit {
        mine.iter()
            .find(|u| unit_types.get(&u.unit_type).map(String::as_str) == Some(name))
            .copied()
            .unwrap_or_else(|| panic!("{name} not among {} own units", mine.len()))
            .clone()
    };
    let settler = find("UNIT_SETTLER");
    let warrior = find("UNIT_WARRIOR");
    assert!(settler.flags.civilian() && !warrior.flags.civilian());
    assert!(settler.flags.can_move() && warrior.flags.can_move());
    assert!(settler.moves_left > 0 && settler.moves_left <= settler.max_moves);
    assert!(warrior.hp > 0 && warrior.hp <= warrior.max_hp && warrior.strength > 0);
    assert!(!settler.name.is_empty());
    assert!(
        units
            .windows(2)
            .all(|w| (w[0].owner, w[0].id) <= (w[1].owner, w[1].id)),
        "by owner then id"
    );
    assert!(game.units(-1).unwrap().len() >= units.len());

    // Q_UNIT_ACTIONS
    let actions = game.unit_actions(me, settler.id).unwrap();
    let found = actions
        .iter()
        .find(|a| i32::from(a.action) == i32::from(ffi::GC_CMD_UNIT_FOUND_CITY))
        .expect("a settler can be asked to found a city");
    assert!(found.enabled && found.reason.is_none(), "{found:?}");
    assert!(
        actions.windows(2).all(|w| w[0].action < w[1].action),
        "command order"
    );
    assert!(game.unit_actions(me, 99_999).is_err());

    // Q_REACHABLE and Q_PATH of the warrior.
    let reach = game.reachable(me, warrior.id).unwrap();
    assert!(!reach.is_empty());
    assert!(
        reach
            .iter()
            .all(|n| n.turn == 0 && (n.x, n.y) != (warrior.x, warrior.y)),
        "this turn, not its own plot"
    );
    let goal = reach[reach.len() / 2];
    let path = game
        .path(me, warrior.id, i32::from(goal.x), i32::from(goal.y))
        .unwrap();
    assert!(path.len() >= 2, "{path:?}");
    assert_eq!((path[0].x, path[0].y), (warrior.x, warrior.y));
    assert_eq!(path[0].turn, 0);
    let last = path.last().unwrap();
    assert_eq!((last.x, last.y), (goal.x, goal.y));
    assert!(path.windows(2).all(|w| w[0].turn <= w[1].turn));
    assert!(
        game.path(me, warrior.id, i32::from(warrior.x), i32::from(warrior.y))
            .unwrap()
            .is_empty(),
        "its own plot is no path"
    );
    assert!(matches!(
        game.path(me, warrior.id, 10_000, 10_000),
        Err(GameError::Core(GcError::InvalidArg(_)))
    ));

    // Commands: a refused one is typed, FOUND_CITY goes through.
    assert!(matches!(
        game.command(me, &Command::UnitFoundCity { unit: 99_999 }),
        Err(CommandError::Rejected { .. })
    ));
    assert!(game.cities(team).unwrap().is_empty());
    game.command(me, &Command::UnitFoundCity { unit: settler.id })
        .unwrap();
    let mut cities = Vec::new();
    for _ in 0..50 {
        game.pump(8).unwrap();
        cities = game.cities(team).unwrap();
        if !cities.is_empty() {
            break;
        }
    }
    let city = cities.first().expect("the settler founded a city").clone();
    assert_eq!((city.x, city.y), (settler.x, settler.y));
    assert_eq!(city.owner, info.human_player);
    assert!(city.flags.capital() && city.flags.visible());
    assert!(!city.name.is_empty());
    assert!(city.population >= 1 && city.hp > 0 && city.max_hp >= city.hp);
    assert!(city.yields[0] > 0, "a city yields food: {:?}", city.yields);
    assert!(game.cities(-1).unwrap().iter().any(|c| c.id == city.id));
    pump_to_human(game);
    assert!(
        !game
            .units(team)
            .unwrap()
            .iter()
            .any(|u| u.id == settler.id && u.owner == info.human_player),
        "the settler is used up"
    );

    // Q_CITY_BUILDS and CITY_SET_PRODUCTION.
    let builds = game.city_builds(me, city.id).unwrap();
    assert!(!builds.is_empty());
    let pick = builds
        .iter()
        .find(|b| {
            b.flags.enabled()
                && b.order == OrderType::Train
                && unit_types.get(&b.item).map(String::as_str) == Some("UNIT_WARRIOR")
        })
        .expect("a city can start a warrior")
        .clone();
    assert!(
        pick.reason.is_none() && pick.production_cost > 0,
        "{pick:?}"
    );
    assert!(
        builds
            .iter()
            .filter(|b| !b.flags.enabled())
            .all(|b| b.reason.is_some())
    );
    game.command(
        me,
        &Command::CitySetProduction {
            city: city.id,
            order: pick.order,
            item: i32::from(pick.item),
        },
    )
    .unwrap();
    pump_to_human(game);
    let city = game.cities(team).unwrap()[0].clone();
    assert_eq!(city.production_order, Some(pick.order));
    assert_eq!(city.production_item, Some(pick.item));
    assert!(city.production_needed > 0);
    let builds = game.city_builds(me, city.id).unwrap();
    assert!(
        builds
            .iter()
            .any(|b| b.flags.current() && b.item == pick.item && b.order == pick.order)
    );

    // Q_TOP_BAR
    let bar = game.top_bar(me).unwrap();
    assert!(
        bar.gold >= 0 && bar.culture >= 0 && bar.faith >= 0,
        "{bar:?}"
    );
    assert!(bar.era >= 0 && bar.next_policy_cost > 0, "{bar:?}");
    assert!(bar.research_tech.is_some() || (bar.research_progress, bar.research_cost) == (0, 0));
    assert!(game.top_bar(99).is_err());

    // Q_RANGE_TARGETS (G1b): a warrior has no ranged attack, the new city has nothing in range;
    // a missing shooter and an id the query cannot carry are argument errors.
    assert_eq!(
        game.range_targets(me, RangeShooter::Unit(warrior.id))
            .unwrap(),
        vec![]
    );
    assert_eq!(
        game.range_targets(me, RangeShooter::City(city.id)).unwrap(),
        vec![]
    );
    for bad in [
        RangeShooter::Unit(99_999),
        RangeShooter::City(99_999),
        RangeShooter::City(0),
        RangeShooter::Unit(-1),
    ] {
        assert!(
            matches!(
                game.range_targets(me, bad),
                Err(GameError::Core(GcError::InvalidArg(_)))
            ),
            "{bad:?}"
        );
    }

    // City and war commands are refused with the core's reason (G1a / G1b), nothing is queued.
    let refused = |game: &mut Game, cmd: Command, needle: &str| match game.command(me, &cmd) {
        Err(CommandError::Rejected { reason, message }) => {
            assert!(message.contains(needle), "{cmd:?}: {message}");
            eprintln!("{cmd:?} refused ({reason:?}): {message}");
        }
        other => panic!("{cmd:?}: expected a refusal, got {other:?}"),
    };
    refused(
        game,
        Command::CityRangeAttack {
            city: city.id,
            x: i32::from(city.x),
            y: i32::from(city.y),
        },
        "cannot shoot at a city",
    );
    refused(
        game,
        Command::CityRangeAttack {
            city: city.id,
            x: -1,
            y: 0,
        },
        "outside the map",
    );
    refused(
        game,
        Command::CityRangeAttack {
            city: 99_999,
            x: 0,
            y: 0,
        },
        "no city",
    );
    if let Some(dear) = builds
        .iter()
        .find(|b| b.flags.enabled() && b.gold_cost.is_some_and(|c| c > bar.gold))
    {
        refused(
            game,
            Command::CityPurchase {
                city: city.id,
                order: dear.order,
                item: i32::from(dear.item),
                with: PurchaseWith::Gold,
            },
            "not enough gold",
        );
    }
    refused(game, Command::DeclareWar { team }, "own team");
    refused(game, Command::DeclareWar { team: 999 }, "does not exist");

    // END_TURN: fortify the warrior (the UNITS blocker), end the turn, see the next one start.
    game.command(me, &Command::UnitFortify { unit: warrior.id })
        .unwrap();
    pump_to_human(game);
    let turn = game.game_info().unwrap().turn;
    events.lock().unwrap().clear();
    game.command(me, &Command::EndTurn).unwrap();
    let after = pump_to_human(game);
    assert_eq!(after.turn, turn + 1);
    {
        let ev = events.lock().unwrap();
        assert!(
            ev.contains(&GameEvent::TurnStarted { turn: turn + 1 }),
            "{ev:?}"
        );
        for e in ev.iter() {
            if let Some(k) = e.auto_choice_kind() {
                assert!(!matches!(k, AutoChoiceKind::Unknown(_)), "{e:?}");
            }
        }
    }

    // Q_NOTIFICATIONS: the human's, ascending by id; nobody else's.
    let notes = game.notifications(me).unwrap();
    eprintln!("{} notifications after turn {}", notes.len(), after.turn);
    assert!(notes.windows(2).all(|w| w[0].id < w[1].id));
    assert!(notes.iter().all(|n| !n.dismissed && n.turn >= 0));
    let other = (0u8..4).find(|&p| p != me).unwrap();
    assert!(game.notifications(other).unwrap().is_empty());
}
