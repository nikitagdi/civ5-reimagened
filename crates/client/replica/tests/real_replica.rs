//! The client's `Replica` on the real Lekmod gamecore (WO-3, B4): a snapshot from a server -> `Replica` -> journal
//! batches -> replica digest; a turn change by full snapshot (B) and by patch (A) -> digest equal; the numbers of
//! AC-G4 (create, join, memory, turn switch, traffic).
//!
//! Returns early on the stub. Run on Linux/WSL with `GAMECORE_LIB_DIR_x86_64` and `CIV5_GAMEDATA` set:
//!
//! ```text
//! cargo test -p replica --test real_replica                          # Tiny, a few seconds
//! cargo test -p replica --test real_replica -- --ignored small       # Small, 2 bots + 4 AI
//! CIV5_LATE_HUGE=<.c5sv> cargo test -p replica --test real_replica -- --ignored late_huge
//! ```
//!
//! The core is a process singleton and a game cannot be created twice in one process, so each scenario runs its
//! server in a child process (this binary again, `REPLICA_SIM_OUT=<dir>`): it plays the server (`Role::Server`),
//! and writes the snapshots, the human-phase journal batches and the digests to the directory. The parent is the
//! client. `REPLICA_REPORT=<file>` writes the metrics as JSON lines.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::time::Instant;

use gamecore::{
    Command, Game, GameEvent, HostServices, Phase, Role, Setup, SlotOp, SlotSetup,
    replica::decode_journal,
};
use platform::Clock;
use protocol::xxh64;
use serde_json::{Value, json};

mod common;

const SCENARIO_ENV: &str = "REPLICA_SIM_OUT";

// ---------------------------------------------------------------- the server (child process)

struct DirHost(PathBuf);

impl HostServices for DirHost {
    fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        if path.split(['/', '\\']).any(|p| p == "..") {
            return None;
        }
        std::fs::read(self.0.join(path)).ok()
    }

    fn game_event(&self, _ev: GameEvent) {}
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

#[derive(Clone, Debug)]
struct Scenario {
    name: &'static str,
    /// New game: world size, bots (humans played by the test bot), AI, turns to play before the first snapshot.
    world: &'static str,
    bots: u8,
    ai: u8,
    warmup_turns: i32,
    /// A saved game to continue instead (`.c5sv` of the server): its two humans end their turns by command.
    save: Option<PathBuf>,
    /// Turn changes to record (snapshots = turns + 1).
    turns: usize,
    /// Compare the digest after every batch (cheap games) or only after the last one of a turn.
    every_batch: bool,
}

fn scenario(name: &str) -> Scenario {
    match name {
        "tiny" => Scenario {
            name: "tiny",
            world: "WORLDSIZE_TINY",
            bots: 2,
            ai: 2,
            warmup_turns: 0,
            save: None,
            turns: 3,
            every_batch: true,
        },
        "small" => Scenario {
            name: "small",
            world: "WORLDSIZE_SMALL",
            bots: 2,
            ai: 4,
            warmup_turns: 30,
            save: None,
            turns: 3,
            every_batch: true,
        },
        "late-huge" => Scenario {
            name: "late-huge",
            world: "",
            bots: 0,
            ai: 0,
            warmup_turns: 0,
            save: Some(late_huge_save()),
            turns: 2,
            every_batch: false,
        },
        other => panic!("unknown scenario {other}"),
    }
}

