//! `cargo xtask dist` and `cargo xtask deploy`.
//!
//! `deploy --target <file>` reads a deploy target supplied by the user (a TOML file; a bare name is
//! `deploy/targets/<name>.toml`, which git ignores; `deploy/targets/example.toml.template` documents the fields).
//! No target is built in. It builds a plan of nine steps first (`plan`), then prints it (`--dry-run`) or
//! runs it. Nothing here talks to a host while planning. Secrets never reach argv or the printed plan: the
//! registration code travels in the environment of `docker compose` and on stdin of `ssh`.

use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::mods::dist::Mode;

use crate::{
    GAMECORE_ENV, build_web_threads, cargo, cmd_gamecore, copy_dir, run, sh_quote, target_dir,
    windows_to_wsl_path,
};

const LINUX_TARGET: &str = "x86_64-unknown-linux-gnu";
/// Placeholder domain of the template: a target that still names it is not filled in.
const PLACEHOLDER_DOMAIN: &str = "example.com";
/// Compose override used when the target names none.
const DEFAULT_COMPOSE: &str = "deploy/compose.public.yaml";
/// Image tags kept on the host (step 9).
const KEEP_TAGS: usize = 3;
const HEALTH_TIMEOUT: Duration = Duration::from_secs(120);

// ---------------------------------------------------------------------------------------------------
// dist

/// Short git id of HEAD; a dirty tree gets `-dirty<unix seconds>` so that tags never collide.
fn build_id(root: &Path) -> Result<String> {
    let out = Command::new("git")
        .args(["rev-parse", "--short=9", "HEAD"])
        .current_dir(root)
        .output()
        .context("git rev-parse")?;
    if !out.status.success() {
        bail!("git rev-parse failed: not a git checkout?");
    }
    let short = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Ok(if tree_is_dirty(root)? {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        format!("{short}-dirty{now}")
    } else {
        short
    })
}

fn tree_is_dirty(root: &Path) -> Result<bool> {
    let out = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=no"])
        .current_dir(root)
        .output()
        .context("git status")?;
    Ok(!out.stdout.iter().all(u8::is_ascii_whitespace))
}

/// `target/dist/civ5-<build>/` stays the same for a clean tree; a dirty one is stamped, so a plan made
/// on a dirty tree names a dist the build will stamp afresh.
fn dist_dir(root: &Path, build: &str) -> Result<PathBuf> {
    Ok(target_dir(root)?.join("dist").join(format!("civ5-{build}")))
}

/// The mods a dist is built with (`--mods <file> [--mod-packs <dir>]`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DistMods {
    /// The operator's mod list (`mods.local.toml`).
    pub list: PathBuf,
    /// `<dir>/<id>.pack` copied over the pack of a mod (an assets-out directory).
    pub packs: Option<PathBuf>,
}

/// Where the image keeps the installed mods (`mods_dir` of the server config of a dist with mods).
const IMAGE_MODS_DIR: &str = "/srv/civ5/mods";

/// `cargo xtask dist [--skip-gamecore] [--pgo] [--mods <file> [--mod-packs <dir>]]`: `target/dist/civ5-<build>/` with
/// the Linux release server, the release web bundle (with `/netcheck/`), the container configs and the Dockerfile
/// (the image build context). On Windows the server is built inside WSL (the gamecore is a Linux library). With
/// `--mods` the server and the web client are the ones of the mod dist (`cargo xtask mods build`), and the mods are
/// installed into `mods/` of the dist (the image's `/srv/civ5/mods`).
pub fn cmd_dist(root: &Path, args: &[String]) -> Result<PathBuf> {
    let mut skip_gamecore = false;
    let mut pgo = false;
    let mut list = None;
    let mut packs = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let (name, inline) = match a.split_once('=') {
            Some((n, v)) => (n, Some(v.to_string())),
            None => (a.as_str(), None),
        };
        let mut value = || -> Result<PathBuf> {
            Ok(PathBuf::from(match &inline {
                Some(v) => v.clone(),
                None => it
                    .next()
                    .cloned()
                    .with_context(|| format!("dist: {name} requires a value"))?,
            }))
        };
        match name {
            "--skip-gamecore" => skip_gamecore = true,
            "--pgo" => pgo = true,
            "--mods" => list = Some(value()?),
            "--mod-packs" => packs = Some(value()?),
            _ => bail!(
                "dist: unknown argument {a:?} (allowed: --skip-gamecore, --pgo, --mods <file>, --mod-packs <dir>)"
            ),
        }
    }
    let mods = dist_mods_opt(list, packs)?;
    let build = build_id(root)?;
    let out = dist_dir(root, &build)?;
    build_dist_into(root, &out, &build, skip_gamecore, pgo, mods.as_ref())?;
    Ok(out)
}

fn dist_mods_opt(list: Option<PathBuf>, packs: Option<PathBuf>) -> Result<Option<DistMods>> {
    match (list, packs) {
        (Some(list), packs) => Ok(Some(DistMods { list, packs })),
        (None, Some(_)) => bail!("--mod-packs needs --mods <file>"),
        (None, None) => Ok(None),
    }
}

/// The server config of the dist: `deploy/server.toml`, with `mods_dir` first when the dist has mods (a key after the
/// first table would land in that table).
fn dist_server_toml(common: &str, mods: bool) -> String {
    if mods {
        format!(
            "# The mods of this dist (`cargo xtask dist --mods`), baked into the image.\nmods_dir = \"{IMAGE_MODS_DIR}\"\n{common}"
        )
    } else {
        common.to_string()
    }
}

