//! The read queries of the UI v2 (`Q_CITY_QUEUE`, `Q_TECHS`, `Q_PLAYER_RESOURCES`, `Q_POLICY_BRANCHES`,
//! `Q_UNIT_PROMOTIONS`, the fields appended to `Q_TOP_BAR`, `Q_CITIES` and `Q_UNIT_ACTIONS`) on the real core.
//!
//! * `ui2_queries_follow_the_game`: a Duel through `Worker<RealCore>`; the queue of a city follows the v15 queue
//!   orders the host gives the core, the research queue follows `ResearchSet`, the other answers are plausible, and
//!   the host refuses a slot past the end of a queue and a technology the team has.
//! * `ui2_queries_change_nothing`: the same game three times, the humans played by the core's test bot: without
//!   queries, with the queries an old client makes (cities, units, the top bar) after every pump, and with those
//!   plus every UI v2 query. The simulation digest (everything the game serializes) and the replica digest of every
//!   turn must be equal in the three runs.
//!
//! The core is a process singleton: each test runs its game in a child process (this binary again, `GH_UI2_CHILD`).
//! Returns early on the stub; run on Linux/WSL with `GAMECORE_LIB_DIR_x86_64` set
//! (`cargo xtask test -p game-host --gamecore-lib <dir> --test real_ui2_queries`) and the game data in
//! `CIV5_GAMEDATA` (or `assets-out/gamedata-b2` above the checkout).

use std::path::PathBuf;
use std::process::Command as Process;

use game_host::{Core, Worker};
use gamecore as gc;
use protocol::{CityBuildFlags, Command, CommandOutcome, PlayerId, RejectCode, Start, TurnPhase};

#[path = "common/real.rs"]
mod common;
use common::{H, gamedata_dir, init, real_core, setup};

const CHILD_ENV: &str = "GH_UI2_CHILD";

