//! The worker shell (`civ5-server worker`, game-host-mp 4.16, section 9 "Тест оболочки") as a child process on
//! the test world: `Init`, an order and its answer, `Heartbeat`, `Shutdown`, the exit codes.

use std::collections::VecDeque;
use std::io::Write;
use std::process::{Child, Command as Process, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use protocol::{
    AutoChoices, CivPick, Cmd, Codec, Command, CommandOutcome, CoreKind, GameSettings, GameSetup,
    IPC_VERSION, Postcard, SlotControl, SlotSetup, Start, TimerMode, ToWorker, TurnMode, TurnPhase,
    ViewUpdate, WorkerConfig, WorkerFrame, WorkerInit, WorkerMsg,
};
use wire::ipc::{FrameReader, write_frame};

const BIN: &str = env!("CARGO_BIN_EXE_civ5-server");
const WAIT: Duration = Duration::from_secs(20);

fn setup(humans: usize, ais: usize) -> GameSetup {
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
        settings: GameSettings {
            name: "shell".into(),
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
            mods: Vec::new(),
        },
        slots,
    }
}

fn init(core: CoreKind, start: Start) -> WorkerInit {
    WorkerInit {
        ipc: IPC_VERSION,
        game: 3,
        core,
        start,
        save_dir: String::new(),
        seed: 1,
        cfg: WorkerConfig::default(),
    }
}

fn fixture() -> CoreKind {
    CoreKind::Fixture {
        huge: false,
        wrap_x: false,
    }
}

/// A worker process with its frames collected on a thread.
struct Proc {
    child: Child,
    frames: Receiver<WorkerFrame>,
    /// Every message seen so far, in order.
    seen: Vec<WorkerMsg>,
    /// Messages of the frames read but not yet looked at by `wait_msg`.
    pending: VecDeque<WorkerMsg>,
}

