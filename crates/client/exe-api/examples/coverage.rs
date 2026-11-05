//! `cargo xtask exe-api-coverage --contexts <K1,K2,K3 | file> [--assets <dir>] [--out <file>] [--strict]`
//!
//! Runs the AC-A1 coverage report over the Lua of a built `ui` set. `--contexts`
//! is a file in the format of `m2-contexts.txt`, or a comma list of the clusters of that file to use
//! (`K1,K2,K3,core`). `--assets` is the output folder of `civ5-assets build --set ui` (default:
//! `CIV5_UI_ASSETS`). The JSON goes to `--out` (default `target/exe-api-coverage.json`) and the table to
//! the terminal. With `--strict` the exit code is 1 when a name of K1-K3 is missing.

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    use std::path::PathBuf;

    use exe_api::coverage::{self, Contexts, Owner};

    let mut args = std::env::args().skip(1);
    let (mut contexts, mut assets, mut out, mut strict) = (None, None, None, false);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--contexts" => contexts = args.next(),
            "--assets" => assets = args.next().map(PathBuf::from),
            "--out" => out = args.next().map(PathBuf::from),
            "--strict" => strict = true,
            other => {
                eprintln!("exe-api-coverage: unknown argument {other:?}");
                std::process::exit(2);
            }
        }
    }
    let default_list = include_str!("../m2-contexts.txt");
    let contexts = match contexts {
        None => Contexts::parse(default_list),
        Some(spec) if std::path::Path::new(&spec).is_file() => std::fs::read_to_string(&spec)
            .map_err(|e| e.to_string())
            .and_then(|t| Contexts::parse(&t)),
        Some(spec) => Contexts::parse(default_list).map(|mut all| {
            let wanted: Vec<&str> = spec.split(',').map(str::trim).collect();
            all.clusters.retain(|(n, _)| wanted.contains(&n.as_str()));
            all
        }),
    };
    let contexts = match contexts {
        Ok(c) if !c.clusters.is_empty() => c,
        Ok(_) => {
            eprintln!("exe-api-coverage: no cluster of the list matches --contexts");
            std::process::exit(2);
        }
        Err(e) => {
            eprintln!("exe-api-coverage: contexts: {e}");
            std::process::exit(2);
        }
    };
    let Some(assets) = assets.or_else(|| std::env::var_os("CIV5_UI_ASSETS").map(PathBuf::from))
    else {
        eprintln!("exe-api-coverage: give --assets <dir> or set CIV5_UI_ASSETS (a built `ui` set)");
        std::process::exit(2);
    };
    let (sources, _index) = match coverage::set::load_lua(&assets) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("exe-api-coverage: {e}");
            std::process::exit(2);
        }
    };
    let report = coverage::run(&sources, &contexts);
    print!("{}", report.table());
    let out = out.unwrap_or_else(|| PathBuf::from("target/exe-api-coverage.json"));
    if let Some(dir) = out.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match serde_json::to_string_pretty(&report.to_json()) {
        Ok(text) => match std::fs::write(&out, text) {
            Ok(()) => println!("\nwritten: {}", out.display()),
            Err(e) => {
                eprintln!("exe-api-coverage: {}: {e}", out.display());
                std::process::exit(2);
            }
        },
        Err(e) => {
            eprintln!("exe-api-coverage: json: {e}");
            std::process::exit(2);
        }
    }
    let (_, _, missing) = report.counts(&["K1", "K2", "K3"], &[Owner::ExeApi, Owner::Events]);
    println!("missing in K1-K3: {missing}");
    if strict && missing > 0 {
        std::process::exit(1);
    }
}

#[cfg(target_arch = "wasm32")]
fn main() {}
