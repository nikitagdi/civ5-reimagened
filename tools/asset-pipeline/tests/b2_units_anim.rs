//! R2.2: clips (`C5A2`) and idle state machines (`C5FM` v2) of the B2 unit models, on the real
//! install when `CIV5_ROOT` is set (the set test also needs `CIV5_DB_SNAPSHOT`). They decode
//! Granny files: run with `cargo test --release -p asset-pipeline --test b2_units_anim`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use asset_format::{
    AnimPack, Channel, FsmCondition, IdleFsm, LogicalId, Manifest, ModelBlobV2, Transform,
};
use asset_pipeline::anim::fsm::{parse_ddtxml, parse_fsmxml};
use asset_pipeline::anim::idle::{ModelKind, PackSetup, build_pack};
use asset_pipeline::anim::resample::Tolerance;
use asset_pipeline::art::resolve_fxsxml;
use asset_pipeline::game::GameSource;
use asset_pipeline::models::bake_v2::primary_skeleton;
use asset_pipeline::models::basis::Basis;
use asset_pipeline::models::convert::convert_skeleton_any;
use asset_pipeline::sets::b2_units::{Options, build_units};
use granny::{GrannyFile, Scene};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let p = std::env::temp_dir().join(format!(
            "civ5-b2units-anim-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn real_game() -> Option<GameSource> {
    let Some(root) = std::env::var_os("CIV5_ROOT") else {
        println!("skipped: CIV5_ROOT not set");
        return None;
    };
    Some(GameSource::open(PathBuf::from(root)).unwrap())
}

fn fsm_of(game: &GameSource, file: &str) -> IdleFsm {
    let bytes = game.read_by_name(file).unwrap().unwrap();
    parse_fsmxml(
        file.trim_end_matches(".fsmxml"),
        &String::from_utf8_lossy(&bytes),
    )
    .unwrap()
}

#[test]
fn the_omni_machine() {
    let Some(game) = real_game() else { return };
    let m = fsm_of(&game, "1_omni_state_01.fsmxml");
    // The default state is the chooser (state 2 of the file).
    assert_eq!(m.states[usize::from(m.default)].name, "IDLE_CHOOSER_DUMMY");
    assert_eq!(m.states[usize::from(m.default)].action, 0);
    // The states that a requested action enters are there with their action codes.
    let action = |name: &str| m.states[m.state(name).expect(name)].action;
    assert_eq!(action("IDLE_LOOP"), 1000);
    assert_eq!(action("FIDGETS_GRAPH"), 1000);
    assert_eq!(action("FORTIFY_LOOP"), 1500);
    assert_eq!(action("COMBAT_READY_LOOP"), 1600);
    assert_eq!(action("BOMBARD_DEFEND_LOOP"), 1280);
    // The chooser leaves to ENTER_FORTIFY and IDLE_LOOP, in the order of the sockets of the
    // file; the weights 0.4 / 0.6 are the random option of the second one.
    let chooser = &m.states[usize::from(m.default)];
    let to: Vec<&str> = chooser
        .transitions
        .iter()
        .map(|t| m.states[usize::from(t.to)].name.as_str())
        .collect();
    assert_eq!(to, ["ENTER_FORTIFY", "IDLE_LOOP"]);
    assert_eq!(
        chooser.transitions[1].conditions,
        [
            FsmCondition::EventRandomPathOption {
                path: 0,
                value: 0,
                use_weights: true,
                options: vec![(1, 0.4), (0, 0.6)],
            },
            FsmCondition::CheckPathOption {
                method: 6,
                path: 4,
                value: 1500
            },
        ]
    );
    // FIDGETS_GRAPH -> IDLE_LOOP: conditions with an OrOperator between two groups, in file order.
    let fidgets = &m.states[m.state("FIDGETS_GRAPH").unwrap()];
    let back = fidgets
        .transitions
        .iter()
        .find(|t| m.states[usize::from(t.to)].name == "IDLE_LOOP")
        .unwrap();
    assert_eq!(back.conditions.len(), 6);
    assert_eq!(back.conditions[3], FsmCondition::Or);
    assert!(matches!(
        back.conditions[2],
        FsmCondition::EcComplete {
            ec: 1040,
            value: None
        }
    ));
    assert!(m.states.iter().all(|s| s.next.is_empty()));
    assert_eq!(IdleFsm::decode(&m.encode()).unwrap(), m);
}

#[test]
fn a_worker_machine_has_sleep() {
    let Some(game) = real_game() else { return };
    let m = fsm_of(&game, "civilian_state_01.fsmxml");
    let s = &m.states[m.state("SLEEP_LOOP").unwrap()];
    assert_eq!(s.action, 1500);
    assert!(!s.transitions.is_empty());
}

#[test]
fn every_machine_of_the_install_compiles_but_two() {
    let Some(game) = real_game() else { return };
    let mut broken = Vec::new();
    let mut n = 0;
    for e in game.iter_ext("fsmxml") {
        let text = String::from_utf8_lossy(&game.read(e).unwrap()).into_owned();
        match parse_fsmxml(&e.name, &text) {
            Ok(m) => {
                n += 1;
                assert_eq!(IdleFsm::decode(&m.encode()).unwrap(), m, "{}", e.name);
            }
            Err(why) => broken.push((e.name.to_ascii_lowercase(), why)),
        }
    }
    println!("{n} machines compile; not compiled: {broken:?}");
    assert!(n >= 60);
    // Lekmod's copy of the civilian machine has an unquoted attribute (not XML).
    // `globe_state` (not a unit's) has a path option compared with 1.1, where the format has an
    // integer; the Lekmod civilian copy has an unquoted attribute (not XML).
    assert!(
        broken.iter().all(|(n, _)| matches!(
            n.as_str(),
            "edited_aim_civilian_state_01.fsmxml" | "globe_state.fsmxml"
        )),
        "{broken:?}"
    );
}

#[test]
fn landmark_decision_trees_give_state_clips() {
    let Some(game) = real_game() else { return };
    let text =
        |f: &str| String::from_utf8_lossy(&game.read_by_name(f).unwrap().unwrap()).into_owned();
    let rows = parse_ddtxml(&text("landmarkdecisiontree.ddtxml")).unwrap();
    let states: Vec<&str> = rows.iter().map(|r| r.state.as_str()).collect();
    assert_eq!(
        states,
        [
            "RawResource",
            "UnderConstruction",
            "Constructed",
            "Pillaged"
        ]
    );
    let mut n = 0;
    for e in game.iter_ext("ddtxml") {
        let rows = parse_ddtxml(&String::from_utf8_lossy(&game.read(e).unwrap()))
            .unwrap_or_else(|why| panic!("{}: {why}", e.name));
        // A tree whose first layer is empty (the buildings') has no animation rows.
        n += usize::from(!rows.is_empty());
    }
    assert!(n >= 5, "{n} trees with rows");
}

/// The settler figure `EuroFemale18` has two skeletons (a one-bone `WORLD_center` first, the
/// 23-bone body second) and each clip one track group per skeleton: the body's group is the one
/// packed, so every idle clip moves bones (it used to pack the one-bone group: 313 bytes for 10
/// clips, the figure stood in the bind pose).
#[test]
fn a_two_skeleton_settler_packs_the_body_tracks() {
    let Some(game) = real_game() else { return };
    let r = resolve_fxsxml(
        &game,
        "eurofemale18.fxsxml",
        "eurofemale18.fxsxml",
        1.0,
        None,
    )
    .unwrap();
    let mesh = game.read_by_name(&r.meshes()[0]).unwrap().unwrap();
    let scene = Scene::read(&GrannyFile::parse(&mesh).unwrap()).unwrap();
    assert!(
        scene.skeletons.len() >= 2,
        "the fixture is a two-skeleton gr2"
    );
    let (_, si) = primary_skeleton(&scene).unwrap();
    let basis = Basis::CIV5_TO_ENGINE;
    let skeleton = convert_skeleton_any(&scene.skeletons[si], &basis, &mut Vec::new()).unwrap();
    let rest: Vec<Transform> = skeleton.rest_pose();
    let pack = build_pack(
        &game,
        r.animations(),
        &PackSetup {
            kind: ModelKind::Unit,
            skeleton: &scene.skeletons[si],
            rest: &rest,
            size: 100.0,
            basis: &basis,
            tolerance: &Tolerance::default(),
            tree_codes: &[],
        },
    )
    .unwrap()
    .unwrap();
    assert!(pack.stats.has_idle_1000);
    assert!(pack.warnings.is_empty(), "{:?}", pack.warnings);
    for (c, clip) in pack.pack.clips.iter().enumerate() {
        let moving = clip
            .tracks
            .iter()
            .filter(|t| matches!(t.rot, Channel::Keys { .. }))
            .count();
        assert!(
            moving >= 5,
            "clip {c} {}: {moving} rotating bones",
            clip.name
        );
    }
}

/// A clip packed from the install, compared with the reference sampler on the 30 Hz grid.
#[test]
fn a_worker_pack_follows_the_reference() {
    let Some(game) = real_game() else { return };
    let r = resolve_fxsxml(&game, "worker_euro.fxsxml", "worker_euro.fxsxml", 1.0, None).unwrap();
    let mesh = game.read_by_name(&r.meshes()[0]).unwrap().unwrap();
    let scene = Scene::read(&GrannyFile::parse(&mesh).unwrap()).unwrap();
    let (_, si) = primary_skeleton(&scene).unwrap();
    let basis = Basis::CIV5_TO_ENGINE;
    let skeleton = convert_skeleton_any(&scene.skeletons[si], &basis, &mut Vec::new()).unwrap();
    let rest: Vec<Transform> = skeleton.rest_pose();
    let tol = Tolerance::default();
    let pack = build_pack(
        &game,
        r.animations(),
        &PackSetup {
            kind: ModelKind::Unit,
            skeleton: &scene.skeletons[si],
            rest: &rest,
            // Civ5 unit models are about 100 units tall; the set uses the box of the meshes.
            size: 100.0,
            basis: &basis,
            tolerance: &tol,
            tree_codes: &[],
        },
    )
    .unwrap()
    .unwrap();
    assert!(pack.stats.has_idle_1000);
    // Keys sit at times in 1/65535 of the clip, so a very fast move can be off by a little more
    // than the tolerance at one frame; the set test below holds the whole install to it.
    let loose = Tolerance {
        rot_deg: tol.rot_deg * 1.1,
        pos_percent: tol.pos_percent * 1.1,
        scale_percent: tol.scale_percent * 1.1,
    };
    assert!(
        pack.stats.max_dev.within(&loose),
        "{:?}",
        pack.stats.max_dev
    );
    assert!(pack.warnings.len() <= 2, "{:?}", pack.warnings);
    assert!(pack.stats.keys * 4 < pack.stats.frames, "{:?}", pack.stats);
    // Fortify, sleep and work clips are there as well, each answering to its code.
    let codes: Vec<u32> = pack.clips.iter().map(|c| c.ec).collect();
    assert!(codes.contains(&1000) && codes.iter().any(|c| (1500..=1580).contains(c)));
    // The blob decodes, and the clips of a worker move some bone.
    let decoded = AnimPack::decode(&pack.pack.encode()).unwrap();
    assert_eq!(decoded, pack.pack);
    assert!(decoded.bone_span() <= skeleton.bones.len());
    let moving = decoded.clips.iter().all(|c| {
        c.tracks
            .iter()
            .any(|t| !matches!(t.rot, Channel::Rest) || !matches!(t.pos, Channel::Rest))
    });
    assert!(moving);
}

fn read_manifest(out: &Path) -> Manifest {
    let hash = std::fs::read_to_string(out.join("manifest")).unwrap();
    Manifest::decode(&std::fs::read(out.join(hash.trim())).unwrap()).unwrap()
}

fn blob(out: &Path, m: &Manifest, id: &LogicalId) -> Vec<u8> {
    let v = m.variants(id);
    assert_eq!(v.len(), 1, "{}", id.as_str());
    std::fs::read(out.join(v[0].hash.to_string())).unwrap()
}

/// The whole set: every model has an idle 1000 clip, the pack and the machine it names exist and
/// agree with the skeleton, nothing failed, the packs fit the budget.
#[test]
fn the_unit_set_packs_clips_and_machines() {
    let (Some(game), Some(snap)) = (real_game(), std::env::var_os("CIV5_DB_SNAPSHOT")) else {
        println!("skipped: CIV5_DB_SNAPSHOT not set");
        return;
    };
    let out = TempDir::new();
    let catalog = std::env::var_os("CIV5_SHADER_CATALOG").map(PathBuf::from);
    let s = build_units(
        &game,
        Path::new(&snap),
        &out.0,
        &Options {
            shader_catalog: catalog,
            clip_tolerance: Tolerance::default(),
        },
    )
    .unwrap();
    println!(
        "models {} failed {} clips {} keys {} packs {:.2} MB fsm {} ({} states)",
        s.models,
        s.failed_models,
        s.anims.clips,
        s.anims.keys,
        s.anims.bytes as f64 / 1e6,
        s.anims.fsm.machines,
        s.anims.fsm.states
    );
    if std::env::var_os("CIV5_SHADER_CATALOG").is_some() {
        assert_eq!(s.failed_models, 0);
    }
    assert!(s.anims.no_idle.is_empty(), "{:?}", s.anims.no_idle);
    assert!(s.anims.max_dev.within(&Tolerance {
        rot_deg: asset_pipeline::anim::DEFAULT_ROT_DEG,
        ..Tolerance::default()
    }));
    // AB criterion 7: at most 17 MB of clips (the other groups of the game come on top).
    assert!(s.anims.bytes <= 17_000_000, "{}", s.anims.bytes);
    assert!(s.anims.fsm.failed.is_empty(), "{:?}", s.anims.fsm.failed);

    let m = read_manifest(&out.0);
    let csv = std::fs::read_to_string(out.0.join("units.csv")).unwrap();
    let mut models: Vec<String> = csv
        .lines()
        .skip(1)
        .filter(|l| l.starts_with("member,"))
        .map(|l| {
            l.split(',')
                .nth(3)
                .unwrap()
                .trim_end_matches(".fxsxml")
                .to_owned()
        })
        .collect();
    models.sort();
    models.dedup();
    let mut checked = 0;
    for id in models.iter().map(|m| LogicalId::new(format!("md:{m}"))) {
        let id = &id;
        let model = ModelBlobV2::decode(&blob(&out.0, &m, id)).unwrap();
        let an = model.anim_pack.as_ref().expect("a pack");
        let pack = AnimPack::decode(&blob(&out.0, &m, an)).unwrap();
        assert!(
            model.clips.iter().any(|c| c.ec == 1000),
            "{} has no ec 1000",
            id.as_str()
        );
        assert!(pack.bone_span() <= model.skeleton.bones.len());
        for c in &model.clips {
            assert!(usize::from(c.index) < pack.clips.len());
        }
        if let Some(f) = &model.fsm {
            let fsm = IdleFsm::decode(&blob(&out.0, &m, f)).unwrap();
            assert!(!fsm.states.is_empty());
        }
        checked += 1;
    }
    assert_eq!(checked, s.models);
}
