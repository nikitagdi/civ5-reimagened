//! A network game with two humans on the real Lekmod gamecore. Returns
//! early on the stub; run on Linux/WSL with `GAMECORE_LIB_DIR_x86_64` set.
//!
//! The flow: a new game with two humans and one AI, the commands of both humans, the end of turn
//! of both (one takes it back first), `Q_TURN` after each step, a kick and the return of one human,
//! then a save, and a load in a second process. The real core is a process singleton, so the load
//! runs in a second process: the test starts its own binary again with `GC_SAVE_FILE` set, and that
//! run loads the save into a new `Game`; it prints the picture of the game it sees and the parent
//! compares it with the one it saved.

use std::path::PathBuf;
use std::process::Command as Process;
use std::sync::{Arc, Mutex};

use gamecore::{
    Command, Game, GameEvent, GameInfo, GameType, HostServices, HumanSlot, Phase, PumpResult,
    RejectReason, SaveInfo, Setup, SlotOp, SlotSetup, TurnMode,
};
use gamecore_sys::ffi;

const TEST_NAME: &str = "real_mp_two_humans";
const NICK_ANNA: &str = "anna";
const NICK_BEN: &str = "ben";

#[derive(Clone)]
struct DirHost {
    dir: PathBuf,
    events: Arc<Mutex<Vec<GameEvent>>>,
}

impl HostServices for DirHost {
    fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        if path.split(['/', '\\']).any(|p| p == "..") {
            return None;
        }
        std::fs::read(self.dir.join(path)).ok()
    }

    fn game_event(&self, ev: GameEvent) {
        self.events.lock().unwrap().push(ev);
    }
}

