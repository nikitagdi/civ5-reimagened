//! `cargo xtask` entry point. Rules: `graph.rs`; file checks: `checks.rs`.

mod checks;
mod deploy;
mod exe_api_coverage;
mod graph;
mod meta;
mod mods;
mod ui;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow, bail};

use graph::{DepsConfig, Graph, Violation};

const WASM_TARGET: &str = "wasm32-unknown-unknown";
const WEB_CRATE: &str = "app-web";
const WEB_OUT_NAME: &str = "app_web";

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR = <root>/tools/xtask
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn load_deps_config(root: &Path) -> Result<DepsConfig> {
    let path = root.join("tools/xtask/deps.toml");
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("failed to parse {}", path.display()))
}

fn cargo() -> Command {
    Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
}

/// Env of `--gamecore-lib`, applied to every child process.
static GAMECORE_ENV: OnceLock<Vec<(&'static str, PathBuf)>> = OnceLock::new();

fn run(root: &Path, mut cmd: Command) -> Result<()> {
    cmd.current_dir(root);
    // A Windows build cannot link the Linux archive (the x86_64 variable would pull it into civ5-server.exe):
    // only the wasm archive goes to native Windows commands. The WSL and deploy scripts read the table themselves.
    cmd.envs(
        GAMECORE_ENV
            .get()
            .into_iter()
            .flatten()
            .filter(|(v, _)| !(cfg!(windows) && *v == "GAMECORE_LIB_DIR_x86_64"))
            .cloned(),
    );
    println!("$ {cmd:?}");
    let status = cmd
        .status()
        .with_context(|| format!("failed to start {cmd:?}"))?;
    if !status.success() {
        bail!("command failed ({status}): {cmd:?}");
    }
    Ok(())
}

fn target_dir(root: &Path) -> Result<PathBuf> {
    let md = cargo_metadata::MetadataCommand::new()
        .manifest_path(root.join("Cargo.toml"))
        .no_deps()
        .exec()
        .context("cargo metadata --no-deps")?;
    Ok(md.target_directory.into_std_path_buf())
}

/// `cargo fmt --check` over the workspace, a few packages at a time: one `--all` call puts every target file
/// on a single rustfmt command line, which no longer fits the Windows limit.
fn fmt_check(root: &Path) -> Result<()> {
    let md = cargo_metadata::MetadataCommand::new()
        .manifest_path(root.join("Cargo.toml"))
        .no_deps()
        .exec()
        .context("cargo metadata --no-deps")?;
    let names: Vec<String> = md
        .workspace_packages()
        .iter()
        .map(|p| p.name.to_string())
        .collect();
    for group in names.chunks(8) {
        let mut c = cargo();
        c.arg("fmt");
        for name in group {
            c.args(["-p", name]);
        }
        c.args(["--", "--check"]);
        run(root, c)?;
    }
    Ok(())
}

/// Two passes per target: default features and `--all-features`.
fn analyze_deps(root: &Path) -> Result<(Vec<String>, Graph)> {
    let cfg = load_deps_config(root)?;
    let host = meta::host_triple()?;
    let mut layer_problems: Vec<String> = Vec::new();
    let mut found: Vec<(Violation, Vec<String>)> = Vec::new();
    let mut host_graph = Graph::default();
    for (label, triple) in [("host", host.as_str()), ("wasm32", WASM_TARGET)] {
        for all_features in [false, true] {
            let full = if all_features {
                format!("{label}+all-features")
            } else {
                label.to_string()
            };
            let md = meta::load(root, triple, all_features)?;
            let (g, layer_errors) = meta::build_graph(&md)?;
            for e in layer_errors {
                let line = format!("layer: {e}");
                if !layer_problems.contains(&line) {
                    layer_problems.push(line);
                }
            }
            graph::merge_violations(&mut found, &full, graph::check_all(&g, &cfg));
            println!("graph {full} ({triple}): {} nodes", g.nodes.len());
            if label == "host" && !all_features {
                host_graph = g;
            }
        }
    }
    let mut problems = layer_problems;
    for (v, labels) in found {
        let line = format!("{v} [graph: {}]", labels.join(", "));
        if v.warning {
            // R10 in `warn` mode: visible, but does not fail the check.
            println!("WARNING {line}");
        } else {
            problems.push(line);
        }
    }
    Ok((problems, host_graph))
}

fn cmd_deps(root: &Path) -> Result<()> {
    let (problems, g) = analyze_deps(root)?;
    let plan = graph::r8_plan(&g);
    println!("R8: wasm32: {}", plan.wasm.join(", "));
    println!("R8: native: {}", plan.native.join(", "));
    println!("R8 is checked by the build in `cargo xtask check`.");
    if problems.is_empty() {
        println!("deps: rules R1–R7, R9 and R10 satisfied");
        return Ok(());
    }
    for p in &problems {
        println!("VIOLATION {p}");
    }
    bail!("deps: violations found: {}", problems.len());
}

fn expected_bindgen_version(root: &Path) -> Result<String> {
    let text =
        std::fs::read_to_string(root.join("Cargo.lock")).context("failed to read Cargo.lock")?;
    let lock: toml::Table = toml::from_str(&text).context("failed to parse Cargo.lock")?;
    lock.get("package")
        .and_then(|p| p.as_array())
        .into_iter()
        .flatten()
        .filter_map(|p| p.as_table())
        .find(|p| p.get("name").and_then(|n| n.as_str()) == Some("wasm-bindgen"))
        .and_then(|p| p.get("version")?.as_str().map(str::to_string))
        .context("Cargo.lock has no wasm-bindgen crate")
}

fn bindgen_cli_problem(root: &Path) -> Result<Option<String>> {
    let want = expected_bindgen_version(root)?;
    let install = format!("cargo install wasm-bindgen-cli --version {want} --locked");
    let out = match Command::new("wasm-bindgen").arg("--version").output() {
        Ok(o) => o,
        Err(_) => {
            return Ok(Some(format!(
                "wasm-bindgen CLI not found in PATH. Install: {install}"
            )));
        }
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let have = text.split_whitespace().nth(1).unwrap_or("").to_string();
    if have != want {
        return Ok(Some(format!(
            "wasm-bindgen CLI version ({have}) does not match the wasm-bindgen crate version in Cargo.lock ({want}). Install: {install} --force"
        )));
    }
    Ok(None)
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from).with_context(|| format!("reading {}", from.display()))? {
        let entry = entry?;
        let dst = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &dst)?;
        } else {
            std::fs::copy(entry.path(), &dst)?;
        }
    }
    Ok(())
}

fn profile_dir(release: bool) -> &'static str {
    if release { "release" } else { "debug" }
}

/// Which package the wasm build compiles and where cargo puts the result: the public `app-web` of the root workspace,
/// or the generated `dist-client-web` of `target/dist/client` (`cargo xtask mods build --target client`). Both give
/// `app_web.wasm`, so the page (`bootstrap.js`) and the bindgen step are the same.
#[derive(Clone)]
struct WebPkg {
    /// `--manifest-path` of the workspace; `None`: the root one.
    manifest: Option<PathBuf>,
    package: String,
    /// Target directory of the single-threaded build.
    target: PathBuf,
    /// Target directory of the threaded build.
    mt_target: PathBuf,
}

fn public_web_pkg(root: &Path) -> Result<WebPkg> {
    let target = target_dir(root)?;
    Ok(WebPkg {
        manifest: None,
        package: WEB_CRATE.into(),
        mt_target: target.join("wasm-mt"),
        target,
    })
}

fn raw_wasm_path(root: &Path, release: bool) -> Result<PathBuf> {
    Ok(raw_wasm_of(&public_web_pkg(root)?.target, release))
}

fn raw_wasm_of(target: &Path, release: bool) -> PathBuf {
    target
        .join(WASM_TARGET)
        .join(profile_dir(release))
        .join(format!("{WEB_OUT_NAME}.wasm"))
}

fn cargo_build_wasm(root: &Path, release: bool) -> Result<()> {
    cargo_build_wasm_with(root, release, &[])
}

fn cargo_build_wasm_with(root: &Path, release: bool, features: &[&str]) -> Result<()> {
    cargo_build_wasm_pkg(root, release, features, &public_web_pkg(root)?)
}

fn cargo_build_wasm_pkg(root: &Path, release: bool, features: &[&str], pkg: &WebPkg) -> Result<()> {
    let mut c = cargo();
    c.args(["build", "--target", WASM_TARGET, "-p", &pkg.package]);
    if let Some(m) = &pkg.manifest {
        c.arg("--manifest-path").arg(m);
    }
    if !features.is_empty() {
        c.arg("--features").arg(features.join(","));
    }
    if release {
        c.arg("--release");
    }
    run(root, c)
}

/// The toolchain of the threaded wasm build (B3 render-mt): `-Z build-std` needs the `rust-src` of the
/// compiler, which the pinned 1.95.0 install lacks and `stable` (1.95.0 too) has. `CIV5_MT_TOOLCHAIN`
/// names another one.
fn mt_toolchain() -> String {
    std::env::var("CIV5_MT_TOOLCHAIN").unwrap_or_else(|_| "stable".into())
}

/// The threaded build of the client (`<target>/wasm-mt`): std rebuilt with atomics (`-Z build-std`
/// through `RUSTC_BOOTSTRAP=1` on a stable compiler), a shared memory of at most 4 GiB.
fn cargo_build_wasm_mt(root: &Path, release: bool, features: &[&str]) -> Result<PathBuf> {
    cargo_build_wasm_mt_pkg(root, release, features, &public_web_pkg(root)?)
}

