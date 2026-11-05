//! `golden` command line. `cargo xtask golden <cmd>` forwards here.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use golden::{camera, captures, scene};

/// Folder of this crate in the source tree (scenes, capture script).
fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// `golden prep S1|S2|S3|S4|restore [--mydocs <dir>] [--cache <dir>]` runs `capture/prep.ps1`.
fn cmd_prep(args: &[String]) -> Result<()> {
    let what = args
        .first()
        .context("prep needs S1|S2|S3|S4|restore")?
        .as_str();
    let mut ps = vec![
        "-NoProfile".to_string(),
        "-ExecutionPolicy".into(),
        "Bypass".into(),
        "-File".into(),
        crate_dir()
            .join("capture/prep.ps1")
            .to_string_lossy()
            .into_owned(),
    ];
    match what {
        "restore" => ps.push("-Restore".into()),
        s @ ("S1" | "S2" | "S3" | "S4") => {
            ps.push("-Scene".into());
            ps.push(s.into());
        }
        other => bail!("prep: unknown scene {other:?} (S1|S2|S3|S4|restore)"),
    }
    let mut it = args[1..].iter();
    while let Some(a) = it.next() {
        let target = match a.as_str() {
            "--mydocs" => "-MyDocs",
            "--cache" => "-CacheDir",
            other => bail!("prep: unknown argument {other:?} (--mydocs <dir>, --cache <dir>)"),
        };
        ps.push(target.into());
        ps.push(
            it.next()
                .with_context(|| format!("{a} needs a value"))?
                .clone(),
        );
    }
    let status = Command::new("powershell")
        .args(&ps)
        .status()
        .context("failed to start powershell")?;
    if !status.success() {
        bail!("prep.ps1 failed ({status})");
    }
    Ok(())
}

/// Splits `--flag value` pairs out of `args`; the rest are positional.
fn take_flags(args: &[String], known: &[&str]) -> Result<(BTreeMap<String, String>, Vec<String>)> {
    let mut flags = BTreeMap::new();
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a.starts_with("--") {
            if !known.contains(&a.as_str()) {
                bail!("unknown argument {a:?} (known: {})", known.join(", "));
            }
            let v = it.next().with_context(|| format!("{a} needs a value"))?;
            flags.insert(a.clone(), v.clone());
        } else {
            rest.push(a.clone());
        }
    }
    Ok((flags, rest))
}

fn cache_arg(flags: &BTreeMap<String, String>) -> Result<PathBuf> {
    match flags.get("--cache") {
        Some(c) => Ok(PathBuf::from(c)),
        None => captures::default_cache(),
    }
}

/// `golden parse [S1a S1b ...] [--targets none|world-top3|12,40] [--cache <dir>]`
/// `golden parse --model <file.json> --scene S1 --capture S1a`   (capture-equivalent JSON, no RenderDoc)
fn cmd_parse(args: &[String]) -> Result<()> {
    let (flags, names) = take_flags(
        args,
        &["--targets", "--cache", "--model", "--scene", "--capture"],
    )?;
    let cache = cache_arg(&flags)?;
    let cfg = captures::RenderdocConfig::load(&cache)?;
    let opts = captures::ParseOpts {
        targets: flags.get("--targets").cloned(),
    };
    if let Some(model) = flags.get("--model") {
        let scene = flags.get("--scene").context("--model needs --scene")?;
        let capture = flags.get("--capture").context("--model needs --capture")?;
        let meta = captures::parse_capture(
            &cfg,
            &cache,
            scene,
            capture,
            captures::Source::Model(Path::new(model)),
            &opts,
        )?;
        println!("parsed {scene}/{capture}: {} draws", meta.draw_count);
        return Ok(());
    }
    let scenes_dir = crate_dir().join("scenes");
    let mut todo = Vec::new();
    for id in scene::SCENE_IDS {
        let s = scene::Scene::load_by_id(&scenes_dir, id)?;
        for name in &s.scene.captures {
            let wanted = names.is_empty() || names.contains(name);
            let rdc = s.capture_path(&cache, name);
            if wanted && rdc.is_file() {
                todo.push((id, name.clone(), rdc));
            } else if wanted && !names.is_empty() {
                bail!(
                    "{} does not exist (capture {name} is not taken yet)",
                    rdc.display()
                );
            }
        }
    }
    for n in &names {
        if !todo.iter().any(|(_, name, _)| name == n) {
            bail!("unknown capture {n:?}");
        }
    }
    if todo.is_empty() {
        println!(
            "parse: no .rdc files in {}",
            cache.join("captures").display()
        );
        return Ok(());
    }
    for (id, name, rdc) in todo {
        let meta =
            captures::parse_capture(&cfg, &cache, id, &name, captures::Source::Rdc(&rdc), &opts)
                .with_context(|| format!("capture {name}"))?;
        println!(
            "parsed {id}/{name}: {} events, {} draws ({} world), UI boundary {}",
            meta.event_count,
            meta.draw_count,
            meta.world_draw_count,
            meta.ui_boundary
                .as_ref()
                .map_or("not found".to_string(), |u| format!(
                    "event {} ({})",
                    u.event, u.reason
                )),
        );
    }
    Ok(())
}

