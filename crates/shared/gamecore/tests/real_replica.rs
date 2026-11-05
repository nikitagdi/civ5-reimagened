//! The replica on the real Lekmod gamecore (B4 gc-replica). Returns early on the
//! stub; run on Linux/WSL with `GAMECORE_LIB_DIR_x86_64` and `CIV5_GAMEDATA` set.
//!
//! The core is a process singleton, so one instance plays both parts: as the server it plays three
//! turns of two test-bot humans and two AI, with one net message of a human, and keeps the journal; then `resync` puts the first
//! snapshot back in place, the instance becomes a replica and replays the journal. The replica digest
//! must end where the server's did, and every CHECK in the journal must pass on the way.

use std::path::PathBuf;

use gamecore::replica::{JournalRecord, decode_journal};
use gamecore::{Game, GameError, GameEvent, HostServices, NetMessage, Role, Setup, SlotSetup};
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

/// Pumps until the game can be saved at a turn after `after` (the bots' human phase).
fn pump_to_save_point(game: &mut Game, after: i32, batches: &mut Vec<Vec<u8>>) -> (i32, Vec<u8>) {
    for _ in 0..20_000 {
        let r = game.pump(64).unwrap();
        assert!(
            r.status == ffi::GC_PUMP_AI_RUNNING || r.status == ffi::GC_PUMP_WAITING_HUMAN,
            "{r:?}"
        );
        let humans = game.game_info().unwrap().phase == gamecore::Phase::Humans;
        batches.push(game.journal_take(humans).unwrap());
        if r.turn > after
            && let Ok(snap) = game.save()
        {
            return (r.turn, snap);
        }
    }
    panic!("no save point after turn {after}");
}

#[test]
fn real_replica_replays_the_server_journal() {
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return;
    }
    let mut game = Game::new(DirHost(gamedata_dir())).unwrap();
    let mut setup = Setup::network(
        77,
        vec![
            SlotSetup::human("random", "a"),
            SlotSetup::human("random", "b"),
            SlotSetup::ai("random"),
            SlotSetup::ai("random"),
        ],
    );
    setup.world_size = "WORLDSIZE_TINY".into();
    setup.handicap = "HANDICAP_CHIEFTAIN".into();
    setup.autoplay_humans = vec![0, 1];
    game.new_game(&setup).unwrap();
    game.set_role(Role::Server).unwrap();

    let mut ignored = Vec::new();
    let (start, first) = pump_to_save_point(&mut game, -1, &mut ignored);
    let start_digest = game
        .state_digest(gamecore::replica::REPLICA_DIGEST)
        .unwrap();
    // U-D10: a UI's net message through the server (the turn handshake is not one).
    assert!(matches!(
        game.net_submit(0, &[NetMessage::new("sendTurnComplete", &[]).unwrap()]),
        Err(GameError::Rejected { .. })
    ));
    game.net_submit(
        0,
        &[NetMessage::new("sendResearch", &[1, 0, -1, 0]).unwrap()],
    )
    .unwrap();
    let mut batches = vec![game.journal_take(false).unwrap()];
    let mut turn = start;
    let mut last = Vec::new();
    while turn < start + 3 {
        let (t, snap) = pump_to_save_point(&mut game, turn, &mut batches);
        turn = t;
        last = snap;
    }
    let end_digest = game
        .state_digest(gamecore::replica::REPLICA_DIGEST)
        .unwrap();
    let records: Vec<JournalRecord> = batches
        .iter()
        .flat_map(|b| decode_journal(b).unwrap())
        .collect();
    assert!(
        records
            .iter()
            .any(|r| matches!(r, JournalRecord::Command { .. })),
        "the bots acted"
    );
    assert!(
        records
            .iter()
            .any(|r| matches!(r, JournalRecord::Check { .. }))
    );
    assert!(
        records
            .iter()
            .any(|r| matches!(r, JournalRecord::Net { player: 0, .. }))
    );

    // The same instance as a replica from the first snapshot (a replica takes the server's slots on line from it).
    game.set_role(Role::Replica { seat: Some(0) }).unwrap();
    game.resync(&first).unwrap();
    assert_eq!(
        game.state_digest(gamecore::replica::REPLICA_DIGEST)
            .unwrap(),
        start_digest
    );
    assert!(matches!(game.pump(8), Err(GameError::Rejected { .. })));
    assert!(matches!(
        game.net_submit(0, &[NetMessage::new("sendResearch", &[0]).unwrap()]),
        Err(GameError::Rejected { .. })
    ));
    for (i, b) in batches.iter().enumerate() {
        game.journal_apply(b)
            .unwrap_or_else(|e| panic!("batch {i} of {}: {e}", batches.len()));
    }
    assert_eq!(
        game.state_digest(gamecore::replica::REPLICA_DIGEST)
            .unwrap(),
        end_digest
    );
    assert_eq!(game.net_take().unwrap(), vec![]);

    // A resync to the last snapshot gives the server's game too.
    game.resync(&last).unwrap();
    assert_eq!(
        game.state_digest(gamecore::replica::REPLICA_DIGEST)
            .unwrap(),
        end_digest
    );
}
