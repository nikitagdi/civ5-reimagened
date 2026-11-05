//! The multiplayer worker on the real core (game-host-mp section 9 "Тесты на настоящем мозге", N3.31):
//! `Worker<RealCore>`, a Duel with two humans and two AI. Returns early on the stub; run on Linux/WSL with
//! `GAMECORE_LIB_DIR_x86_64` set (`cargo xtask test -p game-host --gamecore-lib <dir> --test real_mp`) and the
//! game data in `CIV5_GAMEDATA` (or `assets-out/gamedata-b2` above the checkout).
//!
//! The real core is a process singleton, so there is one test and the restart runs in a second process: the
//! test starts its own binary again with `GH_RT_SAVE` set, and that run loads the save into a new
//! `Worker`, prints the hashes of the views it sends, and plays a turn; the parent compares them with the
//! hashes it saw when the save was made.
//!
//! The flow: start; the orders of both humans (a city each); ten turns, each with the end of turn of both,
//! the view built from the diffs equal to the view built from scratch (the dirty plots of the core are
//! complete), the fog auditor silent; the kick of one human and its return; a save; the restart.

use std::process::Command as Process;

use game_host::Worker;
use game_host::saves;
use protocol::{
    PlayerId, SeatChange, SlotControl, Start, ToWorker, TurnPhase, ViewUpdate, WorkerMsg,
};

#[path = "common/real.rs"]
mod common;
use common::{H, NICKS, init, real_core, setup};

const TEST_NAME: &str = "real_mp_duel_two_humans";

/// The picture of a worker: the turn and the hash of every view.
fn picture(turn: i32, hashes: &[u64]) -> String {
    let hs: Vec<String> = hashes.iter().map(|h| format!("{h:016x}")).collect();
    format!("turn={turn} views={}", hs.join(","))
}

/// The second process: load the save, report the views, play a turn.
fn child(path: &str) {
    let bytes = std::fs::read(path).unwrap();
    let save = saves::decode(&bytes).expect("the save decodes");
    let seats = save.info.seats.clone();
    let turn = save.info.header.turn;
    let (w, outs) = Worker::resume(
        real_core(),
        init(
            Start::Load {
                file: "t.c5sv".into(),
                seats,
            },
            "",
        ),
        save,
        0,
    );
    let mut h = H::with(w, outs);
    let humans = [0, 1];
    assert_eq!(h.w.phase(), (turn, TurnPhase::Humans));
    // Every human seat got the whole view, and the map basis first.
    for &p in &humans {
        assert!(
            h.msgs().any(|m| matches!(
                m,
                WorkerMsg::View {
                    player,
                    update: ViewUpdate::Full { .. }
                } if *player == p
            )),
            "no Full view for seat {p}"
        );
    }
    assert!(h.msgs().any(|m| matches!(m, WorkerMsg::Basis { .. })));
    println!("PICTURE loaded {}", picture(turn, &h.hashes(&humans)));
    h.check_views(&humans);
    // Nobody logged in: the load does not connect the humans (decision 2026-10-27), the turn still waits.
    let off_line = |h: &mut H| {
        h.w.core_mut()
            .human_states()
            .unwrap()
            .iter()
            .all(|(_, alive, connected)| *alive && !*connected)
    };
    assert!(off_line(&mut h), "the humans are on line after the load");
    assert_eq!(h.w.phase(), (turn, TurnPhase::Humans));
    h.end_turn(0);
    h.run();
    assert_eq!(
        h.w.phase(),
        (turn, TurnPhase::Humans),
        "seat 1 has not ended"
    );
    h.end_turn(1);
    h.run();
    assert_eq!(h.w.phase().0, turn + 1);
    assert!(off_line(&mut h), "the humans are on line");
    println!("TURN {}", h.w.phase().0);
}