fn late_huge_save() -> PathBuf {
    if let Some(p) = std::env::var_os("CIV5_LATE_HUGE") {
        return PathBuf::from(p);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .map(|a| a.join("assets-out/saves/late-huge/state/games/1/keep/00000151-t0150.c5sv"))
        .find(|p| p.is_file())
        .expect("the late-huge save: set CIV5_LATE_HUGE=<.c5sv>")
}

fn hex(v: u64) -> String {
    format!("{v:016x}")
}

/// The server: plays the scenario and writes `index.txt` and the files it names into `out`.
fn simulate(sc: &Scenario, out: &Path) {
    std::fs::create_dir_all(out).unwrap();
    let mut game = Game::new(DirHost(gamedata_dir())).unwrap();
    let mut humans: Vec<u8> = Vec::new();
    let end_by_command;
    if let Some(save) = &sc.save {
        let file = std::fs::read(save).expect("the save");
        let saved = game_host_gcs1(&file);
        game.load(&saved).unwrap();
        humans = game
            .turn_state()
            .unwrap()
            .iter()
            .filter(|s| s.flags.human())
            .map(|s| s.player)
            .collect();
        for (&h, nick) in humans.iter().zip(["late1", "late2", "late3"]) {
            game.slot_control(h, SlotOp::Connect, Some(nick)).unwrap();
        }
        // The connects are queued commands: pump until the humans are up again.
        for _ in 0..5000 {
            let r = game.pump(64).unwrap();
            if r.status == 0 {
                break;
            }
        }
        end_by_command = true;
    } else {
        let mut slots: Vec<SlotSetup> = (0..sc.bots)
            .map(|i| SlotSetup::human("random", format!("bot{i}")))
            .collect();
        slots.extend((0..sc.ai).map(|_| SlotSetup::ai("random")));
        let mut setup = Setup::network(77, slots);
        setup.world_size = sc.world.into();
        setup.handicap = "HANDICAP_CHIEFTAIN".into();
        setup.autoplay_humans = (0..sc.bots).collect();
        game.new_game(&setup).unwrap();
        end_by_command = false;
    }
    game.set_role(Role::Server).unwrap();

    let mut index = String::new();
    let mut jseq: u64 = 0;
    let mut snaps = 0usize;

    // Up to a save point in a human phase once the warm-up is over (the bots end their turns inside the pump, so the
    // core is never "waiting for a human"; a saved game stands at its turn already).
    let mut turn_seen = game.game_info().unwrap().turn;
    if !end_by_command {
        let mut guard = 0;
        loop {
            game.pump(64).unwrap();
            let _ = game.journal_take(false).unwrap();
            guard += 1;
            assert!(guard < 200_000, "no save point");
            let info = game.game_info().unwrap();
            if info.phase == Phase::Humans && info.turn >= sc.warmup_turns && game.save().is_ok() {
                turn_seen = info.turn;
                break;
            }
        }
    }
    let record_snapshot = |game: &mut Game, index: &mut String, snaps: &mut usize, jseq: u64| {
        let raw = game.save().expect("a save point");
        let digest = game
            .state_digest(gamecore::replica::REPLICA_DIGEST)
            .unwrap();
        let turn = game.game_info().unwrap().turn;
        let file = format!("snap-{snaps}.gcs1");
        std::fs::write(out.join(&file), &raw).unwrap();
        writeln!(index, "snap {snaps} {turn} {jseq} {} {file}", hex(digest)).unwrap();
        *snaps += 1;
        turn
    };
    let _ = game.journal_take(false).unwrap();
    turn_seen = record_snapshot(&mut game, &mut index, &mut snaps, jseq).max(turn_seen);

    while snaps <= sc.turns {
        let mut guard = 0;
        loop {
            guard += 1;
            assert!(guard < 200_000, "the turn did not end");
            if end_by_command {
                for &h in &humans {
                    let ended = game
                        .turn_state()
                        .unwrap()
                        .iter()
                        .find(|s| s.player == h)
                        .is_some_and(|s| s.flags.turn_ended());
                    if !ended && game.game_info().unwrap().phase == Phase::Humans {
                        // One human per round: a queued command holds the next ("pump first").
                        game.command(h, &Command::ForceEndTurn).unwrap();
                        push_batch(&mut game, &mut jseq, snaps - 1, out, &mut index);
                        break;
                    }
                }
            }
            let before = game.game_info().unwrap().phase;
            let r = game.pump(64).unwrap();
            let info = game.game_info().unwrap();
            if before == Phase::Humans && info.phase == Phase::Humans {
                push_batch(&mut game, &mut jseq, snaps - 1, out, &mut index);
            } else {
                // The step that ends the human phase, the AI's turn and the step that starts the next phase are not
                // sent: the snapshot of the next turn replaces them.
                let _ = game.journal_take(false).unwrap();
            }
            let _ = r;
            if info.turn > turn_seen && info.phase == Phase::Humans && game.save().is_ok() {
                turn_seen = record_snapshot(&mut game, &mut index, &mut snaps, jseq);
                break;
            }
        }
    }
    std::fs::write(out.join("index.txt"), index).unwrap();
}

/// Takes the journal of a human-phase step; a non-empty batch (with the `CHECK`) is numbered, written and the digest
/// after it recorded.
fn push_batch(game: &mut Game, jseq: &mut u64, group: usize, out: &Path, index: &mut String) {
    let b = game.journal_take(true).unwrap();
    if decode_journal(&b).unwrap().is_empty() {
        return;
    }
    *jseq += 1;
    let digest = game
        .state_digest(gamecore::replica::REPLICA_DIGEST)
        .unwrap();
    let file = format!("batch-{jseq}.bin");
    std::fs::write(out.join(&file), &b).unwrap();
    writeln!(index, "batch {jseq} {group} {} {file}", hex(digest)).unwrap();
}

/// The GCS1 of a `.c5sv` server save: the section is a zstd frame behind a small header; `game-host` reads it.
fn game_host_gcs1(file: &[u8]) -> Vec<u8> {
    game_host::saves::decode(file).expect("a C5SV save").gcs1
}

// ---------------------------------------------------------------- the client (parent)

struct Snap {
    turn: i32,
    jseq: u64,
    digest: u64,
    raw: Vec<u8>,
}

struct BatchRec {
    jseq: u64,
    group: usize,
    digest: u64,
    bytes: Vec<u8>,
}

fn read_index(dir: &Path) -> (Vec<Snap>, Vec<BatchRec>) {
    let text = std::fs::read_to_string(dir.join("index.txt")).expect("index.txt");
    let (mut snaps, mut batches) = (Vec::new(), Vec::new());
    for line in text.lines() {
        let f: Vec<&str> = line.split(' ').collect();
        match f[0] {
            "snap" => snaps.push(Snap {
                turn: f[2].parse().unwrap(),
                jseq: f[3].parse().unwrap(),
                digest: u64::from_str_radix(f[4], 16).unwrap(),
                raw: std::fs::read(dir.join(f[5])).unwrap(),
            }),
            "batch" => batches.push(BatchRec {
                jseq: f[1].parse().unwrap(),
                group: f[2].parse().unwrap(),
                digest: u64::from_str_radix(f[3], 16).unwrap(),
                bytes: std::fs::read(dir.join(f[4])).unwrap(),
            }),
            other => panic!("index line {other}"),
        }
    }
    (snaps, batches)
}

#[derive(Clone)]
struct WallClock(Instant);

impl Clock for WallClock {
    fn now_ms(&self) -> u64 {
        self.0.elapsed().as_millis() as u64
    }
    fn now_ms_f64(&self) -> f64 {
        self.0.elapsed().as_secs_f64() * 1000.0
    }
}

fn rss_mb() -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))
                .and_then(|l| l.split_whitespace().nth(1)?.parse::<f64>().ok())
        })
        .map_or(0.0, |kb| kb / 1024.0)
}

