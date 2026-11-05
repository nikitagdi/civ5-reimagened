//! B11 (audit 2026-11-01): "MISSION_SKIP twice on the same units diverges the issuing replica". Real Lekmod gamecore; returns
//! early on the stub; run on Linux/WSL with `GAMECORE_LIB_DIR_x86_64` and `CIV5_GAMEDATA` set.
//!
//! RESULT: not reproduced. Neither the core nor the replica's UI path (`ui_op` selection, the DLL's own
//! `Game.SelectionListGameNetMessage`) moves the issuing replica's digest on a second skip round; this test pins that.
//!
//! One core instance plays both parts (a process singleton). As the server (two real humans, so nobody ends the turn behind
//! our back) it gets the steps of `SKIP_SEQ` (default in the code): `S` = a round of `sendPushMission(MISSION_SKIP)` for every
//! unit of seat 0 as a UI sends it, `P` = a pump slice, `M` = a unit moves, `Z` = fortify, `E` / `C` = end of turn / take it
//! back. Every batch and the server digest after it are kept. Then the instance is a replica of seat 0 with a Lua state and the
//! batches are replayed three times:
//!  0. journal only (a client that does nothing);
//!  1. as the issuing client: before each skip round the UI path runs on the replica (select the unit, push through the DLL's
//!     `Game.SelectionListGameNetMessage`, clear the selection);
//!  2. the same with the last unit left selected.
//!
//! The UI path must not move the replica digest, its outbox must hold one `sendPushMission` per unit, and after every batch
//! the digest must equal the server's.

use std::path::PathBuf;

use gamecore::replica::REPLICA_DIGEST;
use gamecore::{
    Game, GameEvent, HostServices, NetMessage, Phase, ReplicaGame, Role, Setup, SlotSetup, UiOp,
};
use gamecore_sys::ffi;
use lua_sys::Value;

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

/// Pumps until the game can be saved at a turn after `after`; every batch and the server digest after it are kept.
fn pump_to_save_point(
    game: &mut Game,
    after: i32,
    batches: &mut Vec<(Vec<u8>, u64)>,
) -> (i32, Vec<u8>) {
    for _ in 0..20_000 {
        let r = game.pump(64).unwrap();
        assert!(
            r.status == ffi::GC_PUMP_AI_RUNNING || r.status == ffi::GC_PUMP_WAITING_HUMAN,
            "{r:?}"
        );
        let humans = game.game_info().unwrap().phase == Phase::Humans;
        let batch = game.journal_take(humans).unwrap();
        let digest = game.state_digest(REPLICA_DIGEST).unwrap();
        batches.push((batch, digest));
        if r.turn > after
            && let Ok(snap) = game.save()
        {
            return (r.turn, snap);
        }
    }
    panic!("no save point after turn {after}");
}

fn nums(v: &[Value]) -> Vec<i32> {
    v.iter().filter_map(Value::as_i32).collect()
}

/// What the UI's Lua does after `UI.SelectUnit`: the DLL's own `Game.SelectionListGameNetMessage` with the selection.
const UI_SKIP_ROUND: &str = r#"
function UiPush()
  Game.SelectionListGameNetMessage(GameMessageTypes.GAMEMESSAGE_PUSH_MISSION, GameInfoTypes.MISSION_SKIP, 0, 0, 0, false, false)
end
function SkipId() return GameInfoTypes.MISSION_SKIP end
"#;