/// `pgo`: the Linux server core is built profile-guided (`cargo xtask gamecore --pgo`); the build id is the same.
fn build_dist_into(
    root: &Path,
    out: &Path,
    build: &str,
    skip_gamecore: bool,
    pgo: bool,
    mods: Option<&DistMods>,
) -> Result<()> {
    let target = target_dir(root)?;
    // Linux gamecore: either given by --gamecore-lib, or built (WSL on Windows) into target/gamecore/linux.
    let given = GAMECORE_ENV
        .get()
        .into_iter()
        .flatten()
        .find(|(v, _)| *v == "GAMECORE_LIB_DIR_x86_64")
        .map(|(_, p)| p.clone());
    let linux_lib = match given {
        Some(p) => p,
        None => {
            let p = target.join("gamecore/linux");
            if !skip_gamecore {
                let mut gc_args: Vec<String> = vec!["--target".into(), "linux".into()];
                if pgo {
                    gc_args.push("--pgo".into());
                }
                cmd_gamecore(root, &gc_args)?;
            }
            if !p.join("libgamecore.a").is_file() {
                bail!(
                    "dist: {} has no libgamecore.a (run `cargo xtask gamecore --target linux` or pass --gamecore-lib)",
                    p.display()
                );
            }
            p
        }
    };
    // With mods: the mod dist (checked, locked, graph rules R1-R11), and its server package for WSL.
    let mod_dist = match mods {
        Some(m) => {
            let (prepared, dist) = crate::mods::build::assemble(root, &m.list, Mode::All)?;
            let server_dist = if cfg!(windows) {
                crate::mods::dist::generate_wsl(root, &prepared)?
            } else {
                crate::mods::dist::generate(root, &prepared, Mode::Server)?
            };
            Some((prepared, dist, server_dist))
        }
        None => None,
    };
    let server = match &mod_dist {
        Some((_, _, sd)) => {
            build_linux_server_in(root, &sd.dir.join("target"), &linux_lib, Some(&sd.manifest))?
        }
        None => build_linux_server(root, &target, &linux_lib)?,
    };
    // The web bundle needs the wasm gamecore archives (GAMECORE_LIB_DIR_wasm32, _threads): --gamecore-lib, the
    // process env, or built here into target/gamecore/wasm and wasm-mt. Without them the replica links the stub,
    // which reads no snapshot of the real server (the 2026-10-30 deploy: the original UI never came up).
    let wasm_given = GAMECORE_ENV
        .get()
        .is_some_and(|e| e.iter().any(|(v, _)| *v == "GAMECORE_LIB_DIR_wasm32"))
        || std::env::var_os("GAMECORE_LIB_DIR_wasm32").is_some();
    if !wasm_given {
        let mut env = vec![("GAMECORE_LIB_DIR_x86_64", linux_lib.clone())];
        for (var, sub) in [
            ("GAMECORE_LIB_DIR_wasm32", "wasm"),
            ("GAMECORE_LIB_DIR_wasm32_threads", "wasm-mt"),
        ] {
            let p = target.join("gamecore").join(sub);
            if !skip_gamecore {
                cmd_gamecore(root, &["--target".into(), sub.into()])?;
            }
            if !p.join("libgamecore.a").is_file() {
                bail!(
                    "dist: {} has no libgamecore.a (run `cargo xtask gamecore --target {sub}` or pass --gamecore-lib)",
                    p.display()
                );
            }
            env.push((var, p));
        }
        if GAMECORE_ENV.set(env).is_err() {
            bail!("dist: the gamecore env was already set without the wasm archives");
        }
    }
    // The threaded build (`pkg-mt`) is the main one, the single-threaded `pkg` the fallback of a client that
    // cannot run threads (`bootstrap.js`); the user, 2026-10-29: threads by default.
    let web = match &mod_dist {
        Some((_, dist, _)) => crate::mods::build::build_web(root, dist, true, true)?,
        None => build_web_threads(root, true)?,
    };

    if out.exists() {
        std::fs::remove_dir_all(out).with_context(|| format!("clearing {}", out.display()))?;
    }
    std::fs::create_dir_all(out)?;
    std::fs::copy(&server, out.join("civ5-server"))
        .with_context(|| format!("copying {}", server.display()))?;
    copy_dir(&web, &out.join("web"))?;
    let deploy = root.join("deploy");
    copy_dir(&deploy.join("web/netcheck"), &out.join("web/netcheck"))?;
    std::fs::copy(deploy.join("Dockerfile"), out.join("Dockerfile"))
        .context("copying deploy/Dockerfile")?;
    let common = std::fs::read_to_string(deploy.join("server.toml"))
        .context("reading deploy/server.toml")?;
    std::fs::write(
        out.join("server.toml"),
        dist_server_toml(&common, mod_dist.is_some()),
    )?;
    // `mods/` is always there (the Dockerfile copies it); empty without --mods.
    let mods_out = out.join("mods");
    match (&mod_dist, mods) {
        (Some((prepared, _, _)), Some(m)) => {
            crate::mods::dev::install_mods(prepared, &mods_out, m.packs.as_deref())?
        }
        _ => std::fs::create_dir_all(&mods_out)?,
    }
    std::fs::write(
        out.join("dist.json"),
        format!("{{\"build\":\"{build}\",\"target\":\"{LINUX_TARGET}\"}}\n"),
    )?;
    println!("dist: {}", out.display());
    Ok(())
}

/// `cargo build --release -p server --target x86_64-unknown-linux-gnu`; the path of the binary.
fn build_linux_server(root: &Path, target: &Path, linux_lib: &Path) -> Result<PathBuf> {
    let build_target = if cfg!(windows) {
        target.join("wsl")
    } else {
        target.to_path_buf()
    };
    build_linux_server_in(root, &build_target, linux_lib, None)
}

/// The Linux release server built into the cargo target directory `build_target`: the public package `server`, or
/// with `manifest` the package `dist-server` of a mod dist (on Windows the dist's WSL variant, Linux paths inside).
fn build_linux_server_in(
    root: &Path,
    build_target: &Path,
    linux_lib: &Path,
    manifest: Option<&Path>,
) -> Result<PathBuf> {
    let package = if manifest.is_some() {
        "dist-server"
    } else {
        "server"
    };
    std::fs::create_dir_all(build_target)?;
    if cfg!(windows) {
        let to_wsl = |p: &Path| windows_to_wsl_path(&p.to_string_lossy());
        let lib = std::fs::canonicalize(linux_lib)?;
        let manifest_arg = match manifest {
            Some(m) => format!(
                " --manifest-path {}",
                sh_quote(&to_wsl(&std::fs::canonicalize(m)?)?)
            ),
            None => String::new(),
        };
        let script = format!(
            "export CARGO_TARGET_DIR={} && export GAMECORE_LIB_DIR_x86_64={} && cd {} && cargo build --release{manifest_arg} -p {package} --bin civ5-server --target {LINUX_TARGET}",
            sh_quote(&to_wsl(&std::fs::canonicalize(build_target)?)?),
            sh_quote(&to_wsl(&lib)?),
            sh_quote(&to_wsl(&std::fs::canonicalize(root)?)?),
        );
        let mut c = Command::new("wsl");
        c.args(["-d", &crate::wsl_distro(), "--", "bash", "-lc"])
            .arg(script);
        run(root, c)?;
    } else {
        let mut c = cargo();
        c.arg("build").arg("--release");
        if let Some(m) = manifest {
            c.arg("--manifest-path").arg(m);
        }
        c.args([
            "-p",
            package,
            "--bin",
            "civ5-server",
            "--target",
            LINUX_TARGET,
        ]);
        c.env("GAMECORE_LIB_DIR_x86_64", linux_lib)
            .env("CARGO_TARGET_DIR", build_target);
        run(root, c)?;
    }
    Ok(build_target.join(LINUX_TARGET).join("release/civ5-server"))
}

// ---------------------------------------------------------------------------------------------------
// deploy: profile and options

