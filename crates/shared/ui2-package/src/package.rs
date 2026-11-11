//! `Package`: a package file read and checked, with typed access to its sections.

use std::ops::Range;
use std::sync::Arc;

use ruzstd::decoding::FrameDecoder;

use crate::container::{self, FLAG_REQUIRED, Layout, PackageHash, SectionTag};
use crate::error::PackageError;
use crate::ids::{ARefId, StrId, StyleSetId, TemplateId};
use crate::sections::blobs::{
    self, ARefs, BlobRef, BlobSection, Face, Fonts, Images, Page, Scripts,
};
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
    SectionTag::FONT,
    SectionTag::IMGS,
    SectionTag::SCRP,
    SectionTag::AREF,
];

/// Sections every package carries (`IMGS`, `SCRP` and `AREF` only when the skin has any).
pub(crate) const PRESENT: &[SectionTag] = &[
    SectionTag::MANI,
    SectionTag::STRS,
    SectionTag::TMPL,
    SectionTag::STYL,
    SectionTag::TOKN,
    SectionTag::FONT,
];

/// A decompressed frame may declare a window this large (zstd level 19 uses up to 8 MiB).
const MIN_WINDOW_LIMIT: u64 = 8 << 20;

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
    fonts: Fonts,
    images: Images,
    scripts: Scripts,
    arefs: ARefs,
    /// The byte areas of `FONT`, `IMGS` and `SCRP`, as ranges of `bytes`.
    areas: [Range<usize>; 3],
}

fn decompress(at: SectionTag, stored: &[u8], raw_len: usize) -> Result<Vec<u8>, PackageError> {
    let corrupt = || PackageError::Corrupt { at: Some(at) };
    let mut decoder = FrameDecoder::new();
    decoder.set_max_window_size((raw_len as u64).max(MIN_WINDOW_LIMIT));
    // The capacity is the hard cap: the decoder never grows the vector.
    let mut out = Vec::with_capacity(raw_len);
    decoder
        .decode_all_to_vec(stored, &mut out)
        .map_err(|_| corrupt())?;
    if out.len() != raw_len {
        return Err(corrupt());
    }
    if let (Some(stored), Some(calc)) = (
        decoder.get_checksum_from_data(),
        decoder.get_calculated_checksum(),
    ) && stored != calc
    {
        return Err(corrupt());
    }
    Ok(out)
}

/// Decodes the index of a blob section and finds the range of its byte area in the file; `None`
/// when the package has no such section.
fn blob_section<T: serde::de::DeserializeOwned>(
    layout: &Layout,
    bytes: &[u8],
    tag: SectionTag,
) -> Result<Option<(T, Range<usize>)>, PackageError> {
    let Some((payload, at)) = layout.payload_at(bytes, tag) else {
        return Ok(None);
    };
    let (index, area) = blobs::split(tag, payload)?;
    Ok(Some((
        container::decode(tag, index)?,
        at + area..at + payload.len(),
    )))
}
impl Package {
    /// Parses and checks a package: container, hash, section table, section decoding, index
    /// ranges. Never panics on any bytes. Blob contents are not decompressed here (see
    /// `verify_blobs`); version and model-path checks against the client are separate
    /// (`check_compat`, `check_paths`).
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

        let (fonts, font_area): (Fonts, _) =
            blob_section(&layout, &bytes, SectionTag::FONT)?.unwrap_or_default();
        let (images, image_area): (Images, _) =
            blob_section(&layout, &bytes, SectionTag::IMGS)?.unwrap_or_default();
        let (scripts, script_area): (Scripts, _) =
            blob_section(&layout, &bytes, SectionTag::SCRP)?.unwrap_or_default();
        let arefs: ARefs = match layout.payload(&bytes, SectionTag::AREF) {
            Some(p) => container::decode(SectionTag::AREF, p)?,
            None => ARefs::default(),
        };

