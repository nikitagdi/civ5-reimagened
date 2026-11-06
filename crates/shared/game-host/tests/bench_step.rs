//! The cost of a step (game-host-mp 4.4, 4.17, AC-N2.5, AC-N5.4): 500 random orders of the humans of a game
//! (Duel with 4 humans, Huge with 8) fed to `Worker`, one order at a time; the wall time of each call
//! (`handle` plus the `step`s that follow until the worker is idle) is taken from outside, the parts of the step
//! and the time of the AI phase from `StepStats`. Prints p50 and p95.
//!
//! - the test world (`FixtureCore`, any OS): Duel x 4 and Huge x 8, both in this process;
//! - the real core (Linux/WSL, `GAMECORE_LIB_DIR_x86_64`, game data in `CIV5_GAMEDATA`): the core is one per
//!   process, so each case runs in a child process (this binary again with `GH_BENCH_CASE`); returns early on
//!   the stub.
//!
//! `cargo xtask test -p game-host --release --test bench_step -- --nocapture`

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Command as Process;
use std::time::Instant;

use game_host::fixture::FixtureCore;
use game_host::real::RealCore;
use game_host::{Core, Out, SaveDone, Worker};
use protocol::{
    AutoChoices, CivPick, Cmd, Command, CommandOutcome, CoreKind, GameSettings, GameSetup,
    IPC_VERSION, PlayerId, SlotControl, SlotSetup, Start, StepKind, StepStats, TimerMode, ToWorker,
    TurnMode, TurnPhase, UnitFlags, WorkerConfig, WorkerInit, WorkerMsg,
};

/// AC-N2.5 (preliminary): p95 of a step, ms.
const LIMIT_DUEL_MS: f64 = 20.0;
const LIMIT_HUGE_MS: f64 = 60.0;
const ORDERS: usize = 500;

#[derive(Clone, Copy)]
struct Case {
    name: &'static str,
    huge: bool,
    humans: usize,
    ais: usize,
}

const DUEL4: Case = Case {
    name: "duel4",
    huge: false,
    humans: 4,
    ais: 2,
};
const HUGE8: Case = Case {
    name: "huge8",
    huge: true,
    humans: 8,
    ais: 4,
};

/// The test world has 10 slots at most.
fn for_fixture(c: Case) -> Case {
    Case {
        ais: c.ais.min(10 - c.humans),
        ..c
    }
}

struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) % n.max(1) as u64) as usize
    }
}

fn setup(c: Case, real: bool) -> GameSetup {
    let mut slots = Vec::new();
    for i in 0..c.humans + c.ais {
        slots.push(SlotSetup {
            player: i as i8,
            civ: CivPick::Random,
            control: if i < c.humans {
                SlotControl::Human {
                    nick: format!("p{i}"),
                }
            } else {
                SlotControl::Ai
            },
        });
    }
    GameSetup {
        settings: GameSettings {
            name: "bench".into(),
            world_size: if c.huge {
                "WORLDSIZE_HUGE"
            } else {
                "WORLDSIZE_DUEL"
            }
            .into(),
            map_script: if real {
                "Assets/Maps/Lekmap v5.2/LekmapPangaeaFractalv5.2.lua"
            } else {
                "fixture.lua"
            }
            .into(),
            map_options: Vec::new(),
            speed: "GAMESPEED_STANDARD".into(),
            start_era: "ERA_ANCIENT".into(),
            handicap: "HANDICAP_CHIEFTAIN".into(),
            minor_civs: 0,
            turn_mode: TurnMode::Simultaneous,
            timer: TimerMode::Off,
            game_options: Vec::new(),
            seed: Some(4242),
            map_seed: None,
            auto_choices: AutoChoices::default(),
            diplo_auto_decline: true,
            mods: Vec::new(),
        },
        slots,
    }
}

/// One call of the worker as seen from outside.
struct Sample {
    wall_us: u64,
    /// The game turn changed during the call (the AI phase ran inside it).
    turn_changed: bool,
    stats: Vec<StepStats>,
}

struct Bench<C: Core> {
    w: Worker<C>,
    now: u64,
    next_id: u32,
    turn_before: i32,
    samples: Vec<Sample>,
    /// The answers of the last call.
    results: Vec<(PlayerId, u32, CommandOutcome)>,
    unanswered: Vec<SaveDone>,
}

