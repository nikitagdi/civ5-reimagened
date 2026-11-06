//! The worker shell on the real core (`civ5-server worker` with `CoreKind::Real`, game-host-mp section 9
//! "Тесты на настоящем мозге", N3.31): worker processes on the game data of the export, Linux/WSL only.
//!
//! * AC-N1.4: two worker processes with one seed, one without query noise and one with it (at least 10 000
//!   queries between the orders), 50 turns: the digests of the turn ends are equal.
//! * AC-N4.3: a Huge map with eight humans starts and every seat gets its own view.
//!
//! The tests return at once where there is no real core: on Windows, or when the build did not link a gamecore
//! library (`cargo xtask test -p server --gamecore-lib <dir> --test worker_real` sets
//! `GAMECORE_LIB_DIR_x86_64`). The game data is `CIV5_GAMEDATA` or `assets-out/gamedata-b2` above the checkout.

use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command as Process, Stdio};
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use protocol::{
    AutoChoices, CivPick, Cmd, Codec, Command, CommandOutcome, CoreKind, GameSettings, GameSetup,
    IPC_VERSION, PlayerId, Postcard, Query, QueryAnswer, SlotControl, SlotSetup, Start, TimerMode,
    ToWorker, TurnMode, TurnPhase, UnitFlags, ViewUpdate, WorkerConfig, WorkerFrame, WorkerInit,
    WorkerMsg,
};
use wire::ipc::{FrameReader, write_frame};

const BIN: &str = env!("CARGO_BIN_EXE_civ5-server");
/// The longest wait for one message: a Huge map is made in half a minute or more.
const WAIT: Duration = Duration::from_secs(600);
/// The processes of the tests are heavy: one at a time.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn have_real_core() -> bool {
    if cfg!(windows) || std::env::var_os("GAMECORE_LIB_DIR_x86_64").is_none() {
        eprintln!("skipped: no real gamecore here");
        return false;
    }
    true
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

fn setup(world_size: &str, humans: usize, ais: usize) -> GameSetup {
    let slot = |i: usize, control| SlotSetup {
        player: i as i8,
        civ: CivPick::Random,
        control,
    };
    let slots = (0..humans)
        .map(|i| {
            slot(
                i,
                SlotControl::Human {
                    nick: format!("p{i}"),
                },
            )
        })
        .chain((humans..humans + ais).map(|i| slot(i, SlotControl::Ai)))
        .collect();
    GameSetup {
        settings: GameSettings {
            name: "real".into(),
            world_size: world_size.into(),
            map_script: "Assets/Maps/Lekmap v5.2/LekmapPangaeaFractalv5.2.lua".into(),
            map_options: vec![],
            speed: "GAMESPEED_STANDARD".into(),
            start_era: "ERA_ANCIENT".into(),
            handicap: "HANDICAP_CHIEFTAIN".into(),
            minor_civs: 0,
            turn_mode: TurnMode::Hybrid,
            timer: TimerMode::Off,
            game_options: vec![],
            seed: Some(4242),
            map_seed: None,
            auto_choices: AutoChoices::default(),
            diplo_auto_decline: true,
            mods: Vec::new(),
        },
        slots,
    }
}

fn init(game: GameSetup, cfg: WorkerConfig) -> WorkerInit {
    WorkerInit {
        ipc: IPC_VERSION,
        game: 5,
        core: CoreKind::Real {
            gamedata_dir: gamedata_dir().display().to_string(),
        },
        start: Start::New(game),
        save_dir: String::new(),
        seed: 99,
        cfg,
    }
}

/// A worker process with its frames collected on a thread.
struct Proc {
    child: Child,
    frames: Receiver<WorkerFrame>,
    /// Every message seen so far, in order, and the number of the frame each came in.
    seen: Vec<WorkerMsg>,
    seen_frame: Vec<usize>,
    frames_read: usize,
    /// Messages of the frames read but not yet looked at by `wait_msg`.
    pending: VecDeque<WorkerMsg>,
    next_cmd: u32,
    next_qid: u64,
    /// The turn of the last `Turn` in the human phase.
    turn: i32,
}

