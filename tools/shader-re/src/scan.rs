//! Shader block scan: SM3 (D3D9) and DXBC (D3D10/11) blocks in the data sections of the binaries
//! and the pinned counts of 3.1.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::bins::{BinId, FoundBin};
use crate::elf::{Elf, ElfError};
use crate::pe::{Pe, PeError};
use crate::sha1::sha1_hex;

// research: approach §1.1 (pinned numbers; another number is an extraction error).
/// SM3 blocks in `Civ5XP` and in `CivilizationV.exe`.
pub const SM3_TOTAL: usize = 433;
pub const SM3_VS: usize = 121;
pub const SM3_PS: usize = 312;
/// Distinct blocks (by sha1) among the 433.
pub const SM3_UNIQUE: usize = 401;
/// Sum of the block sizes: Linux 721 548, DX9 722 860 bytes.
pub const SM3_BYTES_LINUX: usize = 721_548;
pub const SM3_BYTES_DX9: usize = 722_860;
/// DXBC blocks of `CivilizationV_DX11.exe`.
pub const DXBC_TOTAL: usize = 1417;
/// Distinct DXBC blocks (by sha1).
pub const DXBC_UNIQUE: usize = 1361;
/// DXBC blocks by profile.
pub const DXBC_PROFILES: [(&str, usize); 8] = [
    ("vs_4_0", 126),
    ("vs_4_1", 126),
    ("vs_5_0", 128),
    ("ps_4_0", 320),
    ("ps_4_1", 352),
    ("ps_5_0", 362),
    ("hs_5_0", 2),
    ("ds_5_0", 1),
];

// D3D9 shader token stream (DirectX 9 SDK, "Shader Code" / d3d9types.h).
/// `D3DVS_VERSION(3, 0)`.
const VS_3_0: u32 = 0xFFFE_0300;
/// `D3DPS_VERSION(3, 0)`.
const PS_3_0: u32 = 0xFFFF_0300;
/// `D3DSIO_COMMENT` opcode (low 16 bits); the length in dwords is in bits 16-30.
const OP_COMMENT: u32 = 0xFFFE;
/// `D3DSIO_END`.
const OP_END: u32 = 0xFFFF;
/// Highest opcode of the SM3 instruction set (`D3DSIO_BREAKP` = 0x60; 0x61.. unused).
const OP_MAX: u32 = 0x60;
/// `'CTAB'` as a little-endian dword.
const CTAB: u32 = 0x4241_5443;
/// A block above this size is not a shader (the biggest real one is a few tens of KB).
const MAX_SM3_BYTES: usize = 1 << 20;

// DXBC container (Microsoft: "DXBC container format", d3d11TokenizedProgramFormat.hpp).
const DXBC_MAGIC: &[u8; 4] = b"DXBC";
const DXBC_HEADER: usize = 32;
const DXBC_MAX_CHUNKS: usize = 64;
const DXBC_MAX_BYTES: usize = 8 << 20;
/// Program type in the high half of the first `SHDR`/`SHEX` token.
const DXBC_TYPES: [&str; 6] = ["ps", "vs", "gs", "hs", "ds", "cs"];

