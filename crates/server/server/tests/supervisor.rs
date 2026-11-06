//! The worker supervisor against the mock worker (GS-D9): start, `Init`/`Catalog`/`Ready`, the
//! IPC tasks, `Describe`, process events.

mod common;

use std::time::{Duration, Instant};

use common::{mock_spec, mock_supervisor, temp_dir};
use protocol::{
    AutoChoices, CivPick, Cmd, Command, CommandOutcome, GameSettings, GameSetup, IPC_VERSION,
    SlotControl, SlotSetup, Start, TimerMode, ToWorker, TurnMode, ViewUpdate, WorkerConfig,
    WorkerInit, WorkerMsg,
};
use server::supervisor::{ExitInfo, StartError, Started, WorkerEvent, WorkerProc, start_worker};
use server::{ServerConfig, Shared};

const SILENCE: Duration = Duration::from_secs(10);

fn setup(humans: &[&str]) -> GameSetup {
    GameSetup {
        settings: GameSettings {
            name: "mock".into(),
            world_size: "WORLDSIZE_DUEL".into(),
            map_script: "m".into(),
            map_options: vec![],
            speed: "s".into(),
            start_era: "e".into(),
            handicap: "h".into(),
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
        slots: humans
            .iter()
            .enumerate()
            .map(|(i, nick)| SlotSetup {
                player: i as i8,
                civ: CivPick::Random,
                control: SlotControl::Human {
                    nick: (*nick).into(),
                },
            })
            .collect(),
    }
}

fn init(start: Start) -> WorkerInit {
    WorkerInit {
        ipc: IPC_VERSION,
        game: 1,
        core: protocol::CoreKind::Fixture {
            huge: false,
            wrap_x: false,
        },
        start,
        save_dir: String::new(),
        seed: 1,
        cfg: WorkerConfig::default(),
    }
}

fn new_game(humans: &[&str]) -> WorkerInit {
    init(Start::New(setup(humans)))
}

fn cmd(id: u32) -> Cmd {
    Cmd {
        id,
        turn: 1,
        cmd: Command::CancelEndTurn,
    }
}

async fn start(dir: &std::path::Path, mode: &str, humans: &[&str]) -> Started {
    start_worker(&mock_spec(dir, mode), new_game(humans), SILENCE)
        .await
        .unwrap_or_else(|e| panic!("start failed: {e}"))
}

/// Next event within 10 s.
async fn event(proc: &mut WorkerProc) -> WorkerEvent {
    tokio::time::timeout(Duration::from_secs(10), proc.next_event())
        .await
        .expect("no event for 10 s")
        .expect("event queue closed")
}

/// The next frame's messages (skipping heartbeats).
async fn frame(proc: &mut WorkerProc) -> Vec<WorkerMsg> {
    loop {
        match event(proc).await {
            WorkerEvent::Frame(f) => {
                let msgs: Vec<_> = f
                    .msgs
                    .into_iter()
                    .filter(|m| !matches!(m, WorkerMsg::Heartbeat { .. }))
                    .collect();
                if !msgs.is_empty() {
                    return msgs;
                }
            }
            other => panic!("expected a frame, got {other:?}"),
        }
    }
}

/// Everything up to and including `Exited`.
async fn until_exit(proc: &mut WorkerProc) -> (Vec<WorkerEvent>, ExitInfo) {
    let mut seen = Vec::new();
    loop {
        match event(proc).await {
            WorkerEvent::Exited(info) => return (seen, info),
            other => seen.push(other),
        }
    }
}

#[tokio::test]
async fn init_catalog_ready() {
    let dir = temp_dir("ready");
    let started = start(&dir, "echo", &["alice", "bob"]).await;
    assert_eq!(started.ready.build_id, "mock-core");
    assert_eq!(started.ready.seats.len(), 2);
    assert!(started.proc.pid().is_some());
    let (hash, body) = started.catalog.expect("the mock sends a catalog");
    assert_eq!(hash, protocol::xxh64(0, &body));
    // NP-D14: `C5CT`, format version 1 as `u16 LE`, then the worker's postcard body.
    assert_eq!(&body[..6], b"C5CT\x01\x00");
    assert_eq!(&body[6..], b"mock catalog");
    assert!(started.early.is_empty());

    // Then the first views: one basis, a full view per human seat, the turn.
    let mut proc = started.proc;
    let msgs = frame(&mut proc).await;
    assert!(matches!(msgs[0], WorkerMsg::Basis { ref players, .. } if players == &[0, 1]));
    let fulls = msgs
        .iter()
        .filter(|m| {
            matches!(
                m,
                WorkerMsg::View {
                    update: ViewUpdate::Full { rev: 1, .. },
                    ..
                }
            )
        })
        .count();
    assert_eq!(fulls, 2);
    assert!(matches!(msgs.last(), Some(WorkerMsg::Turn(_))));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn orders_are_answered_in_order_over_the_pipe() {
    let dir = temp_dir("orders");
    let Started { mut proc, .. } = start(&dir, "echo", &["alice"]).await;
    let _first = frame(&mut proc).await;
    for id in 1..=3 {
        proc.send(ToWorker::Cmd {
            player: 0,
            cmd: cmd(id),
        })
        .unwrap();
    }
    for id in 1..=3 {
        let msgs = frame(&mut proc).await;
        match &msgs[..] {
            [
                WorkerMsg::View {
                    update: ViewUpdate::Delta { base_rev, rev, .. },
                    ..
                },
                WorkerMsg::Results { results, .. },
            ] => {
                assert_eq!((*base_rev, *rev), (id, id + 1));
                assert_eq!(results[0].id, id);
                assert_eq!(results[0].outcome, CommandOutcome::Applied);
            }
            other => panic!("unexpected answer {other:?}"),
        }
    }
    // A query and a full-view request on the same pipe.
    proc.send(ToWorker::WantFull {
        player: 0,
        basis: true,
    })
    .unwrap();
    let msgs = frame(&mut proc).await;
    assert!(matches!(msgs[0], WorkerMsg::Basis { .. }));
    assert!(matches!(
        msgs[1],
        WorkerMsg::View {
            update: ViewUpdate::Full { rev: 5, .. },
            ..
        }
    ));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn shutdown_saves_and_exits_zero() {
    let dir = temp_dir("shutdown");
    let Started { mut proc, .. } = start(&dir, "echo", &["alice"]).await;
    let _first = frame(&mut proc).await;
    proc.send(ToWorker::Cmd {
        player: 0,
        cmd: cmd(9),
    })
    .unwrap();
    let _answer = frame(&mut proc).await;
    proc.send(ToWorker::Shutdown { save: true }).unwrap();
    let (events, exit) = until_exit(&mut proc).await;
    assert_eq!(
        exit,
        ExitInfo {
            code: Some(0),
            killed: false
        }
    );
    // The save the worker wrote came before `Exited`, and names the last order.
    let saved = events.iter().find_map(|e| match e {
        WorkerEvent::Frame(f) => f.msgs.iter().find_map(|m| match m {
            WorkerMsg::Saved(rec) => Some(rec.clone()),
            _ => None,
        }),
        _ => None,
    });
    assert_eq!(saved.expect("Saved").applied_upto, vec![(0, 9)]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn save_now_is_answered_with_a_save() {
    let dir = temp_dir("savenow");
    let Started { mut proc, .. } = start(&dir, "echo", &["alice"]).await;
    let _first = frame(&mut proc).await;
    // A cloned sender queues on the same pipe.
    proc.sender()
        .send(ToWorker::SaveNow {
            reason: protocol::SaveReason::Admin,
        })
        .unwrap();
    let msgs = frame(&mut proc).await;
    assert!(matches!(msgs[0], WorkerMsg::Saved(_)));
    proc.kill();
    let (_, exit) = until_exit(&mut proc).await;
    assert!(exit.killed);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn kill_ends_the_process_and_exited_is_last() {
    let dir = temp_dir("kill");
    let Started { mut proc, .. } = start(&dir, "hang", &["alice"]).await;
    proc.kill();
    let (_, exit) = until_exit(&mut proc).await;
    assert!(exit.killed);
    assert_ne!(exit.code, Some(0));
    // Nothing follows `Exited`.
    assert!(proc.next_event().await.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_crash_is_an_exit_code_after_the_frames() {
    let dir = temp_dir("crash");
    let Started { mut proc, .. } = start(&dir, "crash-after n=2 code=7", &["alice"]).await;
    let _first = frame(&mut proc).await;
    proc.send(ToWorker::Cmd {
        player: 0,
        cmd: cmd(1),
    })
    .unwrap();
    let _answer = frame(&mut proc).await;
    proc.send(ToWorker::Cmd {
        player: 0,
        cmd: cmd(2),
    })
    .unwrap();
    let (events, exit) = until_exit(&mut proc).await;
    assert_eq!(
        exit,
        ExitInfo {
            code: Some(7),
            killed: false
        }
    );
    assert!(
        events.iter().all(|e| matches!(e, WorkerEvent::Eof)),
        "{events:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_garbage_frame_is_reported_once_and_the_stream_stops() {
    let dir = temp_dir("garbage");
    let Started { mut proc, .. } = start(&dir, "garbage n=1", &["alice"]).await;
    let _first = frame(&mut proc).await;
    proc.send(ToWorker::Cmd {
        player: 0,
        cmd: cmd(1),
    })
    .unwrap();
    assert!(matches!(event(&mut proc).await, WorkerEvent::Garbage(_)));
    // The worker is alive (it hangs); the supervisor has to kill it.
    proc.kill();
    let (events, exit) = until_exit(&mut proc).await;
    assert!(events.is_empty(), "no frames after garbage: {events:?}");
    assert!(exit.killed);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_slow_worker_answers_late_but_in_order() {
    let dir = temp_dir("slow");
    let Started { mut proc, .. } = start(&dir, "slow ms=300", &["alice"]).await;
    let _first = frame(&mut proc).await;
    let t = Instant::now();
    proc.send(ToWorker::Cmd {
        player: 0,
        cmd: cmd(1),
    })
    .unwrap();
    let _answer = frame(&mut proc).await;
    assert!(
        t.elapsed() >= Duration::from_millis(300),
        "{:?}",
        t.elapsed()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn heartbeats_arrive_as_frames() {
    let dir = temp_dir("heartbeat");
    let Started { mut proc, .. } = start(&dir, "echo heartbeat=30", &["alice"]).await;
    let mut beats = 0;
    while beats < 3 {
        if let WorkerEvent::Frame(f) = event(&mut proc).await {
            beats += f
                .msgs
                .iter()
                .filter(|m| matches!(m, WorkerMsg::Heartbeat { .. }))
                .count();
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn silence_before_ready_kills_the_worker() {
    let dir = temp_dir("silent");
    let t = Instant::now();
    let res = start_worker(
        &mock_spec(&dir, "hang at=start"),
        new_game(&["alice"]),
        Duration::from_millis(300),
    )
    .await;
    assert!(matches!(res.err(), Some(StartError::Silent(_))));
    assert!(t.elapsed() < Duration::from_secs(5));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn failures_before_ready_are_told_apart() {
    let dir = temp_dir("fail");
    let err = |res: Result<Started, StartError>| res.err().expect("the start must fail");
    let e = err(start_worker(&mock_spec(&dir, "exit code=3"), new_game(&["a"]), SILENCE).await);
    assert!(
        matches!(
            e,
            StartError::Exited(ExitInfo {
                code: Some(3),
                killed: false
            })
        ),
        "{e:?}"
    );
    let e = err(start_worker(&mock_spec(&dir, "fatal code=4"), new_game(&["a"]), SILENCE).await);
    assert!(matches!(e, StartError::Fatal { code: 4, .. }), "{e:?}");
    // A gateway of another IPC version: the worker says so and exits 2.
    let mut foreign = new_game(&["a"]);
    foreign.ipc += 1;
    let e = err(start_worker(&mock_spec(&dir, "echo"), foreign, SILENCE).await);
    assert!(matches!(e, StartError::Fatal { code: 2, .. }), "{e:?}");
    // A program that does not exist.
    let mut spec = mock_spec(&dir, "echo");
    spec.exe = dir.join("no-such-worker");
    let e = err(start_worker(&spec, new_game(&["a"]), SILENCE).await);
    assert!(matches!(e, StartError::Spawn(_)), "{e:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn stderr_goes_to_a_rotated_log() {
    let dir = temp_dir("stderr");
    for run in 0..7 {
        let Started { mut proc, .. } = start(&dir, &format!("echo run={run}"), &["a"]).await;
        proc.kill();
        let _ = until_exit(&mut proc).await;
    }
    let log = dir.join("logs").join("worker-test.log");
    let text = std::fs::read_to_string(&log).unwrap();
    assert!(text.contains("mock-worker started in mode echo"), "{text}");
    for n in 1..=4 {
        assert!(
            dir.join("logs")
                .join(format!("worker-test.log.{n}"))
                .exists(),
            "log .{n} missing"
        );
    }
    assert!(!dir.join("logs").join("worker-test.log.5").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn game_workers_get_their_own_directory_and_log() {
    let dir = temp_dir("game");
    let sup = mock_supervisor(&dir, "echo", SILENCE);
    let started = sup
        .start_game(5, Start::New(setup(&["alice"])), 42)
        .await
        .unwrap();
    assert!(dir.join("games").join("5").is_dir());
    assert!(dir.join("logs").join("worker-5.log").is_file());
    started.proc.kill();
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn describe_opens_the_lobby() {
    let dir = temp_dir("describe");
    let sup = mock_supervisor(&dir, "echo", SILENCE);
    let shared = Shared::new(ServerConfig {
        state_dir: Some(dir.clone()),
        ..ServerConfig::default()
    })
    .unwrap();
    assert!(shared.health.core_build().is_none());

    let described = sup.publish_describe(&shared).await.unwrap();
    assert_eq!(described.build_id.as_deref(), Some("mock-core"));
    // The pick lists are the ones every login gets; healthz now knows the core.
    assert_eq!(shared.choices().hash, 7);
    assert_eq!(shared.health.core_build().as_deref(), Some("mock-core"));
    // The catalog is on disk under its hash.
    let (hash, body) = described.catalog.unwrap();
    let file = dir.join("catalog").join(format!("{hash:016x}.bin"));
    assert_eq!(std::fs::read(file).unwrap(), body);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_failed_describe_leaves_the_lobby_closed() {
    let dir = temp_dir("describe-fail");
    let shared = Shared::new(ServerConfig::default()).unwrap();
    for mode in ["exit code=5", "fatal code=5", "echo no-choices=1"] {
        let sup = mock_supervisor(&dir, mode, SILENCE);
        assert!(sup.publish_describe(&shared).await.is_err(), "{mode}");
        assert!(shared.health.core_build().is_none(), "{mode}");
    }
    let sup = mock_supervisor(&dir, "hang at=start", Duration::from_millis(300));
    assert!(sup.publish_describe(&shared).await.is_err());
    assert!(shared.health.core_build().is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- N2.5: hang, crash, restart ---------------------------------------------------------

use std::sync::Arc;

use protocol::{PlayerId, SaveKind, SaveMeta, SaveRecord};
use server::clock::{Clock, TestClock};
use server::supervisor::policy::{Failure, PolicyConfig, SaveBook};
use server::supervisor::{GameEvent, GameSpec, GameState, GameSupervisor, StartedInfo};
use tokio::sync::mpsc::Receiver;

fn save_entry(book: &mut SaveBook, seq: u64, turn: i32) {
    book.record(&SaveRecord {
        meta: SaveMeta {
            seq,
            build_id: "mock-core".into(),
            turn,
            civ_name: "Mockland".into(),
            saved_at_ms: 0,
        },
        file: format!("ring/{seq:08}-t{turn:04}.c5sv"),
        kind: SaveKind::Ring,
        applied_upto: vec![],
    });
}

/// Saves 1..=n (turn = 10 * seq).
fn saves(n: u64) -> SaveBook {
    let mut book = SaveBook::new();
    for seq in 1..=n {
        save_entry(&mut book, seq, 10 * seq as i32);
    }
    book
}

fn quick_policy() -> PolicyConfig {
    PolicyConfig {
        delays: [
            Duration::ZERO,
            Duration::from_millis(50),
            Duration::from_millis(100),
        ],
        ..PolicyConfig::default()
    }
}

struct Game {
    sup: GameSupervisor,
    rx: Receiver<GameEvent>,
    clock: Arc<TestClock>,
}

fn spec(start: Start, saves: SaveBook, clock: &Arc<TestClock>) -> GameSpec {
    GameSpec {
        game: 1,
        start,
        seed: 1,
        saves,
        seats: setup(&["alice"]).slots,
        policy: quick_policy(),
        shutdown_grace: Duration::from_millis(500),
        clock: clock.clone() as Arc<dyn Clock>,
    }
}

fn supervise(
    dir: &std::path::Path,
    mode: &str,
    hang_ms: u64,
    start: Start,
    saves: SaveBook,
) -> Game {
    let clock = Arc::new(TestClock::new());
    let sup = mock_supervisor(dir, mode, Duration::from_millis(hang_ms));
    let (sup, rx) = sup.supervise(spec(start, saves, &clock));
    Game { sup, rx, clock }
}

fn marker(dir: &std::path::Path) -> String {
    dir.join("marker").to_string_lossy().into_owned()
}

fn load(seq: u64) -> Start {
    Start::Load {
        file: format!("ring/{seq:08}-t{:04}.c5sv", 10 * seq),
        seats: setup(&["alice"]).slots,
    }
}

impl Game {
    async fn event(&mut self) -> GameEvent {
        tokio::time::timeout(Duration::from_secs(10), self.rx.recv())
            .await
            .expect("no game event for 10 s")
            .expect("supervision ended")
    }

    /// Skips frames; the next event that is not one.
    async fn next(&mut self) -> GameEvent {
        loop {
            match self.event().await {
                GameEvent::Frame(_) => {}
                other => return other,
            }
        }
    }

    async fn started(&mut self) -> StartedInfo {
        match self.next().await {
            GameEvent::Started(info) => info,
            other => panic!("expected Started, got {other:?}"),
        }
    }

    /// The next frame's messages.
    async fn frame(&mut self) -> Vec<WorkerMsg> {
        loop {
            match self.event().await {
                GameEvent::Frame(f)
                    if f.msgs
                        .iter()
                        .any(|m| !matches!(m, WorkerMsg::Heartbeat { .. })) =>
                {
                    return f.msgs;
                }
                GameEvent::Frame(_) => {}
                other => panic!("expected a frame, got {other:?}"),
            }
        }
    }

    fn order(&self, player: PlayerId, id: u32) {
        self.sup
            .send(ToWorker::Cmd {
                player,
                cmd: cmd(id),
            })
            .unwrap_or_else(|s| panic!("cannot send in state {s:?}"));
    }
}

#[tokio::test]
async fn a_crash_restarts_from_the_newest_save_and_says_what_was_lost() {
    let dir = temp_dir("g-crash");
    let mode = format!("crash-after n=3 once={}", marker(&dir));
    let mut g = supervise(
        &dir,
        &mode,
        10_000,
        Start::New(setup(&["alice"])),
        SaveBook::new(),
    );
    let first = g.started().await;
    assert!(first.restart.is_none());
    g.frame().await; // the first views
    // A save, an order that is in no save, and an order the worker dies on.
    g.sup
        .send(ToWorker::SaveNow {
            reason: protocol::SaveReason::Admin,
        })
        .unwrap();
    assert!(
        g.frame()
            .await
            .iter()
            .any(|m| matches!(m, WorkerMsg::Saved(_)))
    );
    g.order(0, 1);
    assert!(
        g.frame()
            .await
            .iter()
            .any(|m| matches!(m, WorkerMsg::Results { .. }))
    );
    g.order(0, 2);

    match g.next().await {
        GameEvent::Restarting(info) => {
            assert_eq!(info.failure, Some(Failure::Crash));
            assert_eq!(info.exit.and_then(|e| e.code), Some(1));
            assert_eq!(info.lost, vec![(0, vec![2])]);
            assert_eq!(info.rolled_back, vec![(0, 1)]);
        }
        other => panic!("expected Restarting, got {other:?}"),
    }
    let again = g.started().await;
    let restart = again.restart.expect("a restart");
    assert_eq!((restart.from_save.seq, restart.skipped), (1, 0));
    assert_eq!(restart.rolled_back, vec![(0, 1)]);
    // Not silent: the seat that lost orders gets a journal line with the count.
    assert_eq!(restart.journal.len(), 1);
    let (player, line) = &restart.journal[0];
    assert_eq!(*player, 0);
    assert_eq!(line, "Потеряно ваших приказов при восстановлении: 1");
    // The new worker plays on.
    assert_eq!(g.sup.state(), GameState::Running);
    g.frame().await; // its first views
    g.order(0, 3);
    assert!(
        g.frame()
            .await
            .iter()
            .any(|m| matches!(m, WorkerMsg::Results { .. }))
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_silent_worker_is_killed_and_replaced() {
    let dir = temp_dir("g-hang");
    let mode = format!("hang once={}", marker(&dir));
    let t = Instant::now();
    let mut g = supervise(&dir, &mode, 300, load(1), saves(1));
    g.started().await;
    match g.next().await {
        GameEvent::Restarting(info) => {
            assert_eq!(info.failure, Some(Failure::Hang));
            assert!(info.exit.is_some_and(|e| e.killed), "{:?}", info.exit);
        }
        other => panic!("expected Restarting, got {other:?}"),
    }
    assert!(t.elapsed() >= Duration::from_millis(300));
    let again = g.started().await;
    assert_eq!(
        again.ready.turn, 10,
        "the mock reads the turn from the save name"
    );
    // Nothing was lost, so there is no line (the worker says where the game continues from).
    assert!(again.restart.unwrap().journal.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_worker_that_keeps_talking_is_not_hung() {
    let dir = temp_dir("g-beat");
    // Heartbeats every 50 ms keep a 300 ms hang timer from firing.
    let mut g = supervise(&dir, "echo heartbeat=50", 300, load(1), saves(1));
    g.started().await;
    tokio::time::sleep(Duration::from_millis(1000)).await;
    assert_eq!(g.sup.state(), GameState::Running);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn three_failures_fault_the_game_until_an_admin_restarts_it() {
    let dir = temp_dir("g-fault");
    let mode = format!("crash-after n=0 once={} times=3", marker(&dir));
    let mut g = supervise(&dir, &mode, 10_000, load(3), saves(3));
    g.started().await;
    // The second worker comes from the same save, the third one step back (a quick second failure).
    let r1 = match g.next().await {
        GameEvent::Restarting(i) => i,
        other => panic!("{other:?}"),
    };
    assert_eq!(r1.delay, Duration::ZERO);
    let s2 = g.started().await.restart.unwrap();
    assert_eq!((s2.from_save.seq, s2.skipped), (3, 0));
    let r2 = match g.next().await {
        GameEvent::Restarting(i) => i,
        other => panic!("{other:?}"),
    };
    assert_eq!(r2.delay, Duration::from_millis(50));
    let s3 = g.started().await.restart.unwrap();
    assert_eq!((s3.from_save.seq, s3.skipped), (2, 1));
    // The third failure in ten minutes: no more restarts.
    match g.next().await {
        GameEvent::Faulted(info) => {
            assert!(info.why.contains("3 failures"), "{}", info.why);
            assert_eq!(info.failure, Some(Failure::Crash));
        }
        other => panic!("expected Faulted, got {other:?}"),
    }
    assert_eq!(g.sup.state(), GameState::Faulted);
    // Orders are refused with the state, not queued.
    assert_eq!(
        g.sup.send(ToWorker::Cmd {
            player: 0,
            cmd: cmd(1)
        }),
        Err(GameState::Faulted)
    );
    // The fourth process behaves: an administrator brings the game back from the newest save.
    g.sup.restart();
    let back = g.started().await.restart.unwrap();
    assert_eq!((back.from_save.seq, back.skipped), (3, 0));
    assert_eq!(g.sup.state(), GameState::Running);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn failures_ten_minutes_apart_never_fault_the_game() {
    let dir = temp_dir("g-spread");
    let mode = format!("crash-after n=0 once={} times=4", marker(&dir));
    let mut g = supervise(&dir, &mode, 10_000, load(2), saves(2));
    g.started().await;
    for _ in 0..4 {
        // Each failure is eleven minutes after the one before.
        g.clock.advance(11 * 60 * 1000);
        match g.next().await {
            GameEvent::Restarting(i) => assert_eq!(i.delay, Duration::ZERO),
            other => panic!("{other:?}"),
        }
        let s = g.started().await.restart.unwrap();
        // A restart after a long quiet run takes the newest save again.
        assert_eq!((s.from_save.seq, s.skipped), (2, 0));
    }
    assert_eq!(g.sup.state(), GameState::Running);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn exit_4_loads_the_save_before() {
    let dir = temp_dir("g-code4");
    // The newest save does not load; the one before does.
    let mut g = supervise(&dir, "echo bad=00000003", 10_000, load(3), saves(3));
    match g.next().await {
        GameEvent::Restarting(i) => assert_eq!(i.failure, Some(Failure::LoadFailed)),
        other => panic!("expected Restarting, got {other:?}"),
    }
    let r = g.started().await.restart.unwrap();
    assert_eq!((r.from_save.seq, r.skipped), (2, 1));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn exit_5_refuses_a_new_game_and_faults_a_loaded_one() {
    let dir = temp_dir("g-code5");
    let mut g = supervise(
        &dir,
        "exit code=5",
        10_000,
        Start::New(setup(&["alice"])),
        SaveBook::new(),
    );
    assert!(matches!(g.next().await, GameEvent::StartRefused { .. }));
    assert_eq!(g.sup.state(), GameState::Finished);

    let mut g = supervise(&dir, "exit code=5", 10_000, load(1), saves(1));
    match g.next().await {
        GameEvent::Faulted(info) => assert_eq!(info.failure, Some(Failure::NotStarted)),
        other => panic!("expected Faulted, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn exit_2_gets_one_more_try_with_the_same_save() {
    let dir = temp_dir("g-code2");
    // One IPC error: the same save is tried again and works.
    let mode = format!("exit code=2 once={}", marker(&dir));
    let mut g = supervise(&dir, &mode, 10_000, load(2), saves(2));
    match g.next().await {
        GameEvent::Restarting(i) => assert_eq!(i.failure, Some(Failure::IpcError)),
        other => panic!("{other:?}"),
    }
    let r = g.started().await.restart.unwrap();
    assert_eq!((r.from_save.seq, r.skipped), (2, 0));

    // Two in a row: the game is faulted.
    let dir2 = temp_dir("g-code2b");
    let mode = format!("exit code=2 once={} times=2", marker(&dir2));
    let mut g = supervise(&dir2, &mode, 10_000, load(2), saves(2));
    assert!(matches!(g.next().await, GameEvent::Restarting(_)));
    match g.next().await {
        GameEvent::Faulted(info) => assert!(info.why.contains("exit 2"), "{}", info.why),
        other => panic!("expected Faulted, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dir2);
}

#[tokio::test]
async fn exits_3_and_0_and_garbage_all_restart() {
    for (what, mode, failure) in [
        ("exit 3", "crash-after n=1 code=3", Failure::Crash),
        // The worker ends cleanly, but nobody asked it to.
        ("exit 0", "crash-after n=1 code=0", Failure::Crash),
        ("garbage", "garbage n=1", Failure::Garbage),
    ] {
        // One directory per case: a worker of the last case may still hold files in its own.
        let dir = temp_dir(&format!("g-codes-{}", what.replace(' ', "-")));
        let mode = format!("{mode} once={}", marker(&dir));
        let mut g = supervise(&dir, &mode, 10_000, load(1), saves(1));
        g.started().await;
        g.order(0, 1);
        match g.next().await {
            GameEvent::Restarting(i) => assert_eq!(i.failure, Some(failure), "{what}"),
            other => panic!("{what}: expected Restarting, got {other:?}"),
        }
        g.started().await.restart.expect(what);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[tokio::test]
async fn no_save_means_no_restart() {
    let dir = temp_dir("g-nosave");
    let mode = format!("crash-after n=0 once={}", marker(&dir));
    let mut g = supervise(
        &dir,
        &mode,
        10_000,
        Start::New(setup(&["alice"])),
        SaveBook::new(),
    );
    g.started().await;
    match g.next().await {
        GameEvent::Faulted(info) => assert!(info.why.contains("no save"), "{}", info.why),
        other => panic!("expected Faulted, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_new_game_that_never_came_up_is_refused_not_faulted() {
    // The first worker of a new game hangs while it builds the game: there is no save to go back to, so
    // the start is refused (the room gathers again) instead of a faulted game.
    let dir = temp_dir("g-never-up");
    let mut g = supervise(
        &dir,
        "hang at=start",
        300,
        Start::New(setup(&["alice"])),
        SaveBook::new(),
    );
    match g.next().await {
        GameEvent::StartRefused { why } => assert!(why.contains("Hang"), "{why}"),
        other => panic!("expected StartRefused, got {other:?}"),
    }
    assert_eq!(g.sup.state(), GameState::Finished);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn orders_during_a_restart_are_refused() {
    let dir = temp_dir("g-refuse");
    let mode = format!("crash-after n=1 once={}", marker(&dir));
    let clock = Arc::new(TestClock::new());
    let sup = mock_supervisor(&dir, &mode, Duration::from_secs(10));
    let mut policy = quick_policy();
    policy.delays = [Duration::from_millis(600); 3];
    let (handle, rx) = sup.supervise(GameSpec {
        policy,
        ..spec(load(1), saves(1), &clock)
    });
    let mut g = Game {
        sup: handle,
        rx,
        clock,
    };
    g.started().await;
    g.order(0, 1);
    match g.next().await {
        GameEvent::Restarting(i) => assert_eq!(i.lost, vec![(0, vec![1])]),
        other => panic!("{other:?}"),
    }
    // The next worker is 600 ms away: the order is refused with the state...
    assert_eq!(g.sup.state(), GameState::Restarting);
    assert_eq!(
        g.sup.send(ToWorker::Cmd {
            player: 0,
            cmd: cmd(2)
        }),
        Err(GameState::Restarting)
    );
    // ...and the game comes back.
    g.started().await;
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn shutdown_saves_and_finishes() {
    let dir = temp_dir("g-stop");
    let mut g = supervise(&dir, "echo", 10_000, load(1), saves(1));
    g.started().await;
    g.frame().await; // the first views
    g.order(0, 1);
    g.frame().await;
    g.sup.shutdown(true);
    assert!(
        g.frame()
            .await
            .iter()
            .any(|m| matches!(m, WorkerMsg::Saved(_)))
    );
    match g.next().await {
        GameEvent::Finished { exit } => assert_eq!(exit.and_then(|e| e.code), Some(0)),
        other => panic!("expected Finished, got {other:?}"),
    }
    assert_eq!(g.sup.state(), GameState::Finished);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_worker_that_ignores_shutdown_is_killed_after_the_grace_time() {
    let dir = temp_dir("g-grace");
    let mut g = supervise(&dir, "hang", 10_000, load(1), saves(1));
    g.started().await;
    let t = Instant::now();
    g.sup.shutdown(true);
    match g.next().await {
        GameEvent::Finished { exit } => assert!(exit.is_some_and(|e| e.killed)),
        other => panic!("expected Finished, got {other:?}"),
    }
    assert!(t.elapsed() >= Duration::from_millis(500));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn dropping_the_handle_shuts_the_game_down() {
    let dir = temp_dir("g-drop");
    let Game { sup, mut rx, .. } = supervise(&dir, "echo", 10_000, load(1), saves(1));
    assert!(matches!(rx.recv().await, Some(GameEvent::Started(_))));
    drop(sup);
    let mut saved = false;
    loop {
        match tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .unwrap()
        {
            Some(GameEvent::Frame(f)) => {
                saved |= f.msgs.iter().any(|m| matches!(m, WorkerMsg::Saved(_)));
            }
            Some(GameEvent::Finished { .. }) => break,
            Some(_) => {}
            None => panic!("no Finished"),
        }
    }
    assert!(saved);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_new_game_start_through_the_supervisor_uses_the_setup() {
    // `supervise` of a `New` game: Ready names the seats of the setup.
    let dir = temp_dir("g-new");
    let mut g = supervise(
        &dir,
        "echo",
        10_000,
        Start::New(setup(&["alice", "bob"])),
        SaveBook::new(),
    );
    let s = g.started().await;
    assert_eq!(s.ready.seats.len(), 2);
    assert!(s.restart.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}