        validate::check(&Parts {
            manifest: &manifest,
            strings: &strings,
            templates: &templates,
            styles: &styles,
            tokens: &tokens,
            fonts: &fonts,
            images: &images,
            scripts: &scripts,
            arefs: &arefs,
            font_area: font_area.len(),
            image_area: image_area.len(),
            script_area: script_area.len(),
        })?;
        Ok(Package {
            bytes,
            layout,
            manifest,
            strings,
            templates,
            styles,
            tokens,
            fonts,
            images,
            scripts,
            arefs,
            areas: [font_area, image_area, script_area],
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

    /// The font faces, for the text stack.
    pub fn fonts(&self) -> &[Face] {
        &self.fonts.faces
    }

    /// The picture pages and sprites.
    pub fn images(&self) -> &Images {
        &self.images
    }

    /// The skin's scripts in execution order: path and bytes (as written, never compressed).
    pub fn scripts(&self) -> impl Iterator<Item = (&str, &[u8])> + '_ {
        self.scripts
            .files
            .iter()
            .map(|(path, blob)| (self.str(*path).unwrap_or(""), self.blob(*blob)))
    }

    /// The model roots the scripts read.
    pub fn script_reads(&self) -> impl Iterator<Item = &str> + '_ {
        self.scripts.reads.iter().filter_map(|r| self.str(*r))
    }

    /// The game asset references (`portrait:...`), ascending.
    pub fn asset_refs(&self) -> impl Iterator<Item = &str> + '_ {
        self.arefs.items.iter().filter_map(|r| self.str(*r))
    }

    /// One game asset reference by id.
    pub fn asset_ref(&self, id: ARefId) -> Option<&str> {
        self.arefs.items.get(id.index()).and_then(|r| self.str(*r))
    }

    /// The stored bytes of a blob, a slice of the file (compressed when `blob.zstd`). Never
    /// copies; an out-of-range reference (impossible for a blob of this package) gives an empty
    /// slice.
    pub fn blob(&self, blob: BlobRef) -> &[u8] {
        let area = match blob.section {
            BlobSection::Font => &self.areas[0],
            BlobSection::Imgs => &self.areas[1],
            BlobSection::Scrp => &self.areas[2],
        };
        let start = area.start.saturating_add(blob.offset as usize);
        let end = start.saturating_add(blob.len as usize);
        if end > area.end {
            return &[];
        }
        self.bytes.get(start..end).unwrap_or(&[])
    }

    /// The bytes of a blob after decompression, as a shared buffer. A stored (not compressed)
    /// blob is copied once. A damaged zstd frame, or output of another length than `raw_len`, is
    /// `Corrupt` in the blob's section.
    pub fn blob_owned(&self, blob: BlobRef) -> Result<Arc<[u8]>, PackageError> {
        let stored = self.blob(blob);
        if !blob.zstd {
            return Ok(Arc::from(stored));
        }
        decompress(blob.section.tag(), stored, blob.raw_len as usize).map(Arc::from)
    }

    /// The font file of a face, decompressed and checked against the face's blake3.
    pub fn font_data(&self, face: &Face) -> Result<Arc<[u8]>, PackageError> {
        let data = self.blob_owned(face.blob)?;
        if blake3::hash(&data).as_bytes() != &face.blake3 {
            return Err(PackageError::Corrupt {
                at: Some(SectionTag::FONT),
            });
        }
        Ok(data)
    }

    /// The pixels of a picture page, decompressed and checked against the page's blake3.
    pub fn page_data(&self, page: &Page) -> Result<Arc<[u8]>, PackageError> {
        let data = self.blob_owned(page.blob)?;
        if blake3::hash(&data).as_bytes() != &page.blake3 {
            return Err(PackageError::Corrupt {
                at: Some(SectionTag::IMGS),
            });
        }
        Ok(data)
    }

    /// Decompresses every font and page and checks the hashes. `read` does not do this (it would
    /// decompress everything twice); the host calls it, or `font_data` and `page_data` per blob.
    pub fn verify_blobs(&self) -> Result<(), PackageError> {
        for face in &self.fonts.faces {
            self.font_data(face)?;
        }
        for page in &self.images.pages {
            self.page_data(page)?;
        }
        Ok(())
    }
}
