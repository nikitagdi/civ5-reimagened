//! Save and load on the real Lekmod gamecore (G4b, AC-8.1). Returns early on the stub; run on
//! Linux/WSL with `GAMECORE_LIB_DIR_x86_64` set.
//!
//! The real core is a process singleton, so the load runs in a second process: the test starts its
//! own binary again with `GC_SAVE_FILE` set, and that run loads the save into a new `Game` instead of
//! starting one. Both processes then play the same turns with the same orders (end the turn) and
//! print the state digests of the whole game (-1) and of the human's team; the parent compares them.
//!
//! The game: the default setup (its seed is one known to continue turn by turn; seed 2 with two AI
//! diverges at turn 32, golden `CvGoodyHuts::Read`, FIX-0010 pending), the settler founds a city,
//! three turns, save, two more turns.

use std::path::PathBuf;
use std::process::Command as Process;
use std::time::Instant;

use gamecore::{Command, Game, GameEvent, HostServices, PumpResult, SaveInfo, Setup};
use gamecore_sys::ffi;

const TEST_NAME: &str = "real_save_load_continue";
/// Turns played after the save point, in both processes.
const TURNS_AFTER: usize = 2;

struct DirHost {
    dir: PathBuf,
}

impl HostServices for DirHost {
    fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        if path.split(['/', '\\']).any(|p| p == "..") {
            return None;
        }
        std::fs::read(self.dir.join(path)).ok()
    }

    fn game_event(&self, _ev: GameEvent) {}
}

fn gamedata_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("CIV5_GAMEDATA") {
        return PathBuf::from(d);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .map(|a| a.join("assets-out/gamedata-b1"))
        .find(|d| d.join("vfs-index.json").is_file())
        .expect("game data not found: set CIV5_GAMEDATA=<export-gamedata dir>")
}

fn pump_to_human(game: &mut Game) -> PumpResult {
    for _ in 0..5000 {
        let r = game.pump(64).unwrap();
        if r.status == ffi::GC_PUMP_WAITING_HUMAN {
            return r;
        }
        assert_eq!(r.status, ffi::GC_PUMP_AI_RUNNING, "{r:?}");
    }
    panic!("the human was not up after 5000 pump calls");
}

/// `(turn, digest of the whole game, digest of the human's team)`.
fn digests(game: &mut Game) -> (i32, u64, u64) {
    let info = game.game_info().unwrap();
    let team = i32::from(info.human_team);
    (
        info.turn,
        game.state_digest(-1).unwrap(),
        game.state_digest(team).unwrap(),
    )
}

/// Gives idle cities the first build on their list, ends the turn and runs the rivals; returns the
/// digests of the next human turn. The same orders in the same state in both processes.
fn next_turn(game: &mut Game) -> (i32, u64, u64) {
    let info = game.game_info().unwrap();
    let player = u8::try_from(info.human_player).unwrap();
    let before = info.turn;
    for city in game.cities(i32::from(info.human_team)).unwrap() {
        if i32::from(city.owner) != i32::from(player) || city.production_order.is_some() {
            continue;
        }
        let builds = game.city_builds(player, city.id).unwrap();
        let first = builds
            .iter()
            .find(|b| b.flags.enabled())
            .expect("something to build");
        game.command(
            player,
            &Command::CitySetProduction {
                city: city.id,
                order: first.order,
                item: i32::from(first.item),
            },
        )
        .unwrap();
        pump_to_human(game);
    }
    game.command(player, &Command::ForceEndTurn).unwrap();
    let up = pump_to_human(game);
    assert_eq!(up.turn, before + 1, "one turn passed");
    digests(game)
}

fn line(stage: &str, d: (i32, u64, u64)) -> String {
    format!(
        "DIGEST {stage} turn={} all={:016x} team={:016x}",
        d.0, d.1, d.2
    )
}