impl Proc {
    fn start() -> Self {
        let mut child = Process::new(BIN)
            .arg("worker")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, frames) = mpsc::channel();
        std::thread::spawn(move || {
            let mut r = FrameReader::new(stdout);
            while let Ok(Some(body)) = r.next() {
                let frame: WorkerFrame = Postcard
                    .decode(&body)
                    .expect("the worker wrote something that is not a WorkerFrame");
                if tx.send(frame).is_err() {
                    return;
                }
            }
        });
        Proc {
            child,
            frames,
            seen: Vec::new(),
            seen_frame: Vec::new(),
            frames_read: 0,
            pending: VecDeque::new(),
            next_cmd: 0,
            next_qid: 0,
            turn: -1,
        }
    }

    fn send(&mut self, msg: &ToWorker) {
        let body = Postcard.encode(msg).unwrap();
        let stdin = self.child.stdin.as_mut().unwrap();
        write_frame(stdin, &body).unwrap();
        stdin.flush().unwrap();
    }

    /// Looks at the messages in order until one satisfies `pred` (the ones before it are dropped, the ones
    /// after it stay for the next call); returns that message. A `Fatal` ends the test.
    fn wait_msg(&mut self, what: &str, pred: impl Fn(&WorkerMsg) -> bool) -> WorkerMsg {
        let deadline = Instant::now() + WAIT;
        loop {
            while let Some(m) = self.pending.pop_front() {
                if let WorkerMsg::Fatal { message, code } = &m {
                    panic!("the worker is fatal ({code}) while waiting for {what}: {message}");
                }
                if pred(&m) {
                    return m;
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            let frame = self
                .frames
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("no {what} within {WAIT:?}"));
            self.frames_read += 1;
            self.seen_frame
                .extend(std::iter::repeat_n(self.frames_read, frame.msgs.len()));
            self.seen.extend(frame.msgs.iter().cloned());
            self.pending.extend(frame.msgs);
        }
    }

    fn exit_code(&mut self) -> Option<i32> {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.code();
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
                panic!("the worker did not exit");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn all_msgs(&mut self) -> Vec<WorkerMsg> {
        while let Ok(f) = self.frames.try_recv() {
            self.frames_read += 1;
            self.seen_frame
                .extend(std::iter::repeat_n(self.frames_read, f.msgs.len()));
            self.seen.extend(f.msgs);
        }
        self.seen.clone()
    }

    /// Waits for the human phase of any turn after `self.turn`.
    fn wait_humans(&mut self) -> i32 {
        let after = self.turn;
        let m = self.wait_msg(
            "the human phase of a new turn",
            |m| matches!(m, WorkerMsg::Turn(t) if t.phase == TurnPhase::Humans && t.turn > after),
        );
        let WorkerMsg::Turn(t) = m else {
            unreachable!()
        };
        self.turn = t.turn;
        t.turn
    }

    fn query(&mut self, player: PlayerId, q: Query) -> QueryAnswer {
        self.next_qid += 1;
        let qid = self.next_qid;
        self.send(&ToWorker::Query { player, qid, q });
        match self.wait_msg(
            "a query answer",
            |m| matches!(m, WorkerMsg::QueryResult { qid: id, .. } if *id == qid),
        ) {
            WorkerMsg::QueryResult { answer, .. } => answer,
            _ => unreachable!(),
        }
    }

    /// Gives an order and waits for the final answer.
    fn cmd(&mut self, player: PlayerId, cmd: Command) -> CommandOutcome {
        self.next_cmd += 1;
        let id = self.next_cmd;
        let turn = self.turn;
        self.send(&ToWorker::Cmd {
            player,
            cmd: Cmd { id, turn, cmd },
        });
        loop {
            let m = self.wait_msg("the answer to an order", |m| {
                matches!(m, WorkerMsg::Results { player: p, results }
                    if *p == player && results.iter().any(|r| r.id == id))
            });
            let WorkerMsg::Results { results, .. } = m else {
                unreachable!()
            };
            let r = results.into_iter().find(|r| r.id == id).unwrap();
            if r.outcome != CommandOutcome::Queued {
                return r.outcome;
            }
        }
    }

    /// The units of the seat that wait for orders get some.
    fn rest_idle_units(&mut self, player: PlayerId) {
        let QueryAnswer::Units(units) = self.query(player, Query::Units) else {
            panic!("no units")
        };
        for u in units
            .iter()
            .filter(|u| u.owner == player && u.flags.contains(UnitFlags::NEEDS_ORDERS))
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
    fn set_idle_productions(&mut self, player: PlayerId) {
        let QueryAnswer::Cities(cities) = self.query(player, Query::Cities) else {
            panic!("no cities")
        };
        for c in cities
            .iter()
            .filter(|c| c.owner == player && c.production_order < 0)
        {
            let QueryAnswer::CityBuilds(builds) =
                self.query(player, Query::CityBuilds { city: c.id })
            else {
                panic!("no builds")
            };
            let b = builds
                .iter()
                .find(|b| b.flags.contains(protocol::CityBuildFlags::ENABLED))
                .expect("something to build");
            self.cmd(
                player,
                Command::CitySetProduction {
                    city: c.id,
                    order: b.order_type,
                    item: b.item,
                },
            );
        }
    }

    /// The seat founds a city with a unit that can, if there is one.
    fn found_city(&mut self, player: PlayerId) {
        let QueryAnswer::Units(units) = self.query(player, Query::Units) else {
            panic!("no units")
        };
        for u in units.iter().filter(|u| u.owner == player) {
            let QueryAnswer::UnitActions(actions) =
                self.query(player, Query::UnitActions { unit: u.id })
            else {
                panic!("no actions")
            };
            if actions.iter().any(|a| a.action == 3 && a.enabled) {
                let r = self.cmd(player, Command::UnitFoundCity { unit: u.id });
                assert_eq!(r, CommandOutcome::Applied, "found a city");
                self.set_idle_productions(player);
                return;
            }
        }
        // The start plot of a seat may not allow a city (too near the sea or another city): it plays on
        // with units only.
        eprintln!("seat {player} cannot found a city at its start");
    }

    /// The seat ends its turn; what holds it back is set right, as a client would.
    fn end_turn(&mut self, player: PlayerId) {
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
}

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A game of two humans and two AI for `turns` turns; returns the digests of the turn ends.
fn play(noise: u32, turns: i32) -> Vec<u64> {
    let mut p = Proc::start();
    let cfg = WorkerConfig {
        digests: true,
        stats: true,
        audit_fog: true,
        query_noise: noise,
        ..WorkerConfig::default()
    };
    p.send(&ToWorker::Init(init(setup("WORLDSIZE_DUEL", 2, 2), cfg)));
    p.wait_msg("Ready", |m| matches!(m, WorkerMsg::Ready(_)));
    let first = p.wait_humans();
    for player in [0, 1] {
        p.found_city(player);
    }
    while p.turn < first + turns {
        for player in [0, 1] {
            p.end_turn(player);
        }
        p.wait_humans();
    }
    p.send(&ToWorker::Shutdown { save: false });
    assert_eq!(p.exit_code(), Some(0), "the worker exits cleanly");
    let msgs = p.all_msgs();
    let violations: u32 = msgs
        .iter()
        .filter_map(|m| match m {
            WorkerMsg::Stats(s) => Some(s.fog_violations),
            _ => None,
        })
        .sum();
    assert_eq!(violations, 0, "fog violations");
    msgs.iter()
        .filter_map(|m| match m {
            WorkerMsg::Stats(s) => s.digest,
            _ => None,
        })
        .collect()
}

/// AC-N1.4: queries change nothing. Both runs make the same orders in the same order; the second one asks
/// the core `noise` random questions after every order (at least 100 orders, so at least 10 000 queries).
#[test]
fn query_noise_does_not_change_the_game() {
    if !have_real_core() {
        return;
    }
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    const TURNS: i32 = 50;
    let quiet = play(0, TURNS);
    let noisy = play(100, TURNS);
    assert!(
        quiet.len() > TURNS as usize,
        "a digest per turn: {} of {TURNS}",
        quiet.len()
    );
    assert_eq!(quiet, noisy, "the digests of the turn ends differ");
}

/// AC-N4.3: Huge, eight humans and two AI start; every seat has its own view.
#[test]
fn a_huge_map_with_eight_humans_starts() {
    if !have_real_core() {
        return;
    }
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mut p = Proc::start();
    let cfg = WorkerConfig {
        audit_fog: true,
        stats: true,
        ..WorkerConfig::default()
    };
    p.send(&ToWorker::Init(init(setup("WORLDSIZE_HUGE", 8, 2), cfg)));
    let WorkerMsg::Ready(ready) = p.wait_msg("Ready", |m| matches!(m, WorkerMsg::Ready(_))) else {
        unreachable!()
    };
    assert_eq!(ready.seats.len(), 10);
    assert!(
        u32::from(ready.width) * u32::from(ready.height) >= 6_000,
        "{ready:?}"
    );
    p.wait_humans();
    let msgs = p.all_msgs();
    let mut hashes = Vec::new();
    for player in 0..8i8 {
        let full = msgs.iter().find_map(|m| match m {
            WorkerMsg::View {
                player: pl,
                update: ViewUpdate::Full { view, .. },
            } if *pl == player => Some(view),
            _ => None,
        });
        let view = full.unwrap_or_else(|| panic!("no Full view for seat {player}"));
        assert_eq!(view.human, player);
        assert!(view.units.iter().any(|u| u.owner == player));
        hashes.push(protocol::ViewHash::of(view).value());
    }
    // The seats see different worlds (each has its own start and its own fog).
    let mut distinct = hashes.clone();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(distinct.len(), 8, "every seat has a view of its own");
    let turn = msgs.iter().rev().find_map(|m| match m {
        WorkerMsg::Turn(t) => Some(t.clone()),
        _ => None,
    });
    assert_eq!(turn.expect("Turn").seats.len(), 8);
    let violations: u32 = msgs
        .iter()
        .filter_map(|m| match m {
            WorkerMsg::Stats(s) => Some(s.fog_violations),
            _ => None,
        })
        .sum();
    assert_eq!(violations, 0, "fog violations");
    p.send(&ToWorker::Shutdown { save: false });
    assert_eq!(p.exit_code(), Some(0));
}

// ------------------------------------------------------------------------------------------ replica stream

/// Game data for the replica of the test process.
struct DirHost(PathBuf);

impl gamecore::HostServices for DirHost {
    fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        if path.split(['/', '\\']).any(|p| p == "..") {
            return None;
        }
        std::fs::read(self.0.join(path)).ok()
    }

    fn game_event(&self, _ev: gamecore::GameEvent) {}
}

/// The client side of the stream in the test process.
struct RepClient {
    game: gamecore::replica::ReplicaGame,
    /// Index into `Proc::seen` of the next message.
    at: usize,
    bodies: std::collections::HashMap<u64, Vec<u8>>,
    /// The raw snapshot and its `raw_hash` the replica stands on (the base of the next patch).
    raw: Vec<u8>,
    raw_hash: u64,
    jseq: u64,
    checks: usize,
    nets: usize,
    patches_applied: usize,
    turns: usize,
    bases_for_me: usize,
    rejected: usize,
}

impl RepClient {
    fn new() -> Self {
        RepClient {
            game: gamecore::replica::ReplicaGame::new(DirHost(gamedata_dir()), Some(0)).unwrap(),
            at: 0,
            bodies: Default::default(),
            raw: Vec::new(),
            raw_hash: 0,
            jseq: 0,
            checks: 0,
            nets: 0,
            patches_applied: 0,
            turns: 0,
            bases_for_me: 0,
            rejected: 0,
        }
    }

    fn load(&mut self, raw: Vec<u8>, raw_hash: u64) {
        self.game.resync(&raw).unwrap();
        self.raw = raw;
        self.raw_hash = raw_hash;
    }

    /// Takes in what the worker said since the last call.
    fn feed(&mut self, p: &mut Proc) {
        use gamecore::replica::{Applied, JournalRecord, decode_journal};
        p.all_msgs();
        while self.at < p.seen.len() {
            let m = p.seen[self.at].clone();
            self.at += 1;
            match m {
                WorkerMsg::ReplicaBlob { hash, bytes } => {
                    assert_eq!(protocol::xxh64(0, &bytes), hash, "a body names itself");
                    self.bodies.insert(hash, bytes);
                }
                WorkerMsg::ReplicaBase {
                    jseq,
                    raw_hash,
                    snapshot,
                    for_player,
                    ..
                } => {
                    if for_player.is_some_and(|q| q != 0) {
                        continue;
                    }
                    let raw = wire::replica::decode_snapshot(&self.bodies[&snapshot.hash]).unwrap();
                    assert_eq!(protocol::xxh64(0, &raw), raw_hash);
                    if for_player.is_some() {
                        self.bases_for_me += 1;
                    }
                    self.load(raw, raw_hash);
                    self.jseq = jseq;
                }
                WorkerMsg::ReplicaJournal {
                    first_jseq,
                    batches,
                    ..
                } => {
                    for (i, b) in batches.iter().enumerate() {
                        let j = first_jseq + i as u64;
                        if j <= self.jseq {
                            continue;
                        }
                        assert_eq!(j, self.jseq + 1, "no batch is missing");
                        self.jseq = j;
                        let records = decode_journal(b).unwrap();
                        self.checks += records
                            .iter()
                            .filter(|r| matches!(r, JournalRecord::Check { .. }))
                            .count();
                        self.nets += records
                            .iter()
                            .filter(|r| matches!(r, JournalRecord::Net { player: 0, .. }))
                            .count();
                        assert_eq!(
                            self.game.apply(b).unwrap(),
                            Applied::InStep,
                            "batch {j} keeps the replica in step"
                        );
                    }
                }
                WorkerMsg::ReplicaTurn {
                    turn,
                    jseq,
                    raw_hash,
                    snapshot,
                    patch,
                    ..
                } => {
                    let full = &self.bodies[&snapshot.hash];
                    let raw = match patch {
                        Some(pt) if pt.from_raw_hash == self.raw_hash => {
                            let body = &self.bodies[&pt.blob.hash];
                            let t = Instant::now();
                            let new = wire::replica::apply_patch(body, &self.raw, self.raw_hash)
                                .expect("the patch applies");
                            eprintln!(
                                "turn {turn}: snapshot body {} bytes, patch {} bytes, applied in {:?}",
                                full.len(),
                                body.len(),
                                t.elapsed()
                            );
                            self.patches_applied += 1;
                            // The two ways give the same game.
                            assert_eq!(new, wire::replica::decode_snapshot(full).unwrap());
                            new
                        }
                        _ => wire::replica::decode_snapshot(full).unwrap(),
                    };
                    assert_eq!(protocol::xxh64(0, &raw), raw_hash);
                    self.load(raw, raw_hash);
                    self.jseq = jseq;
                    self.turns += 1;
                }
                WorkerMsg::NetRejected { player: 0, .. } => self.rejected += 1,
                _ => {}
            }
        }
    }
}

/// The replica stream on the real core: see the header of this file.
#[test]
fn the_replica_follows_the_server_through_four_turns() {
    if !have_real_core() {
        return;
    }
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mut rep = RepClient::new();
    let mut p = Proc::start();
    let cfg = WorkerConfig {
        stats: true,
        ..WorkerConfig::default()
    };
    p.send(&ToWorker::Init(init(setup("WORLDSIZE_DUEL", 2, 2), cfg)));
    p.wait_msg("Ready", |m| matches!(m, WorkerMsg::Ready(_)));
    let first = p.wait_humans();
    p.wait_msg("the base", |m| {
        matches!(
            m,
            WorkerMsg::ReplicaBase {
                for_player: None,
                ..
            }
        )
    });
    rep.feed(&mut p);
    assert!(rep.jseq == 0 && !rep.raw.is_empty(), "the base is loaded");
    for player in [0, 1] {
        p.found_city(player);
    }
    // A UI message of the seat: accepted, journaled as NET for everybody. The turn handshake is not one.
    use gamecore::replica::{NetMessage, encode_net};
    let mut pushed = 0;
    p.send(&ToWorker::Net {
        player: 0,
        msgs: encode_net(&[NetMessage::new("sendResearch", &[1, 0, -1, 0]).unwrap()]),
    });
    p.send(&ToWorker::Net {
        player: 0,
        msgs: encode_net(&[NetMessage::new("sendTurnComplete", &[]).unwrap()]),
    });
    p.wait_msg("the refusal", |m| {
        matches!(m, WorkerMsg::NetRejected { player: 0, .. })
    });
    // The city's production as the original UI orders it (`Game.CityPushOrder` -> sendPushOrder(city, ORDER_TRAIN,
    // unit, alt, shift, ctrl); live scenario m5-map-sync 2026-10-30): appended (ctrl, the scenario's order) and in
    // place of the queue (shift, the city screen's click), NET records the replica replays in step.
    let QueryAnswer::Cities(cities) = p.query(0, Query::Cities) else {
        panic!("no cities")
    };
    if let Some(city) = cities.iter().find(|c| c.owner == 0) {
        let QueryAnswer::CityBuilds(builds) = p.query(0, Query::CityBuilds { city: city.id })
        else {
            panic!("no builds")
        };
        let unit = builds
            .iter()
            .find(|b| b.order_type == 0 && b.flags.contains(protocol::CityBuildFlags::ENABLED))
            .expect("a unit to train");
        for (shift, ctrl) in [(0, 1), (1, 0)] {
            let order = [city.id, 0, i32::from(unit.item), 0, shift, ctrl];
            p.send(&ToWorker::Net {
                player: 0,
                msgs: encode_net(&[NetMessage::new("sendPushOrder", &order).unwrap()]),
            });
            // The batches ask for a CHECK at most once a second.
            std::thread::sleep(Duration::from_millis(1100));
            p.query(0, Query::Cities);
        }
        // The queue drag of the city screen (EUI `SwapQueueItem`: `GAMEMESSAGE_SWAP_ORDER`, i -> sendSwapOrder(city, i)):
        // two orders are in the queue (the shift push replaced it with one, a ctrl push appends the second), the swap
        // is accepted and journaled as NET, and the replica stays in step (no divergence at the end).
        let order = [city.id, 0, i32::from(unit.item), 0, 0, 1];
        p.send(&ToWorker::Net {
            player: 0,
            msgs: encode_net(&[NetMessage::new("sendPushOrder", &order).unwrap()]),
        });
        p.send(&ToWorker::Net {
            player: 0,
            msgs: encode_net(&[NetMessage::new("sendSwapOrder", &[city.id, 0]).unwrap()]),
        });
        std::thread::sleep(Duration::from_millis(1100));
        p.query(0, Query::Cities);
        pushed = 4;
    }
    // The view of the seat follows what the UI message changed (the research in the top bar): the 3D map of an
    // original-UI client is drawn from these views (live report 2026-10-30: units and cities stood still).
    let researching = |m: &WorkerMsg| {
        matches!(m, WorkerMsg::View { player: 0, update: ViewUpdate::Delta { diff, .. } }
            if diff.top_bar.as_ref().is_some_and(|t| t.research_tech == 1))
    };
    if !p.seen.iter().any(researching) {
        p.wait_msg("the view after the UI message", researching);
    }
    // A seat asks for a new base: it gets one for itself, a state after the batches so far.
    p.send(&ToWorker::ReplicaSnapshot { player: 0 });
    p.wait_msg("a base for the seat", |m| {
        matches!(
            m,
            WorkerMsg::ReplicaBase {
                for_player: Some(0),
                ..
            }
        )
    });
    p.end_turn(0);
    p.end_turn(1);
    p.wait_humans();
    p.wait_msg("a turn", |m| matches!(m, WorkerMsg::ReplicaTurn { .. }));
    rep.feed(&mut p);
    assert_eq!(rep.rejected, 1, "the handshake was refused");
    assert_eq!(rep.bases_for_me, 1, "the resync base");
    assert!(
        rep.nets > pushed,
        "the accepted UI messages are NET records"
    );
    assert!(rep.checks >= 2, "CHECK records: {}", rep.checks);
    assert_eq!(rep.turns, 1);
    while p.turn < first + 3 {
        for player in [0, 1] {
            p.end_turn(player);
        }
        p.wait_humans();
        p.wait_msg("a turn", |m| matches!(m, WorkerMsg::ReplicaTurn { .. }));
        rep.feed(&mut p);
    }
    assert!(
        rep.turns >= 3 && rep.patches_applied >= 2,
        "{} turns, {} patches",
        rep.turns,
        rep.patches_applied
    );
    let (batches, divergences, _) = rep.game.stats();
    assert_eq!(divergences, 0);
    eprintln!("replica: {batches} batches, {} CHECKs", rep.checks);
    p.send(&ToWorker::Shutdown { save: false });
    assert_eq!(p.exit_code(), Some(0));
}

/// A UI message of a seat that has ended the turn takes the end of turn back first and is applied (the exe's
/// `BroadcastGameplayMessage`). The replica's `CancelEndTurn` order comes behind it, but the worker hands the UI
/// messages to the core ahead of the orders of a step, and the core refuses a message of a finished seat: the first
/// move after END TURN was lost (live report 2026-10-31).
#[test]
fn a_ui_message_after_the_end_of_turn_takes_it_back_and_applies() {
    if !have_real_core() {
        return;
    }
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mut p = Proc::start();
    p.send(&ToWorker::Init(init(
        setup("WORLDSIZE_DUEL", 2, 2),
        WorkerConfig::default(),
    )));
    p.wait_msg("Ready", |m| matches!(m, WorkerMsg::Ready(_)));
    p.wait_humans();
    for player in [0, 1] {
        p.found_city(player);
    }
    p.end_turn(0);
    use gamecore::replica::{NetMessage, encode_net};
    p.send(&ToWorker::Net {
        player: 0,
        msgs: encode_net(&[NetMessage::new("sendResearch", &[1, 0, -1, 0]).unwrap()]),
    });
    // The replica's own CancelEndTurn, behind it: nothing left to take back, and not refused.
    assert_eq!(p.cmd(0, Command::CancelEndTurn), CommandOutcome::Applied);
    let researching = |m: &WorkerMsg| {
        matches!(m, WorkerMsg::View { player: 0, update: ViewUpdate::Delta { diff, .. } }
            if diff.top_bar.as_ref().is_some_and(|t| t.research_tech == 1))
    };
    if !p.seen.iter().any(researching) {
        p.wait_msg("the view after the UI message", researching);
    }
    assert!(
        !p.seen
            .iter()
            .any(|m| matches!(m, WorkerMsg::NetRejected { player: 0, .. })),
        "the UI message was refused"
    );
    p.send(&ToWorker::Shutdown { save: false });
    assert_eq!(p.exit_code(), Some(0));
}

/// The last human's end of turn reaches the replicas before the AI phase runs (live report 2026-10-31): the checked batch
/// that holds it goes out in a frame of its own, before the frame of the pump that runs the AI (one core call of 10-20 s
/// in a late game, during which the worker sends nothing). Before, both were one frame after the pump: the clients did
/// not know that everybody was done, and what they clicked meanwhile flipped the END TURN button.
#[test]
fn the_last_end_of_turn_goes_out_before_the_ai_runs() {
    if !have_real_core() {
        return;
    }
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mut p = Proc::start();
    p.send(&ToWorker::Init(init(
        setup("WORLDSIZE_DUEL", 2, 2),
        WorkerConfig::default(),
    )));
    p.wait_msg("Ready", |m| matches!(m, WorkerMsg::Ready(_)));
    let t0 = p.wait_humans();
    p.wait_msg("the base", |m| matches!(m, WorkerMsg::ReplicaBase { .. }));
    for player in [0, 1] {
        p.found_city(player);
    }
    p.end_turn(0);
    let from = p.seen.len();
    p.end_turn(1);
    p.wait_humans();
    p.wait_msg("a turn", |m| matches!(m, WorkerMsg::ReplicaTurn { .. }));
    let msgs = &p.seen[from..];
    let frames = &p.seen_frame[from..];
    let turn_at = msgs
        .iter()
        .position(|m| matches!(m, WorkerMsg::ReplicaTurn { .. }))
        .expect("the next turn's snapshot");
    // The last batch of the human phase (the one with the last END_TURN, before the next turn's snapshot).
    let batch = msgs[..turn_at]
        .iter()
        .rposition(|m| matches!(m, WorkerMsg::ReplicaJournal { .. }))
        .expect("the batch of the last end of turn");
    let ai = msgs
        .iter()
        .position(
            |m| matches!(m, WorkerMsg::Turn(t) if t.turn > t0 || t.phase != TurnPhase::Humans),
        )
        .expect("the AI phase or the next turn");
    assert!(
        frames[batch] < frames[ai],
        "the batch came in frame {}, the AI phase in frame {}",
        frames[batch],
        frames[ai]
    );
    p.send(&ToWorker::Shutdown { save: false });
    assert_eq!(p.exit_code(), Some(0));
}

/// The load of the replica stream on a late Huge game (`CIV5_LATE_HUGE=<a .c5sv of the B2 save late-huge>`): the
/// worker loads the save, then plays turns; the numbers of the base, the turn bodies and patches, and the CPU time
/// of the worker process per turn go to stderr (WO-2 report). Not a pass/fail test beyond "it works".
#[test]
fn late_huge_replica_load() {
    let Some(file) = std::env::var_os("CIV5_LATE_HUGE") else {
        eprintln!("skipped: CIV5_LATE_HUGE is not set");
        return;
    };
    if !have_real_core() {
        return;
    }
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let turns: i32 = std::env::var("CIV5_LATE_TURNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let cpu_ms = |pid: u32| -> f64 {
        // utime + stime of the worker process, clock ticks of 10 ms.
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        let rest = stat.rsplit_once(')').map_or("", |x| x.1);
        let f: Vec<&str> = rest.split_whitespace().collect();
        let t = |i: usize| f.get(i).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
        (t(11) + t(12)) * 10.0
    };
    let slots: Vec<SlotSetup> = (0..12)
        .map(|i| SlotSetup {
            player: i as i8,
            civ: CivPick::Random,
            control: if i < 2 {
                SlotControl::Human {
                    nick: format!("late{}", i + 1),
                }
            } else {
                SlotControl::Ai
            },
        })
        .collect();
    let mut p = Proc::start();
    let pid = p.child.id();
    let cfg = WorkerConfig {
        stats: true,
        ..WorkerConfig::default()
    };
    let mut ini = init(setup("WORLDSIZE_HUGE", 2, 10), cfg);
    ini.start = Start::Load {
        file: file.to_string_lossy().into_owned(),
        seats: slots,
    };
    let t0 = Instant::now();
    p.send(&ToWorker::Init(ini));
    p.wait_msg("Ready", |m| matches!(m, WorkerMsg::Ready(_)));
    let first = p.wait_humans();
    p.wait_msg("the base", |m| {
        matches!(
            m,
            WorkerMsg::ReplicaBase {
                for_player: None,
                ..
            }
        )
    });
    eprintln!(
        "LATE: loaded to the base in {:?}, cpu {:.0} ms, turn {first}",
        t0.elapsed(),
        cpu_ms(pid)
    );
    let mut rep = RepClient::new();
    let t = Instant::now();
    rep.feed(&mut p);
    eprintln!("LATE: replica loaded the base in {:?}", t.elapsed());
    for _ in 0..turns {
        let cpu = cpu_ms(pid);
        let t = Instant::now();
        for player in [0, 1] {
            p.end_turn(player);
        }
        p.wait_humans();
        p.wait_msg("a turn", |m| matches!(m, WorkerMsg::ReplicaTurn { .. }));
        // The pack thread may still be busy with a patch: the message comes after it.
        let wall = t.elapsed();
        eprintln!(
            "LATE: turn {} wall {:?} (orders + AI + snapshot + pack), worker cpu {:.0} ms",
            p.turn,
            wall,
            cpu_ms(pid) - cpu
        );
        rep.feed(&mut p);
    }
    eprintln!(
        "LATE: {} patches applied of {} turns",
        rep.patches_applied, rep.turns
    );
    p.send(&ToWorker::Shutdown { save: false });
    assert_eq!(p.exit_code(), Some(0));
}

// ------------------------------------------------------------------------------------------ live saves

/// The UI messages of `CIV5_LIVE_NETS` (`name:a,b,c;name:...`, the `gDLL->send*` names and their leading int
/// arguments, as the replica sends them in `GcNet`).
fn live_nets(spec: &str) -> Vec<gamecore::replica::NetMessage> {
    spec.split(';')
        .filter(|s| !s.trim().is_empty())
        .map(|m| {
            let (name, args) = m.trim().split_once(':').unwrap_or((m.trim(), ""));
            let args: Vec<i32> = args
                .split(',')
                .filter(|a| !a.trim().is_empty())
                .map(|a| {
                    a.trim()
                        .parse()
                        .unwrap_or_else(|_| panic!("{m:?}: not an int"))
                })
                .collect();
            gamecore::replica::NetMessage::new(name, &args)
                .unwrap_or_else(|| panic!("{m:?}: no such net message"))
        })
        .collect()
}

/// Triage of a live crash: `CIV5_LIVE_SAVE=<a .c5sv of the server>` is loaded with the seats it names (humans
/// keep their nicks), `CIV5_LIVE_NETS` go to the worker as the UI messages of seat `CIV5_LIVE_PLAYER` (default 0),
/// and the worker must still be alive 5 s later and shut down cleanly. Game 925 (2026-10-30, the worker died on
/// "negotiate peace" with an AI at war, the trade button of LeaderHeadRoot; core scenario `diplo` is the regression
/// test): `CIV5_LIVE_NETS="sendFromUIDiploEvent:1,1,0,0"` on its turn-148 ring save.
#[test]
fn live_save_takes_ui_messages() {
    let Some(file) = std::env::var_os("CIV5_LIVE_SAVE") else {
        eprintln!("skipped: CIV5_LIVE_SAVE is not set");
        return;
    };
    if !have_real_core() {
        return;
    }
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let bytes = std::fs::read(&file).expect("CIV5_LIVE_SAVE");
    let info = game_host::saves::read_header(&bytes).expect("a C5SV save");
    let player: PlayerId = std::env::var("CIV5_LIVE_PLAYER")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let nets = live_nets(&std::env::var("CIV5_LIVE_NETS").unwrap_or_default());
    let mut p = Proc::start();
    let mut ini = init(setup("WORLDSIZE_DUEL", 1, 1), WorkerConfig::default());
    ini.start = Start::Load {
        file: file.to_string_lossy().into_owned(),
        seats: info.seats.clone(),
    };
    p.send(&ToWorker::Init(ini));
    p.wait_msg("Ready", |m| matches!(m, WorkerMsg::Ready(_)));
    p.wait_humans();
    for m in &nets {
        eprintln!("LIVE: seat {player} sends {} {:?}", m.name(), &m.args[..4]);
    }
    p.send(&ToWorker::Net {
        player,
        msgs: gamecore::replica::encode_net(&nets),
    });
    std::thread::sleep(Duration::from_secs(5));
    let st = p.child.try_wait().unwrap();
    assert!(st.is_none(), "the worker died on the UI messages: {st:?}");
    p.send(&ToWorker::Shutdown { save: false });
    assert_eq!(p.exit_code(), Some(0));
}

/// Live game 936 (2026-10-31, turn 126, a Huge map of 12 seats): the END TURN of the last human ran the whole AI
/// phase in one core call, with no frame for longer than the gateway's 30 s, and the worker was killed on every
/// restart. `CIV5_LIVE_SAVE` (its ring save 00000235-t0126) is loaded, every human's turn is ended by the timer, and
/// the worker must bring the next turn's humans up with no gap between two frames longer than `MAX_GAP` (the watch
/// of the core thread beats while a call runs, `worker/watch.rs`).
#[test]
fn a_live_save_ends_its_turn_without_falling_silent() {
    const MAX_GAP: Duration = Duration::from_secs(5);
    let Some(file) = std::env::var_os("CIV5_LIVE_SAVE") else {
        eprintln!("skipped: CIV5_LIVE_SAVE is not set");
        return;
    };
    if !have_real_core() {
        return;
    }
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let bytes = std::fs::read(&file).expect("CIV5_LIVE_SAVE");
    let info = game_host::saves::read_header(&bytes).expect("a C5SV save");
    let humans: Vec<PlayerId> = info
        .seats
        .iter()
        .filter(|s| matches!(s.control, SlotControl::Human { .. }))
        .map(|s| s.player)
        .collect();
    let mut p = Proc::start();
    let mut ini = init(setup("WORLDSIZE_DUEL", 1, 1), WorkerConfig::default());
    ini.start = Start::Load {
        file: file.to_string_lossy().into_owned(),
        seats: info.seats.clone(),
    };
    p.send(&ToWorker::Init(ini));
    p.wait_msg("Ready", |m| matches!(m, WorkerMsg::Ready(_)));
    let turn = p.wait_humans();
    // Whatever is still queued is taken before the end of turn.
    while p.frames.try_recv().is_ok() {}
    p.send(&ToWorker::ForceEndTurn { players: humans });
    let began = Instant::now();
    let mut longest = Duration::ZERO;
    let mut last = Instant::now();
    'turn: loop {
        let frame = p.frames.recv_timeout(MAX_GAP).unwrap_or_else(|_| {
            panic!(
                "the worker was silent for {MAX_GAP:?}, {:?} into the end of turn {turn}",
                began.elapsed()
            )
        });
        longest = longest.max(last.elapsed());
        last = Instant::now();
        for m in &frame.msgs {
            match m {
                WorkerMsg::Fatal { message, code } => panic!("fatal {code}: {message}"),
                WorkerMsg::Turn(t) if t.phase == TurnPhase::Humans && t.turn > turn => break 'turn,
                _ => {}
            }
        }
        assert!(began.elapsed() < WAIT, "turn {turn} did not end");
    }
    eprintln!(
        "turn {turn} ended in {:?}; the longest gap between two frames {longest:?}",
        began.elapsed()
    );
    p.send(&ToWorker::Shutdown { save: false });
    assert_eq!(p.exit_code(), Some(0));
}

impl Proc {
    /// A move of a unit of `player` to the plot of the move tree it reaches this turn with the most steps (a
    /// move the core times one pump slice per step, `CvUnitMission::CalculateMissionTimer`).
    fn far_move(&mut self, player: PlayerId) -> Option<Command> {
        let QueryAnswer::Units(units) = self.query(player, Query::Units) else {
            panic!("no units")
        };
        let mut best: Option<(usize, Command)> = None;
        for u in units
            .iter()
            .filter(|u| u.owner == player && u.moves_left > 0)
        {
            let QueryAnswer::Reach(tree) = self.query(
                player,
                Query::Reach {
                    unit: u.id,
                    horizon: 1,
                },
            ) else {
                continue;
            };
            for (i, n) in tree.iter().enumerate().skip(1) {
                if n.turn != 0 {
                    continue;
                }
                let mut depth = 0;
                let mut at = i;
                while at != 0 && depth < 64 {
                    at = usize::from(tree[at].parent);
                    depth += 1;
                }
                if best.as_ref().is_none_or(|(d, _)| depth > *d) {
                    best = Some((
                        depth,
                        Command::UnitMoveTo {
                            unit: u.id,
                            x: n.x,
                            y: n.y,
                        },
                    ));
                }
            }
        }
        best.map(|(_, c)| c)
    }
}

/// Live game 931 (2026-10-30, Simultaneous, 3 humans): a human moved a unit and ended the turn at once. The pump
/// after the move stopped at the first slice because the other humans were up, the unit was still "busy" (the
/// core times a move one slice per step) and nothing pumped again: END_TURN was refused with "a unit / city is
/// busy; pump first" and the turn hung, and `gc_save` refused every save ("not at a save point") in a hot loop
/// that ended in a fatal error. The end of turn right after a move is taken, and a save asked for right after a
/// move is made.
#[test]
fn a_move_holds_neither_the_end_of_turn_nor_a_save() {
    if !have_real_core() {
        return;
    }
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join(format!("civ5-busy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut game = setup("WORLDSIZE_DUEL", 3, 1);
    game.settings.turn_mode = TurnMode::Simultaneous;
    let mut ini = init(game, WorkerConfig::default());
    ini.save_dir = dir.display().to_string();
    let mut p = Proc::start();
    p.send(&ToWorker::Init(ini));
    p.wait_msg("Ready", |m| matches!(m, WorkerMsg::Ready(_)));
    p.wait_humans();

    let mv = p.far_move(0).expect("a unit of seat 0 that can move");
    eprintln!("seat 0: {mv:?}");
    assert_eq!(p.cmd(0, mv), CommandOutcome::Applied, "the move");
    let end = p.cmd(0, Command::EndTurn { force: true });
    assert_eq!(
        end,
        CommandOutcome::Applied,
        "the end of turn right after a move"
    );

    let mv = p.far_move(1).expect("a unit of seat 1 that can move");
    eprintln!("seat 1: {mv:?}");
    assert_eq!(p.cmd(1, mv), CommandOutcome::Applied, "the move");
    p.send(&ToWorker::SaveNow {
        reason: protocol::SaveReason::Admin,
    });
    let m = p.wait_msg("the save or its failure", |m| {
        matches!(m, WorkerMsg::Saved(_) | WorkerMsg::SaveFailed { .. })
    });
    assert!(
        matches!(m, WorkerMsg::Saved(_)),
        "a save right after a move: {m:?}"
    );
    p.send(&ToWorker::Shutdown { save: false });
    assert_eq!(p.exit_code(), Some(0), "the worker exits cleanly");
    let _ = std::fs::remove_dir_all(&dir);
}
