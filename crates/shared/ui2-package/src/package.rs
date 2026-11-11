//! `Package`: a package file read and checked, with typed access to its sections.

use std::sync::Arc;

use crate::container::{self, FLAG_REQUIRED, Layout, PackageHash, Tag};
use crate::error::PackageError;
use crate::ids::StrId;
use crate::sections::manifest::Manifest;
use crate::sections::strings::StringTable;

/// Sections this reader understands. A section outside the list is skipped when the writer did
/// not mark it "reader must understand", and refused otherwise.
pub(crate) const KNOWN: &[Tag] = &[Tag::MANI, Tag::STRS];

/// Sections every package carries.
pub(crate) const PRESENT: &[Tag] = &[Tag::MANI, Tag::STRS];

/// A package that passed the structural checks of `Package::read`.
#[derive(Debug, Clone)]
pub struct Package {
    bytes: Arc<[u8]>,
    layout: Layout,
    manifest: Manifest,
    strings: StringTable,
}

impl Package {
    /// Parses and checks a package: container, hash, section table, section decoding, index
    /// ranges. Never panics on any bytes. Version and model-path checks against the client are
    /// separate (`check_compat`, `check_paths`).
    pub fn read(bytes: Arc<[u8]>) -> Result<Package, PackageError> {
        let layout = container::parse(&bytes)?;
        for e in &layout.entries {
            if !KNOWN.contains(&e.tag) && e.flags & FLAG_REQUIRED != 0 {
                return Err(PackageError::Incompatible {
                    need: format!("section {}", e.tag),
                });
            }
        }
        for &tag in PRESENT {
            if layout.payload(&bytes, tag).is_none() {
                return Err(PackageError::Corrupt { at: Some(tag) });
            }
        }
        let payload = |tag| {
            layout
                .payload(&bytes, tag)
                .ok_or(PackageError::Corrupt { at: Some(tag) })
        };
        let manifest: Manifest = container::decode(Tag::MANI, payload(Tag::MANI)?)?;
        manifest.validate(Tag::MANI)?;
        let strings: StringTable = container::decode(Tag::STRS, payload(Tag::STRS)?)?;
        strings.validate(Tag::STRS)?;
        Ok(Package {
            bytes,
            layout,
            manifest,
            strings,
        })
    }

    /// The blake3 of the file contents before the hash, as stored in the tail.
    pub fn hash(&self) -> PackageHash {
        self.layout.hash
    }

    /// The format minor the file was written with.
    pub fn format_minor(&self) -> u16 {
        self.layout.minor
    }

    /// The manifest.
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The whole file.
    pub fn bytes(&self) -> &Arc<[u8]> {
        &self.bytes
    }

    /// The string of an id, `None` when out of range.
    pub fn str(&self, id: StrId) -> Option<&str> {
        self.strings.items.get(id.index()).map(String::as_str)
    }

    /// The id of a string, `None` when the package does not contain it.
    pub fn str_id(&self, s: &str) -> Option<StrId> {
        let i = self
            .strings
            .items
            .binary_search_by(|p| p.as_str().cmp(s))
            .ok()?;
        Some(StrId(i as u32))
    }

    /// Number of strings in the table.
    pub fn str_count(&self) -> usize {
        self.strings.items.len()
    }
}
