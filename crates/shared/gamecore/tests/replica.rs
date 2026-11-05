//! Stub tests of the replica calls (B4 gc-replica): roles, the empty journal and outbox, the
//! replica's first snapshot. Replay and resync need the real core (`real_replica.rs`).

use std::sync::{Mutex, MutexGuard};

use gamecore::replica::{decode_journal, encode_journal};
use gamecore::{
    Applied, Game, GameError, GameEvent, HostServices, LogLevel, ReplicaGame, Role, Setup, UiOp,
};

struct Quiet;

impl HostServices for Quiet {
    fn log(&self, _level: LogLevel, _msg: &str) {}
    fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        (path == "vfs-index.json").then(|| b"hello".to_vec())
    }
    fn game_event(&self, _ev: GameEvent) {}
}

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn stub() -> bool {
    if gamecore::IMPLEMENTATION == "stub" {
        return true;
    }
    eprintln!("skipped: linked gamecore is not the stub");
    false
}

fn snapshot() -> Vec<u8> {
    let mut game = Game::new(Quiet).unwrap();
    game.new_game(&Setup {
        seed: 5,
        ..Setup::default()
    })
    .unwrap();
    while game.pump(8).unwrap().status != 0 {}
    game.set_role(Role::Server).unwrap();
    let journal = game.journal_take(true).unwrap();
    assert_eq!(decode_journal(&journal).unwrap(), vec![]);
    game.set_role(Role::Plain).unwrap();
    assert!(matches!(
        game.journal_take(false),
        Err(GameError::Rejected { .. })
    ));
    game.save().unwrap()
}

#[test]
fn a_replica_starts_from_a_snapshot() {
    if !stub() {
        return;
    }
    let _serial = serial();
    let bytes = snapshot();

    let mut replica = ReplicaGame::new(Quiet, Some(0)).unwrap();
    assert_eq!(
        replica.apply(&encode_journal(&[])).err(),
        Some(GameError::NotStarted)
    );
    replica.resync(&bytes).unwrap();
    assert!(replica.in_step());
    assert_eq!(
        replica.apply(&encode_journal(&[])).unwrap(),
        Applied::InStep
    );
    assert_eq!(replica.take_ui_messages().unwrap(), vec![]);
    assert!(matches!(
        replica.apply(b"GCJ0\x01\x00\x00\x00\x00\x00\x00\x00"),
        Err(GameError::BadData(_))
    ));
    // A replica never takes inputs for the server.
    assert!(matches!(
        replica.game().net_submit(0, &[]),
        Err(GameError::Rejected { .. })
    ));
    assert_eq!(replica.stats(), (1, 0, 0));
}

/// The UI bridge on the stub: attach registers empty tables and leaves the stack alone, there are no events, the
/// interface ops are not implemented, and nothing works outside the replica role.
#[test]
fn the_ui_bridge_on_the_stub() {
    if !stub() {
        return;
    }
    let _serial = serial();
    let bytes = snapshot();
    let mut replica = ReplicaGame::new(Quiet, Some(0)).unwrap();
    replica.resync(&bytes).unwrap();

    let lua = lua_sys::Lua::new().unwrap();
    replica.attach_lua(&lua).unwrap();
    lua.exec(
        "assert(type(Game) == 'table' and type(Players) == 'table')",
        "=check",
    )
    .unwrap();
    replica.setup_graphical().unwrap();
    replica.subscribe_game_event("PlayerAdoptPolicy");
    let mut events = Vec::new();
    replica.take_ui_events(&mut events);
    assert!(events.is_empty());
    let mut out = Vec::new();
    assert!(matches!(
        replica.ui_op(UiOp::ClearSelection, &[], &mut out),
        Err(GameError::Core(_))
    ));
    assert!(out.is_empty());
    replica.invalidate_lua(&lua);
    lua.exec("assert(type(Players) == 'table')", "=check")
        .unwrap();

    // Not a replica: nothing to serve.
    replica.game().set_role(Role::Plain).unwrap();
    assert!(matches!(
        replica.setup_graphical(),
        Err(GameError::Rejected { .. })
    ));
    assert!(matches!(
        replica.ui_op(UiOp::ClearSelection, &[], &mut out),
        Err(GameError::Rejected { .. })
    ));
}
