//! `civ5-server [--config server.toml] [flags]`: the gateway. Subcommands: `worker`,
//! `describe --json`, `healthcheck`, `admin <GET|POST> <path>`.

use std::process::ExitCode;
use std::sync::Arc;

use anyhow::Context;
use server::config::{Cli, Command, Config, PASSWORD_ENV, resolve_password};
use server::identity::Secret;
use server::probe;
use server::state::StateDir;
use server::supervisor;
use server::{ServerConfig, Shared};
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    let cli = match Cli::parse(std::env::args().skip(1)) {
        Ok(cli) => cli,
        Err(e) => return fail(&e),
    };
    // The worker owns its own threads and logging (game-host-mp); no tokio runtime here.
    if let Command::Worker { args } = &cli.command {
        return exit_code(server::worker::run(args));
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => return fail(&anyhow::Error::from(e).context("cannot start the async runtime")),
    };
    match runtime.block_on(run(cli)) {
        Ok(code) => exit_code(code),
        Err(e) => fail(&e),
    }
}

async fn run(cli: Cli) -> anyhow::Result<i32> {
    let config = cli.load_config()?;
    match cli.command {
        Command::Worker { .. } => unreachable!("handled in main"),
        Command::Gateway => {
            gateway(config, cli.dev).await?;
            Ok(0)
        }
        Command::Describe { json } => describe(&config, json).await,
        Command::Healthcheck => match probe::healthcheck(&config).await {
            Ok(true) => Ok(0),
            Ok(false) => {
                eprintln!("healthcheck: /healthz did not answer 200");
                Ok(1)
            }
            Err(e) => {
                eprintln!("healthcheck: {e:#}");
                Ok(1)
            }
        },
        Command::Admin { method, path } => {
            let r = probe::admin(&config, &method, &path).await?;
            println!("{}", r.body);
            Ok(if (200..300).contains(&r.status) { 0 } else { 1 })
        }
    }
}

/// `describe [--json]`: one `Describe` worker run; prints the lobby lists and the core build id (the deploy
/// preflight compares the build id with the running one, game-server 4.2, 4.15).
async fn describe(config: &Config, json: bool) -> anyhow::Result<i32> {
    let sup = supervisor::Supervisor::new(supervisor::SupervisorConfig::from_config(config)?);
    let described = match sup.describe().await {
        Ok(d) => d,
        Err(e) => {
            eprintln!("describe: {e}");
            return Ok(server::worker::EXIT_NOT_STARTED);
        }
    };
    let build_id = described.build_id.as_deref().unwrap_or("unknown");
    if json {
        let doc = serde_json::json!({ "build_id": build_id, "choices": described.choices });
        println!("{}", serde_json::to_string_pretty(&doc)?);
    } else {
        let c = &described.choices;
        println!("core build: {build_id}");
        println!(
            "civilizations {}, world sizes {}, map scripts {}, speeds {}, eras {}, difficulties {}, options {}",
            c.civs.len(),
            c.world_sizes.len(),
            c.map_scripts.len(),
            c.speeds.len(),
            c.eras.len(),
            c.handicaps.len(),
            c.options.len()
        );
    }
    Ok(0)
}

async fn gateway(config: Config, dev: bool) -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();
    // Startup order: config (done) -> password -> state_dir lock -> accounts ->
    // listeners. The password comes from the environment only (`--dev`: the fixed dev one). It is the
    // registration code of new accounts, not a login password.
    let env_password = std::env::var(PASSWORD_ENV).ok();
    // Open access: registration is open to anyone, no code; a password in the environment is ignored.
    let password = if config.open_access {
        None
    } else {
        Some(resolve_password(dev, env_password.as_deref())?)
    };
    let state = StateDir::open(&config.state_dir)?;
    // The game data of the real core must be the data the saves were made with.
    if config.core == server::config::CoreKind::Real {
        let dir = config
            .gamedata_dir
            .as_deref()
            .context("core = real needs gamedata_dir")?;
        server::state::meta::verify_gamedata(dir)?;
    }
    let admin_token = state.ensure_admin_token()?;
    if config.web_root.is_none() {
        tracing::warn!("web_root not set: the web client will not be served");
    }
    if config.open_access {
        tracing::warn!(
            "open_access: anyone can register an account; the room code is the only protection"
        );
    }
    if dev {
        tracing::warn!(
            "--dev: the registration code is the public dev one, a password login creates the account; loopback only"
        );
    }
    let mut server_config = ServerConfig::from_config(&config);
    server_config.open_access = config.open_access;
    server_config.password = password.map(Secret::new);
    server_config.admin_token = Some(Secret::new(admin_token));
    server_config.supervisor = Some(supervisor::SupervisorConfig::from_config(&config)?);
    let shared = Shared::new(server_config)?;
    // The `Describe` pass fills the lobby lists and the catalog; /healthz stays 503 until it succeeds
    // (GS-D17). A failed pass leaves the lobby closed, the gateway still serves /healthz and the admin API.
    let describing = match shared.supervisor() {
        Some(sup) => sup.publish_describe(&shared).await,
        None => Err(anyhow::anyhow!("no worker configured")),
    };
    let described_ok = describing.is_ok();
    match describing {
        Ok(described) => tracing::info!(
            core_build = described.build_id.as_deref().unwrap_or("unknown"),
            civs = described.choices.civs.len(),
            "Describe pass done, the lobby is open"
        ),
        Err(e) => tracing::error!("the Describe pass failed, the lobby stays closed: {e:#}"),
    }

    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("failed to bind {}", config.listen))?;
    let admin_listener = tokio::net::TcpListener::bind(config.admin_listen)
        .await
        .with_context(|| format!("failed to bind admin API {}", config.admin_listen))?;
    tracing::info!(
        addr = %listener.local_addr()?,
        admin = %admin_listener.local_addr()?,
        state_dir = %state.root().display(),
        core = ?config.core,
        protocol = protocol::PROTOCOL_VERSION,
        "civ5-server started"
    );
    // The games of the last run come back; without the pick lists none can start.
    if described_ok {
        server::game::restore::restore_all(&shared)?;
    } else {
        tracing::error!("games of the last run are not brought back: the Describe pass failed");
    }
    // Old finished games, their logs and old recordings go (retention); a pass at start.
    server::state::retention::spawn_loop(Arc::clone(&shared));
    // Stdout line for scripts that start the gateway with `--admin-port 0` (playtest, xtask).
    println!(
        "civ5-server: listening {} admin {}",
        listener.local_addr()?,
        admin_listener.local_addr()?
    );
    // SIGTERM and Ctrl+C start the drain of spec 4.12; so does `POST /admin/drain?exit=1`.
    let serving = Arc::clone(&shared);
    let admin_shared = Arc::clone(&shared);
    tokio::select! {
        res = server::serve(listener, serving) => {
            res.context("server stopped with an error")?;
        }
        res = server::serve_admin(admin_listener, admin_shared) => {
            res.context("admin API stopped with an error")?;
        }
        () = stop_signal() => {
            tracing::info!("stop signal received: draining");
            shared.drain(true).await;
        }
        () = shared.exit.notified() => {
            // The answer of the admin request that asked for it is on its way out.
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }
    }
    // `state` (and its lock) lives to here.
    drop(state);
    Ok(())
}

/// Ctrl+C, or SIGTERM on Unix.
async fn stop_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn fail(e: &anyhow::Error) -> ExitCode {
    eprintln!("civ5-server: {e:#}");
    ExitCode::from(2)
}

fn exit_code(code: i32) -> ExitCode {
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}
