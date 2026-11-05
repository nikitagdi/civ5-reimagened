//! Stub tests (ABI v1). Game-creating tests take `serial()`: one instance per process.

use std::sync::{Arc, Mutex, MutexGuard};

use gamecore::{
    Attacker, Command, CommandError, Game, GameError, GameEvent, GameType, HostServices, LogLevel,
    SaveInfo, Setup, SlotOp, SlotSetup,
};

#[derive(Clone, Default)]
struct Recorder {
    events: Arc<Mutex<Vec<GameEvent>>>,
    logs: Arc<Mutex<Vec<String>>>,
}

impl HostServices for Recorder {
    fn log(&self, _level: LogLevel, msg: &str) {
        self.logs.lock().unwrap().push(msg.to_owned());
    }

    fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        (path == "vfs-index.json").then(|| b"hello".to_vec())
    }

    fn game_event(&self, ev: GameEvent) {
        self.events.lock().unwrap().push(ev);
    }
}

static SERIAL: Mutex<()> = Mutex::new(());

/// Serializes game tests; a poisoned lock is ignored.
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

#[test]
fn game_is_send() {
    fn assert_send<T: Send>() {}
    assert_send::<Game>();
}

#[test]
fn new_game_pump_digest() {
    if !stub() {
        return;
    }
    let _serial = serial();
    let host = Recorder::default();
    let mut game = Game::new(host.clone()).unwrap();

    assert_eq!(
        Game::new(Recorder::default()).err(),
        Some(GameError::AlreadyExists)
    );
    assert_eq!(game.pump(1).err(), Some(GameError::NotStarted));
    game.new_game(&Setup {
        seed: 7,
        ai_count: 2,
        ..Setup::default()
    })
    .unwrap();

    let first = game.pump(1).unwrap();
    assert_eq!(first.status, 1);
    let second = game.pump(1).unwrap();
    assert_eq!(
        (second.status, second.turn, second.active_player),
        (0, 1, 0)
    );
    let digest = game.state_digest(-1).unwrap();
    assert_eq!(game.state_digest(3).unwrap(), digest);

    // Staged in the ABI: argument checks pass, then NOT_IMPLEMENTED.
    assert!(matches!(
        game.command(0, &Command::EndTurn),
        Err(CommandError::Other(GameError::Core(
            gamecore_sys::GcError::NotImplemented
        )))
    ));
    // The v2 commands encode and pass the same argument checks.
    for cmd in [
        Command::MakePeace { team: 1 },
        Command::CancelEndTurn,
        Command::EndTurnTimeout,
    ] {
        assert!(
            matches!(
                game.command(0, &cmd),
                Err(CommandError::Other(GameError::Core(
                    gamecore_sys::GcError::NotImplemented
                )))
            ),
            "{cmd:?}"
        );
    }
    assert!(matches!(
        game.game_info(),
        Err(GameError::Core(gamecore_sys::GcError::NotImplemented))
    ));
    assert!(matches!(
        game.plots(0),
        Err(GameError::Core(gamecore_sys::GcError::NotImplemented))
    ));
    assert!(matches!(
        game.new_game_json("{}"),
        Err(GameError::AlreadyExists)
    ));

    let events = host.events.lock().unwrap().clone();
    assert_eq!(
        events,
        vec![
            GameEvent::TurnStarted { turn: 1 },
            GameEvent::PlayerTurnStarted { player: 0 },
        ]
    );
    let logs = host.logs.lock().unwrap().clone();
    assert!(
        logs.iter()
            .any(|l| l.contains("vfs-index.json has 5 bytes")),
        "{logs:?}"
    );
}

#[test]
fn save_then_load_in_a_new_game() {
    if !stub() {
        return;
    }
    let _serial = serial();
    let mut game = Game::new(Recorder::default()).unwrap();
    assert_eq!(game.save().err(), Some(GameError::NotStarted));
    game.new_game(&Setup {
        seed: 9,
        ..Setup::default()
    })
    .unwrap();
    // The save point is the human's turn after a pump.
    assert!(
        matches!(game.save(), Err(GameError::Rejected { message, .. }) if message.contains("save point"))
    );
    while game.pump(8).unwrap().status != 0 {}
    let bytes = game.save().unwrap();
    let info = SaveInfo::parse(&bytes).unwrap();
    assert_eq!((info.build_id.as_str(), info.turn), ("gamecore-stub/1", 1));
    let digests = (
        game.state_digest(-1).unwrap(),
        game.state_digest(0).unwrap(),
    );
    drop(game);

    let mut game = Game::new(Recorder::default()).unwrap();
    // Bad bytes do not use up the instance.
    let mut corrupt = bytes.clone();
    corrupt[40] ^= 0xff;
    assert!(
        matches!(game.load(&corrupt), Err(GameError::BadData(why)) if why.contains("checksum"))
    );
    game.load(&bytes).unwrap();
    assert_eq!(game.load(&bytes).err(), Some(GameError::AlreadyExists));
    assert_eq!(
        (
            game.state_digest(-1).unwrap(),
            game.state_digest(0).unwrap()
        ),
        digests
    );
    assert_eq!(game.pump(8).unwrap().status, 0, "the human's turn goes on");
}

