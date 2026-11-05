//! `shader-re` command line. `cargo xtask shader-re <cmd>` forwards here.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use shader_re::bins::{self, BinId, Search};
use shader_re::catalog::{self, Catalog};
use shader_re::check;
use shader_re::harness::Gpu;
use shader_re::harness::compare;
use shader_re::harness::formats::Formats;
use shader_re::harness::subject::Subject;
use shader_re::names;
use shader_re::scan;
use shader_re::sm3;
use shader_re::status::{self, StatusFile};
use shader_re::validate;

/// `render::shaders::catalog()`: the modules of the products. The file uses
/// only `std` and paths relative to itself, so it is included here and the tool needs no `wgpu`.
#[allow(dead_code)]
#[path = "../../../crates/client/render/src/shaders/catalog.rs"]
mod render_catalog;

const USAGE: &str = "usage: shader-re <command>
  extract [linux|dx9|dx11 ...] [--bin <id>=<path>]... [--cache <dir>] [--re-dir <dir>]
      find the shader blocks in the pinned binaries, check the pinned counts,
      write <cache>/<sha256-12>/{blocks/<sha1>.bin, blocks.json}; with linux (and dx9)
      also read the entry names out of the registration code (checked against
      <re-dir>/shaderdef.tsv when that exists), check the world counts and write
      <cache>/<sha256-12 of Civ5XP>/catalog.json and the stable copy <cache>/catalog.json
  disasm <entry | sha1-prefix | file.bin> [--from linux|dx9] [--plain] [--bin <id>=<path>]...
      SM3 listing in the text of D3DDisassemble; CTAB names as trailing comments
      unless --plain; <entry> is Package::Entry[:STAGE] from the catalog
  status-init [--catalog <file>] [--status <file>]
      write status.toml: every world entry of the catalog, new ones as todo, old ones kept
  check [--stage accept] [--catalog <file>] [--status <file>]
      status of every world entry (AC-R9.1); writes target/shader-re/check.json; with
      --stage accept a todo entry fails
  oracle <entry | --all> [--print] [--catalog <file>]
      SM3 -> WGSL oracle of an entry (or of every world entry with a block): written to
      <cache>/<sha256-12 of Civ5XP>/oracle/<sha1>.wgsl, checked with naga (WGSL, GLSL ES 3.00)
  compare <entry> [--module <file.wgsl>]... [--seed <n>] [--catalog <file>] [--status <file>]
      AC-R9.2: the hand-written module of a `ported` entry (status.toml: wgsl = path#entry,
      body `<entry>_body`) against the oracle, on a native GPU: 64x64 grid, three seeds, linear
      and point sampling, metric 1e-4; --module lists the files to concatenate instead of the
      one in status.toml; the result goes to target/shader-re/compare.json (read by `check`)
  crosscheck-d3d [linux|dx9 ...] [--python <exe>] [--bin <id>=<path>]... [--cache <dir>]
      Windows: compare our listing with D3DDisassemble (d3dcompiler_47.dll through
      py/d3d_disasm.py, python -I) for every SM3 block; fails on any difference
  dx11 asm [--python <exe>]
      Windows: the text of D3DDisassemble of every DXBC block (py/d3d_disasm.py) into
      <cache>/<sha256-12 of the DX11 exe>/dx11/asm/<sha1>.txt
  dx11 info <sha1-prefix>
      one DXBC container: chunks, signatures, declarations
  dx11 twins [--confirm <draws.json>]... [--catalog <file>]
      every container parsed; the DX11 twin of every SM3 entry (features and order, SR-D8);
      draws.json files of `golden` (also found in <cache>/golden) confirm twins; writes
      <dx11>/twins.json and puts the twins into the catalog
  dx11 cbmap <entry> | --m1 | --all [--python <exe>]
      the map of the constants of an entry: CTAB name -> (cb, byte offset, rows) of the twin;
      hand-written maps/<Package>.<Entry>.<STAGE>.toml go over it; --m1 the milestone M1 entries
      (fails on an unmapped constant), --all every world entry with a twin
  validate [--all] [<file.wgsl> ...]
      naga: WGSL, GLSL ES 3.00 translation and WebGL2 limits; --all adds every module of
      render::shaders::catalog() (the parts joined) and every other .wgsl under
      crates/client/render/src";

/// Splits `--name value` / `--name=value` out of `args`; the rest is returned.
fn take_flag(args: &mut Vec<String>, name: &str) -> Result<Vec<String>> {
    let mut values = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == name {
            args.remove(i);
            if i >= args.len() {
                bail!("{name} needs a value");
            }
            values.push(args.remove(i));
        } else if let Some(v) = args[i].strip_prefix(&format!("{name}=")) {
            values.push(v.to_string());
            args.remove(i);
        } else {
            i += 1;
        }
    }
    Ok(values)
}

fn cmd_extract(mut args: Vec<String>) -> Result<()> {
    let mut search = Search::from_env();
    for spec in take_flag(&mut args, "--bin")? {
        let (id, path) = spec
            .split_once('=')
            .with_context(|| format!("--bin expects <linux|dx9|dx11>=<path>, got {spec:?}"))?;
        let id = BinId::from_name(id).with_context(|| format!("--bin: unknown binary {id:?}"))?;
        search.explicit.insert(id, PathBuf::from(path));
    }
    let cache = take_flag(&mut args, "--cache")?.pop().map(PathBuf::from);
    let re_dir = take_flag(&mut args, "--re-dir")?.pop().map(PathBuf::from);
    let mut wanted = Vec::new();
    for a in &args {
        match BinId::from_name(a) {
            Some(id) => wanted.push(id),
            None => bail!("extract: unknown argument {a:?}\n{USAGE}"),
        }
    }
    if wanted.is_empty() {
        wanted = BinId::ALL.to_vec();
    }
    let cache_root = bins::cache_root(cache.as_deref())?;

    println!(
        "{:<6} {:>7} {:>5} {:>5} {:>7} {:>10}  cache",
        "bin", "blocks", "vs", "ps", "unique", "bytes"
    );
    let mut sm3_bins = Vec::new();
    let mut dx11_scan = None;
    for id in wanted {
        let bin = bins::find(id, &search)?;
        let data = bin.read()?;
        let blocks = scan::extract(id, &data)?;
        let vs = blocks.iter().filter(|b| b.stage == scan::Stage::Vs).count();
        let ps = blocks.iter().filter(|b| b.stage == scan::Stage::Ps).count();
        let bytes: usize = blocks.iter().map(|b| b.size).sum();
        let dir = bins::bin_cache_dir(&cache_root, &bin);
        println!(
            "{:<6} {:>7} {:>5} {:>5} {:>7} {:>10}  {}",
            id,
            blocks.len(),
            vs,
            ps,
            scan::unique_count(&blocks),
            bytes,
            dir.display()
        );
        if id == BinId::Dx11 {
            for (profile, n) in scan::profile_counts(&blocks) {
                println!("         {profile}: {n}");
            }
        }
        // Pinned numbers: any other count is an error, not "new shaders".
        scan::verify_pins(id, &blocks)?;
        scan::write_cache(&dir, &bin, &data, &blocks)?;
        if id == BinId::Dx11 {
            dx11_scan = Some((data, blocks, dir.join("dx11")));
        } else {
            sm3_bins.push((bin, data, blocks));
        }
    }
    let Some(li) = sm3_bins.iter().position(|(b, _, _)| b.id == BinId::Linux) else {
        return Ok(());
    };
    build_catalog(&sm3_bins, li, &cache_root, re_dir.as_deref())?;
    // The catalog was just made without twins: put them in when the DX11 blocks are at hand.
    if let Some((data, blocks, dx11_dir)) = dx11_scan {
        let mut cat = Catalog::load(&cache_root.join("catalog.json"))?;
        let parsed = blocks
            .iter()
            .map(|b| shader_re::dxbc::Dxbc::parse(&data[b.offset..b.offset + b.size]))
            .collect::<Result<Vec<_>, _>>()?;
        let report = shader_re::dxbc::twins::compute(&cat, &blocks, &parsed);
        std::fs::create_dir_all(&dx11_dir)?;
        std::fs::write(dx11_dir.join("twins.json"), report.to_json())?;
        write_twins_to_catalog(&cache_root, &mut cat, &report)?;
        println!(
            "twins: {} SM3 entries have a DX11 twin ({} DX11 shaders without one)",
            report.twins.len(),
            report.unmatched_groups.len()
        );
    }
    Ok(())
}

