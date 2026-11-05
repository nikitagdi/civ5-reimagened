//! civ5-reimagened server (§5.8). Tests bind port 0.

pub mod auth;
pub mod clock;
pub mod config;
pub mod cookie;
pub mod game;
pub mod game_host;
pub mod http;
pub mod identity;
pub mod lobby;
pub mod metrics;
pub mod prefs;
pub mod probe;
pub mod session;
pub mod state;
pub mod supervisor;
pub mod worker;

use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::serve::ListenerExt;
use protocol::LobbyChoices;
use tokio::net::TcpListener;

use crate::clock::{Clock, SystemClock, SystemWallClock, WallClock};
use crate::config::Cidr;
use crate::identity::Secret;

#[derive(Clone)]
pub struct ServerConfig {
    /// Web client dir; unset → 404.
    pub web_root: Option<PathBuf>,
    /// COOP/COEP on the web files and CORP on the blobs (`cross_origin_isolation`, B3 render-mt).
    pub cross_origin_isolation: bool,
    /// Asset dir; unset → 404.
    pub assets_root: Option<PathBuf>,
    /// A logged-in session with no frame from the client (pong included) for this long is lost
    /// (20 s).
    pub idle_timeout: Duration,
    /// Server WS ping interval (5 s).
    pub ping_interval: Duration,
    /// Rate and size limits.
    pub limits: session::limits::LimitsConfig,
    /// Send timeout: closes clients not reading.
    pub send_timeout: Duration,
    /// Window for `Hello` + `Login` from the moment the socket opens.
    pub login_timeout: Duration,
    /// The server password (`CIV5_PASSWORD`, GS-D2). It is no login password any more: it is the registration code
    /// (`code` of `POST /api/auth/register`). `None`: nobody can register (unless `open_access`).
    pub password: Option<Secret>,
    /// Registration is open to anyone (`open_access`, the public profile): no code. Rate limits, nick rules and the
    /// admin API are unchanged.
    pub open_access: bool,
    /// A WebSocket password login of an unknown nick creates the account (that password becomes the account's; the
    /// registration gate still applies). For bots and tests only: `--dev` and the in-process test config
    /// (`ServerConfig::default()`) turn it on, `Config` and the deploy profiles have it off.
    pub auto_register: bool,
    /// The session cookie carries `Secure` (default). Off for a profile served over plain http on a LAN address, where a
    /// browser would not keep a Secure cookie (loopback is exempt). SIMPLIFIED(SIMP-exe-855).
    pub cookie_secure: bool,
    /// `state_dir`: accounts and catalogs live there. `None`: accounts in memory, no catalogs.
    pub state_dir: Option<PathBuf>,
    /// Token for the admin API. `None`: the admin API answers 503.
    pub admin_token: Option<Secret>,
    /// Sessions over this many get `Reject(Full)`.
    pub max_sessions: u32,
    /// Proxies whose `X-Forwarded-For` is believed (GS-D14).
    pub trusted_proxies: Vec<Cidr>,
    /// `/netcheck/ws` connection length limit.
    pub netcheck_max_duration: Duration,
    /// `/netcheck/ws` messages per second limit.
    pub netcheck_max_per_second: usize,
    /// Monotonic gateway time (`server_ms`, timers).
    pub clock: Arc<dyn Clock>,
    /// Calendar time for `last_seen_ms`.
    pub wall_clock: Arc<dyn WallClock>,
    /// How workers are started. `None`: no games can start (the lobby answers with an error).
    pub supervisor: Option<supervisor::SupervisorConfig>,
    /// Games running at once.
    pub max_games: u32,
    /// A game does not start with less free memory than this, MiB (Linux).
    pub min_free_mem_mb: u64,
    /// Restart delays and limits of the worker supervision.
    pub restart_policy: supervisor::policy::PolicyConfig,
    /// A room member or a game host without a connection for this long loses the place or the host
    /// rights: 60 s.
    pub host_lost_s: u32,
    /// A game with nobody online for this many minutes goes to sleep; 0: never.
    pub hibernate_idle_min: u32,
    /// A game with no human online for this many minutes is closed: seats freed, saves kept (the user, 2026-11-02); 0: never.
    pub empty_close_min: u32,
    /// The test hooks  of the admin API (`--test-hooks`, GS-D18): worker signals and seat faults.
    pub test_hooks: bool,
    /// What the server keeps on disk ([retention]).
    pub retention: config::Retention,
    /// The recording of every game ([recording]).
    pub recording: config::Recording,
}

