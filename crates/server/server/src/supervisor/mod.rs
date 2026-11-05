//! Worker supervisor. Starts worker processes
//! (`current_exe() worker`), speaks IPC with them and runs the `Describe` pass that fills the
//! lobby. This part covers start-up: `Init`, `Catalog`, `Ready`. Hang detection, exit codes and
//! restarts (N2.5) build on [`WorkerProc`] and [`WorkerEvent`].

pub mod ipc;
pub mod policy;
pub mod spawn;

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use protocol::{
    CmdId, CoreKind, GameId, IPC_VERSION, LobbyChoices, PlayerId, SlotControl, SlotSetup, Start,
    ToWorker, WorkerConfig, WorkerFrame, WorkerInit, WorkerMsg, WorkerReady,
};
use tokio::sync::mpsc;
use tokio::time::Instant;

pub use self::ipc::{ExitInfo, IpcTx, SendError, WorkerEvent};
use self::policy::{
    Decision, ExitKind, Failure, OrderLedger, PolicyConfig, RestartPolicy, SaveBook, SaveEntry,
    classify_exit, restored_line,
};
pub use self::spawn::{SpawnSpec, WorkerProc};
use crate::Shared;
use crate::clock::Clock;
use crate::config::{self, Config};
use crate::http::catalog;

/// A worker did not come up.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("cannot start the worker process: {0}")]
    Spawn(#[source] io::Error),
    #[error("the worker's pipe is closed")]
    Closed,
    #[error("the worker reported a fatal error (code {code}): {message}")]
    Fatal { message: String, code: u8 },
    #[error("the worker wrote something that is not an IPC frame: {0}")]
    Garbage(String),
    #[error("the worker said nothing for {0:?}")]
    Silent(Duration),
    #[error("the worker exited before it was ready: {0:?}")]
    Exited(ExitInfo),
    #[error("the worker answered `Describe` without `Choices`")]
    NoChoices,
    #[error("catalog hash {claimed:016x} does not match its body ({actual:016x})")]
    CatalogHash { claimed: u64, actual: u64 },
}

/// A worker that answered `Init` with `Ready`.
pub struct Started {
    pub proc: WorkerProc,
    pub ready: WorkerReady,
    /// The catalog the worker sent before `Ready`: `(hash, body)`.
    pub catalog: Option<(u64, Vec<u8>)>,
    /// Messages that came in the same frame after `Ready`: the caller handles them first.
    pub early: Vec<WorkerMsg>,
}

/// The answer of a `Describe` run (GS-D17).
#[derive(Debug)]
pub struct Described {
    pub choices: LobbyChoices,
    pub catalog: Option<(u64, Vec<u8>)>,
    /// Core build id (with the FIX/HYG flags), from the `Ready` of the run if the worker sent one.
    pub build_id: Option<String>,
}

/// Starts `spec`, sends `Init` and waits for `Catalog` and `Ready`. Silence of `silence` (any
/// frame, `Heartbeat` included, resets it) kills the process: `Starting` is bound by the same
/// hang rule as `Running`.
pub async fn start_worker(
    spec: &SpawnSpec,
    init: WorkerInit,
    silence: Duration,
) -> Result<Started, StartError> {
    let mut proc = spawn::spawn(spec).map_err(StartError::Spawn)?;
    proc.send(ToWorker::Init(init))
        .map_err(|_| StartError::Closed)?;
    let mut catalog = None;
    loop {
        let event = next_event(&mut proc, silence).await?;
        let WorkerEvent::Frame(frame) = event else {
            match event {
                WorkerEvent::Garbage(why) => {
                    proc.kill();
                    return Err(StartError::Garbage(why));
                }
                WorkerEvent::Exited(info) => return Err(StartError::Exited(info)),
                // stdout closed: `Exited` follows.
                _ => continue,
            }
        };
        let mut msgs = frame.msgs.into_iter();
        while let Some(msg) = msgs.next() {
            match msg {
                WorkerMsg::Catalog { hash, bytes } => {
                    catalog = Some(normalize_catalog(hash, bytes)?);
                }
                WorkerMsg::Ready(ready) => {
                    return Ok(Started {
                        proc,
                        ready,
                        catalog,
                        early: msgs.collect(),
                    });
                }
                WorkerMsg::Fatal { message, code } => {
                    proc.kill();
                    return Err(StartError::Fatal { message, code });
                }
                // `Heartbeat`: it only kept the silence timer from firing.
                _ => {}
            }
        }
    }
}

