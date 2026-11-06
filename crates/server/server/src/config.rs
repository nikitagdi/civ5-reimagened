//! Gateway configuration: `server.toml` plus command-line flags.
//! Flags override the file. The password is never read from here: only from the environment
//! (see [`resolve_password`]).

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, bail};
use serde::Deserialize;

/// Environment variable with the shared server password (GS-D2).
pub const PASSWORD_ENV: &str = "CIV5_PASSWORD";
/// Minimum password length in characters (GS-D2).
pub const MIN_PASSWORD_CHARS: usize = 8;
/// Password used by `--dev`.
pub const DEV_PASSWORD: &str = "dev";
/// `state_dir` used by `--dev` unless overridden.
pub const DEV_STATE_DIR: &str = "target/dev-state";

/// Which brain the workers run (GS-D13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CoreKind {
    /// Test world, runs everywhere.
    Fixture,
    /// Real game core, Linux only.
    Real,
}

impl CoreKind {
    /// `fixture` on Windows, `real` elsewhere.
    pub fn platform_default() -> Self {
        if cfg!(windows) {
            Self::Fixture
        } else {
            Self::Real
        }
    }
}

impl FromStr for CoreKind {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s {
            "fixture" => Ok(Self::Fixture),
            "real" => Ok(Self::Real),
            other => bail!("unknown core {other:?}; expected fixture or real"),
        }
    }
}

/// An IP network from `trusted_proxies`, like `172.16.0.0/12` or a bare address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr {
    addr: IpAddr,
    prefix: u8,
}

impl Cidr {
    /// True if `ip` lies inside the network. Address families must match.
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr, ip) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                prefix_eq(&net.octets(), &ip.octets(), self.prefix)
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                prefix_eq(&net.octets(), &ip.octets(), self.prefix)
            }
            _ => false,
        }
    }
}

fn prefix_eq(a: &[u8], b: &[u8], prefix: u8) -> bool {
    let full = usize::from(prefix / 8);
    let rest = prefix % 8;
    if a[..full] != b[..full] {
        return false;
    }
    if rest == 0 {
        return true;
    }
    let mask = 0xFFu8 << (8 - rest);
    a[full] & mask == b[full] & mask
}

impl FromStr for Cidr {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> anyhow::Result<Self> {
        let (ip, prefix) = match s.split_once('/') {
            Some((ip, p)) => (ip, Some(p)),
            None => (s, None),
        };
        let addr: IpAddr = ip
            .parse()
            .with_context(|| format!("bad address in {s:?}"))?;
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(p) => p
                .parse::<u8>()
                .with_context(|| format!("bad prefix in {s:?}"))?,
            None => max,
        };
        if prefix > max {
            bail!("prefix of {s:?} is longer than {max} bits");
        }
        Ok(Self { addr, prefix })
    }
}

impl<'de> Deserialize<'de> for Cidr {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// `[worker.autosave]`: autosave ring policy handed to workers (game-host-mp 4.11).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Autosave {
    pub min_interval_s: u32,
    pub ring: u32,
    pub keep_every: u32,
}

impl Default for Autosave {
    fn default() -> Self {
        Self {
            min_interval_s: 60,
            ring: 10,
            keep_every: 10,
        }
    }
}

/// `[worker.fixture_test]`: test world parameters (GS-R6). Only valid with `test_hooks`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct FixtureTest {
    pub ai_step_delay_ms: u32,
    pub leak_fog: bool,
    /// Change the owner of one plot at the start of a turn (the table of owners for mods, IPC v9).
    pub flip_owner: Option<protocol::FlipOwner>,
}

/// `[worker]` table.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct WorkerConfig {
    /// Workers audit their own fog filtering (net-test turns it on).
    pub audit_fog: bool,
    pub autosave: Autosave,
    pub fixture_test: Option<FixtureTest>,
}

