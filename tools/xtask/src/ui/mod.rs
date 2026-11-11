//! `cargo xtask ui ...`: UI v2 tooling.

mod gen_model;

use std::path::Path;

use anyhow::{Result, bail};

pub use gen_model::check_generated;

/// Dispatch `cargo xtask ui <subcommand>`.
pub fn cmd_ui(root: &Path, args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("gen-model") => gen_model::run(root, &args[1..]),
        // The skin converter: `cargo xtask ui check engraved` = `cargo run -p ui2-convert -- check engraved`.
        Some("build" | "check" | "verify") => {
            let mut c = crate::cargo();
            c.args(["run", "-q", "-p", "ui2-convert", "--"]).args(args);
            crate::run(root, c)
        }
        _ => bail!(
            "usage: cargo xtask ui gen-model [--check]\n       cargo xtask ui check|build|verify <skin> [options]"
        ),
    }
}