/// Runs `name` in a child process; returns its stdout.
fn in_child(test: &str, name: &str, extra: &[(&str, &str)]) -> String {
    let mut p = Process::new(std::env::current_exe().unwrap());
    p.args(["--exact", test, "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, name);
    for (k, v) in extra {
        p.env(k, v);
    }
    let out = p.output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "child {name} failed:\n{text}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    text
}

fn stub() -> bool {
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return true;
    }
    false
}

// ------------------------------------------------------------------ the queries follow the game

type Item = (gc::OrderType, i16);

fn queue(h: &mut H, player: u8, city: i32) -> Vec<Item> {
    h.w.core_mut()
        .gamecore_mut()
        .city_queue(player, city)
        .expect("Q_CITY_QUEUE")
        .iter()
        .map(|o| (o.order, o.item))
        .collect()
}

fn own_city(h: &mut H, player: u8, city: i32) -> gc::City {
    h.w.core_mut()
        .gamecore_mut()
        .cities(-1)
        .expect("Q_CITIES")
        .into_iter()
        .find(|c| c.owner == player as i8 && c.id == city)
        .expect("the city")
}

fn queries_child() {
    let (w, outs) = Worker::new(real_core(), init(Start::New(setup()), ""), 0);
    let mut h = H::with(w, outs);
    assert_eq!(h.w.phase().1, TurnPhase::Humans);
    let humans: [PlayerId; 2] = [0, 1];
    for p in humans {
        h.found_city(p);
    }
    let me: u8 = 0;
    let city =
        h.w.fresh_view(0)
            .unwrap()
            .cities
            .iter()
            .find(|c| c.owner == 0)
            .expect("the city of seat 0")
            .id;

    // Slot 0 of the queue is the city's production of `Q_CITIES`, with its cost.
    let c = own_city(&mut h, me, city);
    let head: Item = (
        c.production_order.expect("a production"),
        c.production_item.expect("an item"),
    );
    let rows = h.w.core_mut().gamecore_mut().city_queue(me, city).unwrap();
    println!("QUEUE {rows:?}");
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].slot, rows[0].order, rows[0].item),
        (0, head.0, head.1)
    );
    assert_eq!(rows[0].needed, c.production_needed);
    assert_eq!(rows[0].turns, c.production_turns);
    // The own city has its overflow, a foreign one none.
    assert!(c.overflow.is_some_and(|o| o >= 0), "{c:?}");
    let foreign =
        h.w.core_mut()
            .gamecore_mut()
            .cities(-1)
            .unwrap()
            .into_iter()
            .find(|c| c.owner == 1)
            .expect("the city of seat 1");
    assert!(
        foreign.overflow.is_some(),
        "the whole-truth view: {foreign:?}"
    );
    let seen_by_0 = h.w.core_mut().gamecore_mut().cities(0).unwrap();
    assert!(
        seen_by_0
            .iter()
            .filter(|c| c.owner != 0)
            .all(|c| c.overflow.is_none()),
        "a foreign city has no overflow: {seen_by_0:?}"
    );

    // The queue follows the v15 orders: [x] -> [x, a] -> [b, x, a] -> [x, a, b] -> [x, b] -> [x].
    let units: Vec<Item> =
        h.w.core_mut()
            .city_builds(0, city)
            .unwrap()
            .iter()
            .filter(|b| b.order_type == 0 && b.flags.contains(CityBuildFlags::ENABLED))
            .map(|b| (gc::OrderType::Train, b.item))
            .filter(|i| *i != head)
            .collect();
    assert!(units.len() >= 2, "two units to train");
    let (a, b) = (units[0], units[1]);
    let push = |item: Item, front| Command::CityQueuePush {
        city,
        order: item.0.abi() as i8,
        item: item.1,
        front,
    };
    let step = |h: &mut H, cmd: Command, want: Vec<Item>| {
        assert_eq!(h.cmd(0, cmd), CommandOutcome::Applied, "{cmd:?}");
        assert_eq!(queue(h, me, city), want, "after {cmd:?}");
        let c = own_city(h, me, city);
        assert_eq!(
            (c.production_order, c.production_item),
            (Some(want[0].0), Some(want[0].1)),
            "slot 0 is the production after {cmd:?}"
        );
    };
    step(&mut h, push(a, false), vec![head, a]);
    step(&mut h, push(b, true), vec![b, head, a]);
    step(
        &mut h,
        Command::CityQueueMove {
            city,
            from: 0,
            to: 2,
        },
        vec![head, a, b],
    );
    step(
        &mut h,
        Command::CityQueuePop { city, slot: 1 },
        vec![head, b],
    );
    let rows = h.w.core_mut().gamecore_mut().city_queue(me, city).unwrap();
    assert_eq!(rows.iter().map(|r| r.slot).collect::<Vec<_>>(), vec![0, 1]);
    assert!(rows.iter().all(|r| r.needed > 0), "{rows:?}");
    // The host refuses a slot past the end now that the core tells the length; the queue stays.
    for bad in [
        Command::CityQueuePop { city, slot: 2 },
        Command::CityQueueMove {
            city,
            from: 0,
            to: 2,
        },
        Command::CityQueueMove {
            city,
            from: 5,
            to: 0,
        },
    ] {
        assert!(
            matches!(
                h.cmd(0, bad),
                CommandOutcome::Rejected {
                    code: RejectCode::Invalid,
                    ..
                }
            ),
            "{bad:?}"
        );
    }
    step(&mut h, Command::CityQueuePop { city, slot: 1 }, vec![head]);
    // The ordinary production order replaces the queue.
    assert_eq!(
        h.cmd(
            0,
            Command::CitySetProduction {
                city,
                order: a.0.abi() as i8,
                item: a.1,
            }
        ),
        CommandOutcome::Applied
    );
    assert_eq!(queue(&mut h, me, city), vec![a]);

    // Techs: the row with queue 0 is the top bar's research, with the same progress and cost.
    let g = h.w.core_mut().gamecore_mut();
    let catalog = g.catalog(gc::CatalogTable::Technologies).unwrap();
    let techs = g.techs(me).unwrap();
    let top = g.top_bar(me).unwrap();
    println!("TOP {top:?}");
    assert_eq!(techs.len(), catalog.len(), "a row per technology");
    assert!(techs.windows(2).all(|w| w[0].tech < w[1].tech), "by id");
    let have = techs
        .iter()
        .filter(|t| t.status == gc::TechStatus::Have)
        .count();
    let can = techs
        .iter()
        .filter(|t| t.status == gc::TechStatus::CanResearch)
        .count();
    assert!(have >= 1 && can >= 2, "have {have}, can research {can}");
    assert!(
        techs
            .iter()
            .all(|t| !matches!(t.status, gc::TechStatus::Other(_)))
    );
    assert!(
        techs.iter().all(|t| t.cost > 0 && t.progress >= 0),
        "{techs:?}"
    );
    assert!(
        techs
            .iter()
            .filter(|t| t.status == gc::TechStatus::Have)
            .all(|t| t.turns.is_none() && t.queue.is_none()),
        "a known technology has no turns and no queue place"
    );
    let current = techs.iter().find(|t| t.queue == Some(0));
    assert_eq!(current.map(|t| t.tech), top.research_tech, "{techs:?}");
    if let Some(cur) = current {
        assert_eq!(
            (cur.progress, cur.cost),
            (top.research_progress, top.research_cost)
        );
        assert_eq!(cur.turns, Some(top.research_turns).filter(|t| *t >= 0));
    }
    // Tourism is a number from the start.
    assert!(top.tourism.is_some_and(|t| t >= 0), "{top:?}");

    // ResearchSet: one first-tier technology now, another behind it in the queue.
    let options: Vec<i16> = techs
        .iter()
        .filter(|t| t.status == gc::TechStatus::CanResearch && Some(t.tech) != top.research_tech)
        .map(|t| t.tech)
        .collect();
    let (now, later) = (options[0], options[1]);
    assert_eq!(
        h.cmd(
            0,
            Command::ResearchSet {
                tech: now,
                queue: false
            }
        ),
        CommandOutcome::Applied
    );
    assert_eq!(
        h.cmd(
            0,
            Command::ResearchSet {
                tech: later,
                queue: true
            }
        ),
        CommandOutcome::Applied
    );
    let techs = h.w.core_mut().gamecore_mut().techs(me).unwrap();
    let place = |t: i16| techs.iter().find(|r| r.tech == t).and_then(|r| r.queue);
    println!(
        "RESEARCH QUEUE {:?}",
        techs
            .iter()
            .filter(|t| t.queue.is_some())
            .map(|t| (t.tech, t.queue))
            .collect::<Vec<_>>()
    );
    assert_eq!(place(now), Some(0), "{now} is the research");
    assert!(
        place(later).is_some_and(|q| q >= 1),
        "{later} waits in the queue"
    );
    let top = h.w.core_mut().gamecore_mut().top_bar(me).unwrap();
    assert_eq!(top.research_tech, Some(now));
    // A technology the team has is refused by the host.
    let known = techs
        .iter()
        .find(|t| t.status == gc::TechStatus::Have)
        .unwrap()
        .tech;
    for queue in [false, true] {
        assert!(matches!(
            h.cmd(0, Command::ResearchSet { tech: known, queue }),
            CommandOutcome::Rejected {
                code: RejectCode::Invalid,
                ..
            }
        ));
    }
    // A locked technology (prerequisites missing) is taken: the core queues its prerequisites first.
    let locked = techs
        .iter()
        .find(|t| t.status == gc::TechStatus::Locked)
        .expect("a locked technology")
        .tech;
    assert_eq!(
        h.cmd(
            0,
            Command::ResearchSet {
                tech: locked,
                queue: false
            }
        ),
        CommandOutcome::Applied
    );
    let techs = h.w.core_mut().gamecore_mut().techs(me).unwrap();
    let queued: Vec<(i16, u8)> = techs
        .iter()
        .filter_map(|t| t.queue.map(|q| (t.tech, q)))
        .collect();
    println!("LOCKED {locked} QUEUE {queued:?}");
    assert!(
        queued.iter().any(|&(t, q)| t == locked && q >= 1),
        "the locked technology is behind its prerequisites: {queued:?}"
    );
    assert!(queued.iter().any(|&(_, q)| q == 0));

    // Resources, policy branches, promotions, the extra unit rows: plausible.
    let g = h.w.core_mut().gamecore_mut();
    let resources = g.player_resources(me).unwrap();
    println!("RESOURCES {resources:?}");
    let res_catalog = g.catalog(gc::CatalogTable::Resources).unwrap();
    for r in &resources {
        assert!(r.total > 0 || r.used > 0, "{r:?}");
        assert!(r.used >= 0, "{r:?}");
        assert!(res_catalog.iter().any(|e| e.id == r.resource), "{r:?}");
        assert!(!matches!(r.usage, gc::ResourceUsage::Other(_)), "{r:?}");
    }
    let branches = g.policy_branches(me).unwrap();
    let branch_catalog = g.catalog(gc::CatalogTable::PolicyBranches).unwrap();
    let policies = g.catalog(gc::CatalogTable::Policies).unwrap();
    println!("BRANCHES {branches:?}");
    assert_eq!(branches.len(), branch_catalog.len());
    assert!(!branches.is_empty());
    assert!(
        branches.iter().all(|b| b.owned <= b.total && b.total > 0),
        "{branches:?}"
    );
    let in_branches: usize = branches.iter().map(|b| usize::from(b.total)).sum();
    assert!(
        in_branches <= policies.len(),
        "{in_branches} of {}",
        policies.len()
    );
    assert!(
        branches.iter().all(|b| b.owned == 0),
        "no policy at the start"
    );

    let units: Vec<gc::Unit> = g
        .units(0)
        .unwrap()
        .into_iter()
        .filter(|u| u.owner == 0)
        .collect();
    assert!(!units.is_empty());
    let promo_catalog = g.catalog(gc::CatalogTable::Promotions).unwrap();
    for u in &units {
        let p = g.unit_promotions(me, u.id).unwrap();
        println!("PROMOTIONS unit {} type {}: {p:?}", u.id, u.unit_type);
        assert!(p.windows(2).all(|w| w[0] < w[1]), "by id, once: {p:?}");
        assert!(p.iter().all(|id| promo_catalog.iter().any(|e| e.id == *id)));
        let plain = g.unit_actions(me, u.id).unwrap();
        let rows = g.unit_actions_with_rows(me, u.id).unwrap();
        println!("ACTIONS unit {}: {rows:?}", u.id);
        assert_eq!(
            &rows[..plain.len()],
            &plain[..],
            "the commands come first, unchanged"
        );
        assert!(plain.iter().all(|a| a.row().is_none() && a.cost.is_none()));
        for r in &rows[plain.len()..] {
            let row = r.row().expect("an extra row");
            match row {
                gc::UnitRow::Upgrade => {
                    assert!(r.cost.is_some_and(|c| c >= 0), "{r:?}");
                }
                gc::UnitRow::Alert | gc::UnitRow::Pillage => assert!(r.cost.is_none(), "{r:?}"),
            }
        }
    }
    // Unit ids are unique inside the owner: an id the seat does not have is an argument error.
    let max_id = units.iter().map(|u| u.id).max().unwrap_or(0);
    assert!(g.unit_promotions(me, max_id + 1).is_err());
    assert!(g.city_queue(me, 999_999).is_err());

    // A turn on, the queue is still the production and the answers still hold together.
    h.next_turn(&humans);
    let c = own_city(&mut h, me, city);
    let rows = h.w.core_mut().gamecore_mut().city_queue(me, city).unwrap();
    assert_eq!(
        rows.first().map(|r| (r.order, r.item)),
        c.production_order.zip(c.production_item)
    );
    let g = h.w.core_mut().gamecore_mut();
    let top = g.top_bar(me).unwrap();
    let techs = g.techs(me).unwrap();
    assert_eq!(
        techs.iter().find(|t| t.queue == Some(0)).map(|t| t.tech),
        top.research_tech
    );
    println!("DONE turn {}", h.w.phase().0);
}

