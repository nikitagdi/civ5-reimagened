//! The recording of a live game on the real core (evidence of the packet): the gateway (in this
//! process) runs a real worker on a live save, two or more bots take the human seats and play turns, and the
//! recording the game actor writes is left in `CIV5_REC_STATE` for `civ5-replay` (P3). Linux/WSL only; the test
//! returns at once without the real core or `CIV5_LIVE_SAVE`.
//!
//! `CIV5_LIVE_SAVE=<a .c5sv of the server>` (game 936: `ring/00000235-t0126.c5sv`), `CIV5_REC_STATE=<state dir>`
//! (default: a temporary one that is removed), `CIV5_REC_TURNS` (default 3). The numbers of the recording (bytes per
//! turn, the writer's time) go to stderr.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use protocol::{
    Cmd, Command, GameSetup, LobbyChoices, PlayerId, Query, QueryAnswer, ServerMsg, SlotControl,
    TurnPhase,
};
use server::game::{
    ActorMsg, AdminCtl, AttachRequest, ClientGameMsg, GameInit, GameLoad, SessionLink,
};
use server::session::outbox;
use server::supervisor::{GameState, SupervisorConfig};
use server::{ServerConfig, Shared};
use wire::Limits;

const BIN: &str = env!("CARGO_BIN_EXE_civ5-server");

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

/// CPU time of the threads named `name` of this process, milliseconds (Linux `/proc`).
fn thread_cpu_ms(name: &str) -> f64 {
    let mut total = 0.0;
    for t in std::fs::read_dir("/proc/self/task")
        .into_iter()
        .flatten()
        .flatten()
    {
        let comm = std::fs::read_to_string(t.path().join("comm")).unwrap_or_default();
        if comm.trim() != name {
            continue;
        }
        let stat = std::fs::read_to_string(t.path().join("stat")).unwrap_or_default();
        let rest = stat.rsplit_once(')').map_or("", |x| x.1);
        let f: Vec<&str> = rest.split_whitespace().collect();
        let v = |i: usize| f.get(i).and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0);
        total += (v(11) + v(12)) * 10.0;
    }
    total
}

struct Bot {
    player: PlayerId,
    session: u64,
    rx: Arc<Mutex<Vec<ServerMsg>>>,
    next_id: u32,
}