/// The folder of an optional reference `shaderdef.tsv` (RW-1): `--re-dir`, else `CIV5_RE_DIR`.
fn reference_dir(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(d) = explicit {
        return Some(d.to_path_buf());
    }
    let d = PathBuf::from(std::env::var_os("CIV5_RE_DIR").filter(|v| !v.is_empty())?);
    d.is_dir().then_some(d)
}

/// `tools/shader-re/status.toml` and the repository root.
fn repo_root() -> Result<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .map(Path::to_path_buf)
        .context("crate is not two levels below the repo root")
}

fn default_status_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("status.toml")
}

fn build_catalog(
    sm3_bins: &[(bins::FoundBin, Vec<u8>, Vec<scan::Block>)],
    li: usize,
    cache_root: &Path,
    re_dir: Option<&Path>,
) -> Result<()> {
    let (lbin, ldata, lblocks) = &sm3_bins[li];
    let found = names::extract(ldata).context("reading the names out of the registration code")?;
    let ps = found
        .registrations
        .iter()
        .filter(|r| r.stage == scan::Stage::Ps)
        .count();
    let vs = found.registrations.len() - ps;
    println!(
        "names: {} registrations ({ps} ps, {vs} vs; world {}), {} constant sets (world {}), {} packages",
        found.registrations.len(),
        found
            .registrations
            .iter()
            .filter(|r| r.in_world_init)
            .count(),
        found.const_sets.len(),
        found.const_sets.iter().filter(|c| c.in_world_init).count(),
        found.packages.len()
    );
    match reference_dir(re_dir) {
        Some(dir) if dir.join("shaderdef.tsv").is_file() => {
            let path = dir.join("shaderdef.tsv");
            let tsv = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            names::compare_reference(&found, &tsv, &path.to_string_lossy())?;
            println!("names: match {}", path.display());
        }
        _ => println!(
            "names: no reference shaderdef.tsv, not cross-checked (--re-dir or CIV5_RE_DIR)"
        ),
    }
    let status_path = default_status_path();
    let status = if status_path.is_file() {
        StatusFile::load(&status_path)?
    } else {
        StatusFile::default()
    };
    let linux = catalog::Scanned {
        bin: lbin,
        data: ldata,
        blocks: lblocks,
    };
    let dx9 = sm3_bins
        .iter()
        .find(|(b, _, _)| b.id == BinId::Dx9)
        .map(|(bin, data, blocks)| catalog::Scanned { bin, data, blocks });
    let cat = catalog::build(&linux, dx9.as_ref(), &found, &status)?;
    let (per_bin, stable) = catalog::write(cache_root, lbin, &cat)?;
    let c = &cat.counts;
    println!(
        "catalog: {} blocks ({} distinct, {} unnamed), {} entries ({} world, {} with SM3), {} shader sets, {} constant sets, {} samplers, {} DX9 blocks differ",
        c.blocks,
        c.unique_blocks,
        c.unnamed_blocks,
        c.entries,
        c.world_entries,
        c.entries_with_sm3,
        c.shader_sets,
        c.const_sets,
        cat.samplers_by_package.len(),
        c.dx9_unique_differing
    );
    println!(
        "catalog: {}\ncatalog: {}",
        per_bin.display(),
        stable.display()
    );
    Ok(())
}

/// `--bin <id>=<path>` flags into a search.
fn search_from(args: &mut Vec<String>) -> Result<Search> {
    let mut search = Search::from_env();
    for spec in take_flag(args, "--bin")? {
        let (id, path) = spec
            .split_once('=')
            .with_context(|| format!("--bin expects <linux|dx9|dx11>=<path>, got {spec:?}"))?;
        let id = BinId::from_name(id).with_context(|| format!("--bin: unknown binary {id:?}"))?;
        search.explicit.insert(id, PathBuf::from(path));
    }
    Ok(search)
}

fn take_switch(args: &mut Vec<String>, name: &str) -> bool {
    match args.iter().position(|a| a == name) {
        Some(i) => {
            args.remove(i);
            true
        }
        None => false,
    }
}

fn cmd_disasm(mut args: Vec<String>) -> Result<()> {
    let search = search_from(&mut args)?;
    let annotate = !take_switch(&mut args, "--plain");
    let id = match take_flag(&mut args, "--from")?.pop() {
        Some(n) => BinId::from_name(&n).with_context(|| format!("--from: unknown binary {n:?}"))?,
        None => BinId::Linux,
    };
    let [what] = args.as_slice() else {
        bail!("disasm: give one block (sha1 or a unique prefix of it, or a .bin file)\n{USAGE}");
    };
    let bytes = if Path::new(what).is_file() {
        std::fs::read(what).with_context(|| format!("reading {what}"))?
    } else if what.contains("::") {
        let cache_root = bins::cache_root(None)?;
        let cat = Catalog::load(&cache_root.join("catalog.json"))
            .context("disasm by entry name needs the catalog: run `extract` first")?;
        let e = cat.find_entry(what).with_context(|| {
            format!(
                "disasm: no entry {what:?} in the catalog (or the name is ambiguous: add :VS / :PS)"
            )
        })?;
        let sm3 = e
            .sm3
            .as_ref()
            .with_context(|| format!("disasm: {} has no SM3 block (DX11 only)", e.name))?;
        let sha1 = match (id, &e.dx9_sha1) {
            (BinId::Dx9, Some(d)) => d.clone(),
            _ => sm3.sha1.clone(),
        };
        println!("// {} ({id}) sha1 {sha1}", e.name);
        cached_block(&cache_root, &sha1)?
    } else {
        let bin = bins::find(id, &search)?;
        let data = bin.read()?;
        let blocks = scan::extract(id, &data)?;
        let hits: Vec<&scan::Block> = blocks
            .iter()
            .filter(|b| b.sha1.starts_with(what.as_str()))
            .collect();
        let first = hits
            .first()
            .with_context(|| format!("disasm: no block with sha1 {what} in {id}"))?;
        if hits.iter().any(|b| b.sha1 != first.sha1) {
            bail!("disasm: {what} is ambiguous in {id}");
        }
        data[first.offset..first.offset + first.size].to_vec()
    };
    let shader = sm3::parse(&bytes).context("parsing the block")?;
    print!("{}", sm3::disassemble(&shader, annotate));
    Ok(())
}

