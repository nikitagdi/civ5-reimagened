//! Tests against the pinned original binaries.
//! Skipped with a reason when the binaries are not found (`CIV5_BIN_DIR`, `CIV5_ROOT`, see `bins.rs`).

use shader_re::bins::{self, BinError, BinId, Search};
use shader_re::scan::{self, Stage};

/// The binary and its bytes, or `None` (with the reason on stderr) when it is not available.
fn load(id: BinId) -> Option<(bins::FoundBin, Vec<u8>)> {
    match bins::find(id, &Search::from_env()) {
        Ok(bin) => {
            let data = bin.read().expect("read binary");
            Some((bin, data))
        }
        Err(e @ BinError::NotFound { .. }) => {
            eprintln!("SKIP {id}: {e}");
            None
        }
        Err(e) => panic!("{e}"),
    }
}

fn check_sm3(id: BinId) {
    let Some((_, data)) = load(id) else { return };
    let blocks = scan::extract(id, &data).expect("extract");
    // research: approach §1.1 — 433 blocks, 121 vs_3_0 / 312 ps_3_0, 401 unique.
    assert_eq!(blocks.len(), scan::SM3_TOTAL);
    assert_eq!(
        blocks.iter().filter(|b| b.stage == Stage::Vs).count(),
        scan::SM3_VS
    );
    assert_eq!(
        blocks.iter().filter(|b| b.stage == Stage::Ps).count(),
        scan::SM3_PS
    );
    assert_eq!(scan::unique_count(&blocks), scan::SM3_UNIQUE);
    scan::verify_pins(id, &blocks).expect("pinned counts");
    assert!(
        blocks
            .iter()
            .all(|b| b.profile == "vs_3_0" || b.profile == "ps_3_0")
    );
}

#[test]
fn linux_has_433_sm3_blocks() {
    check_sm3(BinId::Linux);
}

#[test]
fn dx9_has_433_sm3_blocks() {
    check_sm3(BinId::Dx9);
}

#[test]
fn linux_and_dx9_share_382_of_401_blocks() {
    let (Some((_, linux)), Some((_, dx9))) = (load(BinId::Linux), load(BinId::Dx9)) else {
        return;
    };
    let l = scan::extract(BinId::Linux, &linux).unwrap();
    let w = scan::extract(BinId::Dx9, &dx9).unwrap();
    let lset: std::collections::BTreeSet<&str> = l.iter().map(|b| b.sha1.as_str()).collect();
    let wset: std::collections::BTreeSet<&str> = w.iter().map(|b| b.sha1.as_str()).collect();
    // research: approach §1.1 — 382 of 401 blocks are byte-equal.
    assert_eq!(lset.intersection(&wset).count(), 382);
}

#[test]
fn dx11_has_1417_dxbc_blocks_by_profile() {
    let Some((_, data)) = load(BinId::Dx11) else {
        return;
    };
    let blocks = scan::extract(BinId::Dx11, &data).expect("extract");
    assert_eq!(blocks.len(), scan::DXBC_TOTAL);
    let counts = scan::profile_counts(&blocks);
    for (profile, n) in scan::DXBC_PROFILES {
        assert_eq!(counts.get(profile).copied(), Some(n), "{profile}");
    }
    assert_eq!(scan::unique_count(&blocks), scan::DXBC_UNIQUE);
    scan::verify_pins(BinId::Dx11, &blocks).expect("pinned counts");
}

/// R1.9a: every DXBC container of the DX11 exe is read: chunks, signatures, version, declarations.
#[test]
fn all_dx11_containers_are_parsed() {
    use shader_re::dxbc::Dxbc;
    let Some((_, data)) = load(BinId::Dx11) else {
        return;
    };
    let blocks = scan::extract(BinId::Dx11, &data).unwrap();
    let mut by_profile = std::collections::BTreeMap::<String, usize>::new();
    for b in &blocks {
        let d = Dxbc::parse(&data[b.offset..b.offset + b.size])
            .unwrap_or_else(|e| panic!("{} ({}): {e}", b.sha1, b.profile));
        // The scanner and the container reader agree.
        assert_eq!(d.profile(), b.profile, "{}", b.sha1);
        assert_eq!(d.size, b.size, "{}", b.sha1);
        assert!(d.decls().instructions > 0, "{}", b.sha1);
        // Every stage but compute has an input and an output signature; only hull and domain
        // shaders have patch constants.
        assert_eq!(
            d.pcsg.is_some(),
            matches!(b.stage, scan::Stage::Hs | scan::Stage::Ds),
            "{}",
            b.sha1
        );
        *by_profile.entry(b.profile.clone()).or_default() += 1;
    }
    assert_eq!(by_profile.values().sum::<usize>(), scan::DXBC_TOTAL);
}

