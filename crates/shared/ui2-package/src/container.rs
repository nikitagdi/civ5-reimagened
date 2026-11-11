//! The `C5UI2PKG` container: header, section table, alignment and the trailing blake3 hash.
//!
//! ```text
//! 0   8   magic "C5UI2PKG"
//! 8   2+2 major, minor (u16 LE)
//! 12  4   section count N (u32 LE, N <= 32)
//! 16  16N entries: tag [u8; 4], flags u32, offset u32, len u32
//! ..      sections by ascending tag, each starting on a multiple of 8, gaps are zero bytes
//! end-32  32  blake3 of every byte before it
//! ```

use std::fmt;

use serde::de::DeserializeOwned;

use crate::error::PackageError;

/// The first eight bytes of every package.
pub const MAGIC: [u8; 8] = *b"C5UI2PKG";
/// Format major written and understood by this crate.
pub const FORMAT_MAJOR: u16 = 1;
/// Format minor written by this crate.
pub const FORMAT_MINOR: u16 = 0;
/// Most sections one package may hold.
pub const MAX_SECTIONS: usize = 32;
/// The reader's hard size limit (the converter enforces the much smaller skin budget).
pub const MAX_PACKAGE_LEN: usize = 16 << 20;
/// Section flag, bit 0: a reader that does not understand the section must refuse the package.
pub const FLAG_REQUIRED: u32 = 1;

const HEADER_LEN: usize = 16;
const ENTRY_LEN: usize = 16;
const HASH_LEN: usize = 32;
const ALIGN: usize = 8;

/// A four-byte section tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SectionTag(pub [u8; 4]);

impl SectionTag {
    /// The manifest.
    pub const MANI: SectionTag = SectionTag(*b"MANI");
    /// The string table.
    pub const STRS: SectionTag = SectionTag(*b"STRS");
    /// Node templates.
    pub const TMPL: SectionTag = SectionTag(*b"TMPL");
    /// Baked styles.
    pub const STYL: SectionTag = SectionTag(*b"STYL");
    /// Color tokens and themes.
    pub const TOKN: SectionTag = SectionTag(*b"TOKN");
    /// Bindings, expressions, intents and state.
    pub const BIND: SectionTag = SectionTag(*b"BIND");
    /// The skin's texts by language.
    pub const L10N: SectionTag = SectionTag(*b"L10N");
    /// Extension points, parts, screens, islands, hotkeys and tooltips.
    pub const EXTN: SectionTag = SectionTag(*b"EXTN");
    /// The source map.
    pub const SRCM: SectionTag = SectionTag(*b"SRCM");
    /// Font faces.
    pub const FONT: SectionTag = SectionTag(*b"FONT");
    /// Picture pages and sprites.
    pub const IMGS: SectionTag = SectionTag(*b"IMGS");
    /// Classic scripts.
    pub const SCRP: SectionTag = SectionTag(*b"SCRP");
    /// Game asset references.
    pub const AREF: SectionTag = SectionTag(*b"AREF");
}

impl fmt::Display for SectionTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for &b in &self.0 {
            let c = if b.is_ascii_graphic() { b as char } else { '?' };
            write!(f, "{c}")?;
        }
        Ok(())
    }
}

/// One row of the section table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectionEntry {
    /// Which section.
    pub tag: SectionTag,
    /// `FLAG_REQUIRED` or zero.
    pub flags: u32,
    /// Start of the payload from the start of the file.
    pub offset: u32,
    /// Payload length.
    pub len: u32,
}

/// A package's blake3 hash, the last 32 bytes of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PackageHash(pub [u8; 32]);

impl PackageHash {
    /// The first 16 hex digits, used in the file name `<id>-<short>.ui2`.
    pub fn short(&self) -> String {
        let mut s = String::with_capacity(16);
        for b in &self.0[..8] {
            s.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
            s.push(char::from_digit(u32::from(b & 15), 16).unwrap_or('0'));
        }
        s
    }
}

/// The checked outer structure of a package file.
#[derive(Debug, Clone)]
pub(crate) struct Layout {
    pub minor: u16,
    pub hash: PackageHash,
    pub entries: Vec<SectionEntry>,
}