/// The fixture of `replica::selftest` from the server's output: snapshot and patch bodies as the server makes them
/// (zstd by the C library), the batches, the entropy header, the game data, and the script that ties them together.
fn fixture(
    snaps: &[Snap],
    batches: &[BatchRec],
    every_batch: bool,
    stats: &mut Vec<(String, f64)>,
) -> BTreeMap<String, Vec<u8>> {
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut steps = vec![json!({"op": "create"})];
    // The join snapshot as the server makes it: zstd -19.
    let t = Instant::now();
    let body0 = common::snapshot_body_zstd(&snaps[0].raw, 19);
    stats.push((
        "snapshot0_zstd19_encode_ms".into(),
        t.elapsed().as_secs_f64() * 1000.0,
    ));
    stats.push((
        "snapshot0_raw_mb".into(),
        snaps[0].raw.len() as f64 / 1048576.0,
    ));
    stats.push((
        "snapshot0_body_ruzstd_fastest_kb".into(),
        wire::replica::encode_snapshot(&snaps[0].raw).unwrap().len() as f64 / 1024.0,
    ));
    files.insert("snap-0.body".into(), body0);
    steps.push(json!({"op": "base", "epoch": 1, "turn": snaps[0].turn, "jseq": snaps[0].jseq,
        "raw_hash": hex(xxh64(0, &snaps[0].raw)), "body": "snap-0.body", "digest": hex(snaps[0].digest)}));

    for k in 1..snaps.len() {
        // The human phase of turn k-1.
        let mine: Vec<&BatchRec> = batches.iter().filter(|b| b.group == k - 1).collect();
        if let Some(first) = mine.first() {
            let list: Vec<Value> = mine
                .iter()
                .map(|b| {
                    let name = format!("batch-{}.bin", b.jseq);
                    files.insert(name.clone(), b.bytes.clone());
                    json!({"file": name, "digest": hex(b.digest)})
                })
                .collect();
            steps.push(json!({"op": "journal", "epoch": 1, "first": first.jseq, "batches": list}));
        }
        // The turn change: by snapshot (B) at odd turns, by patch (A) at even ones. zstd -3 is the server's
        // snapshot until its -19 is ready.
        let s = &snaps[k];
        let t = Instant::now();
        files.insert(
            format!("snap-{k}.body"),
            common::snapshot_body_zstd(&s.raw, 3),
        );
        let mut step = json!({"op": "turn", "epoch": 1, "turn": s.turn, "jseq": s.jseq,
            "raw_hash": hex(xxh64(0, &s.raw)), "body": format!("snap-{k}.body"),
            "from_raw_hash": hex(xxh64(0, &snaps[k - 1].raw)), "digest": hex(s.digest)});
        if k % 2 == 0 {
            let tp = Instant::now();
            files.insert(
                format!("patch-{k}.body"),
                common::patch_body(&snaps[k - 1].raw, &s.raw, 9),
            );
            stats.push((
                format!("turn{k}_patch9_encode_ms"),
                tp.elapsed().as_secs_f64() * 1000.0,
            ));
            step["patch"] = json!(format!("patch-{k}.body"));
        }
        stats.push((
            format!("turn{k}_snapshot3_encode_ms"),
            t.elapsed().as_secs_f64() * 1000.0,
        ));
        steps.push(step);
    }
    let script = json!({"seat": 0, "check_always": true, "every_batch": every_batch,
        "patches": true, "steps": steps});
    files.insert(
        "script.json".into(),
        serde_json::to_vec_pretty(&script).unwrap(),
    );

    // The game data as the core reads it: both databases and the Lua files of the index.
    let dir = gamedata_dir();
    let index = std::fs::read(dir.join("vfs-index.json")).unwrap();
    let parsed: Value = serde_json::from_slice(&index).unwrap();
    files.insert("gamedata/vfs-index.json".into(), index);
    for path in ["db/gameplay.db", "db/loc-en_us.db"] {
        files.insert(
            format!("gamedata/{path}"),
            std::fs::read(dir.join(path)).unwrap(),
        );
    }
    for f in parsed["files"].as_array().unwrap() {
        let path = f["path"].as_str().unwrap();
        files.insert(
            format!("gamedata/{path}"),
            std::fs::read(dir.join(path)).unwrap(),
        );
    }
    files
}

