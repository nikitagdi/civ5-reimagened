//! `ui2-convert`: the skin converter's command line (`cargo xtask ui ...` forwards here).
//!
//! ```text
//! ui2-convert check  <skin> [--dump-ir <file>] [--from-ir <file>]
//! ui2-convert build  <skin>        (not yet: package writing)
//! ui2-convert verify <skin>        (not yet: layout check against the runtime)
//! ```
//!
//! `<skin>` is a folder with `skin.toml` or the id of a folder under `skins/`. Errors go to
//! stderr in the profile's format; the exit code is 0 without errors, 1 with errors and 2 for a
//! usage problem.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ui2_convert::{Input, Run, check_skin};

const USAGE: &str = "usage: ui2-convert check <skin> [--dump-ir <file>] [--from-ir <file>]\n       ui2-convert build <skin>\n       ui2-convert verify <skin> [--state <name>]";

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

fn report(run: &Run) -> ExitCode {
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
    let mut skin = None;
    let mut dump_ir = None;
    let mut from_ir = None;
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--dump-ir" => dump_ir = it.next().cloned(),
            "--from-ir" => from_ir = it.next().cloned(),
            "--state" => {
                it.next();
            }
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
    let Some(dir) = skin_dir(&skin) else {
        eprintln!(
            "ui2-convert: `{skin}` is neither a folder with skin.toml nor a skin id under skins/"
        );
        return ExitCode::from(2);
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
            let input = match &stored {
                Some(t) => Input::Dump(t),
                None => Input::Chrome,
            };
            let run = check_skin(&dir, input);
            if let (Some(path), Some(text)) = (&dump_ir, run.dump())
                && let Err(e) = std::fs::write(path, text)
            {
                eprintln!("ui2-convert: cannot write {path}: {e}");
                return ExitCode::from(2);
            }
            report(&run)
        }
        "build" | "verify" => {
            eprintln!(
                "ui2-convert {cmd}: not implemented yet (package writing and the layout check come later); `check` runs every check that exists"
            );
            ExitCode::from(2)
        }
        other => {
            eprintln!("unknown command {other}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}