/// Runs a `Describe` pass: the worker answers `Choices` and `Catalog` and exits 0.
pub async fn describe(
    spec: &SpawnSpec,
    init: WorkerInit,
    silence: Duration,
) -> Result<Described, StartError> {
    let mut proc = spawn::spawn(spec).map_err(StartError::Spawn)?;
    proc.send(ToWorker::Init(init))
        .map_err(|_| StartError::Closed)?;
    let (mut choices, mut catalog, mut build_id) = (None, None, None);
    loop {
        match next_event(&mut proc, silence).await? {
            WorkerEvent::Frame(frame) => {
                for msg in frame.msgs {
                    match msg {
                        WorkerMsg::Choices(c) => choices = Some(c),
                        WorkerMsg::Catalog { hash, bytes } => {
                            catalog = Some(normalize_catalog(hash, bytes)?);
                        }
                        WorkerMsg::Ready(ready) => build_id = Some(ready.build_id),
                        WorkerMsg::Fatal { message, code } => {
                            proc.kill();
                            return Err(StartError::Fatal { message, code });
                        }
                        _ => {}
                    }
                }
            }
            WorkerEvent::Garbage(why) => {
                proc.kill();
                return Err(StartError::Garbage(why));
            }
            WorkerEvent::Eof => {}
            WorkerEvent::Exited(info) if info.code == Some(0) => {
                let choices = choices.ok_or(StartError::NoChoices)?;
                return Ok(Described {
                    choices,
                    catalog,
                    build_id,
                });
            }
            WorkerEvent::Exited(info) => return Err(StartError::Exited(info)),
        }
    }
}

/// Next event within `silence`; on silence the process is killed.
async fn next_event(proc: &mut WorkerProc, silence: Duration) -> Result<WorkerEvent, StartError> {
    match tokio::time::timeout(silence, proc.next_event()).await {
        Err(_) => {
            proc.kill();
            Err(StartError::Silent(silence))
        }
        Ok(None) => Err(StartError::Closed),
        Ok(Some(event)) => Ok(event),
    }
}

/// The body `GET /catalog/{hash}` serves: `C5CT`, `u16 LE` version, the postcard catalog; its hash is
/// `xxh64` of the whole body (NP-D14). A worker that already sends the prefix is taken as it is;
/// one that sends the bare postcard is wrapped here.
fn normalize_catalog(claimed: u64, bytes: Vec<u8>) -> Result<(u64, Vec<u8>), StartError> {
    check_catalog(claimed, &bytes)?;
    if bytes.starts_with(&wire::catalog::CATALOG_MAGIC) {
        return Ok((claimed, bytes));
    }
    let mut body = Vec::with_capacity(bytes.len() + wire::catalog::CATALOG_HEADER_LEN);
    body.extend_from_slice(&wire::catalog::catalog_header());
    body.extend_from_slice(&bytes);
    Ok((protocol::xxh64(0, &body), body))
}

fn check_catalog(claimed: u64, bytes: &[u8]) -> Result<(), StartError> {
    let actual = protocol::xxh64(0, bytes);
    if actual == claimed {
        Ok(())
    } else {
        Err(StartError::CatalogHash { claimed, actual })
    }
}

/// What the supervisor needs to start workers.
#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    /// The worker program: `current_exe()` for the gateway, the mock worker in tests.
    pub exe: PathBuf,
    /// Arguments of the worker program (`["worker"]` for the gateway).
    pub args: Vec<OsString>,
    pub state_dir: PathBuf,
    pub core: CoreKind,
    /// Settings every worker gets in `Init`.
    pub worker: WorkerConfig,
    /// Silence of a worker that counts as a hang (`hang_s`, 30 s).
    pub hang: Duration,
    /// A worker's stderr file turns over at this size (`retention.worker_log_max_mb`); 0: never.
    pub stderr_max_bytes: u64,
}

