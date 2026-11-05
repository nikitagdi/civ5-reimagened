//! A human takes an AI seat in the middle of a turn on the real Lekmod gamecore (live game 931, 2026-10-30). Returns early on
//! the stub; run on Linux/WSL with `GAMECORE_LIB_DIR_x86_64` and `CIV5_GAMEDATA` set.
//!
//! The AI had played its turn when the human took its seat (`gc_slot_control CONNECT` onto an AI slot, SIMP-056: the human plays
//! from the next turn, the slot's turn is marked ended). The slot kept the AI's auto-moves flag with its turn over, and the core
//! counted it as "a human in auto moves": no safe point until the turn ended. In game 931 another human's end of turn was held
//! by a blocker, so no save, no kick and no replica snapshot (the joining player's UI) came for the rest of the game.

use std::path::PathBuf;

use gamecore::{Game, GameEvent, HostServices, PumpResult, Setup, SlotOp, SlotSetup, TurnMode};
use gamecore_sys::ffi;

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

#[test]
fn a_human_taking_an_ai_seat_mid_turn_keeps_the_safe_point() {
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return;
    }
    let mut game = Game::new(DirHost(gamedata_dir())).unwrap();
    let mut setup = Setup::network(
        931,
        vec![
            SlotSetup::human("random", "anna"),
            SlotSetup::human("random", "ben"),
            SlotSetup::ai("random"),
        ],
    );
    setup.world_size = "WORLDSIZE_TINY".into();
    setup.handicap = "HANDICAP_CHIEFTAIN".into();
    setup.turn_mode = TurnMode::Simultaneous;
    game.new_game(&setup).unwrap();
    pump_to_humans(&mut game);
    let info = game.game_info().unwrap();
    assert!(info.safe_point, "{:?}", info.unsafe_reason);

    // The AI has played this turn; a third human takes its seat.
    game.slot_control(2, SlotOp::Connect, Some("cara")).unwrap();
    pump_to_humans(&mut game);
    let cara = game
        .turn_state()
        .unwrap()
        .into_iter()
        .find(|s| s.player == 2)
        .expect("the claimed seat");
    eprintln!("claimed seat: {cara:?}");
    assert!(cara.flags.human() && cara.flags.connected(), "{cara:?}");
    assert!(
        !cara.flags.turn_active(),
        "the seat plays from the next turn: {cara:?}"
    );

    // The other humans have not ended the turn, and the game is at a safe point: saves, kicks and snapshots are possible.
    let info = game.game_info().unwrap();
    assert!(
        info.safe_point,
        "no safe point after the claim: {:?}",
        info.unsafe_reason
    );
    game.save().expect("a save after the claim");
    game.slot_control(1, SlotOp::Kick, None)
        .expect("a kick after the claim");
    pump_to_humans(&mut game);
    let info = game.game_info().unwrap();
    assert!(info.safe_point, "after the kick: {:?}", info.unsafe_reason);
}
