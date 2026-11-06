//! `cargo xtask mods build --mods <file> [--target server] [--release]`: checks the list, generates `target/dist/` and
//! builds it. The public build (`cargo build`, `cargo xtask build` without `--mods`) is not involved.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::check;
use super::dist::{self, Mode};

/// The artifacts a `--target` asks for. The client targets come with the client assembly (T41).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Server,
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
                    .context("`--target` needs server, client or all")?,
            );
        }
    }
    match value.as_deref() {
        None | Some("server") => Ok(Target::Server),
        Some("client" | "all") => {
            bail!(
                "`--target client|all`: the client assembly is not implemented yet; use `--target server`"
            )
        }
        Some(other) => bail!("unknown target `{other}`; use server, client or all"),
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

pub fn cmd_build(root: &Path, args: &[String]) -> Result<()> {
    let list = super::mods_arg(args)?;
    let Target::Server = parse_target(args)?;
    let release = args.iter().any(|a| a == "--release");
    let work = super::work_dir(root)?;

    let prepared = check::prepare(&list, &work)?;
    check::report(&prepared);
    check::write_lock_file(&prepared)?;
    // The rules first: a mod that breaks R11 does not get built.
    dist::graph_step(root, &prepared)?;

    let dist = dist::generate(root, &prepared, Mode::Server)?;
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
    println!("built {}", bin.display());
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
            parse_target(&v(&["--target=server"])).unwrap(),
            Target::Server
        );
        assert!(parse_target(&v(&["--target", "client"])).is_err());
        assert!(parse_target(&v(&["--target=all"])).is_err());
        assert!(parse_target(&v(&["--target", "x"])).is_err());
        assert!(parse_target(&v(&["--target"])).is_err());
    }

    #[test]
    fn the_binary_lives_below_the_dist_target() {
        let p = server_binary(Path::new("/t/dist"), true);
        assert!(p.starts_with("/t/dist/target/release"));
        assert!(server_binary(Path::new("/t/dist"), false).starts_with("/t/dist/target/debug"));
    }
}