impl SupervisorConfig {
    /// The gateway's own settings: `current_exe() worker` with the core and worker options of
    /// the configuration.
    pub fn from_config(c: &Config) -> io::Result<Self> {
        let core = match c.core {
            config::CoreKind::Fixture => CoreKind::Fixture {
                huge: false,
                // Dev only: a map that wraps in X (the terrain-scroll scenario looks for the seam).
                wrap_x: std::env::var_os("CIV5_FIXTURE_WRAP_X").is_some_and(|v| v != "0"),
            },
            config::CoreKind::Real => CoreKind::Real {
                gamedata_dir: c
                    .gamedata_dir
                    .as_deref()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            },
        };
        let mut worker = WorkerConfig {
            audit_fog: c.worker.audit_fog,
            // The step statistics feed `fog_violations` and the step histograms of the metrics.
            stats: true,
            record: c.recording.enabled,
            autosave: protocol::AutosaveCfg {
                min_interval_s: c.worker.autosave.min_interval_s,
                ring: c.worker.autosave.ring,
                keep_every: c.worker.autosave.keep_every,
            },
            ..WorkerConfig::default()
        };
        if c.test_hooks {
            worker.fixture_test = c
                .worker
                .fixture_test
                .as_ref()
                .map(|f| protocol::FixtureTest {
                    ai_step_delay_ms: f.ai_step_delay_ms,
                    leak_fog: f.leak_fog,
                    change_basis_at_turn: None,
                });
        }
        Ok(Self {
            exe: std::env::current_exe()?,
            args: vec![OsString::from("worker")],
            state_dir: c.state_dir.clone(),
            core,
            worker,
            hang: Duration::from_secs(u64::from(c.hang_s)),
            stderr_max_bytes: c.retention.worker_log_max_mb.saturating_mul(1024 * 1024),
        })
    }
}

/// Starts workers on behalf of the gateway.
#[derive(Debug, Clone)]
pub struct Supervisor {
    cfg: SupervisorConfig,
}

impl Supervisor {
    pub fn new(cfg: SupervisorConfig) -> Self {
        Self { cfg }
    }

    pub fn config(&self) -> &SupervisorConfig {
        &self.cfg
    }

    fn spec(&self, log_name: &str, work_dir: PathBuf) -> SpawnSpec {
        SpawnSpec {
            exe: self.cfg.exe.clone(),
            args: self.cfg.args.clone(),
            work_dir,
            stderr_log: self
                .cfg
                .state_dir
                .join("logs")
                .join(format!("worker-{log_name}.log")),
            stderr_max_bytes: self.cfg.stderr_max_bytes,
        }
    }

    fn init(&self, game: GameId, start: Start, seed: u64, save_dir: String) -> WorkerInit {
        WorkerInit {
            ipc: IPC_VERSION,
            game,
            core: self.cfg.core.clone(),
            start,
            save_dir,
            seed,
            cfg: self.cfg.worker.clone(),
        }
    }

    /// The `Describe` pass (GS-D17): pick lists, catalog and core build id, no game.
    pub async fn describe(&self) -> Result<Described, StartError> {
        let spec = self.spec("describe", self.cfg.state_dir.clone());
        let init = self.init(0, Start::Describe, 0, String::new());
        describe(&spec, init, self.cfg.hang).await
    }

    /// Runs `Describe` and opens the lobby with its answer: the pick lists go to `shared`, the
    /// catalog to `state_dir/catalog/<hash>.bin`, `/healthz` turns 200. On an error the lobby
    /// stays closed and `/healthz` stays 503 (GS-D17).
    pub async fn publish_describe(&self, shared: &Shared) -> anyhow::Result<Described> {
        let described = self.describe().await?;
        if let Some((_, body)) = &described.catalog {
            let dir = self.cfg.state_dir.join("catalog");
            let body = body.clone();
            tokio::task::spawn_blocking(move || {
                std::fs::create_dir_all(&dir)?;
                catalog::store(&dir, &body)
            })
            .await??;
        }
        shared.set_choices(described.choices.clone());
        shared.health.set_ready(
            described
                .build_id
                .clone()
                .unwrap_or_else(|| "unknown".to_owned()),
        );
        Ok(described)
    }