#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error(transparent)]
    Elf(#[from] ElfError),
    #[error(transparent)]
    Pe(#[from] PeError),
    #[error("{bin}: section {section} not found")]
    NoSection { bin: BinId, section: &'static str },
    #[error("{bin}: {what}: found {found}, pinned {pinned}")]
    CountMismatch {
        bin: BinId,
        what: String,
        found: usize,
        pinned: usize,
    },
    #[error("{bin}: this binary holds {expected} blocks only")]
    WrongKind { bin: BinId, expected: &'static str },
    #[error("{path}: {source}", path = .path.display())]
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
}

/// A section of a binary: where it lives in the file and in memory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub name: String,
    /// Virtual address of the first byte (ELF `sh_addr`, PE image base + RVA).
    pub addr: u64,
    /// File offset of the first byte.
    pub offset: usize,
    /// Bytes present in the file.
    pub size: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    Vs,
    Ps,
    Gs,
    Hs,
    Ds,
    Cs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Sm3,
    Dxbc,
}

/// One block as found in the binary.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    pub sha1: String,
    pub kind: Kind,
    pub stage: Stage,
    /// `vs_3_0`, `ps_3_0`, `ps_5_0`, ...
    pub profile: String,
    /// File offset.
    pub offset: usize,
    /// Virtual address.
    pub addr: u64,
    pub size: usize,
    pub section: String,
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

/// Size in bytes of the SM3 block that starts at `d[0]`, or `None` when it is not one.
///
/// Layout: version token, a comment token with `CTAB`, instructions (length in bits 24-27),
/// end token. A block must walk to `D3DSIO_END` without a token that does not fit.
pub fn sm3_block_len(d: &[u8]) -> Option<usize> {
    let version = u32_at(d, 0)?;
    if version != VS_3_0 && version != PS_3_0 {
        return None;
    }
    let first = u32_at(d, 4)?;
    if first & 0xFFFF != OP_COMMENT || u32_at(d, 8)? != CTAB {
        return None;
    }
    let limit = d.len().min(MAX_SM3_BYTES);
    let mut pos = 4usize;
    loop {
        let tok = u32_at(d, pos)?;
        let op = tok & 0xFFFF;
        pos += 4;
        if op == OP_COMMENT {
            let len = ((tok >> 16) & 0x7FFF) as usize;
            pos += len * 4;
        } else if op == OP_END {
            return (pos <= limit).then_some(pos);
        } else {
            // Instruction token: opcode in 0..=OP_MAX, bit 31 clear, length in bits 24-27.
            if op > OP_MAX || tok & 0x8000_0000 != 0 {
                return None;
            }
            pos += (((tok >> 24) & 0xF) as usize) * 4;
        }
        if pos >= limit {
            return None;
        }
    }
}

/// DXBC block at `d[0]`: size and profile, or `None`.
pub fn dxbc_block_info(d: &[u8]) -> Option<(usize, Stage, String)> {
    if d.get(..4)? != DXBC_MAGIC {
        return None;
    }
    // 16-byte checksum at 4, then version 1 at 20.
    if u32_at(d, 20)? != 1 {
        return None;
    }
    let total = u32_at(d, 24)? as usize;
    let count = u32_at(d, 28)? as usize;
    if count == 0 || count > DXBC_MAX_CHUNKS || total > d.len() || total > DXBC_MAX_BYTES {
        return None;
    }
    let table_end = DXBC_HEADER + count * 4;
    if total < table_end {
        return None;
    }
    let mut program = None;
    for i in 0..count {
        let off = u32_at(d, DXBC_HEADER + i * 4)? as usize;
        if off < table_end || off.checked_add(8)? > total {
            return None;
        }
        let fourcc = d.get(off..off + 4)?;
        if !fourcc.iter().all(|b| b.is_ascii_alphanumeric()) {
            return None;
        }
        let size = u32_at(d, off + 4)? as usize;
        if off.checked_add(8 + size)? > total {
            return None;
        }
        if fourcc == b"SHDR" || fourcc == b"SHEX" {
            program = Some(u32_at(d, off + 8)?);
        }
    }
    // First program token: minor in bits 0-3, major in 4-7, type in 16-31.
    let tok = program?;
    let (minor, major, ty) = (tok & 0xF, (tok >> 4) & 0xF, (tok >> 16) as usize);
    let prefix = *DXBC_TYPES.get(ty)?;
    let stage = match ty {
        0 => Stage::Ps,
        1 => Stage::Vs,
        2 => Stage::Gs,
        3 => Stage::Hs,
        4 => Stage::Ds,
        _ => Stage::Cs,
    };
    Some((total, stage, format!("{prefix}_{major}_{minor}")))
}

fn section_bytes<'a>(data: &'a [u8], s: &Section) -> &'a [u8] {
    &data[s.offset..s.offset + s.size]
}

