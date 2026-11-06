//! Native client entry (`app-desktop`). Args parsed by hand: no clap in the Tech Stack.

use anyhow::{Context, bail};
use client_app::autostart::RoomParams;
use client_app::render::{BackendPref, Preset};
use client_app::settings_store::{GraphicsOptions, split_gfx};
use client_app::test_port::TurnMode;
use client_app::{AppConfig, Services};
use tracing_subscriber::EnvFilter;

const DEFAULT_SERVER: &str = "ws://127.0.0.1:8080/ws";
const DEFAULT_NICK: &str = "desktop";
const USAGE: &str = "\
civ5 desktop client

USAGE: app-desktop [--server <ws-url>] [--nick <nick>] [--gfx <list>] [--preset low|medium|high] [--disable <names>] [--debug <views>] [--manifest <hash>] [--showcase] [--demo-skinned] [--autostart] [--size <size>] [--ai <n>] [--civ <civ>] [--seed <n>] [--mode hybrid|simultaneous] [--stats] [--hud] [--interim-ui]
       app-desktop --golden <scene dir> --out <dir> [--assets <dir>] [--capture <name>]... [--gfx <list>]

  --server <url>   server WebSocket (default ws://127.0.0.1:8080/ws)
  --nick <nick>    player nick (default desktop)
  --gfx <list>     comma list: auto — Vulkan/DX12/Metal, gl — OpenGL; and Key:value pairs of the
                   graphics settings on top of the preset, e.g. gl,ShadowLevel:0,FOWLevel:1
  --preset <name>  graphics preset: low, medium or high (default high)
  --disable <names> comma list of [DisableSystems] names to switch off, e.g. FOW,FluidFOW
  --debug <views>  comma list of debug views: shadow shows the shadow map in place of the picture
  --manifest <hash> asset manifest blob hash (default: ask the server)
  --showcase       orbit camera and the Warrior showcase instead of the game camera
  --demo-skinned   show the built-in synthetic skinned model (no assets needed), N cycles clips; implies --showcase
  --autostart      skip the lobby: after the login a game against the AI starts at once (the quick start script)
  --size <size>    with --autostart: world size, e.g. Duel or WORLDSIZE_SMALL (default Duel)
  --ai <n>         with --autostart: AI players, 1 to 20 (default 1)
  --civ <civ>      with --autostart: your civilization, e.g. CIVILIZATION_ROME (default random)
  --seed <n>       with --autostart: the game seed
  --mode <mode>    with --autostart: hybrid (default) or simultaneous
  --stats          log FPS, draw calls and triangles once per second, and a `perf` line (frame, CPU/GPU, memory)
  --hud            show the performance panel from the start (F3 toggles it)
  --interim-ui     draw the interim egui game panels (B1/B2) instead of the original UI (EUI), which is the default
  --golden <dir>   no window, no server: draw the captures of a `golden` scene folder (`mapbasis.wv3`, `world.wv3`,
                   `catalog.wv3`, `<capture>.golden`) and write the frames to --out (`<capture>-native.rgba`, `.mask.r8`, `.txt`)
  --out <dir>      with --golden: where the frames go
  --assets <dir>   with --golden: the assets as the server serves them (`manifest` and the blobs); default none
  --capture <name> with --golden: draw only this capture (repeatable); default every `*.golden` of the folder
  -h, --help       show this help

Log level: RUST_LOG env var (default info).";

#[derive(Debug, PartialEq)]
struct Args {
    server: String,
    nick: String,
    backend: BackendPref,
    manifest: Option<String>,
    showcase: bool,
    demo_skinned: bool,
    autostart: bool,
    room: RoomParams,
    stats: bool,
    original_ui: bool,
    hud: bool,
    graphics: GraphicsOptions,
    golden: Option<GoldenArgs>,
}

/// `--golden`: one run without a window.
#[derive(Debug, PartialEq, Default)]
struct GoldenArgs {
    scene: Option<String>,
    out: Option<String>,
    assets: Option<String>,
    captures: Vec<String>,
}

/// `None` means help was requested.
fn parse_args(args: impl IntoIterator<Item = String>) -> anyhow::Result<Option<Args>> {
    let mut out = Args {
        server: DEFAULT_SERVER.to_owned(),
        nick: DEFAULT_NICK.to_owned(),
        backend: BackendPref::Auto,
        manifest: None,
        showcase: false,
        demo_skinned: false,
        autostart: false,
        room: RoomParams::default(),
        stats: false,
        original_ui: true,
        hud: false,
        graphics: GraphicsOptions::default(),
        golden: None,
    };
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        let (key, inline) = match arg.split_once('=') {
            Some((k, v)) if k.starts_with("--") => (k.to_owned(), Some(v.to_owned())),
            _ => (arg.clone(), None),
        };
        let mut value = |name: &str| -> anyhow::Result<String> {
            match inline.clone() {
                Some(v) => Ok(v),
                None => it.next().with_context(|| format!("{name}: value required")),
            }
        };
        match key.as_str() {
            "-h" | "--help" => return Ok(None),
            "--server" => out.server = value("--server")?,
            "--nick" => out.nick = value("--nick")?,
            "--gfx" => {
                let gfx = split_gfx(&value("--gfx")?);
                if let Some(other) = gfx.unknown.first() {
                    bail!("--gfx: {other:?} is neither a backend (auto | gl) nor Key:value");
                }
                out.backend = gfx.backend;
                out.graphics.overrides = gfx.overrides;
            }
            "--preset" => {
                let name = value("--preset")?;
                if Preset::from_name(&name).is_none() {
                    bail!("--preset: {name:?} is not low, medium or high");
                }
                out.graphics.preset = Some(name);
            }
            "--disable" => out.graphics.disable = Some(value("--disable")?),
            "--debug" => out.graphics.debug = Some(value("--debug")?),
            "--manifest" => out.manifest = Some(value("--manifest")?),
            "--showcase" => out.showcase = true,
            "--demo-skinned" => out.demo_skinned = true,
            "--autostart" => out.autostart = true,
            "--size" => out.room.size = Some(value("--size")?),
            "--ai" => {
                let n = value("--ai")?;
                out.room.ai = match n.parse::<u8>() {
                    Ok(n @ 1..=20) => n,
                    _ => bail!("--ai: {n:?} is not a number from 1 to 20"),
                };
            }
            "--civ" => out.room.civ = Some(value("--civ")?),
            "--seed" => {
                let s = value("--seed")?;
                out.room.seed = Some(
                    s.parse()
                        .with_context(|| format!("--seed: {s:?} is not a number"))?,
                );
            }
            "--mode" => {
                out.room.mode = match value("--mode")?.as_str() {
                    "hybrid" => TurnMode::Hybrid,
                    "simultaneous" => TurnMode::Simultaneous,
                    other => bail!("--mode: {other:?} is not hybrid or simultaneous"),
                };
            }
            "--stats" => out.stats = true,
            "--original-ui" => out.original_ui = true,
            "--interim-ui" => out.original_ui = false,
            "--hud" => out.hud = true,
            "--golden" => out.golden.get_or_insert_default().scene = Some(value("--golden")?),
            "--out" => out.golden.get_or_insert_default().out = Some(value("--out")?),
            "--assets" => out.golden.get_or_insert_default().assets = Some(value("--assets")?),
            "--capture" => {
                let name = value("--capture")?;
                out.golden.get_or_insert_default().captures.push(name);
            }
            other => bail!("unknown argument {other:?}\n\n{USAGE}"),
        }
    }
    if let Some(g) = &out.golden {
        if g.scene.is_none() {
            bail!("--out, --assets and --capture need --golden <scene dir>");
        }
        if g.out.is_none() {
            bail!("--golden needs --out <dir>");
        }
    }
    if out.nick.trim().is_empty() {
        bail!("--nick: nick must not be empty");
    }
    // The original UI has the popups of the choices (research, policies, religion ...): they are the human's
    // and block END TURN; the interim UI has none, the core makes them (SIMP-002).
    out.room.manual_choices = out.original_ui;
    Ok(Some(out))
}