    /// Starts the worker of game `game`: `state_dir/games/<game>/` is its working directory and
    /// its save directory, its stderr goes to `logs/worker-<game>.log`.
    pub async fn start_game(
        &self,
        game: GameId,
        start: Start,
        seed: u64,
    ) -> Result<Started, StartError> {
        let work_dir = self.cfg.state_dir.join("games").join(game.to_string());
        let spec = self.spec(&game.to_string(), work_dir.clone());
        let init = self.init(game, start, seed, work_dir.to_string_lossy().into_owned());
        start_worker(&spec, init, self.cfg.hang).await
    }
}

// ---------------------------------------------------------------- one game's worker

/// Where the worker of a game is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameState {
    /// The first worker is starting.
    Starting,
    Running,
    /// A worker failed; the next one is on its way. Orders are refused meanwhile.
    Restarting,
    /// Too many failures: no worker, nothing starts it but an administrator.
    Faulted,
    /// Stopped on request, or a new game that was refused.
    Finished,
}

impl GameState {
    fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Starting,
            1 => Self::Running,
            2 => Self::Restarting,
            3 => Self::Faulted,
            _ => Self::Finished,
        }
    }
}

/// What the supervisor tells the game actor, in order.
#[derive(Debug)]
pub enum GameEvent {
    /// A worker is ready (the first one, or after a restart).
    Started(StartedInfo),
    /// A frame of the worker, after the supervisor took its notes of it.
    Frame(WorkerFrame),
    /// An order that cannot go to a worker: answer `Rejected { Restarting }`.
    Rejected { player: PlayerId, id: CmdId },
    /// A worker failed; the next one starts after `delay`. Orders in `lost` get `Lost`.
    Restarting(RestartingInfo),
    /// No more restarts: the seats get `Detached { GameStopped }`.
    Faulted(FaultedInfo),
    /// A new game did not start (exit 5): the room is told.
    StartRefused { why: String },
    /// The worker stopped on request; `None`: it was not running.
    Finished { exit: Option<ExitInfo> },
}

#[derive(Debug)]
pub struct StartedInfo {
    pub ready: WorkerReady,
    pub catalog: Option<(u64, Vec<u8>)>,
    /// Set when this worker replaces one that failed.
    pub restart: Option<RestartInfo>,
    /// The process id of the worker.
    pub pid: Option<u32>,
}

/// The restored game after a restart.
#[derive(Debug)]
pub struct RestartInfo {
    /// The save the new worker loaded.
    pub from_save: SaveEntry,
    /// How many newer saves were passed over.
    pub skipped: usize,
    /// One `System` journal line per human seat (AC-N1.5: the loss is not silent).
    pub journal: Vec<(PlayerId, String)>,
    /// Orders that were applied but are not in the loaded save, per seat.
    pub rolled_back: Vec<(PlayerId, u32)>,
}

#[derive(Debug)]
pub struct RestartingInfo {
    /// `None`: an administrator restarted the game.
    pub failure: Option<Failure>,
    pub exit: Option<ExitInfo>,
    pub delay: Duration,
    pub lost: Vec<(PlayerId, Vec<CmdId>)>,
    pub rolled_back: Vec<(PlayerId, u32)>,
}

#[derive(Debug)]
pub struct FaultedInfo {
    pub why: String,
    pub failure: Option<Failure>,
    pub lost: Vec<(PlayerId, Vec<CmdId>)>,
}

/// How to supervise one game.
pub struct GameSpec {
    pub game: GameId,
    /// `New` for a fresh game, `Load` to continue a save.
    pub start: Start,
    pub seed: u64,
    /// The saves of the game, for restarts.
    pub saves: SaveBook,
    /// Who sits where now; a restart loads with the seats of that moment.
    pub seats: Vec<SlotSetup>,
    pub policy: PolicyConfig,
    /// A `Shutdown` that is not obeyed within this long becomes a kill (5 s).
    pub shutdown_grace: Duration,
    /// Time of the restart rules (`Clock`, not tokio time).
    pub clock: Arc<dyn Clock>,
}

// One value per message, moved once: boxing would only add an allocation.
#[allow(clippy::large_enum_variant)]
enum Control {
    Send(ToWorker),
    Shutdown { save: bool },
    Restart,
}