/// `blocks/<sha1>.bin` from any binary's cache folder.
fn cached_block(cache_root: &Path, sha1: &str) -> Result<Vec<u8>> {
    for e in std::fs::read_dir(cache_root)
        .with_context(|| format!("reading {}", cache_root.display()))?
    {
        let p = e?.path().join("blocks").join(format!("{sha1}.bin"));
        if p.is_file() {
            return std::fs::read(&p).with_context(|| format!("reading {}", p.display()));
        }
    }
    bail!(
        "block {sha1} is not in the cache {}: run `extract`",
        cache_root.display()
    )
}

fn load_catalog_arg(args: &mut Vec<String>) -> Result<Catalog> {
    let path = match take_flag(args, "--catalog")?.pop() {
        Some(p) => PathBuf::from(p),
        None => bins::cache_root(None)?.join("catalog.json"),
    };
    Catalog::load(&path).with_context(|| {
        format!(
            "loading the catalog {}: run `extract` first",
            path.display()
        )
    })
}

fn status_path_arg(args: &mut Vec<String>) -> Result<PathBuf> {
    Ok(take_flag(args, "--status")?
        .pop()
        .map_or_else(default_status_path, PathBuf::from))
}

fn cmd_status_init(mut args: Vec<String>) -> Result<()> {
    let cat = load_catalog_arg(&mut args)?;
    let path = status_path_arg(&mut args)?;
    if let Some(a) = args.first() {
        bail!("status-init: unexpected argument {a:?}\n{USAGE}");
    }
    let keep = if path.is_file() {
        StatusFile::load(&path)?
    } else {
        StatusFile::default()
    };
    let names = cat.world_entry_names();
    let text = status::render(&names, &keep);
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    println!(
        "status-init: {} world entries -> {}",
        names.len(),
        path.display()
    );
    Ok(())
}

fn cmd_check(mut args: Vec<String>) -> Result<()> {
    let accept = match take_flag(&mut args, "--stage")?.pop().as_deref() {
        None => false,
        Some("accept") => true,
        Some(other) => bail!("check: unknown stage {other:?} (only `accept`)"),
    };
    let cat = load_catalog_arg(&mut args)?;
    let status_path = status_path_arg(&mut args)?;
    if let Some(a) = args.first() {
        bail!("check: unexpected argument {a:?}\n{USAGE}");
    }
    let st = StatusFile::load(&status_path)?;
    let repo = repo_root()?;
    let world: Vec<(String, Option<String>)> = cat
        .entries
        .iter()
        .filter(|e| e.world)
        .map(|e| (e.name.clone(), e.sm3.as_ref().map(|b| b.sha1.clone())))
        .collect();
    let compare_path = repo.join("target/shader-re/compare.json");
    let compare: check::CompareResults = if compare_path.is_file() {
        let text = std::fs::read_to_string(&compare_path)?;
        serde_json::from_str(&text)
            .with_context(|| format!("parsing {}", compare_path.display()))?
    } else {
        check::CompareResults::default()
    };
    let tracked = git_ls_files(&repo);
    let report = check::run(&check::Input {
        world: &world,
        status: &st,
        repo: &repo,
        compare: &compare,
        tracked: &tracked,
        accept,
    });
    print!("{}", report.table());
    let out = repo.join("target/shader-re/check.json");
    std::fs::create_dir_all(out.parent().expect("has a parent"))?;
    std::fs::write(&out, report.to_json()).with_context(|| format!("writing {}", out.display()))?;
    println!("check: {}", out.display());
    if report.failed() {
        bail!("check: failed");
    }
    Ok(())
}

/// `git ls-files` of the repository; a warning and an empty list when git is not there.
fn git_ls_files(repo: &Path) -> Vec<String> {
    match std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("ls-files")
        .output()
    {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(str::to_string)
            .collect(),
        _ => {
            eprintln!(
                "check: warning: git ls-files failed, the executable-derived-files check is skipped"
            );
            Vec::new()
        }
    }
}

/// Mismatches listed in full by `crosscheck-d3d`.
const SHOW_MISMATCHES: usize = 20;

/// Windows: our listing against `D3DDisassemble` (`py/d3d_disasm.py`) for every SM3 block.
fn cmd_crosscheck(mut args: Vec<String>) -> Result<()> {
    if !cfg!(windows) {
        bail!("crosscheck-d3d needs Windows (d3dcompiler_47.dll)");
    }
    let search = search_from(&mut args)?;
    let cache = take_flag(&mut args, "--cache")?.pop().map(PathBuf::from);
    let python = take_flag(&mut args, "--python")?
        .pop()
        .or_else(|| std::env::var("PYTHON").ok().filter(|v| !v.is_empty()))
        .unwrap_or_else(|| "python".to_string());
    let mut wanted = Vec::new();
    for a in &args {
        match BinId::from_name(a) {
            Some(id @ (BinId::Linux | BinId::Dx9)) => wanted.push(id),
            _ => bail!("crosscheck-d3d: SM3 binaries are linux and dx9, got {a:?}"),
        }
    }
    if wanted.is_empty() {
        wanted = vec![BinId::Linux, BinId::Dx9];
    }
    let cache_root = bins::cache_root(cache.as_deref())?;
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("py/d3d_disasm.py");
    let mut failed = false;
    for id in wanted {
        let bin = bins::find(id, &search)?;
        let data = bin.read()?;
        let blocks = scan::extract(id, &data)?;
        scan::verify_pins(id, &blocks)?;
        let dir = bins::bin_cache_dir(&cache_root, &bin);
        scan::write_cache(&dir, &bin, &data, &blocks)?;
        let out_dir = dir.join("d3d");
        let status = std::process::Command::new(&python)
            .arg("-I")
            .arg(&script)
            .arg(&out_dir)
            .arg(dir.join("blocks"))
            .status()
            .with_context(|| format!("running {python} -I {}", script.display()))?;
        if !status.success() {
            bail!("d3d_disasm.py failed ({status})");
        }
        let mut seen = std::collections::BTreeMap::<&str, Option<String>>::new();
        for b in &blocks {
            if seen.contains_key(b.sha1.as_str()) {
                continue;
            }
            let bytes = &data[b.offset..b.offset + b.size];
            let theirs = std::fs::read_to_string(out_dir.join(format!("{}.txt", b.sha1)))
                .with_context(|| format!("reading the D3D text of {}", b.sha1))?;
            let verdict = match sm3::parse(bytes) {
                Err(e) => Some(format!("parse error: {e}")),
                Ok(sh) => sm3::disasm::first_diff(&sm3::disassemble(&sh, false), &theirs).map(
                    |(n, ours, theirs)| {
                        format!("line {n}\n    ours:   {ours}\n    d3d:    {theirs}")
                    },
                ),
            };
            seen.insert(b.sha1.as_str(), verdict);
        }
        // Per block entry (433), not per distinct sha1.
        let ok_entries = blocks
            .iter()
            .filter(|b| seen[b.sha1.as_str()].is_none())
            .count();
        let unique_bad: Vec<_> = seen
            .iter()
            .filter_map(|(k, v)| v.as_ref().map(|v| (k, v)))
            .collect();
        println!(
            "{id}: {ok_entries}/{} blocks match D3DDisassemble line by line ({} distinct, {} differ)",
            blocks.len(),
            seen.len(),
            unique_bad.len()
        );
        for (sha1, why) in unique_bad.iter().take(SHOW_MISMATCHES) {
            println!("  {sha1}: {why}");
        }
        if ok_entries != blocks.len() {
            failed = true;
        }
    }
    if failed {
        bail!("crosscheck-d3d: listings differ");
    }
    Ok(())
}