/// All SM3 blocks of `sections`, in file order.
pub fn scan_sm3(data: &[u8], sections: &[&Section]) -> Vec<Block> {
    let mut out = Vec::new();
    for s in sections {
        let bytes = section_bytes(data, s);
        let mut i = 0;
        while i + 12 <= bytes.len() {
            // Quick reject on the version token `00 03 FE|FF FF`.
            if bytes[i + 1] == 0x03
                && bytes[i] == 0x00
                && bytes[i + 3] == 0xFF
                && let Some(len) = sm3_block_len(&bytes[i..])
            {
                {
                    let version = u32_at(bytes, i).unwrap_or(0);
                    let (stage, profile) = if version == VS_3_0 {
                        (Stage::Vs, "vs_3_0")
                    } else {
                        (Stage::Ps, "ps_3_0")
                    };
                    out.push(Block {
                        sha1: sha1_hex(&bytes[i..i + len]),
                        kind: Kind::Sm3,
                        stage,
                        profile: profile.to_string(),
                        offset: s.offset + i,
                        addr: s.addr + i as u64,
                        size: len,
                        section: s.name.clone(),
                    });
                    i += len;
                    continue;
                }
            }
            i += 1;
        }
    }
    out
}

/// All DXBC blocks of `sections`, in file order.
pub fn scan_dxbc(data: &[u8], sections: &[&Section]) -> Vec<Block> {
    let mut out = Vec::new();
    for s in sections {
        let bytes = section_bytes(data, s);
        let mut i = 0;
        while i + DXBC_HEADER <= bytes.len() {
            if bytes[i] == b'D'
                && bytes[i + 1] == b'X'
                && let Some((len, stage, profile)) = dxbc_block_info(&bytes[i..])
            {
                {
                    out.push(Block {
                        sha1: sha1_hex(&bytes[i..i + len]),
                        kind: Kind::Dxbc,
                        stage,
                        profile,
                        offset: s.offset + i,
                        addr: s.addr + i as u64,
                        size: len,
                        section: s.name.clone(),
                    });
                    i += len;
                    continue;
                }
            }
            i += 1;
        }
    }
    out
}

/// Blocks of one pinned binary: ELF `.rodata` for Linux, PE `.rdata`, `.data` and `.shader` for the exes.
/// The pinned counts are checked separately by [`verify_pins`].
pub fn extract(bin: BinId, data: &[u8]) -> Result<Vec<Block>, ScanError> {
    match bin {
        BinId::Linux => {
            let elf = Elf::parse(data)?;
            let ro = elf.section(".rodata").ok_or(ScanError::NoSection {
                bin,
                section: ".rodata",
            })?;
            Ok(scan_sm3(data, &[ro]))
        }
        BinId::Dx9 | BinId::Dx11 => {
            let pe = Pe::parse(data)?;
            // `.shader` holds the SM3 blocks of the DX9 exe (a packer-style section of its own).
            let secs: Vec<&Section> = [".rdata", ".data", ".shader"]
                .into_iter()
                .filter_map(|n| pe.section(n))
                .collect();
            if secs.is_empty() {
                return Err(ScanError::NoSection {
                    bin,
                    section: ".rdata",
                });
            }
            Ok(if bin == BinId::Dx9 {
                scan_sm3(data, &secs)
            } else {
                scan_dxbc(data, &secs)
            })
        }
    }
}

fn mismatch(bin: BinId, what: &str, found: usize, pinned: usize) -> Result<(), ScanError> {
    if found == pinned {
        Ok(())
    } else {
        Err(ScanError::CountMismatch {
            bin,
            what: what.to_string(),
            found,
            pinned,
        })
    }
}

pub fn unique_count(blocks: &[Block]) -> usize {
    let mut seen: Vec<&str> = blocks.iter().map(|b| b.sha1.as_str()).collect();
    seen.sort_unstable();
    seen.dedup();
    seen.len()
}

/// Blocks per profile, sorted by profile name.
pub fn profile_counts(blocks: &[Block]) -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for b in blocks {
        *m.entry(b.profile.clone()).or_insert(0) += 1;
    }
    m
}