/// The handle of a supervised game. Dropping it stops the game: the worker gets a `Shutdown` with
/// a save.
pub struct GameSupervisor {
    ctl: mpsc::UnboundedSender<Control>,
    state: Arc<AtomicU8>,
    seats: Arc<Mutex<Vec<SlotSetup>>>,
}

impl GameSupervisor {
    pub fn state(&self) -> GameState {
        GameState::from_u8(self.state.load(Ordering::Acquire))
    }

    /// Sends a frame to the worker. While the game is not `Running` the answer is the state, and
    /// the caller refuses the order (`Rejected { Restarting }`); an order that races a failure
    /// comes back as [`GameEvent::Rejected`] instead.
    pub fn send(&self, msg: ToWorker) -> Result<(), GameState> {
        let state = self.state();
        if state != GameState::Running {
            return Err(state);
        }
        self.ctl
            .send(Control::Send(msg))
            .map_err(|_| GameState::Finished)
    }

    /// `Shutdown { save }`; a worker that does not exit in `shutdown_grace` is killed.
    pub fn shutdown(&self, save: bool) {
        let _ = self.ctl.send(Control::Shutdown { save });
    }

    /// Administrator restart (`POST /admin/games/{id}/restart`): from `Faulted`, or a running
    /// worker is killed and replaced from the newest save. The failure history is forgotten.
    pub fn restart(&self) {
        let _ = self.ctl.send(Control::Restart);
    }

    /// The seats a restart loads with (the truth about who sits where, game-host-mp 4.12).
    pub fn set_seats(&self, seats: Vec<SlotSetup>) {
        *self.seats.lock().unwrap_or_else(PoisonError::into_inner) = seats;
    }
}

impl Drop for GameSupervisor {
    fn drop(&mut self) {
        let _ = self.ctl.send(Control::Shutdown { save: true });
    }
}

impl Supervisor {
    /// Starts supervising a game: starts its worker and keeps it alive by the rules of spec 4.10.
    /// Events come in order on the returned queue; it closes when supervision ends.
    pub fn supervise(&self, spec: GameSpec) -> (GameSupervisor, mpsc::Receiver<GameEvent>) {
        let (ctl_tx, ctl) = mpsc::unbounded_channel();
        let (events, rx) = mpsc::channel(256);
        let state = Arc::new(AtomicU8::new(GameState::Starting as u8));
        let seats = Arc::new(Mutex::new(spec.seats));
        let task = GameTask {
            sup: self.clone(),
            game: spec.game,
            seed: spec.seed,
            policy: RestartPolicy::new(spec.policy),
            ledger: OrderLedger::new(),
            saves: spec.saves,
            seats: Arc::clone(&seats),
            clock: spec.clock,
            grace: spec.shutdown_grace,
            events,
            ctl,
            state: Arc::clone(&state),
            new_game: matches!(spec.start, Start::New(_)),
            ever_ready: false,
            rolled_back: Vec::new(),
        };
        tokio::spawn(task.run(spec.start));
        (
            GameSupervisor {
                ctl: ctl_tx,
                state,
                seats,
            },
            rx,
        )
    }
}

/// How one worker's run ended.
enum Outcome {
    /// Stopped on request, or nobody listens any more.
    Stopped,
    Failed {
        failure: Failure,
        exit: Option<ExitInfo>,
    },
    AdminRestart {
        exit: Option<ExitInfo>,
    },
}

/// What to do after a failure.
// One value per failure; see `Control`.
#[allow(clippy::large_enum_variant)]
enum Next {
    Start(Start, Option<(SaveEntry, usize)>),
    Stop,
}

struct GameTask {
    sup: Supervisor,
    game: GameId,
    seed: u64,
    policy: RestartPolicy,
    ledger: OrderLedger,
    saves: SaveBook,
    seats: Arc<Mutex<Vec<SlotSetup>>>,
    clock: Arc<dyn Clock>,
    grace: Duration,
    events: mpsc::Sender<GameEvent>,
    ctl: mpsc::UnboundedReceiver<Control>,
    state: Arc<AtomicU8>,
    /// The game started as a new one (not from a save).
    new_game: bool,
    /// Some worker of this game has been ready.
    ever_ready: bool,
    /// Rolled-back orders of the last failure, for the journal lines of the restart.
    rolled_back: Vec<(PlayerId, u32)>,
}