/// Every `.wgsl` under `dir`, sorted.
fn collect_wgsl(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for e in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let p = e?.path();
        if p.is_dir() {
            collect_wgsl(&p, out)?;
        } else if p.extension().is_some_and(|x| x == "wgsl") {
            out.push(p);
        }
    }
    out.sort();
    Ok(())
}

fn cmd_validate(mut args: Vec<String>) -> Result<()> {
    let all = if let Some(i) = args.iter().position(|a| a == "--all") {
        args.remove(i);
        true
    } else {
        false
    };
    let mut files: Vec<PathBuf> = args.iter().map(PathBuf::from).collect();
    let mut failed = 0;
    let mut modules = 0;
    if all {
        // The modules of the catalog, as products use them: the parts joined.
        for module in render_catalog::catalog() {
            let report =
                validate::validate_wgsl(&format!("catalog:{}", module.name), &module.source());
            print!("{report}");
            let mut bad = !report.ok();
            // The entry points the catalog names are those the text has.
            let mut have: Vec<&str> = report.entries.iter().map(|e| e.name.as_str()).collect();
            let mut want: Vec<&str> = module.entries.iter().map(|e| e.name).collect();
            have.sort_unstable();
            want.sort_unstable();
            if report.ok() && have != want {
                println!(
                    "catalog:{}: entry points {want:?} in the catalog, {have:?} in the text",
                    module.name
                );
                bad = true;
            }
            modules += 1;
            failed += usize::from(bad);
        }
        // The other files of the render crate (B1's own shaders); the files under `shaders/` are
        // parts of catalog modules and are not whole modules by themselves.
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .context("crate is not two levels below the repo root")?;
        let mut loose = Vec::new();
        collect_wgsl(&repo.join("crates/client/render/src"), &mut loose)?;
        let shaders = repo.join("crates/client/render/src/shaders");
        files.extend(loose.into_iter().filter(|f| !f.starts_with(&shaders)));
    }
    if files.is_empty() && modules == 0 {
        bail!("validate: give files or --all\n{USAGE}");
    }
    for f in &files {
        let mut src =
            std::fs::read_to_string(f).with_context(|| format!("reading {}", f.display()))?;
        // B1's model shader is built with `common/skin.wgsl` in front of it (model.rs).
        if f.file_name().is_some_and(|n| n == "model.wgsl")
            && let Some(dir) = f.parent()
            && let Ok(skin) = std::fs::read_to_string(dir.join("shaders/common/skin.wgsl"))
        {
            src = format!("{skin}\n{src}");
        }
        let report = validate::validate_wgsl(&f.to_string_lossy(), &src);
        print!("{report}");
        if !report.ok() {
            failed += 1;
        }
    }
    println!(
        "validate: {} module(s), {failed} failed",
        files.len() + modules
    );
    if all {
        // SR criterion 3: the oracle of every world entry with an SM3 block passes naga, the
        // WebGL2 limits and the GLSL ES 3.00 translation. Needs the catalog and the block cache.
        failed += validate_world_oracles()?;
    }
    if failed > 0 {
        bail!("validate: {failed} module(s) failed");
    }
    Ok(())
}

/// Oracle WGSL of every world entry with a block, validated. The catalog comes from `extract`;
/// without it (or without the binaries) there is nothing to translate and this says so.
fn validate_world_oracles() -> Result<usize> {
    let cache_root = bins::cache_root(None)?;
    let cat = match Catalog::load(&cache_root.join("catalog.json")) {
        Ok(c) => c,
        Err(e) => {
            println!("validate: oracle of the world entries skipped: {e}");
            return Ok(0);
        }
    };
    let dir = match oracle_dir(&cache_root) {
        Ok(d) => d,
        Err(e) => {
            println!("validate: oracle of the world entries skipped: {e}");
            return Ok(0);
        }
    };
    let names = world_sm3_names(&cat);
    let formats = load_formats()?;
    let (ok, failed) = oracle_entries(&cat, &cache_root, &dir, &names, &formats, true, false)?;
    println!(
        "validate: oracle of {} world entries (of {} world entries; the rest have no SM3 block): {ok} ok, {failed} failed",
        names.len(),
        cat.counts.world_entries
    );
    Ok(failed)
}

/// The SM3 block of `e` as the oracle sees it: bytes, parsed shader, sha1.
fn entry_block(cache_root: &Path, e: &catalog::Entry) -> Result<(String, sm3::Shader)> {
    let sm3_ref = e
        .sm3
        .as_ref()
        .with_context(|| format!("{} has no SM3 block (DX11 only)", e.name))?;
    let bytes = cached_block(cache_root, &sm3_ref.sha1)?;
    let shader = sm3::parse(&bytes).with_context(|| format!("parsing the block of {}", e.name))?;
    Ok((sm3_ref.sha1.clone(), shader))
}

/// `<cache>/<sha256-12 of Civ5XP>/oracle/`.
fn oracle_dir(cache_root: &Path) -> Result<PathBuf> {
    let bin = bins::find(BinId::Linux, &Search::from_env())?;
    Ok(bins::bin_cache_dir(cache_root, &bin).join("oracle"))
}

/// Translates the named entries, writes the WGSL to `dir`, checks it with naga. Returns
/// `(ok, failed)`; failures are printed.
fn oracle_entries(
    cat: &Catalog,
    cache_root: &Path,
    dir: &Path,
    names: &[String],
    formats: &Formats,
    quiet: bool,
    print: bool,
) -> Result<(usize, usize)> {
    std::fs::create_dir_all(dir)?;
    let (mut ok, mut failed) = (0, 0);
    for name in names {
        let e = cat.find_entry(name).expect("name from the catalog");
        let result = entry_block(cache_root, e).and_then(|(sha1, sh)| {
            let o = compare::oracle_for(&sh, &e.name, &sha1, formats)?;
            Ok((sha1, o))
        });
        match result {
            Ok((sha1, o)) => {
                let path = dir.join(format!("{sha1}.wgsl"));
                std::fs::write(&path, &o.wgsl)?;
                let report = validate::validate_wgsl(&e.name, &o.wgsl);
                if report.ok() {
                    ok += 1;
                } else {
                    failed += 1;
                    print!("{report}");
                }
                if !quiet {
                    println!("oracle: {}", path.display());
                    if print {
                        print!("{}", o.wgsl);
                    }
                }
            }
            Err(err) => {
                failed += 1;
                println!("{}: {err:#}", e.name);
            }
        }
    }
    Ok((ok, failed))
}