#[test]
fn ui2_queries_follow_the_game() {
    if stub() {
        return;
    }
    if std::env::var(CHILD_ENV).as_deref() == Ok("queries") {
        queries_child();
        return;
    }
    if std::env::var_os(CHILD_ENV).is_some() {
        return;
    }
    let out = in_child("ui2_queries_follow_the_game", "queries", &[]);
    assert!(out.contains("DONE turn"), "{out}");
}

// ------------------------------------------------------------------ the queries change nothing

/// What a run asks the core after every pump.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Noise {
    /// Nothing.
    Quiet,
    /// What a client without the UI v2 asks: cities, units, the top bar of each human.
    Old,
    /// That plus every UI v2 query, for every human, city and unit.
    New,
}

impl Noise {
    fn name(self) -> &'static str {
        match self {
            Noise::Quiet => "quiet",
            Noise::Old => "old",
            Noise::New => "new",
        }
    }

    fn of(name: &str) -> Option<Noise> {
        [Noise::Quiet, Noise::Old, Noise::New]
            .into_iter()
            .find(|n| n.name() == name)
    }
}

struct DirHost(PathBuf);

impl gc::HostServices for DirHost {
    fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        if path.split(['/', '\\']).any(|p| p == "..") {
            return None;
        }
        std::fs::read(self.0.join(path)).ok()
    }

    fn game_event(&self, _ev: gc::GameEvent) {}
}

