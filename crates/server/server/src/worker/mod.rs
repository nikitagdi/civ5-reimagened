//! `civ5-server worker`: the game worker process (game-host-mp 4.16, GH-D2, GH-D3, GH-D16). The same binary
//! as the gateway; everything comes in `Init`, so the subcommand takes no keys.
//!
//! Threads: this one runs the core and the `Worker`; `ipc-stdin` reads frames; `ipc-stdout` writes them;
//! `saver` packs and writes the saves (`saver.rs`) and answers into the channel of `ipc-stdin`. The exit codes
//! are those of game-host-mp 4.12.

mod core_boot;
mod io;
pub mod packer;
mod saver;
mod watch;

use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::time::{Duration, Instant};

use game_host::saves::Save;
use game_host::{Core, Out, Worker};
use protocol::{Start, ToWorker, WorkerInit};

use self::core_boot::Booted;
use self::io::{Input, StartPacer, Writer};
use self::packer::Packer;
use self::saver::Saver;
use self::watch::Watch;

/// Exit code "the game did not start".
pub const EXIT_NOT_STARTED: i32 = 5;
/// Exit code "IPC error": a frame that does not parse, no `Init`.
const EXIT_START: u8 = 5;
const EXIT_IPC: u8 = 2;
/// Exit code "the save did not load" (the gateway takes the one before it).
const EXIT_BAD_SAVE: u8 = 4;
/// Exit code "other".
const EXIT_OTHER: u8 = 1;

/// Worker entry point. Returns the process exit code.
pub fn run(_args: &[String]) -> i32 {
    // Logs go to stderr only: stdout is the frame pipe.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init();
    // One channel for the frames of the gateway and the answers of the save thread.
    let (tx, rx) = mpsc::channel();
    let out = match (
        io::spawn_reader(std::io::stdin(), tx.clone()),
        Writer::spawn(std::io::stdout()),
    ) {
        (Ok(()), Ok(out)) => out,
        (Err(e), _) | (_, Err(e)) => {
            tracing::error!("cannot start the I/O threads: {e}");
            return i32::from(EXIT_OTHER);
        }
    };
    let code = serve(&rx, tx, &out);
    // Everything queued (the `Fatal` included) is on the pipe before the process ends.
    out.finish();
    i32::from(code)
}

/// The first frame, `Init`, then the loop of the worker. Returns the exit code.
fn serve(rx: &Receiver<Input>, tx: Sender<Input>, out: &Writer) -> u8 {
    let init = match rx.recv() {
        Ok(Input::Msg(msg)) => match *msg {
            ToWorker::Init(init) => init,
            other => {
                out.fatal(format!("the first frame is not Init: {other:?}"), EXIT_IPC);
                return EXIT_IPC;
            }
        },
        Ok(Input::Bad(why)) => {
            out.fatal(why, EXIT_IPC);
            return EXIT_IPC;
        }
        Ok(Input::Eof) | Err(_) => {
            // The gateway went away before it said anything.
            tracing::warn!("stdin closed before Init");
            return EXIT_IPC;
        }
        Ok(Input::Saved(_) | Input::Replica(_)) => return EXIT_IPC,
    };
    let span = tracing::info_span!("worker", game = init.game);
    let _in = span.enter();
    let dir = Path::new(&init.save_dir).to_path_buf();
    // The save of `Start::Load` is read before anything is built: a file that does not read is code 4.
    let save = match &init.start {
        Start::Load { file, .. } => match saver::load_save(&dir, file) {
            Ok(save) => Some(save),
            Err(why) => {
                tracing::error!("{why}");
                out.fatal(why, EXIT_BAD_SAVE);
                return EXIT_BAD_SAVE;
            }
        },
        _ => None,
    };
    // No save directory: no saves (the same rule as the worker's own).
    let saver = if init.save_dir.is_empty() {
        None
    } else {
        match Saver::spawn(dir.clone(), tx.clone()) {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::error!("cannot start the save thread: {e}");
                return EXIT_OTHER;
            }
        }
    };
    // The snapshots of the replica stream are packed on a thread of their own, whether the game saves or not.
    let packer = match Packer::spawn(replica_patches(), tx) {
        Ok(p) => p,
        Err(e) => {
            tracing::error!("cannot start the pack thread: {e}");
            return EXIT_OTHER;
        }
    };
    // Building the core and the game is a few long calls without frames: heartbeats meanwhile (`Starting`,
    // GH-D16), within a budget that grows with the map.
    let pacer = out.start_pacer(PACE, start_budget(&init.start));
    let code = match core_boot::boot(&init) {
        Booted::Fixture(core) => guarded(out, || {
            run_game(
                *core,
                init,
                save,
                &dir,
                rx,
                out,
                saver.as_ref(),
                &packer,
                pacer,
            )
        }),
        Booted::Real(core) => guarded(out, || {
            run_game(
                *core,
                init,
                save,
                &dir,
                rx,
                out,
                saver.as_ref(),
                &packer,
                pacer,
            )
        }),
        Booted::Unavailable(why) => {
            pacer.stop();
            tracing::error!("{why}");
            out.fatal(why, EXIT_START);
            EXIT_START
        }
    };
    // What the save thread was given is on disk before the process ends.
    if let Some(s) = saver {
        s.finish();
    }
    packer.finish();
    code
}