/// `golden check-captures [--cache <dir>]`
fn cmd_check_captures(args: &[String]) -> Result<()> {
    let (flags, rest) = take_flags(args, &["--cache"])?;
    if !rest.is_empty() {
        bail!("check-captures takes no positional arguments");
    }
    let cache = cache_arg(&flags)?;
    let report = captures::check_captures(&cache, &crate_dir().join("scenes"))?;
    for n in &report.notes {
        println!("note: {n}");
    }
    for c in &report.ok {
        println!("ok: {c}");
    }
    for (c, p) in &report.problems {
        println!("PROBLEM {c}: {p}");
    }
    if !report.passed() {
        bail!("check-captures: {} problem(s)", report.problems.len());
    }
    println!("check-captures: all captures pass");
    Ok(())
}

fn floats<const N: usize>(s: &str, what: &str) -> Result<[f64; N]> {
    let v: Vec<f64> = s
        .split(',')
        .map(|x| x.trim().parse::<f64>())
        .collect::<std::result::Result<_, _>>()
        .with_context(|| format!("{what}: numbers separated by commas, got {s:?}"))?;
    v.try_into()
        .map_err(|_| anyhow::anyhow!("{what} needs {N} numbers, got {s:?}"))
}

/// `golden camera --shift X,Y,Z | (--terrain-scale SX,SY --half-world HW,HH --origin-texel U,V)`
/// `[--capture S1a] [--event N] [--eye-const NAME] [--cbmap-dir DIR] [--cache DIR] [--out FILE]`
fn cmd_camera(args: &[String]) -> Result<()> {
    use camera::{CameraOpts, OursToOrig, shift_from_terrain_constants};
    let (flags, rest) = take_flags(
        args,
        &[
            "--cache",
            "--shift",
            "--terrain-scale",
            "--half-world",
            "--origin-texel",
            "--capture",
            "--event",
            "--eye-const",
            "--cbmap-dir",
            "--out",
        ],
    )?;
    if !rest.is_empty() {
        bail!("camera takes only --flags");
    }
    let cache = cache_arg(&flags)?;
    let shift = if let Some(s) = flags.get("--shift") {
        glam::DVec3::from_array(floats::<3>(s, "--shift")?)
    } else if let (Some(a), Some(b), Some(c)) = (
        flags.get("--terrain-scale"),
        flags.get("--half-world"),
        flags.get("--origin-texel"),
    ) {
        shift_from_terrain_constants(
            floats::<2>(a, "--terrain-scale")?,
            floats::<2>(b, "--half-world")?,
            floats::<2>(c, "--origin-texel")?,
        )
    } else {
        bail!(
            "camera: the origin shift T of ours_to_orig is needed: --shift X,Y,Z (from TerrainFrame, R2.8), \
             or --terrain-scale SX,SY --half-world HW,HH --origin-texel U,V (constants of the capture and the \
             texel of the centre of hex (0, 0))"
        );
    };
    let cbmap_dirs = match flags.get("--cbmap-dir") {
        Some(d) => vec![PathBuf::from(d)],
        None => camera::cbmap_dirs(&cache),
    };
    let opts = CameraOpts {
        frame: OursToOrig { shift },
        cbmap_dirs,
        event: flags
            .get("--event")
            .map(|e| e.parse())
            .transpose()
            .context("--event")?,
        eye_const: flags.get("--eye-const").cloned(),
        only: flags.get("--capture").cloned().into_iter().collect(),
    };
    let result = camera::run(&cache, &opts)?;
    let out = flags.get("--out").map_or_else(
        || crate_dir().join("../../target/golden/camera.json"),
        PathBuf::from,
    );
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(&out, serde_json::to_string_pretty(&result)? + "\n")?;
    for n in &result.notes {
        println!("note: {n}");
    }
    for c in &result.captures {
        let r = &c.report;
        println!(
            "{}: {} (event {}, {}): fov {:.4} deg, pitch {:.4} deg, near {:.2}, far {:.2}{}",
            c.capture,
            if r.passed { "ok" } else { "FAIL" },
            c.event,
            c.entry,
            r.decomposed.fov_y_deg,
            r.decomposed.pitch_deg,
            r.decomposed.near,
            r.decomposed.far,
            r.matrix_max_err
                .map_or(String::new(), |e| format!(", matrix error {e:.6}")),
        );
        for p in &r.problems {
            println!("  {p}");
        }
    }
    if let Some(ok) = result.s1_pair_passed {
        println!(
            "S1a against S1b: {}",
            if ok {
                "same camera"
            } else {
                "FAIL, the cameras differ"
            }
        );
    }
    println!("report: {}", out.display());
    if !result.passed {
        bail!("camera check failed");
    }
    Ok(())
}