fn cargo_build_wasm_mt_pkg(
    root: &Path,
    release: bool,
    features: &[&str],
    pkg: &WebPkg,
) -> Result<PathBuf> {
    let target = pkg.mt_target.clone();
    // The rustup proxy, not `$CARGO`: the toolchain is the one with `rust-src`.
    let mut c = Command::new("cargo");
    c.env("RUSTUP_TOOLCHAIN", mt_toolchain())
        .env("RUSTC_BOOTSTRAP", "1")
        .env("CARGO_TARGET_DIR", &target)
        .env_remove("CARGO")
        .env_remove("RUSTC")
        // Replaces `[target.wasm32-unknown-unknown] rustflags` of `.cargo/config.toml`: its stack size is
        // repeated here.
        .env(
            "RUSTFLAGS",
            "-C target-feature=+atomics,+bulk-memory,+mutable-globals \
             -C link-arg=-zstack-size=8388608 -C link-arg=--max-memory=4294967296 \
             -C link-arg=--shared-memory -C link-arg=--import-memory \
             -C link-arg=--export=__wasm_init_tls -C link-arg=--export=__tls_size \
             -C link-arg=--export=__tls_align -C link-arg=--export=__tls_base",
        )
        .args(["build", "--target", WASM_TARGET, "-p", &pkg.package])
        .args(["-Z", "build-std=std,panic_abort"]);
    if let Some(m) = &pkg.manifest {
        c.arg("--manifest-path").arg(m);
    }
    if !features.is_empty() {
        c.arg("--features").arg(features.join(","));
    }
    if release {
        c.arg("--release");
    }
    run(root, c).context(
        "the threaded wasm build failed: it needs the `rust-src` component of the toolchain \
         (CIV5_MT_TOOLCHAIN, default `stable`)",
    )?;
    Ok(raw_wasm_of(&target, release))
}

/// `wasm-bindgen` of `wasm` into `pkg`, and `wasm-opt` in a release. (No brain check: the client holds the replica core.)
fn bindgen_pkg(root: &Path, wasm: &Path, pkg: &Path, release: bool) -> Result<()> {
    let mut c = Command::new("wasm-bindgen");
    c.args(["--target", "web", "--out-name", WEB_OUT_NAME, "--out-dir"])
        .arg(pkg)
        .arg(wasm);
    run(root, c)?;
    patch_glue(pkg)?;
    let bg = pkg.join(format!("{WEB_OUT_NAME}_bg.wasm"));
    if release {
        optimise_wasm(root, &bg)?;
    }
    Ok(())
}

/// The glue of `pkg` with the trap hook ([`hook_trap_in_glue`]); another glue gets a warning.
fn patch_glue(pkg: &Path) -> Result<()> {
    let glue = pkg.join(format!("{WEB_OUT_NAME}.js"));
    let js = std::fs::read_to_string(&glue).with_context(|| format!("read {}", glue.display()))?;
    match hook_trap_in_glue(&js) {
        Some(patched) => {
            std::fs::write(&glue, patched).with_context(|| format!("write {}", glue.display()))?
        }
        None => println!(
            "warning: {} has no `__wbg_handle_catch` termination line: the first trap is not reported \
             (only the page's error events are)",
            glue.display()
        ),
    }
    Ok(())
}

/// The line of wasm-bindgen's glue (0.2.129) that marks the module terminated, in `__wbg_handle_catch`.
const GLUE_TERMINATE_LINE: &str = "getInt32ArrayMemory0()[__wbg_terminated_addr] = 1;";

/// The glue with a call of `globalThis.__civ5OnTrap(e, alreadyTerminated)` just before it marks the module terminated, or `None`
/// when the glue has not exactly one such line.
///
/// SIMPLIFIED(SIMP-exe-690): wasm-bindgen's glue marks the module terminated on the first exception that is
/// not a wrapped JS one (a trap of any thread, a JS exception through wasm) and then only throws "Module
/// terminated"; the first error is lost when a thread other than the page's trapped or a frame between
/// caught it (live report 2026-10-31). Its own hook (`set_on_abort`) gets no error and fires only with
/// panic=unwind, so the generated text is patched to hand the error to the page's report
/// (`bootstrap.js`, `jobs-worker.js`).
fn hook_trap_in_glue(js: &str) -> Option<String> {
    if js.matches(GLUE_TERMINATE_LINE).count() != 1 {
        return None;
    }
    Some(js.replacen(
        GLUE_TERMINATE_LINE,
        &format!(
            "try {{ globalThis.__civ5OnTrap?.(e, getInt32ArrayMemory0()[__wbg_terminated_addr] !== 0); }} catch (_) {{}}\n    {GLUE_TERMINATE_LINE}"
        ),
        1,
    ))
}

/// `wasm-opt -O` of a release `.wasm` in place: required (binaryen >= 116, `CIV5_WASM_OPT` or PATH);
/// `CIV5_SKIP_WASM_OPT=1` opts out. `--all-features` keeps the exceptions of the C++ and, in the threaded
/// build, the atomics, the bulk memory and the shared memory.
fn optimise_wasm(root: &Path, bg: &Path) -> Result<()> {
    if std::env::var_os("CIV5_SKIP_WASM_OPT").is_some() {
        println!(
            "warning: CIV5_SKIP_WASM_OPT set, {} is NOT optimised",
            bg.display()
        );
        return Ok(());
    }
    let opt = wasm_opt_path();
    check_wasm_opt(&opt)?;
    let mut c = Command::new(&opt);
    c.args(["-O", "--enable-exception-handling", "--all-features"]);
    // CIV5_WASM_NAMES=1 keeps the name section (function names in a CPU profile; same code).
    if std::env::var_os("CIV5_WASM_NAMES").is_some() {
        c.arg("-g");
    }
    c.arg(bg).arg("-o").arg(bg);
    run(root, c)
}

/// The bundle with the threaded build next to the single-threaded one (`<target>/web/pkg-mt`): the
/// page takes it when it is cross-origin isolated (`bootstrap.js`).
fn build_web_threads(root: &Path, release: bool) -> Result<PathBuf> {
    let web = build_web(root, release)?;
    build_web_threads_into(root, release, &web, &public_web_pkg(root)?)?;
    Ok(web)
}

/// The threaded build of `pkg` into `<web>/pkg-mt`.
fn build_web_threads_into(root: &Path, release: bool, web: &Path, pkg: &WebPkg) -> Result<()> {
    // The replica links the game core: the threaded link needs its archive built with atomics
    // (`wasm-mt/`, GAMECORE_LIB_DIR_wasm32_threads); a failed link is an error, not a build without threads.
    let wasm = cargo_build_wasm_mt_pkg(root, release, &[], pkg)?;
    bindgen_pkg(root, &wasm, &web.join("pkg-mt"), release)?;
    println!("web bundle (threads in pkg-mt): {}", web.display());
    Ok(())
}

/// Returns the bundle directory (`<target>/web`).
fn build_web(root: &Path, release: bool) -> Result<PathBuf> {
    build_web_with(root, release, &[], "web")
}

/// The web bundle with `features` of `app-web`, in `<target>/<dir>`.
fn build_web_with(root: &Path, release: bool, features: &[&str], dir: &str) -> Result<PathBuf> {
    let web = target_dir(root)?.join(dir);
    build_web_pkg(root, release, features, &web, &public_web_pkg(root)?)?;
    Ok(web)
}

/// The web bundle of `wp` in `web`: wasm, bindgen, glue patch, the page files of `apps/web/www`, wasm-opt in a release.
fn build_web_pkg(
    root: &Path,
    release: bool,
    features: &[&str],
    web: &Path,
    wp: &WebPkg,
) -> Result<()> {
    if let Some(problem) = bindgen_cli_problem(root)? {
        bail!("{problem}");
    }
    cargo_build_wasm_pkg(root, release, features, wp)?;
    let pkg = web.join("pkg");
    let wasm = raw_wasm_of(&wp.target, release);
    let mut c = Command::new("wasm-bindgen");
    c.args(["--target", "web", "--out-name", WEB_OUT_NAME, "--out-dir"])
        .arg(&pkg)
        .arg(&wasm);
    run(root, c)?;
    patch_glue(&pkg)?;
    copy_dir(&root.join("apps/web/www"), web)?;
    // A bundle built without threads has no threaded build: an old one would load with new glue.
    let _ = std::fs::remove_dir_all(web.join("pkg-mt"));

    let bg = pkg.join(format!("{WEB_OUT_NAME}_bg.wasm"));
    if release {
        // Release and deploy builds must be optimised: a missing or old wasm-opt is a hard error
        // (CIV5_SKIP_WASM_OPT=1 opts out explicitly).
        optimise_wasm(root, &bg)?;
    } else if which(&wasm_opt_path()) {
        println!("note: dev build, wasm-opt not run (only --release optimises)");
    } else {
        println!(
            "warning: dev build is unoptimised; wasm-opt is not installed (needed for --release)"
        );
    }
    println!("web bundle: {}", web.display());
    Ok(())
}