/// Runs `f`; a panic becomes `Fatal` and exit code 1 (game-host-mp 4.12: "other").
fn guarded(out: &Writer, f: impl FnOnce() -> u8) -> u8 {
    match panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(code) => code,
        Err(_) => {
            out.fatal("the worker panicked", EXIT_OTHER);
            EXIT_OTHER
        }
    }
}

/// The start heartbeats come this often (the core thread's own come after 1 s without a frame).
const PACE: Duration = Duration::from_secs(1);

/// How long the start may take before the heartbeats stop and the gateway's silence rule (30 s) calls it a
/// hang. A Huge map script of 12 players that searches long for fair starts, on a busy machine, takes minutes;
/// a script that never ends must still end as a hang. A load builds no map.
fn start_budget(start: &Start) -> Duration {
    let minutes = match start {
        Start::New(setup) => match setup.settings.world_size.as_str() {
            "WORLDSIZE_DUEL" | "WORLDSIZE_TINY" => 3,
            "WORLDSIZE_SMALL" | "WORLDSIZE_STANDARD" => 5,
            "WORLDSIZE_LARGE" => 8,
            // Huge, and a size of a mod this table does not know.
            _ => 12,
        },
        Start::Load { .. } => 5,
        Start::Describe => 2,
    };
    Duration::from_secs(minutes * 60)
}

/// The loop of the core thread (game-host-mp 4.16): work while `wants_step`, else wait for a frame no longer
/// than `next_wake_ms` (the heartbeat, a save that waits).
#[allow(clippy::too_many_arguments)]
fn run_game<C: Core>(
    core: C,
    init: WorkerInit,
    save: Option<Save>,
    dir: &Path,
    rx: &Receiver<Input>,
    out: &Writer,
    saver: Option<&Saver>,
    packer: &Packer,
    pacer: StartPacer,
) -> u8 {
    let clock = Instant::now();
    let now = || u64::try_from(clock.elapsed().as_millis()).unwrap_or(u64::MAX);
    let game = u64::from(init.game);
    let record = init.cfg.record;
    let (mut worker, outs) = match save {
        Some(save) => Worker::resume(core, init, save, now()),
        None => Worker::new(core, init, now()),
    };
    // The game is built; from here the worker's own heartbeat goes (`Worker::finish`).
    pacer.stop();
    // New saves go above the files that are there: a restart from an older save, a game that starts from the
    // save of another game.
    worker.set_seq_floor(saver::newest_seq(dir));
    worker.set_replica_epoch(replica_epoch());
    if let Some(code) = emit(out, saver, packer, outs) {
        return code;
    }
    // From here every call of the core thread is watched: heartbeats while one runs long (`watch.rs`).
    let watch = Watch::spawn(game, out.queue(), PACE, watch::budget_from_env()).with_record(record);
    worker.set_trace_level(turntrace::Level::from_env());
    loop {
        let input = if worker.wants_step() {
            match rx.try_recv() {
                Ok(input) => Some(input),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => return EXIT_OTHER,
            }
        } else {
            let wait = worker.next_wake_ms().map_or(Duration::from_secs(1), |at| {
                Duration::from_millis(at.saturating_sub(now()))
            });
            match rx.recv_timeout(wait) {
                Ok(input) => Some(input),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => return EXIT_OTHER,
            }
        };
        let turn = worker.phase().0;
        // The heartbeats of a long call say what the call is: the AI after the last END_TURN, not `Humans`.
        let phase = worker.watch_phase();
        let outs = match input {
            Some(input) => {
                let _call = watch.enter(|| describe(&input, turn, phase), phase, turn);
                feed(&mut worker, input, now())
            }
            None => {
                let _call =
                    watch.enter(|| format!("a step in turn {turn}, {phase:?}"), phase, turn);
                worker.step(now())
            }
        };
        if let Some(code) = emit(out, saver, packer, outs) {
            return code;
        }
    }
}

