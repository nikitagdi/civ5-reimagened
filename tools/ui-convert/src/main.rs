//! `ui2-convert`: the skin converter's command line (`cargo xtask ui ...` forwards here).
//!
//! ```text
//! ui2-convert check  <skin> [--dump-ir <file>] [--from-ir <file>]
//! ui2-convert build  <skin> [--dump-ir <file>]   (the package in target/ui2/<id>/)
//! ui2-convert verify <skin> [--state <name>]   (the runtime's layout against Chromium's)
//! ui2-convert web    <web-dir>    (every skin of skins/ into <web-dir>/ui2/ with skins.json)
//! ui2-convert watch  <skin> [--port <n>]   (the package server for hot reload, 127.0.0.1:8095)
//! ui2-convert fragment <mod dir> --out <dir>   (the panels of a mod's [ui]: <id>-<version>-<hash16>.ui2)
//! ui2-convert docs [--check]   (the profile section of docs/ui-skins.md from profile.toml)
//! ```
//!
//! `fragment` prints one line `ui2-fragment <file> <blake3 hex>` on stdout when the package is written (what
//! `cargo xtask mods build` reads); warnings and errors go to stderr.
//!
//! `<skin>` is a folder with `skin.toml` or the id of a folder under `skins/`. Errors go to
//! stderr in the profile's format; the exit code is 0 without errors, 1 with errors and 2 for a
//! usage problem.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ui2_convert::{Input, Run, check_skin};

const USAGE: &str = "usage: ui2-convert check <skin> [--dump-ir <file>] [--from-ir <file>]\n       ui2-convert build <skin>\n       ui2-convert verify <skin> [--state <name>]\n       ui2-convert web <web-dir>\n       ui2-convert watch <skin> [--port <n>]\n       ui2-convert fragment <mod dir> --out <dir>\n       ui2-convert docs [--check]";

/// `docs [--check]`: writes the profile section of `docs/ui-skins.md` from the profile file; `--check` only says
/// whether it is current (exit code 1 when not).
fn docs(args: &[String]) -> ExitCode {
    let check = match args {
        [] => false,
        [a] if a == "--check" => true,
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let path = ui2_convert::docs::doc_path();
    let old = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!(
                "ui2-convert: cannot read {}: {e}",
                ui2_convert::docs::DOC_FILE
            );
            return ExitCode::from(2);
        }
    };
    let new = ui2_convert::docs::update(&old);
    if new == old.replace("\r\n", "\n") {
        println!(
            "{}: the profile section is current",
            ui2_convert::docs::DOC_FILE
        );
        return ExitCode::SUCCESS;
    }
    if check {
        eprintln!(
            "{}: the profile section is stale; run `cargo xtask ui docs`",
            ui2_convert::docs::DOC_FILE
        );
        return ExitCode::from(1);
    }
    match std::fs::write(&path, new) {
        Ok(()) => {
            println!(
                "{}: the profile section written",
                ui2_convert::docs::DOC_FILE
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!(
                "ui2-convert: cannot write {}: {e}",
                ui2_convert::docs::DOC_FILE
            );
            ExitCode::from(1)
        }
    }
}

/// `fragment <mod dir> --out <dir>`: the fragment package of a mod's panels.
fn fragment(dir: &str, out: Option<&str>) -> ExitCode {
    let Some(out) = out else {
        eprintln!("fragment needs --out <dir>\n{USAGE}");
        return ExitCode::from(2);
    };
    let o = ui2_convert::fragment::build(Path::new(dir), Path::new(out));
    for w in &o.run.warnings {
        eprintln!("warning: {w}");
    }
    for e in &o.run.errors {
        eprintln!("{e}\n");
    }
    match &o.written {
        Some(w) if o.run.errors.is_empty() => {
            eprintln!(
                "ui2-convert: fragment {} ({} bytes)",
                w.file_name,
                w.bytes.len()
            );
            println!("ui2-fragment {} {}", w.file_name, w.hash);
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("ui2-convert: {} error(s)", o.run.errors.len());
            ExitCode::from(1)
        }
    }
}