/// `target/golden/<sub>` of the workspace.
fn target_dir(sub: &str) -> PathBuf {
    crate_dir().join("../../target/golden").join(sub)
}

/// `golden replay [--entry Pkg::Entry[:STAGE]] [--capture S1a ...] [--cache DIR] [--out DIR]`
/// replays the draws of the ported entries and compares them with `after`.
fn cmd_replay(args: &[String]) -> Result<()> {
    use golden::replay::driver;
    // `--capture` may repeat, so it is read here and not by `take_flags`.
    let mut captures = Vec::new();
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--capture" {
            captures.push(it.next().context("--capture needs a name")?.clone());
        } else {
            rest.push(a.clone());
        }
    }
    let (flags, extra) = take_flags(&rest, &["--entry", "--cache", "--out", "--status"])?;
    if !extra.is_empty() {
        bail!("replay takes only --flags");
    }
    let opts = driver::Options {
        cache: cache_arg(&flags)?,
        entry: flags.get("--entry").cloned(),
        captures,
        out_dir: flags
            .get("--out")
            .map_or_else(|| target_dir("replay"), PathBuf::from),
        status: flags.get("--status").map_or_else(
            || crate_dir().join("../shader-re/status.toml"),
            PathBuf::from,
        ),
    };
    let report = driver::run(&opts, &crate_dir().join("scenes"))?;
    let path = driver::write_report(&report, &opts.out_dir)?;
    for e in &report.entries {
        println!(
            "{}: {} (replayed {}, pass {}, fail {}, cannot replay {})",
            e.entry, e.verdict, e.replayed, e.passed, e.failed, e.errors
        );
    }
    println!("report: {}", path.display());
    if !report.passed {
        bail!("replay: a call differs from the capture by more than 2/255");
    }
    Ok(())
}

/// `golden report [--stage accept] [--cache DIR] [--out DIR] [--replay FILE] [--ours DIR] [--triage FILE]
/// [--thresholds FILE]`
/// writes the HTML report; `--stage accept` is the gate that needs an `accept`
/// verdict for every capture in `triage.toml`.
fn cmd_report(args: &[String]) -> Result<()> {
    use golden::report;
    let (flags, extra) = take_flags(
        args,
        &[
            "--stage",
            "--cache",
            "--out",
            "--replay",
            "--ours",
            "--triage",
            "--thresholds",
        ],
    )?;
    if !extra.is_empty() {
        bail!("report takes only --flags");
    }
    let opts = report::Options {
        cache: cache_arg(&flags)?,
        out: flags
            .get("--out")
            .map_or_else(|| target_dir("report"), PathBuf::from),
        replay: flags
            .get("--replay")
            .map_or_else(|| target_dir("replay").join("report.json"), PathBuf::from),
        ours: flags
            .get("--ours")
            .map_or_else(|| target_dir("render"), PathBuf::from),
        triage: flags
            .get("--triage")
            .map_or_else(|| crate_dir().join("triage.toml"), PathBuf::from),
        thresholds: flags
            .get("--thresholds")
            .map_or_else(|| crate_dir().join("thresholds.toml"), PathBuf::from),
    };
    match flags.get("--stage").map(String::as_str) {
        None => {
            let (path, caps, summary) = report::build(&opts)?;
            for line in summary {
                println!("{line}");
            }
            println!("report: {} ({})", path.display(), caps.join(", "));
            Ok(())
        }
        Some("accept") => {
            let accepted = report::accept(&opts)?;
            println!("accepted: {}", accepted.join(", "));
            Ok(())
        }
        Some(other) => bail!("report: unknown stage {other:?} (accept)"),
    }
}

