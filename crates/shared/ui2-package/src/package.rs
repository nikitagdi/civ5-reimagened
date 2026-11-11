//! `Package`: a package file read and checked, with typed access to its sections.

use std::sync::Arc;

use crate::container::{self, FLAG_REQUIRED, Layout, PackageHash, SectionTag};
use crate::error::PackageError;
use crate::ids::{StrId, StyleSetId, TemplateId};
use crate::sections::manifest::Manifest;
use crate::sections::strings::StringTable;
use crate::sections::styles::{StyleSet, Styles};
use crate::sections::templates::{Template, Templates};
use crate::sections::tokens::Tokens;
use crate::validate::{self, Parts};

/// Sections this reader understands. A section outside the list is skipped when the writer did
/// not mark it "reader must understand", and refused otherwise.
pub(crate) const KNOWN: &[SectionTag] = &[
    SectionTag::MANI,
    SectionTag::STRS,
    SectionTag::TMPL,
    SectionTag::STYL,
    SectionTag::TOKN,
];

/// Sections every package carries.
pub(crate) const PRESENT: &[SectionTag] = KNOWN;

/// A package that passed the structural checks of `Package::read`.
#[derive(Debug, Clone)]
pub struct Package {
    bytes: Arc<[u8]>,
    layout: Layout,
    manifest: Manifest,
    strings: StringTable,
    templates: Templates,
    styles: Styles,
    tokens: Tokens,
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
        let section = |tag| {
            layout
                .payload(&bytes, tag)
                .ok_or(PackageError::Corrupt { at: Some(tag) })
        };
        let manifest: Manifest = container::decode(SectionTag::MANI, section(SectionTag::MANI)?)?;
        let strings: StringTable = container::decode(SectionTag::STRS, section(SectionTag::STRS)?)?;
        let templates: Templates = container::decode(SectionTag::TMPL, section(SectionTag::TMPL)?)?;
        let styles: Styles = container::decode(SectionTag::STYL, section(SectionTag::STYL)?)?;
        let tokens: Tokens = container::decode(SectionTag::TOKN, section(SectionTag::TOKN)?)?;
        validate::check(&Parts {
            manifest: &manifest,
            strings: &strings,
            templates: &templates,
            styles: &styles,
            tokens: &tokens,
        })?;
        Ok(Package {
            bytes,
            layout,
            manifest,
            strings,
            templates,
            styles,
            tokens,
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

    /// The templates; template 0 is the document.
    pub fn templates(&self) -> &[Template] {
        &self.templates.items
    }

    /// One template.
    pub fn template(&self, id: TemplateId) -> Option<&Template> {
        self.templates.items.get(id.index())
    }

    /// The baked styles: breakpoints, records and sets. The runtime looks styles up through
    /// `Styles::cell` and `StyleSet::get` only.
    pub fn styles(&self) -> &Styles {
        &self.styles
    }

    /// One style set.
    pub fn style_set(&self, id: StyleSetId) -> Option<&StyleSet> {
        self.styles.sets.get(id.index())
    }

    /// The color tokens and themes.
    pub fn tokens(&self) -> &Tokens {
        &self.tokens
    }
}