/// The declarations read from the tokens agree with the declarations in the text of
/// `D3DDisassemble` (Windows; `python` must find `d3dcompiler_47.dll`, else skipped).
#[test]
fn dx11_declarations_agree_with_the_disassembly() {
    use shader_re::dxbc::{Dxbc, asm_text};
    if !cfg!(windows) {
        eprintln!("SKIP: D3DDisassemble needs Windows");
        return;
    }
    let Some((bin, data)) = load(BinId::Dx11) else {
        return;
    };
    let blocks = scan::extract(BinId::Dx11, &data).unwrap();
    let cache_root = bins::cache_root(None).expect("cache root");
    let key = bins::bin_cache_dir(&cache_root, &bin);
    scan::write_cache(&key, &bin, &data, &blocks).unwrap();
    let dx11 = key.join("dx11");
    let mut sha1s: Vec<String> = blocks.iter().map(|b| b.sha1.clone()).collect();
    sha1s.sort();
    sha1s.dedup();
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("py/d3d_disasm.py");
    let python = std::env::var("PYTHON").unwrap_or_else(|_| "python".to_string());
    if let Err(e) = asm_text::ensure_listings(&python, &script, &dx11, &key.join("blocks"), &sha1s)
    {
        eprintln!("SKIP: no DX11 listings: {e}");
        return;
    }
    for b in &blocks {
        let d = Dxbc::parse(&data[b.offset..b.offset + b.size]).unwrap();
        let l = asm_text::read_listing(&dx11, &b.sha1).unwrap();
        assert_eq!(l.profile, b.profile, "{}", b.sha1);
        let x = d.decls();
        let c = &l.decl_counts;
        assert_eq!(c.constant_buffers, x.cbuffers.len(), "{} cb", b.sha1);
        assert_eq!(c.samplers, x.samplers.len(), "{} samplers", b.sha1);
        // `dcl_resource_raw` and `dcl_resource_structured` (opcodes 161, 162) are in `other_decls`.
        let raw = x
            .other_decls
            .iter()
            .filter(|o| matches!(**o, 161 | 162))
            .count();
        assert_eq!(c.resources, x.resources.len() + raw, "{} resources", b.sha1);
        assert_eq!(c.inputs, x.inputs.len(), "{} inputs", b.sha1);
        assert_eq!(c.outputs, x.outputs.len(), "{} outputs", b.sha1);
        // Instruction lines of the text and instructions of the tokens: `customdata` and the
        // phase markers are not instructions in the text either.
        let fetches = l.insts.iter().filter(|i| i.fetch.is_some()).count();
        assert!(fetches >= x.fetches.saturating_sub(1), "{} fetches", b.sha1);
    }
}

/// Blocks and containers of the DX11 exe, the twins of the catalog entries (R1.9b), and the
/// folder of the DX11 listings when they could be made (Windows).
struct Twins {
    cat: shader_re::catalog::Catalog,
    report: shader_re::dxbc::twins::TwinReport,
    listings: Option<std::path::PathBuf>,
}

fn twins() -> Option<Twins> {
    use shader_re::dxbc::{Dxbc, asm_text, twins};
    let (cat, _) = build_catalog()?;
    let (bin, data) = load(BinId::Dx11)?;
    let blocks = scan::extract(BinId::Dx11, &data).unwrap();
    let parsed: Vec<Dxbc> = blocks
        .iter()
        .map(|b| Dxbc::parse(&data[b.offset..b.offset + b.size]).unwrap())
        .collect();
    let report = twins::compute(&cat, &blocks, &parsed);
    let listings = if cfg!(windows) {
        let cache_root = bins::cache_root(None).expect("cache root");
        let key = bins::bin_cache_dir(&cache_root, &bin);
        scan::write_cache(&key, &bin, &data, &blocks).unwrap();
        let dx11 = key.join("dx11");
        let mut sha1s: Vec<String> = blocks.iter().map(|b| b.sha1.clone()).collect();
        sha1s.sort();
        sha1s.dedup();
        let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("py/d3d_disasm.py");
        let python = std::env::var("PYTHON").unwrap_or_else(|_| "python".to_string());
        match asm_text::ensure_listings(&python, &script, &dx11, &key.join("blocks"), &sha1s) {
            Ok(()) => Some(dx11),
            Err(e) => {
                eprintln!("no DX11 listings: {e}");
                None
            }
        }
    } else {
        None
    };
    Some(Twins {
        cat,
        report,
        listings,
    })
}