/// Names of the world entries that have an SM3 block.
fn world_sm3_names(cat: &Catalog) -> Vec<String> {
    cat.entries
        .iter()
        .filter(|e| e.world && e.sm3.is_some())
        .map(|e| e.name.clone())
        .collect()
}

fn cmd_oracle(mut args: Vec<String>) -> Result<()> {
    let all = take_switch(&mut args, "--all");
    let print = take_switch(&mut args, "--print");
    let cat = load_catalog_arg(&mut args)?;
    let cache_root = bins::cache_root(None)?;
    let dir = oracle_dir(&cache_root)?;
    let names: Vec<String> = if all {
        world_sm3_names(&cat)
    } else {
        let [what] = args.as_slice() else {
            bail!("oracle: give one entry (Package::Entry[:STAGE]) or --all\n{USAGE}");
        };
        vec![
            cat.find_entry(what)
                .with_context(|| format!("oracle: no entry {what:?} in the catalog"))?
                .name
                .clone(),
        ]
    };
    let formats = load_formats()?;
    let (ok, failed) = oracle_entries(&cat, &cache_root, &dir, &names, &formats, all, print)?;
    println!("oracle: {} entries, {ok} ok, {failed} failed", names.len());
    if failed > 0 {
        bail!("oracle: {failed} entries failed");
    }
    Ok(())
}

/// `tools/shader-re/maps/formats.toml`.
fn load_formats() -> Result<Formats> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("maps/formats.toml");
    Formats::load(&path).with_context(|| format!("loading {}", path.display()))
}

/// `target/shader-re/compare.json`: what the last `compare` said about each entry, read by
/// `check` (4.9).
fn compare_json_path() -> Result<PathBuf> {
    Ok(repo_root()?.join("target/shader-re/compare.json"))
}

