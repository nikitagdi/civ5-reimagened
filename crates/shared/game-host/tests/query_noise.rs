//! Query noise and turn digests on the test world (game-host-mp 4.17, AC-N1.4): `cfg.digests` puts the digest of
//! every turn end into the stats, `cfg.query_noise` asks the core random questions after every order. Two games
//! with the same seed and the same orders, one with the noise, one without, must have the same digests. The
//! same test on the real core is `server/tests/worker_real.rs`.

use game_host::fixture::FixtureCore;
use game_host::{Out, Worker};
use protocol::{
    AutoChoices, Cmd, Command, CoreKind, GameSettings, GameSetup, IPC_VERSION, PlayerId,
    SlotControl, SlotSetup, Start, TimerMode, ToWorker, TurnMode, WorkerConfig, WorkerInit,
    WorkerMsg,
};

fn game() -> GameSetup {
    let slot = |player: PlayerId, control| SlotSetup {
        player,
        civ: protocol::CivPick::Random,
        control,
    };
    GameSetup {
        settings: GameSettings {
            name: "noise".into(),
            world_size: "WORLDSIZE_DUEL".into(),
            map_script: "fixture.lua".into(),
            map_options: Vec::new(),
            speed: "GAMESPEED_STANDARD".into(),
            start_era: "ERA_ANCIENT".into(),
            handicap: "HANDICAP_PRINCE".into(),
            minor_civs: 0,
            turn_mode: TurnMode::Simultaneous,
            timer: TimerMode::Off,
            game_options: Vec::new(),
            seed: Some(3),
            map_seed: None,
            auto_choices: AutoChoices::default(),
            diplo_auto_decline: true,
        },
        slots: vec![
            slot(0, SlotControl::Human { nick: "a".into() }),
            slot(1, SlotControl::Human { nick: "b".into() }),
            slot(2, SlotControl::Ai),
        ],
    }
}

/// Plays `turns` turns: both seats fortify their first unit and end the turn. Returns the digests of the
/// turn ends.
fn play(noise: u32, turns: i32) -> Vec<u64> {
    let cfg = WorkerConfig {
        digests: true,
        stats: true,
        query_noise: noise,
        ..WorkerConfig::default()
    };
    let init = WorkerInit {
        ipc: IPC_VERSION,
        game: 1,
        core: CoreKind::Fixture {
            huge: false,
            wrap_x: false,
        },
        start: Start::New(game()),
        save_dir: String::new(),
        seed: 11,
        cfg,
    };
    let (mut w, outs) = Worker::new(FixtureCore::new(false, false), init, 0);
    let mut digests = Vec::new();
    let mut now = 0;
    let mut next_id = 0;
    let take = |outs: Vec<Out>, digests: &mut Vec<u64>| {
        for o in outs {
            if let Out::Frame(f) = o {
                for m in f.msgs {
                    match m {
                        WorkerMsg::Stats(s) => digests.extend(s.digest),
                        WorkerMsg::Fatal { message, .. } => panic!("fatal: {message}"),
                        _ => {}
                    }
                }
            }
        }
    };
    take(outs, &mut digests);
    let run = |w: &mut Worker<FixtureCore>, digests: &mut Vec<u64>, now: &mut u64| {
        for _ in 0..1000 {
            if !w.wants_step() {
                return;
            }
            *now += 1;
            take(w.step(*now), digests);
        }
        panic!("never idle");
    };
    run(&mut w, &mut digests, &mut now);
    for _ in 0..turns {
        let turn = w.phase().0;
        for p in [0, 1] {
            let unit = w
                .fresh_view(p)
                .unwrap()
                .units
                .iter()
                .find(|u| u.owner == p)
                .map(|u| u.id)
                .unwrap();
            for cmd in [
                Command::UnitFortify { unit },
                Command::EndTurn { force: false },
            ] {
                next_id += 1;
                now += 1;
                let outs = w.handle(
                    ToWorker::Cmd {
                        player: p,
                        cmd: Cmd {
                            id: next_id,
                            turn,
                            cmd,
                        },
                    },
                    now,
                );
                take(outs, &mut digests);
                run(&mut w, &mut digests, &mut now);
            }
        }
        assert_eq!(w.phase().0, turn + 1);
    }
    digests
}

#[test]
fn the_noise_does_not_change_the_digests() {
    let quiet = play(0, 6);
    let noisy = play(50, 6);
    assert!(quiet.len() >= 6, "a digest per turn end: {quiet:?}");
    assert_eq!(quiet, noisy);
    // The digest says something: the game moves on.
    assert!(quiet.windows(2).all(|w| w[0] != w[1]), "{quiet:?}");
}
