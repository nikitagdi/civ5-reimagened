//! Names of the shader entries: the registration code of `Civ5XP` read as bytes (SR-D7).
//!
//! `initFireGrafix_ShaderDefDX9` (and its strategic-view sibling) is generated code: one block per
//! shader, i386 position-independent. The template RW-1 found (`civ5-re` `rw-w0-answers.txt`, RW-1):
//!
//! ```text
//! lea  ecx,[ebx+<bytecode>]     ; mov [esp+8],ecx    the block in .rodata
//! lea  ecx,[ebx+<name>]         ; mov [esp+4],ecx    "g_pdw<Package>_<Entry>_<Stage>_SM30"
//! mov  [esp],eax                                      the renderer
//! mov  dword [esp+0x10],<arg4>                        vertex shaders only
//! mov  dword [esp+0x0c],<size>                        size of the block
//! call CreatePixelShaderWithName / CreateVertexShaderWithName
//! lea  edi,[ebx+<id slot>]      ; mov [edi],ax        the 2-byte id of the entry (a `.dynsym` global)
//! ```
//!
//! `InitializeGlobalCS` calls pass eight words at `[esp+4..=esp+0x20]` and store the record pointer
//! in a global. Nothing here decodes instructions: the scan matches the call targets and the few
//! store patterns above, and the counts pinned from the research (278 / 109 / 43) tell when the
//! template does not fit.

use std::collections::BTreeMap;

use crate::elf::{Elf, ElfError};
use crate::scan::{Section, Stage};

// research: civ5-re RW-1 — addresses in `Civ5XP` 1.0.3.279.
/// `initFireGrafix_ShaderDefDX9`.
pub const INIT_DX9_ADDR: u32 = 0x0878_1869;
/// Its size in bytes.
pub const INIT_DX9_SIZE: u32 = 41_784;
/// `CRenderer9::CreatePixelShaderWithName`.
pub const CREATE_PS: u32 = 0x08d0_c4f4;
/// `CRenderer9::CreateVertexShaderWithName`.
pub const CREATE_VS: u32 = 0x08d0_c652;
/// `FGXConstantSetManager::InitializeGlobalCS`.
pub const INIT_CS: u32 = 0x08d0_9964;
/// Calls in `initFireGrafix_ShaderDefDX9`: pixel shaders, vertex shaders, constant sets.
pub const PINNED_PS: usize = 278;
pub const PINNED_VS: usize = 109;
pub const PINNED_CS: usize = 43;
/// Table of the shader-package names: `{char* name, u32 id}` pairs, ids 0, 1, 2 ...
pub const PACKAGE_TABLE_ADDR: u32 = 0x0a2f_2700;
/// Names in that table.
pub const PACKAGE_COUNT: usize = 94;

/// A name of a block must look like this (`g_pdw...._SM30`).
const NAME_PREFIX: &str = "g_pdw";
const NAME_SUFFIX: &str = "_SM30";
/// How far before a call the arguments are looked for (bytes).
const WINDOW: usize = 200;
/// How far after a call the id store is looked for (bytes).
const AFTER: usize = 24;

#[derive(Debug, thiserror::Error)]
pub enum NamesError {
    #[error(transparent)]
    Elf(#[from] ElfError),
    #[error("Civ5XP: section {0} not found")]
    NoSection(&'static str),
    #[error("registration code: {0}")]
    Template(String),
    #[error("registration scan: {what}: found {found}, pinned {pinned}")]
    Count {
        what: &'static str,
        found: usize,
        pinned: usize,
    },
    #[error("reference table {path}: {why}")]
    Reference { path: String, why: String },
}

/// One `Create{Pixel,Vertex}ShaderWithName` call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registration {
    pub stage: Stage,
    /// `g_pdwUnitShader_UnitPS_PS_SM30`, the name string passed to the call.
    pub code_name: String,
    /// Address of the block in `.rodata`.
    pub block_addr: u32,
    pub block_size: u32,
    /// Fourth argument of vertex shaders (vertex declaration data).
    pub vs_arg4: Option<u32>,
    /// Address of the 2-byte id slot.
    pub id_addr: u32,
    /// `FireGrafix::g_UnitShader_UnitPS_PS` from `.dynsym`.
    pub id_global: String,
    pub call_site: u32,
    /// Called from `initFireGrafix_ShaderDefDX9` (the world) and not from the sibling registrar.
    pub in_world_init: bool,
}

/// One `InitializeGlobalCS` call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConstSetCall {
    pub call_site: u32,
    /// `FireGrafix::g_pCameraParameters`.
    pub global: String,
    /// Words at `[esp+4]` ... `[esp+0x20]`: set index, byte size, six more.
    pub args: [u32; 8],
    pub in_world_init: bool,
}