/// The web bundle of the generated `dist-client-web` (`target/dist/client`, `cargo xtask mods build --target client`) in
/// `bundle`, with `pkg-mt` beside `pkg` when `threads`: the public pipeline over another package.
fn build_dist_web(
    root: &Path,
    release: bool,
    threads: bool,
    client_dir: &Path,
    bundle: &Path,
) -> Result<()> {
    let wp = WebPkg {
        manifest: Some(client_dir.join("Cargo.toml")),
        package: "dist-client-web".into(),
        target: client_dir.join("target"),
        mt_target: client_dir.join("target-mt"),
    };
    build_web_pkg(root, release, &[], bundle, &wp)?;
    if threads {
        build_web_threads_into(root, release, bundle, &wp)?;
    }
    Ok(())
}

/// `web-selftest [--release] [--fixture <dir>] [--scenario tiny|small|late-huge|all]`: builds the web bundle with the
/// replica self-test (feature `replica-selftest` of `app-web`; with `--gamecore-lib` the real wasm core) and runs
/// the case `replica_load` in headless Chromium (`tools/web-selftest/run.mjs`): the scripted replica of the
/// fixture (made by `cargo test -p replica --test real_replica` with `REPLICA_FIXTURE_OUT=<dir>` on Linux/WSL)
/// joins on a snapshot, applies the journal, changes turns by snapshot and by patch, and its digest must equal the
/// server's after each step. The report (the metrics of AC-G4 in the browser) is printed.
fn cmd_web_selftest(root: &Path, args: &[String]) -> Result<()> {
    let mut release = false;
    let mut fixture: Option<PathBuf> = None;
    let mut scenario = "tiny".to_owned();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--release" => release = true,
            "--fixture" => {
                fixture = Some(PathBuf::from(it.next().context("--fixture needs a dir")?));
            }
            "--scenario" => scenario = it.next().context("--scenario needs a name")?.clone(),
            bad => bail!(
                "web-selftest: unknown argument {bad:?} (allowed: --release, --fixture <dir>, --scenario <name|all>)"
            ),
        }
    }
    let fixture = fixture
        .or_else(|| std::env::var_os("REPLICA_FIXTURE_OUT").map(PathBuf::from))
        .context("web-selftest needs the fixture of the replica test: --fixture <dir> (or REPLICA_FIXTURE_OUT); make it with `cargo test -p replica --test real_replica` on Linux/WSL with REPLICA_FIXTURE_OUT=<dir>")?;
    let scenarios: Vec<String> = if scenario == "all" {
        ["tiny", "small", "late-huge"]
            .into_iter()
            .filter(|s| fixture.join(s).join("script.json").is_file())
            .map(str::to_owned)
            .collect()
    } else {
        vec![scenario]
    };
    for s in &scenarios {
        if !fixture.join(s).join("script.json").is_file() {
            bail!(
                "web-selftest: {} has no script.json",
                fixture.join(s).display()
            );
        }
    }
    let web = build_web_with(root, release, &["replica-selftest"], "web-selftest")?;
    // The same self-test in the threaded build (the game core with atomics, a shared memory): digests must be the same.
    let mt = GAMECORE_ENV.get().is_some_and(|e| {
        e.iter()
            .any(|(v, _)| *v == "GAMECORE_LIB_DIR_wasm32_threads")
    }) || std::env::var_os("GAMECORE_LIB_DIR_wasm32_threads").is_some();
    if mt {
        let wasm = cargo_build_wasm_mt(root, release, &["replica-selftest"])?;
        bindgen_pkg(root, &wasm, &web.join("pkg-mt"), release)?;
    } else {
        println!(
            "note: web-selftest: no wasm-mt game core (--gamecore-lib <dir> with wasm-mt/), the threaded build is not tested"
        );
    }
    let mut c = Command::new("node");
    c.arg(root.join("tools/web-selftest/run.mjs"))
        .arg(&web)
        .arg("--fixture")
        .arg(&fixture)
        .arg("--scenarios")
        .arg(scenarios.join(","));
    if mt {
        c.arg("--mt");
    }
    run(root, c)
}

/// Minimum binaryen release for the threaded wasm (atomics, bulk memory, exnref EH). Dev machine: version 123.
const WASM_OPT_MIN_VERSION: u32 = 116;

/// `CIV5_WASM_OPT` (full path to wasm-opt) or `wasm-opt` from PATH.
fn wasm_opt_path() -> String {
    std::env::var("CIV5_WASM_OPT").unwrap_or_else(|_| "wasm-opt".into())
}

fn check_wasm_opt(tool: &str) -> Result<()> {
    let hint = "Install binaryen >= 116 (https://github.com/WebAssembly/binaryen/releases, tested with version_123), \
put wasm-opt on PATH or set CIV5_WASM_OPT=<path to wasm-opt>; CIV5_SKIP_WASM_OPT=1 builds an unoptimised release.";
    let out = Command::new(tool).arg("--version").output().map_err(|_| {
        anyhow!("wasm-opt not found ({tool}); release/deploy builds must optimise the wasm. {hint}")
    })?;
    let text = String::from_utf8_lossy(&out.stdout);
    let ver: Option<u32> = text
        .split_whitespace()
        .skip_while(|w| *w != "version")
        .nth(1)
        .and_then(|v| v.parse().ok());
    match ver {
        Some(v) if v >= WASM_OPT_MIN_VERSION => Ok(()),
        _ => bail!(
            "wasm-opt is too old or unreadable ({}). {hint}",
            text.trim()
        ),
    }
}

fn which(tool: &str) -> bool {
    Command::new(tool).arg("--version").output().is_ok()
}

fn build_native(root: &Path, release: bool) -> Result<()> {
    let mut c = cargo();
    // xtask is not rebuilt: a running exe can't be overwritten on Windows.
    c.args(["build", "-p", "app-desktop", "-p", "server"]);
    if release {
        c.arg("--release");
    }
    run(root, c)
}

fn cmd_build(root: &Path, args: &[String]) -> Result<()> {
    let mut only: Option<String> = None;
    let mut release = false;
    // Threads by default (B3 render-mt, the user 2026-10-29): `pkg-mt` beside `pkg`; `--single` builds `pkg` only.
    let mut threads = true;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--release" => release = true,
            "--threads" => threads = true,
            "--single" => threads = false,
            "--only" => {
                only = Some(
                    it.next()
                        .cloned()
                        .context("--only requires a value: web or native")?,
                )
            }
            s if s.starts_with("--only=") => only = Some(s["--only=".len()..].to_string()),
            other => {
                bail!(
                    "build: unknown argument {other:?} (allowed: --only web|native, --release, --threads (default), --single)"
                )
            }
        }
    }
    let web = |root: &Path| {
        let dir = if threads {
            build_web_threads(root, release)?
        } else {
            build_web(root, release)?
        };
        ui::web::build_skins(root, &dir)?;
        Ok::<_, anyhow::Error>(dir)
    };
    match only.as_deref() {
        None => {
            build_native(root, release)?;
            web(root)?;
        }
        Some("native") => build_native(root, release)?,
        Some("web") => {
            web(root)?;
        }
        Some(x) => bail!("build: --only {x:?}: expected web or native"),
    }
    Ok(())
}

fn check_wasm_imports(root: &Path) -> Result<()> {
    cargo_build_wasm(root, false)?;
    let mut files = vec![raw_wasm_path(root, false)?];
    match bindgen_cli_problem(root)? {
        None => {
            let pkg = target_dir(root)?.join("web/pkg");
            let mut c = Command::new("wasm-bindgen");
            c.args(["--target", "web", "--out-name", WEB_OUT_NAME, "--out-dir"])
                .arg(&pkg)
                .arg(&files[0]);
            run(root, c)?;
            patch_glue(&pkg)?;
            files.push(pkg.join(format!("{WEB_OUT_NAME}_bg.wasm")));
        }
        Some(p) => println!("note: post-bindgen .wasm check skipped: {p}"),
    }
    let mut bad = Vec::new();
    for f in &files {
        let bytes = std::fs::read(f).with_context(|| format!("failed to read {}", f.display()))?;
        for (m, field) in checks::forbidden_imports(&bytes, WEB_OUT_NAME)? {
            bad.push(format!("{}: forbidden import `{m}`.`{field}`", f.display()));
        }
        println!("imports {}: checked", f.display());
    }
    if !bad.is_empty() {
        bail!(
            "forbidden .wasm imports (only wasm-bindgen modules are allowed; WASI and `env` must be implemented inside the module):\n  {}",
            bad.join("\n  ")
        );
    }
    Ok(())
}

/// Lekmod (gamecore) checkout: `LEKMOD_ROOT`, else `<workspace>/../Lekmod` when it exists.
/// Only the real gamecore build and the ABI header comparison need it.
fn lekmod_root_opt(root: &Path) -> Option<PathBuf> {
    match std::env::var_os("LEKMOD_ROOT") {
        Some(r) => Some(PathBuf::from(r)),
        None => root
            .parent()
            .map(|p| p.join("Lekmod"))
            .filter(|p| p.is_dir()),
    }
}

fn lekmod_root(root: &Path) -> Result<PathBuf> {
    lekmod_root_opt(root)
        .context("Lekmod (gamecore) checkout not found: set LEKMOD_ROOT or place it at <workspace>/../Lekmod")
}

/// WSL distribution used by the WSL build paths: `CIV5_WSL_DISTRO`, default `Ubuntu-24.04`.
pub(crate) fn wsl_distro() -> String {
    std::env::var("CIV5_WSL_DISTRO").unwrap_or_else(|_| "Ubuntu-24.04".into())
}