fn run(name: &str) {
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return;
    }
    let sc = scenario(name);
    let out = std::env::temp_dir().join(format!("replica-sim-{}-{}", sc.name, std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    // The server, in a child process.
    let t0 = Instant::now();
    let status = Process::new(std::env::current_exe().unwrap())
        .args(["--exact", "sim", "--nocapture", "--test-threads=1"])
        .env(SCENARIO_ENV, &out)
        .env("REPLICA_SCENARIO", sc.name)
        .status()
        .unwrap();
    assert!(status.success(), "the server child failed");
    let (snaps, batches) = read_index(&out);
    assert!(snaps.len() > sc.turns, "{} snapshots", snaps.len());

    let mut stats: Vec<(String, f64)> =
        vec![("server_ms".into(), t0.elapsed().as_secs_f64() * 1000.0)];
    let files = fixture(&snaps, &batches, sc.every_batch, &mut stats);
    let journal_bytes: usize = batches.iter().map(|b| b.bytes.len()).sum();
    stats.push(("journal_bytes_total".into(), journal_bytes as f64));
    stats.push(("batches_sent".into(), batches.len() as f64));
    if let Some(dir) = std::env::var_os("REPLICA_FIXTURE_OUT") {
        // For the browser: the same files, and the list the page fetches.
        let dir = PathBuf::from(dir).join(sc.name);
        let _ = std::fs::remove_dir_all(&dir);
        for (name, bytes) in &files {
            let p = dir.join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, bytes).unwrap();
        }
        let names: Vec<&String> = files.keys().collect();
        std::fs::write(dir.join("list.json"), serde_json::to_vec(&names).unwrap()).unwrap();
        println!("REPLICA {} fixture written to {}", sc.name, dir.display());
    }

    let files: HashMap<String, Vec<u8>> = files.into_iter().collect();
    let clock = WallClock(Instant::now());
    let report = replica::selftest::run(&files, Box::new(clock), Some(&rss_mb))
        .unwrap_or_else(|e| panic!("{}: {e}", sc.name));
    for (k, v) in stats.iter().chain(report.lines.iter()) {
        println!("REPLICA {} {k} = {v:.1}", sc.name);
    }
    println!("REPLICA {} missed files: {:?}", sc.name, report.missed);
    assert!(
        report.missed.is_empty(),
        "the game data lacks files the core asked for"
    );
    assert_eq!(report.get("diverged"), Some(0.0));
    assert_eq!(
        report.get("turn_patch"),
        Some(((snaps.len() - 1) / 2) as f64)
    );
    if let Some(path) = std::env::var_os("REPLICA_REPORT") {
        let r = replica::selftest::SelftestReport {
            lines: stats.into_iter().chain(report.lines).collect(),
            ..Default::default()
        };
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        writeln!(f, "{}", r.to_json(sc.name)).unwrap();
    }
    let _ = std::fs::remove_dir_all(&out);
    // The process ends here: the core is a singleton and not meant to be torn down.
}