/// `golden blind [--seed N] [--port P] [--cache DIR] [--ours DIR] [--out DIR] [--self-test]`:
/// the blind test. Ten pairs of 384x384 crops of S1 to S3, the original and ours in a random order, on
/// `127.0.0.1:8090` (`--port`); the answers and the key go to `target/golden/blind/<date>.json`. Needs `golden render`
/// of S1, S2 and S3 first. `--self-test` runs the server against a client on synthetic crops and exits.
fn cmd_blind(args: &[String]) -> Result<()> {
    use golden::blind;
    let mut args = args.to_vec();
    let self_test = args
        .iter()
        .position(|a| a == "--self-test")
        .map(|i| args.remove(i))
        .is_some();
    let (flags, extra) = take_flags(&args, &["--seed", "--port", "--cache", "--ours", "--out"])?;
    if !extra.is_empty() {
        bail!("blind takes only --flags");
    }
    let out = flags
        .get("--out")
        .map_or_else(|| target_dir("blind"), PathBuf::from);
    if self_test {
        println!("{}", blind::self_test(&out.join("self-test"))?);
        return Ok(());
    }
    let cache = cache_arg(&flags)?;
    let ours = flags
        .get("--ours")
        .map_or_else(|| target_dir("render"), PathBuf::from);
    let mut sources = Vec::new();
    for (scene, capture) in golden::report::discover(&cache) {
        if !matches!(scene.as_str(), "S1" | "S2" | "S3")
            || capture == "S1b"
            || capture.ends_with("-bake")
        {
            continue;
        }
        let dir = golden::captures::parsed_dir(&cache, &scene, &capture);
        let Some((_, reference)) = golden::report::frame_of(&dir) else {
            continue;
        };
        let (Ok(frame), Ok(classes)) = (
            golden::image::read_dds_file(&ours.join(format!("{capture}.dds"))),
            std::fs::read(ours.join(format!("{capture}-native.mask.r8"))),
        ) else {
            println!(
                "{capture}: no render of ours with its class mask in {}: skipped",
                ours.display()
            );
            continue;
        };
        if (frame.width, frame.height) != (reference.width, reference.height)
            || classes.len() != frame.width * frame.height
        {
            println!("{capture}: our frame is not the size of the capture: skipped");
            continue;
        }
        sources.push(blind::Source {
            capture,
            reference,
            ours: frame,
            classes,
        });
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let seed: u64 = match flags.get("--seed") {
        Some(s) => s.parse().with_context(|| format!("--seed {s:?}"))?,
        None => secs ^ (u64::from(std::process::id()) << 32),
    };
    let pairs = blind::pick_pairs(&sources, seed)?;
    // 127.0.0.1:8090 unless `--port` says otherwise; when that port is taken by another program and no port was
    // asked for, any free port of 127.0.0.1 is used and printed.
    let (listener, addr) = match flags.get("--port") {
        Some(p) => blind::bind(p.parse().with_context(|| format!("--port {p:?}"))?)?,
        None => blind::bind(8090).or_else(|e| {
            println!("{e:#}; using a free port");
            blind::bind(0)
        })?,
    };
    println!(
        "blind test: {} pairs from {} (seed {seed}). Open http://{addr}/ in a browser; the page closes the test after the tenth answer.",
        pairs.len(),
        sources
            .iter()
            .map(|s| s.capture.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let outcome = blind::serve(&listener, &pairs, seed, &out, &blind::stamp(secs))?;
    println!(
        "answers saved: {} ({} of {} guessed; the test is passed at {} or fewer)",
        outcome.file.display(),
        outcome.correct,
        pairs.len(),
        blind::PASS_AT_MOST
    );
    Ok(())
}
/// `golden noise [--cache DIR] [--ours DIR] [--out FILE]`: S1a against S1b over the masks of our S1a
/// frame; writes the proposal for the `[noise.*]` tables of `thresholds.toml` (`target/golden/noise.toml`) and prints it.
/// Changing the thresholds is a record in BUILD section 9, so the file is not touched.
fn cmd_noise(args: &[String]) -> Result<()> {
    let (flags, extra) = take_flags(args, &["--cache", "--ours", "--out", "--thresholds"])?;
    if !extra.is_empty() {
        bail!("noise takes only --flags");
    }
    let opts = golden::report::Options {
        cache: cache_arg(&flags)?,
        out: PathBuf::new(),
        replay: PathBuf::new(),
        ours: flags
            .get("--ours")
            .map_or_else(|| target_dir("render"), PathBuf::from),
        triage: PathBuf::new(),
        thresholds: flags
            .get("--thresholds")
            .map_or_else(|| crate_dir().join("thresholds.toml"), PathBuf::from),
    };
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let text = golden::report::noise(&opts, &format!("unix {secs}"))?;
    let out = flags
        .get("--out")
        .map_or_else(|| target_dir("noise.toml"), PathBuf::from);
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&out, &text)?;
    print!("{text}");
    println!("proposal: {}", out.display());
    Ok(())
}

/// `golden all [--scene S1,S2,S3,S4] [--stage accept] [--cache DIR] [--out DIR] [--assets DIR] [--gamecore-lib DIR]
/// [--server FILE] [--gamedata DIR] [--backend native|webgpu|webgl2]` (R3.10, R5.14): the world of every scene whose
/// files are missing (`golden world`), our frame of every scene (`golden render`), the noise proposal and the report;
/// with `--stage accept` the report is the gate. The gate runs last, so a failing scene does not hide the others.
fn cmd_all(args: &[String]) -> Result<()> {
    let (flags, extra) = take_flags(
        args,
        &[
            "--scene",
            "--stage",
            "--cache",
            "--out",
            "--assets",
            "--gamecore-lib",
            "--server",
            "--gamedata",
            "--backend",
        ],
    )?;
    if !extra.is_empty() {
        bail!("all takes only --flags");
    }
    let cache = cache_arg(&flags)?;
    let scenes: Vec<String> = flags.get("--scene").map_or_else(
        || ["S1", "S2", "S3", "S4"].map(String::from).to_vec(),
        |s| s.split(',').map(str::to_owned).collect(),
    );
    let pass = |names: &[&str]| -> Vec<String> {
        let mut v = Vec::new();
        for n in names {
            if let Some(x) = flags.get(*n) {
                v.push((*n).to_owned());
                v.push(x.clone());
            }
        }
        v
    };
    for scene in &scenes {
        if !cache.join("golden").join(scene).join("world.wv3").is_file() {
            let mut a = vec![scene.clone()];
            a.extend(pass(&["--cache", "--server", "--gamedata"]));
            cmd_world(&a)?;
        }
        let mut a = vec![scene.clone()];
        a.extend(pass(&[
            "--cache",
            "--assets",
            "--gamecore-lib",
            "--backend",
        ]));
        cmd_render(&a)?;
    }
    if scenes.iter().any(|s| s == "S1")
        && let Err(e) = cmd_noise(&pass(&["--cache"]))
    {
        println!("noise: {e:#}");
    }
    cmd_report(&pass(&["--cache", "--out", "--stage"]))
}
/// `golden mesh <scene> [--match VS_ENTRY ...] [--capture NAME ...] [--cache DIR] [--out DIR]`
/// OBJ files of the water and river draws (or of the vertex shaders matched).
fn cmd_mesh(args: &[String]) -> Result<()> {
    let mut matches = Vec::new();
    let mut captures = Vec::new();
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--match" => matches.push(it.next().context("--match needs a name")?.clone()),
            "--capture" => captures.push(it.next().context("--capture needs a name")?.clone()),
            _ => rest.push(a.clone()),
        }
    }
    let (flags, pos) = take_flags(&rest, &["--cache", "--out"])?;
    let [scene] = pos.as_slice() else {
        bail!("mesh needs one scene (S1|S2|S3|S4)");
    };
    let opts = golden::mesh::Options {
        cache: cache_arg(&flags)?,
        scenes_dir: crate_dir().join("scenes"),
        out: flags
            .get("--out")
            .map_or_else(|| target_dir("mesh"), PathBuf::from),
        matches,
        captures,
    };
    let meshes = golden::mesh::run(&opts, scene)?;
    for m in &meshes {
        println!(
            "{} event {}: {} ({} vertices, {} triangles) {}",
            m.capture, m.event, m.entry, m.vertices, m.triangles, m.obj
        );
    }
    if meshes.is_empty() {
        bail!("mesh: no draw of {scene} uses the vertex shaders asked for");
    }
    println!(
        "summary: {}",
        golden::mesh::summary_path(&opts.out, scene).display()
    );
    Ok(())
}