fn record_compare(name: &str, sha1: &str, pass: bool) -> Result<()> {
    let path = compare_json_path()?;
    let mut doc: serde_json::Value = match std::fs::read_to_string(&path) {
        Ok(t) => serde_json::from_str(&t).unwrap_or_else(|_| serde_json::json!({})),
        Err(_) => serde_json::json!({}),
    };
    if !doc.get("entries").is_some_and(serde_json::Value::is_object) {
        doc["entries"] = serde_json::json!({});
    }
    doc["entries"][name] = serde_json::json!({ "sha1": sha1, "pass": pass });
    std::fs::create_dir_all(path.parent().expect("has a parent"))?;
    std::fs::write(&path, serde_json::to_string_pretty(&doc)? + "\n")
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// `compare --self`: the oracle of an entry (or of every world entry with a block) against
/// itself. It proves nothing about a port; it shows that the harness can run the entry (every
/// resource kind, the device limits, the register file) before a hand-written module exists.
fn compare_self(cat: &Catalog, args: &[String], all: bool) -> Result<()> {
    let cache_root = bins::cache_root(None)?;
    let names: Vec<String> = if all {
        world_sm3_names(cat)
    } else {
        let [what] = args else {
            bail!("compare --self: give one entry or --all\n{USAGE}");
        };
        vec![
            cat.find_entry(what)
                .with_context(|| format!("compare: no entry {what:?} in the catalog"))?
                .name
                .clone(),
        ]
    };
    let formats = load_formats()?;
    let gpu = Gpu::new().map_err(|e| anyhow::anyhow!("compare needs a GPU adapter: {e}"))?;
    println!("compare --self: {} ({:?})", gpu.info.name, gpu.info.backend);
    let opts = compare::CompareOptions::new(&formats);
    let mut failed = 0;
    for name in &names {
        let e = cat.find_entry(name).expect("name from the catalog");
        let one = || -> Result<compare::Report> {
            let (sha1, shader) = entry_block(&cache_root, e)?;
            let o = compare::oracle_for(&shader, &e.name, &sha1, &formats)?;
            let s = Subject::from_oracle(&o)?;
            // The hand-written side would take its constants by CTAB name; the oracle takes the
            // registers directly.
            Ok(compare::compare_oracle(&gpu, &o, &s, &opts)?)
        };
        match one() {
            // A run that compared only zeros and NaNs is no proof of anything (a constant
            // shader such as `mov oC0, 0` is that, and is no failure).
            Ok(r) if r.pass() && r.live == 0 => {
                println!("{name}: note: nothing but zeros and non-finite values to compare");
            }
            Ok(r) if r.pass() => {}
            Ok(r) => {
                failed += 1;
                print!("{}", r.summary());
            }
            Err(err) => {
                failed += 1;
                println!("{name}: {err:#}");
            }
        }
    }
    println!(
        "compare --self: {} entries, {} ok, {failed} failed",
        names.len(),
        names.len() - failed
    );
    if failed > 0 {
        bail!("compare --self: {failed} entries failed");
    }
    Ok(())
}

/// The hand-written module of a `ported` entry: `wgsl = "path#entry"` (path from the repo root,
/// `entry` the entry-point name; the body is `<entry>_body`). `--module` replaces the file by an
/// ordered list of files, the way `wgsl_module!` concatenates them.
fn cmd_compare(mut args: Vec<String>) -> Result<()> {
    let modules: Vec<PathBuf> = take_flag(&mut args, "--module")?
        .into_iter()
        .map(PathBuf::from)
        .collect();
    let only_seed = take_flag(&mut args, "--seed")?
        .pop()
        .map(|s| s.parse::<u64>())
        .transpose()
        .context("--seed needs a number")?;
    let self_check = take_switch(&mut args, "--self");
    let all = take_switch(&mut args, "--all");
    let cat = load_catalog_arg(&mut args)?;
    let status_path = status_path_arg(&mut args)?;
    if self_check {
        return compare_self(&cat, &args, all);
    }
    let [what] = args.as_slice() else {
        bail!("compare: give one entry (Package::Entry[:STAGE])\n{USAGE}");
    };
    let e = cat
        .find_entry(what)
        .with_context(|| format!("compare: no entry {what:?} in the catalog"))?;
    let st = StatusFile::load(&status_path)?;
    let se = st
        .entry
        .get(&e.name)
        .with_context(|| format!("compare: {} is not in {}", e.name, status_path.display()))?;
    if se.status != status::Status::Ported {
        bail!(
            "compare: {} has the status {:?}; only `ported` entries have a module to compare",
            e.name,
            se.status.name()
        );
    }
    let cache_root = bins::cache_root(None)?;
    let (sha1, shader) = entry_block(&cache_root, e)?;
    if se.sha1.as_deref() != Some(sha1.as_str()) {
        bail!(
            "compare: {} was ported from block {:?}, the catalog has {sha1}: port again or fix status.toml",
            e.name,
            se.sha1
        );
    }
    let target = se
        .wgsl
        .as_deref()
        .with_context(|| format!("compare: {} has no wgsl in status.toml", e.name))?;
    let (file, entry_fn) = target
        .split_once('#')
        .with_context(|| format!("compare: wgsl = {target:?} must be path#entry"))?;
    let repo = repo_root()?;
    let files: Vec<PathBuf> = if modules.is_empty() {
        se.modules
            .iter()
            .map(|m| repo.join(m))
            .chain(std::iter::once(repo.join(file)))
            .collect()
    } else {
        modules
    };
    let mut text = String::new();
    for f in &files {
        text += &std::fs::read_to_string(f).with_context(|| format!("reading {}", f.display()))?;
        text.push('\n');
    }
    let stage = if e.stage == "VS" {
        scan::Stage::Vs
    } else {
        scan::Stage::Ps
    };
    let subject = Subject::hand(target, &text, stage, &format!("{entry_fn}_body"))?;
    let formats = load_formats()?;
    let gpu = Gpu::new().map_err(|e| anyhow::anyhow!("compare needs a GPU adapter: {e}"))?;
    println!("compare: {} ({:?})", gpu.info.name, gpu.info.backend);
    let mut opts = compare::CompareOptions::new(&formats);
    opts.zero_in_compare = &se.zero_in_compare;
    opts.data_moves = &se.data_moves;
    opts.input_ranges = se
        .input_ranges
        .iter()
        .map(|r| (r.input.clone(), (r.min, r.max)))
        .collect();
    let seeds = only_seed.map(|s| vec![s]);
    if let Some(s) = &seeds {
        opts.seeds = s;
    }
    let report = compare::compare_entry(&gpu, &shader, &e.name, &sha1, &subject, &opts)?;
    print!("{}", report.summary());
    record_compare(&e.name, &sha1, report.pass())?;
    if !report.pass() {
        bail!("compare: {} differs from the oracle", e.name);
    }
    Ok(())
}

/// The DX11 binary: its blocks (cached on disk, as `extract` does) and the folder `dx11/` of the
/// cache entry of the exe.
struct Dx11 {
    data: Vec<u8>,
    blocks: Vec<scan::Block>,
    /// `<cache>/<sha256-12>/dx11`.
    dir: PathBuf,
    /// `<cache>/<sha256-12>/blocks`.
    blocks_dir: PathBuf,
}

impl Dx11 {
    fn load(args: &mut Vec<String>) -> Result<Dx11> {
        let search = search_from(args)?;
        let cache = take_flag(args, "--cache")?.pop().map(PathBuf::from);
        let cache_root = bins::cache_root(cache.as_deref())?;
        let bin = bins::find(BinId::Dx11, &search)?;
        let data = bin.read()?;
        let blocks = scan::extract(BinId::Dx11, &data)?;
        scan::verify_pins(BinId::Dx11, &blocks)?;
        let key = bins::bin_cache_dir(&cache_root, &bin);
        scan::write_cache(&key, &bin, &data, &blocks)?;
        Ok(Dx11 {
            data,
            blocks,
            dir: key.join("dx11"),
            blocks_dir: key.join("blocks"),
        })
    }

    fn bytes(&self, b: &scan::Block) -> &[u8] {
        &self.data[b.offset..b.offset + b.size]
    }

    /// Every container, parsed; the first failure names the block.
    fn parse_all(&self) -> Result<Vec<shader_re::dxbc::Dxbc>> {
        self.blocks
            .iter()
            .map(|b| {
                shader_re::dxbc::Dxbc::parse(self.bytes(b)).with_context(|| {
                    format!("DX11 block {} ({} at {:#x})", b.sha1, b.profile, b.offset)
                })
            })
            .collect()
    }

    fn python(args: &mut Vec<String>) -> Result<String> {
        Ok(take_flag(args, "--python")?
            .pop()
            .or_else(|| std::env::var("PYTHON").ok().filter(|v| !v.is_empty()))
            .unwrap_or_else(|| "python".to_string()))
    }

    /// Listings of every distinct block, made when they are not in the cache yet.
    fn ensure_listings(&self, python: &str) -> Result<()> {
        let mut sha1s: Vec<String> = self.blocks.iter().map(|b| b.sha1.clone()).collect();
        sha1s.sort();
        sha1s.dedup();
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("py/d3d_disasm.py");
        shader_re::dxbc::asm_text::ensure_listings(
            python,
            &script,
            &self.dir,
            &self.blocks_dir,
            &sha1s,
        )?;
        Ok(())
    }
}

fn cmd_dx11(mut args: Vec<String>) -> Result<()> {
    if args.is_empty() {
        bail!("dx11: give asm, info, twins or cbmap\n{USAGE}");
    }
    let sub = args.remove(0);
    match sub.as_str() {
        "asm" => {
            let python = Dx11::python(&mut args)?;
            let dx = Dx11::load(&mut args)?;
            dx.ensure_listings(&python)?;
            let mut n = 0;
            for b in &dx.blocks {
                let l = shader_re::dxbc::asm_text::read_listing(&dx.dir, &b.sha1)?;
                n += usize::from(l.profile == b.profile);
            }
            println!(
                "dx11 asm: {} blocks, {n} listings with the profile of the container, {}",
                dx.blocks.len(),
                dx.dir.join("asm").display()
            );
            if n != dx.blocks.len() {
                bail!("dx11 asm: some listings name another profile than the container");
            }
            Ok(())
        }
        "info" => dx11_info(args),
        "twins" => dx11_twins(args),
        "cbmap" => dx11_cbmap(args),
        other => bail!("dx11: unknown subcommand {other:?}\n{USAGE}"),
    }
}

/// One container, dumped.
fn dx11_info(mut args: Vec<String>) -> Result<()> {
    let dx = Dx11::load(&mut args)?;
    let [what] = args.as_slice() else {
        bail!("dx11 info: give one block (sha1 or a unique prefix of it)\n{USAGE}");
    };
    let hits: Vec<&scan::Block> = dx
        .blocks
        .iter()
        .filter(|b| b.sha1.starts_with(what.as_str()))
        .collect();
    let first = hits
        .first()
        .with_context(|| format!("dx11 info: no block with sha1 {what}"))?;
    if hits.iter().any(|b| b.sha1 != first.sha1) {
        bail!("dx11 info: {what} is ambiguous");
    }
    let d = shader_re::dxbc::Dxbc::parse(dx.bytes(first))?;
    println!(
        "{} {} {} bytes, chunks {:?}",
        first.sha1,
        d.profile(),
        d.size,
        d.chunk_names()
    );
    for (name, sig) in [
        ("ISGN", Some(&d.isgn)),
        ("OSGN", Some(&d.osgn)),
        ("PCSG", d.pcsg.as_ref()),
    ] {
        let Some(sig) = sig else { continue };
        for e in &sig.elements {
            println!(
                "  {name} {}{} reg {} mask {:04b} rw {:04b} sv {}",
                e.name,
                e.index,
                e.register,
                e.mask,
                e.rw_mask,
                shader_re::dxbc::chunks::sysval_name(e.sysval)
            );
        }
    }
    let x = d.decls();
    for c in &x.cbuffers {
        println!(
            "  cb{}[{}]{}",
            c.reg,
            c.size,
            if c.dynamic { " dynamic" } else { "" }
        );
    }
    for r in &x.resources {
        println!("  t{} {}", r.reg, r.dim.name());
    }
    for s in &x.samplers {
        println!("  s{} mode {}", s.reg, s.mode);
    }
    println!(
        "  temps {}, instructions {}, fetches {}, other decls {:?}",
        x.temps, x.instructions, x.fetches, x.other_decls
    );
    Ok(())
}

/// Every `draws.json` of the captures of `golden` (`cache/golden/<scene>/draws.json` and
/// `cache/golden/<scene>/<capture>/draws.json`), when there are any.
fn golden_draw_files(cache_root: &Path) -> Vec<PathBuf> {
    let Some(golden) = cache_root.parent().map(|p| p.join("golden")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let subdirs = |d: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(d)
            .map(|rd| {
                rd.filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.is_dir())
                    .collect()
            })
            .unwrap_or_default()
    };
    for scene in subdirs(&golden) {
        out.push(scene.join("draws.json"));
        out.extend(subdirs(&scene).into_iter().map(|c| c.join("draws.json")));
    }
    out.retain(|p| p.is_file());
    out.sort();
    out
}