/// `[retention]`: how long the server keeps things on disk. Applies to `state_dir`; a pass
/// runs at start and every `interval_min` minutes.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Retention {
    pub enabled: bool,
    pub interval_min: u32,
    /// Saves: the newest this many games are never deleted.
    pub keep_games: u32,
    /// Saves: an older game is deleted once it has been finished for this many days. A game that is not
    /// finished (running, sleeping, faulted) is never deleted.
    pub finished_max_age_days: u32,
    /// `logs/`: the total of `game-*.jsonl*` and `worker-*.log*`, MiB. Oldest files go first.
    pub logs_max_mb: u64,
    /// A worker's stderr file turns over at this size, MiB; the file and its turned-over copies stay.
    pub worker_log_max_mb: u64,
    /// Recordings (`recordings/`): the newest this many games stay ...
    pub recordings_games: u32,
    /// ... within this many MiB in all (the cap wins).
    pub recordings_max_mb: u64,
}

impl Default for Retention {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_min: 60,
            keep_games: 10,
            finished_max_age_days: 30,
            logs_max_mb: 256,
            worker_log_max_mb: 16,
            recordings_games: 5,
            recordings_max_mb: 1024,
        }
    }
}

/// `[recording]`: the recording of every game. How many games and how many bytes it may take in all are
/// `[retention] recordings_games` and `recordings_max_mb`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Recording {
    /// Record every game (the decision of 2026-10-31: always on).
    pub enabled: bool,
    /// A full snapshot body is kept every this many turns; the other turns keep the patch from the previous one.
    pub full_every: u32,
    /// The newest this many turns keep every snapshot; older ones only the full snapshots (`full_every`).
    pub detail_turns: u32,
    /// One game's recording over this many MiB keeps no more snapshots; over twice this it stops.
    pub max_game_mb: u64,
    /// A client report keeps the recording of its game for this many days ...
    pub pin_days: u32,
    /// ... at most this many recordings at once (the oldest pin is let go).
    pub max_pins: u32,
    /// The recording stops writing (and says so in its manifest) when the disk under it has less than this many MiB
    /// free; 0 turns the guard off.
    pub min_free_mb: u64,
}

impl Default for Recording {
    fn default() -> Self {
        Self {
            enabled: true,
            full_every: 10,
            detail_turns: 20,
            max_game_mb: 300,
            pin_days: 14,
            max_pins: 3,
            min_free_mb: 512,
        }
    }
}