/// `golden world <scene> [--server FILE] [--gamedata DIR] [--distro NAME] [--cache DIR] [--out DIR] [--save FILE.Civ5Save]
/// [--civsim DIR]`
/// the worker with the real core makes the scene; writes `mapbasis.wv3`, `world.wv3`,
/// `plots.json`, `digest.json` and, if the map does not match the capture (G-D4), `map-mismatch.json`.
fn cmd_world(args: &[String]) -> Result<()> {
    let (flags, pos) = take_flags(
        args,
        &[
            "--server",
            "--gamedata",
            "--distro",
            "--cache",
            "--out",
            "--save",
            "--civsim",
        ],
    )?;
    let [scene] = pos.as_slice() else {
        bail!("world needs one scene (S1|S2|S3)");
    };
    let cache = cache_arg(&flags)?;
    let root = crate_dir().join("../..");
    let workspace = cache.join("..");
    let opts = golden::world::Options {
        scene: scene.clone(),
        scenes_dir: crate_dir().join("scenes"),
        server: flags.get("--server").map_or_else(
            || root.join("target/linux/release/civ5-server"),
            PathBuf::from,
        ),
        gamedata: flags
            .get("--gamedata")
            .map_or_else(|| workspace.join("assets-out/gamedata-b2"), PathBuf::from),
        distro: flags
            .get("--distro")
            .cloned()
            .unwrap_or_else(|| "Ubuntu-24.04".into()),
        out: flags
            .get("--out")
            .map_or_else(|| cache.join("golden").join(scene), PathBuf::from),
        save: flags.get("--save").map(PathBuf::from),
        civsim: flags
            .get("--civsim")
            .map_or_else(|| workspace.join("Lekmod/headless/tools"), PathBuf::from),
        cache,
    };
    if let Some(why) = golden::world::skip_reason(&opts)? {
        println!("{scene}: skipped: {why}");
        return Ok(());
    }
    let r = golden::world::run(&opts)?;
    println!(
        "{scene}: map {}x{}, basis hash {:016x}, files in {}",
        r.width,
        r.height,
        r.basis_hash,
        r.out.display()
    );
    if !r.map_match_note.is_empty() {
        println!("note: {}", r.map_match_note);
    }
    if let Some(m) = &r.map_match {
        println!(
            "map check against {}: {} (ours revealed {} / visible {}, capture known {} / seen {}, {} hexes differ; {})",
            m.capture,
            if m.matches { "MATCH" } else { "MISMATCH" },
            m.ours_revealed,
            m.ours_visible,
            m.capture_known,
            m.capture_seen,
            m.differing.len(),
            m.mapping
        );
        if !m.matches {
            println!(
                "map-mismatch.json: {} (G-D4: ask the user)",
                r.out.join("map-mismatch.json").display()
            );
        }
    }
    Ok(())
}