const GC_HEADER_OURS: &str = "crates/shared/gamecore-sys/cpp/include/gamecore_abi.h";
const GC_HEADER_LEKMOD: &str = "headless/portable/abi/gamecore_abi.h";

/// The ABI header copy in `gamecore-sys` must be byte-identical to the one in the gamecore checkout.
/// Skipped with a notice when no checkout is available (`LEKMOD_ROOT` unset and no `../Lekmod`).
fn check_gc_header(root: &Path) -> Result<()> {
    let Some(lekmod) = lekmod_root_opt(root) else {
        println!(
            "notice: gamecore ABI header comparison skipped: no Lekmod checkout (set LEKMOD_ROOT to compare {GC_HEADER_OURS} with {GC_HEADER_LEKMOD})"
        );
        return Ok(());
    };
    // Components re-joined: one separator style in the messages.
    let theirs_path: PathBuf = lekmod.join(GC_HEADER_LEKMOD).components().collect();
    let ours_path: PathBuf = root.join(GC_HEADER_OURS).components().collect();
    println!(
        "Lekmod: {} (LEKMOD_ROOT or <workspace>/../Lekmod)",
        lekmod.display()
    );
    let theirs = std::fs::read(&theirs_path).with_context(|| {
        format!(
            "cannot read {}: Lekmod checkout {} has no ABI header (set LEKMOD_ROOT to a gamecore checkout that has this header)",
            theirs_path.display(),
            lekmod.display()
        )
    })?;
    let ours = std::fs::read(&ours_path)
        .with_context(|| format!("cannot read {}", ours_path.display()))?;
    if let Err(diff) = checks::compare_headers(&ours, &theirs) {
        bail!(
            "gamecore_abi.h differs from Lekmod: {diff}.\n  \
             Refresh the copy:  Copy-Item -Force '{}' '{}'   (PowerShell; on POSIX: cp)\n  \
             then mirror the header change in crates/shared/gamecore-sys/src/ffi.rs and cpp/stub/gamecore_stub.cpp.\n  \
             If the copy is newer than your Lekmod checkout, point LEKMOD_ROOT at the checkout with the new header.",
            theirs_path.display(),
            ours_path.display()
        );
    }
    println!(
        "gamecore_abi.h: identical to {} (fnv1a64 {:016x})",
        theirs_path.display(),
        checks::header_hash(&ours)
    );
    Ok(())
}

fn check_game_files(root: &Path) -> Result<()> {
    let out = Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(root)
        .output()
        .context("failed to start git ls-files")?;
    if !out.status.success() {
        bail!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let bad = checks::forbidden_game_files(text.split('\0').filter(|s| !s.is_empty()));
    if !bad.is_empty() {
        bail!(
            "game files in git index (S8), forbidden outside third_party/:\n  {}",
            bad.join("\n  ")
        );
    }
    println!("game files in git index: none found");
    Ok(())
}

/// GH-D2: the gateway code does not build a core; only `crates/server/server/src/worker/` may.
fn check_gateway_without_brain(root: &Path) -> Result<()> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
        for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
            let path = entry?.path();
            if path.is_dir() {
                walk(&path, out)?;
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
        Ok(())
    }
    let mut paths = Vec::new();
    walk(&root.join("crates/server/server/src"), &mut paths)?;
    let mut texts = Vec::new();
    for p in paths {
        let rel = p
            .strip_prefix(root)
            .unwrap_or(&p)
            .to_string_lossy()
            .replace('\\', "/");
        texts.push((
            rel,
            std::fs::read_to_string(&p).with_context(|| p.display().to_string())?,
        ));
    }
    let bad = checks::brain_in_gateway(texts.iter().map(|(p, t)| (p.as_str(), t.as_str())));
    if !bad.is_empty() {
        bail!(
            "the gateway names the brain outside src/worker/ (GH-D2):
  {}",
            bad.join(
                "
  "
            )
        );
    }
    println!("gateway without a brain: ok");
    Ok(())
}

fn cmd_check(root: &Path) -> Result<()> {
    let mut failures: Vec<String> = Vec::new();
    let mut step = |name: &str, r: Result<()>| match r {
        Ok(()) => println!("== {name}: ok"),
        Err(e) => {
            println!("== {name}: FAILED\n{e:#}");
            failures.push(name.to_string());
        }
    };

    step("fmt", fmt_check(root));
    // R8 native: clippy over the whole workspace builds everything that must build natively.
    step("clippy (native, R8)", {
        let mut c = cargo();
        c.args([
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ]);
        run(root, c)
    });
    // R8 wasm32: clippy over the wasm layer crates.
    let plan = analyze_deps(root);
    match &plan {
        Ok((_, g)) => {
            let plan = graph::r8_plan(g);
            step("clippy (wasm32, R8)", {
                let mut c = cargo();
                c.args(["clippy", "--target", WASM_TARGET]);
                for n in &plan.wasm {
                    c.args(["-p", n]);
                }
                c.args(["--", "-D", "warnings"]);
                run(root, c)
            });
        }
        Err(_) => step(
            "clippy (wasm32, R8)",
            Err(anyhow::anyhow!("dependency graph not built, see deps")),
        ),
    }
    step(
        "deps (R1–R7, R9, R10)",
        match plan {
            Ok((problems, _)) if problems.is_empty() => Ok(()),
            Ok((problems, _)) => {
                for p in &problems {
                    println!("VIOLATION {p}");
                }
                Err(anyhow::anyhow!("violations found: {}", problems.len()))
            }
            Err(e) => Err(e),
        },
    );
    step("game files (S8)", check_game_files(root));
    step("ui model generated files", ui::check_generated(root));
    step(
        "gateway without a brain (GH-D2)",
        check_gateway_without_brain(root),
    );
    step("gamecore ABI header", check_gc_header(root));
    step("wasm imports", check_wasm_imports(root));

    if failures.is_empty() {
        println!("check: all green");
        Ok(())
    } else {
        bail!("check: failed: {}", failures.join(", "));
    }
}

/// `cargo test` arguments: the whole workspace, unless the caller picked packages with `-p`.
fn test_args(extra: &[String]) -> Vec<String> {
    let picked = extra.iter().any(|a| {
        a == "-p"
            || a == "--package"
            || a.starts_with("--package=")
            || (a.starts_with("-p") && a.len() > 2 && !a.starts_with("--"))
    });
    let mut args = vec!["test".to_string()];
    if !picked && !has_flag(extra, "--workspace") {
        args.push("--workspace".into());
    }
    args.extend(extra.iter().cloned());
    args
}

fn cmd_test(root: &Path, extra: &[String]) -> Result<()> {
    let mut c = cargo();
    c.args(test_args(extra));
    run(root, c)
}

fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter()
        .any(|a| a == flag || a.starts_with(&format!("{flag}=")))
}