/// Whole gateway configuration. Defaults match the example in spec 4.2.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    pub listen: SocketAddr,
    /// Without TLS the gateway listens on loopback only; `true` lifts that (container profile).
    pub allow_plain_nonlocal: bool,
    /// Test hooks (GS-D18). Never in deployment profiles.
    pub test_hooks: bool,
    /// Open access: no shared password; the room code is the only protection. Players log in with a nick
    /// alone. The admin API is not affected (loopback and its own token). Default off.
    pub open_access: bool,
    /// A WebSocket password login of an unknown nick creates the account (bots and tests; `--dev` turns it on). Default
    /// off: accounts are made by `POST /api/auth/register`.
    pub auto_register: bool,
    /// `Secure` on the session cookie (default on). `false` / `--no-secure-cookie`: for a profile that is served over
    /// plain http on a LAN address, where browsers drop Secure cookies. Every https deploy keeps it.
    pub cookie_secure: bool,
    /// Admin API listener. Always loopback.
    pub admin_listen: SocketAddr,
    pub state_dir: PathBuf,
    pub web_root: Option<PathBuf>,
    /// COOP `same-origin` and COEP `require-corp` on the web files, CORP on the blobs (B3 render-mt): the page is
    /// `crossOriginIsolated` and loads the threaded build. Default on (the user, 2026-10-29: threads by default,
    /// the single-threaded build only when the client cannot run them); `false` / `--no-cross-origin-isolation`: off.
    pub cross_origin_isolation: bool,
    pub assets_root: Option<PathBuf>,
    /// Where the installed mods live (`<mods_dir>/<id>/mod.toml`). `None`: no mods are installed.
    pub mods_dir: Option<PathBuf>,
    pub core: CoreKind,
    pub gamedata_dir: Option<PathBuf>,
    pub max_games: u32,
    pub max_sessions: u32,
    pub min_free_mem_mb: u64,
    pub trusted_proxies: Vec<Cidr>,
    pub hang_s: u32,
    /// Minutes without a human before a game sleeps; 0 = off.
    pub hibernate_idle_min: u32,
    /// Minutes without a human online before a game (running or asleep) is closed: its seats are freed, its saves
    /// stay loadable from the lobby (the user, 2026-11-02); 0 = never.
    pub empty_close_min: u32,
    pub host_lost_s: u32,
    pub worker: WorkerConfig,
    pub retention: Retention,
    pub recording: Recording,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            listen: ([127, 0, 0, 1], 8080).into(),
            allow_plain_nonlocal: false,
            test_hooks: false,
            open_access: false,
            auto_register: false,
            cookie_secure: true,
            admin_listen: ([127, 0, 0, 1], 9090).into(),
            state_dir: PathBuf::from(if cfg!(windows) {
                "civ5-state"
            } else {
                "/var/lib/civ5"
            }),
            web_root: None,
            cross_origin_isolation: true,
            assets_root: None,
            mods_dir: None,
            core: CoreKind::platform_default(),
            gamedata_dir: None,
            max_games: 4,
            max_sessions: 64,
            min_free_mem_mb: 800,
            trusted_proxies: Vec::new(),
            hang_s: 30,
            hibernate_idle_min: 15,
            empty_close_min: 5,
            host_lost_s: 60,
            worker: WorkerConfig::default(),
            retention: Retention::default(),
            recording: Recording::default(),
        }
    }
}

impl Config {
    /// Parses a `server.toml` text.
    pub fn from_toml(text: &str) -> anyhow::Result<Self> {
        toml::from_str(text).context("invalid server config")
    }

    /// Checks the whole configuration for the current platform.
    pub fn validate(&self) -> anyhow::Result<()> {
        self.validate_for(cfg!(windows))
    }

    /// [`Config::validate`] with the platform given explicitly (tests).
    pub fn validate_for(&self, windows: bool) -> anyhow::Result<()> {
        check_listen(self.listen, self.allow_plain_nonlocal)?;
        if !self.admin_listen.ip().is_loopback() {
            bail!(
                "admin_listen {} is not loopback; the admin API is reachable on loopback only",
                self.admin_listen
            );
        }
        if self.core == CoreKind::Real && windows {
            bail!("core = real: the real brain runs only on Linux (use WSL); use core = fixture");
        }
        if self.worker.fixture_test.is_some() && !self.test_hooks {
            bail!("[worker.fixture_test] needs test_hooks = true");
        }
        for (name, v) in [
            ("max_games", self.max_games),
            ("max_sessions", self.max_sessions),
            ("hang_s", self.hang_s),
            ("retention.interval_min", self.retention.interval_min),
            ("recording.full_every", self.recording.full_every),
            ("host_lost_s", self.host_lost_s),
            (
                "worker.autosave.min_interval_s",
                self.worker.autosave.min_interval_s,
            ),
            ("worker.autosave.ring", self.worker.autosave.ring),
            (
                "worker.autosave.keep_every",
                self.worker.autosave.keep_every,
            ),
        ] {
            if v == 0 {
                bail!("{name} must be at least 1");
            }
        }
        Ok(())
    }

    /// Address for local probes (`healthcheck`): the listen address, with an unspecified IP
    /// replaced by loopback.
    pub fn probe_addr(&self) -> SocketAddr {
        let mut a = self.listen;
        if a.ip().is_unspecified() {
            a.set_ip(if a.is_ipv4() {
                IpAddr::from([127, 0, 0, 1])
            } else {
                IpAddr::from([0, 0, 0, 0, 0, 0, 0, 1])
            });
        }
        a
    }
}

