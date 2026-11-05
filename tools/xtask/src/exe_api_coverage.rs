//! `cargo xtask exe-api-coverage --contexts <K1,K2,K3|file> [--assets <dir>] [--out <file>] [--strict]`
//!: the AC-A1 report of the exe-level names the Lua of the contexts calls, against
//! what the `exe-api` crate registers. The scanner and the report are in `exe-api`
//! (`coverage`, example `coverage`); this command checks the arguments, fills in the defaults and runs it.
//! Output: `target/exe-api-coverage.json` and a table on the terminal.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::{cargo, flag_value, run};

pub fn cmd_exe_api_coverage(root: &Path, args: &[String]) -> Result<()> {
    for a in args {
        let known = ["--contexts", "--assets", "--out"]
            .iter()
            .any(|f| a == f || a.starts_with(&format!("{f}=")));
        if a.starts_with("--") && !known && a != "--strict" {
            bail!(
                "exe-api-coverage: unknown argument {a:?} (allowed: --contexts <K1,K2,K3|file>, --assets <dir>, --out <file>, --strict)"
            );
        }
    }
    // The clusters of `crates/client/exe-api/m2-contexts.txt`, or a file in its format.
    let contexts = flag_value(args, "--contexts")?.unwrap_or_else(|| "K1,K2,K3,core".to_owned());
    let assets = match flag_value(args, "--assets")? {
        Some(a) => PathBuf::from(a),
        None => match std::env::var_os("CIV5_UI_ASSETS") {
            Some(a) => PathBuf::from(a),
            None => bail!(
                "exe-api-coverage: give --assets <dir> or set CIV5_UI_ASSETS (the output of `cargo xtask assets --set ui --out <dir>`)"
            ),
        },
    };
    let out = flag_value(args, "--out")?
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target/exe-api-coverage.json"));
    let mut c = cargo();
    c.args(["run", "-q", "-p", "exe-api", "--example", "coverage", "--"])
        .arg("--contexts")
        .arg(&contexts)
        .arg("--assets")
        .arg(&assets)
        .arg("--out")
        .arg(&out);
    if args.iter().any(|a| a == "--strict") {
        c.arg("--strict");
    }
    run(root, c)
}
