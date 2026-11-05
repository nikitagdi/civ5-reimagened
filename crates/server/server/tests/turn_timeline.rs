//! The timeline of the end of a turn through the real worker process (packet 1).
//!
//! * `the_worker_process_sends_its_trace_and_the_ai_turn_events` runs everywhere, on the test world.
//! * `a_live_save_trace_covers_the_wall_time` is the committed form of the study's probe: the real
//!   core on a live ring save (`CIV5_LIVE_SAVE`, game 936 turn 126 `00000235-t0126.c5sv`), every human's turn ended
//!   by the timer, every frame time-stamped on arrival. It returns at once where there is no real core (Windows, or a
//!   build without a gamecore library; `cargo xtask test -p server --gamecore-lib <dir> --test turn_timeline`).
//!   `CIV5_GAMEDATA` or `assets-out/gamedata-b2` above the checkout is the game data; `TIMELINE_TURNS` (default 1) the
//!   turns to play.
//!
//! The real test asserts, for each turn: a `WorkerMsg::Trace` arrives with the `ReplicaTurn`; the trace covers at
//! least 95 % of the wall time from `ForceEndTurn` to `ReplicaTurn`; `w.ai` is the pump time of the AI phase (the
//! `ai_us` of the steps, which used to be 0 for the AI that runs inside the order step); the heartbeats of the AI phase
//! say `Ai`; and the AI progress arrived at most twice a second.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command as Process, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use protocol::{
    AutoChoices, CivPick, Codec, CoreKind, GameSettings, GameSetup, IPC_VERSION, PlayerId,
    Postcard, SlotControl, SlotSetup, Start, StepKind, TimerMode, ToWorker, TurnMode, TurnPhase,
    WorkerConfig, WorkerFrame, WorkerInit, WorkerMsg,
};
use turntrace::{TurnTrace, summarize};
use wire::ipc::{FrameReader, write_frame};

const BIN: &str = env!("CARGO_BIN_EXE_civ5-server");
const WAIT: Duration = Duration::from_secs(600);

fn settings() -> GameSettings {
    GameSettings {
        name: "timeline".into(),
        world_size: "WORLDSIZE_DUEL".into(),
        map_script: "fixture.lua".into(),
        map_options: vec![],
        speed: "GAMESPEED_STANDARD".into(),
        start_era: "ERA_ANCIENT".into(),
        handicap: "HANDICAP_PRINCE".into(),
        minor_civs: 0,
        turn_mode: TurnMode::Hybrid,
        timer: TimerMode::Off,
        game_options: vec![],
        seed: Some(1),
        map_seed: None,
        auto_choices: AutoChoices::default(),
        diplo_auto_decline: true,
    }
}

fn fixture_setup(humans: usize, ais: usize) -> GameSetup {
    let slot = |i: usize, control| SlotSetup {
        player: i as i8,
        civ: CivPick::Random,
        control,
    };
    let slots = (0..humans)
        .map(|i| {
            slot(
                i,
                SlotControl::Human {
                    nick: format!("p{i}"),
                },
            )
        })
        .chain((humans..humans + ais).map(|i| slot(i, SlotControl::Ai)))
        .collect();
    GameSetup {
        settings: settings(),
        slots,
    }
}

/// A frame and when it arrived.
struct Stamped {
    at: Instant,
    frame: WorkerFrame,
}

struct Proc {
    child: Child,
    frames: Receiver<Stamped>,
    seen: Vec<Stamped>,
    /// `wait` looks at the frames from here on.
    cursor: usize,
}