/// A deploy target (see `deploy/targets/example.toml.template`).
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct Profile {
    /// docker context of the host ("default" for the local one).
    pub docker_context: String,
    /// `user@host` for the asset and game data sync.
    pub ssh: String,
    /// Data directory on the host (assets, gamedata, civ5.env).
    pub dir: String,
    /// The public name of the server (https://<domain>), or an IP address with `--tls internal`.
    pub domain: String,
    /// `host` (Caddy on the host), `docker` (Caddy in a container) or `none` (your own proxy; no Caddy step).
    pub caddy_mode: String,
    /// Directory of the included Caddy files (caddy_mode host/docker).
    pub caddy_dir: String,
    /// Command that reloads Caddy (caddy_mode host/docker).
    pub reload_cmd: String,
    /// Compose override file next to `deploy/compose.yaml` (default `deploy/compose.public.yaml`).
    pub compose: String,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Opts {
    pub target: String,
    pub dry_run: bool,
    pub rollback: bool,
    pub allow_core_change: bool,
    pub skip_dist: bool,
    pub skip_smoke: bool,
    /// `--pgo`: the Linux server core is built profile-guided (same build id, same saves).
    pub pgo: bool,
    pub tls_internal: bool,
    pub assets: Option<PathBuf>,
    pub gamedata: Option<PathBuf>,
    /// `--mods <file>`: the dist is built with these mods (see [`cmd_dist`]).
    pub mods: Option<PathBuf>,
    /// `--mod-packs <dir>`: packs copied over the mods' own (needs `--mods`).
    pub mod_packs: Option<PathBuf>,
}

fn parse_opts(args: &[String]) -> Result<Opts> {
    let mut o = Opts::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let (name, inline) = match a.split_once('=') {
            Some((n, v)) => (n, Some(v.to_string())),
            None => (a.as_str(), None),
        };
        let value = |it: &mut std::slice::Iter<String>| -> Result<String> {
            match &inline {
                Some(v) => Ok(v.clone()),
                None => it
                    .next()
                    .cloned()
                    .with_context(|| format!("deploy: {name} requires a value")),
            }
        };
        match name {
            "--target" => o.target = value(&mut it)?,
            "--assets" => o.assets = Some(PathBuf::from(value(&mut it)?)),
            "--gamedata" => o.gamedata = Some(PathBuf::from(value(&mut it)?)),
            "--tls" => {
                let v = value(&mut it)?;
                if v != "internal" {
                    bail!("deploy: --tls {v:?}: only `internal` is supported");
                }
                o.tls_internal = true;
            }
            "--dry-run" => o.dry_run = true,
            "--rollback" => o.rollback = true,
            "--allow-core-change" => o.allow_core_change = true,
            "--skip-dist" => o.skip_dist = true,
            "--skip-smoke" => o.skip_smoke = true,
            "--pgo" => o.pgo = true,
            "--mods" => o.mods = Some(PathBuf::from(value(&mut it)?)),
            "--mod-packs" => o.mod_packs = Some(PathBuf::from(value(&mut it)?)),
            _ => bail!(
                "deploy: unknown argument {a:?} (allowed: --target <file.toml|name>, --dry-run, --rollback, --allow-core-change, --assets <dir>, --gamedata <dir>, --tls internal, --skip-dist, --skip-smoke, --pgo, --mods <file>, --mod-packs <dir>)"
            ),
        }
    }
    if o.mod_packs.is_some() && o.mods.is_none() {
        bail!("deploy: --mod-packs needs --mods <file>");
    }
    if o.target.trim().is_empty() {
        bail!(
            "deploy: --target is required: a target file (copy deploy/targets/example.toml.template to deploy/targets/<name>.toml and fill it in)"
        );
    }
    Ok(o)
}

/// The file of `--target`: a path as given, or `deploy/targets/<name>.toml` for a bare name.
fn target_path(root: &Path, target: &str) -> PathBuf {
    let p = Path::new(target);
    if p.extension().is_some_and(|e| e == "toml") || p.components().count() > 1 {
        p.to_path_buf()
    } else {
        root.join("deploy/targets").join(format!("{target}.toml"))
    }
}

/// The name of a target in messages and on the host: the file stem.
fn target_name(target: &str) -> String {
    Path::new(target)
        .file_stem()
        .map_or_else(|| target.to_string(), |s| s.to_string_lossy().into_owned())
}

/// Reads the target file and checks it (step 1).
fn load_profile(root: &Path, target: &str) -> Result<Profile> {
    let path = target_path(root, target);
    let text = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "reading {} (copy deploy/targets/example.toml.template there and fill it in)",
            path.display()
        )
    })?;
    let p: Profile =
        toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    validate_profile(&target_name(target), &p)?;
    Ok(p)
}

fn validate_profile(name: &str, p: &Profile) -> Result<()> {
    let mut empty: Vec<&str> = Vec::new();
    let mut need = |k: &'static str, v: &str| {
        if v.trim().is_empty() || v.contains(PLACEHOLDER_DOMAIN) {
            empty.push(k);
        }
    };
    need("docker_context", &p.docker_context);
    need("ssh", &p.ssh);
    need("dir", &p.dir);
    need("domain", &p.domain);
    match p.caddy_mode.as_str() {
        "host" | "docker" => {
            need("caddy_dir", &p.caddy_dir);
            need("reload_cmd", &p.reload_cmd);
        }
        "none" => {}
        "" => empty.push("caddy_mode"),
        other => {
            bail!("target {name}: caddy_mode must be `host`, `docker` or `none`, not {other:?}")
        }
    }
    if !empty.is_empty() {
        bail!(
            "target {name} is not filled in (empty or still the template's placeholder: {})",
            empty.join(", ")
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------
// deploy: commands

/// One child process. The environment is never printed (it may hold the password).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Cmd {
    argv: Vec<String>,
    env: Vec<(String, String)>,
}

impl Cmd {
    fn new<S: AsRef<str>>(argv: &[S]) -> Self {
        Self {
            argv: argv.iter().map(|s| s.as_ref().to_string()).collect(),
            env: Vec::new(),
        }
    }

    fn env(mut self, k: &str, v: impl Into<String>) -> Self {
        self.env.push((k.into(), v.into()));
        self
    }

    fn show(&self) -> String {
        let env: String = self
            .env
            .iter()
            .map(|(k, _)| format!("{k}=<set> "))
            .collect();
        format!("{env}{}", self.argv.join(" "))
    }

    fn command(&self) -> Command {
        let mut c = Command::new(&self.argv[0]);
        c.args(&self.argv[1..]).envs(self.env.iter().cloned());
        c
    }

    fn run(&self, root: &Path) -> Result<()> {
        println!("$ {}", self.show());
        let st = self
            .command()
            .current_dir(root)
            .status()
            .with_context(|| format!("starting {}", self.argv[0]))?;
        if !st.success() {
            bail!("command failed ({st}): {}", self.show());
        }
        Ok(())
    }

    /// stdout on success, `None` when the command exits non-zero (stderr passes through quietly).
    fn output(&self, root: &Path) -> Result<Option<String>> {
        let out = self
            .command()
            .current_dir(root)
            .stderr(Stdio::null())
            .output()
            .with_context(|| format!("starting {}", self.argv[0]))?;
        Ok(out
            .status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).into_owned()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    Note(String),
    Dist,
    DistPgo,
    Run(Cmd),
    /// `docker compose up`: the password goes into its environment (the container reads `CIV5_PASSWORD`).
    Up(Cmd),
    GcLocal {
        dir: PathBuf,
    },
    EnsureEnv,
    SyncAssets {
        local: PathBuf,
    },
    SyncGamedata {
        local: PathBuf,
    },
    Preflight,
    WaitHealthy,
    Caddy,
    Smoke(Cmd),
    Summary,
    RollbackPick,
}

#[derive(Debug)]
struct Step {
    n: u8,
    title: String,
    actions: Vec<Action>,
}

struct Ctx<'a> {
    root: &'a Path,
    name: String,
    profile: Profile,
    opts: &'a Opts,
    build: String,
    dist: PathBuf,
}

impl Ctx<'_> {
    fn docker(&self) -> Vec<String> {
        vec![
            "docker".into(),
            "--context".into(),
            self.profile.docker_context.clone(),
        ]
    }

    fn docker_cmd(&self, rest: &[&str]) -> Cmd {
        let mut v = self.docker();
        v.extend(rest.iter().map(|s| s.to_string()));
        Cmd::new(&v)
    }

    fn base_url(&self) -> String {
        format!("https://{}", self.profile.domain)
    }

    fn config_file(&self) -> String {
        "/etc/civ5/server.toml".into()
    }

    fn has_caddy_step(&self) -> bool {
        self.profile.caddy_mode != "none"
    }

    fn compose(&self, build: &str, tail: &[&str]) -> Cmd {
        let mut v = self.docker();
        v.extend(["compose", "-p", "civ5", "-f", "deploy/compose.yaml", "-f"].map(String::from));
        v.push(if self.profile.compose.is_empty() {
            DEFAULT_COMPOSE.to_string()
        } else {
            self.profile.compose.clone()
        });
        v.extend(tail.iter().map(|s| s.to_string()));
        Cmd::new(&v)
            .env("CIV5_BUILD", build)
            .env("CIV5_DIST", self.dist.to_string_lossy())
            .env("CIV5_DIR", self.profile.dir.as_str())
    }

    fn ssh(&self, script: &str) -> Cmd {
        Cmd::new(&[
            "ssh",
            "-o",
            "BatchMode=yes",
            self.profile.ssh.as_str(),
            script,
        ])
    }
}