fn cmd_dev(root: &Path, extra: &[String]) -> Result<()> {
    // `--wsl` is ours; `--dev` is always on (the server flag is passed once below).
    let wsl = has_flag(extra, "--wsl");
    let extra: Vec<String> = extra
        .iter()
        .filter(|a| *a != "--wsl" && *a != "--dev")
        .cloned()
        .collect();
    // `--mods <file>` (and `--mod-packs <dir>`): the dist of that list instead of the public server and client.
    // `--config <file>` is merged with `mods_dir` into a derived file; the caller's file is not written.
    let mods::dev::ModsArgs {
        list: mods_list,
        packs: mod_packs,
        config: user_config,
        rest: extra,
    } = mods::dev::take_args(&extra)?;
    let extra = &extra[..];
    let dist = match &mods_list {
        Some(list) => Some(mods::dev::prepare_dev(
            root,
            list,
            mod_packs.as_deref(),
            wsl,
            user_config.as_deref(),
        )?),
        None => None,
    };
    let web = match &dist {
        Some(d) => d.web.clone(),
        None => build_web(root, false)?,
    };
    // Server flags: `--dev` (the `dev` password, `target/dev-state`, the test world), `--core
    // fixture|real` (real with `--wsl`), `--state-dir`, `--listen`, `--web-root`, `--assets-root`.
    let mut flags: Vec<String> = vec!["--dev".into()];
    if !has_flag(extra, "--core") {
        flags.extend(["--core".into(), if wsl { "real" } else { "fixture" }.into()]);
    }
    if !has_flag(extra, "--listen") {
        flags.extend(["--listen".into(), "127.0.0.1:8080".into()]);
    }
    // `--assets <dir>` is the friendly spelling of the server's `--assets-root`.
    let mut passthrough: Vec<String> = Vec::new();
    let mut assets: Option<PathBuf> = None;
    // `--gallery <dir>`: the test world with one unit of every type of `<dir>/units.csv` and one landmark
    // of every row of `<dir>/landmarks.csv` (an asset report directory, e.g. `assets-out/b2-rp27`).
    let mut gallery: Option<PathBuf> = None;
    let mut it = extra.iter();
    while let Some(a) = it.next() {
        if a == "--assets" {
            let v = it.next().context("--assets requires a directory")?;
            assets = Some(PathBuf::from(v));
        } else if let Some(v) = a.strip_prefix("--assets=") {
            assets = Some(PathBuf::from(v));
        } else if a == "--gallery" {
            let v = it.next().context("--gallery requires a directory")?;
            gallery = Some(PathBuf::from(v));
        } else if let Some(v) = a.strip_prefix("--gallery=") {
            gallery = Some(PathBuf::from(v));
        } else {
            passthrough.push(a.clone());
        }
    }
    if assets.is_none() && !has_flag(&passthrough, "--assets-root") {
        let default = root.join("../assets-out");
        if default.is_dir() {
            assets = Some(default);
        }
    }
    let assets_dir = if let Some(dir) = assets {
        Some(dir)
    } else if !has_flag(&passthrough, "--assets-root") {
        let empty = target_dir(root)?.join("assets-empty");
        std::fs::create_dir_all(&empty)?;
        Some(empty)
    } else {
        None
    };
    if !wsl {
        let mut c = cargo();
        match &dist {
            Some(d) => {
                c.arg("run").arg("--manifest-path").arg(&d.manifest);
                c.args(["-p", "dist-server", "--bin", "civ5-server", "--"]);
                c.arg("--config").arg(&d.config);
            }
            None => {
                c.args(["run", "-p", "server", "--bin", "civ5-server", "--"]);
            }
        }
        c.args(&flags);
        c.arg("--web-root").arg(&web);
        if let Some(dir) = assets_dir {
            c.arg("--assets-root").arg(dir);
        }
        c.args(passthrough);
        if let Some(dir) = gallery {
            c.env("CIV5_FIXTURE_GALLERY", std::fs::canonicalize(dir)?);
        }
        return run(root, c);
    }
    if gallery.is_some() {
        bail!("dev --gallery: the full set is a fixture mode; it does not run with --wsl");
    }
    if !has_flag(&passthrough, "--gamedata-dir")
        && flag_value(&flags, "--core")?
            .or(flag_value(&passthrough, "--core")?)
            .as_deref()
            == Some("real")
    {
        bail!(
            "dev --wsl: the real core needs the game data: add --gamedata-dir <dir> (a game files copy, not in git)              and --gamecore-lib <dir> with a linux/libgamecore.a"
        );
    }
    // WSL: the gateway with the real brain runs in Linux (the gamecore is a Linux library); the port is
    // forwarded to Windows, so `http://localhost:8080/healthz` answers from Windows too.
    let target = target_dir(root)?;
    // The dist (`--mods`) has its own workspace, so its own target directory (`target/dist-wsl/target`).
    let wsl_target = match &dist {
        Some(d) => d
            .manifest
            .parent()
            .context("dist manifest has no directory")?
            .join("target"),
        None => target.join("wsl"),
    };
    let to_wsl = |p: &Path| windows_to_wsl_path(&p.to_string_lossy());
    std::fs::create_dir_all(&wsl_target)?;
    let mut script = format!(
        "export CARGO_TARGET_DIR={} && cd {}",
        sh_quote(&to_wsl(&std::fs::canonicalize(&wsl_target)?)?),
        sh_quote(&to_wsl(root)?)
    );
    for (var, path) in GAMECORE_ENV.get().into_iter().flatten() {
        let dir = std::fs::canonicalize(path)?;
        script.push_str(&format!(" && export {var}={}", sh_quote(&to_wsl(&dir)?)));
    }
    match &dist {
        Some(d) => {
            let manifest = std::fs::canonicalize(&d.manifest)?;
            let config = std::fs::canonicalize(&d.config)?;
            script.push_str(&format!(
                " && cargo run --manifest-path {} -p dist-server --bin civ5-server -- --config {}",
                sh_quote(&to_wsl(&manifest)?),
                sh_quote(&to_wsl(&config)?)
            ));
        }
        None => script.push_str(" && cargo run -p server --bin civ5-server --"),
    }
    for f in &flags {
        script.push(' ');
        script.push_str(&sh_quote(f));
    }
    // Paths in the flags handed over by the caller are Windows paths, absolute or relative to the
    // repository; they are made absolute and translated (a relative path would be read from the
    // worker's own directory in WSL). A path that starts with `/` is taken as a Linux path.
    let mut args = passthrough.iter().peekable();
    let mut paths = vec![("--web-root", web.clone())];
    if let Some(dir) = assets_dir {
        paths.push(("--assets-root", dir));
    }
    for (flag, p) in paths {
        script.push_str(&format!(
            " {flag} {}",
            sh_quote(&to_wsl(&std::fs::canonicalize(p)?)?)
        ));
    }
    while let Some(a) = args.next() {
        script.push(' ');
        let is_path_flag = matches!(
            a.as_str(),
            "--state-dir" | "--gamedata-dir" | "--assets-root"
        );
        script.push_str(&sh_quote(a));
        if is_path_flag && let Some(v) = args.next() {
            let v = if v.starts_with('/') {
                v.clone()
            } else {
                let p = root.join(v);
                to_wsl(&std::fs::canonicalize(&p).unwrap_or(p))?
            };
            script.push(' ');
            script.push_str(&sh_quote(&v));
        }
    }
    let mut c = Command::new("wsl");
    c.args(["-d", &wsl_distro(), "--", "bash", "-lc"])
        .arg(script);
    run(root, c)
}

fn cmd_run_desktop(root: &Path, args: &[String]) -> Result<()> {
    let args = args.strip_prefix(&["--".to_string()]).unwrap_or(args);
    let mut c = cargo();
    c.args(["run", "-p", "app-desktop", "--"]).args(args);
    run(root, c)
}

/// Value of `--name <v>` or `--name=<v>`.
fn flag_value(args: &[String], flag: &str) -> Result<Option<String>> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == flag {
            return it
                .next()
                .cloned()
                .map(Some)
                .with_context(|| format!("{flag} requires a value"));
        }
        if let Some(v) = a.strip_prefix(&format!("{flag}=")) {
            return Ok(Some(v.to_string()));
        }
    }
    Ok(None)
}

/// Forwards to the `golden` tool: `cargo xtask golden prep S1|S2|S3|S4|restore ...`.
fn cmd_golden(root: &Path, args: &[String]) -> Result<()> {
    let mut c = cargo();
    c.args(["run", "-p", "golden", "--release", "--"])
        .args(args);
    run(root, c)
}

/// Forwards to the `shader-re` tool: `cargo xtask shader-re extract|validate ...`.
fn cmd_shader_re(root: &Path, args: &[String]) -> Result<()> {
    let mut c = cargo();
    c.args(["run", "-p", "shader-re", "--release", "--"])
        .args(args);
    run(root, c)
}

/// Runs the asset pipeline. The default output dir is a sibling of the repo, outside git.
fn cmd_assets(root: &Path, args: &[String]) -> Result<()> {
    for a in args {
        let known = ["--game", "--game-dir", "--out", "--set", "--db"]
            .iter()
            .any(|f| a == f || a.starts_with(&format!("{f}=")));
        if a.starts_with("--") && !known {
            bail!(
                "assets: unknown argument {a:?} (allowed: --game <dir> (alias --game-dir; default: the Steam install), --out <dir>, --set <name>, --db <dir>)"
            );
        }
    }
    let game = match flag_value(args, "--game")? {
        Some(g) => Some(g),
        None => flag_value(args, "--game-dir")?,
    };
    let out = flag_value(args, "--out")?.unwrap_or_else(|| "../assets-out".into());
    let set = flag_value(args, "--set")?.unwrap_or_else(|| "warrior".into());
    let mut c = cargo();
    c.args(["run", "-p", "asset-pipeline", "--release", "--"]);
    if let Some(game) = game {
        c.args(["--game", &game]);
    }
    c.args(["build", "--out", &out, "--set", &set]);
    // The composite `b1` reads the game tables from the DB snapshot; without `--db` the
    // pipeline falls back to CIV5_DB_SNAPSHOT.
    if let Some(db) = flag_value(args, "--db")? {
        c.args(["--db", &db]);
    }
    run(root, c)
}

/// Removes `--gamecore-lib <dir>` from `args` and points the build at `<dir>/linux`, `<dir>/wasm` and `<dir>/wasm-mt`
/// (`GAMECORE_LIB_DIR_x86_64`, `GAMECORE_LIB_DIR_wasm32`; see `gamecore-sys/build.rs`).
fn take_gamecore_lib(args: &mut Vec<String>) -> Result<()> {
    let Some(dir) = flag_value(args, "--gamecore-lib")? else {
        return Ok(());
    };
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--gamecore-lib" {
            args.drain(i..=i + 1);
        } else if args[i].starts_with("--gamecore-lib=") {
            args.remove(i);
        } else {
            i += 1;
        }
    }
    let dir = std::path::absolute(&dir).with_context(|| format!("--gamecore-lib {dir}"))?;
    let mut env = Vec::new();
    for (var, sub) in [
        ("GAMECORE_LIB_DIR_x86_64", "linux"),
        ("GAMECORE_LIB_DIR_wasm32", "wasm"),
        // The same game core built with atomics and bulk memory, for the link of the threaded web build (pkg-mt).
        ("GAMECORE_LIB_DIR_wasm32_threads", "wasm-mt"),
    ] {
        let path = dir.join(sub);
        if path.join("libgamecore.a").is_file() {
            env.push((var, path));
        } else {
            println!(
                "note: {} has no libgamecore.a, {var} stays unset",
                path.display()
            );
        }
    }
    if env.is_empty() {
        bail!(
            "--gamecore-lib {}: neither linux/ nor wasm/ holds libgamecore.a (cargo xtask gamecore builds them)",
            dir.display()
        );
    }
    let _ = GAMECORE_ENV.set(env);
    Ok(())
}

