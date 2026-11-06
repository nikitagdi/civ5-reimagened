//! Helpers of the real-core worker tests (`real_mp`, `real_mp_elim`): the Duel setup, the worker harness `H`.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use game_host::real::RealCore;
use game_host::{Core, Out, SaveDone, SaveJob, Worker};
use protocol::{
    AutoChoices, CivPick, Cmd, Command, CommandOutcome, CoreKind, GameSettings, GameSetup,
    IPC_VERSION, PlayerId, SlotControl, SlotSetup, Start, TimerMode, ToWorker, TurnMode, TurnPhase,
    ViewHash, WorkerConfig, WorkerFrame, WorkerInit, WorkerMsg,
};

pub const NICKS: [&str; 2] = ["anna", "ben"];

pub fn gamedata_dir() -> PathBuf {
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

pub fn real_core() -> RealCore {
    let dir = gamedata_dir();
    RealCore::boot(Box::new(move |path| {
        if path.split(['/', '\\']).any(|p| p == "..") {
            return None;
        }
        std::fs::read(dir.join(path)).ok()
    }))
    .expect("the real core boots")
}

pub fn setup() -> GameSetup {
    let slot = |player: i8, civ, control| SlotSetup {
        player,
        civ,
        control,
    };
    GameSetup {
        settings: GameSettings {
            name: "real-mp".into(),
            world_size: "WORLDSIZE_DUEL".into(),
            map_script: "Assets/Maps/Lekmap v5.2/LekmapPangaeaFractalv5.2.lua".into(),
            map_options: Vec::new(),
            speed: "GAMESPEED_STANDARD".into(),
            start_era: "ERA_ANCIENT".into(),
            handicap: "HANDICAP_CHIEFTAIN".into(),
            minor_civs: 0,
            turn_mode: TurnMode::Hybrid,
            timer: TimerMode::Off,
            game_options: Vec::new(),
            seed: Some(4242),
            map_seed: None,
            auto_choices: AutoChoices::default(),
            diplo_auto_decline: true,
            mods: Vec::new(),
        },
        slots: vec![
            slot(
                0,
                CivPick::Civ("CIVILIZATION_ROME".into()),
                SlotControl::Human {
                    nick: NICKS[0].into(),
                },
            ),
            slot(
                1,
                CivPick::Random,
                SlotControl::Human {
                    nick: NICKS[1].into(),
                },
            ),
            slot(2, CivPick::Random, SlotControl::Ai),
            slot(3, CivPick::Random, SlotControl::Ai),
        ],
    }
}

pub fn cfg() -> WorkerConfig {
    WorkerConfig {
        audit_fog: true,
        ..WorkerConfig::default()
    }
}

pub fn init(start: Start, save_dir: &str) -> WorkerInit {
    WorkerInit {
        ipc: IPC_VERSION,
        game: 9,
        core: CoreKind::Real {
            gamedata_dir: gamedata_dir().display().to_string(),
        },
        start,
        save_dir: save_dir.into(),
        seed: 77,
        cfg: cfg(),
    }
}

/// The worker and what came out of it.
pub struct H {
    pub w: Worker<RealCore>,
    pub now: u64,
    pub jobs: Vec<SaveJob>,
    pub frames: Vec<WorkerFrame>,
    pub exit: Option<u8>,
    pub next_id: BTreeMap<PlayerId, u32>,
    /// Jobs the "save thread" has not answered yet.
    pub unanswered: Vec<SaveDone>,
}

impl H {
    pub fn with(w: Worker<RealCore>, outs: Vec<Out>) -> Self {
        let mut h = H {
            w,
            now: 0,
            jobs: Vec::new(),
            frames: Vec::new(),
            exit: None,
            next_id: BTreeMap::new(),
            unanswered: Vec::new(),
        };
        h.feed(outs);
        h.run();
        h
    }

    pub fn feed(&mut self, outs: Vec<Out>) {
        for o in outs {
            match o {
                Out::Frame(f) => {
                    for m in &f.msgs {
                        if let WorkerMsg::Fatal { message, code } = m {
                            panic!("fatal {code}: {message}");
                        }
                    }
                    self.frames.push(f);
                }
                Out::Save(j) => {
                    // The save thread writes the file at once.
                    self.unanswered.push(SaveDone {
                        file: j.file,
                        saved_at_ms: 1_000 + j.file.seq,
                        result: Ok(j.gcs1.len() as u64),
                    });
                    self.jobs.push(*j);
                }
                // The replica stream has its own test (`real_replica.rs`).
                Out::Replica(_) => {}
                Out::Exit(c) => self.exit = Some(c),
            }
        }
    }

    pub fn run(&mut self) {
        for _ in 0..50_000 {
            while let Some(d) = self.unanswered.pop() {
                self.now += 1;
                let outs = self.w.save_done(d, self.now);
                self.feed(outs);
            }
            if !self.w.wants_step() {
                return;
            }
            self.now += 1;
            let outs = self.w.step(self.now);
            self.feed(outs);
        }
        panic!("the worker never went idle");
    }

    pub fn send(&mut self, msg: ToWorker) {
        self.now += 1;
        let outs = self.w.handle(msg, self.now);
        self.feed(outs);
    }

    pub fn msgs(&self) -> impl Iterator<Item = &WorkerMsg> {
        self.frames.iter().flat_map(|f| f.msgs.iter())
    }

    /// Gives the order and runs on; returns what the core answered.
    pub fn cmd(&mut self, player: PlayerId, cmd: Command) -> CommandOutcome {
        let turn = self.w.phase().0;
        let n = self.next_id.entry(player).or_insert(0);
        *n += 1;
        let id = *n;
        let before = self.frames.len();
        self.send(ToWorker::Cmd {
            player,
            cmd: Cmd { id, turn, cmd },
        });
        self.run();
        self.frames[before..]
            .iter()
            .flat_map(|f| f.msgs.iter())
            .filter_map(|m| match m {
                WorkerMsg::Results { player: p, results } if *p == player => Some(results),
                _ => None,
            })
            .flatten()
            .rev()
            .find(|r| r.id == id)
            .map(|r| r.outcome.clone())
            .unwrap_or_else(|| panic!("no answer to order {id} of seat {player}"))
    }

    /// The seat ends its turn. A city that finished what it built holds the end of turn (that is the core's
    /// rule); the player then picks the next build, as a client would.
    pub fn end_turn(&mut self, player: PlayerId) {
        for _ in 0..8 {
            match self.cmd(player, Command::EndTurn { force: true }) {
                CommandOutcome::Applied => return,
                CommandOutcome::Rejected { reason, .. } if reason.contains("PRODUCTION") => {
                    self.set_idle_productions(player);
                }
                CommandOutcome::Rejected { reason, .. } if reason.contains("need orders") => {
                    self.rest_idle_units(player);
                }
                // Two units on one plot: the one the core names is disbanded (a bot's way out).
                CommandOutcome::Rejected { reason, .. } if reason.contains("STACKED_UNITS") => {
                    let unit = reason
                        .split("unit ")
                        .nth(1)
                        .and_then(|t| t.split_whitespace().next())
                        .and_then(|n| n.parse::<i32>().ok())
                        .unwrap_or_else(|| panic!("no unit in {reason:?}"));
                    self.cmd(player, Command::UnitDisband { unit });
                }
                other => panic!("end of turn of seat {player}: {other:?}"),
            }
        }
        panic!("seat {player} cannot end the turn");
    }

    /// Gives orders to the units of the seat that wait for them.
    pub fn rest_idle_units(&mut self, player: PlayerId) {
        let view = self.w.fresh_view(player).unwrap();
        for u in view
            .units
            .iter()
            .filter(|u| u.owner == player && u.flags.contains(protocol::UnitFlags::NEEDS_ORDERS))
        {
            // What a unit may do depends on the unit: fortify, else sleep, else wait a turn.
            for cmd in [
                Command::UnitFortify { unit: u.id },
                Command::UnitSleep { unit: u.id },
                Command::UnitSkip { unit: u.id },
            ] {
                if self.cmd(player, cmd) == CommandOutcome::Applied {
                    break;
                }
            }
        }
    }

    /// A build for every city of the seat that builds nothing.
    pub fn set_idle_productions(&mut self, player: PlayerId) {
        let view = self.w.fresh_view(player).unwrap();
        for c in view
            .cities
            .iter()
            .filter(|c| c.owner == player && c.production_order < 0)
        {
            let build = self.pick_build(player, c.id);
            let r = self.cmd(
                player,
                Command::CitySetProduction {
                    city: c.id,
                    order: build.order_type,
                    item: build.item,
                },
            );
            assert_eq!(r, CommandOutcome::Applied, "set the production");
        }
    }

    pub fn pick_build(&mut self, player: PlayerId, city: i32) -> protocol::CityBuildView {
        self.w
            .core_mut()
            .city_builds(player, city)
            .unwrap()
            .into_iter()
            .find(|b| b.flags.contains(protocol::CityBuildFlags::ENABLED))
            .expect("something to build")
    }

    /// Both humans end the turn; the AI moves; the next turn starts.
    pub fn next_turn(&mut self, humans: &[PlayerId]) {
        let turn = self.w.phase().0;
        for &p in humans {
            self.end_turn(p);
        }
        self.run();
        assert_eq!(self.w.phase(), (turn + 1, TurnPhase::Humans));
    }

    /// The hash of the view the worker holds for each seat.
    pub fn hashes(&self, humans: &[PlayerId]) -> Vec<u64> {
        humans
            .iter()
            .map(|&p| self.w.view_hash(p).expect("a view"))
            .collect()
    }

    /// The view built from the diffs is the view built from scratch, for every seat; no fog violations.
    pub fn check_views(&mut self, humans: &[PlayerId]) {
        for &p in humans {
            let fresh = self.w.fresh_view(p).unwrap();
            assert_eq!(
                self.w.view_hash(p),
                Some(ViewHash::of(&fresh).value()),
                "seat {p}, turn {}",
                fresh.turn
            );
        }
        assert_eq!(self.w.fog_violations(), 0, "fog violations");
    }

    pub fn found_city(&mut self, player: PlayerId) {
        let view = self.w.fresh_view(player).unwrap();
        let settler = view
            .units
            .iter()
            .filter(|u| u.owner == player)
            .find(|u| {
                self.w
                    .core_mut()
                    .unit_actions(player, u.id)
                    .is_ok_and(|a| a.iter().any(|a| a.action == 3 && a.enabled))
            })
            .unwrap_or_else(|| panic!("seat {player} has no settler"))
            .id;
        let r = self.cmd(player, Command::UnitFoundCity { unit: settler });
        assert_eq!(r, CommandOutcome::Applied, "found a city");
        // A city without a build holds the end of turn.
        let view = self.w.fresh_view(player).unwrap();
        let city = view
            .cities
            .iter()
            .find(|c| c.owner == player)
            .expect("the new city")
            .id;
        let build = self
            .w
            .core_mut()
            .city_builds(player, city)
            .unwrap()
            .into_iter()
            .find(|b| b.flags.contains(protocol::CityBuildFlags::ENABLED))
            .expect("something to build");
        let r = self.cmd(
            player,
            Command::CitySetProduction {
                city,
                order: build.order_type,
                item: build.item,
            },
        );
        assert_eq!(r, CommandOutcome::Applied, "set the production");
        let v = self.w.fresh_view(player).unwrap();
        let c = v.cities.iter().find(|c| c.id == city).unwrap();
        assert_eq!(
            (c.production_order, c.production_item),
            (build.order_type, build.item),
            "the city builds what it was told: {build:?} {c:?}"
        );
    }
}