/// `--golden`: draws the captures of a scene folder with no window and no server and writes the frames.
fn run_golden(g: GoldenArgs, backend: BackendPref) -> anyhow::Result<()> {
    use client_app::golden::{NativeRun, run_native, write_outputs};
    let (Some(scene), Some(out)) = (g.scene, g.out) else {
        bail!("--golden needs <scene dir> and --out <dir>");
    };
    let run = NativeRun {
        scene_dir: scene.into(),
        assets_dir: g.assets.map(Into::into),
        captures: g.captures,
        backend,
    };
    // Native wgpu futures are ready: run in place.
    let spawner: client_app::Spawner = Box::new(platform_native::block_on);
    let frames = run_native(&run, &spawner).map_err(anyhow::Error::msg)?;
    for frame in &frames {
        write_outputs(std::path::Path::new(&out), "native", frame).map_err(anyhow::Error::msg)?;
        tracing::info!(capture = %frame.capture, w = frame.size.0, h = frame.size.1, "golden frame written");
    }
    Ok(())
}

/// Counts the heap by owner for the performance panel (`ram.*`).
#[global_allocator]
static ALLOC: client_app::render::perf::CountingAlloc = client_app::render::perf::CountingAlloc;

/// The whole client, with the hooks of the client halves of mods. `main.rs` passes none (the public client); the
/// generated dist package (`cargo xtask mods build --target client`) passes the ones of its mods.
pub fn main_with(hooks: client_app::mod_hooks::ModHooks) -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    // `CIV5_JOBS=N` (B3 render-mt): N workers of the pool of `jobs`, so that a golden run shows the
    // frames built on threads are the bytes of the frames built on one. Without it: no worker, as before.
    if let Some(n) = std::env::var("CIV5_JOBS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
    {
        jobs::start_native_workers(n);
        tracing::info!(workers = n, "jobs pool started");
    }

    let Some(args) = parse_args(std::env::args().skip(1))? else {
        // Help bypasses tracing.
        tracing::info!("\n{USAGE}");
        return Ok(());
    };
    if let Some(g) = args.golden {
        return run_golden(g, args.backend);
    }
    tracing::info!(server = %args.server, nick = %args.nick, gfx = ?args.backend, "app-desktop");

    let config = AppConfig {
        server_url: args.server,
        nick_given: true,
        page_url: None,
        nick: args.nick,
        backend: args.backend,
        build: env!("CARGO_PKG_VERSION").to_owned(),
        manifest: args.manifest,
        showcase: args.showcase,
        demo_skinned: args.demo_skinned,
        autostart: args.autostart,
        room_params: args.room,
        stats: args.stats,
        hud: args.hud,
        graphics: args.graphics,
        golden: None,
        original_ui: args.original_ui,
        original_ui_m1: false,
        ui_volume: 0.5,
        touch_device: false,
    };
    let transport: Box<dyn client_app::Transport> = Box::new(platform_native::WsTransport);
    let services = Services {
        transport,
        http: Box::new(platform_native::StdHttp),
        clock: Box::new(platform_native::NativeClock::new()),
        tasks: Box::new(platform_native::ThreadPool::with_default_size()),
        // Native wgpu futures are ready: run in place.
        spawner: Box::new(platform_native::block_on),
        surface_provider: None,
        window_attributes: None,
        // Stats go to the log only; `window.__civ5stats` is web-only.
        stats_sink: None,
        perf_sink: None,
        report_sink: None,
        milestone_sink: None,
        // The error screen's Reload resets the game state in place.
        reload: None,
        test_port: None,
        // The disk cache is a stub (`platform_native::StubBlobStore`): settings come from the
        // command line each time until it exists.
        settings_blobs: None,
        // No store on the desktop yet: the token lives in memory.
        kv_store: None,
        // The graphics choices of this computer, in a file (`civ5.gfx.*`; the account does not follow them).
        device_store: Some(Box::new(platform_native::FileStore::open(
            platform_native::FileStore::default_path(),
        ))),
        // The app turns the network from its event loop (`about_to_wait`).
        net_driver: None,
        group_stats_sink: None,
        // SIMPLIFIED(SIMP-uiau-4): no sound device output on the desktop yet.
        audio: None,
        // The native client draws the sign-in, lobby and room with egui.
        ui_bridge: None,
        pointer_lock_probe: None,
    };
    // No mods: `run`, which also takes the debug hook of the environment; with mods: the hooks of the build.
    if hooks.is_empty() {
        client_app::run(config, services)
    } else {
        client_app::run_with(config, services, hooks)
    }
    .context("client exited with an error")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(v: &[&str]) -> anyhow::Result<Option<Args>> {
        parse_args(v.iter().map(|s| s.to_string()))
    }

    #[test]
    fn defaults() {
        let a = parse(&[]).unwrap().unwrap();
        assert_eq!(a.server, DEFAULT_SERVER);
        assert_eq!(a.nick, DEFAULT_NICK);
        assert_eq!(a.backend, BackendPref::Auto);
        assert_eq!(a.manifest, None);
        assert!(!a.demo_skinned);
        assert!(!a.showcase);
        assert!(!a.stats);
        assert!(a.original_ui);
        // The original UI leaves the choices to the human; the interim UI lets the core make them.
        assert!(a.room.manual_choices);
        assert!(
            !parse(&["--interim-ui"])
                .unwrap()
                .unwrap()
                .room
                .manual_choices
        );
    }

    #[test]
    fn both_forms() {
        let a = parse(&["--server", "ws://h:1/ws", "--nick=bob", "--gfx", "gl"])
            .unwrap()
            .unwrap();
        assert_eq!(a.server, "ws://h:1/ws");
        assert_eq!(a.nick, "bob");
        assert_eq!(a.backend, BackendPref::WebGl2);
        let a = parse(&["--manifest=ab"]).unwrap().unwrap();
        assert_eq!(a.manifest.as_deref(), Some("ab"));
        assert!(parse(&["--demo-skinned"]).unwrap().unwrap().demo_skinned);
        assert!(parse(&["--showcase"]).unwrap().unwrap().showcase);
        assert!(parse(&["--stats"]).unwrap().unwrap().stats);
        assert!(parse(&["--original-ui"]).unwrap().unwrap().original_ui);
        assert!(!parse(&["--interim-ui"]).unwrap().unwrap().original_ui);
        assert!(parse(&["--autostart"]).unwrap().unwrap().autostart);
    }

    #[test]
    fn graphics_options() {
        let a = parse(&[]).unwrap().unwrap();
        assert_eq!(a.graphics, GraphicsOptions::default());
        let a = parse(&["--preset", "low", "--disable=FOW,FluidFOW"])
            .unwrap()
            .unwrap();
        assert_eq!(a.graphics.preset.as_deref(), Some("low"));
        assert_eq!(a.graphics.disable.as_deref(), Some("FOW,FluidFOW"));
        assert!(parse(&["--preset=Medium"]).is_ok());
        assert!(parse(&["--preset", "ultra"]).is_err());
        assert!(parse(&["--preset"]).is_err());
        // `--gfx` takes the backend and the key overrides in one list.
        let a = parse(&["--gfx", "gl,ShadowLevel:0,FOWLevel:1"])
            .unwrap()
            .unwrap();
        assert_eq!(a.backend, BackendPref::WebGl2);
        assert_eq!(
            a.graphics.overrides.as_deref(),
            Some("ShadowLevel:0,FOWLevel:1")
        );
        let a = parse(&["--gfx", "ShadowLevel:0"]).unwrap().unwrap();
        assert_eq!(a.backend, BackendPref::Auto);
        assert!(parse(&["--gfx", "vulkan"]).is_err());
    }

    #[test]
    fn errors_and_help() {
        assert!(parse(&["--nick"]).is_err());
        assert!(parse(&["--bogus"]).is_err());
        assert!(parse(&["--nick", " "]).is_err());
        assert!(parse(&["--help"]).unwrap().is_none());
    }
}
