//! `Replica` over the real wiring (`CoreEngine` -> `gamecore::ReplicaGame` -> `gamecore-sys`) on the stub core:
//! the join loads a snapshot, an empty journal batch is applied, the game data comes through the `GameDataHost`.
//! Replay and resync need the real core (`real_replica.rs`).

use std::sync::{Arc, Mutex};

use gamecore::replica::{JournalRecord, encode_journal};
use gamecore::{Game, GameEvent, HostServices, LogLevel, Setup};
use protocol::xxh64;
use replica::{
    GameDataHost, GameFiles, GcBase, GcBlobRef, GcJournal, Replica, ReplicaMsg, ReplicaStep, State,
};
use wire::replica::encode_snapshot;

struct Quiet;

impl HostServices for Quiet {
    fn log(&self, _level: LogLevel, _msg: &str) {}
    fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        (path == "vfs-index.json").then(|| b"{}".to_vec())
    }
    fn game_event(&self, _ev: GameEvent) {}
}

fn stub() -> bool {
    if gamecore::IMPLEMENTATION == "stub" {
        return true;
    }
    eprintln!("skipped: linked gamecore is not the stub");
    false
}

#[test]
fn a_replica_joins_a_stub_game_and_applies_an_empty_batch() {
    if !stub() {
        return;
    }
    // A snapshot from a game of its own; the process holds one game at a time, so it goes first.
    let snapshot = {
        let mut game = Game::new(Quiet).unwrap();
        game.new_game(&Setup {
            seed: 5,
            ..Setup::default()
        })
        .unwrap();
        while game.pump(8).unwrap().status != 0 {}
        game.save().unwrap()
    };
    let body = encode_snapshot(&snapshot).unwrap();
    let blob = GcBlobRef {
        hash: xxh64(0, &body),
        size: body.len() as u32,
    };

    let files = Arc::new(Mutex::new(
        GameFiles::from_index(
            br#"{"db":{"gameplay":"data:gameplay-db","localization":"data:loc-en_us"},"files":[]}"#
                .to_vec(),
        )
        .unwrap(),
    ));
    let mut r = Replica::new(GameDataHost::new(files.clone()), 0).expect("the core boots");
    assert_eq!(r.state(), State::Empty);
    r.on_msg(ReplicaMsg::Base(GcBase {
        epoch: 1,
        turn: 1,
        jseq: 0,
        raw_hash: xxh64(0, &snapshot),
        snapshot: blob,
    }));
    assert_eq!(r.wanted_blobs(), vec![blob]);
    r.blob(blob.hash, body);
    assert_eq!(r.pump(10), ReplicaStep::Working);
    assert_eq!(r.pump(10), ReplicaStep::Loaded { turn: 1 });
    assert_eq!(r.state(), State::Live);
    assert_eq!(r.generation(), 1);

    let empty = encode_journal(&[]);
    let pump = encode_journal(&[JournalRecord::Check { digest: 0 }]);
    // The stub accepts an empty journal; a record needs the real core.
    r.config_mut().check_always = false;
    r.on_msg(ReplicaMsg::Journal(GcJournal {
        epoch: 1,
        first_jseq: 1,
        batches: vec![empty],
    }));
    assert_eq!(r.pump(10), ReplicaStep::Applied { jseq: 1 });
    // A batch with only a `CHECK` is cut to an empty one when the policy skips it.
    r.note_input();
    r.on_msg(ReplicaMsg::Journal(GcJournal {
        epoch: 1,
        first_jseq: 2,
        batches: vec![pump],
    }));
    assert_eq!(r.pump(10), ReplicaStep::Applied { jseq: 2 });
    assert_eq!(r.stats().applied, 2);
    assert_eq!(r.stats().checks_skipped, 1);
    // The router sees the core's (empty) box.
    assert!(r.take_outgoing().is_empty());

    // The read facade of the real wiring: the stub serves no query, which reads as "unsupported".
    let read = r.read().expect("a live core engine has a reader");
    assert_eq!(read.seat(), 0);
    assert_eq!(read.team(), -1, "no Q_GAME answered yet");
    assert_eq!(read.top_bar(), Err(replica::ReadError::Unsupported));
    assert_eq!(read.game_info(), Err(replica::ReadError::Unsupported));
    assert_eq!(
        read.catalog(replica::CatalogTable::Eras),
        Err(replica::ReadError::Unsupported)
    );
    // Team-wide reads ask for the seat's team first; that fails the same way.
    assert_eq!(read.players(), Err(replica::ReadError::Unsupported));
}