/// The second process: load, then the same turns.
fn child(path: &str) {
    let bytes = std::fs::read(path).unwrap();
    let mut game = Game::new(DirHost {
        dir: gamedata_dir(),
    })
    .unwrap();
    let t = Instant::now();
    game.load(&bytes).unwrap();
    eprintln!("load: {:?}", t.elapsed());
    let before_pump = digests(&mut game);
    // The loaded game runs the slices back to the human's turn (no new turn).
    let t = Instant::now();
    let up = pump_to_human(&mut game);
    eprintln!("pump to the human after the load: {:?}", t.elapsed());
    assert_eq!(up.turn, before_pump.0);
    let loaded = digests(&mut game);
    eprintln!(
        "digests right after the load {}: {}",
        if before_pump == loaded {
            "(same as after the pump)"
        } else {
            "(differ from after the pump)"
        },
        line("raw", before_pump)
    );
    println!("{}", line("loaded", loaded));
    for i in 0..TURNS_AFTER {
        println!("{}", line(&format!("after{i}"), next_turn(&mut game)));
    }
}

#[test]
fn real_save_load_continue() {
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return;
    }
    if let Ok(path) = std::env::var("GC_SAVE_FILE") {
        child(&path);
        return;
    }

    let mut game = Game::new(DirHost {
        dir: gamedata_dir(),
    })
    .unwrap();
    game.new_game(&Setup::default()).unwrap();
    pump_to_human(&mut game);
    let info = game.game_info().unwrap();
    let player = u8::try_from(info.human_player).unwrap();
    let team = i32::from(info.human_team);

    // A city first, so the save has more than the start.
    let settler = game
        .units(team)
        .unwrap()
        .into_iter()
        .find(|u| {
            i32::from(u.owner) == i32::from(player)
                && game
                    .unit_actions(player, u.id)
                    .is_ok_and(|a| a.iter().any(|a| a.action == 3 && a.enabled))
        })
        .expect("a unit that can found a city");
    game.command(player, &Command::UnitFoundCity { unit: settler.id })
        .unwrap();
    pump_to_human(&mut game);
    assert_eq!(game.cities(team).unwrap().len(), 1, "the city stands");
    for _ in 0..3 {
        next_turn(&mut game);
    }

    let saved_at = digests(&mut game);
    let t = Instant::now();
    let bytes = game.save().unwrap();
    let save_ms = t.elapsed();
    let info = SaveInfo::parse(&bytes).unwrap();
    eprintln!(
        "save at turn {}: {} bytes in {save_ms:?}, build {}",
        info.turn,
        bytes.len(),
        info.build_id
    );
    assert_eq!(info.turn, saved_at.0);
    assert!(info.build_id.starts_with("lekmod-portable/"));
    assert_eq!(
        digests(&mut game),
        saved_at,
        "saving does not change the game"
    );

    let path = std::env::temp_dir().join(format!("gc-real-save-{}.gcs1", std::process::id()));
    std::fs::write(&path, &bytes).unwrap();
    let mut expected = vec![line("loaded", saved_at)];
    for i in 0..TURNS_AFTER {
        expected.push(line(&format!("after{i}"), next_turn(&mut game)));
    }

    let t = Instant::now();
    let out = Process::new(std::env::current_exe().unwrap())
        .args([TEST_NAME, "--exact", "--nocapture", "--test-threads=1"])
        .env("GC_SAVE_FILE", &path)
        .output()
        .unwrap();
    let _ = std::fs::remove_file(&path);
    let stdout = String::from_utf8_lossy(&out.stdout);
    eprintln!(
        "child process ({:?}):\n{}{}",
        t.elapsed(),
        stdout,
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "the loading process failed");
    // libtest prints "test <name> ... " in front of the first line.
    let got: Vec<&str> = stdout
        .lines()
        .filter_map(|l| l.find("DIGEST ").map(|at| &l[at..]))
        .collect();
    for e in &expected {
        eprintln!("expected {e}");
    }
    assert_eq!(
        got, expected,
        "same digests after the load and turn by turn"
    );
}