/// The no-TLS rule (GS-D14): a non-loopback listener needs `allow_plain_nonlocal`.
pub fn check_listen(addr: SocketAddr, allow_plain_nonlocal: bool) -> anyhow::Result<()> {
    if addr.ip().is_loopback() || allow_plain_nonlocal {
        return Ok(());
    }
    bail!(
        "listen {addr} is not loopback and the gateway has no TLS; \
         bind 127.0.0.1 or set allow_plain_nonlocal = true (container behind a LAN-only port)"
    )
}

/// Password from the environment value (GS-D2). `--dev` uses [`DEV_PASSWORD`].
pub fn resolve_password(dev: bool, env_value: Option<&str>) -> anyhow::Result<String> {
    if dev {
        return Ok(DEV_PASSWORD.to_string());
    }
    let Some(p) = env_value else {
        bail!("{PASSWORD_ENV} is not set (the password comes from the environment only)");
    };
    if p.chars().count() < MIN_PASSWORD_CHARS {
        bail!("{PASSWORD_ENV} must be at least {MIN_PASSWORD_CHARS} characters");
    }
    Ok(p.to_string())
}

/// Subcommand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// No subcommand: the gateway.
    Gateway,
    /// `worker ...`: game worker; arguments are for game-host-mp, passed through.
    Worker { args: Vec<String> },
    /// `describe --json`.
    Describe { json: bool },
    /// `healthcheck`: exit 0 if `/healthz` answers 200.
    Healthcheck,
    /// `admin <GET|POST> <path>`.
    Admin { method: String, path: String },
}

/// Flag values that override the file. `None` = not given.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Overrides {
    pub listen: Option<SocketAddr>,
    pub allow_plain_nonlocal: Option<bool>,
    pub test_hooks: Option<bool>,
    pub open_access: Option<bool>,
    pub cookie_secure: Option<bool>,
    pub admin_listen: Option<SocketAddr>,
    /// `--admin-port`: only the port of `admin_listen`; 0 lets the system pick a free one.
    pub admin_port: Option<u16>,
    pub state_dir: Option<PathBuf>,
    pub web_root: Option<PathBuf>,
    pub cross_origin_isolation: Option<bool>,
    pub assets_root: Option<PathBuf>,
    pub core: Option<CoreKind>,
    pub gamedata_dir: Option<PathBuf>,
    pub max_games: Option<u32>,
}

/// Parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cli {
    pub command: Command,
    pub config_path: Option<PathBuf>,
    pub dev: bool,
    pub overrides: Overrides,
}

const FLAGS: &str = "--config, --dev, --listen, --allow-plain-nonlocal, --test-hooks, --open-access, --no-secure-cookie, \
     --admin-listen, --admin-port, --state-dir, --web-root, --cross-origin-isolation, --no-cross-origin-isolation, --assets-root, --core, --gamedata-dir, --max-games";

