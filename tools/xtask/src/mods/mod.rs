//! `cargo xtask mods ...`: the distribution of the operator's mods (spec: `SPEC-dist-assembly.md`).
//!
//! `mods check --mods <file>` resolves the list and writes `mods.lock`; `mods build --mods <file> --target server|client|all`
//! assembles `target/dist/` and builds it; `dev --mods <file>` runs the result. The public build (no `--mods`) is not touched by any of this.

pub mod build;
pub mod check;
pub mod dev;
pub mod dist;
pub mod gen_client;
pub mod gen_server;
pub mod local;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

pub fn cmd_mods(root: &Path, args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("check") => check::cmd_check(root, &args[1..]),
        Some("build") => build::cmd_build(root, &args[1..]),
        Some(other) => {
            bail!("unknown `mods` subcommand `{other}`; use `mods check|build --mods <file>`")
        }
        None => {
            bail!(
                "usage: cargo xtask mods check|build --mods <file> [--target server|client|web|desktop|all] [--release] [--single]"
            )
        }
    }
}

/// Value of `--mods <file>` / `--mods=<file>`.
pub fn mods_arg(args: &[String]) -> Result<PathBuf> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if let Some(v) = a.strip_prefix("--mods=") {
            return Ok(PathBuf::from(v));
        }
        if a == "--mods" {
            return it
                .next()
                .map(PathBuf::from)
                .ok_or_else(|| anyhow!("`--mods` needs a file"));
        }
    }
    bail!("`--mods <file>` is required")
}

/// Scratch space of the mods commands: `target/mods-work` (git checkouts, staged copies for hashing).
pub fn work_dir(root: &Path) -> Result<PathBuf> {
    let d = crate::target_dir(root)?.join("mods-work");
    std::fs::create_dir_all(&d).with_context(|| format!("cannot create {}", d.display()))?;
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_mods_argument_has_two_spellings() {
        assert_eq!(
            mods_arg(&v(&["--mods", "x.toml"])).unwrap(),
            Path::new("x.toml")
        );
        assert_eq!(
            mods_arg(&v(&["--release", "--mods=y"])).unwrap(),
            Path::new("y")
        );
        assert!(mods_arg(&v(&["--mods"])).is_err());
        assert!(mods_arg(&v(&[])).is_err());
    }
}