fn default_dir(root: &Path, sub: &str) -> Option<PathBuf> {
    let p = root.join("..").join("assets-out").join(sub);
    p.is_dir().then_some(p)
}

/// The nine steps of 4.15 (or the rollback). Pure: reads nothing from a host.
fn plan(cx: &Ctx) -> Vec<Step> {
    let mut steps = Vec::new();
    let p = &cx.profile;
    let o = cx.opts;
    steps.push(Step {
        n: 1,
        title: format!(
            "target {}: context {}, host {}, {}",
            cx.name,
            p.docker_context,
            p.ssh,
            cx.base_url()
        ),
        actions: vec![Action::Note(
            "warn when the working tree is not clean (the build id is then stamped -dirty)".into(),
        )],
    });
    if o.rollback {
        steps.push(Step {
            n: 5,
            title: "rollback: start the previous image tag (the last 3 are kept)".into(),
            actions: vec![Action::RollbackPick],
        });
        steps.push(Step {
            n: 6,
            title: format!(
                "wait for {}/healthz 200 and restoring == 0 (up to 120 s)",
                cx.base_url()
            ),
            actions: vec![Action::WaitHealthy],
        });
        steps.push(Step {
            n: 9,
            title: "result".into(),
            actions: vec![Action::Summary],
        });
        return steps;
    }
    steps.push(Step {
        n: 2,
        title: format!(
            "build target/dist/civ5-{}/ (gamecore linux, server release, web release)",
            cx.build
        ),
        actions: {
            let mut a = vec![if o.skip_dist {
                Action::Note("--skip-dist: the existing dist directory is used".into())
            } else if o.pgo {
                Action::DistPgo
            } else {
                Action::Dist
            }];
            if let (Some(list), false) = (&o.mods, o.skip_dist) {
                a.push(Action::Note(format!(
                    "with the mods of {}{}: the mod dist server and web client, the mods in the image at {IMAGE_MODS_DIR}",
                    list.display(),
                    o.mod_packs
                        .as_ref()
                        .map(|p| format!(" (packs from {})", p.display()))
                        .unwrap_or_default()
                )));
            }
            a
        },
    });
    let mut s3 = vec![Action::EnsureEnv];
    let assets = o.assets.clone().or_else(|| default_dir(cx.root, "b2"));
    let gamedata = o
        .gamedata
        .clone()
        .or_else(|| default_dir(cx.root, "gamedata"));
    match assets {
        Some(local) => {
            s3.push(Action::GcLocal { dir: local.clone() });
            s3.push(Action::SyncAssets { local });
        }
        None => s3.push(Action::Note("no assets: pass --assets <dir> (a `civ5-assets build --set b2` output); the host assets stay as they are".into())),
    }
    match gamedata {
        Some(local) => s3.push(Action::SyncGamedata { local }),
        None => s3.push(Action::Note("no game data: pass --gamedata <dir> (`civ5-assets export-gamedata` output); the host game data stays as it is".into())),
    }
    steps.push(Step {
        n: 3,
        title: format!(
            "assets and game data to {}:{} (changed parts only), civ5.env",
            p.ssh, p.dir
        ),
        actions: s3,
    });
    let tag = format!("civ5-server:{}", cx.build);
    steps.push(Step {
        n: 4,
        title: "preflight of the core build id".into(),
        actions: vec![
            Action::Run(cx.docker_cmd(&["build", "-t", &tag, &cx.dist.to_string_lossy()])),
            Action::Preflight,
        ],
    });
    steps.push(Step {
        n: 5,
        title: format!("docker compose up on context {}", p.docker_context),
        actions: vec![Action::Up(cx.compose(&cx.build, &["up", "-d", "--build"]))],
    });
    steps.push(Step {
        n: 6,
        title: format!(
            "wait for {}/healthz 200 and restoring == 0 (up to 120 s)",
            cx.base_url()
        ),
        actions: vec![Action::WaitHealthy],
    });
    if cx.has_caddy_step() {
        steps.push(Step {
            n: 7,
            title: format!(
                "Caddy block on {} (replace only when changed, validate, reload)",
                p.caddy_dir
            ),
            actions: vec![Action::Caddy],
        });
    }
    // Through `cargo xtask nettest`: it builds the bot the scenario drives (`--bot`).
    let smoke = Cmd::new(&[
        "cargo",
        "xtask",
        "nettest",
        "--attach",
        &cx.base_url(),
        "--admin-via",
        &format!("docker:{}:civ5", p.docker_context),
        "--scenario",
        "deploy-smoke",
    ]);
    steps.push(Step {
        n: 8,
        title: "smoke check: deploy-smoke".into(),
        actions: vec![if o.skip_smoke {
            Action::Note("--skip-smoke".into())
        } else {
            Action::Smoke(smoke)
        }],
    });
    steps.push(Step {
        n: 9,
        title: "result, image id, rollback command, old tags".into(),
        actions: vec![Action::Summary],
    });
    steps
}

fn describe(a: &Action) -> String {
    match a {
        Action::Note(s) => s.clone(),
        Action::Dist => "cargo xtask dist".into(),
        Action::DistPgo => "cargo xtask dist --pgo".into(),
        Action::Run(c) | Action::Smoke(c) => c.show(),
        Action::Up(c) => format!("CIV5_PASSWORD=<set> {}", c.show()),
        Action::GcLocal { dir } => format!("civ5-assets gc --out {} (keeps the last 2 manifests)", dir.display()),
        Action::EnsureEnv => "first deploy: write the registration code (CIV5_PASSWORD, else asked on stdin) into <dir>/civ5.env (0600) when absent; never printed".into(),
        Action::SyncAssets { local } => format!("upload blobs missing on the host from {}, then manifest and manifests.log last, then remove the blobs the local store no longer has", local.display()),
        Action::SyncGamedata { local } => format!("copy {} to <dir>/gamedata when gamedata.lock differs from the host's", local.display()),
        Action::Preflight => "describe --json of the new image vs core_build of the running container (docker exec ... admin GET /admin/metrics); with games running and a different build: stop unless --allow-core-change".into(),
        Action::WaitHealthy => "poll /healthz".into(),
        Action::Caddy => "write the Caddy block, caddy validate, reload_cmd".into(),
        Action::Summary => "print the result; keep the last 3 image tags".into(),
        Action::RollbackPick => "newest image tag other than the running one -> compose up -d --no-build".into(),
    }
}