/// Noise bursts after every pump.
const BURSTS_PER_STEP: u32 = 10;

/// The queries of one noise burst; returns how many were answered.
fn burst(game: &mut gc::Game, humans: &[(u8, i32)], noise: Noise) -> u64 {
    let mut n = 0u64;
    let mut ok = |r: bool| n += u64::from(r);
    if noise == Noise::Quiet {
        return 0;
    }
    for &(p, team) in humans {
        let cities = game.cities(team).unwrap_or_default();
        let units = game.units(team).unwrap_or_default();
        ok(game.top_bar(p).is_ok());
        if noise != Noise::New {
            continue;
        }
        ok(game.techs(p).is_ok());
        ok(game.player_resources(p).is_ok());
        ok(game.policy_branches(p).is_ok());
        for c in cities.iter().filter(|c| c.owner == p as i8) {
            ok(game.city_queue(p, c.id).is_ok());
        }
        for u in units.iter().filter(|u| u.owner == p as i8) {
            ok(game.unit_promotions(p, u.id).is_ok());
            ok(game.unit_actions_with_rows(p, u.id).is_ok());
        }
        // Again, so that a query that is not idempotent shows.
        ok(game.techs(p).is_ok());
    }
    n
}

/// One game: two test-bot humans and two AI on a Duel map, `turns` turns. Prints a `DIGEST` line per turn.
fn purity_child(noise: Noise, turns: i32) {
    let mut game = gc::Game::new(DirHost(gamedata_dir())).unwrap();
    let slots = vec![
        gc::SlotSetup::human("random", "bot0"),
        gc::SlotSetup::human("random", "bot1"),
        gc::SlotSetup::ai("random"),
        gc::SlotSetup::ai("random"),
    ];
    let mut setup = gc::Setup::network(4242, slots);
    setup.world_size = "WORLDSIZE_DUEL".into();
    setup.handicap = "HANDICAP_CHIEFTAIN".into();
    setup.autoplay_humans = vec![0, 1];
    game.new_game(&setup).unwrap();
    game.set_role(gc::Role::Server).unwrap();
    let players = game.players(-1).unwrap();
    let humans: Vec<(u8, i32)> = [0u8, 1]
        .iter()
        .map(|&p| {
            let team = players.iter().find(|x| x.id == p as i8).unwrap().team;
            (p, i32::from(team))
        })
        .collect();

    let mut answered = 0u64;
    let mut seen = game.game_info().unwrap().turn;
    let start = seen;
    let mut guard = 0u32;
    while seen < start + turns {
        guard += 1;
        assert!(guard < 200_000, "the game does not move");
        // Small steps: the queries come between many slices of the AI's turn too.
        game.pump(8).unwrap();
        let _ = game.journal_take(false);
        // A client asks many times between two steps of the core (one rebuild per frame).
        for _ in 0..BURSTS_PER_STEP {
            answered += burst(&mut game, &humans, noise);
        }
        let info = game.game_info().unwrap();
        if info.turn > seen {
            seen = info.turn;
            let sim = game.state_digest(-2).unwrap();
            let rep = game.state_digest(gc::replica::REPLICA_DIGEST).unwrap();
            println!("DIGEST {seen} {sim:016x} {rep:016x}");
        }
    }
    println!("ANSWERED {answered}");
}