/// What the scan of the registration code found.
#[derive(Clone, Debug, Default)]
pub struct Names {
    pub registrations: Vec<Registration>,
    pub const_sets: Vec<ConstSetCall>,
    /// Package names (the ShaderSet values of FGX materials) in id order.
    pub packages: Vec<String>,
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

/// `_ZN10FireGrafix18g_pCameraParametersE` → `["FireGrafix", "g_pCameraParameters"]`.
pub fn demangle_nested(sym: &str) -> Option<Vec<String>> {
    let mut rest = sym.strip_prefix("_ZN")?;
    let mut parts = Vec::new();
    while !rest.starts_with('E') {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        let len: usize = rest[..digits].parse().ok()?;
        rest = &rest[digits..];
        parts.push(rest.get(..len)?.to_string());
        rest = &rest[len..];
    }
    (rest == "E").then_some(parts)
}

/// Address-indexed view of the file: bytes by virtual address.
struct Image<'a> {
    data: &'a [u8],
    sections: &'a [Section],
}

impl<'a> Image<'a> {
    fn offset(&self, addr: u32) -> Option<usize> {
        let a = u64::from(addr);
        self.sections
            .iter()
            .find(|s| s.addr != 0 && s.size != 0 && a >= s.addr && a < s.addr + s.size as u64)
            .map(|s| s.offset + (a - s.addr) as usize)
    }

    fn cstr(&self, addr: u32) -> Option<String> {
        let o = self.offset(addr)?;
        let rest = self.data.get(o..)?;
        let end = rest.iter().take(256).position(|&b| b == 0)?;
        std::str::from_utf8(&rest[..end]).ok().map(str::to_string)
    }

    fn u32(&self, addr: u32) -> Option<u32> {
        u32_at(self.data, self.offset(addr)?)
    }
}

/// `lea reg,[ebx+imm32]` at `w[i..]`: (register number, imm32).
fn lea_ebx(w: &[u8], i: usize) -> Option<(u8, u32)> {
    if *w.get(i)? != 0x8d {
        return None;
    }
    let modrm = *w.get(i + 1)?;
    // mod = 10 (disp32), rm = 3 (ebx).
    (modrm & 0xC7 == 0x83).then_some(())?;
    Some(((modrm >> 3) & 7, u32_at(w, i + 2)?))
}

/// Arguments stored before a call, by stack offset: pointers from `lea`+`mov [esp+off],reg`
/// (value = `ebx + imm`), immediates from `mov dword [esp+off],imm32`.
struct CallArgs {
    ptrs: BTreeMap<u8, u32>,
    imms: BTreeMap<u8, u32>,
}

fn parse_args(w: &[u8], ebx: u32) -> CallArgs {
    let mut a = CallArgs {
        ptrs: BTreeMap::new(),
        imms: BTreeMap::new(),
    };
    for i in 0..w.len() {
        if let Some((reg, imm)) = lea_ebx(w, i) {
            // mov [esp+off],reg = 89 /r with modrm 01 reg 100, sib 24, disp8.
            if w.get(i + 6) == Some(&0x89)
                && w.get(i + 7) == Some(&(0x44 | (reg << 3)))
                && w.get(i + 8) == Some(&0x24)
                && let Some(&off) = w.get(i + 9)
            {
                a.ptrs.insert(off, ebx.wrapping_add(imm));
            }
        }
        // mov dword [esp+off],imm32 = c7 44 24 off imm32.
        if w.get(i) == Some(&0xc7)
            && w.get(i + 1) == Some(&0x44)
            && w.get(i + 2) == Some(&0x24)
            && let (Some(&off), Some(imm)) = (w.get(i + 3), u32_at(w, i + 4))
        {
            a.imms.insert(off, imm);
        }
    }
    a
}