/// `golden render <scene> [--backend native|webgpu|webgl2] [--capture NAME]... [--assets DIR] [--gamecore-lib DIR]
/// [--cache DIR] [--out DIR] [--cbmap-dir DIR]`: our frame of the captures of a scene, in
/// `target/golden/render` (`<capture>.dds` for the report, `<capture>-<backend>.png`).
fn cmd_render(args: &[String]) -> Result<()> {
    use golden::render::{Backend, Options, run_scene};
    let mut captures = Vec::new();
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--capture" {
            captures.push(it.next().context("--capture needs a name")?.clone());
        } else {
            rest.push(a.clone());
        }
    }
    let (flags, pos) = take_flags(
        &rest,
        &[
            "--backend",
            "--assets",
            "--gamecore-lib",
            "--cache",
            "--out",
            "--cbmap-dir",
        ],
    )?;
    let [scene] = pos.as_slice() else {
        bail!("render needs one scene (S1|S2|S3|S4)");
    };
    let cache = cache_arg(&flags)?;
    let root = crate_dir().join("../..");
    let workspace = cache.join("..");
    let gamecore_lib = flags.get("--gamecore-lib").map(PathBuf::from).or_else(|| {
        let d = workspace.join("civ5-reimagened/target/gamecore");
        d.is_dir().then_some(d)
    });
    let o = Options {
        scenes_dir: crate_dir().join("scenes"),
        out: flags
            .get("--out")
            .map_or_else(|| target_dir("render"), PathBuf::from),
        root,
        scene: scene.clone(),
        backend: Backend::parse(flags.get("--backend").map_or("native", String::as_str))?,
        captures,
        assets: flags
            .get("--assets")
            .map_or_else(|| workspace.join("assets-out/b2"), PathBuf::from),
        gamecore_lib,
        cbmap_dirs: match flags.get("--cbmap-dir") {
            Some(d) => vec![PathBuf::from(d)],
            None => camera::cbmap_dirs(&cache),
        },
        cache,
    };
    let drawn = run_scene(&o)?;
    println!("rendered: {} -> {}", drawn.join(", "), o.out.display());
    Ok(())
}

