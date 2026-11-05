//! civ5-assets: offline asset pipeline.

mod probe_gr2;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use asset_pipeline::game::GameSource;
use asset_pipeline::game::source::ExtStat;
use asset_pipeline::gamedata;
use asset_pipeline::models;
use asset_pipeline::report::{Report, SetReport};
use asset_pipeline::sets::{self, TextureOptions};
use clap::{Parser, Subcommand, ValueEnum};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "civ5-assets", version, about)]
struct Cli {
    /// Civ5 install directory (the folder with `Assets` and `Resource`). Falls back to the CIV5_ROOT environment
    /// variable, then to the Steam install found on this machine (Steam's library folders).
    #[arg(long, visible_alias = "game-dir", global = true)]
    game: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

/// Settings of the `ui` set and of the UI file system commands.
#[derive(clap::Args, Clone, Debug)]
struct UiArgs {
    /// UI set: `basegame`, `expansion1` or `expansion2` (Brave New World).
    #[arg(long = "ui-set", default_value = "expansion2")]
    ui_set: String,
    /// Index the `Override` folder of Lekmod: `include` or `exclude`.
    #[arg(long = "lekmod-override", default_value = "exclude")]
    lekmod_override: String,
    /// Package priority, `NAME=VALUE` (package file stem or folder name); repeatable.
    #[arg(long = "priority")]
    priority: Vec<String>,
    /// Priority of a package that has no `<Priority>`.
    #[arg(long = "default-priority", default_value_t = 0)]
    default_priority: i32,
    /// Text languages to write, e.g. `en_US ru_RU`, or `all`.
    #[arg(long = "ui-lang", num_args = 1.., default_values_t = ["en_US".to_owned(), "ru_RU".to_owned()])]
    ui_lang: Vec<String>,
    /// Also write the spoken lines of the interface (tech, era, wonder, advisor).
    #[arg(long = "ui-voice")]
    ui_voice: bool,
    /// Also write the music (civilization playlists, leader themes, main menu loop: about 1.5 GB, streamed).
    #[arg(long = "ui-music")]
    ui_music: bool,
}

impl UiArgs {
    fn options(&self) -> Result<asset_pipeline::ui::UiOptions> {
        let mut o = asset_pipeline::ui::UiOptions {
            set: self.ui_set.clone(),
            include_override: match self.lekmod_override.as_str() {
                "include" => true,
                "exclude" => false,
                other => bail!("--lekmod-override takes include or exclude, not {other:?}"),
            },
            default_priority: self.default_priority,
            voice: self.ui_voice,
            music: self.ui_music,
            ..Default::default()
        };
        for p in &self.priority {
            let (k, v) = p
                .split_once('=')
                .with_context(|| format!("--priority {p:?}: expected NAME=VALUE"))?;
            o.priority_overrides.insert(
                k.trim().to_ascii_lowercase(),
                v.trim()
                    .parse()
                    .with_context(|| format!("--priority {p:?}"))?,
            );
        }
        o.langs = if self.ui_lang.iter().any(|l| l.eq_ignore_ascii_case("all")) {
            None
        } else {
            Some(self.ui_lang.clone())
        };
        Ok(o)
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum ProbeKind {
    Gr2,
    Dds,
}

#[derive(Subcommand)]
enum Command {
    /// Index the install and print pack/file statistics.
    Scan,
    /// Decode every file of one kind and report failures.
    Probe {
        #[arg(long, value_enum)]
        kind: ProbeKind,
    },
    /// Convert an asset set into content-addressed blobs.
    Build {
        /// Output directory (blobs, build cache and the `manifest` pointer); keep it outside the repo.
        #[arg(long, default_value = "../assets-out")]
        out: PathBuf,
        /// Set to build; repeat for several (`--set a --set b`). Composite `b1` expands to its
        /// member sets. All of them go into one manifest.
        #[arg(long = "set", default_value = "warrior")]
        sets: Vec<String>,
        /// Directory of the game DB snapshot (`Civ5DebugDatabase.db`, `Localization-Merged.db`);
        /// falls back to CIV5_DB_SNAPSHOT. Required by `b1` and `game-data`.
        #[arg(long)]
        db: Option<PathBuf>,
        /// Pinned sha256 sums of the snapshot files. Default: tools/asset-pipeline/gamedata.lock (the reference
        /// snapshot; another snapshot is a warning). A lock named here must match (see `lock-gamedata`).
        #[arg(long)]
        gamedata_lock: Option<PathBuf>,
        /// civhost load log to compare the Lua closure with (optional; differences are warnings).
        #[arg(long)]
        civhost_log: Option<PathBuf>,
        /// Largest side of converted B1 textures (sets b1-*); 0 keeps the source size.
        #[arg(long, default_value_t = 512)]
        max_texture: u32,
        /// Write the B1 textures as BC7 only, without their ASTC variant (SIMP-022).
        #[arg(long)]
        no_astc: bool,
        /// Skip the texture variants pass at the end (BC7 and ASTC variants of every texture;
        /// same as the `variants` command).
        #[arg(long)]
        no_texture_variants: bool,
        /// `catalog.json` of `shader-re`, names for the FGX blocks of the B2 models (set
        /// `b2-units`). Default: `cache/shader-re/catalog.json` next to the workspace, if it
        /// exists.
        #[arg(long)]
        shader_catalog: Option<PathBuf>,
        /// Largest angle in degrees a packed clip may be off the reference rotation (set
        /// `b2-units`, SIMP-029). Default: the value of `asset_pipeline::anim::DEFAULT_ROT_DEG`.
        #[arg(long)]
        clip_rot_deg: Option<f64>,
        /// After the sets: repack RGBA8 textures to BC1 / BC7 (same as the `repack` command).
        #[arg(long)]
        repack_textures: bool,
        #[command(flatten)]
        ui: UiArgs,
    },
    /// Print the layers of the UI file system (by `Civ5Pkg` priority) and the name collisions.
    UiVfs {
        #[command(flatten)]
        ui: UiArgs,
        /// Also write the report as JSON.
        #[arg(long)]
        json: Option<PathBuf>,
    },
    /// What the `Override` folder of a package would change.
    UiOverride {
        #[command(flatten)]
        ui: UiArgs,
    },
    /// Compare the texts the XML files merge to with the real game's `Localization-Merged.db` (the DB snapshot), per
    /// language: differing, missing and extra tags. Exit code 1 when the build's texts (the snapshot applied unless
    /// `--xml-only`) still differ.
    UiLocDiff {
        #[command(flatten)]
        ui: UiArgs,
        /// Directory of the DB snapshot; falls back to CIV5_DB_SNAPSHOT.
        #[arg(long)]
        db: Option<PathBuf>,
        /// Compare the XML merge alone (before the snapshot is applied) instead of the build's texts.
        #[arg(long)]
        xml_only: bool,
        /// Write the full list of differences here.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Size report of a B2 output directory: by load group, by id prefix and by budget article
    /// against 600 MB. Writes `report-b2.json` and `report-b2.txt` there; fails when a limit is
    /// broken (total over 600 MB, `boot` over 30 MB).
    Report {
        /// Output directory of the B2 build (the one with the `manifest` pointer).
        #[arg(long)]
        out: PathBuf,
    },
    /// Check a B2 output directory: every blob is there and reads with its own type, every group
    /// member and every texture the settings name is in the manifest, `boot` agrees with the boot
    /// flag, and no stub event is missing from `stubs.allow`. Fails with the list of problems.
    Verify {
        #[arg(long)]
        out: PathBuf,
        /// Allow list of stand-ins (default: tools/asset-pipeline/stubs.allow).
        #[arg(long)]
        stubs_allow: Option<PathBuf>,
    },
    /// Write the game data for the server as a folder: vfs-index.json with
    /// all map scripts of the DB and the Lua closure of each, the world-builder entry file,
    /// every .Civ5Map under maps/ (index section civ5_maps), db/, gamedata.lock, and last
    /// manifest.sha256. FIX-0004 (Lekmod_policies.lua from the Lekmod repository) is applied
    /// unless --no-lua-override is given. Used by `runtime/run.sh --game <dir>` in Lekmod.
    ExportGamedata {
        /// Output folder; must be absent, empty or an earlier export (replaced).
        #[arg(long)]
        out: PathBuf,
        /// Other Lua replacements, `FILE=PATH` (repeatable); the file of that name in the Lua
        /// closure is read from PATH. Replaces the default FIX-0004 file of the same name.
        #[arg(long = "lua-override", value_name = "FILE=PATH")]
        lua_overrides: Vec<String>,
        /// Do not apply FIX-0004; the installed Lekmod_policies.lua is exported (regression
        /// against the installed game). The state is written to gamedata.lock.
        #[arg(long)]
        no_lua_override: bool,
        /// Lekmod repository for FIX-0004 (default: LEKMOD_REPO, else LEKMOD_ROOT, else a Lekmod
        /// folder next to the repository).
        #[arg(long)]
        lekmod_repo: Option<PathBuf>,
        /// Directory of the game DB snapshot; falls back to CIV5_DB_SNAPSHOT.
        #[arg(long)]
        db: Option<PathBuf>,
        /// Pinned sha256 sums of the snapshot files (default: tools/asset-pipeline/gamedata.lock).
        #[arg(long)]
        gamedata_lock: Option<PathBuf>,
        /// civhost load log to compare the Lua closure with (optional; differences are warnings).
        #[arg(long)]
        civhost_log: Option<PathBuf>,
    },
    /// Remove the blobs that the last N manifests of an output folder do not need (the server's
    /// asset folder grows with every release otherwise). Only 64-digit-hash file names are
    /// touched; `build` keeps the history in manifests.log.
    Gc {
        /// Output folder with the `manifest` pointer.
        #[arg(long)]
        out: PathBuf,
        /// How many of the latest manifests to keep (the current one counts).
        #[arg(long, default_value_t = asset_pipeline::gc::DEFAULT_KEEP)]
        keep: usize,
        /// Only list what would be removed.
        #[arg(long)]
        dry_run: bool,
    },
    /// Write `<file>.zst` (zstd) and `<file>.br` (brotli, needs the `brotli` binary) next to the
    /// files of a folder, for the gateway and Caddy to serve by Accept-Encoding. A sidecar is
    /// kept only for files of 256 bytes or more and if it is at most 95 % of the original.
    Precompress {
        /// Asset folder (blobs) or web root.
        #[arg(long)]
        dir: PathBuf,
        /// `brotli` binary (default: found in PATH; without it only zstd is made).
        #[arg(long)]
        brotli_bin: Option<PathBuf>,
        #[arg(long, default_value_t = asset_pipeline::precompress::DEFAULT_ZSTD_LEVEL)]
        level_zstd: i32,
        /// Only files named by 64 hex digits (the blobs of an asset folder).
        #[arg(long)]
        only_hash_names: bool,
    },
    /// Print (or write to --out) a lock file with the sha256 of the two files of a DB snapshot, for
    /// `--gamedata-lock`: pins your own snapshot instead of the reference one in gamedata.lock.
    LockGamedata {
        /// Directory of the DB snapshot; falls back to CIV5_DB_SNAPSHOT.
        #[arg(long)]
        db: Option<PathBuf>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Write (or, with --check, verify) manifest.sha256 of a game-data folder. Run after adding
    /// a file to an export, e.g. map-scripts.json from the `mapscripts` scenario.
    SealGamedata {
        /// Folder made by export-gamedata.
        #[arg(long)]
        dir: PathBuf,
        /// Only compare manifest.sha256 with the files; fails on any difference.
        #[arg(long)]
        check: bool,
    },
    /// Texture formats and sizes of an output folder: bytes by format and id prefix, and the
    /// largest RGBA8 2D textures (the BC7 candidates of B3 asset-delivery).
    TexAudit {
        #[arg(long)]
        out: PathBuf,
        /// How many candidates to list.
        #[arg(long, default_value_t = 40)]
        top: usize,
    },
    /// Repack the RGBA8 2D textures of an output folder to BC1 (opaque) or BC7. A texture is
    /// replaced only when PSNR of level 0 and 1 is at least --min-psnr in RGB and alpha; the
    /// exceptions list (repack-exceptions.txt) and the texture-array layers of the art tables stay
    /// RGBA8. Idempotent. Prints the table of decisions and the totals.
    Repack {
        #[arg(long)]
        out: PathBuf,
        /// Exceptions list, `pattern<TAB>reason` per line (default: repack-exceptions.txt, built in).
        #[arg(long)]
        exceptions: Option<PathBuf>,
        /// Decide and print, write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Gate for RGB and alpha, dB.
        #[arg(long, default_value_t = asset_pipeline::textures::repack::MIN_PSNR)]
        min_psnr: f64,
    },
    /// Add the compressed variants of every texture of an output folder:
    /// BC7 for the plain colour textures (desktop), ASTC 4x4 for all (phones; the top level dropped
    /// above 1024), each checked against its source. Writes `report-textures.txt` with the bytes per
    /// variant and the textures kept uncompressed, each with its rule. Idempotent; `build` runs it
    /// last.
    Variants {
        #[arg(long)]
        out: PathBuf,
        /// Decide and encode, write nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Check the FGX materials of every unit model: names, shader sets, texture paths.
    FgxCheck {
        /// `catalog.json` of `shader-re` (default: `cache/shader-re/catalog.json` next to the
        /// workspace, when it exists). With a catalog an unknown name fails the check; without,
        /// the unknown names are listed.
        #[arg(long)]
        shader_catalog: Option<PathBuf>,
    },
    /// Copy visible files out of the install by name glob (`*` and `?`, case-insensitive,
    /// names have no directories). Without `--out` only lists the matches.
    Extract {
        #[arg(long)]
        name: String,
        /// Folder for read-only copies; created if absent. Keep it outside the repo.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Export one model as glTF for inspection.
    Gltf {
        /// Unit member art id, e.g. ART_DEF_UNIT_MEMBER_WARRIOR; or `city:EUROPEAN/Ancient`,
        /// `forest:europe`, `jungle:asia` for the B1 city and tree models.
        #[arg(long)]
        model: String,
        #[arg(long)]
        out: PathBuf,
    },
}

fn game_root(cli: &Cli) -> Result<PathBuf> {
    if let Some(g) = cli
        .game
        .clone()
        .or_else(|| std::env::var_os("CIV5_ROOT").map(PathBuf::from))
    {
        return Ok(g);
    }
    let found = asset_pipeline::game::steam::find_game_dir().context(
        "game directory not set and no Steam install of Civilization V found: pass --game <dir> or set CIV5_ROOT",
    )?;
    eprintln!(
        "game directory: {} (found in the Steam libraries)",
        found.display()
    );
    Ok(found)
}

fn print_histogram(title: &str, hist: &BTreeMap<String, ExtStat>) {
    let total = hist.values().fold(ExtStat::default(), |a, s| ExtStat {
        count: a.count + s.count,
        bytes: a.bytes + s.bytes,
    });
    println!(
        "{title}: {} files, {:.1} MB",
        total.count,
        total.bytes as f64 / 1e6
    );
    let mut rows: Vec<_> = hist.iter().collect();
    rows.sort_by(|a, b| b.1.bytes.cmp(&a.1.bytes).then(a.0.cmp(b.0)));
    for (ext, s) in rows {
        let ext = if ext.is_empty() { "(none)" } else { ext };
        println!(
            "  .{ext:<10} {:>8} {:>10.1} MB",
            s.count,
            s.bytes as f64 / 1e6
        );
    }
}

fn scan(root: PathBuf) -> Result<()> {
    let src = GameSource::open(&root).with_context(|| format!("scanning {}", root.display()))?;
    let st = src.stats();
    println!("game root: {}", root.display());
    println!(
        "lekmod folder: {}",
        src.lekmod_dir()
            .unwrap_or("none (Lekmod v34.15 not installed, see docs/install.md#the-game)")
    );
    println!("packs: {} (Resource/Common + Resource/DX9)", st.packs);
    print_histogram("packed entries", &st.packed);
    print_histogram("loose files", &st.loose);
    println!(
        "visible names: {}; shadowed duplicates: {}",
        src.len(),
        st.shadowed
    );
    if st.excluded.is_empty() {
        println!("excluded dirs: none");
    } else {
        println!("excluded dirs:");
        for e in &st.excluded {
            println!("  {} ({})", e.dir, e.reason);
        }
    }
    if !st.pack_errors.is_empty() {
        println!("pack errors:");
        for (path, err) in &st.pack_errors {
            println!("  {}: {err}", path.display());
        }
        bail!("{} pack(s) failed to parse", st.pack_errors.len());
    }
    Ok(())
}

fn build_ui_set(game: &Path, out: &Path, opts: &asset_pipeline::ui::UiOptions) -> Result<()> {
    let s = asset_pipeline::ui::build_ui(game, out, opts)?;
    std::fs::create_dir_all(out)?;
    std::fs::write(out.join("report-ui.txt"), s.doc.to_text())?;
    std::fs::write(
        out.join("report-ui.json"),
        serde_json::to_vec_pretty(&s.doc)?,
    )?;
    let mut report = Report::load(out);
    report.put(asset_pipeline::ui::SET_NAME, s.report);
    report.write(out)?;
    println!("set ui: {:.1}s", s.seconds);
    for (k, v) in &s.doc.counts {
        println!("  {k}: {v}");
    }
    for (k, v) in &s.doc.bytes {
        println!("  {k}: {:.2} MB", *v as f64 / 1e6);
    }
    for n in s.doc.notes.iter().filter(|n| n.starts_with("AC-P2")) {
        println!("  {n}");
    }
    println!(
        "report: report-ui.txt and report-ui.json in {}",
        out.display()
    );
    Ok(())
}

/// Set `web-lobby`: the art and the data of the DOM lobby page (`web:lobby`, `web:png:*`).
fn build_web_lobby_set(
    game: &Path,
    db: &Path,
    out: &Path,
    opts: &asset_pipeline::ui::UiOptions,
) -> Result<()> {
    let name = sets::web_lobby::SET_NAME;
    let s = sets::web_lobby::build(game, db, out, opts).with_context(|| format!("set {name}"))?;
    let st = &s.stats;
    println!(
        "set {name}: {} civ(s): {} leader portrait(s), {} civ icon(s), {}/{} unique cell(s); {} font icon(s) ({} dropped) in {} texture(s); {} color(s); {:.2}s",
        st.civs,
        st.civ_portraits,
        st.civ_icons,
        st.unique_cells,
        st.uniques,
        st.icons,
        st.icons_dropped,
        st.font_pngs,
        st.colors,
        s.seconds
    );
    println!(
        "{}: {} B; {} PNG(s), {:.2} MB; {} warning(s)",
        sets::web_lobby::LOBBY_ID,
        s.lobby_bytes,
        s.png_count,
        s.png_bytes as f64 / 1e6,
        s.warnings.len()
    );
    for w in s.warnings.iter().take(40) {
        println!("warning: {} {}: {}", w.kind, w.subject, w.reason);
    }
    if s.warnings.len() > 40 {
        println!("... and {} more (see report-b1.txt)", s.warnings.len() - 40);
    }
    let mut report = Report::load(out);
    report.put(name, s.report);
    report.write(out)?;
    asset_pipeline::gc::record(out)
        .with_context(|| format!("recording the manifest of {}", out.display()))?;
    Ok(())
}

fn ui_vfs_command(
    game: &Path,
    opts: &asset_pipeline::ui::UiOptions,
    json: Option<&Path>,
) -> Result<()> {
    let vfs = asset_pipeline::ui::open_vfs(game, opts)?;
    let mut doc = asset_pipeline::ui::UiReportDoc {
        layers: asset_pipeline::ui::layer_rows(&vfs),
        collisions: asset_pipeline::ui::collision_summary(&vfs)?,
        ..Default::default()
    };
    doc.notes.push(format!(
        "files under Scenarios folders left out: {}",
        vfs.skipped_scenario_files
    ));
    for s in &vfs.skipped_packages {
        doc.notes
            .push(format!("package {} skipped: {}", s.stem, s.reason));
    }
    for (p, n) in &vfs.missing_packs {
        doc.notes.push(format!(
            "package {p} lists pack {n}, which the install lacks"
        ));
    }
    doc.notes.push(format!(
        "Override files {} ({})",
        vfs.override_files.len(),
        if opts.include_override {
            "indexed"
        } else {
            "left out"
        }
    ));
    let text = doc.to_text();
    print!("{text}");
    if let Some(j) = json {
        std::fs::write(j, serde_json::to_vec_pretty(&doc)?)?;
    }
    Ok(())
}

fn ui_loc_diff_command(
    game: &Path,
    opts: &asset_pipeline::ui::UiOptions,
    db: Option<PathBuf>,
    xml_only: bool,
    out: Option<&Path>,
) -> Result<()> {
    use asset_pipeline::ui::oracle;
    let snapshot = snapshot_dir(db)?.context("pass --db <snapshot dir> or set CIV5_DB_SNAPSHOT")?;
    let loc_db = snapshot.join(gamedata::LOC_DB_FILE);
    let mut texts = asset_pipeline::ui::xml_texts(game, opts)?;
    let langs: Vec<String> = match &opts.langs {
        Some(l) => l.clone(),
        None => texts.langs.values().map(|(n, _)| n.clone()).collect(),
    };
    let mut full = String::new();
    let mut remaining = 0;
    for lang in &langs {
        let rows = oracle::read_language(&loc_db, lang)?;
        if !xml_only {
            oracle::apply(&mut texts, rows.clone(), lang, false);
        }
        let d = oracle::diff(&texts, &rows, lang);
        println!(
            "{lang}: snapshot {} rows, build {} rows: {} texts differ, {} tags missing, {} tags extra, {} grammar rows differ",
            d.oracle_rows,
            d.our_rows,
            d.differing.len(),
            d.missing.len(),
            d.extra.len(),
            d.grammar.len()
        );
        remaining += d.total();
        for (tag, ours, theirs) in &d.differing {
            full.push_str(&format!(
                "{lang} DIFF {tag}
  build:    {ours:?}
  snapshot: {theirs:?}
"
            ));
        }
        for (tag, theirs) in &d.missing {
            full.push_str(&format!(
                "{lang} MISSING {tag}
  snapshot: {theirs:?}
"
            ));
        }
        for (tag, ours) in &d.extra {
            full.push_str(&format!(
                "{lang} EXTRA {tag}
  build:    {ours:?}
"
            ));
        }
        for tag in &d.grammar {
            full.push_str(&format!(
                "{lang} GRAMMAR {tag}
"
            ));
        }
    }
    if let Some(out) = out {
        std::fs::write(out, &full)?;
        println!("written: {}", out.display());
    }
    if remaining > 0 {
        std::process::exit(1);
    }
    Ok(())
}

fn ui_override_command(game: &Path, opts: &asset_pipeline::ui::UiOptions) -> Result<()> {
    let s = asset_pipeline::ui::override_report(game, opts)?;
    println!("{}", serde_json::to_string_pretty(&s)?);
    Ok(())
}

/// Opens the install for a set or a tool; an install without a Lekmod v34.15 folder is an error
/// (without it the Lekmod layer would be skipped silently).
fn open_game(root: &Path) -> Result<GameSource> {
    let src = GameSource::open(root).with_context(|| format!("scanning {}", root.display()))?;
    if src.lekmod_dir().is_none() {
        let others: Vec<&str> = src
            .stats()
            .excluded
            .iter()
            .map(|e| e.dir.as_str())
            .filter(|d| d.to_ascii_lowercase().starts_with("lekmod"))
            .collect();
        let found = if others.is_empty() {
            "no LEKMOD folder".to_owned()
        } else {
            format!("only {}", others.join(", "))
        };
        bail!(
            "Lekmod v34.15 is not installed in {}: {found} under Assets/DLC. Copy the LEKMOD folder of \
             Lekmod v34.15 to Assets/DLC/{} (LEKMODv34_15 and other spellings of v34.15 work too); \
             see docs/install.md#the-game",
            root.display(),
            asset_pipeline::game::source::LEKMOD_DIR
        );
    }
    Ok(src)
}

#[allow(clippy::too_many_arguments)]
fn build(
    root: Result<PathBuf>,
    out: &Path,
    sets: &[String],
    db: Option<PathBuf>,
    gamedata_lock: Option<&Path>,
    civhost_log: Option<&Path>,
    textures: TextureOptions,
    shader_catalog: Option<&Path>,
    clip_rot_deg: Option<f64>,
    repack_textures: bool,
) -> Result<()> {
    let db = snapshot_dir(db)?;
    let expanded = models::set::expand_names(sets);
    let want_game_data = expanded
        .iter()
        .any(|n| n.eq_ignore_ascii_case(models::set::GAME_DATA));
    let want_terrain = expanded
        .iter()
        .any(|n| n.eq_ignore_ascii_case(sets::terrain::SET_NAME));
    let want_sv = expanded
        .iter()
        .any(|n| n.eq_ignore_ascii_case(sets::sv::SET_NAME));
    let want_trees = expanded
        .iter()
        .any(|n| n.eq_ignore_ascii_case(sets::trees::SET_NAME));
    let want_cities = expanded
        .iter()
        .any(|n| n.eq_ignore_ascii_case(sets::cities::SET_NAME));
    let want_units = expanded
        .iter()
        .any(|n| n.eq_ignore_ascii_case(sets::units::SET_NAME));
    let want_light = expanded
        .iter()
        .any(|n| n.eq_ignore_ascii_case(sets::b2_light::SET_NAME));
    let want_b2_terrain = expanded
        .iter()
        .any(|n| n.eq_ignore_ascii_case(sets::b2_terrain::SET_NAME));
    let want_b2_water = expanded
        .iter()
        .any(|n| n.eq_ignore_ascii_case(sets::b2_water::SET_NAME));
    let want_table = expanded
        .iter()
        .any(|n| n.eq_ignore_ascii_case(sets::table::SET_NAME));
    let want_units_b2 = expanded
        .iter()
        .any(|n| n.eq_ignore_ascii_case(sets::b2_units::SET_NAME));
    let want_landmarks = expanded
        .iter()
        .any(|n| n.eq_ignore_ascii_case(sets::b2_landmarks::SET_NAME));
    let want_city = expanded
        .iter()
        .any(|n| n.eq_ignore_ascii_case(sets::b2_city::SET_NAME));
    let want_misc = expanded
        .iter()
        .any(|n| n.eq_ignore_ascii_case(sets::b2_misc::SET_NAME));
    let want_fx = expanded
        .iter()
        .any(|n| n.eq_ignore_ascii_case(sets::b2_fx::SET_NAME));
    let want_client_data = expanded
        .iter()
        .any(|n| n.eq_ignore_ascii_case(sets::client_data::SET_NAME));
    if db.is_none()
        && (want_game_data
            || want_units_b2
            || want_landmarks
            || want_city
            || want_misc
            || want_table
            || want_terrain
            || want_sv
            || want_trees
            || want_cities
            || want_units
            || sets.iter().any(|s| models::set::is_composite(s)))
    {
        bail!(
            "sets b1, b1-terrain, b1-trees, b1-cities, b1-sv, b1-units, b1-table and game-data need the DB snapshot directory: pass --db <dir> or set {}",
            gamedata::SNAPSHOT_ENV
        );
    }
    let defs = models::set::resolve_sets(sets).map_err(anyhow::Error::msg)?;
    let game_dir = root.as_ref().ok().cloned();
    let mut report = Report::load(out);
    let mut report_changed = false;
    if !defs.is_empty() {
        build_unit_sets(root?, out, &defs)?;
    }
    if want_terrain && let Some(db) = &db {
        let game = game_dir.clone().context(
            "set b1-terrain needs the game directory: pass --game <dir> or set CIV5_ROOT",
        )?;
        build_terrain_set(&game, db, out, textures)?;
    }
    if want_light {
        let game = game_dir
            .clone()
            .context("set b2-light needs the game directory: pass --game <dir> or set CIV5_ROOT")?;
        build_light_set(&game, out)?;
    }
    if want_b2_terrain {
        let game = game_dir.clone().context(
            "set b2-terrain needs the game directory: pass --game <dir> or set CIV5_ROOT",
        )?;
        build_b2_terrain_set(&game, out)?;
    }
    if want_b2_water {
        let game = game_dir
            .clone()
            .context("set b2-water needs the game directory: pass --game <dir> or set CIV5_ROOT")?;
        build_b2_water_set(&game, out)?;
    }
    if want_trees {
        let game = game_dir
            .clone()
            .context("set b1-trees needs the game directory: pass --game <dir> or set CIV5_ROOT")?;
        build_trees_set(&game, out, textures, &mut report)?;
        report_changed = true;
    }
    if want_cities {
        let game = game_dir.clone().context(
            "set b1-cities needs the game directory: pass --game <dir> or set CIV5_ROOT",
        )?;
        build_cities_set(&game, out, textures, &mut report)?;
        report_changed = true;
    }
    if want_sv && let Some(db) = &db {
        let game = game_dir
            .clone()
            .context("set b1-sv needs the game directory: pass --game <dir> or set CIV5_ROOT")?;
        build_sv_set(&game, db, out, textures, &mut report)?;
        report_changed = true;
    }
    if want_units && let Some(db) = &db {
        let game = game_dir
            .clone()
            .context("set b1-units needs the game directory: pass --game <dir> or set CIV5_ROOT")?;
        build_units_set(&game, db, out, textures, &mut report)?;
        report_changed = true;
    }
    let mut units_b2_error = None;
    if want_units_b2 && let Some(db) = &db {
        let game = game_dir
            .clone()
            .context("set b2-units needs the game directory: pass --game <dir> or set CIV5_ROOT")?;
        match build_units_b2_set(&game, db, out, shader_catalog, clip_rot_deg, &mut report) {
            Ok(()) => {}
            Err(e) => units_b2_error = Some(e),
        }
        report_changed = true;
    }
    let mut landmarks_error = None;
    if want_landmarks && let Some(db) = &db {
        let game = game_dir.clone().context(
            "set b2-landmarks needs the game directory: pass --game <dir> or set CIV5_ROOT",
        )?;
        if let Err(e) =
            build_landmarks_set(&game, db, out, shader_catalog, clip_rot_deg, &mut report)
        {
            landmarks_error = Some(e);
        }
        report_changed = true;
    }
    let mut city_error = None;
    if want_city && let Some(db) = &db {
        let game = game_dir
            .clone()
            .context("set b2-city needs the game directory: pass --game <dir> or set CIV5_ROOT")?;
        if let Err(e) = build_city_set(&game, db, out, shader_catalog, clip_rot_deg, &mut report) {
            city_error = Some(e);
        }
        report_changed = true;
    }
    let mut misc_error = None;
    if want_misc && let Some(db) = &db {
        let game = game_dir
            .clone()
            .context("set b2-misc needs the game directory: pass --game <dir> or set CIV5_ROOT")?;
        if let Err(e) = build_misc_set(&game, db, out, shader_catalog, clip_rot_deg, &mut report) {
            misc_error = Some(e);
        }
        report_changed = true;
    }
    let mut fx_error = None;
    if want_fx {
        let game = game_dir
            .clone()
            .context("set b2-fx needs the game directory: pass --game <dir> or set CIV5_ROOT")?;
        if let Err(e) = build_fx_set(&game, out, &mut report) {
            fx_error = Some(e);
        }
        report_changed = true;
    }
    if want_client_data {
        let game = game_dir.clone().context(
            "set client-data needs the game directory: pass --game <dir> or set CIV5_ROOT",
        )?;
        build_client_data_set(&game, out)?;
    }
    // The groups of the world objects (civ, era, obj, trees, roads, the overlays in boot) come
    // from the art table and the manifest as the sets left them, so they are made after the last
    // of the sets that fill those.
    if (want_units_b2 || want_landmarks || want_city || want_misc)
        && let Some(db) = &db
    {
        build_object_groups(db, out)?;
    }
    // Last of the table sets: checks the table the others filled. The report is written even when
    // the check fails, so it lists what is missing.
    let mut table_error = None;
    if want_table && let Some(db) = &db {
        match check_table(db, out) {
            Ok(r) => report.put(sets::table::SET_NAME, r),
            Err(b) => {
                let (r, e) = *b;
                report.put(sets::table::SET_NAME, r);
                table_error = Some(e);
            }
        }
        report_changed = true;
    }
    if report_changed {
        std::fs::create_dir_all(out)?;
        report
            .write(out)
            .with_context(|| format!("writing the report into {}", out.display()))?;
        println!(
            "report: {} and {} in {}",
            asset_pipeline::report::REPORT_JSON,
            asset_pipeline::report::REPORT_TXT,
            out.display()
        );
    }
    if let Some(e) = table_error {
        return Err(e);
    }
    if let Some(e) = units_b2_error {
        return Err(e);
    }
    if let Some(e) = landmarks_error {
        return Err(e);
    }
    if let Some(e) = city_error {
        return Err(e);
    }
    if let Some(e) = misc_error {
        return Err(e);
    }
    if let Some(e) = fx_error {
        return Err(e);
    }
    if want_game_data && let Some(db) = &db {
        let game = game_dir.context(
            "set game-data needs the game directory: pass --game <dir> or set CIV5_ROOT",
        )?;
        let lock = gamedata_lock.map_or_else(gamedata::default_lock_path, Path::to_path_buf);
        let s = gamedata::build_package(&game, db, &lock, out, civhost_log)
            .with_context(|| format!("set game-data from {}", db.display()))?;
        println!(
            "set game-data: {} {} B, {} {} B ({} en_US rows); snapshot matches {}",
            gamedata::GAMEPLAY_DB_ID,
            s.db.gameplay_bytes,
            gamedata::LOC_EN_US_ID,
            s.db.loc_bytes,
            s.db.loc_rows,
            lock.display()
        );
        println!(
            "set game-data: {} lua file(s), {} B, {} {} B",
            s.lua.files,
            s.lua.lua_bytes,
            gamedata::INDEX_ID,
            s.lua.index_bytes
        );
        for w in &s.warnings {
            println!("warning: {w}");
        }
    }
    if repack_textures {
        repack(out, None, false, asset_pipeline::textures::repack::MIN_PSNR)?;
    }
    // The history `gc` keeps the last N manifests by.
    asset_pipeline::gc::record(out)
        .with_context(|| format!("recording the manifest of {}", out.display()))?;
    Ok(())
}

/// Set `client-data`: the cursors and the unit blend graphs of the install.
fn build_client_data_set(game: &Path, out: &Path) -> Result<()> {
    let src = open_game(game)?;
    let s = sets::client_data::build_client_data(&src, out)
        .with_context(|| format!("set {}", sets::client_data::SET_NAME))?;
    println!(
        "set {}: {} cursor(s), {} B; {} blend graph(s), {} B; {:.2}s",
        sets::client_data::SET_NAME,
        s.cursors,
        s.cursor_bytes,
        s.graphs,
        s.graph_bytes,
        s.seconds
    );
    for (what, why) in &s.failed {
        println!("warning: {what}: {why}");
    }
    Ok(())
}

/// Options of the sets that build models: the shader catalog (the argument, else
/// `CIV5_SHADER_CATALOG`, else `cache/shader-re/catalog.json` when it exists) and the clip
/// tolerance.
fn model_options(
    shader_catalog: Option<&Path>,
    clip_rot_deg: Option<f64>,
) -> sets::b2_models::Options {
    let catalog = shader_catalog.map(Path::to_path_buf).or_else(|| {
        let d = default_catalog();
        d.is_file().then_some(d)
    });
    match &catalog {
        Some(p) => println!("shader catalog: {}", p.display()),
        None => println!("shader catalog: none (unknown FGX names are listed)"),
    }
    sets::b2_models::Options {
        shader_catalog: catalog,
        clip_tolerance: asset_pipeline::anim::resample::Tolerance {
            rot_deg: clip_rot_deg.unwrap_or(asset_pipeline::anim::DEFAULT_ROT_DEG),
            ..Default::default()
        },
    }
}

/// Set `b2-units`: every unit model as `C5MD` v2, its textures, `tb:art-b2` and `units.csv`.
/// The report section is written even when models failed; then the build fails.
fn build_units_b2_set(
    game: &Path,
    db: &Path,
    out: &Path,
    shader_catalog: Option<&Path>,
    clip_rot_deg: Option<f64>,
    report: &mut Report,
) -> Result<()> {
    let src = open_game(game)?;
    let opts = model_options(shader_catalog, clip_rot_deg);
    let s = sets::b2_units::build_units(&src, db, out, &opts)
        .with_context(|| format!("set {}", sets::b2_units::SET_NAME))?;
    println!(
        "set {}: {} unit(s), {} member(s), {} model(s) built, {} failed; {:.2}s",
        sets::b2_units::SET_NAME,
        s.units,
        s.members,
        s.models,
        s.failed_models,
        s.seconds
    );
    println!(
        "meshes {} (before the merge {}), triangles {}, materials {}, textures {}, models with damage meshes {}",
        s.meshes, s.meshes_before_merge, s.triangles, s.materials, s.textures, s.damage_models
    );
    println!(
        "models {:.2} MB, textures {}, table {:.2} MB ({} vis effects); total {:.2} MB; {}",
        s.model_bytes as f64 / 1e6,
        format_bytes(&s.texture_bytes),
        s.table_bytes as f64 / 1e6,
        s.vis_effects,
        s.total_bytes() as f64 / 1e6,
        sets::b2_units::UNITS_CSV
    );
    let a = &s.anims;
    let fams: Vec<String> = a
        .by_family
        .iter()
        .map(|(k, n)| format!("{k} {n}"))
        .collect();
    println!(
        "clips: {} pack(s), {} clip(s) ({}), {} tracks, {} keys of {} frames at 30 Hz; packs {:.2} MB; max deviation {:.4} deg, {:.4} % of the size, {:.4} % scale; {} model(s) without an idle 1000 clip",
        a.packs,
        a.clips,
        fams.join(", "),
        a.tracks,
        a.keys,
        a.frames,
        a.bytes as f64 / 1e6,
        a.max_dev.rot_deg,
        a.max_dev.pos_percent,
        a.max_dev.scale_percent,
        a.no_idle.len()
    );
    let fk: Vec<String> = a
        .keys_by_family
        .iter()
        .map(|(k, n)| format!("{k} {n}"))
        .collect();
    println!(
        "clip keys by family: {}; by channel rot {} pos {} scale {}; distinct gr2 files {}",
        fk.join(", "),
        a.channel_keys[0],
        a.channel_keys[1],
        a.channel_keys[2],
        a.distinct_files.len()
    );
    let f = &a.fsm;
    let fa: Vec<String> = f
        .action_states
        .iter()
        .map(|(k, n)| format!("{k} {n}"))
        .collect();
    println!(
        "state machines: {} compiled ({} failed), {} states ({}), {} transitions, {} conditions; {:.3} MB; {} model(s) with state clips ({} rows)",
        f.machines,
        f.failed.len(),
        f.states,
        fa.join(", "),
        f.transitions,
        f.conditions,
        f.bytes as f64 / 1e6,
        f.models_with_state_clips,
        f.state_clip_rows
    );
    for st in &s.substitutes {
        println!("stub: {st}");
    }
    let kinds: Vec<String> = s
        .report
        .kinds()
        .iter()
        .map(|(k, n)| format!("{k} {n}"))
        .collect();
    println!("report entries: {}", kinds.join(", "));
    let failures = s.failures();
    report.put(sets::b2_units::SET_NAME, s.report);
    if failures > 0 {
        bail!(
            "set {}: {failures} failure(s), see {} in {}",
            sets::b2_units::SET_NAME,
            sets::b2_units::UNITS_CSV,
            out.display()
        );
    }
    Ok(())
}

/// Set `b2-landmarks`: every `ArtDefine_Landmarks` row, its model as `C5MD` v2, textures, clips,
/// the `landmark/` records of `tb:art-b2` and `landmarks.csv`.
fn build_landmarks_set(
    game: &Path,
    db: &Path,
    out: &Path,
    shader_catalog: Option<&Path>,
    clip_rot_deg: Option<f64>,
    report: &mut Report,
) -> Result<()> {
    let src = open_game(game)?;
    let opts = model_options(shader_catalog, clip_rot_deg);
    let s = sets::b2_landmarks::build_landmarks(&src, db, out, &opts)
        .with_context(|| format!("set {}", sets::b2_landmarks::SET_NAME))?;
    println!(
        "set {}: {} row(s) ({} without a model, {} failed), {} model(s) built, {} failed; {:.2}s",
        sets::b2_landmarks::SET_NAME,
        s.rows,
        s.rows_without_model,
        s.row_failures,
        s.models,
        s.failed_models,
        s.seconds
    );
    println!(
        "models merged whole {}, with parts {} ({} shared by SNAPSHOT and other handlers); meshes {} (before the merge {}), triangles {}, materials {}, textures {}, models with damage meshes {}",
        s.whole_models,
        s.part_models,
        s.shared_models,
        s.meshes,
        s.meshes_before_merge,
        s.triangles,
        s.materials,
        s.textures,
        s.damage_models
    );
    println!(
        "models {:.2} MB, textures {}, clips {:.2} MB, state machines {:.3} MB, table {:.2} MB; total {:.2} MB; {}",
        s.model_bytes as f64 / 1e6,
        format_bytes(&s.texture_bytes),
        s.anims.bytes as f64 / 1e6,
        s.anims.fsm.bytes as f64 / 1e6,
        s.table_bytes as f64 / 1e6,
        s.total_bytes() as f64 / 1e6,
        sets::b2_landmarks::LANDMARKS_CSV
    );
    println!(
        "clips: {} pack(s), {} clip(s), {} keys of {} frames at 30 Hz; max deviation {:.4} deg",
        s.anims.packs, s.anims.clips, s.anims.keys, s.anims.frames, s.anims.max_dev.rot_deg
    );
    for st in &s.substitutes {
        println!("stub: {st}");
    }
    let kinds: Vec<String> = s
        .report
        .kinds()
        .iter()
        .map(|(k, n)| format!("{k} {n}"))
        .collect();
    println!("report entries: {}", kinds.join(", "));
    let failures = s.failures();
    report.put(sets::b2_landmarks::SET_NAME, s.report);
    if failures > 0 {
        bail!(
            "set {}: {failures} failure(s), see {} in {}",
            sets::b2_landmarks::SET_NAME,
            sets::b2_landmarks::LANDMARKS_CSV,
            out.display()
        );
    }
    Ok(())
}

/// Set `b2-city`: city catalogs, walls, wonders, building art and civilizations as `tb:art-b2`
/// records, their models as `C5MD` v2, and `buildings.csv`.
fn build_city_set(
    game: &Path,
    db: &Path,
    out: &Path,
    shader_catalog: Option<&Path>,
    clip_rot_deg: Option<f64>,
    report: &mut Report,
) -> Result<()> {
    let src = open_game(game)?;
    let opts = model_options(shader_catalog, clip_rot_deg);
    let s = sets::b2_city::build_city(&src, db, out, &opts)
        .with_context(|| format!("set {}", sets::b2_city::SET_NAME))?;
    println!(
        "set {}: {} city catalog(s), {} wall record(s), {} wonder(s), {} building art record(s) ({} with a model, {} by variants, {} without art), {} civilization(s), {} damage model(s); {} model(s) built, {} failed; {:.2}s",
        sets::b2_city::SET_NAME,
        s.cities,
        s.walls,
        s.wonders,
        s.buildings,
        s.buildings_with_model,
        s.buildings_with_variants,
        s.buildings_without_art,
        s.civs,
        s.damage_models,
        s.models,
        s.failed_models,
        s.seconds
    );
    println!(
        "meshes {} (before the merge {}), triangles {}, materials {}, textures {}",
        s.meshes, s.meshes_before_merge, s.triangles, s.materials, s.textures
    );
    println!(
        "models {:.2} MB, textures {}, clips {:.2} MB, table {:.2} MB; total {:.2} MB; {}",
        s.model_bytes as f64 / 1e6,
        format_bytes(&s.texture_bytes),
        s.anims.bytes as f64 / 1e6,
        s.table_bytes as f64 / 1e6,
        s.total_bytes() as f64 / 1e6,
        sets::b2_city::BUILDINGS_CSV
    );
    for st in &s.substitutes {
        println!("stub: {st}");
    }
    for m in &s.missing {
        println!("left out: {m}");
    }
    let kinds: Vec<String> = s
        .report
        .kinds()
        .iter()
        .map(|(k, n)| format!("{k} {n}"))
        .collect();
    println!("report entries: {}", kinds.join(", "));
    let failures = s.failures();
    report.put(sets::b2_city::SET_NAME, s.report);
    if failures > 0 {
        bail!(
            "set {}: {failures} failure(s), see {} in {}",
            sets::b2_city::SET_NAME,
            sets::b2_city::BUILDINGS_CSV,
            out.display()
        );
    }
    Ok(())
}

/// The load groups of the world objects, made from the art table and the
/// manifest of `out` after the model sets.
fn build_object_groups(db: &Path, out: &Path) -> Result<()> {
    let input = asset_pipeline::world::object_groups::read_input(db)
        .with_context(|| format!("reading the objects from {}", db.display()))?;
    let Some(s) = asset_pipeline::world::object_groups::run(out, &input)
        .context("the groups of the world objects")?
    else {
        println!("groups of the world objects: the art table has no records for them");
        return Ok(());
    };
    let classes: Vec<String> = s.groups.iter().map(|(c, n)| format!("{n} {c}")).collect();
    println!(
        "groups of the world objects: {}; {} id(s) joined boot; group table {:.3} MB",
        classes.join(", "),
        s.boot_added,
        s.group_table_bytes as f64 / 1e6
    );
    for (class, (id, bytes)) in &s.largest {
        println!("  largest {class}: {id} {:.2} MB", *bytes as f64 / 1e6);
    }
    for n in &s.notes {
        println!("  note: {n}");
    }
    for e in &s.empty {
        println!("  empty group: {e}");
    }
    Ok(())
}

/// Set `b2-misc`: trees, roads and bridges, path models, ring and highlights.
fn build_fx_set(game: &Path, out: &Path, report: &mut Report) -> Result<()> {
    let src = open_game(game)?;
    let s = sets::b2_fx::build_fx(&src, out)
        .with_context(|| format!("set {}", sets::b2_fx::SET_NAME))?;
    println!(
        "set {}: {} .psb file(s), {} emitter(s), {} failed; {} texture(s) ({}), {} missing; {} visual effect(s) in {}; {:.2}s",
        sets::b2_fx::SET_NAME,
        s.files,
        s.emitters,
        s.failed.len(),
        s.textures,
        format_bytes(&s.texture_bytes),
        s.missing_textures.len(),
        s.vis_effects,
        asset_format::FX_INDEX_ID,
        s.seconds
    );
    println!(
        "effects {:.2} MB, index {:.3} MB; group {}: {:.2} MB",
        s.fx_bytes as f64 / 1e6,
        s.index_bytes as f64 / 1e6,
        sets::b2_fx::GROUP_ID,
        s.group_bytes as f64 / 1e6
    );
    for (f, why) in &s.failed {
        println!("failed: {f}: {why}");
    }
    if !s.missing_textures.is_empty() {
        println!("missing textures: {}", s.missing_textures.join(", "));
    }
    println!(
        "blobs: {} written, {} reused",
        s.stats.blobs_written, s.stats.blobs_reused
    );
    let failed = s.failed.len();
    report.put(sets::b2_fx::SET_NAME, s.report);
    if failed > 0 {
        bail!(
            "set {}: {failed} .psb file(s) did not read, see the report in {}",
            sets::b2_fx::SET_NAME,
            out.display()
        );
    }
    Ok(())
}

fn build_misc_set(
    game: &Path,
    db: &Path,
    out: &Path,
    shader_catalog: Option<&Path>,
    clip_rot_deg: Option<f64>,
    report: &mut Report,
) -> Result<()> {
    let src = open_game(game)?;
    let opts = model_options(shader_catalog, clip_rot_deg);
    let s = sets::b2_misc::build_misc(&src, db, out, &opts)
        .with_context(|| format!("set {}", sets::b2_misc::SET_NAME))?;
    println!(
        "set {}: {} tree record(s) ({} model(s)), {} route record(s) ({} model(s)), {} path model(s), {} overlay record(s); {} model(s) built, {} failed; {:.2}s",
        sets::b2_misc::SET_NAME,
        s.tree_records,
        s.tree_models,
        s.route_records,
        s.route_models,
        s.path_models,
        s.overlay_records,
        s.models,
        s.failed_models,
        s.seconds
    );
    println!(
        "meshes {}, triangles {}, textures {}",
        s.meshes, s.triangles, s.textures
    );
    println!(
        "models {:.2} MB, textures {}, clips {:.2} MB, table {:.2} MB; total {:.2} MB",
        s.model_bytes as f64 / 1e6,
        format_bytes(&s.texture_bytes),
        s.anims.bytes as f64 / 1e6,
        s.table_bytes as f64 / 1e6,
        s.total_bytes() as f64 / 1e6
    );
    for st in &s.substitutes {
        println!("stub: {st}");
    }
    for m in &s.missing {
        println!("left out: {m}");
    }
    let kinds: Vec<String> = s
        .report
        .kinds()
        .iter()
        .map(|(k, n)| format!("{k} {n}"))
        .collect();
    println!("report entries: {}", kinds.join(", "));
    let failures = s.failures();
    report.put(sets::b2_misc::SET_NAME, s.report);
    if failures > 0 {
        bail!(
            "set {}: {failures} failure(s), see the report in {}",
            sets::b2_misc::SET_NAME,
            out.display()
        );
    }
    Ok(())
}

fn build_terrain_set(game: &Path, db: &Path, out: &Path, textures: TextureOptions) -> Result<()> {
    let started = std::time::Instant::now();
    let src = open_game(game)?;
    let terrains = sets::terrain::terrain_types_from_snapshot(db)?;
    let read =
        |name: &str| -> Result<Option<Vec<u8>>, models::ModelError> { Ok(src.read_by_name(name)?) };
    let s = sets::terrain::build_terrain(&read, &terrains, out, textures)
        .with_context(|| format!("set {}", sets::terrain::SET_NAME))?;
    for f in &s.fallbacks {
        tracing::debug!("b1-terrain fallback: {f}");
    }
    let bytes: Vec<String> = s
        .bytes
        .iter()
        .map(|(f, b)| format!("{f:?} {:.1} MB", *b as f64 / 1e6))
        .collect();
    println!(
        "set {}: {} texture(s) ({}), table {} B; {} terrain + {} feature record(s) + river, {} table entries; {} style fallback(s); {:.2}s",
        sets::terrain::SET_NAME,
        s.textures,
        bytes.join(", "),
        s.table_bytes,
        s.terrain_records,
        s.feature_records,
        s.table_entries,
        s.fallbacks.len(),
        started.elapsed().as_secs_f64()
    );
    println!(
        "blobs: {} written, {} reused; cache: {} hit(s), {} miss(es); total {:.1} MB",
        s.stats.blobs_written,
        s.stats.blobs_reused,
        s.stats.cache_hits,
        s.stats.cache_misses,
        s.total_bytes() as f64 / 1e6
    );
    Ok(())
}

/// Set `b2-light`: light cubes, colour-key volumes and the fog ramp (needs no DB snapshot).
fn build_light_set(game: &Path, out: &Path) -> Result<()> {
    let started = std::time::Instant::now();
    let src = open_game(game)?;
    let read =
        |name: &str| -> Result<Option<Vec<u8>>, models::ModelError> { Ok(src.read_by_name(name)?) };
    let s = sets::b2_light::build_light(&read, out)
        .with_context(|| format!("set {}", sets::b2_light::SET_NAME))?;
    println!(
        "set {}: {} texture(s) ({}), boot group; {:.2}s",
        sets::b2_light::SET_NAME,
        s.textures,
        format_bytes(&s.bytes),
        started.elapsed().as_secs_f64()
    );
    println!(
        "boot textures {:.2} MB, other (minimap) {:.2} MB; settings {} {} B; groups {} {} B",
        s.boot_bytes as f64 / 1e6,
        s.other_bytes as f64 / 1e6,
        asset_format::WORLD_SETTINGS_ID,
        s.settings_bytes,
        asset_format::GROUP_TABLE_ID,
        s.group_table_bytes
    );
    println!(
        "blobs: {} written, {} reused; total {:.2} MB",
        s.stats.blobs_written,
        s.stats.blobs_reused,
        (s.total_bytes() + s.settings_bytes) as f64 / 1e6
    );
    for w in &s.warnings {
        println!("warning: {w}");
    }
    println!(
        "{}",
        asset_pipeline::world::settings::highlights_copy_note(&src)?
    );
    let conflicts = sets::b2_light::layer_conflicts(&src)?;
    for c in &conflicts {
        println!("layer conflict: {c}");
    }
    println!("layer conflicts: {}", conflicts.len());
    Ok(())
}

/// Set `b2-terrain`: terrain pieces, terrain and decal textures, terrain settings (needs no DB).
fn build_b2_terrain_set(game: &Path, out: &Path) -> Result<()> {
    let started = std::time::Instant::now();
    let src = open_game(game)?;
    let s = sets::b2_terrain::build_terrain(&src, out)
        .with_context(|| format!("set {}", sets::b2_terrain::SET_NAME))?;
    let mb = |b: u64| b as f64 / 1e6;
    println!(
        "set {}: {} terrain texture(s) + {} @low + {} decal texture(s) ({}); {:.2}s",
        sets::b2_terrain::SET_NAME,
        s.textures,
        s.low_textures,
        s.decal_textures,
        format_bytes(&s.texture_bytes),
        started.elapsed().as_secs_f64()
    );
    let total_pieces: usize = s.pieces.values().map(|p| p.0).sum();
    let total_bytes: u64 = s.pieces.values().map(|p| p.1).sum();
    for (style, (n, bytes)) in &s.pieces {
        println!("pieces pc:{style}: {n} piece(s), {:.2} MB", mb(*bytes));
    }
    println!(
        "pieces total: {total_pieces} piece(s), {:.2} MB; settings {} {} B; groups {} {} B",
        mb(total_bytes),
        asset_format::WORLD_SETTINGS_ID,
        s.settings_bytes,
        asset_format::GROUP_TABLE_ID,
        s.group_table_bytes
    );
    for (id, bytes) in &s.groups {
        println!("group {id}: {:.2} MB", mb(*bytes));
    }
    println!(
        "terrain ids: {} distinct code(s) in the palettes; {} the table does not explain",
        s.ids.codes,
        s.ids.unmatched.len()
    );
    for (code, pieces) in &s.ids.unmatched {
        let list: Vec<&str> = pieces.iter().map(String::as_str).take(4).collect();
        println!(
            "report: terrain id {code:#010x} is not in terraindef.csv ({} piece(s), e.g. {})",
            pieces.len(),
            list.join(", ")
        );
    }
    for n in &s.notes {
        println!("{n}");
    }
    println!(
        "blobs: {} written, {} reused",
        s.stats.blobs_written, s.stats.blobs_reused
    );
    Ok(())
}

/// Set `b2-water`: water, river, ice and wave textures, the `.lean` files, coast/wave/ice settings
/// (needs no DB).
fn build_b2_water_set(game: &Path, out: &Path) -> Result<()> {
    let started = std::time::Instant::now();
    let src = open_game(game)?;
    let s = sets::b2_water::build_water(&src, out)
        .with_context(|| format!("set {}", sets::b2_water::SET_NAME))?;
    let mb = |b: u64| b as f64 / 1e6;
    println!(
        "set {}: {} texture(s) ({}) + {} .lean file(s); {:.2}s",
        sets::b2_water::SET_NAME,
        s.textures,
        format_bytes(&s.texture_bytes),
        s.lean.len(),
        started.elapsed().as_secs_f64()
    );
    for n in &s.lean_notes {
        println!("{n}");
    }
    println!(
        "settings {} {} B; groups {} {} B; group {}: {:.2} MB",
        asset_format::WORLD_SETTINGS_ID,
        s.settings_bytes,
        asset_format::GROUP_TABLE_ID,
        s.group_table_bytes,
        sets::b2_water::GROUP,
        mb(s.group_bytes)
    );
    for n in &s.notes {
        println!("{n}");
    }
    println!(
        "blobs: {} written, {} reused",
        s.stats.blobs_written, s.stats.blobs_reused
    );
    Ok(())
}

/// `report`: the size tables of a B2 output directory.
fn size_report(out: &Path) -> Result<()> {
    use asset_pipeline::report::budget;
    let r = budget::run(out).with_context(|| format!("report of {}", out.display()))?;
    print!("{}", r.to_text());
    println!(
        "report: {} and {} in {}",
        budget::REPORT_B2_JSON,
        budget::REPORT_B2_TXT,
        out.display()
    );
    if !r.errors.is_empty() {
        bail!("{} budget error(s)", r.errors.len());
    }
    Ok(())
}

/// `verify`: closure of the manifest and the stand-in list.
fn verify_output(out: &Path, allow: Option<&Path>) -> Result<()> {
    use asset_pipeline::report::stubs::StubAllow;
    let path = allow.map_or_else(StubAllow::default_path, Path::to_path_buf);
    let allow = StubAllow::load(&path)?;
    let r = asset_pipeline::report::verify::verify(out, &allow)
        .with_context(|| format!("verify {}", out.display()))?;
    println!(
        "verify {}: {} id(s), {} blob(s), {} read with their own type, {} group(s); stubs allowed by {}: {}",
        out.display(),
        r.ids,
        r.blobs,
        r.decoded,
        r.groups,
        path.display(),
        allow.entries.len()
    );
    for p in &r.problems {
        println!("problem: {p}");
    }
    if !r.is_ok() {
        bail!("verify found {} problem(s)", r.problems.len());
    }
    println!("verify: ok, 0 stubs");
    Ok(())
}

fn format_bytes(bytes: &BTreeMap<asset_format::VariantFormat, u64>) -> String {
    bytes
        .iter()
        .map(|(f, b)| format!("{f:?} {:.1} MB", *b as f64 / 1e6))
        .collect::<Vec<_>>()
        .join(", ")
}

fn build_trees_set(
    game: &Path,
    out: &Path,
    textures: TextureOptions,
    report: &mut Report,
) -> Result<()> {
    let started = std::time::Instant::now();
    let src = open_game(game)?;
    let read =
        |name: &str| -> Result<Option<Vec<u8>>, models::ModelError> { Ok(src.read_by_name(name)?) };
    let s = sets::trees::build_trees(&read, out, textures)
        .with_context(|| format!("set {}", sets::trees::SET_NAME))?;
    println!(
        "set {}: {} model(s) {:.1} KB, {} texture(s) ({}), table {} B; {} feature + {} formation record(s), {} table entries; {:.2}s",
        sets::trees::SET_NAME,
        s.models,
        s.model_bytes as f64 / 1024.0,
        s.textures,
        format_bytes(&s.bytes),
        s.table_bytes,
        s.feature_records,
        s.formation_records,
        s.table_entries,
        started.elapsed().as_secs_f64()
    );
    for f in &s.fallbacks {
        println!("report: tree-fallback {f}");
    }
    println!(
        "forests.xml: {}",
        s.forest_params
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    report.put(
        sets::trees::SET_NAME,
        SetReport {
            entries: s
                .fallbacks
                .iter()
                .map(|f| asset_pipeline::report::ReportEntry::new("tree-fallback", f, ""))
                .collect(),
            counts: BTreeMap::from([
                ("models".to_owned(), s.models as u64),
                ("textures".to_owned(), s.textures as u64),
                ("feature records".to_owned(), s.feature_records as u64),
                ("formation records".to_owned(), s.formation_records as u64),
            ]),
            bytes: set_bytes(&s.bytes, s.model_bytes, s.table_bytes),
        },
    );
    println!(
        "blobs: {} written, {} reused; cache: {} hit(s), {} miss(es); total {:.1} MB",
        s.stats.blobs_written,
        s.stats.blobs_reused,
        s.stats.cache_hits,
        s.stats.cache_misses,
        s.total_bytes() as f64 / 1e6
    );
    Ok(())
}

fn set_bytes(
    textures: &BTreeMap<asset_format::VariantFormat, u64>,
    models: u64,
    table: u64,
) -> BTreeMap<String, u64> {
    textures
        .iter()
        .map(|(f, b)| (format!("textures {f:?}"), *b))
        .chain([("models".to_owned(), models), ("table".to_owned(), table)])
        .collect()
}

fn build_cities_set(
    game: &Path,
    out: &Path,
    textures: TextureOptions,
    report: &mut Report,
) -> Result<()> {
    let started = std::time::Instant::now();
    let src = open_game(game)?;
    let s = sets::cities::build_cities(&src, out, textures)
        .with_context(|| format!("set {}", sets::cities::SET_NAME))?;
    println!(
        "set {}: {} model(s) {:.1} KB, {} texture(s) ({}), table {} B; {} city record(s), {} table entries; {} report line(s); {:.2}s",
        sets::cities::SET_NAME,
        s.models,
        s.model_bytes as f64 / 1024.0,
        s.textures,
        format_bytes(&s.bytes),
        s.table_bytes,
        s.city_records,
        s.table_entries,
        s.report.len(),
        started.elapsed().as_secs_f64()
    );
    for r in &s.report {
        println!("report: {r}");
    }
    report.put(
        sets::cities::SET_NAME,
        SetReport {
            entries: s
                .report
                .iter()
                .map(|r| asset_pipeline::report::ReportEntry::new("city-note", r, ""))
                .collect(),
            counts: BTreeMap::from([
                ("models".to_owned(), s.models as u64),
                ("textures".to_owned(), s.textures as u64),
                ("city records".to_owned(), s.city_records as u64),
            ]),
            bytes: set_bytes(&s.bytes, s.model_bytes, s.table_bytes),
        },
    );
    println!(
        "blobs: {} written, {} reused; cache: {} hit(s), {} miss(es); total {:.1} MB",
        s.stats.blobs_written,
        s.stats.blobs_reused,
        s.stats.cache_hits,
        s.stats.cache_misses,
        s.total_bytes() as f64 / 1e6
    );
    Ok(())
}

fn build_sv_set(
    game: &Path,
    db: &Path,
    out: &Path,
    textures: TextureOptions,
    report: &mut Report,
) -> Result<()> {
    let started = std::time::Instant::now();
    let src = open_game(game)?;
    let input = sets::sv::load_input(db).with_context(|| format!("set {}", sets::sv::SET_NAME))?;
    let read =
        |name: &str| -> Result<Option<Vec<u8>>, models::ModelError> { Ok(src.read_by_name(name)?) };
    let s = sets::sv::build_sv(&read, &input, out, textures)
        .with_context(|| format!("set {}", sets::sv::SET_NAME))?;
    let bytes: Vec<String> = s
        .bytes
        .iter()
        .map(|(f, b)| format!("{f:?} {:.1} MB", *b as f64 / 1e6))
        .collect();
    println!(
        "set {}: {} icon(s) ({}), table {} B; {} sv + {} player_color record(s), {} table entries; {} report entr(ies); {:.2}s",
        sets::sv::SET_NAME,
        s.textures,
        bytes.join(", "),
        s.table_bytes,
        s.sv_records,
        s.color_records,
        s.table_entries,
        s.report.len(),
        started.elapsed().as_secs_f64()
    );
    for e in &s.report {
        println!("report: {e}");
    }
    report.put(
        sets::sv::SET_NAME,
        SetReport {
            entries: s
                .report
                .iter()
                .map(|e| asset_pipeline::report::ReportEntry::new(e.kind, &e.subject, &e.reason))
                .collect(),
            counts: BTreeMap::from([
                ("icons".to_owned(), s.textures as u64),
                ("sv records".to_owned(), s.sv_records as u64),
                ("player_color records".to_owned(), s.color_records as u64),
            ]),
            bytes: s
                .bytes
                .iter()
                .map(|(f, b)| (format!("textures {f:?}"), *b))
                .chain([("table".to_owned(), s.table_bytes)])
                .collect(),
        },
    );
    println!(
        "blobs: {} written, {} reused; cache: {} hit(s), {} miss(es); total {:.1} MB",
        s.stats.blobs_written,
        s.stats.blobs_reused,
        s.stats.cache_hits,
        s.stats.cache_misses,
        s.total_bytes() as f64 / 1e6
    );
    Ok(())
}

/// Set `b1-table`: validates `table:art-b1` against the manifest and the snapshot.
fn check_table(db: &Path, out: &Path) -> Result<SetReport, Box<(SetReport, anyhow::Error)>> {
    let name = sets::table::SET_NAME;
    let fail =
        |e: anyhow::Error| Box::new((SetReport::default(), e.context(format!("set {name}"))));
    let expected = sets::table::Expected {
        terrains: sets::terrain::terrain_types_from_snapshot(db).map_err(|e| fail(e.into()))?,
        units: sets::units::load_input(db)
            .map_err(|e| fail(e.into()))?
            .units
            .into_iter()
            .map(|u| u.unit_type)
            .collect(),
    };
    let store = asset_pipeline::store::BlobStore::open(out).map_err(|e| fail(e.into()))?;
    let result = sets::table::check(&store, &expected);
    let report = match &result {
        Ok(r) => r,
        Err(b) => &b.0,
    };
    let counts: Vec<String> = report
        .counts
        .iter()
        .map(|(k, v)| format!("{k} {v}"))
        .collect();
    println!(
        "set {name}: table {} B; {}",
        report.bytes.get("table").copied().unwrap_or(0),
        counts.join(", ")
    );
    result.map_err(|b| {
        let (r, e) = *b;
        Box::new((r, anyhow::Error::from(e).context(format!("set {name}"))))
    })
}

fn build_units_set(
    game: &Path,
    db: &Path,
    out: &Path,
    textures: TextureOptions,
    report: &mut Report,
) -> Result<()> {
    let src = open_game(game)?;
    let input =
        sets::units::load_input(db).with_context(|| format!("set {}", sets::units::SET_NAME))?;
    let s = sets::units::build_units(&src, &input, out, textures)
        .with_context(|| format!("set {}", sets::units::SET_NAME))?;
    let bytes: Vec<String> = s
        .texture_bytes
        .iter()
        .map(|(f, b)| format!("{f:?} {:.1} MB", *b as f64 / 1e6))
        .collect();
    println!(
        "set {}: {} unit(s) ({} variant art(s)), {} model(s) ({:.1} MB, {} meshes, {} triangles, {} dropped mesh(es)), {} texture(s) ({}), table {} B ({} entries); {:.2}s",
        sets::units::SET_NAME,
        s.units,
        s.variant_arts,
        s.models,
        s.model_bytes as f64 / 1e6,
        s.meshes,
        s.triangles,
        s.dropped_meshes,
        s.textures,
        bytes.join(", "),
        s.table_bytes,
        s.table_entries,
        s.seconds
    );
    println!(
        "set {}: {} placeholder unit(s), {} placeholder variant(s), {} failed model(s); {} report entr(ies); total {:.1} MB",
        sets::units::SET_NAME,
        s.placeholder_units,
        s.placeholder_variants,
        s.failed_models,
        s.report.entries.len(),
        s.total_bytes() as f64 / 1e6
    );
    for e in s.report.entries.iter().filter(|e| {
        matches!(
            e.kind.as_str(),
            "placeholder" | "placeholder-variant" | "model-failed"
        )
    }) {
        println!("report: {} {}: {}", e.kind, e.subject, e.reason);
    }
    println!(
        "blobs: {} written, {} reused; cache: {} hit(s), {} miss(es)",
        s.stats.blobs_written, s.stats.blobs_reused, s.stats.cache_hits, s.stats.cache_misses
    );
    report.put(sets::units::SET_NAME, s.report);
    Ok(())
}

/// `--db` or CIV5_DB_SNAPSHOT; must be a directory.
fn snapshot_dir(db: Option<PathBuf>) -> Result<Option<PathBuf>> {
    let db = db.or_else(|| std::env::var_os(gamedata::SNAPSHOT_ENV).map(PathBuf::from));
    if let Some(db) = &db
        && !db.is_dir()
    {
        bail!("--db {} is not a directory", db.display());
    }
    Ok(db)
}

#[allow(clippy::too_many_arguments)]
fn export_gamedata(
    root: PathBuf,
    db: Option<PathBuf>,
    out: &Path,
    gamedata_lock: Option<&Path>,
    civhost_log: Option<&Path>,
    lua_overrides: &[String],
    no_lua_override: bool,
    lekmod_repo: Option<&Path>,
) -> Result<()> {
    let db = snapshot_dir(db)?.with_context(|| {
        format!(
            "the DB snapshot directory is not set: pass --db <dir> or set {}",
            gamedata::SNAPSHOT_ENV
        )
    })?;
    let lock = gamedata_lock.map_or_else(gamedata::default_lock_path, Path::to_path_buf);
    let mut overrides = Vec::new();
    for arg in lua_overrides {
        overrides.push(gamedata::lua_override::parse_arg(arg)?);
    }
    // FIX-0004 is on unless switched off, or replaced by an explicit file of the same name.
    let fix_0004_explicit = overrides.iter().any(|o| {
        o.file
            .eq_ignore_ascii_case(gamedata::lua_override::FIX_0004_FILE)
    });
    let fix_0004 = !no_lua_override;
    if fix_0004 && !fix_0004_explicit {
        overrides.insert(0, gamedata::lua_override::fix_0004(lekmod_repo)?);
    }
    let opts = gamedata::ServerOptions {
        civhost_log,
        overrides,
        fix_0004,
    };
    let (s, warnings) = gamedata::export_server(&root, &db, &lock, out, &opts)
        .with_context(|| format!("export-gamedata to {}", out.display()))?;
    for w in &warnings {
        println!("warning: {w}");
    }
    for a in &s.applied {
        println!(
            "lua override: {} <- {} ({} bytes{})",
            a.path,
            a.source.display(),
            a.size,
            if a.is_noop() {
                "; identical to the installed file"
            } else {
                ""
            }
        );
    }
    if s.applied.is_empty() {
        println!("lua override: none (the installed Lua is exported)");
    }
    let e = &s.export;
    println!(
        "exported {} file(s), {:.1} MB to {}: {} map script(s), {} lua, {} .Civ5Map ({:.1} MB), {}, {} ({} en_US rows), vfs-index.json, {}, {} ({} files); snapshot matches {}",
        e.files,
        e.total_bytes as f64 / 1e6,
        e.out.display(),
        s.map_scripts,
        e.lua_files,
        s.maps,
        s.map_bytes as f64 / 1e6,
        gamedata::EXPORT_GAMEPLAY_DB,
        gamedata::EXPORT_LOC_DB,
        e.loc_rows,
        gamedata::LOCK_FILE,
        gamedata::MANIFEST_FILE,
        s.sealed,
        lock.display()
    );
    Ok(())
}

fn gc_output(out: &Path, keep: usize, dry_run: bool) -> Result<()> {
    let s = asset_pipeline::gc::gc(out, keep, dry_run)
        .with_context(|| format!("gc of {}", out.display()))?;
    let verb = if s.dry_run { "would remove" } else { "removed" };
    println!(
        "gc {}: kept {} manifest(s) (dropped {} from the history), {} blob(s) kept, {:.1} MB; {verb} {} blob(s), {:.1} MB",
        out.display(),
        s.kept_manifests.len(),
        s.dropped_manifests,
        s.blobs_kept,
        s.bytes_kept as f64 / 1e6,
        s.removed.len(),
        s.removed_bytes() as f64 / 1e6
    );
    Ok(())
}

fn seal_gamedata(dir: &Path, check: bool) -> Result<()> {
    if check {
        let n = gamedata::check_seal(dir)?;
        println!("{} matches its {n} file(s)", gamedata::MANIFEST_FILE);
    } else {
        let n = gamedata::seal(dir)?;
        println!("{} written: {n} file(s)", gamedata::MANIFEST_FILE);
    }
    Ok(())
}

fn build_unit_sets(root: PathBuf, out: &Path, defs: &[&models::set::SetDef]) -> Result<()> {
    let game = open_game(&root)?;
    let s = models::set::build_sets(&game, defs, out)?;
    let names: Vec<_> = defs.iter().map(|d| d.name).collect();
    let def = names.join("+");
    for w in &s.warnings {
        println!("warning: {w}");
    }
    let st = s.stats;
    println!(
        "set {}: {} member(s), {} texture(s), {} animation(s) in {:.2}s",
        def, s.members, s.textures, s.animations, s.seconds
    );
    println!(
        "blobs: {} written ({:.1} KB), {} reused; cache: {} hit(s), {} miss(es); manifest writes: {}",
        st.blobs_written,
        st.bytes_written as f64 / 1024.0,
        st.blobs_reused,
        st.cache_hits,
        st.cache_misses,
        st.manifest_writes
    );
    println!(
        "manifest {} -> {}",
        s.manifest,
        out.join("manifest").display()
    );
    Ok(())
}

/// Where `shader-re` leaves the stable catalog (`<parent of the repo>/cache/shader-re/catalog.json`),
/// relative to this crate.
fn default_catalog() -> PathBuf {
    if let Some(p) = std::env::var_os("CIV5_SHADER_CATALOG") {
        return PathBuf::from(p);
    }
    // The nearest `cache/shader-re/catalog.json` above this crate (a cache folder next to the repository).
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    here.ancestors()
        .map(|a| a.join("cache/shader-re/catalog.json"))
        .find(|p| p.is_file())
        .unwrap_or_else(|| here.join("../../../cache/shader-re/catalog.json"))
}

fn fgx_check(root: PathBuf, catalog: Option<&Path>) -> Result<()> {
    let catalog = catalog.map(Path::to_path_buf).or_else(|| {
        let d = default_catalog();
        d.is_file().then_some(d)
    });
    let dict = match &catalog {
        Some(p) => {
            println!("shader catalog: {}", p.display());
            models::load_shader_catalog(p)?
        }
        None => {
            println!("shader catalog: none (unknown names are listed, not errors)");
            models::NameDict::builtin()
        }
    };
    let game = open_game(&root)?;
    let report = models::check_unit_models(&game, &dict)?;
    for l in report.lines() {
        println!("{l}");
    }
    let failures = report.failures(catalog.is_some());
    if !failures.is_empty() {
        bail!("{} FGX problem(s) in unit models", failures.len());
    }
    Ok(())
}

fn gltf(root: PathBuf, model: &str, out: &Path) -> Result<()> {
    let game = open_game(&root)?;
    // `city:<CULTURE>/<Era>` (table key, e.g. `city:EUROPEAN/Ancient`), `forest:<style>` and
    // `jungle:<style>` (e.g. `forest:europe`) export the B1 city and tree models.
    let bytes = if let Some(key) = model.strip_prefix("city:") {
        let (culture, era) = key
            .split_once('/')
            .context("expected city:<CULTURE>/<Era>")?;
        sets::cities::export_city_glb(&game, culture, era)?
    } else if let Some((kind @ ("forest" | "jungle"), style)) = model.split_once(':') {
        sets::trees::export_tree_glb(&game, kind, style)?
    } else {
        models::set::export_member_glb(&game, model)?
    };
    std::fs::write(out, &bytes).with_context(|| format!("writing {}", out.display()))?;
    println!(
        "wrote {} ({:.1} KB)",
        out.display(),
        bytes.len() as f64 / 1024.0
    );
    Ok(())
}

/// `*` matches any run, `?` one character; both sides lower-case.
fn glob_match(pat: &[u8], text: &[u8]) -> bool {
    match pat.split_first() {
        None => text.is_empty(),
        Some((b'*', rest)) => (0..=text.len()).any(|i| glob_match(rest, &text[i..])),
        Some((b'?', rest)) => !text.is_empty() && glob_match(rest, &text[1..]),
        Some((c, rest)) => text.first() == Some(c) && glob_match(rest, &text[1..]),
    }
}

fn extract(root: PathBuf, name: &str, out: Option<&Path>) -> Result<()> {
    let game = open_game(&root)?;
    let pat = name.to_ascii_lowercase();
    let mut n = 0usize;
    let mut bytes = 0u64;
    for entry in game.iter_all() {
        if !glob_match(pat.as_bytes(), entry.name.as_bytes()) {
            continue;
        }
        n += 1;
        bytes += entry.size();
        println!("{:?} {} {} B", entry.layer, entry.name, entry.size());
        if let Some(dir) = out {
            std::fs::create_dir_all(dir)?;
            let data = game.read(entry)?;
            let dest = dir.join(&entry.name);
            std::fs::write(&dest, data).with_context(|| format!("writing {}", dest.display()))?;
        }
    }
    if n == 0 {
        bail!("no file matches {name:?}");
    }
    println!("{n} file(s), {:.2} MB", bytes as f64 / 1e6);
    Ok(())
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    // A DB snapshot that is not the reference one goes on with a warning, unless a lock is named on purpose.
    let lock_named = match &cli.command {
        Command::Build { gamedata_lock, .. } | Command::ExportGamedata { gamedata_lock, .. } => {
            gamedata_lock.is_some()
        }
        _ => false,
    };
    let strict = std::env::var("CIV5_GAMEDATA_STRICT").is_ok_and(|v| v == "1");
    gamedata::allow_unpinned_snapshot(!lock_named && !strict);
    match &cli.command {
        Command::LockGamedata { db, out } => {
            let snapshot = snapshot_dir(db.clone())?
                .context("pass --db <snapshot dir> or set CIV5_DB_SNAPSHOT")?;
            let text = gamedata::lock_text_for(&snapshot)?;
            match out {
                Some(p) => {
                    std::fs::write(p, &text).with_context(|| format!("writing {}", p.display()))?;
                    println!("written: {}", p.display());
                }
                None => print!("{text}"),
            }
            Ok(())
        }
        Command::Scan => scan(game_root(&cli)?),
        Command::Probe {
            kind: ProbeKind::Dds,
        } => probe_dds(game_root(&cli)?),
        Command::Probe {
            kind: ProbeKind::Gr2,
        } => {
            let root = game_root(&cli)?;
            let src =
                GameSource::open(&root).with_context(|| format!("scanning {}", root.display()))?;
            probe_gr2::probe_gr2(&src)
        }
        Command::Build {
            out,
            sets,
            db,
            gamedata_lock,
            civhost_log,
            max_texture,
            no_astc,
            no_texture_variants,
            shader_catalog,
            clip_rot_deg,
            repack_textures,
            ui,
        } => {
            // `ui` and `web-lobby` read the UI file system (the `--ui-*` options) and are built
            // outside `build`; `web-lobby` last, so its manifest entries go on top of the others.
            let (own, rest): (Vec<String>, Vec<String>) = sets.iter().cloned().partition(|s| {
                s.eq_ignore_ascii_case(asset_pipeline::ui::SET_NAME)
                    || s.eq_ignore_ascii_case(sets::web_lobby::SET_NAME)
            });
            let want_ui = own
                .iter()
                .any(|s| s.eq_ignore_ascii_case(asset_pipeline::ui::SET_NAME));
            let want_web_lobby = own
                .iter()
                .any(|s| s.eq_ignore_ascii_case(sets::web_lobby::SET_NAME));
            if want_ui {
                let mut opts = ui.options()?;
                opts.loc_db = snapshot_dir(db.clone())?.map(|d| d.join(gamedata::LOC_DB_FILE));
                build_ui_set(&game_root(&cli)?, out, &opts)?;
            }
            if own.is_empty() || !rest.is_empty() {
                build(
                    game_root(&cli),
                    out,
                    if own.is_empty() { sets } else { &rest },
                    db.clone(),
                    gamedata_lock.as_deref(),
                    civhost_log.as_deref(),
                    TextureOptions {
                        max_side: (*max_texture > 0).then_some(*max_texture),
                        astc: !*no_astc,
                    },
                    shader_catalog.as_deref(),
                    *clip_rot_deg,
                    *repack_textures,
                )?;
            }
            if want_web_lobby {
                let Some(db) = snapshot_dir(db.clone())? else {
                    bail!(
                        "set web-lobby needs the DB snapshot directory: pass --db <dir> or set {}",
                        gamedata::SNAPSHOT_ENV
                    );
                };
                build_web_lobby_set(&game_root(&cli)?, &db, out, &ui.options()?)?;
            }
            // Last, so every set's textures are in the manifest.
            if !*no_texture_variants {
                texture_variants(out, false)?;
            }
            Ok(())
        }
        Command::Variants { out, dry_run } => texture_variants(out, *dry_run),
        Command::UiVfs { ui, json } => {
            ui_vfs_command(&game_root(&cli)?, &ui.options()?, json.as_deref())
        }
        Command::UiOverride { ui } => ui_override_command(&game_root(&cli)?, &ui.options()?),
        Command::UiLocDiff {
            ui,
            db,
            xml_only,
            out,
        } => ui_loc_diff_command(
            &game_root(&cli)?,
            &ui.options()?,
            db.clone(),
            *xml_only,
            out.as_deref(),
        ),
        Command::Report { out } => size_report(out),
        Command::Repack {
            out,
            exceptions,
            dry_run,
            min_psnr,
        } => repack(out, exceptions.as_deref(), *dry_run, *min_psnr),
        Command::Verify { out, stubs_allow } => verify_output(out, stubs_allow.as_deref()),
        Command::ExportGamedata {
            out,
            lua_overrides,
            no_lua_override,
            lekmod_repo,
            db,
            gamedata_lock,
            civhost_log,
        } => export_gamedata(
            game_root(&cli)?,
            db.clone(),
            out,
            gamedata_lock.as_deref(),
            civhost_log.as_deref(),
            lua_overrides,
            *no_lua_override,
            lekmod_repo.as_deref(),
        ),
        Command::SealGamedata { dir, check } => seal_gamedata(dir, *check),
        Command::Precompress {
            dir,
            brotli_bin,
            level_zstd,
            only_hash_names,
        } => precompress_dir(dir, brotli_bin.as_deref(), *level_zstd, *only_hash_names),
        Command::Gc { out, keep, dry_run } => gc_output(out, *keep, *dry_run),
        Command::TexAudit { out, top } => {
            use asset_pipeline::report::texaudit;
            let rows =
                texaudit::run(out).with_context(|| format!("tex-audit {}", out.display()))?;
            print!("{}", texaudit::to_text(&rows, *top));
            Ok(())
        }
        Command::FgxCheck { shader_catalog } => {
            fgx_check(game_root(&cli)?, shader_catalog.as_deref())
        }
        Command::Gltf { model, out } => gltf(game_root(&cli)?, model, out),
        Command::Extract { name, out } => extract(game_root(&cli)?, name, out.as_deref()),
    }
}

fn precompress_dir(
    dir: &Path,
    brotli_bin: Option<&Path>,
    level_zstd: i32,
    only_hash_names: bool,
) -> Result<()> {
    use asset_pipeline::precompress;
    let s = precompress::run(&precompress::Options {
        dir: dir.to_owned(),
        brotli_bin: brotli_bin.map(Path::to_path_buf),
        zstd_level: level_zstd,
        only_hash_names,
    })
    .with_context(|| format!("precompress {}", dir.display()))?;
    if s.brotli_missing {
        eprintln!(
            "warning: brotli binary not found (PATH or --brotli-bin): only zstd sidecars made"
        );
    }
    print!("{}", s.to_text());
    Ok(())
}

fn repack(out: &Path, exceptions: Option<&Path>, dry_run: bool, min_psnr: f64) -> Result<()> {
    use asset_pipeline::textures::repack::{Exceptions, Options, repack_pack};
    let exceptions = match exceptions {
        Some(p) => Exceptions::load(p)?,
        None => Exceptions::builtin(),
    };
    let opts = Options {
        min_psnr,
        dry_run,
        exceptions,
    };
    let report = repack_pack(out, &opts).with_context(|| format!("repack {}", out.display()))?;
    print!("{}", report.to_text());
    Ok(())
}

/// The texture variants pass; prints the head of its report (the whole list is in
/// `report-textures.txt`).
fn texture_variants(out: &Path, dry_run: bool) -> Result<()> {
    use asset_pipeline::textures::variants::{Options, REPORT_TXT, add_variants};
    let started = std::time::Instant::now();
    let report = add_variants(out, &Options { dry_run })
        .with_context(|| format!("texture variants of {}", out.display()))?;
    let text = report.to_text();
    let head: String = text
        .split("\n== exceptions")
        .next()
        .unwrap_or_default()
        .to_owned();
    print!("{head}");
    println!(
        "{} texture(s) kept uncompressed on a device (rules in {}), {} failed encode(s); {:.0} s",
        report.kept.len(),
        out.join(REPORT_TXT).display(),
        report.failed.len(),
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

/// Fails only on panics and read errors; unsupported formats are expected and just listed.
fn probe_dds(root: PathBuf) -> Result<()> {
    let src = GameSource::open(&root).with_context(|| format!("scanning {}", root.display()))?;
    let started = std::time::Instant::now();
    let r = asset_pipeline::textures::probe_dds(&src);
    println!(
        "dds: {} files, {} decoded, {} unsupported, {} other failures ({:.1}s)",
        r.total,
        r.decoded,
        r.unsupported(),
        r.total - r.decoded - r.unsupported() - r.io_errors.len() as u64 - r.panics.len() as u64,
        started.elapsed().as_secs_f64()
    );
    println!("source formats:");
    let mut formats: Vec<_> = r.formats.iter().collect();
    formats.sort_by(|a, b| b.1.count.cmp(&a.1.count).then(a.0.cmp(b.0)));
    for (name, s) in formats {
        println!(
            "  {name:<14} {:>6} files {:>9.1} MB, {} decoded, {} with full mip chain",
            s.count,
            s.bytes as f64 / 1e6,
            s.decoded,
            s.full_chain
        );
        let mut sizes: Vec<_> = s.sizes.iter().collect();
        sizes.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        let shown: Vec<_> = sizes
            .iter()
            .take(8)
            .map(|((w, h), n)| format!("{w}x{h} x{n}"))
            .collect();
        let more = sizes.len().saturating_sub(8);
        let more = if more > 0 {
            format!(", +{more} sizes")
        } else {
            String::new()
        };
        println!("      sizes: {}{more}", shown.join(", "));
    }
    if !r.failures.is_empty() {
        println!("failures by reason:");
        for (reason, (n, examples)) in &r.failures {
            println!("  {n:>6}  {reason}  e.g. {}", examples.join(", "));
        }
    }
    for e in &r.io_errors {
        println!("io error: {e}");
    }
    for p in &r.panics {
        println!("PANIC decoding {p}");
    }
    if !r.io_errors.is_empty() || !r.panics.is_empty() {
        bail!(
            "{} read error(s), {} panic(s)",
            r.io_errors.len(),
            r.panics.len()
        );
    }
    Ok(())
}