impl<C: Core> Bench<C> {
    fn feed(&mut self, outs: Vec<Out>, stats: &mut Vec<StepStats>) {
        for o in outs {
            match o {
                Out::Frame(f) => {
                    for m in f.msgs {
                        match m {
                            WorkerMsg::Fatal { message, code } => panic!("fatal {code}: {message}"),
                            WorkerMsg::Stats(s) => stats.push(s),
                            WorkerMsg::Results { player, results } => {
                                for r in results {
                                    self.results.push((player, r.id, r.outcome));
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Out::Save(j) => self.unanswered.push(SaveDone {
                    file: j.file,
                    saved_at_ms: 1_000 + j.file.seq,
                    result: Ok(j.gcs1.len() as u64),
                }),
                Out::Replica(_) => {}
                Out::Exit(c) => panic!("the worker exits with {c}"),
            }
        }
    }

    /// Runs the worker until idle; every call is timed. `first` is the outs of the call that started it.
    fn settle(&mut self, first: Option<(Vec<Out>, u64)>, record: bool) {
        let mut stats = Vec::new();
        let mut wall = 0;
        if let Some((outs, us)) = first {
            wall += us;
            self.feed(outs, &mut stats);
        }
        for _ in 0..50_000 {
            while let Some(d) = self.unanswered.pop() {
                self.now += 1;
                let t = Instant::now();
                let outs = self.w.save_done(d, self.now);
                wall += t.elapsed().as_micros() as u64;
                self.feed(outs, &mut stats);
            }
            if !self.w.wants_step() {
                if record {
                    self.samples.push(Sample {
                        wall_us: wall,
                        turn_changed: self.w.phase().0 != self.turn_before,
                        stats,
                    });
                }
                return;
            }
            self.now += 1;
            let t = Instant::now();
            let outs = self.w.step(self.now);
            wall += t.elapsed().as_micros() as u64;
            self.feed(outs, &mut stats);
        }
        panic!("the worker never went idle");
    }

    fn start(&mut self, outs: Vec<Out>) {
        let mut s = Vec::new();
        self.feed(outs, &mut s);
        self.settle(None, false);
    }

    /// One order; returns the answer.
    fn order(&mut self, player: PlayerId, cmd: Command, record: bool) -> CommandOutcome {
        self.next_id += 1;
        let id = self.next_id;
        let turn = self.w.phase().0;
        self.turn_before = turn;
        self.results.clear();
        self.now += 1;
        let t = Instant::now();
        let outs = self.w.handle(
            ToWorker::Cmd {
                player,
                cmd: Cmd { id, turn, cmd },
            },
            self.now,
        );
        let us = t.elapsed().as_micros() as u64;
        self.settle(Some((outs, us)), record);
        self.results
            .iter()
            .rev()
            .find(|r| r.0 == player && r.1 == id)
            .map_or(CommandOutcome::Queued, |r| r.2.clone())
    }

    /// The order the seat would give next (the queries are not timed).
    fn next_cmd(&mut self, p: PlayerId, rng: &mut Rng, pending: &mut Option<Command>) -> Command {
        if let Some(c) = pending.take() {
            return c;
        }
        let view = self.w.fresh_view(p).expect("a view");
        let idle: Vec<i32> = view
            .units
            .iter()
            .filter(|u| u.owner == p && u.flags.contains(UnitFlags::NEEDS_ORDERS))
            .map(|u| u.id)
            .collect();
        if !idle.is_empty() {
            let unit = idle[rng.below(idle.len())];
            if rng.below(100) < 60 {
                let tree = self.w.core_mut().reach(p, unit, 1).unwrap_or_default();
                if tree.len() > 1 {
                    let n = &tree[1 + rng.below(tree.len() - 1)];
                    return Command::UnitMoveTo {
                        unit,
                        x: n.x,
                        y: n.y,
                    };
                }
            }
            return match rng.below(3) {
                0 => Command::UnitFortify { unit },
                1 => Command::UnitSleep { unit },
                _ => Command::UnitSkip { unit },
            };
        }
        if let Some(c) = view
            .cities
            .iter()
            .find(|c| c.owner == p && c.production_order < 0)
        {
            let build = self.w.core_mut().city_builds(p, c.id).ok().and_then(|bs| {
                bs.into_iter()
                    .find(|b| b.flags.contains(protocol::CityBuildFlags::ENABLED))
            });
            if let Some(b) = build {
                return Command::CitySetProduction {
                    city: c.id,
                    order: b.order_type,
                    item: b.item,
                };
            }
        }
        Command::EndTurn { force: true }
    }

    /// Founds a city with every settler, and gives every city a build; not timed.
    fn found_cities(&mut self, humans: usize) {
        for p in 0..humans as PlayerId {
            let view = self.w.fresh_view(p).expect("a view");
            let settlers: Vec<i32> = view
                .units
                .iter()
                .filter(|u| u.owner == p)
                .filter(|u| {
                    self.w
                        .core_mut()
                        .unit_actions(p, u.id)
                        .is_ok_and(|a| a.iter().any(|a| a.action == 3 && a.enabled))
                })
                .map(|u| u.id)
                .collect();
            for unit in settlers {
                self.order(p, Command::UnitFoundCity { unit }, false);
            }
        }
    }

    fn play(&mut self, humans: usize, seed: u64) -> usize {
        let mut rng = Rng(seed);
        self.found_cities(humans);
        let mut ended: BTreeSet<PlayerId> = BTreeSet::new();
        let mut turn = self.w.phase().0;
        let mut pending: Vec<Option<Command>> = vec![None; humans];
        let mut turns = 0;
        for _ in 0..ORDERS {
            if self.w.phase().0 != turn {
                turn = self.w.phase().0;
                turns += 1;
                ended.clear();
            }
            let open: Vec<PlayerId> = (0..humans as PlayerId)
                .filter(|p| !ended.contains(p))
                .collect();
            if open.is_empty() || self.w.phase().1 != TurnPhase::Humans {
                ended.clear();
                continue;
            }
            let p = open[rng.below(open.len())];
            let mut pend = pending[p as usize].take();
            let cmd = self.next_cmd(p, &mut rng, &mut pend);
            let end = matches!(cmd, Command::EndTurn { .. });
            let out = self.order(p, cmd, true);
            match out {
                CommandOutcome::Applied if end => {
                    ended.insert(p);
                }
                CommandOutcome::Rejected { reason, .. } if end => {
                    // A unit stacked on another: the one the core names goes (a bot's way out).
                    if let Some(unit) = reason
                        .split("unit ")
                        .nth(1)
                        .and_then(|t| t.split_whitespace().next())
                        .and_then(|n| n.parse::<i32>().ok())
                        .filter(|_| reason.contains("STACKED_UNITS"))
                    {
                        pending[p as usize] = Some(Command::UnitDisband { unit });
                    }
                }
                _ => {}
            }
        }
        turns
    }
}

fn percentile(v: &mut [f64], q: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() as f64 * q).ceil() as usize).clamp(1, v.len()) - 1]
}

/// Prints the report of a case; returns p95 of the order steps, ms.
fn report(core: &str, c: Case, b: &Bench<impl Core>, turns: usize) -> f64 {
    let is_turn_step = |s: &Sample| {
        s.turn_changed
            || s.stats
                .iter()
                .any(|x| matches!(x.kind, StepKind::AiPhase | StepKind::TurnChange))
    };
    let mut orders: Vec<f64> = b
        .samples
        .iter()
        .filter(|s| !is_turn_step(s))
        .map(|s| s.wall_us as f64 / 1000.0)
        .collect();
    let mut turn_steps: Vec<f64> = b
        .samples
        .iter()
        .filter(|s| is_turn_step(s))
        .map(|s| s.wall_us as f64 / 1000.0)
        .collect();
    let mut ai: Vec<f64> = b
        .samples
        .iter()
        .filter(|s| is_turn_step(s))
        .map(|s| s.stats.iter().map(|x| x.ai_us as f64).sum::<f64>() / 1000.0)
        .collect();
    let n = orders.len();
    let mean = |f: fn(&StepStats) -> u32| {
        b.samples
            .iter()
            .filter(|s| !is_turn_step(s))
            .flat_map(|s| s.stats.iter())
            .map(|x| f(x) as f64)
            .sum::<f64>()
            / 1000.0
            / n.max(1) as f64
    };
    let diff_bytes: u64 = b
        .samples
        .iter()
        .flat_map(|s| s.stats.iter())
        .flat_map(|x| x.diff_bytes.iter())
        .map(|d| d.1 as u64)
        .sum();
    let fog: u32 = b
        .samples
        .iter()
        .flat_map(|s| s.stats.iter())
        .map(|x| x.fog_violations)
        .sum();
    let p50 = percentile(&mut orders, 0.5);
    let p95 = percentile(&mut orders, 0.95);
    let limit = if c.huge { LIMIT_HUGE_MS } else { LIMIT_DUEL_MS };
    println!(
        "BENCH {core} {}: {} humans + {} AI, {n} order steps over {turns} turns: step p50 {p50:.2} ms, p95 {p95:.2} ms \
         (limit {limit} ms: {}); mean parts apply {:.2} pump {:.2} views {:.2} diff {:.2} ctx {:.2} save {:.2} ms; \
         diff bytes {diff_bytes}; fog violations {fog}",
        c.name,
        c.humans,
        c.ais,
        if p95 <= limit { "ok" } else { "OVER" },
        mean(|x| x.apply_us),
        mean(|x| x.pump_us),
        mean(|x| x.views_us),
        mean(|x| x.diff_us),
        mean(|x| x.ctx_us),
        mean(|x| x.save_us),
    );
    println!(
        "BENCH {core} {}: turn-change steps {}: wall p50 {:.2} ms, p95 {:.2} ms, max {:.2} ms; AI phase p50 {:.2} ms, p95 {:.2} ms, max {:.2} ms",
        c.name,
        turn_steps.len(),
        percentile(&mut turn_steps, 0.5),
        percentile(&mut turn_steps, 0.95),
        turn_steps.iter().copied().fold(0.0, f64::max),
        percentile(&mut ai, 0.5),
        percentile(&mut ai, 0.95),
        ai.iter().copied().fold(0.0, f64::max),
    );
    assert_eq!(fog, 0, "fog violations");
    p95
}

fn cfg() -> WorkerConfig {
    WorkerConfig {
        stats: true,
        audit_fog: true,
        ..WorkerConfig::default()
    }
}

fn run_case<C: Core>(core: C, kind: CoreKind, c: Case, real: bool) -> f64 {
    let init = WorkerInit {
        ipc: IPC_VERSION,
        game: 5,
        core: kind,
        start: Start::New(setup(c, real)),
        save_dir: "bench".into(),
        seed: 77,
        cfg: cfg(),
    };
    let (w, outs) = Worker::new(core, init, 0);
    let mut b = Bench {
        w,
        now: 0,
        next_id: 0,
        turn_before: 0,
        samples: Vec::new(),
        results: Vec::new(),
        unanswered: Vec::new(),
    };
    b.start(outs);
    let turns = b.play(c.humans, 0x9e37_79b9_7f4a_7c15);
    report(if real { "real" } else { "fixture" }, c, &b, turns)
}

fn gamedata_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CIV5_GAMEDATA") {
        return PathBuf::from(d);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .flat_map(|a| {
            ["gamedata-b2", "gamedata-b1"]
                .into_iter()
                .map(move |n| a.join("assets-out").join(n))
        })
        .find(|d| d.join("vfs-index.json").is_file())
        .expect("game data not found: set CIV5_GAMEDATA=<export-gamedata dir>")
}

fn real_case(c: Case) -> f64 {
    let dir = gamedata_dir();
    let kind = CoreKind::Real {
        gamedata_dir: dir.display().to_string(),
    };
    let core = RealCore::boot(Box::new(move |path| {
        if path.split(['/', '\\']).any(|p| p == "..") {
            return None;
        }
        std::fs::read(dir.join(path)).ok()
    }))
    .expect("the real core boots");
    run_case(core, kind, c, true)
}

#[test]
fn bench_step_fixture() {
    for c in [DUEL4, HUGE8].map(for_fixture) {
        let p95 = run_case(
            FixtureCore::new(c.huge, false),
            CoreKind::Fixture {
                huge: c.huge,
                wrap_x: false,
            },
            c,
            false,
        );
        let limit = if c.huge { LIMIT_HUGE_MS } else { LIMIT_DUEL_MS };
        if !cfg!(debug_assertions) {
            assert!(p95 <= limit, "{}: p95 {p95:.2} ms over {limit} ms", c.name);
        }
    }
}

#[test]
fn bench_step_real() {
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return;
    }
    if let Ok(name) = std::env::var("GH_BENCH_CASE") {
        let c = [DUEL4, HUGE8]
            .into_iter()
            .find(|c| c.name == name)
            .expect("a known case");
        real_case(c);
        return;
    }
    for c in [DUEL4, HUGE8] {
        let out = Process::new(std::env::current_exe().unwrap())
            .args([
                "bench_step_real",
                "--exact",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("GH_BENCH_CASE", c.name)
            .output()
            .unwrap();
        print!("{}", String::from_utf8_lossy(&out.stdout));
        eprint!("{}", String::from_utf8_lossy(&out.stderr));
        assert!(out.status.success(), "case {} failed", c.name);
    }
}