/// Checks the pinned numbers; any other number is an error.
pub fn verify_pins(bin: BinId, blocks: &[Block]) -> Result<(), ScanError> {
    match bin {
        BinId::Linux | BinId::Dx9 => {
            let vs = blocks.iter().filter(|b| b.stage == Stage::Vs).count();
            let ps = blocks.iter().filter(|b| b.stage == Stage::Ps).count();
            let bytes: usize = blocks.iter().map(|b| b.size).sum();
            mismatch(bin, "SM3 blocks", blocks.len(), SM3_TOTAL)?;
            mismatch(bin, "vs_3_0 blocks", vs, SM3_VS)?;
            mismatch(bin, "ps_3_0 blocks", ps, SM3_PS)?;
            mismatch(bin, "unique SM3 blocks", unique_count(blocks), SM3_UNIQUE)?;
            let want = if bin == BinId::Linux {
                SM3_BYTES_LINUX
            } else {
                SM3_BYTES_DX9
            };
            mismatch(bin, "SM3 bytes", bytes, want)
        }
        BinId::Dx11 => {
            mismatch(bin, "DXBC blocks", blocks.len(), DXBC_TOTAL)?;
            let counts = profile_counts(blocks);
            for (profile, n) in DXBC_PROFILES {
                mismatch(
                    bin,
                    &format!("{profile} blocks"),
                    counts.get(profile).copied().unwrap_or(0),
                    n,
                )?;
            }
            mismatch(bin, "unique DXBC blocks", unique_count(blocks), DXBC_UNIQUE)
        }
    }
}

/// `blocks.json` of one binary.
#[derive(Debug, Serialize, Deserialize)]
pub struct BlocksFile {
    pub schema: u32,
    pub bin: String,
    pub sha256: String,
    pub blocks: Vec<Block>,
}

pub const BLOCKS_SCHEMA: u32 = 1;