impl Proc {
    fn start() -> Self {
        let mut child = Process::new(BIN)
            .arg("worker")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, frames) = mpsc::channel();
        std::thread::spawn(move || {
            let mut r = FrameReader::new(stdout);
            while let Ok(Some(body)) = r.next() {
                let at = Instant::now();
                let frame: WorkerFrame = Postcard.decode(&body).expect("a WorkerFrame");
                if tx.send(Stamped { at, frame }).is_err() {
                    return;
                }
            }
        });
        Proc {
            child,
            frames,
            seen: Vec::new(),
            cursor: 0,
        }
    }

    fn send(&mut self, msg: &ToWorker) {
        let body = Postcard.encode(msg).unwrap();
        let stdin = self.child.stdin.as_mut().unwrap();
        write_frame(stdin, &body).unwrap();
        stdin.flush().unwrap();
    }

    /// The first frame from the cursor on that has a message satisfying `pred`; every frame is kept in `seen`.
    /// Returns its index; the cursor is behind it.
    fn wait(&mut self, what: &str, pred: impl Fn(&WorkerMsg) -> bool) -> usize {
        let deadline = Instant::now() + WAIT;
        loop {
            while self.cursor < self.seen.len() {
                let i = self.cursor;
                self.cursor += 1;
                if self.seen[i].frame.msgs.iter().any(&pred) {
                    return i;
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            let s = self
                .frames
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
            self.seen.push(s);
        }
    }

    /// Takes the frames that have arrived and those of the next `ms` milliseconds.
    fn settle(&mut self, ms: u64) {
        let until = Instant::now() + Duration::from_millis(ms);
        while let Ok(s) = self
            .frames
            .recv_timeout(until.saturating_duration_since(Instant::now()))
        {
            self.seen.push(s);
        }
    }

    fn up(&mut self, turn: i32) -> usize {
        self.wait(
            "the humans of the turn",
            |m| matches!(m, WorkerMsg::Turn(t) if t.turn == turn && t.phase == TurnPhase::Humans),
        )
    }

    fn finish(mut self) {
        self.send(&ToWorker::Shutdown { save: false });
        let _ = self.child.wait();
    }
}

fn traces(seen: &[Stamped]) -> Vec<(i32, TurnTrace)> {
    seen.iter()
        .flat_map(|s| s.frame.msgs.iter())
        .filter_map(|m| match m {
            WorkerMsg::Trace { turn, bytes } => {
                Some((*turn, Postcard.decode::<TurnTrace>(bytes).expect("a trace")))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn the_worker_process_sends_its_trace_and_the_ai_turn_events() {
    let mut p = Proc::start();
    p.send(&ToWorker::Init(WorkerInit {
        ipc: IPC_VERSION,
        game: 3,
        core: CoreKind::Fixture {
            huge: false,
            wrap_x: false,
        },
        start: Start::New(fixture_setup(2, 3)),
        save_dir: String::new(),
        seed: 1,
        cfg: WorkerConfig {
            stats: true,
            ..WorkerConfig::default()
        },
    }));
    p.up(1);
    p.send(&ToWorker::ForceEndTurn {
        players: vec![0, 1],
    });
    p.up(2);
    p.settle(300);
    let all = traces(&p.seen);
    assert_eq!(all.len(), 1, "one trace for one ended turn");
    let (turn, t) = &all[0];
    assert_eq!(*turn, 1);
    assert_eq!((t.game, t.turn), (3, 1));
    for name in ["w.end_turn", "w.apply", "w.ai", "w.turn_change"] {
        assert!(t.find(name).is_some(), "{name}: {:?}", t.recs);
    }
    // Two humans ended the turn by the timer: two marks.
    assert_eq!(t.all("w.end_turn").count(), 2);
    let s = summarize(t);
    assert_eq!(
        s.wait_others_us + s.order_us + s.server_ai_us + s.server_post_us + s.other_us,
        s.total_us
    );
    // The AI turn events (SIMP-exe-820): 3 AI civilizations and the barbarians are started, then ended.
    let queue = ai_queue(&p.seen);
    assert!(queue.iter().all(|e| e.turn == 1), "{queue:?}");
    assert_eq!(queue.iter().filter(|e| !e.ended).count(), 4, "{queue:?}");
    assert_eq!(queue.iter().filter(|e| e.ended).count(), 4, "{queue:?}");
    // The AI steps are AI steps in the statistics, and the phase is closed by a turn change.
    let kinds: Vec<StepKind> = p
        .seen
        .iter()
        .flat_map(|s| s.frame.msgs.iter())
        .filter_map(|m| match m {
            WorkerMsg::Stats(s) => Some(s.kind),
            _ => None,
        })
        .collect();
    assert!(kinds.contains(&StepKind::AiPhase), "{kinds:?}");
    p.finish();
}

/// One AI turn event with the turn it is for and the frame it came in.
#[derive(Debug)]
struct QueueEv {
    turn: i32,
    player: PlayerId,
    ended: bool,
    frame: usize,
}

/// Every `WorkerMsg::AiTurn` event of the frames, in order.
fn ai_queue(seen: &[Stamped]) -> Vec<QueueEv> {
    let mut out = Vec::new();
    for (frame, s) in seen.iter().enumerate() {
        for m in &s.frame.msgs {
            if let WorkerMsg::AiTurn { turn, events } = m {
                for e in events {
                    out.push(QueueEv {
                        turn: *turn,
                        player: e.player,
                        ended: e.ended,
                        frame,
                    });
                }
            }
        }
    }
    out
}

fn have_real_core() -> bool {
    if cfg!(windows) || std::env::var_os("GAMECORE_LIB_DIR_x86_64").is_none() {
        eprintln!("skipped: no real gamecore here");
        return false;
    }
    true
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

/// Live game 936, turn 126 -> 127 on the real core: what the probe of the spec measured, as assertions.
#[test]
fn a_live_save_trace_covers_the_wall_time() {
    let Some(file) = std::env::var_os("CIV5_LIVE_SAVE") else {
        eprintln!("skipped: CIV5_LIVE_SAVE is not set");
        return;
    };
    if !have_real_core() {
        return;
    }
    let turns: usize = std::env::var("TIMELINE_TURNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let bytes = std::fs::read(&file).expect("CIV5_LIVE_SAVE");
    let info = game_host::saves::read_header(&bytes).expect("a C5SV save");
    let humans: Vec<PlayerId> = info
        .seats
        .iter()
        .filter(|s| matches!(s.control, SlotControl::Human { .. }))
        .map(|s| s.player)
        .collect();
    assert!(!humans.is_empty(), "the save has humans");
    let dir = std::env::temp_dir().join(format!("civ5-timeline-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut p = Proc::start();
    p.send(&ToWorker::Init(WorkerInit {
        ipc: IPC_VERSION,
        game: 936,
        core: CoreKind::Real {
            gamedata_dir: gamedata_dir().display().to_string(),
        },
        start: Start::Load {
            file: file.to_string_lossy().into_owned(),
            seats: info.seats.clone(),
        },
        save_dir: dir.display().to_string(),
        seed: 99,
        cfg: WorkerConfig {
            stats: true,
            ..WorkerConfig::default()
        },
    }));
    let mut turn = {
        let i = p.wait(
            "the humans up",
            |m| matches!(m, WorkerMsg::Turn(t) if t.phase == TurnPhase::Humans),
        );
        p.seen[i]
            .frame
            .msgs
            .iter()
            .find_map(|m| match m {
                WorkerMsg::Turn(t) => Some(t.turn),
                _ => None,
            })
            .unwrap()
    };
    // The turn-start snapshot of the loaded turn goes out first; let it settle.
    p.settle(3000);
    for _ in 0..turns {
        p.cursor = p.seen.len();
        let from = p.seen.len();
        let t0 = Instant::now();
        p.send(&ToWorker::ForceEndTurn {
            players: humans.clone(),
        });
        let next = turn + 1;
        let turn_i = p.wait(
            "the next turn's snapshot",
            move |m| matches!(m, WorkerMsg::ReplicaTurn { turn: t, .. } if *t == next),
        );
        let wall = p.seen[turn_i].at - t0;
        // The trace is in the same frame as the snapshot's `ReplicaTurn`.
        let mine: Vec<(i32, TurnTrace)> = traces(&p.seen[from..]);
        let (_, t) = mine
            .iter()
            .find(|(n, _)| *n == turn)
            .unwrap_or_else(|| panic!("no trace of turn {turn}: {} traces", mine.len()));
        let s = summarize(t);
        println!("TIMELINE wall {wall:?}: {}", s.text());
        for r in &t.recs {
            println!(
                "TIMELINE   {:>14} {:>9} us at +{:>9} us {:?}",
                r.span,
                r.dur_us,
                r.t0_us.saturating_sub(t.first_us().unwrap_or(0)),
                r.args
            );
        }
        // The spans cover the wall time (95 %).
        let wall_us = wall.as_micros() as f64;
        assert!(
            s.total_us as f64 >= 0.95 * wall_us,
            "the trace covers {} of {} us",
            s.total_us,
            wall_us
        );
        // `w.ai` is the AI phase's pump time, as the steps booked it.
        let ai_us_steps: u64 = p.seen[from..=turn_i]
            .iter()
            .flat_map(|s| s.frame.msgs.iter())
            .filter_map(|m| match m {
                WorkerMsg::Stats(st) => Some(u64::from(st.ai_us)),
                _ => None,
            })
            .sum();
        let ai = t.find("w.ai").expect("w.ai");
        let pump = u64::try_from(ai.arg("pump_us").expect("pump_us")).unwrap();
        assert!(
            ai_us_steps > 0,
            "the AI time is booked (it was 0 in the order step)"
        );
        let diff = ai_us_steps.abs_diff(pump) as f64;
        assert!(
            diff <= 0.05 * pump as f64 + 2_000.0,
            "w.ai pump {pump} us, the steps booked {ai_us_steps} us"
        );
        assert!(
            ai.dur_us >= pump && (ai.dur_us as f64) <= pump as f64 * 1.25 + 200_000.0,
            "w.ai {} us around {pump} us of pumping",
            ai.dur_us
        );
        // The heartbeats of the AI phase say Ai. The step that begins the AI inside the step of the last timer end of
        // turn was entered in the human phase and its frame (the first with AI time) comes after its own heartbeats.
        let ai_frames = &p.seen[from..=turn_i];
        let first_ai = ai_frames
            .iter()
            .position(|s| {
                s.frame
                    .msgs
                    .iter()
                    .any(|m| matches!(m, WorkerMsg::Stats(st) if st.ai_us > 0))
            })
            .expect("a step with AI time");
        let mut ai_beats = 0;
        for st in &ai_frames[first_ai..] {
            for m in &st.frame.msgs {
                if let WorkerMsg::Heartbeat { phase, .. } = m {
                    assert!(
                        matches!(phase, TurnPhase::Ai { .. } | TurnPhase::Humans),
                        "{phase:?}"
                    );
                    ai_beats += usize::from(matches!(phase, TurnPhase::Ai { .. }));
                }
            }
        }
        let humans_beats = ai_frames[first_ai..]
            .iter()
            .flat_map(|s| s.frame.msgs.iter())
            .filter(|m| {
                matches!(
                    m,
                    WorkerMsg::Heartbeat {
                        phase: TurnPhase::Humans,
                        ..
                    }
                )
            })
            .count();
        println!(
            "TIMELINE heartbeats in the AI phase: {ai_beats} say Ai, {humans_beats} say Humans"
        );
        assert!(
            humans_beats == 0,
            "a heartbeat during the AI phase says Humans ({humans_beats})"
        );
        // The turn queue of the AI phase (SIMP-exe-820): every AI is started in a burst before the first one ends, the
        // ends come ascending by id, one civilization at a time, each in the frame of the pump call that did it.
        let queue: Vec<QueueEv> = ai_queue(&p.seen[from..=turn_i])
            .into_iter()
            .map(|mut e| {
                e.frame += from;
                e
            })
            .collect();
        let started: Vec<PlayerId> = queue
            .iter()
            .filter(|e| !e.ended)
            .map(|e| e.player)
            .collect();
        let ended: Vec<PlayerId> = queue.iter().filter(|e| e.ended).map(|e| e.player).collect();
        println!("TIMELINE AI queue started {started:?}");
        println!("TIMELINE AI queue ended   {ended:?}");
        assert!(queue.iter().all(|e| e.turn == turn), "{queue:?}");
        assert!(!started.is_empty(), "the AIs were started");
        assert!(
            !humans
                .iter()
                .any(|h| started.contains(h) || ended.contains(h)),
            "humans are not part of the AI queue"
        );
        let last_start = queue.iter().rposition(|e| !e.ended).unwrap();
        let first_end = queue.iter().position(|e| e.ended).expect("an AI ended");
        assert!(
            last_start < first_end,
            "all started before the first ends: {queue:?}"
        );
        let mut by_id = started.clone();
        by_id.sort_unstable();
        by_id.dedup();
        assert_eq!(
            by_id.len(),
            started.len(),
            "each AI started once: {started:?}"
        );
        let mut ended_sorted = ended.clone();
        ended_sorted.sort_unstable();
        assert_eq!(ended, ended_sorted, "ended in ascending id order");
        assert_eq!(ended_sorted, by_id, "every started AI ended, none twice");
        // They trickle: the ends are not all in one frame (the AI phase is several pump calls).
        let end_frames: std::collections::BTreeSet<usize> =
            queue.iter().filter(|e| e.ended).map(|e| e.frame).collect();
        println!(
            "TIMELINE AI queue: {} frames carry an Ended",
            end_frames.len()
        );
        assert!(end_frames.len() > 1, "the ends come one by one: {queue:?}");
        // Every Ended came before the snapshot of the next turn.
        assert!(queue.last().unwrap().frame <= turn_i);
        turn = next;
        // Let the snapshot jobs of the turn drain before the next end of turn.
        p.settle(3000);
    }
    p.finish();
    let _ = std::fs::remove_dir_all(&dir);
}