/// R1.9b: every entry with an SM3 block has a DX11 twin of the same stage.
#[test]
fn every_sm3_entry_has_a_dx11_twin() {
    use shader_re::dxbc::twins::Quality;
    let Some(t) = twins() else { return };
    assert_eq!(t.report.groups, 493);
    assert_eq!(t.report.twins.len(), scan::SM3_TOTAL);
    let mut world = 0;
    for e in t.cat.entries.iter().filter(|e| e.sm3.is_some()) {
        let tw = t
            .report
            .twin_of(&e.name)
            .unwrap_or_else(|| panic!("no twin for {}", e.name));
        assert_eq!(tw.sm3_sha1, e.sm3.as_ref().unwrap().sha1);
        // All profiles of the group have the signatures of the first one.
        assert!(tw.profiles_agree, "{}", e.name);
        assert!(tw.blocks.contains_key("5_0"), "{}", e.name);
        if e.world {
            world += 1;
            assert_eq!(tw.quality, Quality::Strong, "{}", e.name);
            assert!(!tw.ambiguous, "{}", e.name);
        }
    }
    // The 160 world entries minus the 11 that have no SM3 block.
    assert_eq!(world, 149);
    // Twins keep the order of the file: one group per SM3 block, growing.
    let mut groups: Vec<(u64, usize)> = t
        .cat
        .entries
        .iter()
        .filter_map(|e| {
            Some((
                u64::from(e.sm3.as_ref()?.addr),
                t.report.twin_of(&e.name)?.group,
            ))
        })
        .collect();
    groups.sort();
    assert!(groups.windows(2).all(|w| w[0].1 < w[1].1));
}

/// The entries of the world without an SM3 block: the three DX11-only ones that the groups in
/// between name, and the ones no group fits.
#[test]
fn dx11_only_entries_get_candidates() {
    let Some(t) = twins() else { return };
    let unique = |name: &str| {
        let c = t
            .report
            .candidate_of(name)
            .unwrap_or_else(|| panic!("no candidate entry for {name}"));
        assert_eq!(c.groups.len(), 1, "{name}: {:?}", c.groups);
        assert!(c.blocks.contains_key("5_0"));
    };
    unique("TerrainShader::TerrainShaderScreenVS40:VS");
    unique("TerrainShader::TerrainSpecShaderScreenPS40:PS");
    unique("Shadow::BlurAndProcess_DX11:PS");
    unique("BuildingShader::BuildingPS_CODE_Opaque:PS");
    // Two groups sit between the instanced building shaders: not decidable by position.
    let inst = t
        .report
        .candidate_of("BuildingShader::BuildingVS_Instanced:VS")
        .unwrap();
    assert_eq!(inst.groups.len(), 2);
    let missing = t
        .cat
        .entries
        .iter()
        .filter(|e| e.world && e.sm3.is_none())
        .count();
    assert_eq!(t.report.candidates.len(), missing);
    assert_eq!(missing, 11);
}