#[test]
fn real_mp_duel_two_humans() {
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return;
    }
    if let Ok(path) = std::env::var("GH_RT_SAVE") {
        child(&path);
        return;
    }

    let humans: [PlayerId; 2] = [0, 1];
    let (w, outs) = Worker::new(real_core(), init(Start::New(setup()), "saves"), 0);
    let mut h = H::with(w, outs);
    assert_eq!(h.w.phase().1, TurnPhase::Humans);
    let start_turn = h.w.phase().0;

    // Start: the build id, the map, a basis and a Full view for each human, who see different things.
    let ready = h
        .msgs()
        .find_map(|m| match m {
            WorkerMsg::Ready(r) => Some(r.clone()),
            _ => None,
        })
        .expect("Ready");
    assert!(ready.width > 0 && ready.height > 0, "{ready:?}");
    assert_eq!(ready.seats.len(), 4, "{:?}", ready.seats);
    assert!(
        matches!(&ready.seats[0].control, SlotControl::Human { nick } if nick == NICKS[0]),
        "{:?}",
        ready.seats
    );
    assert!(h.msgs().any(|m| matches!(m, WorkerMsg::Basis { .. })));
    let v0 = h.w.fresh_view(0).unwrap();
    let v1 = h.w.fresh_view(1).unwrap();
    assert!(v0.units.iter().any(|u| u.owner == 0));
    assert!(
        !v0.units.iter().any(|u| u.owner == 1) && !v1.units.iter().any(|u| u.owner == 0),
        "at the start neither human sees the other's units"
    );
    assert_ne!(v0.human_team, v1.human_team);
    h.check_views(&humans);

    // The orders of both humans.
    for p in humans {
        h.found_city(p);
    }
    h.check_views(&humans);
    for p in humans {
        let v = h.w.fresh_view(p).unwrap();
        assert_eq!(v.cities.iter().filter(|c| c.owner == p).count(), 1);
    }

    // Ten turns.
    for _ in 0..10 {
        h.next_turn(&humans);
        h.check_views(&humans);
    }
    assert_eq!(h.w.phase().0, start_turn + 10);

    // Kick the second human: the AI plays its slot, the turn no longer waits for it.
    h.send(ToWorker::Seat {
        player: 1,
        change: SeatChange::Kick,
    });
    h.run();
    let kicked = h.msgs().any(|m| {
        matches!(
            m,
            WorkerMsg::SeatDone {
                player: 1,
                change: SeatChange::Kick,
                error: None
            }
        )
    });
    assert!(kicked, "the kick was done");
    h.next_turn(&[0]);
    // The human takes the slot back.
    h.send(ToWorker::Seat {
        player: 1,
        change: SeatChange::Claim {
            nick: NICKS[1].into(),
        },
    });
    h.run();
    let claimed = h.msgs().any(|m| {
        matches!(
            m,
            WorkerMsg::SeatDone {
                player: 1,
                change: SeatChange::Claim { .. },
                error: None
            }
        )
    });
    assert!(claimed, "the claim was done");
    h.check_views(&humans);
    h.next_turn(&humans);
    h.check_views(&humans);

    // The save of the start of this turn, and the hashes of the views at that moment.
    let job = h.jobs.last().expect("a save of the turn start").clone();
    let turn = h.w.phase().0;
    assert_eq!(job.file.turn, turn);
    let saved = picture(turn, &h.hashes(&humans));
    let mut header = job.header.clone();
    header.saved_at_ms = 1;
    let bytes = saves::encode(&header, &job.gcs1, &job.sections);

    // Load it in a new process.
    let path = std::env::temp_dir().join(format!("gh-real-mp-{}.c5sv", std::process::id()));
    std::fs::write(&path, &bytes).unwrap();
    let out = Process::new(std::env::current_exe().unwrap())
        .args([TEST_NAME, "--exact", "--nocapture", "--test-threads=1"])
        .env("GH_RT_SAVE", &path)
        .output()
        .unwrap();
    let _ = std::fs::remove_file(&path);
    let stdout = String::from_utf8_lossy(&out.stdout);
    eprintln!(
        "child process:\n{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.status.success(), "the loading process failed");
    let lines: Vec<&str> = stdout
        .lines()
        .filter_map(|l| l.find("PICTURE ").map(|i| &l[i..]))
        .collect();
    assert_eq!(
        lines.first().copied(),
        Some(format!("PICTURE loaded {saved}").as_str()),
        "the views after the restart are the ones the save was made with"
    );
    assert!(
        stdout.contains(&format!("TURN {}", turn + 1)),
        "the loaded game played on: {stdout}"
    );
}