/// The skin folder of an argument: an existing folder, or `skins/<id>` of the repository.
fn skin_dir(arg: &str) -> Option<PathBuf> {
    let p = Path::new(arg);
    if p.join("skin.toml").is_file() {
        return Some(p.to_path_buf());
    }
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    for base in [Path::new("."), repo.as_path()] {
        let d = base.join("skins").join(arg);
        if d.join("skin.toml").is_file() {
            return Some(d);
        }
    }
    None
}

/// The first `[verify] states` entry of a skin.
fn first_state(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("skin.toml")).ok()?;
    let t: toml::Table = text.parse().ok()?;
    t.get("verify")?
        .get("states")?
        .as_array()?
        .first()?
        .as_str()
        .map(str::to_string)
}

/// `web <web-dir>`: builds every skin under `skins/` and publishes the packages and `skins.json` into
/// `<web-dir>/ui2/`.
fn web(web_dir: &str) -> ExitCode {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    let skins = repo.join("skins");
    let mut ids: Vec<PathBuf> = match std::fs::read_dir(&skins) {
        Ok(rd) => rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.join("skin.toml").is_file())
            .collect(),
        Err(e) => {
            eprintln!("ui2-convert: cannot read {}: {e}", skins.display());
            return ExitCode::from(2);
        }
    };
    ids.sort();
    let mut packages = Vec::new();
    let mut samples = Vec::new();
    let mut failed = false;
    for dir in &ids {
        let dir = &match ui2_convert::extends::resolve(dir) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("{e}\n");
                failed = true;
                continue;
            }
        };
        let out = ui2_convert::build::build(dir, true);
        for e in &out.run.errors {
            eprintln!("{e}\n");
        }
        match out.written {
            Some(w) if out.run.errors.is_empty() => {
                println!(
                    "skin {}: {} ({} bytes)",
                    dir.display(),
                    w.file_name,
                    w.bytes.len()
                );
                packages.push(out.out_dir.join(&w.file_name));
                // `<id>-<hash16>.ui2`: the samples go under the skin's id.
                if let Some((id, _)) = w.file_name.rsplit_once('-') {
                    samples.push((id.to_string(), dir.clone()));
                }
            }
            _ => {
                eprintln!("ui2-convert: skin {} did not build", dir.display());
                failed = true;
            }
        }
    }
    if failed {
        return ExitCode::from(1);
    }
    match ui2_convert::index::publish(Path::new(web_dir), &packages)
        .and_then(|e| ui2_convert::index::publish_samples(Path::new(web_dir), &samples).map(|_| e))
    {
        Ok(entries) => {
            println!(
                "ui2: {} skin(s) and {} in {}",
                entries.len(),
                ui2_convert::index::INDEX_FILE,
                Path::new(web_dir)
                    .join(ui2_convert::index::UI2_DIR)
                    .display()
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("ui2-convert: {e}");
            ExitCode::from(1)
        }
    }
}