#[test]
fn a_live_save_is_recorded_for_a_few_turns() {
    let Some(file) = std::env::var_os("CIV5_LIVE_SAVE") else {
        eprintln!("skipped: CIV5_LIVE_SAVE is not set");
        return;
    };
    if !have_real_core() {
        return;
    }
    let turns: i32 = std::env::var("CIV5_REC_TURNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let file = PathBuf::from(file).canonicalize().expect("CIV5_LIVE_SAVE");
    let bytes = std::fs::read(&file).unwrap();
    let info = game_host::saves::read_header(&bytes).expect("a C5SV save");
    drop(bytes);
    let (state, keep) = match std::env::var_os("CIV5_REC_STATE") {
        Some(d) => (PathBuf::from(d), true),
        None => (
            std::env::temp_dir().join(format!("civ5-real-rec-{}", std::process::id())),
            false,
        ),
    };
    let _ = std::fs::remove_dir_all(&state);
    std::fs::create_dir_all(&state).unwrap();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let _guard = rt.enter();
    let worker = protocol::WorkerConfig {
        stats: true,
        record: true,
        ..protocol::WorkerConfig::default()
    };
    let sup = SupervisorConfig {
        exe: BIN.into(),
        args: vec!["worker".into()],
        state_dir: state.clone(),
        core: protocol::CoreKind::Real {
            gamedata_dir: gamedata_dir().display().to_string(),
        },
        worker,
        hang: Duration::from_secs(300),
        stderr_max_bytes: 0,
    };
    let config = ServerConfig {
        state_dir: Some(state.clone()),
        supervisor: Some(sup),
        ..ServerConfig::default()
    };
    let shared = Shared::new(config).unwrap();
    let id = shared.games.next_id();
    let humans: Vec<(PlayerId, String)> = info
        .seats
        .iter()
        .filter_map(|s| match &s.control {
            SlotControl::Human { nick } => Some((s.player, nick.clone())),
            _ => None,
        })
        .collect();
    assert!(!humans.is_empty(), "the save has no human seat");
    let seed = info.settings.seed.map_or(1, u64::from);
    let handle = shared
        .start_game(
            GameInit {
                id,
                setup: GameSetup {
                    // The turn timer ends the turns the bots cannot end (a league vote blocks END_TURN even when
                    // forced): the gateway sends `ForceEndTurn`, which the recording keeps as `force_end_turn`.
                    settings: protocol::GameSettings {
                        timer: protocol::TimerMode::Fixed { seconds: 12 },
                        ..info.settings.clone()
                    },
                    slots: info.seats.clone(),
                },
                host: humans[0].1.clone(),
                room: None,
                seed,
                load: Some(GameLoad {
                    file: file.display().to_string(),
                    turn: info.header.turn,
                    book: None,
                }),
                kicked: Default::default(),
                created_ms: None,
                mods_lock: None,
            },
            Arc::new(LobbyChoices::default()),
        )
        .expect("the game starts");

    let wait = |what: &str, mut f: Box<dyn FnMut() -> bool>| {
        let t = Instant::now();
        while !f() {
            assert!(t.elapsed() < Duration::from_secs(900), "timeout: {what}");
            assert_ne!(
                handle.info.snapshot().state,
                GameState::Faulted,
                "the game faulted while waiting for {what}"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    };
    let h2 = handle.clone();
    wait(
        "the humans of the loaded save",
        Box::new(move || {
            let s = h2.info.snapshot();
            s.state == GameState::Running && s.phase == Some(TurnPhase::Humans)
        }),
    );
    let first_turn = handle.info.snapshot().turn;
    eprintln!(
        "REC: the save is up at turn {first_turn}, {} human seats",
        humans.len()
    );

    // The bots take the seats.
    let mut bots = Vec::new();
    for (i, (player, nick)) in humans.iter().enumerate() {
        let (out, mut rx) = outbox::channel(100_000, 256 << 20);
        let inbox = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&inbox);
        rt.spawn(async move {
            while let Some(frame) = rx.recv().await {
                if let Ok(m) = wire::decode::<ServerMsg>(&frame, &Limits::CLIENT) {
                    // Only what the bot reads: query answers (the rest would grow without end).
                    if matches!(m, ServerMsg::QueryResult { .. }) {
                        sink.lock().unwrap().push(m);
                    }
                }
            }
        });
        let session = 100 + i as u64;
        let link: SessionLink = Arc::new(Mutex::new(None));
        assert!(handle.send(ActorMsg::Attach(AttachRequest {
            player: *player,
            nick: nick.clone(),
            session,
            out,
            link,
            resume: None,
            claim: None,
        })));
        bots.push(Bot {
            player: *player,
            session,
            rx: inbox,
            next_id: 1,
        });
    }
    std::thread::sleep(Duration::from_secs(3));

    let started = Instant::now();
    let cpu0 = thread_cpu_ms("game-rec");
    for _ in 0..turns {
        let turn = handle.info.snapshot().turn;
        for bot in &mut bots {
            // Skip a few units: orders that change the game and are journaled with the unit's number.
            let qid = 1000 + bot.next_id;
            assert!(handle.send(ActorMsg::Client {
                player: bot.player,
                session: bot.session,
                msg: ClientGameMsg::Query {
                    id: qid,
                    q: Query::Units,
                },
                bytes: 8,
            }));
            let mut units = Vec::new();
            let t = Instant::now();
            while t.elapsed() < Duration::from_secs(30) && units.is_empty() {
                for m in bot.rx.lock().unwrap().drain(..) {
                    if let ServerMsg::QueryResult {
                        id,
                        result: QueryAnswer::Units(u),
                    } = m
                        && id == qid
                    {
                        units = u;
                    }
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            let mine: Vec<i32> = units
                .iter()
                .filter(|u| u.owner == bot.player && u.moves_left > 0)
                .map(|u| u.id)
                .take(3)
                .collect();
            for unit in mine {
                send_cmd(&handle, bot, turn, Command::UnitSkip { unit });
            }
            std::thread::sleep(Duration::from_millis(300));
            send_cmd(&handle, bot, turn, Command::EndTurn { force: true });
        }
        let h3 = handle.clone();
        wait(
            "the next turn",
            Box::new(move || {
                let s = h3.info.snapshot();
                s.turn > turn && s.phase == Some(TurnPhase::Humans)
            }),
        );
        eprintln!(
            "REC: turn {} up after {:?}",
            handle.info.snapshot().turn,
            started.elapsed()
        );
        std::thread::sleep(Duration::from_secs(3));
    }
    let play_time = started.elapsed();
    let stats = handle.info.rec.stats().expect("the recorder is on").clone();
    let rec_name = handle.info.rec.dir_name().unwrap().to_owned();
    // Let the writer thread flush what is queued before the numbers are read.
    std::thread::sleep(Duration::from_secs(3));
    let cpu = thread_cpu_ms("game-rec") - cpu0;
    use std::sync::atomic::Ordering::Relaxed;
    eprintln!(
        "REC: {turns} turns in {play_time:?}; recording {rec_name}: {} records, {} KiB of records raw, {} KiB of segments, \
         {} KiB of keyframes, {} flushes; writer busy {} ms, thread cpu {cpu:.0} ms",
        stats.records.load(Relaxed),
        stats.raw_bytes.load(Relaxed) / 1024,
        stats.seg_bytes.load(Relaxed) / 1024,
        stats.kf_bytes.load(Relaxed) / 1024,
        stats.flushes.load(Relaxed),
        stats.busy_us.load(Relaxed) / 1000
    );
    // Stop the game: saved, the worker ends, the recording is closed.
    assert!(handle.send(ActorMsg::Admin(AdminCtl::Stop)));
    let dir = state.join("recordings").join(&rec_name);
    wait(
        "the recording is closed",
        Box::new({
            let dir = dir.clone();
            move || recording::Recording::open(&dir).is_ok_and(|r| r.manifest.finished_ms.is_some())
        }),
    );
    let r = recording::Recording::open(&dir).unwrap();
    assert!(
        r.manifest.keyframes.len() > usize::try_from(turns).unwrap(),
        "{} keyframes",
        r.manifest.keyframes.len()
    );
    eprintln!("REC: wrote {}", dir.display());
    drop(handle);
    drop(shared);
    if !keep {
        let _ = std::fs::remove_dir_all(&state);
    }
}

fn send_cmd(handle: &server::game::GameHandle, bot: &mut Bot, turn: i32, cmd: Command) {
    let id = bot.next_id;
    bot.next_id += 1;
    assert!(handle.send(ActorMsg::Client {
        player: bot.player,
        session: bot.session,
        msg: ClientGameMsg::Cmd(Cmd { id, turn, cmd }),
        bytes: 16,
    }));
}
