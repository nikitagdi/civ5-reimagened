//! A parsed gr2 file: expanded sections, pointer fixups and a lazily filled type cache.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use crate::bytes::{sub, u32_at};
use crate::header::{COMPRESSION_NONE, COMPRESSION_OODLE1, Header, SectionHeader};
use crate::{GrannyError, Object, TypeDef};

/// Guards against absurd allocations from corrupt headers (largest Civ5 section is a few MB).
const MAX_SECTION_SIZE: u32 = 512 << 20;
const FIXUP_SIZE: usize = 12;
const MARSHAL_SIZE: usize = 16;

/// A location inside an expanded section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Ptr {
    pub section: u32,
    pub offset: u32,
}

impl Ptr {
    pub fn offset_by(self, delta: u32) -> Option<Ptr> {
        Some(Ptr {
            section: self.section,
            offset: self.offset.checked_add(delta)?,
        })
    }
}

impl fmt::Display for Ptr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "s{}:{:#x}", self.section, self.offset)
    }
}

pub struct GrannyFile<'a> {
    header: Header,
    sections: Vec<Cow<'a, [u8]>>,
    /// Per section, sorted by source offset.
    fixups: Vec<Vec<(u32, Ptr)>>,
    types: Mutex<HashMap<Ptr, Arc<TypeDef>>>,
}

impl fmt::Debug for GrannyFile<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GrannyFile")
            .field("header", &self.header)
            .finish_non_exhaustive()
    }
}

impl<'a> GrannyFile<'a> {
    /// Parses the container. Raw sections borrow from `data`; Oodle1 sections are expanded.
    /// `data` may extend past the file (e.g. a slice of a pack); the header's total size bounds it.
    pub fn parse(data: &'a [u8]) -> Result<Self, GrannyError> {
        let header = Header::parse(data)?;
        let data = sub(data, 0, header.total_size as usize, "file")?;
        let sections = header
            .sections
            .iter()
            .enumerate()
            .map(|(i, s)| expand_section(data, i as u32, s))
            .collect::<Result<Vec<_>, _>>()?;
        let fixups = header
            .sections
            .iter()
            .enumerate()
            .map(|(i, s)| read_fixups(data, i as u32, s, &sections))
            .collect::<Result<Vec<_>, _>>()?;
        for s in &header.sections {
            // Little-endian file on any host: marshalling (byte swaps) never applies,
            // but the table must still be inside the file.
            let len = (s.marshal_count as usize)
                .checked_mul(MARSHAL_SIZE)
                .ok_or(GrannyError::OutOfBounds("marshalling table"))?;
            sub(data, s.marshal_offset as usize, len, "marshalling table")?;
        }
        let file = Self {
            header,
            sections,
            fixups,
            types: Mutex::new(HashMap::new()),
        };
        file.bytes(file.header.root_object, 0)?;
        file.bytes(file.header.root_type, 0)?;
        Ok(file)
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    pub fn section_count(&self) -> u32 {
        self.sections.len() as u32
    }

    /// Expanded bytes of a section.
    pub fn section(&self, index: u32) -> Option<&[u8]> {
        self.sections.get(index as usize).map(|s| &**s)
    }

    pub(crate) fn section_bytes(&self, index: u32) -> Result<&[u8], GrannyError> {
        self.section(index).ok_or(GrannyError::BadPointer(Ptr {
            section: index,
            offset: 0,
        }))
    }

    /// Fixups of a section as `(source offset, target)`, sorted by source offset.
    pub fn fixups(&self, section: u32) -> &[(u32, Ptr)] {
        self.fixups.get(section as usize).map_or(&[], |v| v)
    }

    /// The pointer stored in the 4-byte slot at `slot`, if a fixup targets it (else null).
    pub fn pointer_at(&self, slot: Ptr) -> Option<Ptr> {
        let f = self.fixups.get(slot.section as usize)?;
        f.binary_search_by_key(&slot.offset, |e| e.0)
            .ok()
            .map(|i| f[i].1)
    }

    /// `len` bytes at `at`, bounds-checked.
    pub fn bytes(&self, at: Ptr, len: usize) -> Result<&[u8], GrannyError> {
        let s = self
            .section(at.section)
            .ok_or(GrannyError::BadPointer(at))?;
        sub(s, at.offset as usize, len, "pointer target").map_err(|_| GrannyError::BadPointer(at))
    }

    /// NUL-terminated byte string at `at` (without the terminator).
    pub fn c_string(&self, at: Ptr) -> Result<&[u8], GrannyError> {
        let s = self
            .section(at.section)
            .ok_or(GrannyError::BadPointer(at))?;
        let tail = s
            .get(at.offset as usize..)
            .ok_or(GrannyError::BadPointer(at))?;
        let end = tail
            .iter()
            .position(|&b| b == 0)
            .ok_or(GrannyError::UnterminatedString(at))?;
        Ok(&tail[..end])
    }

    /// Type definition at `at`, parsed once and cached.
    pub fn type_at(&self, at: Ptr) -> Result<Arc<TypeDef>, GrannyError> {
        self.type_at_depth(at, 0)
    }

    pub(crate) fn type_at_depth(&self, at: Ptr, depth: u32) -> Result<Arc<TypeDef>, GrannyError> {
        if let Some(t) = self.cache().get(&at) {
            return Ok(t.clone());
        }
        // Parsed without holding the lock: inline members recurse into this function.
        let t = Arc::new(TypeDef::parse(self, at, depth)?);
        Ok(self.cache().entry(at).or_insert(t).clone())
    }

    fn cache(&self) -> std::sync::MutexGuard<'_, HashMap<Ptr, Arc<TypeDef>>> {
        // A poisoned cache still holds only fully parsed types.
        self.types.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Every type parsed so far, sorted by location (for name harvesting and stats).
    pub fn parsed_types(&self) -> Vec<Arc<TypeDef>> {
        let mut v: Vec<_> = self.cache().values().cloned().collect();
        v.sort_by_key(|t| t.at);
        v
    }

    pub fn root_type(&self) -> Result<Arc<TypeDef>, GrannyError> {
        self.type_at(self.header.root_type)
    }

    /// The root object (`granny_file_info` in exporter output).
    pub fn root(&self) -> Result<Object<'_>, GrannyError> {
        Object::new(self, self.root_type()?, self.header.root_object)
    }
}