/// R1.9b: the constants of the milestone M1 entries are all placed, and the placed profiles agree.
#[test]
fn m1_constants_are_mapped() {
    use shader_re::dxbc::{asm_text, cbmap};
    let Some(t) = twins() else { return };
    let Some(dx11) = &t.listings else {
        eprintln!("SKIP: no DX11 listings");
        return;
    };
    let Some((_, data)) = load(BinId::Linux) else {
        return;
    };
    let m1 = [
        "UnitShader::UnitVS:VS",
        "UnitShader::UnitPS:PS",
        "UnitShader::UnitAlphaPS:PS",
        "TerrainShader::TerrainRenderCachedDiffusePS:PS",
        "TerrainShader::TerrainRenderCachedDiffuseAndSpecPS:PS",
        "TerrainShader::TerrainShaderScreenPS:PS",
    ];
    let mut maps = std::collections::BTreeMap::new();
    for name in m1 {
        let e = t.cat.find_entry(name).unwrap();
        let b = e.sm3.as_ref().unwrap();
        let info = t.cat.block(&b.sha1).unwrap();
        let at = info.linux[0].offset;
        let sh = shader_re::sm3::parse(&data[at..at + info.size]).unwrap();
        let tw = t.report.twin_of(name).unwrap();
        let mut by_profile = Vec::new();
        for p in ["5_0", "4_1", "4_0"] {
            let sha = &tw.blocks[p];
            let l = asm_text::read_listing(dx11, sha).unwrap();
            by_profile.push(cbmap::cb_map(name, &b.sha1, sha, &sh, &l));
        }
        let m = &by_profile[0];
        assert!(m.unmapped.is_empty(), "{name}: {:?}", m.unmapped);
        assert!(m.suspect.is_empty(), "{name}: {:?}", m.suspect);
        // The three profiles say the same.
        for other in &by_profile[1..] {
            assert_eq!(
                m.consts
                    .iter()
                    .map(|c| (&c.name, c.cb, c.offset, c.rows))
                    .collect::<Vec<_>>(),
                other
                    .consts
                    .iter()
                    .map(|c| (&c.name, c.cb, c.offset, c.rows))
                    .collect::<Vec<_>>(),
                "{name}"
            );
        }
        maps.insert(name, by_profile.swap_remove(0));
    }
    // Places checked by hand against the listings of the DX11 twins.
    let at = |entry: &str, c: &str| {
        let k = maps[entry].get(c).unwrap_or_else(|| panic!("{entry}: {c}"));
        (k.cb, k.offset, k.rows)
    };
    let unit_ps = "UnitShader::UnitPS:PS";
    assert_eq!(at(unit_ps, "TeamColorR"), (0, 48, 1));
    assert_eq!(at(unit_ps, "g_FogConsts"), (1, 144, 1));
    assert_eq!(at(unit_ps, "f3FogColor"), (1, 160, 1));
    assert_eq!(at(unit_ps, "f3CameraWorld"), (1, 176, 1));
    let unit_vs = "UnitShader::UnitVS:VS";
    assert_eq!(at(unit_vs, "WorldMatrix"), (0, 0, 3));
    assert_eq!(at(unit_vs, "ViewInverseMatrix"), (1, 0, 4));
    assert_eq!(at(unit_vs, "ViewProjMatrix"), (1, 64, 4));
    let terrain = "TerrainShader::TerrainRenderCachedDiffuseAndSpecPS:PS";
    assert_eq!(at(terrain, "f3FogColor"), (2, 0, 1));
    assert_eq!(at(terrain, "g_FogConsts"), (2, 16, 1));
    assert_eq!(at(terrain, "g_ShadowVP"), (1, 31 * 16, 4));
    assert_eq!(at(terrain, "g_bApplySpecular"), (1, 27 * 16, 1));
    assert_eq!(at(terrain, "g_bApplyDiffuse"), (1, 28 * 16, 1));
}

#[test]
fn another_count_is_an_extraction_error() {
    let Some((_, data)) = load(BinId::Linux) else {
        return;
    };
    let mut blocks = scan::extract(BinId::Linux, &data).unwrap();
    blocks.pop();
    assert!(scan::verify_pins(BinId::Linux, &blocks).is_err());
}

/// Catalog built from Civ5XP and the DX9 exe, or `None` (skipped) without the binaries.
fn build_catalog() -> Option<(shader_re::catalog::Catalog, shader_re::names::Names)> {
    use shader_re::catalog::{self, Scanned};
    let (lbin, ldata) = load(BinId::Linux)?;
    let (wbin, wdata) = load(BinId::Dx9)?;
    let lblocks = scan::extract(BinId::Linux, &ldata).unwrap();
    let wblocks = scan::extract(BinId::Dx9, &wdata).unwrap();
    let names = shader_re::names::extract(&ldata).expect("names");
    let cat = catalog::build(
        &Scanned {
            bin: &lbin,
            data: &ldata,
            blocks: &lblocks,
        },
        Some(&Scanned {
            bin: &wbin,
            data: &wdata,
            blocks: &wblocks,
        }),
        &names,
        &shader_re::status::StatusFile::default(),
    )
    .expect("catalog");
    Some((cat, names))
}

#[test]
fn registration_scan_has_the_pinned_counts() {
    use shader_re::names;
    let Some((cat, n)) = build_catalog() else {
        return;
    };
    // research: 278 / 109 / 43 calls in initFireGrafix_ShaderDefDX9.
    let world = |s| {
        n.registrations
            .iter()
            .filter(|r| r.in_world_init && r.stage == s)
            .count()
    };
    assert_eq!(world(Stage::Ps), names::PINNED_PS);
    assert_eq!(world(Stage::Vs), names::PINNED_VS);
    assert_eq!(
        n.const_sets.iter().filter(|c| c.in_world_init).count(),
        names::PINNED_CS
    );
    assert_eq!(n.packages.len(), names::PACKAGE_COUNT);
    // Every one of the 433 blocks has a name.
    assert_eq!(cat.counts.unnamed_blocks, 0);
    assert_eq!(cat.counts.world_entries, 160);
    assert_eq!(cat.counts.dx9_unique_differing, 19);
}