/// The global that the code stores to after a call: the first `lea reg,[ebx+imm32]` that is not
/// the start of the next call's arguments.
fn after_call_global(w: &[u8], ebx: u32) -> Option<u32> {
    for i in 0..w.len() {
        if let Some((reg, imm)) = lea_ebx(w, i) {
            let next_is_arg = w.get(i + 6) == Some(&0x89)
                && w.get(i + 7) == Some(&(0x44 | (reg << 3)))
                && w.get(i + 8) == Some(&0x24)
                && matches!(w.get(i + 9), Some(4 | 8));
            if !next_is_arg {
                return Some(ebx.wrapping_add(imm));
            }
        }
    }
    None
}

/// Value of `ebx` (the GOT) in `initFireGrafix_ShaderDefDX9`: `call +0; pop ebx; add ebx,imm32`.
fn got_base(img: &Image) -> Result<u32, NamesError> {
    let o = img
        .offset(INIT_DX9_ADDR)
        .ok_or(NamesError::NoSection(".text"))?;
    let w = &img.data[o..];
    let thunk: [u8; 8] = [0xe8, 0, 0, 0, 0, 0x5b, 0x81, 0xc3];
    // Prologue: push ebp, ebx, edi, esi (4 bytes), sub esp,imm32 (6 bytes), then the thunk.
    let at = (0..32)
        .find(|&i| w.get(i..i + 8) == Some(&thunk))
        .ok_or_else(|| {
            NamesError::Template("no `call +0; pop ebx; add ebx` thunk at the start".into())
        })?;
    let imm = u32_at(w, at + 8).ok_or_else(|| NamesError::Template("thunk cut off".into()))?;
    Ok(INIT_DX9_ADDR.wrapping_add(at as u32 + 5).wrapping_add(imm))
}

/// Names of `.dynsym` globals by address, as `Ns::name` (demangled nested names).
fn dynsym_by_addr(elf: &Elf) -> Result<BTreeMap<u32, String>, NamesError> {
    let mut m = BTreeMap::new();
    for s in elf.dynamic_symbols()? {
        if s.shndx == 0 || s.value == 0 {
            continue;
        }
        if let Some(parts) = demangle_nested(&s.name) {
            m.entry(s.value).or_insert_with(|| parts.join("::"));
        }
    }
    Ok(m)
}

