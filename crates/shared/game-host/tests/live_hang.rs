//! A live save that hung the worker (game 936, turn 126, 2026-10-31): `CIV5_LIVE_SAVE=<a .c5sv of the server>` is
//! loaded as the worker loads it (`Worker::resume` on the real core), every human seat's turn is ended by the timer
//! (`ForceEndTurn`), and the worker runs until the next turn's humans are up. Every step that takes longer than a
//! second is printed with the phase. Run on Linux/WSL with `GAMECORE_LIB_DIR_x86_64` and `CIV5_GAMEDATA`; skipped
//! otherwise. `CIV5_LIVE_TURNS` (default 1): how many turns to play.

use std::time::{Duration, Instant};

use game_host::saves;
use game_host::{Core, Worker};
use protocol::{SlotControl, Start, ToWorker, TurnPhase};

#[path = "common/real.rs"]
mod common;
use common::{H, init, real_core};

/// The longest a turn may take here; the hang ran for minutes.
const TURN_LIMIT: Duration = Duration::from_secs(600);

#[test]
fn live_save_plays_its_turn_to_the_end() {
    let Some(file) = std::env::var_os("CIV5_LIVE_SAVE") else {
        eprintln!("skipped: CIV5_LIVE_SAVE is not set");
        return;
    };
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return;
    }
    let turns: i32 = std::env::var("CIV5_LIVE_TURNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let bytes = std::fs::read(&file).expect("CIV5_LIVE_SAVE");
    let save = saves::decode(&bytes).expect("a C5SV save");
    let seats = save.info.seats.clone();
    let turn = save.info.header.turn;
    eprintln!("turn {turn} seats {seats:?}");
    let humans: Vec<i8> = seats
        .iter()
        .filter(|s| matches!(s.control, SlotControl::Human { .. }))
        .map(|s| s.player)
        .collect();
    let t0 = Instant::now();
    let (w, outs) = Worker::resume(
        real_core(),
        init(
            Start::Load {
                file: "live.c5sv".into(),
                seats,
            },
            // The harness keeps the save jobs in memory (no file is written): the save of each turn start.
            "live",
        ),
        save,
        0,
    );
    let mut h = H::with(w, outs);
    eprintln!(
        "loaded in {:?}: phase {:?}, humans {humans:?}",
        t0.elapsed(),
        h.w.phase()
    );
    for t in 0..turns {
        let start = h.w.phase().0;
        assert_eq!(h.w.phase(), (turn + t, TurnPhase::Humans));
        h.send(ToWorker::ForceEndTurn {
            players: humans.clone(),
        });
        let began = Instant::now();
        let mut steps = 0u64;
        loop {
            while let Some(d) = h.unanswered.pop() {
                h.now += 1;
                let outs = h.w.save_done(d, h.now);
                h.feed(outs);
            }
            if !h.w.wants_step() {
                break;
            }
            let before = h.w.phase();
            let ts = Instant::now();
            h.now += 1;
            let outs = h.w.step(h.now);
            h.feed(outs);
            steps += 1;
            if ts.elapsed() > Duration::from_secs(1) {
                eprintln!(
                    "step {steps}: {:?} in {before:?} -> {:?}",
                    ts.elapsed(),
                    h.w.phase()
                );
            }
            assert!(
                began.elapsed() < TURN_LIMIT,
                "turn {start} did not end in {TURN_LIMIT:?}"
            );
        }
        let took = began.elapsed();
        assert_eq!(h.w.phase(), (start + 1, TurnPhase::Humans));
        // The proof that a change of the core keeps the game: the digest of the whole state and the hash of the
        // core's part of the save of the new turn's start are the same before and after it.
        let digest = h.w.core_mut().digest(-1).expect("digest");
        let save = h
            .jobs
            .iter()
            .rev()
            .find(|j| j.file.turn == start + 1)
            .map(|j| protocol::xxh64(0, &j.gcs1));
        eprintln!(
            "TURN {start} ended in {:.3} s, {steps} steps; digest {digest:016x} save {}",
            took.as_secs_f64(),
            save.map_or("none".to_owned(), |s| format!("{s:016x}"))
        );
    }
}