/// The child: the server of the scenario named by `REPLICA_SCENARIO`.
#[test]
fn sim() {
    let Some(out) = std::env::var_os(SCENARIO_ENV) else {
        return;
    };
    if gamecore::IMPLEMENTATION != "lekmod" {
        return;
    }
    let name = std::env::var("REPLICA_SCENARIO").unwrap();
    simulate(&scenario(&name), Path::new(&out));
}

/// The client of the scenario named by `REPLICA_SCENARIO`, in a process of its own: the core is a singleton and
/// does not start twice in one process, so each scenario (and its server) gets one.
#[test]
fn client() {
    if std::env::var_os("REPLICA_CLIENT").is_none() {
        return;
    }
    run(&std::env::var("REPLICA_SCENARIO").unwrap());
}

fn in_a_process_of_its_own(name: &str) {
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return;
    }
    let status = Process::new(std::env::current_exe().unwrap())
        .args(["--exact", "client", "--nocapture", "--test-threads=1"])
        .env("REPLICA_CLIENT", "1")
        .env("REPLICA_SCENARIO", name)
        .env_remove(SCENARIO_ENV)
        .status()
        .unwrap();
    assert!(status.success(), "scenario {name} failed");
}

#[test]
fn tiny() {
    in_a_process_of_its_own("tiny");
}

#[test]
#[ignore = "Small, 30 warm-up turns"]
fn small() {
    in_a_process_of_its_own("small");
}

#[test]
#[ignore = "needs the late-huge save (CIV5_LATE_HUGE)"]
fn late_huge() {
    in_a_process_of_its_own("late-huge");
}