/// Scans `Civ5XP` for the registrations (SR-D7). `data` is the whole file.
pub fn extract(data: &[u8]) -> Result<Names, NamesError> {
    let elf = Elf::parse(data)?;
    let sections = elf.sections();
    let text = elf.section(".text").ok_or(NamesError::NoSection(".text"))?;
    let img = Image { data, sections };
    let globals = dynsym_by_addr(&elf)?;
    let ebx = got_base(&img)?;

    let code = &data[text.offset..text.offset + text.size];
    let mut calls: Vec<(usize, u32)> = Vec::new();
    for i in 0..code.len().saturating_sub(5) {
        if code[i] != 0xe8 {
            continue;
        }
        let rel = i32::from_le_bytes(code[i + 1..i + 5].try_into().expect("4 bytes"));
        let site = text.addr as u32 + i as u32;
        let target = site.wrapping_add(5).wrapping_add(rel as u32);
        if target == CREATE_PS || target == CREATE_VS || target == INIT_CS {
            calls.push((i, target));
        }
    }

    let mut names = Names::default();
    let mut prev_end = 0usize;
    for &(i, target) in &calls {
        let site = text.addr as u32 + i as u32;
        let in_world = (INIT_DX9_ADDR..INIT_DX9_ADDR + INIT_DX9_SIZE).contains(&site);
        let lo = prev_end.max(i.saturating_sub(WINDOW));
        let args = parse_args(&code[lo..i], ebx);
        let after = &code[i + 5..(i + 5 + AFTER).min(code.len())];
        let slot = after_call_global(after, ebx);
        let bad = |why: &str| NamesError::Template(format!("call at {site:#010x}: {why}"));
        prev_end = i + 5;

        if target == INIT_CS {
            let mut a = [0u32; 8];
            for (k, v) in a.iter_mut().enumerate() {
                *v = *args
                    .imms
                    .get(&(4 + 4 * k as u8))
                    .ok_or_else(|| bad("constant set: argument missing"))?;
            }
            let g = slot.ok_or_else(|| bad("constant set: no global store"))?;
            let global = globals
                .get(&g)
                .ok_or_else(|| bad("constant set: global has no .dynsym name"))?
                .clone();
            names.const_sets.push(ConstSetCall {
                call_site: site,
                global,
                args: a,
                in_world_init: in_world,
            });
            continue;
        }

        let stage = if target == CREATE_PS {
            Stage::Ps
        } else {
            Stage::Vs
        };
        let block_addr = *args.ptrs.get(&8).ok_or_else(|| bad("no block pointer"))?;
        let name_addr = *args.ptrs.get(&4).ok_or_else(|| bad("no name pointer"))?;
        let code_name = img
            .cstr(name_addr)
            .ok_or_else(|| bad("name is not a string"))?;
        if !code_name.starts_with(NAME_PREFIX) || !code_name.ends_with(NAME_SUFFIX) {
            return Err(bad(&format!("name {code_name:?} has the wrong shape")));
        }
        let block_size = *args.imms.get(&0x0c).ok_or_else(|| bad("no block size"))?;
        let id_addr = slot.ok_or_else(|| bad("no id store"))?;
        let id_global = globals
            .get(&id_addr)
            .ok_or_else(|| bad("id slot has no .dynsym name"))?
            .clone();
        names.registrations.push(Registration {
            stage,
            code_name,
            block_addr,
            block_size,
            vs_arg4: (stage == Stage::Vs)
                .then(|| args.imms.get(&0x10).copied())
                .flatten(),
            id_addr,
            id_global,
            call_site: site,
            in_world_init: in_world,
        });
    }

    names.packages = package_table(&img)?;
    verify_pins(&names)?;
    Ok(names)
}

/// The 94 package names: `{char* name, u32 id}` pairs from [`PACKAGE_TABLE_ADDR`].
fn package_table(img: &Image) -> Result<Vec<String>, NamesError> {
    let mut out = Vec::new();
    for i in 0..PACKAGE_COUNT {
        let at = PACKAGE_TABLE_ADDR + 8 * i as u32;
        let (Some(p), Some(id)) = (img.u32(at), img.u32(at + 4)) else {
            return Err(NamesError::Template(format!(
                "package table entry {i} unreadable"
            )));
        };
        if id as usize != i {
            return Err(NamesError::Template(format!(
                "package table entry {i} has id {id}"
            )));
        }
        out.push(
            img.cstr(p)
                .ok_or_else(|| NamesError::Template(format!("package {i}: name unreadable")))?,
        );
    }
    Ok(out)
}

/// The pinned counts of `initFireGrafix_ShaderDefDX9`.
pub fn verify_pins(n: &Names) -> Result<(), NamesError> {
    let count = |what: &'static str, found: usize, pinned: usize| {
        if found == pinned {
            Ok(())
        } else {
            Err(NamesError::Count {
                what,
                found,
                pinned,
            })
        }
    };
    let ps = n
        .registrations
        .iter()
        .filter(|r| r.in_world_init && r.stage == Stage::Ps)
        .count();
    let vs = n
        .registrations
        .iter()
        .filter(|r| r.in_world_init && r.stage == Stage::Vs)
        .count();
    let cs = n.const_sets.iter().filter(|c| c.in_world_init).count();
    count("CreatePixelShaderWithName calls", ps, PINNED_PS)?;
    count("CreateVertexShaderWithName calls", vs, PINNED_VS)?;
    count("InitializeGlobalCS calls", cs, PINNED_CS)
}