impl fmt::Debug for ServerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerConfig")
            .field("web_root", &self.web_root)
            .field("assets_root", &self.assets_root)
            .field("state_dir", &self.state_dir)
            .field("max_sessions", &self.max_sessions)
            .finish_non_exhaustive()
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            web_root: None,
            // Threads by default (B3 render-mt, the user's decision of 2026-10-29).
            cross_origin_isolation: true,
            assets_root: None,
            idle_timeout: Duration::from_secs(20),
            ping_interval: Duration::from_secs(5),
            limits: session::limits::LimitsConfig::default(),
            send_timeout: Duration::from_secs(10),
            login_timeout: Duration::from_secs(10),
            password: None,
            open_access: false,
            auto_register: true,
            cookie_secure: true,
            state_dir: None,
            admin_token: None,
            max_sessions: 64,
            trusted_proxies: Vec::new(),
            netcheck_max_duration: Duration::from_secs(120),
            netcheck_max_per_second: 2,
            clock: Arc::new(SystemClock::new()),
            wall_clock: Arc::new(SystemWallClock),
            supervisor: None,
            max_games: 4,
            min_free_mem_mb: 800,
            restart_policy: supervisor::policy::PolicyConfig::default(),
            host_lost_s: 60,
            hibernate_idle_min: 15,
            empty_close_min: 5,
            test_hooks: false,
            retention: config::Retention::default(),
            recording: config::Recording::default(),
        }
    }
}

impl ServerConfig {
    /// Runtime options from the file/flag configuration. The password and the admin token are
    /// not in [`config::Config`]; the caller sets them.
    pub fn from_config(c: &config::Config) -> Self {
        Self {
            web_root: c.web_root.clone(),
            cross_origin_isolation: c.cross_origin_isolation,
            assets_root: c.assets_root.clone(),
            state_dir: Some(c.state_dir.clone()),
            max_sessions: c.max_sessions,
            max_games: c.max_games,
            min_free_mem_mb: c.min_free_mem_mb,
            host_lost_s: c.host_lost_s,
            hibernate_idle_min: c.hibernate_idle_min,
            empty_close_min: c.empty_close_min,
            test_hooks: c.test_hooks,
            open_access: c.open_access,
            auto_register: c.auto_register,
            cookie_secure: c.cookie_secure,
            retention: c.retention.clone(),
            recording: c.recording.clone(),
            trusted_proxies: c.trusted_proxies.clone(),
            ..Self::default()
        }
    }
}

pub struct Shared {
    pub config: ServerConfig,
    pub identity: identity::Identity,
    pub lobby: lobby::Lobby,
    /// The running games.
    pub games: Arc<game::Games>,
    supervisor: Option<supervisor::Supervisor>,
    pub game_host: game_host::GameHost,
    /// Argon2 pool of the password checks (two at a time).
    pub hasher: auth::Hasher,
    pub metrics: metrics::Metrics,
    pub health: http::Health,
    pub netcheck: Arc<http::netcheck::Netcheck>,
    /// The limits of the full client reports (`POST /client-report/v2`).
    pub reports: http::reports::Reports,
    /// The bodies of the replica snapshots and patches (`GET /replica/{hash}`).
    pub replica: Arc<game::replica::ReplicaBlobs>,
    pub next_session: AtomicU64,
    /// Per-IP limits: password failures, unfinished connections.
    pub gate: Arc<session::limits::Gate>,
    /// Next game and room numbers (`ids.json`).
    pub ids: Arc<state::meta::IdStore>,
    /// The per-account store of settings (protocol v12).
    pub prefs: prefs::Prefs,
    /// The gateway is draining: new WebSockets get 503.
    pub draining: std::sync::atomic::AtomicBool,
    /// `true` once the drain is done: the sessions close their sockets with 1012.
    pub closing: tokio::sync::watch::Sender<bool>,
    /// A drain asked to end the process (`POST /admin/drain?exit=1`, SIGTERM).
    pub exit: tokio::sync::Notify,
    /// Send queues of the logged-in sessions.
    pub sessions: session::Sessions,
    choices: Mutex<Arc<LobbyChoices>>,
}

impl Shared {
    /// Opens `accounts.json` (and migrates `identities.json`) when the config has a `state_dir`.
    pub fn new(config: ServerConfig) -> anyhow::Result<Arc<Self>> {
        let identity = match &config.state_dir {
            Some(dir) => {
                identity::Identity::open(dir.join("accounts.json"), config.wall_clock.clone())?
            }
            None => identity::Identity::in_memory(config.wall_clock.clone()),
        };
        let ids = Arc::new(match &config.state_dir {
            Some(dir) => state::meta::IdStore::open(dir.join("ids.json"))?,
            None => state::meta::IdStore::in_memory(),
        });
        let prefs = match &config.state_dir {
            Some(dir) => prefs::Prefs::open(dir.join("prefs.json"))?,
            None => prefs::Prefs::in_memory(),
        };
        let netcheck = Arc::new(http::netcheck::Netcheck::new(
            config.netcheck_max_duration,
            config.netcheck_max_per_second,
        ));
        let gate = session::limits::Gate::new(config.limits.clone());
        Ok(Arc::new(Self {
            supervisor: config.supervisor.clone().map(supervisor::Supervisor::new),
            games: Arc::new(game::Games::with_ids(Arc::clone(&ids))),
            ids,
            prefs,
            draining: std::sync::atomic::AtomicBool::new(false),
            closing: tokio::sync::watch::channel(false).0,
            exit: tokio::sync::Notify::new(),
            gate,
            sessions: session::Sessions::default(),
            choices: Mutex::new(Arc::new(LobbyChoices::default())),
            config,
            identity,
            lobby: lobby::Lobby::default(),
            game_host: game_host::GameHost::spawn(),
            hasher: auth::Hasher::new(),
            metrics: metrics::Metrics::default(),
            health: http::Health::default(),
            netcheck,
            reports: http::reports::Reports::default(),
            replica: Arc::default(),
            next_session: AtomicU64::new(1),
        }))
    }
}