fn expand_section<'a>(
    data: &'a [u8],
    index: u32,
    s: &SectionHeader,
) -> Result<Cow<'a, [u8]>, GrannyError> {
    if s.expanded_size == 0 {
        return Ok(Cow::Borrowed(&[]));
    }
    if s.expanded_size > MAX_SECTION_SIZE {
        return Err(GrannyError::SectionTooLarge {
            section: index,
            size: s.expanded_size,
        });
    }
    let off = s.data_offset as usize;
    match s.compression {
        COMPRESSION_NONE => Ok(Cow::Borrowed(sub(
            data,
            off,
            s.expanded_size as usize,
            "raw section",
        )?)),
        COMPRESSION_OODLE1 => {
            let src = sub(data, off, s.data_size as usize, "compressed section")?;
            crate::oodle1::decompress(src, s.first_16bit, s.first_8bit, s.expanded_size as usize)
                .map(Cow::Owned)
        }
        kind => Err(GrannyError::UnsupportedCompression {
            section: index,
            kind,
        }),
    }
}

fn read_fixups(
    data: &[u8],
    index: u32,
    s: &SectionHeader,
    sections: &[Cow<'_, [u8]>],
) -> Result<Vec<(u32, Ptr)>, GrannyError> {
    let len = (s.fixup_count as usize)
        .checked_mul(FIXUP_SIZE)
        .ok_or(GrannyError::OutOfBounds("fixup table"))?;
    let table = sub(data, s.fixup_offset as usize, len, "fixup table")?;
    let own_len = sections.get(index as usize).map_or(0, |d| d.len());
    let mut out = Vec::with_capacity(s.fixup_count as usize);
    for (k, e) in table.chunks_exact(FIXUP_SIZE).enumerate() {
        let bad = |reason| GrannyError::BadFixup {
            section: index,
            index: k as u32,
            reason,
        };
        let src = u32_at(e, 0, "fixup")?;
        let dst = Ptr {
            section: u32_at(e, 4, "fixup")?,
            offset: u32_at(e, 8, "fixup")?,
        };
        if (src as usize).saturating_add(4) > own_len {
            return Err(bad("source out of range"));
        }
        let dst_len = sections
            .get(dst.section as usize)
            .ok_or(bad("target section out of range"))?
            .len();
        if dst.offset as usize > dst_len {
            return Err(bad("target out of range"));
        }
        out.push((src, dst));
    }
    // Stable sort + keep-first makes duplicate slots deterministic.
    out.sort_by_key(|e| e.0);
    out.dedup_by_key(|e| e.0);
    Ok(out)
}