/// `g_UnitShader_UnitPS_PS` → (`UnitShader`, `UnitPS`, `PS`). Package names hold no `_`.
/// `None` for an empty slot (`g_UnitShader__HS`).
pub fn split_slot(id_global: &str) -> Option<(String, String, String)> {
    let name = id_global.strip_prefix("FireGrafix::g_")?;
    let (package, rest) = name.split_once('_')?;
    let (entry, stage) = rest.rsplit_once('_')?;
    // `g_SimpleShader_Skinned__HS`: the empty slot of the package `SimpleShader_Skinned`.
    if entry.is_empty()
        || entry.ends_with('_')
        || !matches!(stage, "VS" | "PS" | "HS" | "DS" | "GS")
    {
        return None;
    }
    Some((package.to_string(), entry.to_string(), stage.to_string()))
}

/// All shader slots of `.dynsym` (`g_<Package>_<Entry>_<Stage>`, non-empty entry), with the stage
/// letters: the entries of the catalog, including those without an SM3 block (DX11 only).
pub fn dynsym_slots(data: &[u8]) -> Result<Vec<(u32, String, String, String)>, NamesError> {
    let elf = Elf::parse(data)?;
    let mut out: Vec<(u32, String, String, String)> = Vec::new();
    for (addr, name) in dynsym_by_addr(&elf)? {
        if let Some((p, e, s)) = split_slot(&name) {
            out.push((addr, p, e, s));
        }
    }
    out.sort();
    Ok(out)
}