impl Shared {
    /// The supervisor that starts workers, if the configuration has one.
    pub fn supervisor(&self) -> Option<&supervisor::Supervisor> {
        self.supervisor.as_ref()
    }

    /// Starts the lobby actor (once). Needs a runtime; sessions call it on their first use.
    pub fn ensure_actors(self: &Arc<Self>) {
        self.lobby.start(self);
    }

    /// Starts a game: its worker and its actor. The lobby is told when it is up
    /// (`LobbyMsg::GameStarted`) or when it is not (`GameFailed`).
    pub fn start_game(
        &self,
        init: game::GameInit,
        choices: Arc<LobbyChoices>,
    ) -> Result<game::GameHandle, String> {
        let supervisor = self
            .supervisor
            .clone()
            .ok_or_else(|| "no game worker is configured".to_owned())?;
        let env = game::GameEnv {
            supervisor,
            clock: Arc::clone(&self.config.clock),
            limits: self.config.limits.clone(),
            lobby: self.lobby.tx(),
            games: Arc::clone(&self.games),
            policy: self.config.restart_policy.clone(),
            shutdown_grace: game::SHUTDOWN_GRACE,
            choices,
            host_lost_ms: u64::from(self.config.host_lost_s) * 1000,
            hibernate_idle_ms: u64::from(self.config.hibernate_idle_min) * 60_000,
            empty_close_ms: u64::from(self.config.empty_close_min) * 60_000,
            state_dir: self.config.state_dir.clone(),
            wall: Arc::clone(&self.config.wall_clock),
            replica: Arc::clone(&self.replica),
            recording: self.config.recording.clone(),
        };
        Ok(game::spawn(env, init))
    }

    /// Drains the gateway: new WebSockets get 503, rooms are dissolved, every game is saved and its
    /// worker stopped (25 s in all), then the sockets are closed with 1012. With `exit` the process is asked
    /// to end afterwards. A second call does nothing.
    pub async fn drain(self: &Arc<Self>, exit: bool) {
        if self
            .draining
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            if exit {
                self.exit.notify_one();
            }
            return;
        }
        tracing::info!("draining: saving the games");
        self.lobby.send(lobby::LobbyMsg::Drain);
        for g in self.games.list() {
            g.send(game::ActorMsg::Admin(game::AdminCtl::Drain));
        }
        let deadline = tokio::time::Instant::now() + DRAIN_LIMIT;
        while tokio::time::Instant::now() < deadline
            && self
                .games
                .list()
                .iter()
                .any(|g| g.info.snapshot().state != supervisor::GameState::Faulted)
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let _ = self.closing.send(true);
        // Let the sessions write their Close frames.
        tokio::time::sleep(Duration::from_millis(300)).await;
        if exit {
            self.exit.notify_one();
        }
    }

    /// The pick lists every login receives (`Choices`). Empty until the `Describe` run is in.
    pub fn choices(&self) -> Arc<LobbyChoices> {
        Arc::clone(&self.choices.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Replaces the pick lists (the `Describe` answer).
    pub fn set_choices(&self, choices: LobbyChoices) {
        *self.choices.lock().unwrap_or_else(PoisonError::into_inner) = Arc::new(choices);
    }
}

pub const BUILD: &str = env!("CARGO_PKG_VERSION");

/// How long a drain waits for the games to be saved.
const DRAIN_LIMIT: Duration = Duration::from_secs(25);

/// Serves the game listener: `/ws`, `/healthz`, `/catalog`, `/netcheck/ws`, web and assets.
pub async fn serve(listener: TcpListener, shared: Arc<Shared>) -> std::io::Result<()> {
    let app = http::router(shared).into_make_service_with_connect_info::<SocketAddr>();
    // Small WS frames (Pong, turn state) must not wait for Nagle (GS-D5).
    let listener = listener.tap_io(|tcp| {
        if let Err(e) = tcp.set_nodelay(true) {
            tracing::warn!(error = %e, "cannot set TCP_NODELAY");
        }
    });
    axum::serve(listener, app).await
}

/// Serves the admin API. Refuses a non-loopback listener.
pub async fn serve_admin(listener: TcpListener, shared: Arc<Shared>) -> std::io::Result<()> {
    if !listener.local_addr()?.ip().is_loopback() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the admin API listens on loopback only",
        ));
    }
    axum::serve(listener, http::admin::router(shared)).await
}