#[test]
fn save_info_reads_the_gcs1_header() {
    let header = "format=GCS1 1\nbuild_id=lekmod-portable/dev/x\ndll_version=v\nabi=1\nturn=31\n";
    let mut body = Vec::new();
    body.extend_from_slice(&6u16.to_le_bytes());
    body.extend_from_slice(b"header");
    body.extend_from_slice(&(header.len() as u32).to_le_bytes());
    body.extend_from_slice(header.as_bytes());
    let mut bytes = b"GCS1".to_vec();
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&32u32.to_le_bytes());
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&(body.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&0u64.to_le_bytes());
    bytes.extend_from_slice(&body);
    let info = SaveInfo::parse(&bytes).unwrap();
    assert_eq!(info.build_id, "lekmod-portable/dev/x");
    assert_eq!(info.turn, 31);
    assert_eq!(info.format, 1);
    assert!(SaveInfo::parse(b"GCS2").is_err());
    assert!(SaveInfo::parse(&bytes[..40]).is_err());
}

/// The stub plays the B1 game only: a v2 setup is refused without using up the instance, and the
/// B1 `Setup` still starts.
#[test]
fn stub_refuses_a_network_setup() {
    if !stub() {
        return;
    }
    let _serial = serial();
    let mut game = Game::new(Recorder::default()).unwrap();
    let net = Setup::network(
        3,
        vec![
            SlotSetup::human("random", "a"),
            SlotSetup::human("random", "b"),
            SlotSetup::ai("random"),
        ],
    );
    assert_eq!(game.new_game(&net).err(), Some(GameError::NotStarted));
    let timer = Setup {
        turn_timer: Some("TURNTIMER_BLITZ".into()),
        ..Setup::default()
    };
    assert_eq!(game.new_game(&timer).err(), Some(GameError::NotStarted));
    game.new_game(&Setup::default()).unwrap();
}

/// The v2 queries on the stub: arguments are checked before the core is asked; a stub without the
/// dirty query is `None`; the empty-list shortcuts never reach the core (an empty index list means
/// the whole map to the ABI).
#[test]
fn stub_v2_queries_check_arguments_first() {
    if !stub() {
        return;
    }
    let _serial = serial();
    let mut game = Game::new(Recorder::default()).unwrap();
    game.new_game(&Setup::default()).unwrap();
    let invalid = |e: GameError| matches!(e, GameError::Core(gamecore_sys::GcError::InvalidArg(_)));
    let not_impl =
        |e: GameError| matches!(e, GameError::Core(gamecore_sys::GcError::NotImplemented));

    assert_eq!(game.plot_basis(Some(&[])), Ok(Vec::new()));
    assert_eq!(game.plots_at(0, &[]), Ok(Vec::new()));
    assert_eq!(game.plots_dirty(), Ok(None));
    for horizon in [0, 4] {
        assert!(invalid(game.reachable_tree(0, 1, horizon).unwrap_err()));
    }
    assert!(invalid(
        game.combat_preview(0, Attacker::Unit(-1), 1, 1)
            .unwrap_err()
    ));
    assert!(invalid(
        game.combat_preview(0, Attacker::City(0), 1, 1).unwrap_err()
    ));
    assert!(invalid(game.plots_at(0, &[u32::MAX]).unwrap_err()));
    assert!(invalid(game.plot_basis(Some(&[u32::MAX])).unwrap_err()));

    // Valid arguments go to the core, which the stub has not got.
    assert!(not_impl(game.plot_basis(None).unwrap_err()));
    assert!(not_impl(game.plot_basis(Some(&[3])).unwrap_err()));
    assert!(not_impl(game.plots_at(0, &[3]).unwrap_err()));
    assert!(not_impl(game.reachable_tree(0, 1, 2).unwrap_err()));
    assert!(not_impl(
        game.combat_preview(0, Attacker::Unit(1), 1, 1).unwrap_err()
    ));
    assert!(not_impl(game.diplo(0).unwrap_err()));
}

/// Slot control and the turn state on the stub: a bad slot or a NUL in the nick is refused by the
/// wrapper or the ABI check; a good call reaches the stub, which has no network game; the queries
/// that the stub does not answer say so.
#[test]
fn stub_slot_control_and_turn_state() {
    if !stub() {
        return;
    }
    let _serial = serial();
    let mut game = Game::new(Recorder::default()).unwrap();
    game.new_game(&Setup::default()).unwrap();
    let arg = |e: CommandError| {
        matches!(
            e,
            CommandError::Other(GameError::Core(gamecore_sys::GcError::InvalidArg(_)))
        )
    };
    assert!(arg(game
        .slot_control(0, SlotOp::Connect, Some("a b"))
        .unwrap_err()));
    assert!(arg(game.slot_control(200, SlotOp::Kick, None).unwrap_err()));
    assert_eq!(
        game.slot_control(1, SlotOp::Kick, None),
        Err(CommandError::Other(GameError::NotStarted))
    );
    assert!(matches!(
        game.turn_state().unwrap_err(),
        GameError::Core(gamecore_sys::GcError::NotImplemented)
    ));
    // The B1 game: one human waits, no network game.
    let r = game.pump(100).unwrap();
    assert_eq!(r.humans_left, 1);
    assert_eq!(GameType::default(), GameType::Single);
    assert_eq!(SlotOp::Connect.abi(), 1);
    assert_eq!(SlotOp::Disconnect.abi(), 2);
    assert_eq!(SlotOp::Kick.abi(), 3);
}