/// `golden far [--assets DIR] [--cache DIR] [--out DIR]` (B3 far LOD): the far-view scene S5, our frame with
/// `FarLOD` off, on and off again, and the FLIP of each mask, off against on and off against off again
/// (`target/golden/far/S5-far.md`).
fn cmd_far(args: &[String]) -> Result<()> {
    let (flags, pos) = take_flags(args, &["--assets", "--cache", "--out"])?;
    if !pos.is_empty() {
        bail!("far takes only --flags");
    }
    let cache = cache_arg(&flags)?;
    let workspace = cache.join("..");
    let o = golden::far::Options {
        scenes_dir: crate_dir().join("scenes"),
        out: flags
            .get("--out")
            .map_or_else(|| target_dir("far"), PathBuf::from),
        root: crate_dir().join("../.."),
        assets: flags
            .get("--assets")
            .map_or_else(|| workspace.join("assets-out/b2"), PathBuf::from),
        cache,
    };
    print!("{}", golden::far::run(&o)?);
    Ok(())
}

/// `golden compare-backends --dir <render dir> --capture NAME...` (AC-R8.5; G-T6, G-W6): our WebGL2 frame
/// against our WebGPU frame, mean FLIP over the masks `land`, `water` and `river`; fails over 0.01.
fn cmd_compare_backends(args: &[String]) -> Result<()> {
    use golden::render::{BACKEND_FLIP_MAX, compare_backends};
    let mut captures = Vec::new();
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--capture" {
            captures.push(it.next().context("--capture needs a name")?.clone());
        } else {
            rest.push(a.clone());
        }
    }
    let (flags, pos) = take_flags(&rest, &["--dir"])?;
    if !pos.is_empty() || captures.is_empty() {
        bail!("compare-backends [--dir DIR] --capture NAME [--capture NAME]...");
    }
    let dir = flags
        .get("--dir")
        .map_or_else(|| target_dir("render"), PathBuf::from);
    let mut failed = false;
    for capture in &captures {
        for row in compare_backends(&dir, capture)? {
            let verdict = if row.passes() { "ok" } else { "OVER" };
            println!(
                "{capture} {:<5} {:>8} px  FLIP {:.4} (limit {BACKEND_FLIP_MAX})  {verdict}",
                row.class, row.pixels, row.flip
            );
            failed |= !row.passes();
        }
    }
    if failed {
        bail!("WebGL2 differs from WebGPU over the limit (record it in the registry or fix it)");
    }
    Ok(())
}