/// Twins of the DX11 blocks of `dx`; the draw calls of `draws` confirm some.
fn compute_twins(
    cat: &Catalog,
    dx: &Dx11,
    parsed: &[shader_re::dxbc::Dxbc],
    draws: &[PathBuf],
) -> Result<shader_re::dxbc::twins::TwinReport> {
    use shader_re::dxbc::twins;
    let mut report = twins::compute(cat, &dx.blocks, parsed);
    let mut pairs = Vec::new();
    for f in draws {
        pairs.extend(twins::read_draw_pairs(f).map_err(anyhow::Error::msg)?);
    }
    twins::confirm(&mut report, &pairs);
    Ok(report)
}

/// Puts the twins into `Entry::dx11_twins` of the stable catalog and of the per-binary copies
/// that are the same file.
fn write_twins_to_catalog(
    cache_root: &Path,
    cat: &mut Catalog,
    report: &shader_re::dxbc::twins::TwinReport,
) -> Result<()> {
    let stable = cache_root.join("catalog.json");
    let old = std::fs::read_to_string(&stable).ok();
    for e in &mut cat.entries {
        e.dx11_twins = report.catalog_strings(&e.name);
    }
    let json = cat.to_json();
    std::fs::write(&stable, &json).with_context(|| format!("writing {}", stable.display()))?;
    if let Some(old) = old {
        for d in std::fs::read_dir(cache_root)? {
            let p = d?.path().join("catalog.json");
            if p.is_file() && std::fs::read_to_string(&p).ok().as_deref() == Some(old.as_str()) {
                std::fs::write(&p, &json).with_context(|| format!("writing {}", p.display()))?;
            }
        }
    }
    Ok(())
}

/// `dx11 twins`: every container parsed; the twins of the entries (SR-D8); `--confirm
/// <draws.json>` adds draw files to the ones found in `cache/golden`.
fn dx11_twins(mut args: Vec<String>) -> Result<()> {
    use shader_re::dxbc::twins;
    let mut draws: Vec<PathBuf> = take_flag(&mut args, "--confirm")?
        .into_iter()
        .map(PathBuf::from)
        .collect();
    let mut cat = load_catalog_arg(&mut args)?;
    let dx = Dx11::load(&mut args)?;
    let cache_root = bins::cache_root(None)?;
    draws.extend(golden_draw_files(&cache_root));
    let parsed = dx.parse_all()?;
    println!("dx11 twins: {} containers parsed", parsed.len());
    let groups = twins::group_blocks(&dx.blocks);
    println!(
        "dx11 twins: {} shaders (the blocks of one shader, highest profile first)",
        groups.len()
    );
    for (shape, n) in twins::shapes(&groups) {
        println!("  {shape}: {n}");
    }
    let report = compute_twins(&cat, &dx, &parsed, &draws)?;
    let all = &report.twins;
    let world: Vec<&twins::Twin> = all.iter().filter(|t| t.world).collect();
    let n = |v: &[&twins::Twin], f: fn(&twins::Twin) -> bool| v.iter().filter(|t| f(t)).count();
    let all_refs: Vec<&twins::Twin> = all.iter().collect();
    for (what, v) in [("all entries", &all_refs), ("world entries", &world)] {
        println!(
            "dx11 twins: {what}: {} twins, {} strong, {} ambiguous, {} with differing profiles, {} confirmed",
            v.len(),
            n(v, |t| t.quality == twins::Quality::Strong),
            n(v, |t| t.ambiguous),
            n(v, |t| !t.profiles_agree),
            n(v, |t| t.confirmed),
        );
    }
    println!(
        "dx11 twins: {} DX11 shaders without an SM3 block (vertex and pixel), {} draw file(s) read",
        report.unmatched_groups.len(),
        draws.len()
    );
    for c in &report.candidates {
        println!(
            "  no SM3 block: {:<55} {:<8} groups {:?}{}",
            c.entry,
            c.evidence,
            c.groups,
            if c.blocks.is_empty() {
                ""
            } else {
                "  (unique)"
            }
        );
    }
    for t in world
        .iter()
        .filter(|t| t.quality == twins::Quality::Weak || t.ambiguous)
    {
        println!(
            "  check: {:<55} score {:.2} {}{}",
            t.entry,
            t.score,
            if t.quality == twins::Quality::Weak {
                "weak "
            } else {
                ""
            },
            if t.ambiguous { "ambiguous" } else { "" }
        );
    }
    let missing: Vec<String> = cat
        .entries
        .iter()
        .filter(|e| e.world && e.sm3.is_some() && report.twin_of(&e.name).is_none())
        .map(|e| e.name.clone())
        .collect();
    let out = dx.dir.join("twins.json");
    std::fs::create_dir_all(&dx.dir)?;
    std::fs::write(&out, report.to_json()).with_context(|| format!("writing {}", out.display()))?;
    write_twins_to_catalog(&cache_root, &mut cat, &report)?;
    println!("dx11 twins: {}", out.display());
    if !missing.is_empty() {
        bail!("dx11 twins: no twin for {}", missing.join(", "));
    }
    Ok(())
}

/// Entries of milestone M1 whose constants must all be mapped: the pilot (RC-15: `UnitVS`,
/// `UnitPS`, then `UnitAlphaPS`) and the terrain light of WT-D7. `TerrainSpecShaderScreenPS40`
/// is DX11-only: it has no CTAB, so no map can be made from names.
const M1_ENTRIES: [&str; 6] = [
    "UnitShader::UnitVS:VS",
    "UnitShader::UnitPS:PS",
    "UnitShader::UnitAlphaPS:PS",
    "TerrainShader::TerrainRenderCachedDiffusePS:PS",
    "TerrainShader::TerrainRenderCachedDiffuseAndSpecPS:PS",
    "TerrainShader::TerrainShaderScreenPS:PS",
];

/// `tools/shader-re/maps/`.
fn maps_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("maps")
}