/// Builds the web bundle (real gamecore) and the server, then runs `tools/playtest/run.mjs`
/// report in `<target>/playtest/report.{json,md}`.
fn cmd_playtest(root: &Path, args: &[String]) -> Result<()> {
    const VALUED: [&str; 38] = [
        "--out",
        "--scroll-s",
        // The `gallery` scenario: the asset report dir of the full-set test world (default: --assets).
        "--gallery",
        // The `golden` scenario: `golden render --backend webgpu|webgl2` runs it.
        "--golden-dir",
        "--golden-out",
        "--golden-captures",
        "--golden-backend",
        "--perf-runs",
        "--perf-turns",
        "--perf-idle-s",
        "--save-turns",
        "--after-load-turns",
        "--smoke-turns",
        "--simp-turns",
        "--scenario",
        "--turns",
        "--seed",
        "--ai",
        "--seeds",
        "--ais",
        "--port",
        "--assets",
        "--gfx",
        "--preset",
        "--simp-check",
        "--late-url",
        "--late-nick",
        "--late-token",
        // The `bench` scenario (B3 perf-bench).
        "--bench-modes",
        "--bench-units",
        "--bench-cities",
        "--bench-ai",
        "--bench-turns",
        "--bench-phase-s",
        "--bench-settle-s",
        "--bench-size",
        "--bench-label",
        "--bench-stats",
    ];
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let name = a.split('=').next().unwrap_or(a);
        if VALUED.contains(&name) {
            if !a.contains('=') {
                it.next()
                    .with_context(|| format!("playtest: {a} requires a value"))?;
            }
        } else if a != "--headed"
            && a != "--verbose"
            && a != "--release"
            && a != "--threads"
            && a != "--single"
        {
            bail!(
                "playtest: unknown argument {a:?} (allowed: --scenario <autoplay|saveload|smoke|fatal|simp|perf|terrain-scroll|perf-huge|perf-late|bench|quality|cold-start|golden|gallery|all>[,...], --scroll-s N, --gallery <report dir> (scenario gallery), --golden-dir <dir> --golden-out <dir> --golden-captures S1a,S1b --golden-backend webgpu|webgl2 (scenario golden), --release (optimised wasm bundle), --out <dir>, --perf-runs N, --perf-turns N, --perf-idle-s N, --late-url U --late-nick N --late-token F (scenario perf-late), --turns N, --seed N, --ai 1|2, --seeds 1,2,3, --ais 1,2, --save-turns N, --after-load-turns N, --smoke-turns N, --simp-turns N, --headed, --port N, --assets <dir>, --gfx webgl2, --preset low|medium|high, --simp-check <simp_check.py>, --verbose)"
            );
        }
    }
    let real_core = GAMECORE_ENV
        .get()
        .is_some_and(|e| e.iter().any(|(v, _)| *v == "GAMECORE_LIB_DIR_wasm32"))
        || std::env::var_os("GAMECORE_LIB_DIR_wasm32").is_some();
    if !real_core {
        bail!(
            "playtest needs the real gamecore: pass --gamecore-lib <dir> (with wasm/libgamecore.a; cargo xtask gamecore builds it)"
        );
    }
    let tool = root.join("tools/playtest");
    if !tool.join("node_modules/playwright/package.json").is_file() {
        bail!(
            "Playwright is not installed for the playtest: run `npm ci` in {} (installs the pinned version from package-lock.json; the browsers come from %LOCALAPPDATA%\\ms-playwright)",
            tool.display()
        );
    }
    let assets = match flag_value(args, "--assets")? {
        Some(a) => std::path::absolute(a)?,
        None => {
            let b1 = root.join("../assets-out/b1");
            if b1.join("manifest").is_file() {
                b1
            } else {
                root.join("../assets-out")
            }
        }
    };
    if !assets.join("manifest").is_file() {
        bail!(
            "playtest: {} has no `manifest` (cargo xtask assets --set b1 builds it); pass --assets <dir>",
            assets.display()
        );
    }
    let release = args.iter().any(|a| a == "--release");
    // Threads by default (B3 render-mt, the user 2026-10-29): the threaded build in the bundle and a
    // cross-origin isolated gateway. `--single` (or `CIV5_BENCH_SINGLE=1`): the single-threaded bundle alone
    // and a gateway without the isolation, the fallback a client without threads gets, for comparisons.
    let single = args.iter().any(|a| a == "--single")
        || std::env::var("CIV5_BENCH_SINGLE").is_ok_and(|v| !v.is_empty() && v != "0");
    let web = if single {
        build_web(root, release)?
    } else {
        build_web_threads(root, release)?
    };
    let mut c = cargo();
    c.args(["build", "-p", "server"]);
    run(root, c)?;
    let target = target_dir(root)?;
    let out = match flag_value(args, "--out")? {
        Some(o) => std::path::absolute(o)?,
        None => target.join("playtest"),
    };
    let server = target
        .join("debug")
        .join(format!("civ5-server{}", std::env::consts::EXE_SUFFIX));
    let mut c = Command::new("node");
    c.arg(tool.join("run.mjs"))
        .arg("--web-root")
        .arg(&web)
        .arg("--server")
        .arg(&server)
        .arg("--out")
        .arg(&out)
        .arg("--assets")
        .arg(&assets)
        .arg("--profile")
        .arg(if release { "release" } else { "debug" });
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--assets" || a == "--out" {
            it.next();
        } else if !a.starts_with("--assets=")
            && !a.starts_with("--out=")
            && a != "--release"
            && a != "--threads"
            && a != "--single"
        {
            c.arg(a);
        }
    }
    c.arg(if single { "--single" } else { "--threads" });
    run(root, c)
}

/// `cargo xtask bench [--compare <baseline.json>] [playtest flags]`: the heavy-scene render benchmark
/// (`playtest --scenario bench --headed --release`, report in `<target>/bench/logs/bench/bench.json`) and, when a baseline
/// exists, its comparison (`tools/playtest/bench-compare.mjs`; exit code 1 on a regression beyond tolerance).
/// The baseline defaults to `tools/playtest/baselines/b3-baseline.json`, which the repository does not ship: record your own
/// there (or pass `--compare <file>`); without one the bench only reports. `--no-compare` skips the comparison.
fn cmd_bench(root: &Path, args: &[String]) -> Result<()> {
    let mut rest: Vec<String> = Vec::new();
    let mut baseline: Option<PathBuf> = None;
    let mut compare = true;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--compare" {
            baseline = Some(std::path::absolute(
                it.next().context("bench: --compare requires a file")?,
            )?);
        } else if let Some(v) = a.strip_prefix("--compare=") {
            baseline = Some(std::path::absolute(v)?);
        } else if a == "--no-compare" {
            compare = false;
        } else {
            rest.push(a.clone());
        }
    }
    let target = target_dir(root)?;
    let out = match flag_value(&rest, "--out")? {
        Some(o) => std::path::absolute(o)?,
        None => target.join("bench"),
    };
    let mut pt = vec!["--scenario".to_string(), "bench".to_string()];
    if !rest.iter().any(|a| a == "--headed") {
        pt.push("--headed".into());
    }
    if !rest.iter().any(|a| a == "--release") {
        pt.push("--release".into());
    }
    if flag_value(&rest, "--out")?.is_none() {
        pt.push("--out".into());
        pt.push(out.display().to_string());
    }
    pt.extend(rest);
    let played = cmd_playtest(root, &pt);
    let current = out.join("logs").join("bench").join("bench.json");
    if compare && current.is_file() {
        let base =
            baseline.unwrap_or_else(|| root.join("tools/playtest/baselines/b3-baseline.json"));
        if base.is_file() {
            let mut c = Command::new("node");
            c.arg(root.join("tools/playtest/bench-compare.mjs"))
                .arg(&current)
                .arg(&base);
            let compared = run(root, c);
            played?;
            return compared;
        }
        println!(
            "bench: no baseline at {} (pass --compare <file>)",
            base.display()
        );
    }
    played
}

/// Flags of `nettest` that take a value / that do not.
const NETTEST_VALUED: [&str; 17] = [
    "--scenario",
    "--profile",
    "--core",
    "--link",
    "--attach",
    "--admin-via",
    "--password-file",
    "--seed",
    "--out",
    "--turns",
    "--bots",
    "--netproxy",
    "--gamedata-dir",
    "--bot",
    "--web-root",
    "--assets",
    "--bg-minutes",
];
const NETTEST_PLAIN: [&str; 8] = [
    "--quick",
    "--full",
    "--acceptance",
    "--keep-state",
    "--verbose",
    "--list",
    "--wsl",
    "--release",
];

/// What `nettest` does with its arguments: the ones for the runner, and the build switches.
#[derive(Debug, PartialEq, Eq)]
struct NettestPlan {
    runner: Vec<String>,
    wsl: bool,
    release: bool,
    attach: bool,
}

/// The scenarios that drive the real client in a browser: they need the web bundle served by the server under test.
const NETTEST_BROWSER: [&str; 5] = ["optimistic", "prefetch", "reload", "background", "b1-smoke"];

/// True when the `--scenario` list of the runner arguments has a browser scenario (or `all`).
fn nettest_wants_web(runner: &[String]) -> bool {
    let list = flag_value(runner, "--scenario").ok().flatten();
    let Some(list) = list else { return false };
    list.split(',')
        .map(str::trim)
        .any(|n| n == "all" || NETTEST_BROWSER.contains(&n))
}