impl GameTask {
    fn set(&self, state: GameState) {
        self.state.store(state as u8, Ordering::Release);
    }

    /// `false`: nobody listens any more.
    async fn emit(&self, ev: GameEvent) -> bool {
        self.events.send(ev).await.is_ok()
    }

    fn current_seats(&self) -> Vec<SlotSetup> {
        self.seats
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    async fn run(mut self, first: Start) {
        let mut start = first;
        let mut restart: Option<(SaveEntry, usize)> = None;
        loop {
            self.set(if restart.is_some() {
                GameState::Restarting
            } else {
                GameState::Starting
            });
            let (failure, exit) = match self.sup.start_game(self.game, start, self.seed).await {
                Ok(started) => match self.run_worker(started, restart.take()).await {
                    Outcome::Stopped => {
                        self.set(GameState::Finished);
                        return;
                    }
                    Outcome::Failed { failure, exit } => (Some(failure), exit),
                    Outcome::AdminRestart { exit } => (None, exit),
                },
                Err(e) => {
                    tracing::warn!(game = self.game, error = %e, "the worker did not come up");
                    (Some(failure_of(&e)), exit_of(&e))
                }
            };
            match self.after_failure(failure, exit).await {
                Next::Start(s, r) => {
                    start = s;
                    restart = r;
                }
                Next::Stop => {
                    return;
                }
            }
        }
    }

    /// Decides by the policy; waits out the delay. `failure` `None` is an administrator restart.
    async fn after_failure(&mut self, failure: Option<Failure>, exit: Option<ExitInfo>) -> Next {
        let casualties = self.ledger.worker_lost();
        self.rolled_back.clone_from(&casualties.rolled_back);
        let decision = match failure {
            Some(f) => {
                self.policy
                    .on_failure(f, self.new_game && !self.ever_ready, self.clock.now_ms())
            }
            None => {
                self.policy.reset();
                Decision::Restart {
                    delay: Duration::ZERO,
                    skip: 0,
                }
            }
        };
        match decision {
            Decision::Restart { delay, skip } => {
                let Some(save) = self.saves.pick(skip).cloned() else {
                    // A new game whose first worker never came up has nothing to restart from: the start
                    // is refused and the room gathers again, the game is not faulted.
                    if self.new_game && !self.ever_ready {
                        let why = match failure {
                            Some(f) => format!("the worker did not build the game ({f:?})"),
                            None => "the worker did not build the game".to_owned(),
                        };
                        tracing::warn!(game = self.game, %why, "a new game did not start");
                        self.set(GameState::Finished);
                        self.emit(GameEvent::StartRefused { why }).await;
                        return Next::Stop;
                    }
                    return self
                        .fault("no save to restart from".into(), failure, casualties.lost)
                        .await;
                };
                tracing::warn!(
                    game = self.game, ?failure, ?exit, delay_ms = delay.as_millis() as u64,
                    save = %save.file, skipped = skip, "restarting the worker"
                );
                self.set(GameState::Restarting);
                let info = RestartingInfo {
                    failure,
                    exit,
                    delay,
                    lost: casualties.lost,
                    rolled_back: casualties.rolled_back,
                };
                if !self.emit(GameEvent::Restarting(info)).await {
                    return Next::Stop;
                }
                if !self.wait_restart(delay).await {
                    return Next::Stop;
                }
                let start = Start::Load {
                    file: save.file.clone(),
                    seats: self.current_seats(),
                };
                Next::Start(start, Some((save, skip)))
            }
            Decision::Faulted(why) => self.fault(why, failure, casualties.lost).await,
            Decision::RefuseStart(why) => {
                tracing::warn!(game = self.game, %why, "a new game did not start");
                self.set(GameState::Finished);
                self.emit(GameEvent::StartRefused { why }).await;
                Next::Stop
            }
        }
    }

    /// Waits `delay` for the next worker; orders meanwhile are refused. `false`: stop.
    async fn wait_restart(&mut self, delay: Duration) -> bool {
        let until = Instant::now() + delay;
        loop {
            tokio::select! {
                () = tokio::time::sleep_until(until) => return true,
                c = self.ctl.recv() => match c {
                    Some(Control::Send(msg)) => {
                        if !self.reject(msg).await {
                            return false;
                        }
                    }
                    // An administrator does not want to wait.
                    Some(Control::Restart) => return true,
                    Some(Control::Shutdown { .. }) | None => {
                        self.set(GameState::Finished);
                        self.emit(GameEvent::Finished { exit: None }).await;
                        return false;
                    }
                },
            }
        }
    }

    /// An order that has no worker to go to. `false`: nobody listens any more.
    async fn reject(&self, msg: ToWorker) -> bool {
        match msg {
            ToWorker::Cmd { player, cmd } => {
                self.emit(GameEvent::Rejected { player, id: cmd.id }).await
            }
            // Queries and focus changes get no answer while restarting.
            _ => true,
        }
    }

    async fn fault(
        &mut self,
        why: String,
        failure: Option<Failure>,
        lost: Vec<(PlayerId, Vec<CmdId>)>,
    ) -> Next {
        tracing::error!(game = self.game, %why, "the game is faulted");
        self.set(GameState::Faulted);
        let info = FaultedInfo { why, failure, lost };
        if !self.emit(GameEvent::Faulted(info)).await {
            return Next::Stop;
        }
        loop {
            match self.ctl.recv().await {
                Some(Control::Restart) => {
                    self.policy.reset();
                    let Some(save) = self.saves.pick(0).cloned() else {
                        tracing::warn!(game = self.game, "no save to restart from");
                        continue;
                    };
                    let start = Start::Load {
                        file: save.file.clone(),
                        seats: self.current_seats(),
                    };
                    self.set(GameState::Restarting);
                    return Next::Start(start, Some((save, 0)));
                }
                Some(Control::Send(msg)) => {
                    if !self.reject(msg).await {
                        return Next::Stop;
                    }
                }
                Some(Control::Shutdown { .. }) | None => return Next::Stop,
            }
        }
    }

    /// Watches one worker until it ends.
    async fn run_worker(
        &mut self,
        started: Started,
        restart: Option<(SaveEntry, usize)>,
    ) -> Outcome {
        let Started {
            mut proc,
            ready,
            catalog,
            early,
        } = started;
        self.ever_ready = true;
        let info = restart.map(|(from_save, skipped)| {
            self.policy.on_restarted(self.clock.now_ms());
            let rolled_back = std::mem::take(&mut self.rolled_back);
            let journal = ready
                .seats
                .iter()
                .filter(|s| matches!(s.control, SlotControl::Human { .. }))
                .filter_map(|s| {
                    let n = rolled_back
                        .iter()
                        .find(|(p, _)| *p == s.player)
                        .map_or(0, |(_, n)| *n);
                    (n > 0).then(|| (s.player, restored_line(n)))
                })
                .collect();
            RestartInfo {
                from_save,
                skipped,
                journal,
                rolled_back,
            }
        });
        let started_ev = GameEvent::Started(StartedInfo {
            ready,
            catalog,
            restart: info,
            pid: proc.pid(),
        });
        // Running before the event, so that the actor can send as soon as it has read it.
        self.set(GameState::Running);
        if !self.emit(started_ev).await {
            proc.kill();
            return Outcome::Stopped;
        }
        if !early.is_empty() && !self.forward(WorkerFrame { msgs: early }).await {
            proc.kill();
            return Outcome::Stopped;
        }

        let hang = self.sup.cfg.hang;
        let mut last_frame = Instant::now();
        let mut shutdown_at: Option<Instant> = None;
        let (mut killed_hang, mut killed_garbage, mut killed_grace, mut admin) =
            (false, false, false, false);
        let (mut gone, mut ctl_open) = (false, true);
        let exit = loop {
            let hang_at = last_frame + hang;
            let grace_at = shutdown_at.unwrap_or(hang_at) + self.grace;
            tokio::select! {
                ev = proc.next_event() => match ev {
                    Some(WorkerEvent::Frame(frame)) => {
                        last_frame = Instant::now();
                        if !self.forward(frame).await {
                            gone = true;
                            proc.kill();
                        }
                    }
                    Some(WorkerEvent::Garbage(why)) => {
                        tracing::error!(game = self.game, %why, "garbage from the worker, killing it");
                        killed_garbage = true;
                        proc.kill();
                    }
                    Some(WorkerEvent::Eof) => {}
                    Some(WorkerEvent::Exited(info)) => break Some(info),
                    None => break None,
                },
                c = self.ctl.recv(), if ctl_open => match c {
                    Some(Control::Send(msg)) => self.send_to(&proc, msg).await,
                    Some(Control::Shutdown { save }) => {
                        if shutdown_at.is_none() {
                            let _ = proc.send(ToWorker::Shutdown { save });
                            shutdown_at = Some(Instant::now());
                        }
                    }
                    Some(Control::Restart) => {
                        admin = true;
                        proc.kill();
                    }
                    None => {
                        // The handle is gone. After a `Shutdown` that is the normal end of
                        // things; otherwise nobody can use the game any more.
                        ctl_open = false;
                        if shutdown_at.is_none() {
                            gone = true;
                            proc.kill();
                        }
                    }
                },
                () = tokio::time::sleep_until(hang_at),
                    if shutdown_at.is_none() && !killed_hang && !killed_garbage && !admin => {
                    tracing::error!(game = self.game, silent_s = hang.as_secs(), "the worker is silent, killing it");
                    killed_hang = true;
                    proc.kill();
                }
                () = tokio::time::sleep_until(grace_at), if shutdown_at.is_some() && !killed_grace => {
                    tracing::warn!(game = self.game, "the worker ignores Shutdown, killing it");
                    killed_grace = true;
                    proc.kill();
                }
            }
        };
        if gone {
            return Outcome::Stopped;
        }
        if shutdown_at.is_some() {
            self.set(GameState::Finished);
            self.emit(GameEvent::Finished { exit }).await;
            return Outcome::Stopped;
        }
        if admin {
            return Outcome::AdminRestart { exit };
        }
        match classify_exit(
            exit.and_then(|e| e.code),
            false,
            killed_hang,
            killed_garbage,
        ) {
            ExitKind::Failed(failure) => Outcome::Failed { failure, exit },
            // `classify_exit` calls an exit 0 clean only after a requested shutdown.
            ExitKind::Clean => Outcome::Failed {
                failure: Failure::Crash,
                exit,
            },
        }
    }

    /// Takes the notes of a frame and hands it on. `false`: nobody listens any more.
    async fn forward(&mut self, frame: WorkerFrame) -> bool {
        self.ledger.observe(&frame);
        for msg in &frame.msgs {
            if let WorkerMsg::Saved(rec) = msg {
                self.saves.record(rec);
            }
        }
        self.emit(GameEvent::Frame(frame)).await
    }

    async fn send_to(&mut self, proc: &WorkerProc, msg: ToWorker) {
        if let ToWorker::Cmd { player, cmd } = &msg {
            let (player, id) = (*player, cmd.id);
            if proc.send(msg).is_ok() {
                self.ledger.sent(player, id);
            } else {
                // The pipe is full or closed; the failure itself shows up as an exit.
                self.emit(GameEvent::Rejected { player, id }).await;
            }
        } else {
            let _ = proc.send(msg);
        }
    }
}

/// The failure a start error stands for.
fn failure_of(e: &StartError) -> Failure {
    match e {
        StartError::Silent(_) => Failure::Hang,
        StartError::Garbage(_) => Failure::Garbage,
        StartError::Fatal { code, .. } => {
            match classify_exit(Some(i32::from(*code)), false, false, false) {
                ExitKind::Failed(f) => f,
                ExitKind::Clean => Failure::Crash,
            }
        }
        StartError::Exited(info) => match classify_exit(info.code, false, false, false) {
            ExitKind::Failed(f) => f,
            ExitKind::Clean => Failure::Crash,
        },
        _ => Failure::Crash,
    }
}

fn exit_of(e: &StartError) -> Option<ExitInfo> {
    match e {
        StartError::Exited(info) => Some(*info),
        _ => None,
    }
}