impl Proc {
    fn start() -> Self {
        let mut child = Process::new(BIN)
            .arg("worker")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, frames) = mpsc::channel();
        std::thread::spawn(move || {
            let mut r = FrameReader::new(stdout);
            while let Ok(Some(body)) = r.next() {
                let frame: WorkerFrame = Postcard
                    .decode(&body)
                    .expect("the worker wrote something that is not a WorkerFrame");
                if tx.send(frame).is_err() {
                    return;
                }
            }
        });
        Proc {
            child,
            frames,
            seen: Vec::new(),
            pending: VecDeque::new(),
        }
    }

    fn send_raw(&mut self, body: &[u8]) {
        let stdin = self.child.stdin.as_mut().unwrap();
        write_frame(stdin, body).unwrap();
        stdin.flush().unwrap();
    }

    fn send(&mut self, msg: &ToWorker) {
        let body = Postcard.encode(msg).unwrap();
        self.send_raw(&body);
    }

    /// Looks at the messages in order until one satisfies `pred` (the ones before it are dropped, the ones
    /// after it stay for the next call); returns that message.
    fn wait_msg(&mut self, what: &str, pred: impl Fn(&WorkerMsg) -> bool) -> WorkerMsg {
        let deadline = Instant::now() + WAIT;
        loop {
            while let Some(m) = self.pending.pop_front() {
                if pred(&m) {
                    return m;
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            let frame = self
                .frames
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("no {what} within {WAIT:?}"));
            self.seen.extend(frame.msgs.iter().cloned());
            self.pending.extend(frame.msgs);
        }
    }

    /// The exit code, within `WAIT`.
    fn exit_code(&mut self) -> Option<i32> {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.code();
            }
            if Instant::now() > deadline {
                let _ = self.child.kill();
                panic!("the worker did not exit within {WAIT:?}");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn all_msgs(&mut self) -> Vec<WorkerMsg> {
        while let Ok(f) = self.frames.try_recv() {
            self.seen.extend(f.msgs);
        }
        self.seen.clone()
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn a_game_runs_an_order_a_heartbeat_and_a_clean_shutdown() {
    let mut p = Proc::start();
    p.send(&ToWorker::Init(init(fixture(), Start::New(setup(1, 1)))));
    // Handshake: Catalog, then Ready (same frame or not).
    p.wait_msg("Catalog", |m| matches!(m, WorkerMsg::Catalog { .. }));
    let WorkerMsg::Ready(ready) = p.wait_msg("Ready", |m| matches!(m, WorkerMsg::Ready(_))) else {
        unreachable!()
    };
    assert_eq!(ready.build_id, "fixture");
    assert_eq!((ready.width, ready.height), (40, 24));
    // The first round: a Full view of the human seat, then the turn in the human phase.
    p.wait_msg("a Full view", |m| {
        matches!(
            m,
            WorkerMsg::View {
                player: 0,
                update: ViewUpdate::Full { .. }
            }
        )
    });
    let turn = |m: &WorkerMsg| matches!(m, WorkerMsg::Turn(t) if t.phase == TurnPhase::Humans);
    let WorkerMsg::Turn(first) = p.wait_msg("Turn in the human phase", turn) else {
        unreachable!()
    };
    // An order and its answer.
    p.send(&ToWorker::Cmd {
        player: 0,
        cmd: Cmd {
            id: 41,
            turn: first.turn,
            cmd: Command::EndTurn { force: false },
        },
    });
    let answer = p.wait_msg("Results for the order", |m| {
        matches!(m, WorkerMsg::Results { player: 0, results } if results.iter().any(|r| r.id == 41))
    });
    let WorkerMsg::Results { results, .. } = answer else {
        unreachable!()
    };
    assert!(
        results
            .iter()
            .any(|r| r.id == 41 && matches!(r.outcome, CommandOutcome::Applied)),
        "{results:?}"
    );
    // The AI plays and the next turn begins.
    p.wait_msg(
        "the next turn",
        |m| matches!(m, WorkerMsg::Turn(t) if t.phase == TurnPhase::Humans && t.turn > first.turn),
    );
    // Quiet for a second: a heartbeat (GH-D16).
    p.wait_msg("Heartbeat", |m| matches!(m, WorkerMsg::Heartbeat { .. }));
    p.send(&ToWorker::Shutdown { save: false });
    assert_eq!(p.exit_code(), Some(0));
}

#[test]
fn a_frame_that_does_not_parse_ends_the_worker_with_code_2() {
    let mut p = Proc::start();
    p.send(&ToWorker::Init(init(fixture(), Start::New(setup(1, 1)))));
    p.wait_msg("Ready", |m| matches!(m, WorkerMsg::Ready(_)));
    p.send_raw(&[0xff; 9]);
    assert_eq!(p.exit_code(), Some(2));
    let fatal = p.all_msgs().into_iter().find_map(|m| match m {
        WorkerMsg::Fatal { code, .. } => Some(code),
        _ => None,
    });
    assert_eq!(fatal, Some(2));
}

#[test]
fn a_closed_stdin_is_a_shutdown() {
    let mut p = Proc::start();
    p.send(&ToWorker::Init(init(fixture(), Start::New(setup(1, 1)))));
    p.wait_msg("Ready", |m| matches!(m, WorkerMsg::Ready(_)));
    drop(p.child.stdin.take());
    assert_eq!(p.exit_code(), Some(0));
}

#[test]
fn a_first_frame_that_is_not_init_is_an_ipc_error() {
    let mut p = Proc::start();
    p.send(&ToWorker::Shutdown { save: false });
    assert_eq!(p.exit_code(), Some(2));
    assert!(
        p.all_msgs()
            .iter()
            .any(|m| matches!(m, WorkerMsg::Fatal { code: 2, .. }))
    );
}

#[test]
fn a_foreign_ipc_version_is_an_ipc_error() {
    let mut p = Proc::start();
    let mut i = init(fixture(), Start::New(setup(1, 1)));
    i.ipc = IPC_VERSION + 1;
    p.send(&ToWorker::Init(i));
    assert_eq!(p.exit_code(), Some(2));
    assert!(
        p.all_msgs()
            .iter()
            .any(|m| matches!(m, WorkerMsg::Fatal { code: 2, .. }))
    );
}

#[test]
fn the_real_core_does_not_start_here() {
    let mut p = Proc::start();
    let core = CoreKind::Real {
        gamedata_dir: String::new(),
    };
    p.send(&ToWorker::Init(init(core, Start::New(setup(1, 1)))));
    assert_eq!(p.exit_code(), Some(5));
    assert!(
        p.all_msgs()
            .iter()
            .any(|m| matches!(m, WorkerMsg::Fatal { code: 5, .. }))
    );
}

// ---------------------------------------------------------------- Describe (N2.15)

#[test]
fn describe_answers_choices_catalog_and_the_build_id_and_exits() {
    let mut p = Proc::start();
    p.send(&ToWorker::Init(init(fixture(), Start::Describe)));
    assert_eq!(p.exit_code(), Some(0));
    let msgs = p.all_msgs();
    let choices = msgs.iter().find_map(|m| match m {
        WorkerMsg::Choices(c) => Some(c),
        _ => None,
    });
    let choices = choices.expect("Choices");
    assert!(
        !choices.civs.is_empty(),
        "no civilizations in the lobby lists"
    );
    assert!(
        msgs.iter()
            .any(|m| matches!(m, WorkerMsg::Catalog { bytes, .. } if !bytes.is_empty()))
    );
    let ready = msgs.iter().find_map(|m| match m {
        WorkerMsg::Ready(r) => Some(r),
        _ => None,
    });
    assert_eq!(ready.expect("Ready").build_id, "fixture");
    // No game: no views.
    assert!(!msgs.iter().any(|m| matches!(m, WorkerMsg::View { .. })));
}

#[test]
fn the_describe_subcommand_prints_the_lists_and_the_build_id() {
    let dir = std::env::temp_dir().join(format!("civ5-describe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let out = Process::new(BIN)
        .args(["describe", "--json", "--core", "fixture", "--state-dir"])
        .arg(&dir)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{text}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_str(&text).expect("describe --json prints JSON");
    assert_eq!(v["build_id"], "fixture");
    assert!(
        v["choices"]["civs"]
            .as_array()
            .is_some_and(|a| !a.is_empty()),
        "{text}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------- saves and restart (N3.29)

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use protocol::{SaveReason, SaveRecord};

/// A directory of its own for the saves of one test, empty.
fn save_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("civ5-worker-saves-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn init_in(dir: &Path, start: Start) -> WorkerInit {
    let mut i = init(fixture(), start);
    i.save_dir = dir.to_string_lossy().into_owned();
    i
}

/// Waits for the first round of a game: every human seat has its view.
fn wait_first_views(p: &mut Proc, humans: i8) {
    for seat in 0..humans {
        p.wait_msg("a Full view", move |m| {
            matches!(m, WorkerMsg::View { player, update: ViewUpdate::Full { .. } } if *player == seat)
        });
    }
}

fn saved_records(msgs: &[WorkerMsg]) -> Vec<SaveRecord> {
    msgs.iter()
        .filter_map(|m| match m {
            WorkerMsg::Saved(r) => Some(r.clone()),
            _ => None,
        })
        .collect()
}

/// The hash of the last view every seat was sent.
fn last_hashes(msgs: &[WorkerMsg]) -> BTreeMap<i8, u64> {
    let mut out = BTreeMap::new();
    for m in msgs {
        if let WorkerMsg::View { player, update } = m {
            let h = match update {
                ViewUpdate::Full { hash, .. } | ViewUpdate::Delta { hash, .. } => *hash,
            };
            out.insert(*player, h);
        }
    }
    out
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = ["ring", "keep"]
        .iter()
        .flat_map(|d| {
            std::fs::read_dir(dir.join(d))
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .map(move |e| format!("{d}/{}", e.file_name().to_string_lossy()))
        })
        .collect();
    v.sort();
    v
}

#[test]
fn shutdown_with_save_writes_the_file_tells_the_gateway_and_exits_with_zero() {
    let dir = save_dir("shutdown");
    let mut p = Proc::start();
    p.send(&ToWorker::Init(init_in(&dir, Start::New(setup(2, 1)))));
    wait_first_views(&mut p, 2);
    p.send(&ToWorker::Shutdown { save: true });
    // The start-of-turn save and the final one: both are told.
    let last = p.wait_msg(
        "Saved",
        |m| matches!(m, WorkerMsg::Saved(r) if r.meta.seq == 2),
    );
    assert_eq!(p.exit_code(), Some(0));
    let WorkerMsg::Saved(rec) = last else {
        unreachable!()
    };
    assert_eq!(rec.file, "ring/00000002-t0001.c5sv");
    assert!(
        rec.meta.saved_at_ms > 1_600_000_000_000,
        "a wall clock time"
    );
    let path = dir.join(&rec.file);
    let bytes = std::fs::read(&path).expect("the file is there");
    let info = game_host::saves::read_header(&bytes).expect("a good header");
    assert_eq!(
        (info.header.seq, info.header.turn, info.header.game),
        (2, 1, 3)
    );
    assert_eq!(info.header.build_id, "fixture");
    assert_eq!(info.seats.len(), 3);
    assert_eq!(files_in(&dir).len(), 2);
    assert!(
        !std::fs::read_dir(dir.join("ring")).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")),
        "no .tmp file is left"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_closed_stdin_saves_before_the_exit() {
    let dir = save_dir("eof");
    let mut p = Proc::start();
    p.send(&ToWorker::Init(init_in(&dir, Start::New(setup(2, 1)))));
    wait_first_views(&mut p, 2);
    drop(p.child.stdin.take());
    assert_eq!(p.exit_code(), Some(0));
    // Whatever the gateway saw of it, the final save is on the disk.
    let files = files_in(&dir);
    assert_eq!(files.len(), 2, "{files:?}");
    assert!(files.contains(&"ring/00000002-t0001.c5sv".to_owned()));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_killed_worker_is_continued_from_its_save_with_the_same_views() {
    let dir = save_dir("restart");
    let mut p = Proc::start();
    p.send(&ToWorker::Init(init_in(&dir, Start::New(setup(2, 1)))));
    wait_first_views(&mut p, 2);
    let turn = |m: &WorkerMsg| matches!(m, WorkerMsg::Turn(t) if t.phase == TurnPhase::Humans);
    let WorkerMsg::Turn(first) = p.wait_msg("Turn", turn) else {
        unreachable!()
    };
    // Seat 0 ends its turn; seat 1 is still to move.
    p.send(&ToWorker::Cmd {
        player: 0,
        cmd: Cmd {
            id: 1,
            turn: first.turn,
            cmd: Command::EndTurn { force: false },
        },
    });
    p.wait_msg("Applied", |m| {
        matches!(m, WorkerMsg::Results { player: 0, results } if results.iter().any(|r| matches!(r.outcome, CommandOutcome::Applied)))
    });
    p.send(&ToWorker::SaveNow {
        reason: SaveReason::Admin,
    });
    let WorkerMsg::Saved(rec) = p.wait_msg(
        "Saved",
        |m| matches!(m, WorkerMsg::Saved(r) if r.meta.seq == 2),
    ) else {
        unreachable!()
    };
    assert_eq!(rec.applied_upto, vec![(0, 1)]);
    let before = last_hashes(&p.all_msgs());
    assert_eq!(before.len(), 2);
    // The worker dies without a word.
    let _ = p.child.kill();
    let _ = p.child.wait();
    drop(p);

    // A new process continues from the file, named by an absolute path.
    let file = dir.join(&rec.file);
    assert!(file.is_absolute());
    let mut q = Proc::start();
    q.send(&ToWorker::Init(init_in(
        Path::new(""),
        Start::Load {
            file: file.to_string_lossy().into_owned(),
            seats: setup(2, 1).slots,
        },
    )));
    // (An empty save_dir: loading works, saving is off.)
    wait_first_views(&mut q, 2);
    let msgs = q.all_msgs();
    let ready = msgs.iter().find_map(|m| match m {
        WorkerMsg::Ready(r) => Some(r.clone()),
        _ => None,
    });
    assert_eq!(ready.expect("Ready").turn, first.turn);
    let after = last_hashes(&msgs);
    assert_eq!(after, before, "the views of both seats are what they were");
    q.send(&ToWorker::Shutdown { save: false });
    assert_eq!(q.exit_code(), Some(0));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_restart_goes_on_numbering_above_the_files_of_its_directory_and_loads_a_relative_name() {
    let dir = save_dir("relative");
    let mut p = Proc::start();
    p.send(&ToWorker::Init(init_in(&dir, Start::New(setup(2, 1)))));
    wait_first_views(&mut p, 2);
    p.send(&ToWorker::Shutdown { save: true });
    assert_eq!(p.exit_code(), Some(0));
    drop(p);
    let mut q = Proc::start();
    q.send(&ToWorker::Init(init_in(
        &dir,
        Start::Load {
            file: "ring/00000001-t0001.c5sv".into(),
            seats: setup(2, 1).slots,
        },
    )));
    wait_first_views(&mut q, 2);
    q.send(&ToWorker::SaveNow {
        reason: SaveReason::Admin,
    });
    // The directory holds 1 and 2 already: the next file is 3.
    let WorkerMsg::Saved(rec) = q.wait_msg("Saved", |m| matches!(m, WorkerMsg::Saved(_))) else {
        unreachable!()
    };
    assert_eq!(rec.meta.seq, 3);
    q.send(&ToWorker::Shutdown { save: false });
    assert_eq!(q.exit_code(), Some(0));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_ring_keeps_the_newest_files_and_every_tenth_turn_goes_to_keep() {
    let dir = save_dir("ring");
    let mut p = Proc::start();
    let mut i = init_in(&dir, Start::New(setup(1, 1)));
    i.cfg.autosave.ring = 3;
    p.send(&ToWorker::Init(i));
    wait_first_views(&mut p, 1);
    let mut turn = 1;
    while turn < 11 {
        p.send(&ToWorker::Cmd {
            player: 0,
            cmd: Cmd {
                id: turn as u32,
                turn,
                cmd: Command::EndTurn { force: false },
            },
        });
        turn += 1;
        p.wait_msg("the next turn", move |m| {
            matches!(m, WorkerMsg::Turn(t) if t.phase == TurnPhase::Humans && t.turn == turn)
        });
    }
    // The saves of turns 1-11 are written in order; wait for the last one to be told.
    p.wait_msg(
        "Saved of turn 11",
        |m| matches!(m, WorkerMsg::Saved(r) if r.meta.turn == 11),
    );
    // Turn 10 is in keep/ for good; the ring has its newest 3 of the other 10 saves.
    assert_eq!(
        files_in(&dir),
        [
            "keep/00000010-t0010.c5sv",
            "ring/00000008-t0008.c5sv",
            "ring/00000009-t0009.c5sv",
            "ring/00000011-t0011.c5sv",
        ]
    );
    p.send(&ToWorker::Shutdown { save: false });
    assert_eq!(p.exit_code(), Some(0));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_save_that_is_not_there_or_is_damaged_is_code_4() {
    let dir = save_dir("bad");
    std::fs::create_dir_all(&dir).unwrap();
    for (name, bytes) in [
        ("missing.c5sv", None),
        ("junk.c5sv", Some(&b"C5SV nonsense"[..])),
    ] {
        if let Some(b) = bytes {
            std::fs::write(dir.join(name), b).unwrap();
        }
        let mut p = Proc::start();
        p.send(&ToWorker::Init(init_in(
            &dir,
            Start::Load {
                file: name.into(),
                seats: setup(2, 1).slots,
            },
        )));
        assert_eq!(p.exit_code(), Some(4), "{name}");
        assert!(
            p.all_msgs()
                .iter()
                .any(|m| matches!(m, WorkerMsg::Fatal { code: 4, .. })),
            "{name}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_save_of_another_game_can_be_started_from_by_its_absolute_path() {
    let src = save_dir("src-game");
    let dst = save_dir("dst-game");
    let mut p = Proc::start();
    p.send(&ToWorker::Init(init_in(&src, Start::New(setup(2, 1)))));
    wait_first_views(&mut p, 2);
    p.send(&ToWorker::Shutdown { save: true });
    assert_eq!(p.exit_code(), Some(0));
    drop(p);
    let file = src.join("ring/00000002-t0001.c5sv");
    let mut q = Proc::start();
    let mut i = init_in(
        &dst,
        Start::Load {
            file: file.to_string_lossy().into_owned(),
            seats: setup(2, 1).slots,
        },
    );
    i.game = 9;
    q.send(&ToWorker::Init(i));
    wait_first_views(&mut q, 2);
    q.send(&ToWorker::SaveNow {
        reason: SaveReason::Admin,
    });
    let WorkerMsg::Saved(rec) = q.wait_msg("Saved", |m| matches!(m, WorkerMsg::Saved(_))) else {
        unreachable!()
    };
    // The new game numbers on from the save it started from and writes into its own directory.
    assert_eq!(rec.meta.seq, 3);
    assert!(dst.join(&rec.file).exists());
    assert_eq!(saved_records(&q.all_msgs()).len(), 1);
    q.send(&ToWorker::Shutdown { save: false });
    assert_eq!(q.exit_code(), Some(0));
    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&dst);
}