fn report(run: &Run) -> ExitCode {
    for line in run.summary() {
        println!("{line}");
    }
    for e in &run.errors {
        eprintln!("{e}\n");
    }
    if run.errors.is_empty() {
        println!("ui2-convert: no errors");
        ExitCode::SUCCESS
    } else {
        eprintln!("ui2-convert: {} error(s)", run.errors.len());
        ExitCode::from(1)
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    if cmd == "docs" {
        return docs(&args[1..]);
    }
    let mut skin = None;
    let mut dump_ir = None;
    let mut from_ir = None;
    let mut state = None;
    let mut out = None;
    let mut port = ui2_convert::watch::DEFAULT_PORT;
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--dump-ir" => dump_ir = it.next().cloned(),
            "--from-ir" => from_ir = it.next().cloned(),
            "--state" => state = it.next().cloned(),
            "--out" => out = it.next().cloned(),
            "--port" => match it.next().and_then(|p| p.parse().ok()) {
                Some(p) => port = p,
                None => {
                    eprintln!("--port takes a port number\n{USAGE}");
                    return ExitCode::from(2);
                }
            },
            s if s.starts_with("--") => {
                eprintln!("unknown option {s}\n{USAGE}");
                return ExitCode::from(2);
            }
            s => skin = Some(s.to_string()),
        }
    }
    let Some(skin) = skin else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    if cmd == "web" {
        return web(&skin);
    }
    if cmd == "fragment" {
        return fragment(&skin, out.as_deref());
    }
    let Some(dir) = skin_dir(&skin) else {
        eprintln!(
            "ui2-convert: `{skin}` is neither a folder with skin.toml nor a skin id under skins/"
        );
        return ExitCode::from(2);
    };
    let dir = match ui2_convert::extends::resolve(&dir) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}\n");
            return ExitCode::from(1);
        }
    };
    match cmd.as_str() {
        "check" => {
            let stored = match &from_ir {
                Some(f) => match std::fs::read_to_string(f) {
                    Ok(t) => Some(t),
                    Err(e) => {
                        eprintln!("ui2-convert: cannot read {f}: {e}");
                        return ExitCode::from(2);
                    }
                },
                None => None,
            };
            let run = match &stored {
                Some(t) => check_skin(&dir, Input::Dump(t)),
                None => ui2_convert::build::build(&dir, false).run,
            };
            if let (Some(path), Some(text)) = (&dump_ir, run.dump())
                && let Err(e) = std::fs::write(path, text)
            {
                eprintln!("ui2-convert: cannot write {path}: {e}");
                return ExitCode::from(2);
            }
            report(&run)
        }
        "build" => {
            let out = ui2_convert::build::build(&dir, true);
            if let (Some(path), Some(text)) = (&dump_ir, out.run.dump())
                && let Err(e) = std::fs::write(path, text)
            {
                eprintln!("ui2-convert: cannot write {path}: {e}");
                return ExitCode::from(2);
            }
            if let Some(w) = &out.written {
                println!(
                    "package: {} ({} bytes, blake3 {})",
                    w.file_name,
                    w.bytes.len(),
                    w.hash
                );
                for (tag, n) in &w.sections {
                    println!("  {tag} {n}");
                }
                for (what, n, max) in ui2_convert::write::budget(&w.sections) {
                    println!("  budget {what}: {n} of {max}");
                }
            }
            report(&out.run)
        }
        "verify" => {
            let state = match state {
                Some(s) => s,
                None => match first_state(&dir) {
                    Some(s) => s,
                    None => {
                        eprintln!("ui2-convert: skin.toml lists no [verify] states; pass --state");
                        return ExitCode::from(2);
                    }
                },
            };
            match ui2_convert::verify::verify(&dir, &state) {
                Ok(o) => {
                    for l in o.report.lines() {
                        println!("{l}");
                    }
                    if o.report.ok() {
                        ExitCode::SUCCESS
                    } else {
                        ExitCode::from(1)
                    }
                }
                Err(errs) => {
                    for e in &errs {
                        eprintln!("{e}\n");
                    }
                    eprintln!("ui2-convert: {} error(s)", errs.len());
                    ExitCode::from(1)
                }
            }
        }
        "watch" => {
            let opts = ui2_convert::watch::WatchOpts {
                port,
                ..Default::default()
            };
            match ui2_convert::watch::run(&dir, &opts) {
                Ok(()) => ExitCode::SUCCESS,
                Err(errs) => {
                    for e in &errs {
                        eprintln!("{e}\n");
                    }
                    ExitCode::from(1)
                }
            }
        }
        other => {
            eprintln!("unknown command {other}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}