/// The epoch of the replica stream of this process: a new one whenever a worker starts (a restart, a load), so
/// the clients begin their chain again with a new base. Never 0.
fn replica_epoch() -> u32 {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |d| d.as_millis());
    // The low 32 bits of the clock in milliseconds: different for any two starts more than a millisecond apart.
    (ms as u32).max(1)
}

/// Patches between the snapshots of two turns (variant A) unless `CIV5_REPLICA_PATCH=0` (variant B: whole
/// snapshots only).
fn replica_patches() -> bool {
    std::env::var("CIV5_REPLICA_PATCH").map_or(true, |v| v != "0")
}

/// What an input asks of the core, for the log of a long call (`watch.rs`).
fn describe(input: &Input, turn: i32, phase: protocol::TurnPhase) -> String {
    let what = match input {
        Input::Msg(msg) => match msg.as_ref() {
            ToWorker::Cmd { player, cmd } => {
                format!("order {} of seat {player}: {:?}", cmd.id, cmd.cmd)
            }
            ToWorker::Query { player, q, .. } => format!("query of seat {player}: {q:?}"),
            other => format!("{other:?}"),
        },
        Input::Eof => "stdin closed (a shutdown with a save)".to_owned(),
        Input::Saved(done) => format!("the save {} was written", done.file.rel_path()),
        Input::Replica(_) => "a replica snapshot was packed".to_owned(),
        Input::Bad(why) => format!("a bad frame: {why}"),
    };
    let what: String = what.chars().take(240).collect();
    format!("{what} (turn {turn}, {phase:?})")
}

/// One input of the reader thread or the save thread, as the worker's reply.
fn feed<C: Core>(worker: &mut Worker<C>, input: Input, now_ms: u64) -> Vec<Out> {
    match input {
        Input::Msg(msg) => worker.handle(*msg, now_ms),
        // EOF on stdin is `Shutdown { save: true }` (GH-D16).
        Input::Eof => worker.handle(ToWorker::Shutdown { save: true }, now_ms),
        // A save was written or failed: the gateway hears of it; after the last save the worker ends.
        Input::Saved(done) => worker.save_done(*done, now_ms),
        Input::Replica(done) => worker.replica_done(*done, now_ms),
        Input::Bad(why) => vec![
            Out::Frame(protocol::WorkerFrame {
                msgs: vec![protocol::WorkerMsg::Fatal {
                    message: why,
                    code: EXIT_IPC,
                }],
            }),
            Out::Exit(EXIT_IPC),
        ],
    }
}

/// Sends the saves to the save thread and the frames to stdout; `Some(code)` when the process ends. A closed
/// pipe ends it with 0: the gateway is gone. The saves go first: a worker whose gateway died still writes
/// what it was told to (the process waits for the save thread before it ends).
fn emit(out: &Writer, saver: Option<&Saver>, packer: &Packer, outs: Vec<Out>) -> Option<u8> {
    let (saves, rest): (Vec<Out>, Vec<Out>) = outs
        .into_iter()
        .partition(|o| matches!(o, Out::Save(_) | Out::Replica(_)));
    for o in saves {
        let sent = match o {
            Out::Save(job) => (saver.is_some_and(|s| s.send(*job)), "a save", "save"),
            Out::Replica(job) => (packer.send(*job), "a replica snapshot", "pack"),
            _ => continue,
        };
        if !sent.0 {
            tracing::error!("{} was dropped: there is no {} thread", sent.1, sent.2);
        }
    }
    for o in rest {
        match o {
            Out::Frame(f) => {
                if !out.send(&f) {
                    return Some(0);
                }
            }
            Out::Save(_) | Out::Replica(_) => {}
            Out::Exit(code) => return Some(code),
        }
    }
    None
}