impl Cli {
    /// Parses arguments after the program name. The first argument not starting with `--` is
    /// the subcommand; flags may follow it (except for `worker`, which takes the rest as is).
    pub fn parse(args: impl IntoIterator<Item = String>) -> anyhow::Result<Self> {
        let mut args = args.into_iter();
        let mut cli = Cli {
            command: Command::Gateway,
            config_path: None,
            dev: false,
            overrides: Overrides::default(),
        };
        let mut json = false;
        let mut positional: Vec<String> = Vec::new();
        let mut sub: Option<String> = None;
        while let Some(arg) = args.next() {
            if !arg.starts_with("--") {
                if sub.is_none() {
                    if arg == "worker" {
                        cli.command = Command::Worker {
                            args: args.collect(),
                        };
                        return Ok(cli);
                    }
                    sub = Some(arg);
                } else {
                    positional.push(arg);
                }
                continue;
            }
            let mut value = |name: &str| {
                args.next()
                    .with_context(|| format!("expected a value after {name}"))
            };
            let o = &mut cli.overrides;
            match arg.as_str() {
                "--config" => cli.config_path = Some(PathBuf::from(value("--config")?)),
                "--dev" => cli.dev = true,
                "--json" => json = true,
                "--listen" => o.listen = Some(addr(&value("--listen")?, "--listen")?),
                "--allow-plain-nonlocal" => o.allow_plain_nonlocal = Some(true),
                "--test-hooks" => o.test_hooks = Some(true),
                "--open-access" => o.open_access = Some(true),
                "--no-secure-cookie" => o.cookie_secure = Some(false),
                "--admin-listen" => {
                    o.admin_listen = Some(addr(&value("--admin-listen")?, "--admin-listen")?);
                }
                "--admin-port" => {
                    o.admin_port =
                        Some(value("--admin-port")?.parse().context(
                            "--admin-port: expected a port number (0: pick a free one)",
                        )?);
                }
                "--state-dir" => o.state_dir = Some(PathBuf::from(value("--state-dir")?)),
                "--web-root" => o.web_root = Some(PathBuf::from(value("--web-root")?)),
                "--cross-origin-isolation" => o.cross_origin_isolation = Some(true),
                "--no-cross-origin-isolation" => o.cross_origin_isolation = Some(false),
                "--assets-root" => o.assets_root = Some(PathBuf::from(value("--assets-root")?)),
                "--core" => o.core = Some(value("--core")?.parse()?),
                "--gamedata-dir" => {
                    o.gamedata_dir = Some(PathBuf::from(value("--gamedata-dir")?));
                }
                "--max-games" => {
                    o.max_games = Some(
                        value("--max-games")?
                            .parse()
                            .context("--max-games: expected a number")?,
                    );
                }
                other => bail!("unknown argument {other}; allowed: {FLAGS}"),
            }
        }
        cli.command = match sub.as_deref() {
            None => Command::Gateway,
            Some("describe") => Command::Describe { json },
            Some("healthcheck") => Command::Healthcheck,
            Some("admin") => {
                let [method, path] = <[String; 2]>::try_from(positional)
                    .map_err(|_| anyhow::anyhow!("usage: admin <GET|POST> <path>"))?;
                let method = method.to_ascii_uppercase();
                if method != "GET" && method != "POST" {
                    bail!("admin: method must be GET or POST, got {method}");
                }
                if !path.starts_with('/') {
                    bail!("admin: path must start with /");
                }
                return Ok(Cli {
                    command: Command::Admin { method, path },
                    ..cli
                });
            }
            Some(other) => {
                bail!("unknown subcommand {other}; allowed: worker, describe, healthcheck, admin")
            }
        };
        if !positional.is_empty() {
            bail!("unexpected argument {}", positional[0]);
        }
        Ok(cli)
    }

