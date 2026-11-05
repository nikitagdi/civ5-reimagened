//! `golden world <scene>`: the scene on our side. Starts the game worker with the real core
//! (`civ5-server worker`, Linux, through WSL2 on Windows) with `basis_nw = All`, sends `Init { New(setup) }`
//! built from the `[setup]` of the scene, takes `Basis` and the first `Full` of seat 0 and writes
//! `mapbasis.wv3`, `world.wv3`, `plots.json` and `digest.json`; then the map-match check (G-D4) against the
//! `FowMask` of the capture: `map-mismatch.json`.
//!
//! With a save of the original (`[setup] save` of the scene or `--save`), the worker continues that game instead of
//! making one from the seeds (the seeds did not give the map of the original: SIMP-035 history): the `.Civ5Save` is
//! split by `civsim.saveio extract` (Lekmod `headless/tools`, Python stdlib), its pregame block and inflated body go
//! into a GCS1 import container (Lekmod `rt_save.h`), that into a C5SV file, and the worker starts with
//! `Start::Load` of it.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use game_host::real::WORLD_BUILDER_MAP_PREFIX;
use protocol::{
    AutoChoices, BasisNw, CivPick, Codec, CoreKind, GameSettings, GameSetup, IPC_VERSION, Keyframe,
    MapBasis, PlotFlags, Postcard, SlotControl, SlotSetup, Start, TimerMode, ToWorker, TurnMode,
    ViewUpdate, WorkerConfig, WorkerFrame, WorkerInit, WorkerMsg, WorldView,
};
use serde::Serialize;
use wire::ipc::{FrameReader, write_frame};

use crate::scene::Scene;
use crate::wv3;

/// The longest wait for the worker (a real map is made in seconds to a minute).
const WAIT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone)]
pub struct Options {
    pub scene: String,
    pub cache: PathBuf,
    pub scenes_dir: PathBuf,
    /// The Linux `civ5-server` (a Windows path when run on Windows: it goes through WSL).
    pub server: PathBuf,
    /// The game data of the export (`assets-out/gamedata-b2`).
    pub gamedata: PathBuf,
    pub distro: String,
    pub out: PathBuf,
    /// A `.Civ5Save` of the original to continue (overrides `[setup] save` of the scene).
    pub save: Option<PathBuf>,
    /// The folder with the `civsim` package (Lekmod `headless/tools`).
    pub civsim: PathBuf,
}

/// `E:\a\b` -> `/mnt/e/a/b`; other paths stay as they are.
pub fn to_wsl_path(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    let b = s.as_bytes();
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        format!("/mnt/{}{}", (b[0] as char).to_ascii_lowercase(), &s[2..])
    } else {
        s
    }
}

/// The `Init` setup of a scene: explicit slots (human first, then the AIs), the seeds of the scene, no
/// city-states unless the scene lists some.
pub fn scene_setup(scene: &Scene) -> Result<GameSetup> {
    let s = &scene.setup;
    if let Some(map) = &s.world_builder_map {
        // A map of the game's VFS: the map decides the world size and the players (seat 0 is ours),
        // the speed, era and handicap are the core's defaults. See `WORLD_BUILDER_MAP_PREFIX`.
        let settings = GameSettings {
            name: format!("golden {}", scene.scene.id),
            world_size: String::new(),
            map_script: format!("{WORLD_BUILDER_MAP_PREFIX}{map}"),
            map_options: vec![],
            speed: s
                .speed
                .clone()
                .unwrap_or_else(|| "GAMESPEED_STANDARD".into()),
            start_era: s.era.clone().unwrap_or_else(|| "ERA_ANCIENT".into()),
            handicap: s
                .handicap
                .clone()
                .unwrap_or_else(|| "HANDICAP_PRINCE".into()),
            minor_civs: 0,
            turn_mode: TurnMode::Hybrid,
            timer: TimerMode::Off,
            game_options: vec![],
            seed: s.sync_seed,
            map_seed: s.map_seed,
            auto_choices: AutoChoices::default(),
            diplo_auto_decline: true,
        };
        // The scenario of the map has two players (`LateGameTest`: the core refuses a game with fewer seats).
        return Ok(GameSetup {
            settings,
            slots: vec![
                SlotSetup {
                    player: 0,
                    civ: CivPick::Random,
                    control: SlotControl::Human {
                        nick: "golden".into(),
                    },
                },
                SlotSetup {
                    player: 1,
                    civ: CivPick::Random,
                    control: SlotControl::Ai,
                },
            ],
        });
    }
    let need = |v: &Option<String>, what: &str| {
        v.clone()
            .with_context(|| format!("scene {}: [setup] has no {what}", scene.scene.id))
    };
    let mut slots = vec![SlotSetup {
        player: 0,
        civ: CivPick::Civ(need(&s.human, "human")?),
        control: SlotControl::Human {
            nick: "golden".into(),
        },
    }];
    for (i, civ) in s.ai.iter().enumerate() {
        slots.push(SlotSetup {
            player: (i + 1) as i8,
            civ: CivPick::Civ(civ.clone()),
            control: SlotControl::Ai,
        });
    }
    Ok(GameSetup {
        settings: GameSettings {
            name: format!("golden {}", scene.scene.id),
            world_size: need(&s.world_size, "world_size")?,
            map_script: need(&s.map_script, "map_script")?,
            map_options: vec![],
            speed: need(&s.speed, "speed")?,
            start_era: need(&s.era, "era")?,
            handicap: need(&s.handicap, "handicap")?,
            minor_civs: s.city_states.len() as u8,
            turn_mode: TurnMode::Hybrid,
            timer: TimerMode::Off,
            game_options: vec![],
            seed: s.sync_seed,
            map_seed: s.map_seed,
            auto_choices: AutoChoices::default(),
            diplo_auto_decline: true,
        },
        slots,
    })
}