fn print_plan(cx: &Ctx, steps: &[Step]) {
    println!(
        "deploy plan for {} (build {}){}",
        cx.name,
        cx.build,
        if cx.opts.rollback { ", rollback" } else { "" }
    );
    for s in steps {
        println!("step {}. {}", s.n, s.title);
        for a in &s.actions {
            println!("     - {}", describe(a));
        }
    }
}

pub fn cmd_deploy(root: &Path, args: &[String]) -> Result<()> {
    let opts = parse_opts(args)?;
    let profile = load_profile(root, &opts.target)?;
    let name = target_name(&opts.target);
    let build = build_id(root)?;
    let dist = dist_dir(root, &build)?;
    let cx = Ctx {
        root,
        name,
        profile,
        opts: &opts,
        build,
        dist,
    };
    let steps = plan(&cx);
    if opts.dry_run {
        print_plan(&cx, &steps);
        return Ok(());
    }
    if !cx.opts.rollback && tree_is_dirty(root)? {
        println!(
            "warning: the working tree is not clean; this deploy is stamped {}",
            cx.build
        );
    }
    let mut state = State::default();
    for s in &steps {
        println!("== step {}. {}", s.n, s.title);
        for a in &s.actions {
            exec(&cx, a, &mut state).with_context(|| format!("step {}", s.n))?;
        }
    }
    Ok(())
}

#[derive(Default)]
struct State {
    /// Tag chosen by `RollbackPick`.
    tag: Option<String>,
    /// `SyncGamedata` swapped `<dir>/gamedata` for a new directory: the running container still has the old one
    /// bind-mounted (a mount follows the directory, not the path), so `Up` recreates it.
    gamedata_swapped: bool,
}

/// `compose up` that recreates the container even when the image is the same (after a game data swap).
fn force_recreate(mut c: Cmd) -> Cmd {
    c.argv.push("--force-recreate".into());
    c
}

