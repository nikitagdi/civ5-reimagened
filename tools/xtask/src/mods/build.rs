//! `cargo xtask mods build --mods <file> [--target server|client|web|desktop|all] [--release] [--single]`: checks the
//! list, generates `target/dist/` and builds it. The public build (`cargo build`, `cargo xtask build` without
//! `--mods`) is not involved.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::check;
use super::dist::{self, Mode};

/// The artifacts a `--target` asks for. `client` is the web module and the native client; `all` adds the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Server,
    Client,
    Web,
    Desktop,
    All,
}

impl Target {
    pub fn server(self) -> bool {
        matches!(self, Target::Server | Target::All)
    }
    pub fn web(self) -> bool {
        matches!(self, Target::Client | Target::Web | Target::All)
    }
    pub fn desktop(self) -> bool {
        matches!(self, Target::Client | Target::Desktop | Target::All)
    }
    fn mode(self) -> Mode {
        match self {
            Target::Server => Mode::Server,
            Target::All => Mode::All,
            _ => Mode::Client,
        }
    }
}

pub fn parse_target(args: &[String]) -> Result<Target> {
    let mut value = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if let Some(v) = a.strip_prefix("--target=") {
            value = Some(v.to_string());
        } else if a == "--target" {
            value = Some(
                it.next()
                    .cloned()
                    .context("`--target` needs server, client, web, desktop or all")?,
            );
        }
    }
    match value.as_deref() {
        None | Some("server") => Ok(Target::Server),
        Some("client") => Ok(Target::Client),
        Some("web") => Ok(Target::Web),
        Some("desktop") => Ok(Target::Desktop),
        Some("all") => Ok(Target::All),
        Some(other) => {
            bail!("unknown target `{other}`; use server, client, web, desktop or all")
        }
    }
}

/// The path of the server binary the build of `release` or debug produces.
pub fn server_binary(dist_dir: &Path, release: bool) -> PathBuf {
    let exe = if cfg!(windows) {
        "civ5-server.exe"
    } else {
        "civ5-server"
    };
    dist_dir
        .join("target")
        .join(if release { "release" } else { "debug" })
        .join(exe)
}

/// The native client the build produces (`target/dist/client/target/<profile>/app-desktop`).
pub fn desktop_binary(dist_dir: &Path, release: bool) -> PathBuf {
    let exe = if cfg!(windows) {
        "app-desktop.exe"
    } else {
        "app-desktop"
    };
    dist_dir
        .join("client/target")
        .join(if release { "release" } else { "debug" })
        .join(exe)
}

/// The web bundle: the page files and `pkg/` (and `pkg-mt/`) of `dist-client-web`.
pub fn web_bundle(dist_dir: &Path) -> PathBuf {
    dist_dir.join("web")
}

/// Checks the list, writes the lock, checks the graph (R1-R11) and generates `target/dist/` for `mode`.
pub fn assemble(root: &Path, list: &Path, mode: Mode) -> Result<(check::Prepared, dist::Dist)> {
    let work = super::work_dir(root)?;
    let prepared = check::prepare(list, &work)?;
    check::report(&prepared);
    check::write_lock_file(&prepared)?;
    // The rules first: a mod that breaks R11 does not get built.
    dist::graph_step(root, &prepared)?;
    let dist = dist::generate(root, &prepared, mode)?;
    Ok((prepared, dist))
}

/// `cargo build` of the dist server.
pub fn build_server(root: &Path, dist: &dist::Dist, release: bool) -> Result<PathBuf> {
    let mut cmd = crate::cargo();
    cmd.arg("build")
        .arg("--manifest-path")
        .arg(&dist.manifest)
        .args(["-p", "dist-server"]);
    if release {
        cmd.arg("--release");
    }
    crate::run(root, cmd).context("building target/dist")?;
    let bin = server_binary(&dist.dir, release);
    if !bin.is_file() {
        bail!("the build did not produce {}", bin.display());
    }
    Ok(bin)
}

/// The web bundle of the dist: the public pipeline (wasm-bindgen, glue patch, `apps/web/www`, wasm-opt in a release)
/// over `dist-client-web`. `threads` adds `pkg-mt/` as the public build does.
pub fn build_web(root: &Path, dist: &dist::Dist, release: bool, threads: bool) -> Result<PathBuf> {
    let bundle = web_bundle(&dist.dir);
    crate::build_dist_web(root, release, threads, &dist.dir.join("client"), &bundle)
        .context("building the web client of target/dist")?;
    Ok(bundle)
}

