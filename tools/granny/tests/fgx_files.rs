//! `FireGrafixMaterial` checks on real Civ5 files. Runs only with `CIV5_ROOT` set; use `--release`.
//!
//! Reads `Resource/Common/*.fpk` directly (a minimal directory reader, since `granny` cannot
//! depend on `asset-pipeline`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use granny::GrannyFile;
use granny::extended::read_fgx_materials;

fn civ5_root() -> Option<PathBuf> {
    let root = std::env::var_os("CIV5_ROOT").map(PathBuf::from);
    if root.is_none() {
        println!("skipped: CIV5_ROOT not set");
    }
    root
}

/// FPK directory: `(name, offset, size)`; layout as documented in `asset-pipeline`'s `fpk.rs`.
fn fpk_entries(data: &[u8]) -> Vec<(String, usize, usize)> {
    let u32_at = |p: usize| -> Option<usize> {
        Some(u32::from_le_bytes(data.get(p..p + 4)?.try_into().ok()?) as usize)
    };
    let mut out = Vec::new();
    if data.get(..10) != Some(b"\x06\x00\x00\x00FPK_\x00\x00") {
        return out;
    }
    let (Some(count), mut pos) = (u32_at(10), 14usize) else {
        return out;
    };
    for _ in 0..count {
        let Some(len) = u32_at(pos) else { break };
        pos += 4;
        let Some(name) = data.get(pos..pos + len) else {
            break;
        };
        let name = String::from_utf8_lossy(name).into_owned();
        pos += len;
        pos += (6 - pos % 4) % 4 + 8;
        let (Some(size), Some(offset)) = (u32_at(pos), u32_at(pos + 4)) else {
            break;
        };
        pos += 8;
        out.push((name, offset, size));
    }
    out
}

/// `lower-case name -> bytes` of the gr2 entries of one pack.
fn pack_gr2(root: &Path, pack: &str) -> BTreeMap<String, Vec<u8>> {
    let path = root.join("Resource").join("Common").join(pack);
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    fpk_entries(&data)
        .into_iter()
        .filter(|(n, ..)| n.to_ascii_lowercase().ends_with(".gr2"))
        .filter_map(|(n, off, size)| {
            Some((n.to_ascii_lowercase(), data.get(off..off + size)?.to_vec()))
        })
        .collect()
}

/// The cow (`MiscData.fpk`, names kept): the first material of the spec's example.
#[test]
fn cow_material_reads_fully() {
    let Some(root) = civ5_root() else { return };
    let files = pack_gr2(&root, "MiscData.fpk");
    let data = files.get("cow.gr2").expect("cow.gr2 not found");
    let file = GrannyFile::parse(data).unwrap();
    let mats = read_fgx_materials(&file).unwrap();
    let m = mats.iter().flatten().next().expect("cow has FGX materials");

    assert_eq!(m.shader_set.text.as_deref(), Some("BuildingShader_Skinned"));
    assert_eq!(m.version, 4);
    assert_eq!(m.alpha_mode, 8);
    assert_eq!(m.z_mode, 4);
    assert_eq!(m.alpha_ref, 128);
    assert_eq!(m.skin_bone_count, 32);
    assert_eq!(m.parameter_set_count, 2);

    let global = m.sets[0].as_ref().expect("ParameterSet0");
    assert_eq!(global.block.text.as_deref(), Some("BuildingGlobal"));
    assert_eq!(global.count, 9);
    let names: Vec<_> = global
        .names
        .iter()
        .filter_map(|n| n.text.as_deref())
        .collect();
    assert_eq!(names.len(), 9);
    assert!(names.contains(&"ViewInverseMatrix") && names.contains(&"f3FogColor"));
    assert!(names.contains(&"v3CameraWorld"), "{names:?}");

    let tex = m.sets[1].as_ref().expect("ParameterSet1");
    assert_eq!(tex.block.text.as_deref(), Some("BuildingTextures"));
    assert_eq!(tex.count, 10);
    let names: Vec<_> = tex.names.iter().filter_map(|n| n.text.as_deref()).collect();
    assert_eq!(
        names,
        [
            "Diffuse",
            "BuildingSREF",
            "Irradiance",
            "SpecSharp",
            "SpecSoft",
            "ColorCorrection",
            "DistanceFogRamp",
            "FOWColorKey",
            "FOWMask",
            "FOWBaseLayer"
        ]
    );
    // Types: 2D, 2D, then cubes, the 3D lookup volumes (10) among them.
    assert_eq!(&tex.types[..4], [6, 6, 7, 7]);
    assert_eq!(tex.types[5], 10, "ColorCorrection is a volume");
    assert_eq!(tex.types[7], 10, "FOWColorKey is a volume");
    // `ParamStoreIdx` addresses `ParamDataT` in steps of its 4 slots per texture. Paths the file
    // spells out come with the full source path; lookup volumes the engine fills have none
    // (index -4), and so does the fog ramp.
    assert_eq!(tex.store_idx, [0, 4, 8, 12, 16, -4, 20, 24, 28, 32]);
    let path = |i: usize| tex.data_t[tex.store_idx[i] as usize].text.as_deref();
    assert!(
        path(0).is_some_and(|p| p.ends_with("terrain\\Resources\\cow\\Resource_Cow01_DIFF.dds"))
    );
    assert!(path(1).is_some_and(|p| p.ends_with("Units\\Shared\\Textures\\Black_SREF.dds")));
    assert!(path(2).is_some_and(|p| p.ends_with("Unit_Irradiance.dds")));
    assert!(path(3).is_some_and(|p| p.ends_with("Unit_Environment_Sharp.dds")));
    assert!(path(4).is_some_and(|p| p.ends_with("Unit_Environment_Dull.dds")));
    assert_eq!(path(6), None, "DistanceFogRamp comes from the engine");
}
/// Every gr2 of `UnitModels.fpk` reads without error; stripped names are tallied.
#[test]
fn all_unit_gr2_read_without_errors() {
    let Some(root) = civ5_root() else { return };
    let files = pack_gr2(&root, "UnitModels.fpk");
    assert!(files.len() > 1000, "{} gr2 files", files.len());
    let (mut with_fgx, mut materials) = (0, 0);
    let mut shaders = BTreeSet::new();
    let mut unresolved = BTreeSet::new();
    let mut errors = Vec::new();
    for (name, data) in &files {
        let result = GrannyFile::parse(data).and_then(|f| read_fgx_materials(&f));
        match result {
            Ok(mats) => {
                with_fgx += usize::from(mats.iter().any(Option::is_some));
                for m in mats.iter().flatten() {
                    materials += 1;
                    shaders.extend(m.shader_set.display());
                    // Names only; texture paths are hashed full paths, resolved elsewhere.
                    let names = std::iter::once(&m.shader_set).chain(
                        m.present_sets()
                            .flat_map(|s| std::iter::once(&s.block).chain(&s.names)),
                    );
                    for s in names.filter(|s| s.is_unresolved()) {
                        unresolved.extend(s.display());
                    }
                }
            }
            Err(e) => errors.push(format!("{name}: {e}")),
        }
    }
    println!(
        "UnitModels: {} gr2, {with_fgx} with FGX, {materials} FGX materials, shader sets {shaders:?}",
        files.len()
    );
    println!("unresolved hashes: {}: {unresolved:?}", unresolved.len());
    assert!(
        errors.is_empty(),
        "{} errors, e.g. {:?}",
        errors.len(),
        &errors[..errors.len().min(5)]
    );
    assert!(with_fgx > 100 && materials > with_fgx);
}