    /// Loads and validates the configuration: file (if any), `--dev`, then flags.
    pub fn load_config(&self) -> anyhow::Result<Config> {
        let mut config = match &self.config_path {
            Some(path) => load_file(path)?,
            None => Config::default(),
        };
        if self.dev {
            config.state_dir = PathBuf::from(DEV_STATE_DIR);
            // `cargo xtask dev` and the bots log in without registering first.
            config.auto_register = true;
            // Plain http, perhaps reached by a LAN address: a Secure cookie would be dropped there.
            config.cookie_secure = false;
            if config.core == CoreKind::Real {
                config.core = CoreKind::platform_default();
            }
        }
        let o = &self.overrides;
        if let Some(v) = o.listen {
            config.listen = v;
        }
        if let Some(v) = o.allow_plain_nonlocal {
            config.allow_plain_nonlocal = v;
        }
        if let Some(v) = o.test_hooks {
            config.test_hooks = v;
        }
        if let Some(v) = o.open_access {
            config.open_access = v;
        }
        if let Some(v) = o.cookie_secure {
            config.cookie_secure = v;
        }
        if let Some(v) = o.admin_listen {
            config.admin_listen = v;
        }
        if let Some(v) = o.admin_port {
            config.admin_listen.set_port(v);
        }
        if let Some(v) = &o.state_dir {
            config.state_dir = v.clone();
        }
        if let Some(v) = &o.web_root {
            config.web_root = Some(v.clone());
        }
        if let Some(v) = o.cross_origin_isolation {
            config.cross_origin_isolation = v;
        }
        if let Some(v) = &o.assets_root {
            config.assets_root = Some(v.clone());
        }
        if let Some(v) = o.core {
            config.core = v;
        }
        if let Some(v) = &o.gamedata_dir {
            config.gamedata_dir = Some(v.clone());
        }
        if let Some(v) = o.max_games {
            config.max_games = v;
        }
        if self.dev && !config.listen.ip().is_loopback() {
            bail!("--dev listens on loopback only, got {}", config.listen);
        }
        config.validate()?;
        Ok(config)
    }
}

fn addr(s: &str, flag: &str) -> anyhow::Result<SocketAddr> {
    s.parse()
        .with_context(|| format!("{flag}: expected an address like 127.0.0.1:8080"))
}