/// Writes `blocks/<sha1>.bin` (one file per distinct block) and `blocks.json` into `dir`
/// (`<cache>/<sha256-12>/`, SR-D6). Nothing here goes to git.
pub fn write_cache(
    dir: &Path,
    bin: &FoundBin,
    data: &[u8],
    blocks: &[Block],
) -> Result<(), ScanError> {
    let io = |path: &Path| {
        let path = path.to_path_buf();
        move |source| ScanError::Io { path, source }
    };
    let blocks_dir = dir.join("blocks");
    std::fs::create_dir_all(&blocks_dir).map_err(io(&blocks_dir))?;
    for b in blocks {
        let p = blocks_dir.join(format!("{}.bin", b.sha1));
        if !p.exists() {
            std::fs::write(&p, &data[b.offset..b.offset + b.size]).map_err(io(&p))?;
        }
    }
    let file = BlocksFile {
        schema: BLOCKS_SCHEMA,
        bin: bin.id.name().to_string(),
        sha256: bin.sha256.clone(),
        blocks: blocks.to_vec(),
    };
    let json = serde_json::to_string_pretty(&file).expect("blocks.json serializes");
    let p = dir.join("blocks.json");
    std::fs::write(&p, json + "\n").map_err(io(&p))?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod synth {
    //! Synthetic SM3 and DXBC blocks for tests.
    use super::*;

    fn le(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    /// A tiny block: version, CTAB comment of `ctab_words` dwords, `mov r0, c0`-like instruction
    /// (opcode 1, length 2), `n_extra` `nop`-free `mul` instructions, END.
    pub fn sm3(version: u32, ctab_words: u32, n_extra: usize) -> Vec<u8> {
        let mut w = vec![version, OP_COMMENT | ((ctab_words + 1) << 16), CTAB];
        w.extend((0..ctab_words).map(|i| 0x1111_0000 + i));
        w.push(0x0200_0001); // mov: 2 operand tokens
        w.extend([0x800F_0000, 0x80E4_0000]);
        for _ in 0..n_extra {
            w.extend([0x0300_0005, 0x800F_0001, 0x80E4_0000, 0x80E4_0001]); // mul
        }
        w.push(OP_END);
        le(&w)
    }

    /// DXBC with the given chunks `(fourcc, payload)`; `SHDR` payload starts with the program token.
    pub fn dxbc(program_token: u32, extra: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut chunks: Vec<(&[u8; 4], Vec<u8>)> = extra.to_vec();
        chunks.push((b"SHDR", le(&[program_token, 3])));
        let table_end = DXBC_HEADER + chunks.len() * 4;
        let mut body = Vec::new();
        let mut offs = Vec::new();
        for (cc, payload) in &chunks {
            offs.push((table_end + body.len()) as u32);
            body.extend_from_slice(*cc);
            body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            body.extend_from_slice(payload);
        }
        let total = (table_end + body.len()) as u32;
        let mut out = Vec::new();
        out.extend_from_slice(DXBC_MAGIC);
        out.extend_from_slice(&[0xAB; 16]);
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&total.to_le_bytes());
        out.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
        for o in offs {
            out.extend_from_slice(&o.to_le_bytes());
        }
        out.extend_from_slice(&body);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::synth::*;
    use super::*;
    use crate::elf::build_test_elf;
    use crate::pe::build_test_pe;

    /// Padding that contains near-miss tokens.
    fn noise() -> Vec<u8> {
        let mut v = vec![0u8; 5];
        v.extend_from_slice(&VS_3_0.to_le_bytes()); // version without CTAB comment
        v.extend_from_slice(&[0, 3, 0xFF, 0xFF, 1, 2, 3, 4]);
        v
    }

    #[test]
    fn sm3_block_is_measured() {
        let b = sm3(VS_3_0, 4, 2);
        assert_eq!(sm3_block_len(&b), Some(b.len()));
        let mut with_tail = b.clone();
        with_tail.extend_from_slice(&[0xEE; 40]);
        assert_eq!(sm3_block_len(&with_tail), Some(b.len()));
    }

    #[test]
    fn sm3_rejects_bad_streams() {
        let b = sm3(PS_3_0, 4, 2);
        // Truncated before END.
        assert_eq!(sm3_block_len(&b[..b.len() - 4]), None);
        // Opcode above the SM3 range.
        let mut bad = b.clone();
        // Token 7 is the `mov` (version, comment, CTAB, 4 CTAB words).
        bad[28..32].copy_from_slice(&0x0200_0061u32.to_le_bytes());
        assert_eq!(sm3_block_len(&bad), None);
        // Not SM3 version.
        let mut v = b.clone();
        v[..4].copy_from_slice(&0xFFFF_0200u32.to_le_bytes());
        assert_eq!(sm3_block_len(&v), None);
        // Comment that is not CTAB.
        let mut c = b;
        c[8..12].copy_from_slice(b"XXXX");
        assert_eq!(sm3_block_len(&c), None);
        assert_eq!(sm3_block_len(&[]), None);
    }

    #[test]
    fn elf_scan_finds_blocks_with_addresses_and_unique_count() {
        let a = sm3(VS_3_0, 4, 1);
        let b = sm3(PS_3_0, 6, 3);
        let mut ro = noise();
        let off_a = ro.len();
        ro.extend_from_slice(&a);
        ro.extend_from_slice(&noise());
        let off_b = ro.len();
        ro.extend_from_slice(&b);
        ro.extend_from_slice(&noise());
        let off_a2 = ro.len();
        ro.extend_from_slice(&a); // duplicate of `a`
        let elf = build_test_elf(&[
            (".text", 1, 0x1000, &[0; 16]),
            (".rodata", 1, 0x0900_0000, &ro),
        ]);
        let blocks = extract(BinId::Linux, &elf).unwrap();
        assert_eq!(blocks.len(), 3);
        assert_eq!(unique_count(&blocks), 2);
        assert_eq!(blocks[0].stage, Stage::Vs);
        assert_eq!(blocks[0].profile, "vs_3_0");
        assert_eq!(blocks[1].stage, Stage::Ps);
        assert_eq!(blocks[0].addr, 0x0900_0000 + off_a as u64);
        assert_eq!(blocks[1].addr, 0x0900_0000 + off_b as u64);
        assert_eq!(blocks[2].addr, 0x0900_0000 + off_a2 as u64);
        assert_eq!(blocks[0].sha1, sha1_hex(&a));
        assert_eq!(
            &elf[blocks[1].offset..blocks[1].offset + blocks[1].size],
            &b[..]
        );
        // The synthetic file is not the pinned build: the pinned counts reject it.
        assert!(matches!(
            verify_pins(BinId::Linux, &blocks),
            Err(ScanError::CountMismatch { .. })
        ));
    }

    #[test]
    fn pe_scan_finds_sm3_in_rdata_and_data() {
        let a = sm3(PS_3_0, 4, 1);
        let b = sm3(VS_3_0, 5, 2);
        let img = build_test_pe(0x40_0000, &[(".rdata", 0x1000, &a), (".data", 0x2000, &b)]);
        let blocks = extract(BinId::Dx9, &img).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].addr, 0x40_1000);
        assert_eq!(blocks[1].addr, 0x40_2000);
        assert_eq!(blocks[1].section, ".data");
        assert!(extract(BinId::Linux, &img).is_err());
    }

    #[test]
    fn dxbc_profiles_and_garbage() {
        let ps50 = dxbc(0x0000_0050, &[(b"ISGN", vec![0; 8])]);
        let vs40 = dxbc(0x0001_0040, &[]);
        let ds50 = dxbc(0x0004_0050, &[]);
        assert_eq!(dxbc_block_info(&ps50).unwrap().2, "ps_5_0");
        assert_eq!(dxbc_block_info(&vs40).unwrap().1, Stage::Vs);
        assert_eq!(dxbc_block_info(&vs40).unwrap().2, "vs_4_0");
        assert_eq!(dxbc_block_info(&ds50).unwrap().2, "ds_5_0");
        let mut blob = vec![0u8; 3];
        blob.extend_from_slice(&ps50);
        blob.extend_from_slice(b"DXBC garbage garbage garbage garbage");
        blob.extend_from_slice(&vs40);
        let img = build_test_pe(0x40_0000, &[(".rdata", 0x1000, &blob)]);
        let blocks = extract(BinId::Dx11, &img).unwrap();
        let profiles: Vec<_> = blocks.iter().map(|b| b.profile.as_str()).collect();
        assert_eq!(profiles, ["ps_5_0", "vs_4_0"]);
        // Truncated container: not a block.
        assert!(dxbc_block_info(&ps50[..ps50.len() - 1]).is_none());
        // Chunk past the end of the container.
        let mut bad = ps50.clone();
        bad[DXBC_HEADER..DXBC_HEADER + 4].copy_from_slice(&0xFFFF_0000u32.to_le_bytes());
        assert!(dxbc_block_info(&bad).is_none());
    }

    #[test]
    fn cache_files_are_written() {
        let a = sm3(VS_3_0, 4, 1);
        let b = sm3(PS_3_0, 4, 2);
        let mut ro = a.clone();
        ro.extend_from_slice(&b);
        ro.extend_from_slice(&a);
        let elf = build_test_elf(&[(".rodata", 1, 0x1000, &ro)]);
        let blocks = extract(BinId::Linux, &elf).unwrap();
        let dir = std::env::temp_dir().join(format!("shader-re-cache-{}", std::process::id()));
        let bin = FoundBin {
            id: BinId::Linux,
            path: "Civ5XP".into(),
            sha256: "0".repeat(64),
        };
        write_cache(&dir, &bin, &elf, &blocks).unwrap();
        let json = std::fs::read_to_string(dir.join("blocks.json")).unwrap();
        let parsed: BlocksFile = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.blocks, blocks);
        assert_eq!(parsed.bin, "linux");
        let n = std::fs::read_dir(dir.join("blocks")).unwrap().count();
        assert_eq!(n, 2, "one file per distinct block");
        assert_eq!(
            std::fs::read(dir.join("blocks").join(format!("{}.bin", blocks[1].sha1))).unwrap(),
            b
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
