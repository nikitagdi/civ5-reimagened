//! UI v2 skins in the web build: `cargo xtask build --only web` puts the skin packages and their index into
//! `<web>/ui2/` (`<id>-<hash16>.ui2` and `skins.json`). The converter does the work (`ui2-convert web`); this
//! module only decides whether to run it and explains what is missing.

use std::path::Path;
use std::process::Command;

use anyhow::{Result, bail};

/// The variable that skips the skin build.
pub const SKIP_VAR: &str = "CIV5_SKIP_UI2";

/// Whether the skin build is switched off (`CIV5_SKIP_UI2=1`).
pub fn skipped() -> bool {
    std::env::var(SKIP_VAR).is_ok_and(|v| v == "1")
}

/// The Node executable of the converter (`CIV5_NODE` or `node`).
fn node_exe() -> String {
    std::env::var("CIV5_NODE").unwrap_or_else(|_| "node".to_string())
}

/// Builds the skins into `<web>/ui2/`, unless `CIV5_SKIP_UI2=1`.
pub fn build_skins(root: &Path, web: &Path) -> Result<()> {
    if skipped() {
        println!("ui2 skins: skipped ({SKIP_VAR}=1)");
        return Ok(());
    }
    let node = node_exe();
    let runnable = Command::new(&node)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success());
    if !runnable {
        bail!(
            "the UI v2 skins are built with Node (`{node}` is not runnable): install Node and run `npm ci` in tools/ui-convert/node and tools/playtest, or set {SKIP_VAR}=1 to build the web bundle without skins"
        );
    }
    let mut c = crate::cargo();
    c.args(["run", "-q", "-p", "ui2-convert", "--", "web"])
        .arg(web);
    crate::run(root, c)
}