fn exec(cx: &Ctx, a: &Action, st: &mut State) -> Result<()> {
    let root = cx.root;
    match a {
        Action::Note(s) => println!("note: {s}"),
        Action::Dist | Action::DistPgo => {
            let mods = dist_mods_opt(cx.opts.mods.clone(), cx.opts.mod_packs.clone())?;
            build_dist_into(
                root,
                &cx.dist,
                &cx.build,
                false,
                matches!(a, Action::DistPgo),
                mods.as_ref(),
            )?
        }
        Action::Run(c) => c.run(root)?,
        Action::Up(c) => {
            let c = if st.gamedata_swapped {
                println!("note: the game data changed: the container is recreated");
                force_recreate(c.clone())
            } else {
                c.clone()
            };
            with_password(cx, c)?.run(root)?
        }
        Action::GcLocal { dir } => {
            let mut c = cargo();
            c.args([
                "run",
                "-p",
                "asset-pipeline",
                "--release",
                "--",
                "gc",
                "--out",
            ])
            .arg(dir);
            run(root, c)?;
        }
        Action::EnsureEnv => ensure_env(cx)?,
        Action::SyncAssets { local } => sync_assets(cx, local)?,
        Action::SyncGamedata { local } => st.gamedata_swapped = sync_gamedata(cx, local)?,
        Action::Preflight => preflight(cx)?,
        Action::WaitHealthy => wait_healthy(cx)?,
        Action::Caddy => caddy(cx)?,
        // The runner reads the registration code from NETTEST_PASSWORD (it is not stored locally).
        Action::Smoke(c) => c.clone().env("NETTEST_PASSWORD", password(cx)?).run(root)?,
        Action::RollbackPick => {
            let tag = pick_rollback_tag(cx)?;
            println!("rolling back to {tag}");
            let c = with_password(cx, cx.compose(&tag, &["up", "-d", "--no-build"]))?;
            c.run(root)?;
            st.tag = Some(tag);
        }
        Action::Summary => summary(cx, st)?,
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------
// deploy: password

/// The registration code of the server (`CIV5_PASSWORD`, else asked on stdin); never stored locally.
fn password(_cx: &Ctx) -> Result<String> {
    if let Ok(p) = std::env::var("CIV5_PASSWORD")
        && !p.is_empty()
    {
        return Ok(p);
    }
    eprintln!(
        "registration code of the server (CIV5_PASSWORD; it is not stored locally); type it and press Enter:"
    );
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let line = line.trim().to_string();
    if line.is_empty() {
        bail!("the deploy needs a registration code");
    }
    Ok(line)
}

fn with_password(cx: &Ctx, c: Cmd) -> Result<Cmd> {
    Ok(c.env("CIV5_PASSWORD", password(cx)?))
}

fn ensure_env(cx: &Ctx) -> Result<()> {
    let pw = password(cx)?;
    // civ5.env on the host: only when absent, so that a deploy never changes a code in use. The data dir is
    // created here (owned by the ssh user, so no sudo: keep it in the user's home or a dir the user owns).
    let env_path = format!("{}/civ5.env", cx.profile.dir);
    let script = ensure_env_script(&cx.profile.dir);
    let mut child = cx
        .ssh(&script)
        .command()
        .current_dir(cx.root)
        .stdin(Stdio::piped())
        .spawn()
        .context("ssh")?;
    child
        .stdin
        .take()
        .context("ssh stdin")?
        .write_all(format!("CIV5_PASSWORD={pw}\n").as_bytes())?;
    if !child.wait()?.success() {
        bail!(
            "could not create {} or write {env_path} on {} (is `dir` of the target writable by that user?)",
            cx.profile.dir,
            cx.profile.ssh
        );
    }
    Ok(())
}

/// Shell script for the host: create the data dir (and its `assets`, `gamedata`) and `civ5.env` (0600) from stdin
/// when the file is absent.
fn ensure_env_script(dir: &str) -> String {
    let d = sh_quote(dir);
    let p = sh_quote(&format!("{dir}/civ5.env"));
    format!(
        "umask 022 && mkdir -p {d} {a} {g} && (umask 077 && {{ [ -f {p} ] || cat > {p}; }}) && chmod 600 {p}",
        a = sh_quote(&format!("{dir}/assets")),
        g = sh_quote(&format!("{dir}/gamedata")),
    )
}

// ---------------------------------------------------------------------------------------------------
// deploy: sync

/// A 64-digit hash name: the only files of the asset folder that sync adds or removes.
fn is_blob_name(n: &str) -> bool {
    n.len() == 64 && n.bytes().all(|b| b.is_ascii_hexdigit())
}

fn blob_names(dir: &Path) -> Result<BTreeSet<String>> {
    let mut v = BTreeSet::new();
    for e in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let name = e?.file_name().to_string_lossy().into_owned();
        if is_blob_name(&name) {
            v.insert(name);
        }
    }
    Ok(v)
}

/// Streams `tar` of `names` (relative to `local`) into `ssh host script`.
fn tar_to_ssh(cx: &Ctx, local: &Path, names: &[String], script: &str) -> Result<()> {
    let mut tar = Command::new("tar")
        .arg("-C")
        .arg(local)
        .args(["-cf", "-", "-T", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .context("starting tar")?;
    let mut ssh = cx
        .ssh(script)
        .command()
        .stdin(tar.stdout.take().context("tar stdout")?)
        .spawn()
        .context("starting ssh")?;
    {
        let mut w = tar.stdin.take().context("tar stdin")?;
        for n in names {
            writeln!(w, "{n}")?;
        }
    }
    let t = tar.wait()?;
    let s = ssh.wait()?;
    if !t.success() || !s.success() {
        bail!("upload failed (tar {t}, ssh {s})");
    }
    Ok(())
}

fn remote_lines(cx: &Ctx, script: &str) -> Result<Vec<String>> {
    let out = cx
        .ssh(script)
        .output(cx.root)?
        .with_context(|| format!("ssh to {} failed", cx.profile.ssh))?;
    Ok(out
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

fn sync_assets(cx: &Ctx, local: &Path) -> Result<()> {
    let remote_dir = format!("{}/assets", cx.profile.dir);
    let q = sh_quote(&remote_dir);
    let have: BTreeSet<String> = remote_lines(cx, &format!("mkdir -p {q} && ls -1 {q}"))?
        .into_iter()
        .collect();
    let local_blobs = blob_names(local)?;
    let (upload, remove) = blob_diff(&local_blobs, &have);
    println!(
        "assets: {} to upload, {} on the host already, {} to remove",
        upload.len(),
        local_blobs.len() - upload.len(),
        remove.len()
    );
    let untar = format!("tar -xf - -C {q}");
    if !upload.is_empty() {
        tar_to_ssh(cx, local, &upload, &untar)?;
    }
    // The pointer goes last (4.15 step 3): a client never sees a manifest whose blobs are not there yet.
    let tail: Vec<String> = ["manifest", "manifests.log"]
        .iter()
        .filter(|f| local.join(f).is_file())
        .map(|s| s.to_string())
        .collect();
    if !tail.is_empty() {
        tar_to_ssh(cx, local, &tail, &untar)?;
    }
    if !remove.is_empty() {
        let mut child = cx
            .ssh(&format!("cd {q} && xargs -r rm -f --"))
            .command()
            .stdin(Stdio::piped())
            .spawn()
            .context("ssh")?;
        let mut w = child.stdin.take().context("ssh stdin")?;
        for n in &remove {
            writeln!(w, "{n}")?;
        }
        drop(w);
        if !child.wait()?.success() {
            bail!("removing old blobs failed");
        }
    }
    Ok(())
}

/// (to upload, to remove) for the local and the remote set of blob names.
fn blob_diff(local: &BTreeSet<String>, remote: &BTreeSet<String>) -> (Vec<String>, Vec<String>) {
    let up = local.difference(remote).cloned().collect();
    let rm = remote
        .iter()
        .filter(|n| is_blob_name(n) && !local.contains(*n))
        .cloned()
        .collect();
    (up, rm)
}

/// Whether `<dir>/gamedata` was replaced (false: the host's lock is the same).
fn sync_gamedata(cx: &Ctx, local: &Path) -> Result<bool> {
    let lock = std::fs::read_to_string(local.join("gamedata.lock")).with_context(|| {
        format!(
            "{} has no gamedata.lock (not an export-gamedata output)",
            local.display()
        )
    })?;
    let dir = &cx.profile.dir;
    let remote_lock = cx
        .ssh(&format!(
            "cat {} 2>/dev/null",
            sh_quote(&format!("{dir}/gamedata/gamedata.lock"))
        ))
        .output(cx.root)?
        .unwrap_or_default();
    if remote_lock == lock {
        println!("gamedata: unchanged (gamedata.lock is the same)");
        return Ok(false);
    }
    let new = sh_quote(&format!("{dir}/gamedata.new"));
    let cur = sh_quote(&format!("{dir}/gamedata"));
    let old = sh_quote(&format!("{dir}/gamedata.old"));
    let mut names = Vec::new();
    list_files(local, local, &mut names)?;
    println!("gamedata: {} files", names.len());
    tar_to_ssh(
        cx,
        local,
        &names,
        &format!("rm -rf {new} && mkdir -p {new} && tar -xf - -C {new}"),
    )?;
    let swap = format!(
        "rm -rf {old} && {{ [ ! -d {cur} ] || mv {cur} {old}; }} && mv {new} {cur} && rm -rf {old}"
    );
    cx.ssh(&swap).run(cx.root)?;
    Ok(true)
}

fn list_files(base: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        if e.file_type()?.is_dir() {
            list_files(base, &e.path(), out)?;
        } else {
            let rel = e
                .path()
                .strip_prefix(base)?
                .to_string_lossy()
                .replace('\\', "/");
            out.push(rel);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------------
// deploy: preflight, health, caddy, summary

/// `"key":"value"` or `"key":123` out of a JSON text, by search (the xtask has no JSON crate). The first match.
fn json_field(text: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\"");
    let at = text.find(&pat)? + pat.len();
    let rest = text[at..].trim_start().strip_prefix(':')?.trim_start();
    if let Some(s) = rest.strip_prefix('"') {
        return s.split('"').next().map(str::to_string);
    }
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-'))
        .unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

fn preflight(cx: &Ctx) -> Result<()> {
    let dir = &cx.profile.dir;
    let describe = cx
        .docker_cmd(&[
            "run", "--rm", "--user", "1000:1000", "--tmpfs", "/var/lib/civ5:uid=1000,gid=1000",
            "-v", &format!("{dir}/gamedata:/srv/civ5/gamedata:ro"),
            "-v", &format!("{dir}/assets:/srv/civ5/assets:ro"),
            &format!("civ5-server:{}", cx.build), "--config", &cx.config_file(), "describe", "--json",
        ])
        .output(cx.root)?
        .context("describe --json of the new image failed (are the game data on the host? run without --skip-... or sync them)")?;
    let new_core = json_field(&describe, "build_id").context("describe --json has no build_id")?;
    println!("new core build: {new_core}");
    let running = cx
        .docker_cmd(&[
            "exec",
            "civ5",
            "civ5-server",
            "--config",
            &cx.config_file(),
            "admin",
            "GET",
            "/admin/metrics",
        ])
        .output(cx.root)?;
    let Some(metrics) = running else {
        println!("no running container: first deploy, nothing to compare");
        return Ok(());
    };
    let old_core = json_field(&metrics, "core_build").unwrap_or_default();
    let games: u32 = json_field(&metrics, "running")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    if old_core == new_core {
        println!("core build unchanged ({new_core})");
        return Ok(());
    }
    if games > 0 && !cx.opts.allow_core_change {
        bail!(
            "the core build changes ({old_core} -> {new_core}) and {games} game(s) are running: their saves cannot be continued by the new core (GS-D12, SIMP-059). They stay on disk. List them with `civ5-server admin GET /admin/metrics` inside the container; continue only with --allow-core-change"
        );
    }
    println!("core build changes ({old_core} -> {new_core}); {games} running game(s); continuing");
    Ok(())
}

/// True for a `/healthz` body that says the gateway is up and has nothing left to restore.
fn healthy(body: &str, code: u16) -> bool {
    code == 200 && json_field(body, "restoring").as_deref() == Some("0")
}

/// `GET url` with curl: (status, body); status 0 when nothing answered (refused, no TLS certificate, timeout).
fn curl_get(url: &str) -> (u16, String) {
    let out = Command::new("curl")
        .args(["-sS", "-m", "5", "-w", "\n%{http_code}", url])
        .stderr(Stdio::null())
        .output();
    let Ok(o) = out else {
        return (0, String::new());
    };
    let text = String::from_utf8_lossy(&o.stdout).into_owned();
    match text.rsplit_once('\n') {
        Some((body, code)) => (code.trim().parse().unwrap_or(0), body.to_string()),
        None => (0, String::new()),
    }
}

/// Waits for the gateway behind the public URL.
fn wait_healthy(cx: &Ctx) -> Result<()> {
    let started = Instant::now();
    let url = format!("{}/healthz", cx.base_url());
    loop {
        let (code, body) = curl_get(&url);
        if healthy(&body, code) {
            println!("healthy: {url}");
            return Ok(());
        }
        if started.elapsed() > HEALTH_TIMEOUT {
            bail!(
                "{url} did not become healthy in {} s (docker --context {} logs civ5)",
                HEALTH_TIMEOUT.as_secs(),
                cx.profile.docker_context
            );
        }
        std::thread::sleep(Duration::from_secs(3));
    }
}

fn caddy(cx: &Ctx) -> Result<()> {
    let p = &cx.profile;
    let src = cx.root.join(if cx.opts.tls_internal {
        "deploy/public/Caddyfile.civ5.internal"
    } else {
        "deploy/public/Caddyfile.civ5"
    });
    let text = std::fs::read_to_string(&src)?;
    let text = if cx.opts.tls_internal {
        text.replace("203.0.113.5", &p.domain)
    } else {
        text.replace("civ5.example.com", &p.domain)
    };
    let path = format!("{}/civ5.caddy", p.caddy_dir);
    let q = sh_quote(&path);
    let old = cx
        .ssh(&format!("cat {q} 2>/dev/null"))
        .output(cx.root)?
        .unwrap_or_default();
    if old == text {
        println!("caddy: the block is unchanged, no reload");
        return Ok(());
    }
    let tmp = sh_quote(&format!("{path}.new"));
    let mut child = cx
        .ssh(&format!("cat > {tmp}"))
        .command()
        .current_dir(cx.root)
        .stdin(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .context("ssh stdin")?
        .write_all(text.as_bytes())?;
    if !child.wait()?.success() {
        bail!("could not write the Caddy block");
    }
    if p.caddy_mode == "host" {
        cx.ssh(&format!(
            "caddy validate --adapter caddyfile --config {tmp}"
        ))
        .run(cx.root)?;
    } else {
        println!("note: caddy_mode docker: validation happens at reload");
    }
    cx.ssh(&format!("mv {tmp} {q} && {}", p.reload_cmd))
        .run(cx.root)?;
    Ok(())
}

fn image_tags(cx: &Ctx) -> Result<Vec<String>> {
    let out = cx
        .docker_cmd(&["image", "ls", "civ5-server", "--format", "{{.Tag}}"])
        .output(cx.root)?
        .context("docker image ls failed")?;
    Ok(out
        .lines()
        .map(str::trim)
        .filter(|t| !t.is_empty() && *t != "<none>" && *t != "latest")
        .map(String::from)
        .collect())
}

fn running_tag(cx: &Ctx) -> Result<Option<String>> {
    let out = cx
        .docker_cmd(&["inspect", "civ5", "--format", "{{.Config.Image}}"])
        .output(cx.root)?;
    Ok(out.and_then(|s| s.trim().strip_prefix("civ5-server:").map(String::from)))
}

/// The newest tag (the list is newest first) that is not the running one.
fn rollback_choice(tags: &[String], running: Option<&str>) -> Option<String> {
    tags.iter().find(|t| Some(t.as_str()) != running).cloned()
}

fn pick_rollback_tag(cx: &Ctx) -> Result<String> {
    let running = running_tag(cx)?;
    rollback_choice(&image_tags(cx)?, running.as_deref())
        .context("no previous image tag on the host to roll back to")
}

/// Tags beyond the newest `KEEP_TAGS`, except the running one.
fn tags_to_prune(tags: &[String], running: Option<&str>) -> Vec<String> {
    tags.iter()
        .skip(KEEP_TAGS)
        .filter(|t| Some(t.as_str()) != running)
        .cloned()
        .collect()
}

fn summary(cx: &Ctx, st: &State) -> Result<()> {
    let running = running_tag(cx)?;
    let tags = image_tags(cx)?;
    let id = cx
        .docker_cmd(&["inspect", "civ5", "--format", "{{.Image}}"])
        .output(cx.root)?
        .unwrap_or_default();
    println!(
        "deployed: {} on {} ({})",
        running
            .as_deref()
            .or(st.tag.as_deref())
            .unwrap_or(&cx.build),
        cx.name,
        cx.base_url()
    );
    println!("image id: {}", id.trim());
    for t in tags_to_prune(&tags, running.as_deref()) {
        let _ = cx
            .docker_cmd(&["image", "rm", &format!("civ5-server:{t}")])
            .output(cx.root);
    }
    println!(
        "rollback: cargo xtask deploy --target {} --rollback",
        cx.opts.target
    );
    Ok(())
}

// ---------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn s(a: &[&str]) -> Vec<String> {
        a.iter().map(|x| x.to_string()).collect()
    }

    /// A filled-in target (made-up values).
    fn filled() -> Profile {
        Profile {
            docker_context: "civ5-host".into(),
            ssh: "deploy@civ5.test".into(),
            dir: "/srv/civ5".into(),
            domain: "civ5.test".into(),
            caddy_mode: "host".into(),
            caddy_dir: "/etc/caddy/conf.d".into(),
            reload_cmd: "sudo systemctl reload caddy".into(),
            compose: String::new(),
        }
    }

    fn cx<'a>(name: &str, profile: Profile, opts: &'a Opts) -> Ctx<'a> {
        Ctx {
            root: Path::new("."),
            name: name.into(),
            profile,
            opts,
            build: "abc123".into(),
            dist: PathBuf::from("target/dist/civ5-abc123"),
        }
    }

    #[test]
    fn the_template_refuses_until_it_is_filled_in() {
        let t: Profile = toml::from_str(include_str!(
            "../../../deploy/targets/example.toml.template"
        ))
        .unwrap();
        let e = validate_profile("example", &t).unwrap_err().to_string();
        assert!(e.contains("not filled in"), "{e}");
        validate_profile("mine", &filled()).unwrap();
        let mut bad = filled();
        bad.caddy_mode = "nginx".into();
        assert!(validate_profile("mine", &bad).is_err());
        // Your own proxy: no Caddy fields needed.
        let mut own = filled();
        own.caddy_mode = "none".into();
        own.caddy_dir.clear();
        own.reload_cmd.clear();
        validate_profile("mine", &own).unwrap();
    }

    #[test]
    fn a_target_is_a_file_or_a_name_under_deploy_targets() {
        let root = Path::new("/r");
        assert_eq!(
            target_path(root, "mine"),
            Path::new("/r/deploy/targets/mine.toml")
        );
        assert_eq!(
            target_path(root, "elsewhere/x.toml"),
            Path::new("elsewhere/x.toml")
        );
        assert_eq!(target_name("elsewhere/x.toml"), "x");
        assert_eq!(target_name("mine"), "mine");
    }

    #[test]
    fn the_server_needs_the_registration_code_and_keeps_secure_cookies() {
        let compose = include_str!("../../../deploy/compose.yaml");
        assert!(compose.contains("CIV5_PASSWORD"));
        assert!(!compose.contains("--open-access"));
        let common = include_str!("../../../deploy/server.toml");
        assert!(
            !common
                .lines()
                .any(|l| l.trim_start().starts_with("cookie_secure"))
        );
    }

    #[test]
    fn plan_has_the_nine_steps_and_caddy_unless_the_target_has_its_own_proxy() {
        let o = Opts {
            target: "mine".into(),
            ..Opts::default()
        };
        let steps = plan(&cx("mine", filled(), &o));
        assert_eq!(
            steps.iter().map(|s| s.n).collect::<Vec<_>>(),
            [1, 2, 3, 4, 5, 6, 7, 8, 9]
        );
        let mut own = filled();
        own.caddy_mode = "none".into();
        let steps = plan(&cx("mine", own, &o));
        assert_eq!(
            steps.iter().map(|s| s.n).collect::<Vec<_>>(),
            [1, 2, 3, 4, 5, 6, 8, 9]
        );
        let smoke = steps
            .iter()
            .flat_map(|s| &s.actions)
            .find_map(|a| match a {
                Action::Smoke(c) => Some(c),
                _ => None,
            })
            .expect("smoke");
        assert_eq!(
            smoke.argv.join(" "),
            "cargo xtask nettest --attach https://civ5.test --admin-via docker:civ5-host:civ5 --scenario deploy-smoke"
        );
    }

    #[test]
    fn host_script_creates_the_dir_and_a_private_env_file() {
        let s = ensure_env_script("/srv/civ5");
        assert!(
            s.contains("mkdir -p '/srv/civ5' '/srv/civ5/assets' '/srv/civ5/gamedata'"),
            "{s}"
        );
        assert!(
            s.contains("[ -f '/srv/civ5/civ5.env' ] || cat > '/srv/civ5/civ5.env'"),
            "{s}"
        );
        assert!(s.ends_with("chmod 600 '/srv/civ5/civ5.env'"), "{s}");
        assert!(!s.contains("sudo"));
    }

    #[test]
    fn options() {
        let o = parse_opts(&s(&["--target", "mine", "--dry-run", "--assets=a"])).unwrap();
        assert!(o.dry_run && !o.rollback && o.assets == Some(PathBuf::from("a")));
        assert!(parse_opts(&s(&["--dry-run"])).is_err());
        assert!(parse_opts(&s(&["--target", "mine", "--bogus"])).is_err());
        assert!(parse_opts(&s(&["--target", "mine", "--tls", "acme"])).is_err());
    }

    #[test]
    fn a_game_data_swap_recreates_the_container() {
        let up = Cmd::new(&["docker", "compose", "up", "-d", "--build"]);
        assert_eq!(
            force_recreate(up).argv,
            [
                "docker",
                "compose",
                "up",
                "-d",
                "--build",
                "--force-recreate"
            ]
        );
    }

    #[test]
    fn mods_go_into_the_dist_and_its_server_config() {
        let o = parse_opts(&s(&[
            "--target",
            "mine",
            "--mods",
            "m.toml",
            "--mod-packs=p",
        ]))
        .unwrap();
        assert_eq!(o.mods, Some(PathBuf::from("m.toml")));
        assert_eq!(o.mod_packs, Some(PathBuf::from("p")));
        assert!(parse_opts(&s(&["--target", "mine", "--mod-packs", "p"])).is_err());
        let shown: Vec<String> = plan(&cx("mine", filled(), &o))
            .iter()
            .flat_map(|st| st.actions.iter().map(describe))
            .collect();
        assert!(
            shown
                .iter()
                .any(|d| d.contains("m.toml") && d.contains(IMAGE_MODS_DIR))
        );

        // `mods_dir` is a top-level key even though the common config ends with tables.
        let common = "listen = \"0.0.0.0:8080\"\n[worker]\naudit_fog = false\n";
        let t: toml::Table = dist_server_toml(common, true).parse().unwrap();
        assert_eq!(t["mods_dir"].as_str(), Some(IMAGE_MODS_DIR));
        assert!(t["worker"].get("mods_dir").is_none());
        assert_eq!(dist_server_toml(common, false), common);
    }

    #[test]
    fn pgo_switch_selects_the_pgo_dist_step() {
        let o = parse_opts(&s(&["--target", "mine", "--pgo"])).unwrap();
        assert!(o.pgo);
        let steps = plan(&cx("mine", filled(), &o));
        let dist: Vec<_> = steps
            .iter()
            .flat_map(|s| s.actions.iter())
            .filter(|a| matches!(a, Action::Dist | Action::DistPgo))
            .collect();
        assert_eq!(dist, [&Action::DistPgo]);
        assert_eq!(describe(&Action::DistPgo), "cargo xtask dist --pgo");
        let plain = parse_opts(&s(&["--target", "mine"])).unwrap();
        assert!(!plain.pgo);
        assert!(
            plan(&cx("mine", filled(), &plain))
                .iter()
                .flat_map(|s| s.actions.iter())
                .any(|a| *a == Action::Dist)
        );
    }

    #[test]
    fn compose_uses_the_context_and_hides_the_password() {
        let o = Opts::default();
        let c = cx("mine", filled(), &o);
        let cmd = c.compose("abc123", &["up", "-d", "--build"]);
        assert_eq!(
            cmd.argv.join(" "),
            "docker --context civ5-host compose -p civ5 -f deploy/compose.yaml -f deploy/compose.public.yaml up -d --build"
        );
        assert!(
            cmd.env
                .iter()
                .any(|(k, v)| k == "CIV5_DIR" && v == "/srv/civ5")
        );
        let shown = with_password_for_test(cmd).show();
        assert!(shown.contains("CIV5_PASSWORD=<set>") && !shown.contains("hunter2"));
        let mut other = filled();
        other.compose = "deploy/compose.mine.yaml".into();
        let cmd = cx("mine", other, &o).compose("abc123", &["ps"]);
        assert!(
            cmd.argv
                .join(" ")
                .contains("-f deploy/compose.mine.yaml ps")
        );
    }

    fn with_password_for_test(c: Cmd) -> Cmd {
        c.env("CIV5_PASSWORD", "hunter2")
    }

    #[test]
    fn rollback_plan_skips_the_build() {
        let o = Opts {
            target: "mine".into(),
            rollback: true,
            ..Opts::default()
        };
        let steps = plan(&cx("mine", filled(), &o));
        assert_eq!(steps.iter().map(|s| s.n).collect::<Vec<_>>(), [1, 5, 6, 9]);
        let tags = s(&["c", "b", "a"]);
        assert_eq!(rollback_choice(&tags, Some("c")), Some("b".into()));
        assert_eq!(rollback_choice(&tags[..1], Some("c")), None);
        assert_eq!(
            tags_to_prune(&s(&["e", "d", "c", "b", "a"]), Some("a")),
            ["b"]
        );
    }

    #[test]
    fn blob_sync_diff() {
        let h = |c: char| c.to_string().repeat(64);
        let local: BTreeSet<String> = [h('a'), h('b')].into();
        let remote: BTreeSet<String> = [h('b'), h('c'), "manifest".into()].into();
        let (up, rm) = blob_diff(&local, &remote);
        assert_eq!(up, [h('a')]);
        assert_eq!(rm, [h('c')]);
        assert!(!is_blob_name("manifest") && is_blob_name(&h('f')));
    }

    #[test]
    fn json_fields_and_health() {
        let m = r#"{"schema":1, "core_build" : "x-1", "server":{"games":{"running": 2}}}"#;
        assert_eq!(json_field(m, "core_build").as_deref(), Some("x-1"));
        assert_eq!(json_field(m, "running").as_deref(), Some("2"));
        assert!(healthy(r#"{"status":"ok","restoring":0}"#, 200));
        assert!(!healthy(r#"{"restoring":1}"#, 200));
        assert!(!healthy(r#"{"restoring":0}"#, 503));
    }
}