/// What the worker gave for seat 0.
pub struct Taken {
    pub width: u16,
    pub height: u16,
    pub wrap_x: bool,
    pub build_id: String,
    pub basis: MapBasis,
    pub keyframe: Keyframe,
    /// `gc_state_digest(-1)` if the worker reported one.
    pub state_digest: Option<u64>,
    /// The postcard bytes of the worker's `Catalog` (names behind the type numbers of the view); the client needs
    /// it to draw the scene (`catalog.wv3`).
    pub catalog: Option<Vec<u8>>,
}

struct Proc {
    child: Child,
    frames: Receiver<WorkerFrame>,
}

impl Proc {
    fn start(o: &Options) -> Result<Self> {
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("wsl");
            c.args(["-d", &o.distro, "--", &to_wsl_path(&o.server)]);
            c
        } else {
            Command::new(&o.server)
        };
        let mut child = cmd
            .arg("worker")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("cannot start the worker {}", o.server.display()))?;
        let stdout = child.stdout.take().context("worker stdout")?;
        let (tx, frames) = mpsc::channel();
        std::thread::spawn(move || {
            let mut r = FrameReader::new(stdout);
            while let Ok(Some(body)) = r.next() {
                let Ok(frame) = Postcard.decode::<WorkerFrame>(&body) else {
                    return;
                };
                if tx.send(frame).is_err() {
                    return;
                }
            }
        });
        Ok(Proc { child, frames })
    }

    fn send(&mut self, msg: &ToWorker) -> Result<()> {
        let body = Postcard.encode(msg).context("encode a frame")?;
        let stdin = self.child.stdin.as_mut().context("worker stdin")?;
        write_frame(stdin, &body)?;
        stdin.flush()?;
        Ok(())
    }

    /// Closes the worker's stdin and waits up to 30 s for it to exit, then kills it.
    fn stop(&mut self) -> Result<()> {
        drop(self.child.stdin.take());
        let stop = Instant::now() + Duration::from_secs(30);
        loop {
            if self.child.try_wait()?.is_some() {
                return Ok(());
            }
            if Instant::now() > stop {
                let _ = self.child.kill();
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn worker_init(o: &Options, start: Start, seed: u64) -> WorkerInit {
    WorkerInit {
        ipc: IPC_VERSION,
        game: 1,
        core: CoreKind::Real {
            gamedata_dir: to_wsl_path(&o.gamedata),
        },
        start,
        save_dir: String::new(),
        seed,
        cfg: WorkerConfig {
            basis_nw: BasisNw::All,
            digests: true,
            stats: true,
            ..WorkerConfig::default()
        },
    }
}

/// The build id of the worker's core (`Start::Describe`): a C5SV file must carry it.
pub fn worker_build_id(o: &Options) -> Result<String> {
    let mut p = Proc::start(o)?;
    p.send(&ToWorker::Init(worker_init(o, Start::Describe, 0)))?;
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let frame = p.frames.recv_timeout(left).map_err(|_| {
            anyhow::anyhow!("the worker gave no Ready to Describe within {WAIT:?} (or it died)")
        })?;
        for m in frame.msgs {
            match m {
                WorkerMsg::Fatal { message, code } => {
                    bail!("the worker is fatal ({code}): {message}")
                }
                WorkerMsg::Ready(r) => {
                    p.stop()?;
                    return Ok(r.build_id);
                }
                _ => {}
            }
        }
    }
}

/// Runs the worker on a new game of `setup` up to the first `Full` of seat 0 and stops it.
pub fn take_from_worker(o: &Options, setup: GameSetup) -> Result<Taken> {
    let seed = setup.settings.map_seed.or(setup.settings.seed).unwrap_or(0);
    take_start(o, Start::New(setup), u64::from(seed))
}

/// Runs the worker from `start` up to the first `Full` of seat 0 and stops it.
fn take_start(o: &Options, start: Start, seed: u64) -> Result<Taken> {
    let init = worker_init(o, start, seed);
    let mut p = Proc::start(o)?;
    p.send(&ToWorker::Init(init))?;
    let deadline = Instant::now() + WAIT;
    let (mut ready, mut basis, mut full, mut digest) = (None, None, None, None);
    let mut catalog = None;
    while ready.is_none() || basis.is_none() || full.is_none() {
        let left = deadline.saturating_duration_since(Instant::now());
        let frame = p
            .frames
            .recv_timeout(left)
            .map_err(|_| anyhow::anyhow!("the worker said nothing within {WAIT:?} (or it died)"))?;
        for m in frame.msgs {
            match m {
                WorkerMsg::Fatal { message, code } => {
                    bail!("the worker is fatal ({code}): {message}")
                }
                WorkerMsg::Catalog { bytes, .. } => catalog = Some(bytes),
                WorkerMsg::Ready(r) => ready = Some(r),
                WorkerMsg::Basis { players, basis: b } if players.contains(&0) => basis = Some(b),
                WorkerMsg::View {
                    player: 0,
                    update: ViewUpdate::Full { view, hash, .. },
                } if full.is_none() => full = Some((view, hash)),
                WorkerMsg::Stats(s) => digest = digest.or(s.digest),
                _ => {}
            }
        }
    }
    // The worker may still be writing its first stats; it is not needed.
    let _ = p.send(&ToWorker::Shutdown { save: false });
    p.stop()?;
    let ready = ready.context("no Ready")?;
    let basis = basis.context("no Basis for seat 0")?;
    let (view, hash) = full.context("no Full for seat 0")?;
    let keyframe = Keyframe {
        view,
        hash,
        basis_hash: basis.basis_hash,
    };
    Ok(Taken {
        width: ready.width,
        height: ready.height,
        wrap_x: ready.wrap_x,
        build_id: ready.build_id,
        basis,
        keyframe,
        state_digest: digest,
        catalog,
    })
}

// ------------------------------------------------------------------ a save of the original

/// The parts of a `.Civ5Save` the core reads (`civsim.saveio extract`).
pub struct SaveParts {
    /// The `CvPreGame::write` block.
    pub pregame: Vec<u8>,
    /// The inflated zlib stream: game, map, teams, players, supporting data.
    pub body: Vec<u8>,
    pub turn: i32,
}

/// Splits `save` with `civsim.saveio extract` into `dir` (`pregame.bin`, `body.bin`, `envelope.json`).
pub fn extract_save(civsim: &Path, save: &Path, dir: &Path) -> Result<SaveParts> {
    std::fs::create_dir_all(dir)?;
    let code = format!(
        "import sys; sys.path.insert(0, {:?}); from civsim.saveio.__main__ import main; \
         sys.exit(main(['extract', {:?}, '--out', {:?}, '--allow-outside-out']))",
        civsim.to_string_lossy(),
        save.to_string_lossy(),
        dir.to_string_lossy()
    );
    let st = Command::new("python")
        .args(["-I", "-c", &code])
        .stdout(Stdio::null())
        .status()
        .context("cannot start python (civsim.saveio extract)")?;
    if !st.success() {
        bail!("civsim.saveio extract {} failed ({st})", save.display());
    }
    let read = |n: &str| {
        std::fs::read(dir.join(n)).with_context(|| {
            format!(
                "{}: no {n} (a save without a body is a game configuration)",
                dir.display()
            )
        })
    };
    let env: serde_json::Value = serde_json::from_slice(&read("envelope.json")?)?;
    let mut pregame = read("pregame.bin")?;
    // The exe writes `GameMapTypes` of `writeArchive` as 1 byte (civsim `pregame.py`), the portable core's
    // `CvEnumSerialization` reads every enum as 4: widen it in place (little-endian, the value stays).
    let span = env["spans"]
        .as_array()
        .and_then(|s| s.iter().find(|x| x["name"] == "archive.game_map_type"));
    if let (Some(span), Some(base)) = (span, env["layout"]["pregame"]["offset"].as_u64())
        && span["length"].as_u64() == Some(1)
    {
        let at = span["offset"].as_u64().context("span offset")? + 1 - base;
        let at = usize::try_from(at)?;
        if at > pregame.len() {
            bail!(
                "{}: archive.game_map_type is past the pregame",
                dir.display()
            );
        }
        pregame.splice(at..at, [0u8; 3]);
    }
    Ok(SaveParts {
        pregame,
        body: read("body.bin")?,
        turn: env["summary"]["turn"].as_i64().unwrap_or(0) as i32,
    })
}

/// The core's setup of an imported save: the single game of B1 (the save is a single-player game; its pregame
/// carries the slots, seeds and options, the setup only the runtime's own keys).
pub fn import_setup_json(scene: &Scene) -> Result<String> {
    let s = &scene.setup;
    let need = |v: &Option<String>, what: &str| {
        v.clone()
            .with_context(|| format!("scene {}: [setup] has no {what}", scene.scene.id))
    };
    let setup = gamecore::Setup {
        seed: s.sync_seed.unwrap_or(0),
        map_seed: s.map_seed,
        game_type: gamecore::GameType::Single,
        human_civ: need(&s.human, "human")?,
        ai_count: s.ai.len() as u32,
        world_size: need(&s.world_size, "world_size")?,
        map_script: need(&s.map_script, "map_script")?,
        handicap: need(&s.handicap, "handicap")?,
        game_speed: need(&s.speed, "speed")?,
        start_era: need(&s.era, "era")?,
        minor_civs: gamecore::MinorCivs::Count(s.city_states.len() as u32),
        ..gamecore::Setup::default()
    };
    Ok(setup.to_json())
}

/// The GCS1 import container of the core (Lekmod `rt_save.h`): `header`, `setup`, `pregame`, `body`.
pub fn import_gcs1(setup_json: &str, parts: &SaveParts) -> Vec<u8> {
    let header = format!("format=GCS1 2\nimport=civ5save\nturn={}\n", parts.turn);
    let sections: [(&str, &[u8]); 4] = [
        ("header", header.as_bytes()),
        ("setup", setup_json.as_bytes()),
        ("pregame", &parts.pregame),
        ("body", &parts.body),
    ];
    let mut body = Vec::new();
    for (name, bytes) in sections {
        body.extend_from_slice(&(name.len() as u16).to_le_bytes());
        body.extend_from_slice(name.as_bytes());
        body.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        body.extend_from_slice(bytes);
    }
    let mut out = Vec::with_capacity(32 + body.len());
    out.extend_from_slice(b"GCS1");
    out.extend_from_slice(&2u32.to_le_bytes()); // kGcsFormatVersion
    out.extend_from_slice(&32u32.to_le_bytes()); // kGcsFixedHeader
    out.extend_from_slice(&(sections.len() as u32).to_le_bytes());
    out.extend_from_slice(&(body.len() as u64).to_le_bytes());
    out.extend_from_slice(&protocol::xxh64(0, &body).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// Runs the worker on the save of the original: extract, import container, C5SV file, `Start::Load`.
fn take_from_save(o: &Options, scene: &Scene, save: &Path) -> Result<Taken> {
    let dir = o.out.join("import");
    let parts = extract_save(&o.civsim, save, &dir)?;
    let gcs1 = import_gcs1(&import_setup_json(scene)?, &parts);
    let setup = scene_setup(scene)?;
    let build_id = worker_build_id(o)?;
    let file = game_host::saves::encode(
        &game_host::saves::SaveHeader {
            seq: 1,
            game: 1,
            turn: parts.turn,
            saved_at_ms: 0,
            kind: protocol::SaveKind::Keep,
            build_id,
        },
        &gcs1,
        &game_host::saves::SaveSections {
            seats: setup.slots.clone(),
            settings: setup.settings.clone(),
            journals: vec![],
            cmds: vec![],
            city_memo: vec![],
        },
    );
    let c5sv = dir.join("import.c5sv");
    std::fs::write(&c5sv, file)?;
    let seed = u64::from(setup.settings.map_seed.or(setup.settings.seed).unwrap_or(0));
    take_start(
        o,
        Start::Load {
            file: to_wsl_path(&std::path::absolute(&c5sv)?),
            seats: setup.slots,
        },
        seed,
    )
}

// ------------------------------------------------------------------ plots.json and the digest

#[derive(Debug, Serialize)]
pub struct PlotsDump {
    pub schema: &'static str,
    pub width: u16,
    pub height: u16,
    pub wrap_x: bool,
    pub plots: Vec<PlotDump>,
}

#[derive(Debug, Serialize)]
pub struct PlotDump {
    pub x: u16,
    pub y: u16,
    pub plot_type: String,
    pub terrain: i16,
    /// The basis's feature: relief features only (natural wonders, marsh, flood plains, ice), `-1`
    /// for the rest; forest, jungle, oasis and fallout are in `view_feature` (`Q_PLOT_BASIS`).
    pub feature: i16,
    /// The view's feature: every feature, of an unrevealed plot too (the whole world, NP-D19).
    pub view_feature: i16,
    pub resource: i16,
    pub improvement: i16,
    pub route: i16,
    pub owner: i8,
    pub rivers: u8,
    pub revealed: bool,
    pub visible: bool,
}

pub fn plots_dump(basis: &MapBasis, view: &WorldView) -> Result<PlotsDump> {
    let n = usize::from(basis.width) * usize::from(basis.height);
    if basis.plots.len() != n || view.map.plots.len() != n {
        bail!(
            "plot counts differ: basis {}, view {}, map {}x{}",
            basis.plots.len(),
            view.map.plots.len(),
            basis.width,
            basis.height
        );
    }
    let plots = (0..n)
        .map(|i| {
            let (b, v) = (&basis.plots[i], &view.map.plots[i]);
            PlotDump {
                x: (i % usize::from(basis.width)) as u16,
                y: (i / usize::from(basis.width)) as u16,
                plot_type: format!("{:?}", b.plot_type),
                terrain: b.terrain,
                feature: b.feature,
                view_feature: v.feature,
                resource: v.resource,
                improvement: v.improvement,
                route: v.route,
                owner: v.owner,
                rivers: b.rivers,
                revealed: v.flags.contains(PlotFlags::REVEALED),
                visible: v.flags.contains(PlotFlags::VISIBLE),
            }
        })
        .collect();
    Ok(PlotsDump {
        schema: "civ5-golden-plots/1",
        width: basis.width,
        height: basis.height,
        wrap_x: basis.wrap_x,
        plots,
    })
}

/// FNV-1a 64 of bytes (the digest of `plots.json`).
fn fnv64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

#[derive(Debug, Serialize)]
struct Digest {
    scene: String,
    build_id: String,
    width: u16,
    height: u16,
    basis_hash: String,
    view_hash: String,
    plots_fnv64: String,
    state_digest: Option<String>,
}

// ------------------------------------------------------------------ G-D4: the map-match check

/// The `FowMask` of a capture: 4 bytes per texel, `x` the hex is known, `z` it is seen now.
pub struct FowMask {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

impl FowMask {
    /// The mask the terrain draw of `<cache>/golden/<scene>/<capture>` read as texture slot 4
    /// (`targets/<event>-in-ps4.dds`); `None` if the capture has none.
    pub fn from_capture(dir: &Path) -> Result<Option<FowMask>> {
        let draws = dir.join("draws.json");
        if !draws.is_file() {
            return Ok(None);
        }
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&draws)?)
            .with_context(|| format!("parse {}", draws.display()))?;
        let event = v["draws"].as_array().and_then(|d| {
            d.iter()
                .find(|x| {
                    x["ps_entry"].as_str().is_some_and(|e| {
                        e.starts_with("TerrainShader::TerrainRenderCachedDiffuseAndSpec")
                    })
                })
                .and_then(|x| x["event"].as_u64())
        });
        let Some(event) = event else {
            return Ok(None);
        };
        let file = dir.join("targets").join(format!("{event}-in-ps4.dds"));
        if !file.is_file() {
            return Ok(None);
        }
        let bytes = std::fs::read(&file)?;
        // DDS with the DX10 header: magic 4 + header 124 + DX10 20; the size is at 12 (height) and 16 (width).
        if bytes.len() < 148 || &bytes[..4] != b"DDS " {
            bail!("{}: not a DDS file", file.display());
        }
        let rd = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap()) as usize;
        let (height, width) = (rd(12), rd(16));
        if bytes.len() < 148 + width * height * 4 {
            bail!(
                "{}: the DDS is shorter than {width}x{height}",
                file.display()
            );
        }
        Ok(Some(FowMask {
            width,
            height,
            rgba: bytes[148..148 + width * height * 4].to_vec(),
        }))
    }

    fn at(&self, x: usize, y: usize) -> (bool, bool) {
        let o = (y.min(self.height - 1) * self.width + x.min(self.width - 1)) * 4;
        (self.rgba[o] >= 128, self.rgba[o + 2] >= 128)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CellDiff {
    pub x: u16,
    pub y: u16,
    pub ours_revealed: bool,
    pub ours_visible: bool,
    pub capture_known: bool,
    pub capture_seen: bool,
}

#[derive(Debug, Serialize)]
pub struct MapMatch {
    pub schema: &'static str,
    pub scene: String,
    pub capture: String,
    pub matches: bool,
    pub width: u16,
    pub height: u16,
    pub ours_revealed: usize,
    pub ours_visible: usize,
    pub capture_known: usize,
    pub capture_seen: usize,
    /// How hexes were mapped to mask texels (the best of the tried layouts).
    pub mapping: String,
    pub differing: Vec<CellDiff>,
    pub note: String,
}

/// Best fit so far: Jaccard index, scale, flip, `(known, seen)` of the capture per hex.
type Fit = (f64, f32, bool, Vec<(bool, bool)>);

/// Fog states of hex centres against the mask (G-D4). The mask of the original is not the whole map: it is a
/// window of 1024 x 1024 texels around the camera, at a scale the capture does not tell. So the hex centres
/// (`TerrainFrame::hex_to_tex`) are mapped by `p = c_mask + (t - c_ours) * s`, with the mask row 0 at the
/// top or at the bottom, where `c_ours` is the centre of our visible hexes and `c_mask` the centroid of the
/// texels the capture marks as known; the scale `s` is searched for the best Jaccard index of the sets of
/// "seen" hexes. The result is `matches` if the best sets (known and seen) are equal: the same start
/// place and the same relief around it.
pub fn compare_fog(
    scene: &str,
    capture: &str,
    basis: &MapBasis,
    view: &WorldView,
    mask: &FowMask,
) -> MapMatch {
    let (w, h) = (usize::from(basis.width), usize::from(basis.height));
    let frame = render::world::terrain::TerrainFrame::new(
        u32::from(basis.width),
        u32::from(basis.height),
        basis.wrap_x,
    );
    let flags = |x: usize, y: usize| {
        let f = view.map.plots[y * w + x].flags;
        (
            f.contains(PlotFlags::REVEALED),
            f.contains(PlotFlags::VISIBLE),
        )
    };
    let centres: Vec<(f32, f32)> = (0..w * h)
        .map(|i| {
            let t = frame.hex_to_tex((i % w) as i32, (i / w) as i32);
            (t.x, t.y)
        })
        .collect();
    // Our anchor: the centre of the seen hexes (hexes, not the wrap seam: a start is far from it).
    let seen: Vec<usize> = (0..w * h).filter(|&i| flags(i % w, i / w).1).collect();
    let anchor = if seen.is_empty() {
        (0.0, 0.0)
    } else {
        let n = seen.len() as f32;
        (
            seen.iter().map(|&i| centres[i].0).sum::<f32>() / n,
            seen.iter().map(|&i| centres[i].1).sum::<f32>() / n,
        )
    };
    // The mask anchor: the centroid of the known texels.
    let (mut sx, mut sy, mut cnt) = (0.0f64, 0.0f64, 0u64);
    for y in 0..mask.height {
        for x in 0..mask.width {
            if mask.at(x, y).0 {
                sx += x as f64;
                sy += y as f64;
                cnt += 1;
            }
        }
    }
    let c = if cnt == 0 {
        (mask.width as f64 / 2.0, mask.height as f64 / 2.0)
    } else {
        (sx / cnt as f64, sy / cnt as f64)
    };
    // `(known, seen)` of the capture at every hex under a scale and a flip.
    let sample = |s: f32, flip: bool| -> Vec<(bool, bool)> {
        centres
            .iter()
            .map(|&(tx, ty)| {
                let px = c.0 as f32 + (tx - anchor.0) * s;
                let dy = (ty - anchor.1) * s;
                let py = c.1 as f32 + if flip { -dy } else { dy };
                if px < 0.0 || py < 0.0 || px >= mask.width as f32 || py >= mask.height as f32 {
                    (false, false)
                } else {
                    mask.at(px as usize, py as usize)
                }
            })
            .collect()
    };
    let jaccard = |cap: &[(bool, bool)]| {
        let (mut both, mut any) = (0usize, 0usize);
        for (i, (_, cs)) in cap.iter().enumerate() {
            let os = flags(i % w, i / w).1;
            both += usize::from(*cs && os);
            any += usize::from(*cs || os);
        }
        if any == 0 {
            1.0
        } else {
            both as f64 / any as f64
        }
    };
    let mut best: Option<Fit> = None;
    let mut step = 0;
    while step <= 300 {
        let s = 0.2 + step as f32 * 0.01;
        for flip in [false, true] {
            let cap = sample(s, flip);
            let j = jaccard(&cap);
            if best.as_ref().is_none_or(|b| j > b.0 + 1e-9) {
                best = Some((j, s, flip, cap));
            }
        }
        step += 1;
    }
    let (jac, scale, flip, cap) = best.expect("scales tried");
    let differing: Vec<CellDiff> = (0..w * h)
        .filter_map(|i| {
            let (rev, vis) = flags(i % w, i / w);
            let (known, seen) = cap[i];
            ((rev, vis) != (known, seen)).then_some(CellDiff {
                x: (i % w) as u16,
                y: (i / w) as u16,
                ours_revealed: rev,
                ours_visible: vis,
                capture_known: known,
                capture_seen: seen,
            })
        })
        .collect();
    let count =
        |f: &dyn Fn(PlotFlags) -> bool| view.map.plots.iter().filter(|p| f(p.flags)).count();
    MapMatch {
        schema: "civ5-golden-map-mismatch/1",
        scene: scene.into(),
        capture: capture.into(),
        matches: differing.is_empty(),
        width: basis.width,
        height: basis.height,
        ours_revealed: count(&|f| f.contains(PlotFlags::REVEALED)),
        ours_visible: count(&|f| f.contains(PlotFlags::VISIBLE)),
        capture_known: cap.iter().filter(|c| c.0).count(),
        capture_seen: cap.iter().filter(|c| c.1).count(),
        mapping: format!(
            "fitted: mask px = centroid + (hex centre texel - centre of our seen hexes) * {scale:.2}, mask row 0 at the {}; Jaccard index of the seen sets {jac:.3}",
            if flip { "bottom" } else { "top" }
        ),
        differing,
        note: String::new(),
    }
}

// ------------------------------------------------------------------ the command

#[derive(Debug)]
pub struct WorldResult {
    pub out: PathBuf,
    pub width: u16,
    pub height: u16,
    pub basis_hash: u64,
    pub map_match: Option<MapMatch>,
    pub map_match_note: String,
}

/// Why the scene cannot be compared and is skipped, not failed: a scene built from a world builder map (S3)
/// is only worth building next to the capture it is compared with.
pub fn skip_reason(o: &Options) -> Result<Option<String>> {
    let scene = Scene::load_by_id(&o.scenes_dir, &o.scene)?;
    Ok(missing_capture(&scene, &o.cache))
}

fn missing_capture(scene: &Scene, cache: &Path) -> Option<String> {
    scene.setup.world_builder_map.as_ref()?;
    let absent: Vec<_> = scene
        .scene
        .captures
        .iter()
        .map(|c| scene.capture_path(cache, c))
        .filter(|p| !p.is_file())
        .collect();
    (absent.len() == scene.scene.captures.len()).then(|| {
        format!(
            "the capture {} is not taken (cache/captures/{} is missing); `golden prep {}` takes it",
            absent[0].display(),
            scene.scene.id,
            scene.scene.id
        )
    })
}

pub fn run(o: &Options) -> Result<WorldResult> {
    let scene = Scene::load_by_id(&o.scenes_dir, &o.scene)?;
    let save = o.save.clone().or_else(|| {
        scene
            .setup
            .save
            .as_ref()
            .map(|s| o.cache.join("captures").join(s))
    });
    let taken = match &save {
        Some(save) => take_from_save(o, &scene, save)?,
        None => take_from_worker(o, scene_setup(&scene)?)?,
    };
    std::fs::create_dir_all(&o.out)?;
    std::fs::write(o.out.join("mapbasis.wv3"), wv3::encode_basis(&taken.basis)?)?;
    std::fs::write(o.out.join("world.wv3"), wv3::encode_world(&taken.keyframe)?)?;
    if let Some(bytes) = &taken.catalog {
        std::fs::write(o.out.join("catalog.wv3"), wv3::encode_catalog_bytes(bytes))?;
    }
    let dump = plots_dump(&taken.basis, &taken.keyframe.view)?;
    let plots = serde_json::to_string_pretty(&dump)? + "\n";
    std::fs::write(o.out.join("plots.json"), &plots)?;
    let digest = Digest {
        scene: scene.scene.id.clone(),
        build_id: taken.build_id.clone(),
        width: taken.width,
        height: taken.height,
        basis_hash: format!("{:016x}", taken.basis.basis_hash),
        view_hash: format!("{:016x}", taken.keyframe.hash),
        plots_fnv64: format!("{:016x}", fnv64(plots.as_bytes())),
        state_digest: taken.state_digest.map(|d| format!("{d:016x}")),
    };
    std::fs::write(
        o.out.join("digest.json"),
        serde_json::to_string_pretty(&digest)? + "\n",
    )?;

    // G-D4. S2 is the same game as S1 (the check of S1 covers it); S3 has the mask of its capture only
    // with `reveal = "capture-mask"` and needs the scenario map, which `scene_setup` refused above.
    let mismatch = o.out.join("map-mismatch.json");
    let _ = std::fs::remove_file(&mismatch);
    let (mut map_match, mut note) = (None, String::new());
    if scene.view.reveal == "game" && scene.scene.id == "S1" {
        // The first capture with a mask decides; the others are checked too and a mismatch of any is written.
        for cap in &scene.scene.captures {
            let dir = o.cache.join("golden").join(&scene.scene.id).join(cap);
            match FowMask::from_capture(&dir)? {
                Some(mask) => {
                    let m = compare_fog(
                        &scene.scene.id,
                        cap,
                        &taken.basis,
                        &taken.keyframe.view,
                        &mask,
                    );
                    if !m.matches && !mismatch.exists() {
                        std::fs::write(&mismatch, serde_json::to_string_pretty(&m)? + "\n")?;
                    }
                    println!(
                        "map check against {cap}: {} ({} hexes differ)",
                        if m.matches { "MATCH" } else { "MISMATCH" },
                        m.differing.len()
                    );
                    map_match = map_match.or(Some(m));
                }
                None => {
                    note += &format!("no FowMask in {}: skipped; ", dir.display());
                }
            }
        }
    } else {
        note = "the map check applies to S1 only (S2 is the same game)".into();
    }
    Ok(WorldResult {
        out: o.out.clone(),
        width: taken.width,
        height: taken.height,
        basis_hash: taken.basis.basis_hash,
        map_match,
        map_match_note: note,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wsl_paths() {
        assert_eq!(
            to_wsl_path(Path::new(r"E:\games\civ5 x\a")),
            "/mnt/e/games/civ5 x/a"
        );
        assert_eq!(to_wsl_path(Path::new("/home/a")), "/home/a");
    }

    #[test]
    fn setup_of_s1() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("scenes");
        let s = Scene::load_by_id(&dir, "S1").unwrap();
        let g = scene_setup(&s).unwrap();
        assert_eq!(g.settings.map_seed, Some(12345));
        assert_eq!(g.settings.minor_civs, 0);
        assert_eq!(g.slots.len(), 2);
        assert_eq!(g.slots[0].civ, CivPick::Civ("CIVILIZATION_ROME".into()));
        assert!(matches!(g.slots[1].control, SlotControl::Ai));
    }

    #[test]
    fn s3_is_skipped_without_its_capture_and_s1_never() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("scenes");
        let empty = std::env::temp_dir().join("golden-no-captures");
        let s3 = Scene::load_by_id(&dir, "S3").unwrap();
        let why = missing_capture(&s3, &empty).unwrap();
        assert!(why.contains("S3.rdc") && why.contains("not taken"), "{why}");
        let s1 = Scene::load_by_id(&dir, "S1").unwrap();
        assert!(missing_capture(&s1, &empty).is_none());
    }

    #[test]
    fn s3_is_built_from_the_world_builder_map() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("scenes");
        let s = Scene::load_by_id(&dir, "S3").unwrap();
        let g = scene_setup(&s).unwrap();
        assert_eq!(g.slots.len(), 2);
        // The worker turns it into the core's `world_builder_map` with the scenario data.
        let core = game_host::real::network_setup(&g, 0).unwrap();
        assert_eq!(
            core.world_builder_map.as_deref(),
            Some("maps/Tutorials/Maps/LateGameTest.Civ5Map")
        );
        assert!(core.world_builder_scenario);
        assert!(
            core.to_json()
                .contains("\"world_builder_map\":\"maps/Tutorials/Maps/LateGameTest.Civ5Map\"")
        );
        assert!(!core.to_json().contains("world_size"));
    }

    /// A mask painted from a set of seen hexes at scale 1.5 (a square of 90 px around each centre), as
    /// a window around the camera; `extra` adds a hex that we do not see.
    fn painted(
        w: u16,
        h: u16,
        seen: &[(i32, i32)],
        extra: Option<(i32, i32)>,
    ) -> (MapBasis, WorldView, FowMask) {
        use protocol::{PlotView, TurnStatus};
        let frame = render::world::terrain::TerrainFrame::new(u32::from(w), u32::from(h), false);
        let mut view = WorldView {
            turn: 0,
            year: 0,
            human: 0,
            human_team: 0,
            status: TurnStatus::HumanTurn,
            blocker: None,
            map: Default::default(),
            units: vec![],
            cities: vec![],
            players: vec![],
            top_bar: Default::default(),
            notifications: vec![],
            peace_offers: vec![],
            trade_routes: vec![],
        };
        view.map.plots = vec![PlotView::default(); usize::from(w) * usize::from(h)];
        for &(x, y) in seen {
            view.map.plots[y as usize * usize::from(w) + x as usize].flags =
                PlotFlags(PlotFlags::REVEALED | PlotFlags::VISIBLE);
        }
        let basis = MapBasis {
            width: w,
            height: h,
            plots: vec![Default::default(); usize::from(w) * usize::from(h)],
            ..MapBasis::default()
        };
        let mut mask = FowMask {
            width: 1024,
            height: 1024,
            rgba: vec![0; 1024 * 1024 * 4],
        };
        let c0 = frame.hex_to_tex(seen[0].0, seen[0].1);
        for &(x, y) in seen.iter().chain(extra.iter()) {
            let t = frame.hex_to_tex(x, y);
            let (cx, cy) = (512.0 + (t.x - c0.x) * 1.5, 512.0 + (t.y - c0.y) * 1.5);
            for py in (cy as i32 - 40)..(cy as i32 + 40) {
                for px in (cx as i32 - 40)..(cx as i32 + 40) {
                    let o = (py as usize * 1024 + px as usize) * 4;
                    mask.rgba[o] = 255;
                    mask.rgba[o + 2] = 255;
                }
            }
        }
        (basis, view, mask)
    }

    #[test]
    fn fog_check_finds_the_same_set_and_a_different_one() {
        let seen = [(4, 4), (5, 4), (4, 5), (5, 5), (3, 5)];
        let (b, v, m) = painted(10, 10, &seen, None);
        let r = compare_fog("S1", "S1a", &b, &v, &m);
        assert!(r.matches, "{:?}", r.differing);
        assert_eq!((r.ours_visible, r.capture_seen), (5, 5));
        let (b, v, m) = painted(10, 10, &seen, Some((8, 8)));
        let r = compare_fog("S1", "S1a", &b, &v, &m);
        assert!(!r.matches);
    }

    #[test]
    fn the_import_container_has_the_four_sections_and_the_checksum_of_its_body() {
        let parts = SaveParts {
            pregame: vec![1, 2, 3],
            body: vec![9; 10],
            turn: 0,
        };
        let g = import_gcs1("{}", &parts);
        assert_eq!(&g[..4], b"GCS1");
        let u32_at = |o: usize| u32::from_le_bytes(g[o..o + 4].try_into().unwrap());
        let u64_at = |o: usize| u64::from_le_bytes(g[o..o + 8].try_into().unwrap());
        assert_eq!((u32_at(4), u32_at(8), u32_at(12)), (2, 32, 4));
        assert_eq!(u64_at(16) as usize, g.len() - 32);
        assert_eq!(u64_at(24), protocol::xxh64(0, &g[32..]));
        // The last section is the body, whole.
        assert_eq!(&g[g.len() - 10..], &[9; 10]);
        assert_eq!(&g[g.len() - 18..g.len() - 14], b"body");
    }

    #[test]
    fn the_import_setup_is_a_single_game_of_the_scene() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("scenes");
        let s = Scene::load_by_id(&dir, "S1").unwrap();
        let j = import_setup_json(&s).unwrap();
        assert!(j.contains("\"game_type\":\"single\""), "{j}");
        assert!(j.contains("CIVILIZATION_ROME"));
        assert_eq!(s.setup.save.as_deref(), Some("v2/S1/golden-S1a.Civ5Save"));
    }

    #[test]
    fn fnv_known_values() {
        assert_eq!(fnv64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