impl Layout {
    /// The payload of `tag` and its offset in the file, if the package has it.
    pub fn payload_at<'a>(&self, bytes: &'a [u8], tag: SectionTag) -> Option<(&'a [u8], usize)> {
        let e = self.entries.iter().find(|e| e.tag == tag)?;
        let start = e.offset as usize;
        Some((bytes.get(start..start + e.len as usize)?, start))
    }

    /// The payload of `tag`, if the package has it.
    pub fn payload<'a>(&self, bytes: &'a [u8], tag: SectionTag) -> Option<&'a [u8]> {
        let e = self.entries.iter().find(|e| e.tag == tag)?;
        let start = e.offset as usize;
        bytes.get(start..start + e.len as usize)
    }
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Checks magic, version, size, hash and the section table. Never panics on any input.
pub(crate) fn parse(bytes: &[u8]) -> Result<Layout, PackageError> {
    if bytes.len() < HEADER_LEN + HASH_LEN || bytes.len() > MAX_PACKAGE_LEN {
        return Err(PackageError::Corrupt { at: None });
    }
    if bytes[..8] != MAGIC {
        return Err(PackageError::Incompatible {
            need: "a C5UI2PKG file".to_string(),
        });
    }
    let (major, minor) = (u16_at(bytes, 8), u16_at(bytes, 10));
    if major != FORMAT_MAJOR {
        return Err(PackageError::Incompatible {
            need: format!("package format {FORMAT_MAJOR}.x, the file is {major}.{minor}"),
        });
    }
    let body_end = bytes.len() - HASH_LEN;
    let mut stored = [0u8; HASH_LEN];
    stored.copy_from_slice(&bytes[body_end..]);
    if blake3::hash(&bytes[..body_end]).as_bytes() != &stored {
        return Err(PackageError::Corrupt { at: None });
    }
    let count = u32_at(bytes, 12) as usize;
    if count > MAX_SECTIONS {
        return Err(PackageError::Corrupt { at: None });
    }
    let table_end = HEADER_LEN + ENTRY_LEN * count;
    if table_end > body_end {
        return Err(PackageError::Corrupt { at: None });
    }
    let mut entries = Vec::with_capacity(count);
    let mut cursor = table_end;
    for i in 0..count {
        let at = HEADER_LEN + ENTRY_LEN * i;
        let tag = SectionTag([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
        let entry = SectionEntry {
            tag,
            flags: u32_at(bytes, at + 4),
            offset: u32_at(bytes, at + 8),
            len: u32_at(bytes, at + 12),
        };
        let bad = Err(PackageError::Corrupt { at: Some(tag) });
        if entries.last().is_some_and(|p: &SectionEntry| p.tag >= tag) {
            return bad; // not ascending, or a repeat
        }
        if entry.flags & !FLAG_REQUIRED != 0 || !(entry.offset as usize).is_multiple_of(ALIGN) {
            return bad;
        }
        let start = entry.offset as usize;
        let Some(end) = start.checked_add(entry.len as usize) else {
            return bad;
        };
        if start < cursor || end > body_end || bytes[cursor..start].iter().any(|&b| b != 0) {
            return bad; // overlap, out of the file, or non-zero padding
        }
        cursor = end;
        entries.push(entry);
    }
    if bytes[cursor..body_end].iter().any(|&b| b != 0) {
        return Err(PackageError::Corrupt { at: None });
    }
    Ok(Layout {
        minor,
        hash: PackageHash(stored),
        entries,
    })
}

/// Decodes a structural section: postcard, with no bytes left over.
pub(crate) fn decode<T: DeserializeOwned>(
    tag: SectionTag,
    payload: &[u8],
) -> Result<T, PackageError> {
    match postcard::take_from_bytes::<T>(payload) {
        Ok((value, [])) => Ok(value),
        _ => Err(PackageError::Corrupt { at: Some(tag) }),
    }
}

/// One section ready to be laid out.
#[cfg(feature = "write")]
pub(crate) struct OutSection {
    pub tag: SectionTag,
    pub required: bool,
    pub payload: Vec<u8>,
}

/// Lays the sections out in ascending tag order, 8-aligned, zero padded, and appends the hash.
/// The result is a pure function of the sections.
#[cfg(feature = "write")]
pub(crate) fn assemble(mut sections: Vec<OutSection>) -> Result<Vec<u8>, PackageError> {
    sections.sort_by_key(|s| s.tag);
    if sections.windows(2).any(|w| w[0].tag == w[1].tag) {
        return Err(PackageError::build("a section tag is written twice"));
    }
    if sections.len() > MAX_SECTIONS {
        return Err(PackageError::build("more than 32 sections"));
    }
    let too_big = || PackageError::build("the package is larger than 16 MiB");
    let table_end = HEADER_LEN + ENTRY_LEN * sections.len();
    let mut out = Vec::with_capacity(table_end);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&FORMAT_MAJOR.to_le_bytes());
    out.extend_from_slice(&FORMAT_MINOR.to_le_bytes());
    out.extend_from_slice(&(sections.len() as u32).to_le_bytes());
    let mut offset = table_end;
    let mut offsets = Vec::with_capacity(sections.len());
    for s in &sections {
        offsets.push(offset);
        let end = offset.checked_add(s.payload.len()).ok_or_else(too_big)?;
        offset = end.next_multiple_of(ALIGN);
    }
    if offset > MAX_PACKAGE_LEN {
        return Err(too_big());
    }
    for (s, &start) in sections.iter().zip(&offsets) {
        out.extend_from_slice(&s.tag.0);
        out.extend_from_slice(&u32::from(s.required).to_le_bytes());
        out.extend_from_slice(&(start as u32).to_le_bytes());
        out.extend_from_slice(&(s.payload.len() as u32).to_le_bytes());
    }
    for (s, &start) in sections.iter().zip(&offsets) {
        out.resize(start, 0);
        out.extend_from_slice(&s.payload);
    }
    out.resize(offset, 0);
    let hash = blake3::hash(&out);
    out.extend_from_slice(hash.as_bytes());
    Ok(out)
}

#[cfg(all(test, feature = "write"))]
mod tests {
    use super::*;

    fn sec(tag: &[u8; 4], required: bool, payload: &[u8]) -> OutSection {
        OutSection {
            tag: SectionTag(*tag),
            required,
            payload: payload.to_vec(),
        }
    }

    #[test]
    fn layout_is_sorted_aligned_and_padded() {
        let bytes = assemble(vec![
            sec(b"ZZZZ", true, &[7; 5]),
            sec(b"AAAA", false, &[1, 2, 3]),
        ])
        .expect("assemble");
        let layout = parse(&bytes).expect("parse");
        assert_eq!(layout.entries.len(), 2);
        assert_eq!(layout.entries[0].tag, SectionTag(*b"AAAA"));
        assert_eq!(layout.entries[0].offset, 48);
        assert_eq!(layout.entries[1].offset, 56);
        assert_eq!(layout.entries[1].flags, FLAG_REQUIRED);
        assert_eq!(
            layout.payload(&bytes, SectionTag(*b"ZZZZ")),
            Some(&[7u8; 5][..])
        );
        assert_eq!(bytes.len() % 8, 0);
    }

    #[test]
    fn nonzero_padding_is_corrupt() {
        let mut bytes = assemble(vec![sec(b"AAAA", false, &[1, 2, 3])]).expect("assemble");
        let body_end = bytes.len() - HASH_LEN;
        bytes[body_end - 1] = 9; // inside the padding after the 3-byte payload
        let hash = blake3::hash(&bytes[..body_end]);
        bytes[body_end..].copy_from_slice(hash.as_bytes());
        assert_eq!(
            parse(&bytes).err(),
            Some(PackageError::Corrupt { at: None })
        );
    }

    #[test]
    fn duplicate_tags_are_refused_by_the_writer() {
        let r = assemble(vec![sec(b"AAAA", false, &[]), sec(b"AAAA", false, &[])]);
        assert!(matches!(r, Err(PackageError::Build(_))));
    }
}