fn load_file(path: &Path) -> anyhow::Result<Config> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read config {}", path.display()))?;
    Config::from_toml(&text).with_context(|| format!("in {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli(args: &[&str]) -> anyhow::Result<Cli> {
        Cli::parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn defaults_are_valid() {
        let c = Config::default();
        c.validate_for(false).unwrap();
    }

    #[test]
    fn parses_example_from_spec() {
        let c = Config::from_toml(
            r#"
listen = "0.0.0.0:8080"
allow_plain_nonlocal = true
admin_listen = "127.0.0.1:9090"
state_dir = "/var/lib/civ5"
web_root = "/srv/civ5/web"
core = "fixture"
max_games = 8
trusted_proxies = ["127.0.0.1", "172.16.0.0/12"]
[worker]
audit_fog = true
autosave = { min_interval_s = 30, ring = 5, keep_every = 10 }
[retention]
keep_games = 3
recordings_max_mb = 512
"#,
        )
        .unwrap();
        c.validate_for(true).unwrap();
        assert_eq!(c.max_games, 8);
        assert_eq!(c.worker.autosave.ring, 5);
        assert_eq!(c.retention.keep_games, 3);
        assert_eq!(c.retention.recordings_max_mb, 512);
        // The rest of the table keeps its defaults.
        assert_eq!(c.retention.finished_max_age_days, 30);
        assert_eq!(c.retention.worker_log_max_mb, 16);
        assert!(c.worker.audit_fog);
        assert!(c.trusted_proxies[1].contains("172.20.1.2".parse().unwrap()));
        assert!(!c.trusted_proxies[1].contains("172.32.0.1".parse().unwrap()));
        assert!(c.trusted_proxies[0].contains("127.0.0.1".parse().unwrap()));
        assert!(!c.trusted_proxies[0].contains("127.0.0.2".parse().unwrap()));
    }

    #[test]
    fn unknown_keys_and_bad_values_are_rejected() {
        assert!(Config::from_toml("lisen = \"127.0.0.1:1\"").is_err());
        assert!(Config::from_toml("core = \"fake\"").is_err());
        assert!(Config::from_toml("trusted_proxies = [\"10.0.0.0/33\"]").is_err());
        assert!(Config::from_toml("[worker]\nbogus = 1").is_err());
    }

    #[test]
    fn non_loopback_needs_explicit_permission() {
        let mut c = Config {
            core: CoreKind::Fixture,
            listen: "0.0.0.0:8080".parse().unwrap(),
            ..Config::default()
        };
        let err = c.validate_for(false).unwrap_err().to_string();
        assert!(err.contains("allow_plain_nonlocal"), "{err}");
        c.listen = "192.168.1.10:8480".parse().unwrap();
        assert!(c.validate_for(false).is_err());
        c.allow_plain_nonlocal = true;
        c.validate_for(false).unwrap();
        c.listen = "[::1]:8080".parse().unwrap();
        c.allow_plain_nonlocal = false;
        c.validate_for(false).unwrap();
    }

    #[test]
    fn admin_listener_must_be_loopback_even_when_plain_nonlocal() {
        let c = Config {
            core: CoreKind::Fixture,
            allow_plain_nonlocal: true,
            admin_listen: "0.0.0.0:9090".parse().unwrap(),
            ..Config::default()
        };
        assert!(c.validate_for(false).is_err());
    }

    #[test]
    fn admin_port_flag_replaces_only_the_port() {
        let c = cli(&["--admin-port", "0"]).unwrap().load_config().unwrap();
        assert_eq!(c.admin_listen, "127.0.0.1:0".parse().unwrap());
        assert!(cli(&["--admin-port", "x"]).is_err());
    }

    #[test]
    fn the_session_cookie_is_secure_unless_a_plain_http_profile_says_otherwise() {
        assert!(Config::default().cookie_secure);
        assert!(cli(&[]).unwrap().load_config().unwrap().cookie_secure);
        assert!(
            !cli(&["--no-secure-cookie"])
                .unwrap()
                .load_config()
                .unwrap()
                .cookie_secure
        );
        assert!(
            !Config::from_toml("cookie_secure = false")
                .unwrap()
                .cookie_secure
        );
        // `--dev` is plain http on a loopback or LAN address.
        assert!(
            !cli(&["--dev"])
                .unwrap()
                .load_config()
                .unwrap()
                .cookie_secure
        );
    }

    #[test]
    fn open_access_is_off_by_default_and_a_flag_turns_it_on() {
        assert!(!Config::default().open_access);
        assert!(!cli(&[]).unwrap().load_config().unwrap().open_access);
        let c = cli(&["--open-access"]).unwrap().load_config().unwrap();
        assert!(c.open_access);
        assert!(Config::from_toml("open_access = true").unwrap().open_access);
    }

    #[test]
    fn auto_register_is_off_except_for_dev() {
        assert!(!Config::default().auto_register);
        assert!(!cli(&[]).unwrap().load_config().unwrap().auto_register);
        assert!(
            cli(&["--dev"])
                .unwrap()
                .load_config()
                .unwrap()
                .auto_register
        );
        assert!(
            Config::from_toml("auto_register = true")
                .unwrap()
                .auto_register
        );
        // The deploy config keeps it off.
        let text = include_str!("../../../../deploy/server.toml");
        assert!(!Config::from_toml(text).unwrap().auto_register);
    }

    #[test]
    fn real_core_refused_on_windows() {
        let c = Config {
            core: CoreKind::Real,
            ..Config::default()
        };
        let err = c.validate_for(true).unwrap_err().to_string();
        assert!(err.contains("Linux"), "{err}");
        c.validate_for(false).unwrap();
    }

    #[test]
    fn fixture_test_needs_hooks() {
        let mut c = Config {
            core: CoreKind::Fixture,
            ..Config::default()
        };
        c.worker.fixture_test = Some(FixtureTest::default());
        assert!(c.validate_for(false).is_err());
        c.test_hooks = true;
        c.validate_for(false).unwrap();
    }

    #[test]
    fn zero_limits_rejected() {
        let c = Config {
            core: CoreKind::Fixture,
            max_games: 0,
            ..Config::default()
        };
        assert!(
            c.validate_for(false)
                .unwrap_err()
                .to_string()
                .contains("max_games")
        );
    }

    #[test]
    fn password_rules() {
        assert_eq!(resolve_password(true, None).unwrap(), "dev");
        assert!(resolve_password(false, None).is_err());
        assert!(resolve_password(false, Some("short")).is_err());
        assert!(resolve_password(false, Some("1234567")).is_err());
        assert_eq!(
            resolve_password(false, Some("12345678")).unwrap(),
            "12345678"
        );
        // Counted in characters, not bytes.
        assert!(resolve_password(false, Some("пароль")).is_err());
    }

    #[test]
    fn cli_subcommands() {
        assert_eq!(cli(&[]).unwrap().command, Command::Gateway);
        assert_eq!(cli(&["healthcheck"]).unwrap().command, Command::Healthcheck);
        assert_eq!(
            cli(&["describe", "--json"]).unwrap().command,
            Command::Describe { json: true }
        );
        assert_eq!(
            cli(&["admin", "get", "/admin/metrics"]).unwrap().command,
            Command::Admin {
                method: "GET".into(),
                path: "/admin/metrics".into()
            }
        );
        assert_eq!(
            cli(&["worker", "--anything", "x"]).unwrap().command,
            Command::Worker {
                args: vec!["--anything".into(), "x".into()]
            }
        );
        assert!(cli(&["admin", "PUT", "/x"]).is_err());
        assert!(cli(&["admin", "GET"]).is_err());
        assert!(cli(&["nope"]).is_err());
        assert!(cli(&["--bogus"]).is_err());
        assert!(cli(&["--listen"]).is_err());
    }

    #[test]
    fn dev_defaults_and_flag_overrides() {
        let c = cli(&["--dev"]).unwrap().load_config().unwrap();
        assert_eq!(c.state_dir, PathBuf::from(DEV_STATE_DIR));
        assert_eq!(c.core, CoreKind::platform_default());
        let c = cli(&[
            "--dev",
            "--state-dir",
            "x",
            "--core",
            "fixture",
            "--listen",
            "127.0.0.1:0",
        ])
        .unwrap()
        .load_config()
        .unwrap();
        assert_eq!(c.state_dir, PathBuf::from("x"));
        assert_eq!(c.listen.port(), 0);
        // --dev is loopback only.
        assert!(
            cli(&[
                "--dev",
                "--listen",
                "0.0.0.0:8080",
                "--allow-plain-nonlocal"
            ])
            .unwrap()
            .load_config()
            .is_err()
        );
    }

    #[test]
    fn the_deploy_files_carry_the_retention_defaults() {
        let text = include_str!("../../../../deploy/server.toml");
        let c = Config::from_toml(text).unwrap();
        assert_eq!(c.retention, Retention::default());
        assert_eq!(c.recording, Recording::default());
        // The user's rule (2026-11-02): a game without a human for 5 minutes is closed.
        assert_eq!(c.empty_close_min, 5);
        assert_eq!(Config::default().empty_close_min, 5);
        assert_eq!(
            (c.retention.keep_games, c.retention.finished_max_age_days),
            (10, 30)
        );
        assert_eq!(
            (c.retention.recordings_games, c.retention.recordings_max_mb),
            (5, 1024)
        );
    }

    #[test]
    fn flags_override_file() {
        let dir = std::env::temp_dir().join(format!("civ5-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("server.toml");
        std::fs::write(
            &file,
            "core = \"fixture\"\nmax_games = 2\nlisten = \"127.0.0.1:1111\"\n",
        )
        .unwrap();
        let path = file.to_str().unwrap();
        let c = cli(&["--config", path, "--max-games", "5"])
            .unwrap()
            .load_config()
            .unwrap();
        assert_eq!(c.max_games, 5);
        assert_eq!(c.listen.port(), 1111);
        assert!(
            cli(&["--config", "no/such/file.toml"])
                .unwrap()
                .load_config()
                .is_err()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn probe_addr_maps_unspecified_to_loopback() {
        let c = Config {
            listen: "0.0.0.0:8080".parse().unwrap(),
            ..Config::default()
        };
        assert_eq!(c.probe_addr(), "127.0.0.1:8080".parse().unwrap());
    }
}