/// Compares the scan with `shaderdef.tsv` of `civ5-re` (RW-1): every row must agree on stage,
/// block address and size, id slot and call site. Another answer is an error.
pub fn compare_reference(names: &Names, tsv: &str, path: &str) -> Result<(), NamesError> {
    let bad = |why: String| NamesError::Reference {
        path: path.to_string(),
        why,
    };
    let mut rows = 0usize;
    let by_site: BTreeMap<u32, &Registration> = names
        .registrations
        .iter()
        .map(|r| (r.call_site, r))
        .collect();
    for (n, line) in tsv.lines().enumerate().skip(1) {
        if line.trim().is_empty() {
            continue;
        }
        // stage, entry, code_label, size, vs_arg4, id_global_addr, id_global, call_site
        let c: Vec<&str> = line.split('\t').collect();
        if c.len() < 8 {
            return Err(bad(format!("line {}: {} columns", n + 1, c.len())));
        }
        let site = u32::from_str_radix(c[7].trim(), 16)
            .map_err(|_| bad(format!("line {}: call_site {:?}", n + 1, c[7])))?;
        let r = by_site
            .get(&site)
            .ok_or_else(|| bad(format!("line {}: no registration at {site:08x}", n + 1)))?;
        let stage = if c[0] == "PS" { Stage::Ps } else { Stage::Vs };
        let label = c[2]
            .strip_prefix("DAT_")
            .and_then(|h| u32::from_str_radix(h, 16).ok());
        let size: Option<u32> = c[3].parse().ok();
        let id_addr = c[5]
            .strip_prefix("0x")
            .and_then(|h| u32::from_str_radix(h, 16).ok());
        let ok = r.stage == stage
            && label == Some(r.block_addr)
            && size == Some(r.block_size)
            && id_addr == Some(r.id_addr)
            && c[6] == r.id_global;
        if !ok {
            return Err(bad(format!(
                "line {}: {} differs from the scan ({:?})",
                n + 1,
                c[6],
                r
            )));
        }
        rows += 1;
    }
    let world = names
        .registrations
        .iter()
        .filter(|r| r.in_world_init)
        .count();
    if rows != world {
        return Err(bad(format!("{rows} rows, the scan found {world}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demangles_nested_names() {
        assert_eq!(
            demangle_nested("_ZN10FireGrafix19g_pCameraParametersE").unwrap(),
            ["FireGrafix", "g_pCameraParameters"]
        );
        assert_eq!(demangle_nested("_ZN9CRenderer94InitEv"), None);
        assert_eq!(demangle_nested("main"), None);
        assert_eq!(demangle_nested("_ZN3fooE").unwrap(), ["foo"]);
    }

    #[test]
    fn slots_split_at_the_first_underscore() {
        assert_eq!(
            split_slot("FireGrafix::g_UnitShader_UnitPS_PS"),
            Some(("UnitShader".into(), "UnitPS".into(), "PS".into()))
        );
        assert_eq!(
            split_slot("FireGrafix::g_BuildingShader_BuildingPS_CODE_BASE_SHADOW_PS"),
            Some((
                "BuildingShader".into(),
                "BuildingPS_CODE_BASE_SHADOW".into(),
                "PS".into()
            ))
        );
        // Empty slots and other globals are not entries.
        assert_eq!(split_slot("FireGrafix::g_UnitShader__HS"), None);
        assert_eq!(split_slot("FireGrafix::g_SimpleShader_Skinned__HS"), None);
        assert_eq!(split_slot("FireGrafix::g_pCameraParameters"), None);
        assert_eq!(split_slot("g_UnitShader_UnitPS_PS"), None);
    }

    #[test]
    fn call_arguments_are_read_from_the_setup_code() {
        // lea ecx,[ebx+0x100]; mov [esp+8],ecx; lea ecx,[ebx+0x200]; mov [esp+4],ecx;
        // mov dword [esp+0x10],7; mov dword [esp+0xc],0x164
        let mut w = vec![0x8d, 0x8b, 0x00, 0x01, 0x00, 0x00, 0x89, 0x4c, 0x24, 0x08];
        w.extend([0x8d, 0x8b, 0x00, 0x02, 0x00, 0x00, 0x89, 0x4c, 0x24, 0x04]);
        w.extend([0xc7, 0x44, 0x24, 0x10, 7, 0, 0, 0]);
        w.extend([0xc7, 0x44, 0x24, 0x0c, 0x64, 1, 0, 0]);
        let a = parse_args(&w, 0x1000);
        assert_eq!(a.ptrs[&8], 0x1100);
        assert_eq!(a.ptrs[&4], 0x1200);
        assert_eq!(a.imms[&0x10], 7);
        assert_eq!(a.imms[&0x0c], 0x164);
        // The id store after the call is the first lea that is not an argument.
        let after = [0x8d, 0xbb, 0xf8, 0x00, 0x00, 0x00, 0x66, 0x89, 0x07];
        assert_eq!(after_call_global(&after, 0x1000), Some(0x10f8));
        let next_arg = [0x8d, 0x8b, 0x00, 0x01, 0x00, 0x00, 0x89, 0x4c, 0x24, 0x08];
        assert_eq!(after_call_global(&next_arg, 0x1000), None);
    }

    #[test]
    fn reference_rows_must_match() {
        let r = Registration {
            stage: Stage::Ps,
            code_name: "g_pdwA_B_PS_SM30".into(),
            block_addr: 0x0915_b57c,
            block_size: 348,
            vs_arg4: None,
            id_addr: 0x0a3e_70fa,
            id_global: "FireGrafix::g_A_B_PS".into(),
            call_site: 0x0878_2762,
            in_world_init: true,
        };
        let names = Names {
            registrations: vec![r],
            ..Names::default()
        };
        let head =
            "stage\tentry\tcode_label\tsize\tvs_arg4\tid_global_addr\tid_global\tcall_site\n";
        let row = "PS\tg_pdwA_B_PS_SM30\tDAT_0915b57c\t348\t\t0x0a3e70fa\tFireGrafix::g_A_B_PS\t08782762\n";
        compare_reference(&names, &format!("{head}{row}"), "t").unwrap();
        let wrong = row.replace("348", "349");
        assert!(compare_reference(&names, &format!("{head}{wrong}"), "t").is_err());
        assert!(compare_reference(&names, head, "t").is_err());
    }
}