fn nettest_plan(args: &[String]) -> Result<NettestPlan> {
    let mut plan = NettestPlan {
        runner: Vec::new(),
        wsl: false,
        release: false,
        attach: false,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let name = a.split('=').next().unwrap_or(a);
        if NETTEST_VALUED.contains(&name) {
            plan.attach |= name == "--attach";
            plan.runner.push(a.clone());
            if !a.contains('=') {
                let v = it
                    .next()
                    .with_context(|| format!("nettest: {a} requires a value"))?;
                plan.runner.push(v.clone());
            }
        } else if NETTEST_PLAIN.contains(&a.as_str()) {
            match a.as_str() {
                "--wsl" => plan.wsl = true,
                "--release" => plan.release = true,
                _ => plan.runner.push(a.clone()),
            }
        } else {
            bail!(
                "nettest: unknown argument {a:?} (allowed: {}, {})",
                NETTEST_VALUED.join(" <v>, "),
                NETTEST_PLAIN.join(", ")
            );
        }
    }
    Ok(plan)
}

/// The test stand: builds the server and the bot, then runs
/// `tools/net-test/runner/run.mjs`; the report is `<target>/net-test/report.{json,md}`.
/// `--wsl` builds and runs the server in WSL (real core; the WSL path is not exercised by CI).
fn cmd_nettest(root: &Path, args: &[String]) -> Result<()> {
    let plan = nettest_plan(args)?;
    let runner = root.join("tools/net-test/runner/run.mjs");
    if !runner.is_file() {
        bail!("nettest: {} not found", runner.display());
    }
    let target = target_dir(root)?;
    let profile = profile_dir(plan.release);
    let exe = |name: &str, dir: &Path| {
        dir.join(profile)
            .join(format!("{name}{}", std::env::consts::EXE_SUFFIX))
    };
    let (server, bot, netproxy);
    if plan.wsl {
        // A separate target dir: the Windows and the Linux builds must not meet in one.
        let wsl_target = target.join("wsl");
        let to_wsl = |p: &Path| windows_to_wsl_path(&p.to_string_lossy());
        let mut script = format!(
            "export CARGO_TARGET_DIR={} && cd {}",
            sh_quote(&to_wsl(&wsl_target)?),
            sh_quote(&to_wsl(root)?)
        );
        for (var, path) in GAMECORE_ENV.get().into_iter().flatten() {
            let dir = std::fs::canonicalize(path)?;
            script.push_str(&format!(" && export {var}={}", sh_quote(&to_wsl(&dir)?)));
        }
        script.push_str(" && cargo build -p server -p net-test");
        if plan.release {
            script.push_str(" --release");
        }
        let mut c = Command::new("wsl");
        c.args(["-d", &wsl_distro(), "--", "bash", "-lc"])
            .arg(script);
        run(root, c)?;
        let linux = |name: &str| wsl_target.join(profile).join(name);
        server = linux("civ5-server");
        bot = linux("net-bot");
        netproxy = linux("netproxy");
    } else {
        let mut c = cargo();
        c.args(["build", "-p", "net-test"]);
        if !plan.attach {
            c.args(["-p", "server"]);
        }
        if plan.release {
            c.arg("--release");
        }
        run(root, c)?;
        server = exe("civ5-server", &target);
        bot = exe("net-bot", &target);
        netproxy = exe("netproxy", &target);
    }
    let mut c = Command::new("node");
    c.arg(&runner);
    if !plan.attach {
        c.arg("--server").arg(&server);
    }
    c.arg("--bot").arg(&bot);
    // The browser scenarios: the release or debug web bundle and the assets the server serves with it (a deployed
    // service brings its own).
    let mut runner_args = plan.runner.clone();
    if !plan.attach && nettest_wants_web(&plan.runner) {
        if !has_flag(&runner_args, "--web-root") {
            let web = build_web(root, plan.release)?;
            runner_args.push("--web-root".into());
            runner_args.push(web.to_string_lossy().into_owned());
        }
        if !has_flag(&runner_args, "--assets") {
            let b1 = root.join("../assets-out/b1");
            if !b1.join("manifest").is_file() {
                bail!(
                    "nettest: {} has no `manifest`; the browser scenarios need the assets of the game page (pass --assets <dir>, e.g. the b1 set)",
                    b1.display()
                );
            }
            runner_args.push("--assets".into());
            runner_args.push(b1.to_string_lossy().into_owned());
        }
    }
    if netproxy.is_file() {
        c.arg("--netproxy").arg(&netproxy);
    }
    if plan.wsl && !has_flag(&plan.runner, "--profile") {
        c.args(["--profile", "wsl"]);
    }
    if plan.wsl && !has_flag(&plan.runner, "--core") {
        c.args(["--core", "real"]);
    }
    if !has_flag(&plan.runner, "--out") {
        c.arg("--out").arg(target.join("net-test"));
    }
    c.args(&runner_args);
    run(root, c)
}

/// Builds the real gamecore archives with the fork's `build.sh` into `<out>/linux` and `<out>/wasm`.
///
/// `--pgo`: the Linux (server) archive is built profile-guided from the profile committed in the fork
/// (`headless/tools/clang/pgo/gamecore-linux.profdata`, `tools/pgo/train.sh` makes it). The wasm archives are not
/// profile-guided (the replica does not run the AI). PGO changes code layout only: the build id and the saves are
/// the same as without it, so it is a switch of the build, not of the game.
fn cmd_gamecore(root: &Path, args: &[String]) -> Result<()> {
    let pgo = args.iter().any(|a| a == "--pgo");
    for a in args {
        let known = ["--out", "--target"]
            .iter()
            .any(|f| a == f || a.starts_with(&format!("{f}=")));
        if a.starts_with("--") && !known && a != "--pgo" {
            bail!(
                "gamecore: unknown argument {a:?} (allowed: --out <dir>, --target linux|wasm|wasm-mt|all, --pgo)"
            );
        }
    }
    let out = match flag_value(args, "--out")? {
        Some(o) => std::path::absolute(o)?,
        None => target_dir(root)?.join("gamecore"),
    };
    let targets: Vec<&str> = match flag_value(args, "--target")?.as_deref() {
        // Windows has only the WSL build path; the Windows build links the wasm archive alone.
        // `wasm-mt`: the wasm archive with atomics and bulk memory for the threaded web build (`build.sh --threads`).
        None if cfg!(windows) => vec!["wasm", "wasm-mt"],
        None | Some("all") => vec!["linux", "wasm", "wasm-mt"],
        Some("linux") => vec!["linux"],
        Some("wasm") => vec!["wasm"],
        Some("wasm-mt") => vec!["wasm-mt"],
        Some(x) => bail!("gamecore: --target {x:?}: expected linux, wasm, wasm-mt or all"),
    };
    let lekmod = lekmod_root(root)?;
    let script = lekmod.join("headless/tools/clang/build.sh");
    if !script.is_file() {
        bail!(
            "{} not found (LEKMOD_ROOT={})",
            script.display(),
            lekmod.display()
        );
    }
    for target in targets {
        let target_out = out.join(target);
        let mut c;
        if cfg!(windows) {
            // build.sh needs a POSIX toolchain: run it inside WSL on /mnt/<drive> paths.
            std::fs::create_dir_all(&target_out)?;
            let real = |p: &Path| {
                std::fs::canonicalize(p).with_context(|| format!("canonicalize {}", p.display()))
            };
            let lekmod_wsl = windows_to_wsl_path(&real(&lekmod)?.to_string_lossy())?;
            let out_wsl = windows_to_wsl_path(&real(&target_out)?.to_string_lossy())?;
            c = Command::new("wsl");
            c.args(["-d", &wsl_distro(), "--", "bash", "-lc"])
                .arg(wsl_build_command(&lekmod_wsl, target, &out_wsl, pgo));
        } else {
            c = Command::new("bash");
            c.arg(&script)
                .args(["--mode", "lib", "--target", build_sh_target(target).0])
                .args(build_sh_target(target).1.split_whitespace())
                .args(pgo_args(target, pgo).split_whitespace())
                .arg("--out")
                .arg(&target_out);
        }
        run(root, c)?;
    }
    println!(
        "gamecore archives: {0}/{{linux,wasm,wasm-mt}}; use: cargo xtask test --gamecore-lib {0}",
        out.display()
    );
    Ok(())
}

/// Windows path (optionally `\\?\`-prefixed) -> `/mnt/<drive>/...`. Input must be canonical (junctions resolved).
fn windows_to_wsl_path(p: &str) -> Result<String> {
    let p = p.strip_prefix(r"\\?\").unwrap_or(p).replace('\\', "/");
    let b = p.as_bytes();
    if b.len() < 2 || !b[0].is_ascii_alphabetic() || b[1] != b':' || (b.len() > 2 && b[2] != b'/') {
        bail!("not a local drive path, cannot map into WSL: {p}");
    }
    Ok(format!(
        "/mnt/{}{}",
        (b[0] as char).to_ascii_lowercase(),
        &p[2..]
    ))
}

/// POSIX single-quoting.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The `bash -lc` script run inside WSL for one build.sh invocation.
fn wsl_build_command(lekmod: &str, target: &str, out: &str, pgo: bool) -> String {
    format!(
        "export WASI_SDK=\"${{WASI_SDK:-$HOME/wasi-sdk-33}}\" && cd {} && bash headless/tools/clang/build.sh --mode lib --target {target}{threads}{pgo} --out {}",
        sh_quote(lekmod),
        sh_quote(out),
        target = build_sh_target(target).0,
        threads = build_sh_target(target).1,
        pgo = pgo_args(target, pgo),
    )
}

/// ` --pgo` for the Linux archive when asked for (the wasm archives are never profile-guided), else nothing.
fn pgo_args(target: &str, pgo: bool) -> &'static str {
    if pgo && target == "linux" {
        " --pgo"
    } else {
        ""
    }
}

