//! `cargo xtask ui ...`: UI v2 tooling.

mod gen_model;
pub mod web;

use std::path::Path;

use anyhow::{Result, bail};

pub use gen_model::check_generated;

/// The port `cargo xtask dev --ui-dev` serves the skin's packages on.
pub const DEV_WATCH_PORT: u16 = 8095;

/// `cargo xtask ui watch <skin>` running next to another command (`cargo xtask dev --ui-dev <skin>`): stopped when
/// this goes.
pub struct Watch(std::process::Child);

impl Drop for Watch {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Starts the watch of `skin` (an id under `skins/` or a folder) on `port`, in the background. The page reloads
/// the skin with `?ui=v2:<skin>&ui2dev=http://127.0.0.1:<port>`.
pub fn spawn_watch(root: &Path, skin: &str, port: u16) -> Result<Watch> {
    let mut c = crate::cargo();
    c.current_dir(root);
    c.args([
        "run",
        "-q",
        "-p",
        "ui2-convert",
        "--",
        "watch",
        skin,
        "--port",
    ])
    .arg(port.to_string());
    let child = c.spawn()?;
    let id = Path::new(skin)
        .file_name()
        .map_or_else(|| skin.to_string(), |n| n.to_string_lossy().to_string());
    println!(
        "ui watch {skin}: open the game with ?ui=v2:{id}&ui2dev=http://127.0.0.1:{port} to reload the skin on every save"
    );
    Ok(Watch(child))
}

/// Takes `flag <value>` (or `flag=<value>`) out of `args`: the value, and the other arguments.
pub fn take_value(args: &[String], flag: &str) -> Result<(Option<String>, Vec<String>)> {
    let mut value = None;
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == flag {
            let Some(v) = it.next() else {
                bail!("{flag} requires a value");
            };
            value = Some(v.clone());
        } else if let Some(v) = a.strip_prefix(&format!("{flag}=")) {
            value = Some(v.to_string());
        } else {
            rest.push(a.clone());
        }
    }
    Ok((value, rest))
}

/// Dispatch `cargo xtask ui <subcommand>`.
pub fn cmd_ui(root: &Path, args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("gen-model") => gen_model::run(root, &args[1..]),
        // The skin converter: `cargo xtask ui check engraved` = `cargo run -p ui2-convert -- check engraved`;
        // `cargo xtask ui docs [--check]` writes the profile section of docs/ui-skins.md from profile.toml.
        Some("build" | "check" | "verify" | "watch" | "docs") => {
            let mut c = crate::cargo();
            c.args(["run", "-q", "-p", "ui2-convert", "--"]).args(args);
            crate::run(root, c)
        }
        _ => bail!(
            "usage: cargo xtask ui gen-model [--check]\n       cargo xtask ui check|build|verify|watch <skin> [options]\n       cargo xtask ui docs [--check]"
        ),
    }
}