/// The map of one entry: automatic from the twin, the hand-written file over it. The maps of the
/// profiles `4_1` and `4_0` are made too and must agree; `disagree` names the constants where
/// they do not.
fn entry_cbmap(
    cat: &Catalog,
    dx: &Dx11,
    report: &shader_re::dxbc::twins::TwinReport,
    cache_root: &Path,
    name: &str,
) -> Result<(shader_re::dxbc::cbmap::CbMap, Vec<String>)> {
    use shader_re::dxbc::{asm_text, cbmap};
    let e = cat
        .find_entry(name)
        .with_context(|| format!("dx11 cbmap: no entry {name:?} in the catalog"))?;
    if e.sm3.is_none() {
        bail!(
            "dx11 cbmap: {} has no SM3 block, so no CTAB: its constants are read from the cbuffers as they are",
            e.name
        );
    }
    let twin = report
        .twin_of(&e.name)
        .with_context(|| format!("dx11 cbmap: {} has no twin", e.name))?;
    let (sm3_sha1, sh) = entry_block(cache_root, e)?;
    let listing = |profile: &str| -> Result<(String, asm_text::Listing)> {
        let sha = twin
            .blocks
            .get(profile)
            .with_context(|| format!("{} has no {profile} twin", e.name))?;
        Ok((sha.clone(), asm_text::read_listing(&dx.dir, sha)?))
    };
    let (sha, l50) = listing("5_0")?;
    let mut map = cbmap::cb_map(&e.name, &sm3_sha1, &sha, &sh, &l50);
    let mut disagree = Vec::new();
    for p in ["4_1", "4_0"] {
        let Ok((s, l)) = listing(p) else { continue };
        let other = cbmap::cb_map(&e.name, &sm3_sha1, &s, &sh, &l);
        for c in &map.consts {
            if other.get(&c.name).map(|o| (o.cb, o.offset, o.rows))
                != Some((c.cb, c.offset, c.rows))
            {
                disagree.push(format!("{} ({p})", c.name));
            }
        }
    }
    if let Some(manual) =
        cbmap::ManualMap::load(&maps_dir(), &e.name).map_err(anyhow::Error::msg)?
    {
        manual.apply(&mut map, &sh).map_err(anyhow::Error::msg)?;
        // A hand-written place is the answer, whatever the profiles said.
        disagree.retain(|d| !manual.consts.keys().any(|k| d.starts_with(k.as_str())));
    }
    Ok((map, disagree))
}

/// `dx11 cbmap <entry> | --m1 | --all`: the map of the constants of an entry (CTAB name ->
/// cbuffer, byte offset, rows) from the twin; `--m1` the entries of milestone M1 and fails when a
/// constant of them is left; `--all` every world entry that has a twin (a summary).
fn dx11_cbmap(mut args: Vec<String>) -> Result<()> {
    let python = Dx11::python(&mut args)?;
    let m1 = take_switch(&mut args, "--m1");
    let all = take_switch(&mut args, "--all");
    let cat = load_catalog_arg(&mut args)?;
    let dx = Dx11::load(&mut args)?;
    let cache_root = bins::cache_root(None)?;
    let parsed = dx.parse_all()?;
    let draws = golden_draw_files(&cache_root);
    let report = compute_twins(&cat, &dx, &parsed, &draws)?;
    dx.ensure_listings(&python)?;
    let names: Vec<String> = if m1 {
        M1_ENTRIES.iter().map(|s| (*s).to_string()).collect()
    } else if all {
        cat.entries
            .iter()
            .filter(|e| e.world && e.sm3.is_some())
            .map(|e| e.name.clone())
            .collect()
    } else {
        let [what] = args.as_slice() else {
            bail!("dx11 cbmap: give one entry, --m1 or --all\n{USAGE}");
        };
        vec![
            cat.find_entry(what)
                .with_context(|| format!("dx11 cbmap: no entry {what:?}"))?
                .name
                .clone(),
        ]
    };
    let out_dir = dx.dir.join("cbmap");
    std::fs::create_dir_all(&out_dir)?;
    let single = names.len() == 1;
    let (mut mapped, mut unmapped, mut inferred, mut entries_full, mut failed) = (0, 0, 0, 0, 0);
    for name in &names {
        let (map, disagree) = match entry_cbmap(&cat, &dx, &report, &cache_root, name) {
            Ok(x) => x,
            Err(e) => {
                println!("{name}: {e:#}");
                failed += 1;
                continue;
            }
        };
        let file =
            out_dir.join(shader_re::dxbc::cbmap::manual_file_name(name).replace(".toml", ".json"));
        std::fs::write(&file, map.to_json())?;
        mapped += map.consts.len();
        unmapped += map.unmapped.len();
        inferred += map.consts.iter().filter(|c| c.source == "inferred").count();
        entries_full += usize::from(map.unmapped.is_empty() && disagree.is_empty());
        if single {
            println!(
                "{name}: block {} twin {}; {} reads of constants in SM3, {} in DX11",
                &map.sm3_sha1[..12],
                &map.dx11_sha1[..12],
                map.sm3_reads,
                map.dx11_reads
            );
            println!(
                "  {:<28} {:>4} {:>4}  {:>3} {:>7}  {:<8} {:>5} {:>5}",
                "constant", "reg", "rows", "cb", "offset", "source", "conf", "reads"
            );
            for c in &map.consts {
                println!(
                    "  {:<28} c{:<3} {:>4}  {:>3} {:>7}  {:<8} {:>5.2} {:>5}",
                    c.name, c.reg, c.rows, c.cb, c.offset, c.source, c.confidence, c.reads
                );
            }
            println!("  file: {}", file.display());
        } else if !map.unmapped.is_empty() || !disagree.is_empty() || !map.suspect.is_empty() {
            println!(
                "{name}: {} mapped, {} unmapped, {} suspect, profiles disagree on {}",
                map.consts.len(),
                map.unmapped.len(),
                map.suspect.len(),
                disagree.len()
            );
        }
        for u in &map.unmapped {
            println!("  unmapped: {} (c{}): {}", u.name, u.reg, u.reason);
        }
        for d in &disagree {
            println!("  profiles disagree: {d}");
        }
        for s in &map.scattered {
            println!("  scattered: {s}");
        }
        for s in &map.suspect {
            println!("  suspect: {s}");
        }
        if m1 && (!map.unmapped.is_empty() || !disagree.is_empty() || !map.suspect.is_empty()) {
            failed += 1;
        }
    }
    println!(
        "dx11 cbmap: {} entries, {entries_full} fully mapped; {mapped} constants mapped ({inferred} with rows inferred), {unmapped} unmapped; {failed} failed; {}",
        names.len(),
        out_dir.display()
    );
    if failed > 0 {
        bail!("dx11 cbmap: {failed} entries incomplete");
    }
    Ok(())
}

fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        println!("{USAGE}");
        bail!("no command");
    }
    let cmd = args.remove(0);
    match cmd.as_str() {
        "extract" => cmd_extract(args),
        "validate" => cmd_validate(args),
        "disasm" => cmd_disasm(args),
        "oracle" => cmd_oracle(args),
        "compare" => cmd_compare(args),
        "crosscheck-d3d" => cmd_crosscheck(args),
        "dx11" => cmd_dx11(args),
        "status-init" => cmd_status_init(args),
        "check" => cmd_check(args),
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            Ok(())
        }
        other => {
            println!("{USAGE}");
            bail!("unknown command: {other:?}");
        }
    }
}