#[test]
fn catalog_is_deterministic() {
    let (Some((a, _)), Some((b, _))) = (build_catalog(), build_catalog()) else {
        return;
    };
    assert_eq!(a.to_json(), b.to_json());
}

#[test]
fn registrations_match_the_civ5_re_table() {
    use shader_re::names;
    let Some((_, n)) = build_catalog() else {
        return;
    };
    let Some(dir) = std::env::var_os("CIV5_RE_DIR") else {
        eprintln!("SKIP: CIV5_RE_DIR (folder of a reference shaderdef.tsv) is not set");
        return;
    };
    let path = std::path::Path::new(&dir).join("shaderdef.tsv");
    let Ok(tsv) = std::fs::read_to_string(&path) else {
        eprintln!("SKIP: {} not found", path.display());
        return;
    };
    names::compare_reference(&n, &tsv, &path.to_string_lossy()).expect("shaderdef.tsv");
}

#[test]
fn elf_dynsym_has_the_shader_slots() {
    let Some((_, data)) = load(BinId::Linux) else {
        return;
    };
    let elf = shader_re::elf::Elf::parse(&data).unwrap();
    let syms = elf.dynamic_symbols().unwrap();
    // research: 658 `FireGrafix::g_*` names in .dynsym.
    let n = syms
        .iter()
        .filter(|s| s.name.contains("FireGrafix") && s.name.contains("g_"))
        .count();
    assert!(n >= 600, "{n} FireGrafix globals");
}

/// SR criterion 3: every world entry that has an SM3 block is translated by the oracle,
/// and the WGSL passes naga, the GLSL ES 3.00 translation and the WebGL2 limits.
#[test]
fn world_entries_translate_and_validate() {
    let Some((cat, _)) = build_catalog() else {
        return;
    };
    let Some((_, data)) = load(BinId::Linux) else {
        return;
    };
    let mut done = 0;
    let mut failures = Vec::new();
    for e in cat.entries.iter().filter(|e| e.world) {
        let Some(b) = &e.sm3 else { continue };
        let info = cat.block(&b.sha1).expect("block of the entry");
        let at = info.linux[0].offset;
        let sh = shader_re::sm3::parse(&data[at..at + info.size]).expect("parse");
        match shader_re::oracle::translate(
            &sh,
            &e.name,
            &b.sha1,
            &shader_re::oracle::Options::default(),
        ) {
            Err(err) => failures.push(err.to_string()),
            Ok(o) => {
                let r = shader_re::validate::validate_wgsl(&e.name, &o.wgsl);
                if !r.ok() {
                    failures.push(r.to_string());
                }
                done += 1;
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    // 160 world entries, 11 of them without an SM3 block (6 DXTCompress, 3 DX11-only, 2 unnamed).
    assert_eq!(done, 149);
}

/// The harness can run every world entry that has a block: textures of every kind, the device
/// limits, the register file. The oracle against itself; a port is compared the same way.
#[test]
fn world_entries_run_in_the_harness() {
    use shader_re::harness::compare::{CompareOptions, compare_oracle, oracle_for};
    use shader_re::harness::formats::Formats;
    use shader_re::harness::subject::Subject;
    let Some((cat, _)) = build_catalog() else {
        return;
    };
    let Some((_, data)) = load(BinId::Linux) else {
        return;
    };
    let gpu = match shader_re::harness::Gpu::new() {
        Ok(g) => g,
        Err(e) => {
            eprintln!("SKIP: no GPU adapter: {e}");
            return;
        }
    };
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("maps/formats.toml");
    let formats = Formats::load(&path).expect("formats.toml");
    let opts = CompareOptions::new(&formats);
    let mut failures = Vec::new();
    let mut n = 0;
    for e in cat.entries.iter().filter(|e| e.world) {
        let Some(b) = &e.sm3 else { continue };
        let info = cat.block(&b.sha1).expect("block of the entry");
        let at = info.linux[0].offset;
        let sh = shader_re::sm3::parse(&data[at..at + info.size]).expect("parse");
        let result = oracle_for(&sh, &e.name, &b.sha1, &formats).and_then(|o| {
            let s = Subject::from_oracle(&o)?;
            compare_oracle(&gpu, &o, &s, &opts)
        });
        match result {
            Ok(r) if r.pass() => n += 1,
            Ok(r) => failures.push(r.summary()),
            Err(err) => failures.push(format!("{}: {err}", e.name)),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert_eq!(n, 149);
}