/// The UI v2 panels of the mods: one fragment package per mod with `[ui]` and `ui2/mods-ui.json`, into the web
/// bundle (the client looks a mod of its set up there by id and version). `CIV5_SKIP_UI2=1` writes the index
/// without them. The native client has no UI v2 and gets nothing.
//
// SIMPLIFIED(SIMP-270): mod UI fragments ship only with the client build (`mods-ui.json` in the web bundle); the game
// server does not hand them out and does not check that a client has the same fragment as the other players.
pub fn build_mod_panels(root: &Path, prepared: &check::Prepared, bundle: &Path) -> Result<PathBuf> {
    if super::ui::sections_count(prepared) == 0 || crate::ui::web::skipped() {
        return super::ui::build_fragments(prepared, bundle, None);
    }
    let mut convert = |job: &super::ui::FragmentJob| super::ui::run_converter(root, job);
    super::ui::build_fragments(prepared, bundle, Some(&mut convert))
}

/// `cargo build` of the native dist client.
pub fn build_desktop(root: &Path, dist: &dist::Dist, release: bool) -> Result<PathBuf> {
    let mut cmd = crate::cargo();
    cmd.arg("build")
        .arg("--manifest-path")
        .arg(dist.dir.join("client/Cargo.toml"))
        .args(["-p", "dist-desktop"]);
    if release {
        cmd.arg("--release");
    }
    crate::run(root, cmd).context("building the native client of target/dist")?;
    let bin = desktop_binary(&dist.dir, release);
    if !bin.is_file() {
        bail!("the build did not produce {}", bin.display());
    }
    Ok(bin)
}

pub fn cmd_build(root: &Path, args: &[String]) -> Result<()> {
    let list = super::mods_arg(args)?;
    let target = parse_target(args)?;
    let release = args.iter().any(|a| a == "--release");
    // Threads by default as in the public build (`pkg-mt` beside `pkg`); `--single` builds `pkg` only.
    let threads = !args.iter().any(|a| a == "--single");
    let (prepared, dist) = assemble(root, &list, target.mode())?;
    if target.server() {
        println!("built {}", build_server(root, &dist, release)?.display());
    }
    if target.web() {
        let bundle = build_web(root, &dist, release, threads)?;
        build_mod_panels(root, &prepared, &bundle)?;
        println!("built {}", bundle.display());
    }
    if target.desktop() {
        println!("built {}", build_desktop(root, &dist, release)?.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn target_defaults_to_server_and_rejects_the_rest() {
        assert_eq!(parse_target(&v(&[])).unwrap(), Target::Server);
        assert_eq!(
            parse_target(&v(&["--target", "server"])).unwrap(),
            Target::Server
        );
        assert_eq!(
            parse_target(&v(&["--target=client"])).unwrap(),
            Target::Client
        );
        assert_eq!(parse_target(&v(&["--target", "all"])).unwrap(), Target::All);
        assert!(parse_target(&v(&["--target", "x"])).is_err());
        assert!(parse_target(&v(&["--target"])).is_err());
    }

    #[test]
    fn targets_pick_their_artifacts() {
        assert!(Target::Server.server() && !Target::Server.web() && !Target::Server.desktop());
        assert!(!Target::Client.server() && Target::Client.web() && Target::Client.desktop());
        assert!(Target::Web.web() && !Target::Web.desktop());
        assert!(Target::Desktop.desktop() && !Target::Desktop.web());
        assert!(Target::All.server() && Target::All.web() && Target::All.desktop());
        assert_eq!(Target::Client.mode(), Mode::Client);
        assert_eq!(Target::All.mode(), Mode::All);
    }

    #[test]
    fn the_client_artifacts_live_below_the_dist() {
        let d = Path::new("/t/dist");
        assert!(desktop_binary(d, false).starts_with("/t/dist/client/target/debug"));
        assert_eq!(web_bundle(d), Path::new("/t/dist/web"));
    }

    #[test]
    fn the_binary_lives_below_the_dist_target() {
        let p = server_binary(Path::new("/t/dist"), true);
        assert!(p.starts_with("/t/dist/target/release"));
        assert!(server_binary(Path::new("/t/dist"), false).starts_with("/t/dist/target/debug"));
    }
}