fn host() -> DirHost {
    DirHost {
        dir: gamedata_dir(),
        events: Arc::default(),
    }
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

fn pump_to_humans(game: &mut Game) -> PumpResult {
    for _ in 0..5000 {
        let r = game.pump(64).unwrap();
        if r.status == ffi::GC_PUMP_WAITING_HUMAN {
            return r;
        }
        assert_eq!(r.status, ffi::GC_PUMP_AI_RUNNING, "{r:?}");
    }
    panic!("the humans were not up after 5000 pump calls");
}

fn slot_of(game: &mut Game, player: u8) -> HumanSlot {
    game.turn_state()
        .unwrap()
        .into_iter()
        .find(|s| s.player == player)
        .unwrap_or_else(|| panic!("no Q_TURN record for player {player}"))
}

/// The picture of the game that a save keeps (the connection is not saved): digests of the whole
/// game and of every human's team, the turn, and per human the nick, the kick and the end of turn.
fn picture(game: &mut Game, humans: &[u8]) -> String {
    let info = game.game_info().unwrap();
    let mut s = format!(
        "turn={} all={:016x}",
        info.turn,
        game.state_digest(-1).unwrap()
    );
    for &h in humans {
        let slot = slot_of(game, h);
        s += &format!(
            " | p{h} team={} slot={} nick={:?} kicked={} ended={} digest={:016x}",
            slot.team,
            slot.slot_status,
            slot.nick,
            slot.flags.kicked(),
            slot.flags.turn_ended(),
            game.state_digest(i32::from(slot.team)).unwrap()
        );
    }
    s
}

/// Both humans end the turn; returns the info of the next turn once both are up again.
fn both_end_turn(game: &mut Game, humans: &[u8]) -> GameInfo {
    let before = game.game_info().unwrap().turn;
    for &h in humans {
        game.command(h, &Command::ForceEndTurn).unwrap();
        pump_to_humans(game);
    }
    let info = game.game_info().unwrap();
    assert_eq!(info.turn, before + 1, "one turn passed");
    info
}

/// The second process: load, check, connect the humans, play a turn.
fn child(path: &str) {
    let bytes = std::fs::read(path).unwrap();
    let mut game = Game::new(host()).unwrap();
    game.load(&bytes).unwrap();
    // A loaded game stands at a safe point: nothing is queued.
    let info = game.game_info().unwrap();
    assert!(info.safe_point, "{:?}", info.unsafe_reason);
    assert_eq!(info.game_type, GameType::Network);
    let humans: Vec<u8> = game
        .turn_state()
        .unwrap()
        .iter()
        .filter(|s| s.flags.human())
        .map(|s| s.player)
        .collect();
    println!("PICTURE loaded {}", picture(&mut game, &humans));
    // After a load nobody is on line until the host connects them.
    for &h in &humans {
        assert!(!slot_of(&mut game, h).flags.connected(), "p{h} on line");
    }
    for (&h, nick) in humans.iter().zip([NICK_ANNA, NICK_BEN]) {
        game.slot_control(h, SlotOp::Connect, Some(nick)).unwrap();
        assert!(slot_of(&mut game, h).flags.connected(), "p{h} not on line");
    }
    println!("PICTURE connected {}", picture(&mut game, &humans));
    pump_to_humans(&mut game);
    let next = both_end_turn(&mut game, &humans);
    println!("TURN {}", next.turn);
}

fn found_city(game: &mut Game, player: u8, team: i32) {
    let settler = game
        .units(team)
        .unwrap()
        .into_iter()
        .find(|u| {
            u.owner == player as i8
                && game
                    .unit_actions(player, u.id)
                    .is_ok_and(|a| a.iter().any(|a| a.action == 3 && a.enabled))
        })
        .unwrap_or_else(|| panic!("player {player} has no unit that can found a city"));
    game.command(player, &Command::UnitFoundCity { unit: settler.id })
        .unwrap();
    pump_to_humans(game);
    // A city without a build holds the end of turn.
    let city = game
        .cities(team)
        .unwrap()
        .into_iter()
        .find(|c| c.owner == player as i8)
        .expect("the new city");
    let build = game
        .city_builds(player, city.id)
        .unwrap()
        .into_iter()
        .find(|b| b.flags.enabled())
        .expect("something to build");
    game.command(
        player,
        &Command::CitySetProduction {
            city: city.id,
            order: build.order,
            item: i32::from(build.item),
        },
    )
    .unwrap();
    pump_to_humans(game);
}

#[test]
fn real_mp_two_humans() {
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return;
    }
    if let Ok(path) = std::env::var("GC_SAVE_FILE") {
        child(&path);
        return;
    }

    let h = host();
    let events = h.events.clone();
    let mut game = Game::new(h).unwrap();
    let mut setup = Setup::network(
        4242,
        vec![
            SlotSetup::human("CIVILIZATION_ROME", NICK_ANNA),
            SlotSetup::human("random", NICK_BEN),
            SlotSetup::ai("random"),
        ],
    );
    setup.world_size = "WORLDSIZE_TINY".into();
    setup.handicap = "HANDICAP_CHIEFTAIN".into();
    game.new_game(&setup).unwrap();
    let up = pump_to_humans(&mut game);
    assert_eq!(up.humans_left, 2, "the turn waits for both humans");

    // The game as the core describes it.
    let info = game.game_info().unwrap();
    assert_eq!(info.game_type, GameType::Network);
    assert_eq!(info.turn_mode, Some(TurnMode::Hybrid));
    assert_eq!((info.humans, info.humans_left), (2, 2));
    assert_eq!(info.phase, Phase::Humans);
    assert!(info.safe_point, "{:?}", info.unsafe_reason);
    let start_turn = info.turn;

    // Q_TURN: two humans, on line, the turn waits for both.
    let turn = game.turn_state().unwrap();
    let humans: Vec<u8> = turn
        .iter()
        .filter(|s| s.flags.human())
        .map(|s| s.player)
        .collect();
    assert_eq!(humans.len(), 2, "{turn:?}");
    for s in &turn {
        assert!(
            s.flags.human() && s.flags.alive() && s.flags.connected(),
            "{s:?}"
        );
        assert!(!s.flags.kicked() && !s.flags.turn_ended(), "{s:?}");
    }
    let nicks: Vec<_> = turn.iter().map(|s| s.nick.clone()).collect();
    assert_eq!(
        nicks,
        vec![Some(NICK_ANNA.to_owned()), Some(NICK_BEN.to_owned())]
    );
    let per_human = game.game_info_for(humans[1]).unwrap();
    assert_eq!(per_human.turn, start_turn);
    let teams: Vec<i32> = turn.iter().map(|s| i32::from(s.team)).collect();

    // The commands of both humans: each founds a city; a human cannot command the other's units.
    for (i, &p) in humans.iter().enumerate() {
        found_city(&mut game, p, teams[i]);
    }
    for (i, &p) in humans.iter().enumerate() {
        let mine = game
            .cities(teams[i])
            .unwrap()
            .iter()
            .filter(|c| c.owner == p as i8)
            .count();
        assert_eq!(mine, 1, "player {p} founded a city");
    }
    // The end of turn: the first human ends it, takes it back and ends it again.
    events.lock().unwrap().clear();
    game.command(humans[0], &Command::ForceEndTurn).unwrap();
    let up = pump_to_humans(&mut game);
    assert_eq!(up.humans_left, 1);
    assert!(slot_of(&mut game, humans[0]).flags.turn_ended());
    assert!(!slot_of(&mut game, humans[1]).flags.turn_ended());
    game.command(humans[0], &Command::CancelEndTurn).unwrap();
    pump_to_humans(&mut game);
    assert!(!slot_of(&mut game, humans[0]).flags.turn_ended());
    assert_eq!(game.game_info().unwrap().humans_left, 2);
    // Ending a turn twice, or the other human's, is refused by the core, not by the wrapper.
    let again = game.command(humans[0], &Command::CancelEndTurn);
    assert!(again.is_ok(), "{again:?}");
    let ended: Vec<(i32, bool)> = events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            GameEvent::PlayerTurnEnded { player, ended } => Some((*player, *ended)),
            _ => None,
        })
        .collect();
    assert!(
        ended.contains(&(i32::from(humans[0]), true))
            && ended.contains(&(i32::from(humans[0]), false)),
        "{ended:?}"
    );
    pump_to_humans(&mut game);

    // Both end the turn, the AI moves, the next turn starts.
    events.lock().unwrap().clear();
    let next = both_end_turn(&mut game, &humans);
    assert_eq!(next.turn, start_turn + 1);
    assert_eq!(next.humans_left, 2, "a new turn waits for both again");
    assert!(next.safe_point, "{:?}", next.unsafe_reason);
    {
        let seen = events.lock().unwrap();
        assert!(
            seen.iter()
                .any(|e| matches!(e, GameEvent::HumanPhaseStarted { turn } if *turn == next.turn)),
            "no HumanPhaseStarted in {seen:?}"
        );
    }

    // A slot operation needs a safe point: after a command there is none.
    game.command(humans[1], &Command::CancelEndTurn).unwrap();
    let refused = game
        .slot_control(humans[1], SlotOp::Kick, None)
        .unwrap_err();
    assert!(
        matches!(
            refused,
            gamecore::CommandError::Rejected {
                reason: RejectReason::NotSafePoint,
                ..
            }
        ),
        "{refused:?}"
    );
    pump_to_humans(&mut game);

    // Kick the second human: an AI plays the slot, the turn no longer waits for it.
    // (The refused command above left a command given: the safe point is back after a save-free
    // turn change, so play one more turn first.)
    both_end_turn(&mut game, &humans);
    events.lock().unwrap().clear();
    game.slot_control(humans[1], SlotOp::Kick, None).unwrap();
    pump_to_humans(&mut game);
    let slot = slot_of(&mut game, humans[1]);
    assert!(slot.flags.kicked() && !slot.flags.connected(), "{slot:?}");
    assert_eq!(game.game_info().unwrap().humans_left, 1);
    assert!(events.lock().unwrap().iter().any(|e| matches!(
        e,
        GameEvent::SlotChanged { player, kicked: true, .. } if *player == i32::from(humans[1])
    )));

    // Back: the human takes its own slot again.
    game.slot_control(humans[1], SlotOp::Connect, Some(NICK_BEN))
        .unwrap();
    pump_to_humans(&mut game);
    let slot = slot_of(&mut game, humans[1]);
    assert!(slot.flags.connected() && !slot.flags.kicked(), "{slot:?}");
    assert_eq!(slot.nick.as_deref(), Some(NICK_BEN));

    // Play to the next turn start, which is a safe point, and save there.
    let at = both_end_turn(&mut game, &humans);
    assert!(at.safe_point, "{:?}", at.unsafe_reason);
    let saved_picture = picture(&mut game, &humans);
    let bytes = game.save().unwrap();
    let info = SaveInfo::parse(&bytes).unwrap();
    assert_eq!(info.turn, at.turn);
    assert_eq!(
        picture(&mut game, &humans),
        saved_picture,
        "saving does not change the game"
    );
    assert_eq!(game.save().unwrap(), bytes, "the same bytes twice");

    // Load it in a new process.
    let path = std::env::temp_dir().join(format!("gc-real-mp-{}.gcs1", std::process::id()));
    std::fs::write(&path, &bytes).unwrap();
    let out = Process::new(std::env::current_exe().unwrap())
        .args([TEST_NAME, "--exact", "--nocapture", "--test-threads=1"])
        .env("GC_SAVE_FILE", &path)
        .output()
        .unwrap();
    let _ = std::fs::remove_file(&path);
    let stdout = String::from_utf8_lossy(&out.stdout);
    eprintln!(
        "child process:\n{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "the loading process failed");
    // libtest prints "test <name> ... " in front of the first line.
    let lines: Vec<&str> = stdout
        .lines()
        .filter_map(|l| l.find("PICTURE ").map(|i| &l[i..]))
        .collect();
    assert_eq!(
        lines.first().copied(),
        Some(format!("PICTURE loaded {saved_picture}").as_str()),
        "the loaded game is the saved one"
    );
    assert!(
        lines
            .get(1)
            .is_some_and(|l| l.starts_with("PICTURE connected ")),
        "{lines:?}"
    );
    assert!(
        stdout.contains(&format!("TURN {}", at.turn + 1)),
        "the loaded game played on: {stdout}"
    );
}