#[test]
fn real_replica_survives_mission_skip_twice() {
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return;
    }
    let mut rep = ReplicaGame::new(DirHost(gamedata_dir()), Some(0)).unwrap();
    let mut setup = Setup::network(
        5,
        vec![
            SlotSetup::human("random", "a"),
            SlotSetup::human("random", "b"),
            SlotSetup::ai("random"),
            SlotSetup::ai("random"),
        ],
    );
    setup.world_size = "WORLDSIZE_TINY".into();
    setup.handicap = "HANDICAP_CHIEFTAIN".into();
    setup.turn_mode = gamecore::TurnMode::Simultaneous;
    setup.autoplay_humans = vec![];
    let game = rep.game();
    game.new_game(&setup).unwrap();
    game.set_role(Role::Server).unwrap();

    let mut ignored = Vec::new();
    let (_start, first) = pump_to_save_point(game, -1, &mut ignored);
    let start_digest = game.state_digest(REPLICA_DIGEST).unwrap();
    let mine: Vec<i32> = game
        .units(-1)
        .unwrap()
        .iter()
        .filter(|u| u.owner == 0)
        .map(|u| u.id)
        .collect();
    assert!(!mine.is_empty(), "seat 0 has units");

    // The mission id: the Lua state of a replica knows GameInfoTypes (the server has no Lua here).
    let mission_skip = {
        let lua = lua_sys::Lua::new().unwrap();
        rep.attach_lua(&lua).unwrap();
        lua.exec(UI_SKIP_ROUND, "=skip_round").unwrap();
        let id = nums(&lua.call_global("SkipId", &[]).unwrap())[0];
        rep.invalidate_lua(&lua);
        id
    };
    let game = rep.game();
    assert_eq!(game.state_digest(REPLICA_DIGEST).unwrap(), start_digest);

    // Server: two skip rounds as the UI of seat 0 sends them, then on to the next turns.
    let round: Vec<NetMessage> = mine
        .iter()
        .map(|id| NetMessage::new("sendPushMission", &[*id, mission_skip, 0, 0, 0, 0]).unwrap())
        .collect();
    let mut batches: Vec<(Vec<u8>, u64)> = Vec::new();
    let mut rounds_at: Vec<usize> = Vec::new();
    let seq =
        std::env::var("SKIP_SEQ").unwrap_or_else(|_| "M,P,P,S,P,S,Z,P,E,P,C,S,P,S,P,S,S,P".into());
    for step in seq.split(',') {
        match step {
            "S" => {
                game.net_submit(0, &round)
                    .unwrap_or_else(|e| panic!("skip round refused by the server: {e}"));
                rounds_at.push(batches.len());
            }
            "P" => {
                game.pump(1).unwrap();
            }
            "Z" => {
                for id in &mine {
                    let _ = game.command(0, &gamecore::Command::UnitFortify { unit: *id });
                }
            }
            "M" => {
                let id = *mine.last().unwrap();
                let reach = game.reachable(0, id).unwrap();
                let t = reach[reach.len() / 2];
                game.command(
                    0,
                    &gamecore::Command::UnitMoveTo {
                        unit: id,
                        x: i32::from(t.x),
                        y: i32::from(t.y),
                    },
                )
                .unwrap();
            }
            "E" => game.command(0, &gamecore::Command::ForceEndTurn).unwrap(),
            "C" => game.command(0, &gamecore::Command::CancelEndTurn).unwrap(),
            other => panic!("step {other}"),
        }
        let batch = game.journal_take(false).unwrap();
        let digest = game.state_digest(REPLICA_DIGEST).unwrap();
        eprintln!(
            "step {step}: digest {digest:x}; {:?}",
            game.units(-1)
                .unwrap()
                .iter()
                .filter(|u| u.owner == 0)
                .map(|u| (u.id, u.activity, u.moves_left))
                .collect::<Vec<_>>()
        );
        batches.push((batch, digest));
    }
    // Both end the turn; the AI moves; the next human phase.
    fn keep(game: &mut Game, batches: &mut Vec<(Vec<u8>, u64)>) {
        let batch = game.journal_take(false).unwrap();
        let digest = game.state_digest(REPLICA_DIGEST).unwrap();
        batches.push((batch, digest));
    }
    for _ in 0..6 {
        game.pump(8).unwrap();
        keep(game, &mut batches);
    }
    let end_digest = batches.last().unwrap().1;

    for mode in 0..3 {
        let with_ui = mode > 0;
        let keep_selected = mode == 2;
        let game = rep.game();
        game.set_role(Role::Replica { seat: Some(0) }).unwrap();
        game.resync(&first).unwrap();
        assert_eq!(game.state_digest(REPLICA_DIGEST).unwrap(), start_digest);
        let lua = lua_sys::Lua::new().unwrap();
        rep.attach_lua(&lua).unwrap();
        lua.exec(UI_SKIP_ROUND, "=skip_round").unwrap();
        rep.setup_graphical().unwrap();
        let mut events = Vec::new();
        rep.take_ui_events(&mut events);

        let mut prev = start_digest;
        for (i, (batch, digest)) in batches.iter().enumerate() {
            if with_ui && rounds_at.contains(&i) {
                let mut out = Vec::new();
                for id in &mine {
                    rep.ui_op(UiOp::SelectUnit, &[0, *id], &mut out).unwrap();
                    out.clear();
                    lua.call_global("UiPush", &[]).unwrap();
                }
                if !keep_selected {
                    rep.ui_op(UiOp::ClearSelection, &[], &mut out).unwrap();
                }
                assert_eq!(
                    rep.game().state_digest(REPLICA_DIGEST).unwrap(),
                    prev,
                    "the UI skip round moved the replica digest before batch {i} (round {})",
                    rounds_at.iter().position(|r| *r == i).unwrap()
                );
                let out = rep.take_ui_messages().unwrap();
                assert_eq!(out.len(), mine.len(), "one message per unit: {out:?}");
                assert!(out.iter().all(|m| m.name() == "sendPushMission"));
            }
            rep.game()
                .journal_apply(batch)
                .unwrap_or_else(|e| panic!("mode={mode} batch {i}: {e}"));
            let got = rep.game().state_digest(REPLICA_DIGEST).unwrap();
            assert_eq!(got, *digest, "mode={mode}: digest after batch {i}");
            prev = got;
            events.clear();
            rep.take_ui_events(&mut events);
        }
        assert_eq!(prev, end_digest);
        rep.invalidate_lua(&lua);
        eprintln!(
            "mode={mode}: {} batches replayed, digests equal",
            batches.len()
        );
    }
}