/// `build.sh` arguments of a `cargo xtask gamecore` target: `wasm-mt` is the wasm archive built with `--threads`.
fn build_sh_target(target: &str) -> (&str, &'static str) {
    if target == "wasm-mt" {
        ("wasm", " --threads")
    } else {
        (target, "")
    }
}

fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    take_gamecore_lib(&mut args)?;
    let cmd = args.first().cloned().unwrap_or_default();
    let rest = args.get(1..).unwrap_or(&[]);
    let root = workspace_root();
    match cmd.as_str() {
        "deps" => cmd_deps(&root),
        "check" => cmd_check(&root),
        "build" => cmd_build(&root, rest),
        "test" => cmd_test(&root, rest),
        "dev" => cmd_dev(&root, rest),
        "run-desktop" => cmd_run_desktop(&root, rest),
        "assets" => cmd_assets(&root, rest),
        "golden" => cmd_golden(&root, rest),
        "exe-api-coverage" => exe_api_coverage::cmd_exe_api_coverage(&root, rest),
        "shader-re" => cmd_shader_re(&root, rest),
        "gamecore" => cmd_gamecore(&root, rest),
        "playtest" => cmd_playtest(&root, rest),
        "web-selftest" => cmd_web_selftest(&root, rest),
        "bench" => cmd_bench(&root, rest),
        "nettest" => cmd_nettest(&root, rest),
        "mods" => mods::cmd_mods(&root, rest),
        "ui" => ui::cmd_ui(&root, rest),
        "dist" => deploy::cmd_dist(&root, rest).map(|_| ()),
        "deploy" => deploy::cmd_deploy(&root, rest),
        // Debug: list the import modules of an arbitrary .wasm.
        "wasm-imports" => {
            let file = rest.first().context("usage: wasm-imports <file.wasm>")?;
            let bytes = std::fs::read(file)?;
            let mut mods: Vec<String> = checks::parse_imports(&bytes)?
                .into_iter()
                .map(|(m, _)| m)
                .collect();
            mods.sort();
            mods.dedup();
            println!("import modules: {mods:?}");
            Ok(())
        }
        _ => {
            println!(
                "usage: cargo xtask <build [--only web|native] [--release]|check|test|deps|dev [--wsl] [--core fixture|real] [--state-dir <dir>] [--assets <dir>] [--listen <addr>]|run-desktop -- <args>|golden prep <S1|S2|S3|S4|restore>|exe-api-coverage --contexts <K1,K2,K3,core|file> [--assets <dir>] [--out <file>] [--strict]|shader-re <extract|validate [--all]>|assets [--game <dir>] [--out <dir>] [--set <name>] [--db <dir>]|gamecore [--out <dir>] [--target linux|wasm|wasm-mt|all] [--pgo]|web-selftest [--release] [--fixture <dir>] [--scenario tiny|small|late-huge|all]|bench [--compare <baseline.json>] [--no-compare] [playtest flags]|playtest [--scenario autoplay|saveload|smoke|fatal|simp|all] [--turns N] [--seed N] [--ai 1|2] [--headed] [--port N] [--assets <dir>]|dist [--skip-gamecore] [--pgo]|deploy --target <file.toml|name> [--dry-run] [--rollback] [--allow-core-change] [--assets <dir>] [--gamedata <dir>] [--tls internal] [--skip-dist] [--skip-smoke] [--pgo]|nettest [--scenario <id|a,b|quick|all>] [--quick|--full] [--wsl] [--core fixture|real] [--link none|proxy|netem] [--attach <url> [--admin-via docker:<ctx>:<ctr>]] [--acceptance] [--seed N] [--turns N] [--bots N] [--web-root <dir>] [--assets <dir>] [--bg-minutes N] [--out <dir>] [--verbose] [--release]>; build, check, test, dev, playtest, web-selftest accept --gamecore-lib <dir> (real gamecore archives in <dir>/linux, <dir>/wasm and <dir>/wasm-mt)"
            );
            bail!("unknown command: {cmd:?}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wsl_path_translation() {
        assert_eq!(
            windows_to_wsl_path(r"E:\Dev\x y\Lekmod").unwrap(),
            "/mnt/e/Dev/x y/Lekmod"
        );
        assert_eq!(windows_to_wsl_path(r"\\?\C:\a").unwrap(), "/mnt/c/a");
        assert_eq!(windows_to_wsl_path("D:").unwrap(), "/mnt/d");
        assert!(windows_to_wsl_path(r"\\server\share\a").is_err());
        assert!(windows_to_wsl_path("rel/path").is_err());
    }

    #[test]
    fn test_runs_the_workspace_unless_a_package_is_picked() {
        let v = |a: &[&str]| test_args(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(v(&[]), ["test", "--workspace"]);
        assert_eq!(
            v(&["--", "--nocapture"]),
            ["test", "--workspace", "--", "--nocapture"]
        );
        assert_eq!(v(&["-p", "shader-re"]), ["test", "-p", "shader-re"]);
        assert_eq!(v(&["-pshader-re"]), ["test", "-pshader-re"]);
        assert_eq!(v(&["--package=golden"]), ["test", "--package=golden"]);
        assert_eq!(
            v(&["--workspace", "--exclude", "x"]),
            ["test", "--workspace", "--exclude", "x"]
        );
    }

    #[test]
    fn nettest_arguments_split_into_build_switches_and_runner_arguments() {
        let v = |a: &[&str]| nettest_plan(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        let p = v(&["--scenario", "m1", "--quick", "--wsl", "--turns=5"]).unwrap();
        assert_eq!(p.runner, ["--scenario", "m1", "--quick", "--turns=5"]);
        assert!(p.wsl && !p.release && !p.attach);
        let p = v(&["--attach", "http://h:1", "--release"]).unwrap();
        assert_eq!(p.runner, ["--attach", "http://h:1"]);
        assert!(p.attach && p.release);
        assert!(v(&["--scenario"]).is_err());
        assert!(v(&["--bogus"]).is_err());
    }

    #[test]
    fn nettest_serves_the_web_bundle_to_browser_scenarios_only() {
        let w =
            |a: &[&str]| nettest_wants_web(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert!(w(&["--scenario", "m1,reload"]));
        assert!(w(&["--scenario=b1-smoke"]));
        assert!(w(&["--scenario", "all"]));
        assert!(!w(&["--scenario", "m1,latency"]));
        assert!(!w(&["--quick"]));
    }

    #[test]
    fn wsl_command_quoting() {
        let c = wsl_build_command("/mnt/e/a b/Lek'mod", "wasm", "/mnt/e/o", false);
        assert!(c.contains(
            r"cd '/mnt/e/a b/Lek'\''mod' && bash headless/tools/clang/build.sh --mode lib --target wasm --out '/mnt/e/o'"
        ));
        assert!(c.starts_with("export WASI_SDK="));
        let mt = wsl_build_command("/mnt/e/l", "wasm-mt", "/mnt/e/o", false);
        assert!(mt.contains("--target wasm --threads --out '/mnt/e/o'"));
    }

    #[test]
    fn pgo_is_for_the_linux_archive_only() {
        let lin = wsl_build_command("/mnt/e/l", "linux", "/mnt/e/o", true);
        assert!(lin.contains("--target linux --pgo --out '/mnt/e/o'"));
        let plain = wsl_build_command("/mnt/e/l", "linux", "/mnt/e/o", false);
        assert!(plain.contains("--target linux --out '/mnt/e/o'") && !plain.contains("--pgo"));
        // build.sh refuses --pgo for wasm: the wasm archives are built as before.
        for t in ["wasm", "wasm-mt"] {
            assert!(!wsl_build_command("/mnt/e/l", t, "/mnt/e/o", true).contains("--pgo"));
        }
    }

    #[test]
    fn the_glue_hands_the_first_trap_to_the_page() {
        let glue = "function __wbg_handle_catch(e) {\n    if (x) {\n        throw y;\n    }\n    getInt32ArrayMemory0()[__wbg_terminated_addr] = 1;\n    __wbg_call_abort_hook();\n    throw e;\n}\n";
        let out = hook_trap_in_glue(glue).expect("one termination line");
        assert!(out.contains(
            "    try { globalThis.__civ5OnTrap?.(e, getInt32ArrayMemory0()[__wbg_terminated_addr] !== 0); } catch (_) {}\n    getInt32ArrayMemory0()[__wbg_terminated_addr] = 1;\n"
        ));
        assert_eq!(out.matches(GLUE_TERMINATE_LINE).count(), 1);
        // Another glue (none, or two such lines) is left alone.
        assert!(hook_trap_in_glue("function f() {}").is_none());
        assert!(hook_trap_in_glue(&format!("{glue}{glue}")).is_none());
    }
}
