//! The shader catalog: blocks with CTAB, samplers, inputs and outputs
//! and features; entries (package, entry, stage) with their block; ShaderSets of FGX materials;
//! the report "which packages declare a sampler of this name".
//!
//! `catalog.json` (`schema` 1) lives in the cache (SR-D6), never in git. It is deterministic: the
//! same binaries and the same `status.toml` give the same bytes. Consumers: `assets-b2`
//! (`civ5-assets fgx-check --shader-catalog`, which takes every string of the file as a name),
//! `render-core`, `golden`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::bins::FoundBin;
use crate::names::{self, Names};
use crate::scan::{Block, Stage};
use crate::sm3::ctab::{Ctab, RegisterSet};
use crate::sm3::ir::{DclKind, Opcode, RegType, SamplerKind, Shader, Stmt};
use crate::status::StatusFile;

/// Incompatible changes of the file bump this.
pub const SCHEMA: u32 = 1;
/// Build the pinned binaries belong to.
pub const BUILD: &str = "1.0.3.279";

// research: the table of world packages — input slots per world package (160 in all).
pub const WORLD_PACKAGES: [(&str, usize); 22] = [
    ("TerrainShader", 24),
    ("WaterShader", 8),
    ("RiverShader", 2),
    ("WaveShader", 2),
    ("WorldCurrents", 5),
    ("IceShader", 2),
    ("ForestShader", 4),
    ("MinimapShader", 17),
    ("FOW", 18),
    ("Shadow", 3),
    ("OverlayShaders", 8),
    ("GridShader", 4),
    ("ArrowShader", 2),
    ("TradeArrowShader", 2),
    ("UnitShader", 16),
    ("BuildingShader", 11),
    ("LandmarkShader", 2),
    ("FXShader", 15),
    ("SimpleShader", 4),
    ("BumpFilter", 3),
    ("DXTCompress", 6),
    ("cvResolvePass", 2),
];
/// World inputs in all.
pub const WORLD_ENTRIES: usize = 160;

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error(transparent)]
    Names(#[from] names::NamesError),
    #[error("block {sha1}: {what}")]
    Block { sha1: String, what: String },
    #[error("{0}")]
    Inconsistent(String),
    #[error("{path}: {why}", path = .path.display())]
    Io {
        path: std::path::PathBuf,
        why: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Features {
    pub branches: bool,
    pub loops: bool,
    pub rel_addr: bool,
    pub texkill: bool,
    pub derivatives: bool,
    pub mrt: bool,
    pub depth_out: bool,
    pub vtf: bool,
    pub cube: bool,
    pub volume: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CtabEntry {
    pub name: String,
    /// `float4`, `int4`, `bool`, `sampler`.
    pub set: String,
    pub register: u16,
    pub count: u16,
    pub class: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub rows: u16,
    pub columns: u16,
    pub elements: u16,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SamplerInfo {
    pub name: String,
    pub register: u32,
    /// `2d`, `cube`, `volume`, `1d`.
    pub kind: String,
}

/// An input or output with its semantic.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Io {
    pub reg: String,
    pub semantic: String,
    pub index: u8,
    /// Components, `xyzw` subset.
    pub mask: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub offset: usize,
    pub addr: u64,
}

/// One distinct SM3 block (by sha1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockInfo {
    pub sha1: String,
    pub stage: Stage,
    pub profile: String,
    pub size: usize,
    /// Where it lies in `Civ5XP` (one per copy).
    pub linux: Vec<Location>,
    /// sha1 of the block at the same place in the DX9 exe; empty when the pairing failed.
    pub dx9_sha1: Vec<String>,
    /// Linux and DX9 blocks are byte-equal (`None` when there is no DX9 data).
    pub dx9_same: Option<bool>,
    pub instructions: usize,
    pub opcodes: BTreeMap<String, usize>,
    pub features: Features,
    /// A vertex shader reads `TANGENT0` or `BINORMAL0` (AB-D6).
    pub has_tangent: bool,
    pub ctab_target: String,
    pub ctab: Vec<CtabEntry>,
    pub samplers: Vec<SamplerInfo>,
    pub inputs: Vec<Io>,
    pub outputs: Vec<Io>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockRef {
    pub sha1: String,
    pub addr: u32,
    pub size: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// `UnitShader::UnitPS:PS`.
    pub name: String,
    pub package: String,
    pub entry: String,
    pub stage: String,
    /// In the table of world packages (the world, not the leaderhead, menus, strategic view).
    pub world: bool,
    /// Block SM3 of `Civ5XP`; `None` for DX11-only slots.
    pub sm3: Option<BlockRef>,
    /// The DX9 exe holds another block (UI, DOF, leaders, terrain offsets).
    pub dx9_sha1: Option<String>,
    /// DX11 twins by profile; filled by SR-8 (empty for now).
    pub dx11_twins: Vec<String>,
    /// `init-scan` (machine code of the registrar) or `dynsym` (slot name only, no SM3 block).
    pub name_source: String,
    /// `FireGrafix::g_UnitShader_UnitPS_PS`.
    pub id_global: String,
    /// `g_pdwUnitShader_UnitPS_PS_SM30` (the string of the registration call).
    pub code_name: Option<String>,
    /// `initFireGrafix_ShaderDefDX9` (`world`), the strategic-view registrar (`strategic`) or none.
    pub registrar: String,
    /// CTAB names of the block's constants (hint for UBO groups).
    pub const_use: Vec<String>,
    /// Calls in `golden` captures; unknown until `golden` parses them.
    pub calls: Option<u32>,
    /// `status.toml` for world entries.
    pub status: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Package {
    pub name: String,
    pub world: bool,
    /// Entry names (`Package::Entry:STAGE`).
    pub entries: Vec<String>,
    /// CTAB sampler names of all entries.
    pub samplers: Vec<String>,
    /// CTAB constant names of all entries.
    pub constants: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConstSet {
    /// `FireGrafix::g_pCameraParameters`.
    pub global: String,
    pub index: u32,
    pub byte_size: u32,
    /// The six further arguments of `InitializeGlobalCS`.
    pub args: Vec<u32>,
    pub world: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShaderSet {
    /// Id in the table of `dispatchShaderPackageGetID`.
    pub id: u32,
    /// Package of the entries (`None` when the name rule finds none).
    pub package: Option<String>,
    /// Entries by pass kind: `main`, `shadow`, `code`.
    pub passes: BTreeMap<String, Vec<String>>,
    /// `name-rule` until RW-9 (a) reads the package records.
    pub name_source: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    pub blocks: usize,
    pub unique_blocks: usize,
    pub entries: usize,
    pub world_entries: usize,
    pub entries_with_sm3: usize,
    pub unnamed_blocks: usize,
    pub shader_sets: usize,
    pub const_sets: usize,
    pub dx9_unique_differing: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Catalog {
    pub schema: u32,
    pub build: String,
    /// sha256 of the binaries the catalog was made from.
    pub binaries: BTreeMap<String, String>,
    pub counts: Counts,
    pub packages: Vec<Package>,
    pub blocks: Vec<BlockInfo>,
    pub entries: Vec<Entry>,
    /// Blocks no registration names.
    pub unnamed_blocks: Vec<String>,
    pub shader_sets: BTreeMap<String, ShaderSet>,
    pub const_sets: Vec<ConstSet>,
    /// CTAB sampler name → packages that declare it (who owns an engine texture).
    pub samplers_by_package: BTreeMap<String, Vec<String>>,
}

impl Catalog {
    pub fn load(path: &Path) -> Result<Catalog, CatalogError> {
        let io = |why: String| CatalogError::Io {
            path: path.to_path_buf(),
            why,
        };
        let text = std::fs::read_to_string(path).map_err(|e| io(e.to_string()))?;
        let c: Catalog = serde_json::from_str(&text).map_err(|e| io(e.to_string()))?;
        if c.schema != SCHEMA {
            return Err(io(format!(
                "schema {}, this build reads {SCHEMA}",
                c.schema
            )));
        }
        Ok(c)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("catalog serializes") + "\n"
    }

    /// `Package::Entry:STAGE`, or `Package::Entry` when only one stage has that name.
    pub fn find_entry(&self, name: &str) -> Option<&Entry> {
        if let Some(e) = self.entries.iter().find(|e| e.name == name) {
            return Some(e);
        }
        let mut hits = self
            .entries
            .iter()
            .filter(|e| e.name.rsplit_once(':').is_some_and(|(n, _)| n == name));
        let first = hits.next()?;
        hits.next().is_none().then_some(first)
    }

    pub fn block(&self, sha1: &str) -> Option<&BlockInfo> {
        self.blocks.iter().find(|b| b.sha1 == sha1)
    }

    /// Names of the world entries (the inputs of `status.toml`).
    pub fn world_entry_names(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|e| e.world)
            .map(|e| e.name.clone())
            .collect()
    }
}

/// One scanned binary with its blocks.
pub struct Scanned<'a> {
    pub bin: &'a FoundBin,
    pub data: &'a [u8],
    pub blocks: &'a [Block],
}

fn set_name(s: RegisterSet) -> &'static str {
    match s {
        RegisterSet::Bool => "bool",
        RegisterSet::Int4 => "int4",
        RegisterSet::Float4 => "float4",
        RegisterSet::Sampler => "sampler",
    }
}

fn kind_name(k: SamplerKind) -> &'static str {
    match k {
        SamplerKind::Unknown => "unknown",
        SamplerKind::Tex1d => "1d",
        SamplerKind::Tex2d => "2d",
        SamplerKind::Cube => "cube",
        SamplerKind::Volume => "volume",
    }
}

fn reg_name(ty: RegType, num: u32) -> String {
    match ty {
        RegType::Input => format!("v{num}"),
        RegType::Output => format!("o{num}"),
        RegType::ColorOut => format!("oC{num}"),
        RegType::DepthOut => "oDepth".to_string(),
        RegType::MiscType if num == 0 => "vPos".to_string(),
        RegType::MiscType => "vFace".to_string(),
        other => format!("{other:?}{num}"),
    }
}

fn mask_name(mask: u8) -> String {
    ["x", "y", "z", "w"]
        .iter()
        .enumerate()
        .filter(|(i, _)| mask & (1 << i) != 0)
        .map(|(_, c)| *c)
        .collect()
}

fn ctab_entries(ctab: Option<&Ctab>) -> Vec<CtabEntry> {
    ctab.map(|c| {
        c.constants
            .iter()
            .map(|k| CtabEntry {
                name: k.name.clone(),
                set: set_name(k.set).to_string(),
                register: k.reg,
                count: k.count,
                class: k.ty.class.name().to_string(),
                ty: k.ty.ty.name().to_string(),
                rows: k.ty.rows,
                columns: k.ty.columns,
                elements: k.ty.elements,
            })
            .collect()
    })
    .unwrap_or_default()
}

struct Analysis {
    instructions: usize,
    opcodes: BTreeMap<String, usize>,
    features: Features,
    has_tangent: bool,
    samplers: Vec<SamplerInfo>,
    inputs: Vec<Io>,
    outputs: Vec<Io>,
}

fn analyze(sh: &Shader) -> Analysis {
    let mut a = Analysis {
        instructions: 0,
        opcodes: BTreeMap::new(),
        features: Features::default(),
        has_tangent: false,
        samplers: Vec::new(),
        inputs: Vec::new(),
        outputs: Vec::new(),
    };
    let is_vs = sh.version.stage == Stage::Vs;
    let mut color_outs = BTreeSet::new();
    let mut ps_outs: BTreeMap<u32, u8> = BTreeMap::new();
    let mut fetches = false;
    for st in &sh.stmts {
        match st {
            Stmt::Dcl(d) => match d.kind {
                DclKind::Sampler(k) => {
                    let num = d.dst.reg.num;
                    let name = sh
                        .ctab
                        .as_ref()
                        .and_then(|c| c.find(RegisterSet::Sampler, num))
                        .map(|c| c.name.clone())
                        .unwrap_or_default();
                    match k {
                        SamplerKind::Cube => a.features.cube = true,
                        SamplerKind::Volume => a.features.volume = true,
                        _ => {}
                    }
                    a.samplers.push(SamplerInfo {
                        name,
                        register: num,
                        kind: kind_name(k).to_string(),
                    });
                }
                DclKind::Semantic { usage, index } => {
                    let io = Io {
                        reg: reg_name(d.dst.reg.ty, d.dst.reg.num),
                        semantic: usage.map_or("misc".to_string(), |u| u.name().to_string()),
                        index,
                        mask: mask_name(d.dst.mask),
                    };
                    match d.dst.reg.ty {
                        RegType::Output => a.outputs.push(io),
                        _ => {
                            if is_vs
                                && matches!(
                                    usage,
                                    Some(crate::sm3::ir::Usage::Tangent)
                                        | Some(crate::sm3::ir::Usage::Binormal)
                                )
                                && index == 0
                            {
                                a.has_tangent = true;
                            }
                            a.inputs.push(io);
                        }
                    }
                }
            },
            Stmt::Inst(i) => {
                a.instructions += 1;
                *a.opcodes.entry(i.op.name().to_string()).or_insert(0) += 1;
                match i.op {
                    Opcode::If | Opcode::Ifc | Opcode::Else => a.features.branches = true,
                    Opcode::Rep | Opcode::Loop => a.features.loops = true,
                    Opcode::TexKill => a.features.texkill = true,
                    Opcode::Dsx | Opcode::Dsy => a.features.derivatives = true,
                    Opcode::TexLd | Opcode::TexLdl | Opcode::TexLdd => fetches = true,
                    _ => {}
                }
                if i.dst.as_ref().is_some_and(|d| d.rel.is_some())
                    || i.src.iter().any(|s| s.rel.is_some())
                {
                    a.features.rel_addr = true;
                }
                if let Some(d) = &i.dst {
                    match d.reg.ty {
                        RegType::ColorOut => {
                            color_outs.insert(d.reg.num);
                            *ps_outs.entry(d.reg.num).or_insert(0) |= d.mask;
                        }
                        RegType::DepthOut => {
                            a.features.depth_out = true;
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    a.features.mrt = color_outs.len() > 1;
    a.features.vtf = is_vs && fetches;
    if !is_vs {
        for (n, mask) in ps_outs {
            a.outputs.push(Io {
                reg: reg_name(RegType::ColorOut, n),
                semantic: "color".to_string(),
                index: n as u8,
                mask: mask_name(mask),
            });
        }
        if a.features.depth_out {
            a.outputs.push(Io {
                reg: "oDepth".to_string(),
                semantic: "depth".to_string(),
                index: 0,
                mask: "x".to_string(),
            });
        }
    }
    a
}

fn block_error(sha1: &str, what: impl ToString) -> CatalogError {
    CatalogError::Block {
        sha1: sha1.to_string(),
        what: what.to_string(),
    }
}

/// Pairs the blocks of `Civ5XP` and the DX9 exe by position (same order in both files): sha1 of
/// the DX9 block for each Linux block index, or `None` when the stage sequences differ.
fn pair_dx9(linux: &[Block], dx9: &[Block]) -> Option<Vec<String>> {
    (linux.len() == dx9.len() && linux.iter().zip(dx9).all(|(l, w)| l.stage == w.stage))
        .then(|| dx9.iter().map(|b| b.sha1.clone()).collect())
}

pub fn stage_letters(s: Stage) -> &'static str {
    match s {
        Stage::Vs => "VS",
        Stage::Ps => "PS",
        Stage::Gs => "GS",
        Stage::Hs => "HS",
        Stage::Ds => "DS",
        Stage::Cs => "CS",
    }
}

/// Builds the catalog from the scanned Linux binary (names, blocks) and, when present, the DX9 exe.
pub fn build(
    linux: &Scanned,
    dx9: Option<&Scanned>,
    names: &Names,
    status: &StatusFile,
) -> Result<Catalog, CatalogError> {
    let slots = names::dynsym_slots(linux.data)?;
    let pairing = dx9.and_then(|w| pair_dx9(linux.blocks, w.blocks));
    if dx9.is_some() && pairing.is_none() {
        return Err(CatalogError::Inconsistent(
            "Civ5XP and the DX9 exe do not hold the same sequence of blocks".to_string(),
        ));
    }

    // Distinct blocks.
    let mut blocks: BTreeMap<String, BlockInfo> = BTreeMap::new();
    let mut by_addr: BTreeMap<u64, &Block> = BTreeMap::new();
    for (i, b) in linux.blocks.iter().enumerate() {
        by_addr.insert(b.addr, b);
        let bytes = &linux.data[b.offset..b.offset + b.size];
        let info = match blocks.get_mut(&b.sha1) {
            Some(info) => info,
            None => {
                let sh = crate::sm3::parse(bytes).map_err(|e| block_error(&b.sha1, e))?;
                let an = analyze(&sh);
                blocks.entry(b.sha1.clone()).or_insert(BlockInfo {
                    sha1: b.sha1.clone(),
                    stage: b.stage,
                    profile: b.profile.clone(),
                    size: b.size,
                    linux: Vec::new(),
                    dx9_sha1: Vec::new(),
                    dx9_same: None,
                    instructions: an.instructions,
                    opcodes: an.opcodes,
                    features: an.features,
                    has_tangent: an.has_tangent,
                    ctab_target: sh
                        .ctab
                        .as_ref()
                        .map(|c| c.target.clone())
                        .unwrap_or_default(),
                    ctab: ctab_entries(sh.ctab.as_ref()),
                    samplers: an.samplers,
                    inputs: an.inputs,
                    outputs: an.outputs,
                })
            }
        };
        info.linux.push(Location {
            offset: b.offset,
            addr: b.addr,
        });
        if let Some(p) = &pairing
            && !info.dx9_sha1.contains(&p[i])
        {
            info.dx9_sha1.push(p[i].clone());
        }
    }
    for info in blocks.values_mut() {
        info.dx9_sha1.sort();
        if dx9.is_some() {
            info.dx9_same = Some(info.dx9_sha1.len() == 1 && info.dx9_sha1[0] == info.sha1);
        }
    }

    // Registrations by id slot.
    let reg_by_id: BTreeMap<u32, &names::Registration> =
        names.registrations.iter().map(|r| (r.id_addr, r)).collect();
    let mut named: BTreeSet<String> = BTreeSet::new();
    let mut entries: Vec<Entry> = Vec::new();
    let world_pkgs: BTreeSet<&str> = WORLD_PACKAGES.iter().map(|(p, _)| *p).collect();
    for (addr, package, entry, stage) in &slots {
        let name = format!("{package}::{entry}:{stage}");
        let world = world_pkgs.contains(package.as_str());
        let reg = reg_by_id.get(addr);
        let (sm3, dx9_sha1, name_source, code_name, registrar, const_use) = match reg {
            Some(r) => {
                let b = by_addr.get(&u64::from(r.block_addr)).ok_or_else(|| {
                    CatalogError::Inconsistent(format!(
                        "{name}: no block at {:#010x}",
                        r.block_addr
                    ))
                })?;
                if b.size as u32 != r.block_size {
                    return Err(CatalogError::Inconsistent(format!(
                        "{name}: block size {} but the registration says {}",
                        b.size, r.block_size
                    )));
                }
                if stage_letters(b.stage) != stage {
                    return Err(CatalogError::Inconsistent(format!(
                        "{name}: slot stage {stage} but the block is {:?}",
                        b.stage
                    )));
                }
                named.insert(b.sha1.clone());
                let info = &blocks[&b.sha1];
                let dx9_sha1 = match info.dx9_same {
                    Some(false) if info.dx9_sha1.len() == 1 => Some(info.dx9_sha1[0].clone()),
                    _ => None,
                };
                let mut uses: Vec<String> = info
                    .ctab
                    .iter()
                    .filter(|c| c.set != "sampler")
                    .map(|c| c.name.clone())
                    .collect();
                uses.sort();
                uses.dedup();
                (
                    Some(BlockRef {
                        sha1: b.sha1.clone(),
                        addr: r.block_addr,
                        size: r.block_size,
                    }),
                    dx9_sha1,
                    "init-scan",
                    Some(r.code_name.clone()),
                    if r.in_world_init {
                        "world"
                    } else {
                        "strategic"
                    },
                    uses,
                )
            }
            None => (None, None, "dynsym", None, "none", Vec::new()),
        };
        let status = world.then(|| status.status_of(&name).name().to_string());
        entries.push(Entry {
            name,
            package: package.clone(),
            entry: entry.clone(),
            stage: stage.clone(),
            world,
            sm3,
            dx9_sha1,
            dx11_twins: Vec::new(),
            name_source: name_source.to_string(),
            id_global: format!("FireGrafix::g_{package}_{entry}_{stage}"),
            code_name,
            registrar: registrar.to_string(),
            const_use,
            calls: None,
            status,
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    for r in &names.registrations {
        if !slots.iter().any(|s| s.0 == r.id_addr) {
            return Err(CatalogError::Inconsistent(format!(
                "registration {} has no entry slot",
                r.id_global
            )));
        }
    }

    // World entries per package: the table of world packages.
    let mut world_total = 0;
    for (pkg, want) in WORLD_PACKAGES {
        let n = entries.iter().filter(|e| e.package == pkg).count();
        if n != want {
            return Err(CatalogError::Inconsistent(format!(
                "world package {pkg}: {n} entries, the table says {want}"
            )));
        }
        world_total += n;
    }
    if world_total != WORLD_ENTRIES {
        return Err(CatalogError::Inconsistent(format!(
            "{world_total} world entries, expected {WORLD_ENTRIES}"
        )));
    }

    // Packages.
    let mut pkg_map: BTreeMap<String, Package> = BTreeMap::new();
    for e in &entries {
        let p = pkg_map.entry(e.package.clone()).or_insert_with(|| Package {
            name: e.package.clone(),
            world: e.world,
            entries: Vec::new(),
            samplers: Vec::new(),
            constants: Vec::new(),
        });
        p.entries.push(e.name.clone());
        if let Some(b) = &e.sm3 {
            let info = &blocks[&b.sha1];
            p.samplers
                .extend(info.samplers.iter().map(|s| s.name.clone()));
            p.constants.extend(e.const_use.iter().cloned());
        }
    }
    let mut packages: Vec<Package> = pkg_map.into_values().collect();
    for p in &mut packages {
        p.entries.sort();
        p.samplers.sort();
        p.samplers.dedup();
        p.constants.sort();
        p.constants.dedup();
    }
    let mut samplers_by_package: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for p in &packages {
        for s in p.samplers.iter().filter(|s| !s.is_empty()) {
            samplers_by_package
                .entry(s.clone())
                .or_default()
                .push(p.name.clone());
        }
    }

    let shader_sets = shader_sets(&names.packages, &packages, &entries);

    let const_sets: Vec<ConstSet> = names
        .const_sets
        .iter()
        .map(|c| ConstSet {
            global: c.global.clone(),
            index: c.args[0],
            byte_size: c.args[1],
            args: c.args[2..].to_vec(),
            world: c.in_world_init,
        })
        .collect();

    let mut unnamed: Vec<String> = blocks
        .keys()
        .filter(|k| !named.contains(*k))
        .cloned()
        .collect();
    unnamed.sort();

    let mut binaries = BTreeMap::new();
    binaries.insert(linux.bin.id.name().to_string(), linux.bin.sha256.clone());
    if let Some(w) = dx9 {
        binaries.insert(w.bin.id.name().to_string(), w.bin.sha256.clone());
    }
    let counts = Counts {
        blocks: linux.blocks.len(),
        unique_blocks: blocks.len(),
        entries: entries.len(),
        world_entries: entries.iter().filter(|e| e.world).count(),
        entries_with_sm3: entries.iter().filter(|e| e.sm3.is_some()).count(),
        unnamed_blocks: unnamed.len(),
        shader_sets: shader_sets.len(),
        const_sets: const_sets.len(),
        dx9_unique_differing: blocks
            .values()
            .filter(|b| b.dx9_same == Some(false))
            .count(),
    };
    Ok(Catalog {
        schema: SCHEMA,
        build: BUILD.to_string(),
        binaries,
        counts,
        packages,
        blocks: blocks.into_values().collect(),
        entries,
        unnamed_blocks: unnamed,
        shader_sets,
        const_sets,
        samplers_by_package,
    })
}

// ------------------------------------------------------------------ ShaderSets

/// Leading names of sets that are not a prefix of any package name (name rule).
const SET_PREFIX_ALIASES: [(&str, &str); 2] =
    [("Leader", "Civ5Leaderhead"), ("Fork", "ForkShaders")];

/// Package of a ShaderSet by name: the longest package name that is a prefix of the set name (or
/// of which the set name is a prefix, when unique), else a fixed alias.
fn set_package<'a>(set: &str, packages: &'a [Package]) -> Option<&'a Package> {
    let s = set.to_lowercase();
    let longest = packages
        .iter()
        .filter(|p| s.starts_with(&p.name.to_lowercase()))
        .max_by_key(|p| p.name.len());
    if longest.is_some() {
        return longest;
    }
    let reverse: Vec<&Package> = packages
        .iter()
        .filter(|p| p.name.to_lowercase().starts_with(&s))
        .collect();
    if let [one] = reverse.as_slice() {
        return Some(one);
    }
    SET_PREFIX_ALIASES
        .iter()
        .find(|(prefix, _)| set.starts_with(prefix))
        .and_then(|(_, pkg)| packages.iter().find(|p| p.name == *pkg))
}

/// Entries of `pkg` that a ShaderSet named `set` uses, by pass kind (name rule: suffixes
/// `_Skinned`, `_Alpha`, `_Instanced`, `_CODE`, `_Shadow`, `_Stencil` against the entry names).
fn set_passes(set: &str, pkg: &Package, entries: &[Entry]) -> BTreeMap<String, Vec<String>> {
    let s = set.to_lowercase();
    let (skinned, alpha, code, shadow, stencil, instanced) = (
        s.contains("skinned"),
        s.contains("alpha"),
        s.contains("code"),
        s.contains("shadow"),
        s.contains("stencil"),
        s.contains("instanced"),
    );
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for e in entries
        .iter()
        .filter(|e| e.package == pkg.name && e.sm3.is_some())
    {
        let n = e.entry.to_lowercase();
        let e_shadow = n.contains("shadow");
        let e_code = n.contains("code");
        if n.contains("skinned") != skinned
            || n.contains("alpha") != alpha
            || n.contains("instanced") != instanced
            || e_code != code
            || n.contains("stencil") != stencil
            || (shadow && !e_shadow)
        {
            continue;
        }
        let pass = if e_shadow {
            "shadow"
        } else if e_code {
            "code"
        } else {
            "main"
        };
        out.entry(pass.to_string())
            .or_default()
            .push(e.name.clone());
    }
    out
}

fn shader_sets(
    set_names: &[String],
    packages: &[Package],
    entries: &[Entry],
) -> BTreeMap<String, ShaderSet> {
    set_names
        .iter()
        .enumerate()
        .map(|(id, set)| {
            let pkg = set_package(set, packages);
            let passes = pkg.map(|p| set_passes(set, p, entries)).unwrap_or_default();
            (
                set.clone(),
                ShaderSet {
                    id: id as u32,
                    package: pkg.map(|p| p.name.clone()),
                    passes,
                    name_source: "name-rule".to_string(),
                },
            )
        })
        .collect()
}

/// Writes `<cache>/<sha256-12 of Civ5XP>/catalog.json` and the stable copy `<cache>/catalog.json`.
pub fn write(
    cache_root: &Path,
    linux: &FoundBin,
    catalog: &Catalog,
) -> Result<(std::path::PathBuf, std::path::PathBuf), CatalogError> {
    let json = catalog.to_json();
    let dir = cache_root.join(linux.key());
    let io = |p: &Path, e: std::io::Error| CatalogError::Io {
        path: p.to_path_buf(),
        why: e.to_string(),
    };
    std::fs::create_dir_all(&dir).map_err(|e| io(&dir, e))?;
    let per_bin = dir.join("catalog.json");
    std::fs::write(&per_bin, &json).map_err(|e| io(&per_bin, e))?;
    let stable = cache_root.join("catalog.json");
    std::fs::write(&stable, &json).map_err(|e| io(&stable, e))?;
    Ok((per_bin, stable))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pkg(name: &str) -> Package {
        Package {
            name: name.to_string(),
            world: true,
            entries: Vec::new(),
            samplers: Vec::new(),
            constants: Vec::new(),
        }
    }

    fn entry(package: &str, entry: &str, stage: &str) -> Entry {
        Entry {
            name: format!("{package}::{entry}:{stage}"),
            package: package.to_string(),
            entry: entry.to_string(),
            stage: stage.to_string(),
            world: true,
            sm3: Some(BlockRef {
                sha1: "0".repeat(40),
                addr: 0,
                size: 0,
            }),
            dx9_sha1: None,
            dx11_twins: Vec::new(),
            name_source: "init-scan".into(),
            id_global: String::new(),
            code_name: None,
            registrar: "world".into(),
            const_use: Vec::new(),
            calls: None,
            status: None,
        }
    }

    #[test]
    fn set_names_find_their_package() {
        let pk = vec![
            pkg("UnitShader"),
            pkg("MinimapShader"),
            pkg("Shadow"),
            pkg("MenuShaders"),
            pkg("Civ5Leaderhead"),
            pkg("LeaderResolve"),
            pkg("ForkShaders"),
        ];
        let f = |s: &str| set_package(s, &pk).map(|p| p.name.as_str());
        assert_eq!(f("UnitShader_Skinned"), Some("UnitShader"));
        assert_eq!(f("MiniMapShader"), Some("MinimapShader"));
        assert_eq!(f("ShadowShader"), Some("Shadow"));
        assert_eq!(f("MenuShader"), Some("MenuShaders"));
        assert_eq!(f("Leader_Skin"), Some("Civ5Leaderhead"));
        assert_eq!(f("LeaderResolveShader"), Some("LeaderResolve"));
        assert_eq!(f("ForkGlowShader"), Some("ForkShaders"));
        assert_eq!(f("CompressBC"), None);
    }

    #[test]
    fn set_suffixes_pick_entries_by_pass() {
        let es = vec![
            entry("UnitShader", "UnitVS", "VS"),
            entry("UnitShader", "UnitPS", "PS"),
            entry("UnitShader", "UnitVS_Shadow", "VS"),
            entry("UnitShader", "UnitSkinnedVS", "VS"),
            entry("UnitShader", "UnitPS_CODE", "PS"),
            entry("UnitShader", "UnitPS_CODE_Shadow", "PS"),
            entry("UnitShader", "UnitAlphaPS", "PS"),
        ];
        let p = pkg("UnitShader");
        let plain = set_passes("UnitShader", &p, &es);
        assert_eq!(
            plain["main"],
            ["UnitShader::UnitVS:VS", "UnitShader::UnitPS:PS"]
        );
        assert_eq!(plain["shadow"], ["UnitShader::UnitVS_Shadow:VS"]);
        assert!(!plain.contains_key("code"));
        let skinned = set_passes("UnitShader_Skinned", &p, &es);
        assert_eq!(skinned["main"], ["UnitShader::UnitSkinnedVS:VS"]);
        let code = set_passes("UnitShader_CODE", &p, &es);
        assert_eq!(code["code"], ["UnitShader::UnitPS_CODE:PS"]);
        assert_eq!(code["shadow"], ["UnitShader::UnitPS_CODE_Shadow:PS"]);
        let alpha = set_passes("UnitShader_Alpha", &p, &es);
        assert_eq!(alpha["main"], ["UnitShader::UnitAlphaPS:PS"]);
    }

    #[test]
    fn entries_are_found_by_name_with_or_without_stage() {
        let mut c = Catalog {
            schema: SCHEMA,
            build: BUILD.into(),
            binaries: BTreeMap::new(),
            counts: Counts {
                blocks: 0,
                unique_blocks: 0,
                entries: 0,
                world_entries: 0,
                entries_with_sm3: 0,
                unnamed_blocks: 0,
                shader_sets: 0,
                const_sets: 0,
                dx9_unique_differing: 0,
            },
            packages: Vec::new(),
            blocks: Vec::new(),
            entries: vec![
                entry("A", "x", "VS"),
                entry("A", "y", "PS"),
                entry("A", "y", "VS"),
            ],
            unnamed_blocks: Vec::new(),
            shader_sets: BTreeMap::new(),
            const_sets: Vec::new(),
            samplers_by_package: BTreeMap::new(),
        };
        assert!(c.find_entry("A::x:VS").is_some());
        assert!(c.find_entry("A::x").is_some());
        // `A::y` names two stages: ambiguous.
        assert!(c.find_entry("A::y").is_none());
        assert!(c.find_entry("A::y:PS").is_some());
        // Round trip.
        let back: Catalog = serde_json::from_str(&c.to_json()).unwrap();
        assert_eq!(back, c);
        c.schema = 99;
        let dir = std::env::temp_dir().join(format!("shader-re-cat-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("c.json"), c.to_json()).unwrap();
        assert!(Catalog::load(&dir.join("c.json")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
