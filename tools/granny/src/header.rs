//! File header, file info (revision 7) and section table.

use crate::bytes::{sub, u32_at};
use crate::{GrannyError, Ptr};

/// 32-bit little-endian Granny magic (the only variant Civ5 ships).
pub const MAGIC_LE32: [u8; 16] = [
    0x29, 0xDE, 0x6C, 0xC0, 0xBA, 0xA4, 0x53, 0x2B, 0x25, 0xF5, 0xB7, 0xA5, 0xF6, 0x66, 0xE2, 0xEE,
];
// Recognised only to give a precise error.
const MAGIC_LE64: [u8; 16] = [
    0xE5, 0x9B, 0x49, 0x5E, 0x6F, 0x63, 0x1F, 0x14, 0x1E, 0x13, 0xEB, 0xA9, 0x90, 0xBE, 0xED, 0xC4,
];
const MAGIC_BE32: [u8; 16] = [
    0x0E, 0x11, 0x95, 0xB5, 0x6A, 0xA5, 0xB5, 0x4B, 0xEB, 0x28, 0x28, 0x50, 0x25, 0x78, 0xB3, 0x04,
];
const MAGIC_BE64: [u8; 16] = [
    0x31, 0x95, 0xD4, 0xE3, 0x20, 0xDC, 0x4F, 0x62, 0xCC, 0x36, 0xD0, 0x3A, 0xB1, 0x82, 0xFF, 0x89,
];

pub const FORMAT_REVISION: u32 = 7;
const FILE_INFO: usize = 32;
const SECTION_HEADER_SIZE: usize = 44;

/// Section compression as stored in the section header.
pub const COMPRESSION_NONE: u32 = 0;
pub const COMPRESSION_OODLE1: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionHeader {
    pub compression: u32,
    /// Offsets are relative to the start of the gr2 file.
    pub data_offset: u32,
    pub data_size: u32,
    pub expanded_size: u32,
    pub alignment: u32,
    /// Oodle1 stop offsets: end of the 32-bit-aligned and 16-bit-aligned parts.
    pub first_16bit: u32,
    pub first_8bit: u32,
    pub fixup_offset: u32,
    pub fixup_count: u32,
    pub marshal_offset: u32,
    pub marshal_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub header_size: u32,
    pub revision: u32,
    pub total_size: u32,
    pub crc32: u32,
    pub root_type: Ptr,
    pub root_object: Ptr,
    pub type_tag: u32,
    pub sections: Vec<SectionHeader>,
}

impl Header {
    pub fn parse(data: &[u8]) -> Result<Self, GrannyError> {
        let magic = sub(data, 0, 16, "magic")?;
        if magic != MAGIC_LE32 {
            return Err(match magic {
                m if m == MAGIC_LE64 => GrannyError::UnsupportedVariant("64-bit little-endian"),
                m if m == MAGIC_BE32 => GrannyError::UnsupportedVariant("32-bit big-endian"),
                m if m == MAGIC_BE64 => GrannyError::UnsupportedVariant("64-bit big-endian"),
                _ => GrannyError::BadMagic,
            });
        }
        let header_size = u32_at(data, 16, "header size")?;
        let fi = |k: usize| u32_at(data, FILE_INFO + k, "file info");
        let revision = fi(0)?;
        if revision != FORMAT_REVISION {
            return Err(GrannyError::UnsupportedRevision(revision));
        }
        let total_size = fi(4)?;
        if total_size as usize > data.len() {
            return Err(GrannyError::OutOfBounds("file shorter than its total size"));
        }
        let ptr = |k: usize| -> Result<Ptr, GrannyError> {
            Ok(Ptr {
                section: fi(k)?,
                offset: fi(k + 4)?,
            })
        };
        let table = FILE_INFO
            .checked_add(fi(12)? as usize)
            .ok_or(GrannyError::OutOfBounds("section table"))?;
        let count = fi(16)? as usize;
        let table_len = count
            .checked_mul(SECTION_HEADER_SIZE)
            .ok_or(GrannyError::OutOfBounds("section table"))?;
        let table_bytes = sub(data, table, table_len, "section table")?;
        let sections = table_bytes
            .chunks_exact(SECTION_HEADER_SIZE)
            .map(|s| {
                let f = |k: usize| u32_at(s, k * 4, "section header");
                Ok(SectionHeader {
                    compression: f(0)?,
                    data_offset: f(1)?,
                    data_size: f(2)?,
                    expanded_size: f(3)?,
                    alignment: f(4)?,
                    first_16bit: f(5)?,
                    first_8bit: f(6)?,
                    fixup_offset: f(7)?,
                    fixup_count: f(8)?,
                    marshal_offset: f(9)?,
                    marshal_count: f(10)?,
                })
            })
            .collect::<Result<Vec<_>, GrannyError>>()?;
        Ok(Self {
            header_size,
            revision,
            total_size,
            crc32: fi(8)?,
            root_type: ptr(20)?,
            root_object: ptr(28)?,
            type_tag: fi(36)?,
            sections,
        })
    }
}