fn digests(out: &str) -> Vec<&str> {
    out.lines().filter(|l| l.starts_with("DIGEST ")).collect()
}

#[test]
fn ui2_queries_change_nothing() {
    if stub() {
        return;
    }
    let turns: i32 = std::env::var("GH_UI2_TURNS")
        .ok()
        .and_then(|t| t.parse().ok())
        .unwrap_or(40);
    if let Some(noise) = std::env::var(CHILD_ENV).ok().as_deref().and_then(Noise::of) {
        purity_child(noise, turns);
        return;
    }
    if std::env::var_os(CHILD_ENV).is_some() {
        return;
    }
    let runs: Vec<(Noise, String)> = [Noise::Quiet, Noise::Old, Noise::New]
        .into_iter()
        .map(|n| (n, in_child("ui2_queries_change_nothing", n.name(), &[])))
        .collect();
    let quiet = digests(&runs[0].1);
    // A digest per turn change the pump showed (one step may cross two turns).
    assert!(
        quiet.len() * 4 >= turns as usize * 3,
        "{} digests in {turns} turns",
        quiet.len()
    );
    for (noise, out) in &runs[1..] {
        let answered = out
            .lines()
            .find_map(|l| l.strip_prefix("ANSWERED "))
            .and_then(|n| n.parse::<u64>().ok())
            .unwrap_or(0);
        println!("{noise:?}: {answered} queries answered");
        assert!(answered >= 2000, "{noise:?}: only {answered} queries");
        let d = digests(out);
        let first = quiet.iter().zip(&d).position(|(a, b)| a != b);
        assert_eq!(
            first,
            None,
            "{noise:?}: the first turn that differs from the quiet run: {:?} vs {:?}",
            first.map(|i| quiet[i]),
            first.map(|i| d[i])
        );
        assert_eq!(d.len(), quiet.len(), "{noise:?}");
    }
}