/// `golden catalog-json <catalog.json> <catalog.wv3>`: a stopgap for scenes made before `golden world` saved the
/// worker's catalog; the JSON is a `protocol::Catalog`. Prefer `golden world <scene>` again.
fn cmd_catalog_json(args: &[String]) -> Result<()> {
    let [input, output] = args else {
        bail!("catalog-json needs <catalog.json> <catalog.wv3>");
    };
    let catalog: protocol::Catalog = serde_json::from_slice(&std::fs::read(input)?)
        .with_context(|| format!("{input} is not a Catalog"))?;
    std::fs::write(output, golden::wv3::encode_catalog(&catalog)?)?;
    println!(
        "{output}: {} terrains, {} features, {} units",
        catalog.terrains.len(),
        catalog.features.len(),
        catalog.units.len()
    );
    Ok(())
}

/// `golden terrain <scene>... [--assets DIR] [--cache DIR] [--out DIR]` (G-T1 for each scene. The scene's `mapbasis.wv3` comes from `golden world`.
fn cmd_terrain(args: &[String]) -> Result<()> {
    let (flags, scenes) = take_flags(args, &["--assets", "--cache", "--out"])?;
    if scenes.is_empty() {
        bail!("terrain needs one or more scenes (S1|S2|S3)");
    }
    let cache = cache_arg(&flags)?;
    let workspace = cache.join("..");
    for scene in scenes {
        let o = golden::terrain_check::Options {
            scene: scene.clone(),
            assets: flags
                .get("--assets")
                .map_or_else(|| workspace.join("assets-out/b2-rp27"), PathBuf::from),
            out: flags
                .get("--out")
                .map_or_else(|| target_dir("terrain"), PathBuf::from),
            cache: cache.clone(),
        };
        let r = golden::terrain_check::run(&o)?;
        println!(
            "{scene}: map {}x{} wrap {}, {} tile(s), within 1: {:.4}, within 4: {:.4}",
            r.map.0,
            r.map.1,
            r.map.2,
            r.tiles.len(),
            r.within_1,
            r.within_4
        );
        for t in &r.tiles {
            println!(
                "  tile {} ({},{}) flip {} offset {:?}: error {:.2} (nominal {:.2}), {} texels, <=1 {:.4}, <=4 {:.4}, land/water {:.4}",
                t.resource,
                t.tile.0,
                t.tile.1,
                t.flipped,
                t.offset,
                t.mean_error_best,
                t.mean_error_nominal,
                t.texels,
                t.within_1,
                t.within_4,
                t.land_water_agreement
            );
            for (k, (n, a, b)) in &t.by_relief {
                println!("    {k}: {n} texels, <=1 {a:.4}, <=4 {b:.4}");
            }
        }
        for n in &r.note {
            println!("  note: {n}");
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    const KNOWN: &str = "prep, parse, check-captures, camera, render, replay, report, noise, blind, all, mesh, world, terrain, catalog-json, compare-backends, far";
    match args.first().map(String::as_str) {
        Some("prep") => cmd_prep(&args[1..]),
        Some("parse") => cmd_parse(&args[1..]),
        Some("check-captures") => cmd_check_captures(&args[1..]),
        Some("camera") => cmd_camera(&args[1..]),
        Some("render") => cmd_render(&args[1..]),
        Some("compare-backends") => cmd_compare_backends(&args[1..]),
        Some("catalog-json") => cmd_catalog_json(&args[1..]),
        Some("replay") => cmd_replay(&args[1..]),
        Some("report") => cmd_report(&args[1..]),
        Some("noise") => cmd_noise(&args[1..]),
        Some("blind") => cmd_blind(&args[1..]),
        Some("all") => cmd_all(&args[1..]),
        Some("mesh") => cmd_mesh(&args[1..]),
        Some("world") => cmd_world(&args[1..]),
        Some("terrain") => cmd_terrain(&args[1..]),
        Some("far") => cmd_far(&args[1..]),
        Some(other) => bail!("golden: unknown command {other:?} (implemented so far: {KNOWN})"),
        None => bail!("golden: command needed (implemented so far: {KNOWN})"),
    }
}
