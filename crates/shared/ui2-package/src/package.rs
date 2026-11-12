//! `Package`: a package file read and checked, with typed access to its sections.

use std::ops::Range;
use std::sync::Arc;

use ruzstd::decoding::FrameDecoder;

use crate::container::{self, FLAG_REQUIRED, Layout, PackageHash, SectionTag};
use crate::error::PackageError;
use crate::ids::{ARefId, ExprId, NodeRef, StrId, StyleSetId, TemplateId, TplId};
use crate::profile::expr::Expr;
use crate::profile::{SchemaView, TypeSig, resolve_path};
use crate::sections::bindings::{Binding, Bindings, Lit, Seg};
use crate::sections::blobs::{
    self, ARefs, BlobRef, BlobSection, Face, Fonts, Images, Page, Scripts,
};
use crate::sections::extn::{Extn, Hotkey, Island};
use crate::sections::l10n::L10n;
use crate::sections::manifest::Manifest;
use crate::sections::srcmap::SrcMap;
use crate::sections::strings::StringTable;
use crate::sections::styles::{StylePatches, StyleSet, Styles};
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
    SectionTag::STYP,
    SectionTag::TOKN,
    SectionTag::BIND,
    SectionTag::L10N,
    SectionTag::EXTN,
    SectionTag::SRCM,
    SectionTag::FONT,
    SectionTag::IMGS,
    SectionTag::SCRP,
    SectionTag::AREF,
];

/// Sections every package carries (`IMGS`, `SCRP`, `AREF` and `SRCM` only when the skin has any).
pub(crate) const PRESENT: &[SectionTag] = &[
    SectionTag::MANI,
    SectionTag::STRS,
    SectionTag::TMPL,
    SectionTag::STYL,
    SectionTag::TOKN,
    SectionTag::BIND,
    SectionTag::L10N,
    SectionTag::EXTN,
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
    bindings: Bindings,
    l10n: L10n,
    extn: Extn,
    srcmap: Option<SrcMap>,
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
        let mut styles: Styles = container::decode(SectionTag::STYL, section(SectionTag::STYL)?)?;
        if let Some(p) = layout.payload(&bytes, SectionTag::STYP) {
            let patches: StylePatches = container::decode(SectionTag::STYP, p)?;
            patches.expand(SectionTag::STYP, &mut styles)?;
        }
        let tokens: Tokens = container::decode(SectionTag::TOKN, section(SectionTag::TOKN)?)?;
        let bindings: Bindings = container::decode(SectionTag::BIND, section(SectionTag::BIND)?)?;
        let l10n: L10n = container::decode(SectionTag::L10N, section(SectionTag::L10N)?)?;
        let extn: Extn = container::decode(SectionTag::EXTN, section(SectionTag::EXTN)?)?;
        let srcmap: Option<SrcMap> = match layout.payload(&bytes, SectionTag::SRCM) {
            Some(p) => Some(container::decode(SectionTag::SRCM, p)?),
            None => None,
        };

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
            bindings: &bindings,
            l10n: &l10n,
            extn: &extn,
            srcmap: srcmap.as_ref(),
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
            bindings,
            l10n,
            extn,
            srcmap,
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

    /// The bindings section as stored.
    pub fn bindings(&self) -> &Bindings {
        &self.bindings
    }

    /// One expression tree.
    pub fn expr(&self, id: ExprId) -> Option<&Expr> {
        self.bindings.exprs.get(id.index())
    }

    /// The pieces of one text template.
    pub fn tpl(&self, id: TplId) -> Option<&[Seg]> {
        self.bindings.tpls.get(id.index()).map(Vec::as_slice)
    }

    /// The bindings of one node, in the order they were written; empty when it has none.
    pub fn bindings_of(&self, tpl: TemplateId, node: u32) -> &[Binding] {
        let items = &self.bindings.items;
        let key = |b: &Binding| (b.tpl.index(), b.node);
        let from = items.partition_point(|b| key(b) < (tpl.index(), node));
        let to = items.partition_point(|b| key(b) <= (tpl.index(), node));
        &items[from..to]
    }

    /// The skin's state paths (without `state.`) with their default values, ascending.
    pub fn state_defaults(&self) -> impl Iterator<Item = (&str, &Lit)> + '_ {
        self.bindings
            .state
            .iter()
            .filter_map(|(path, lit)| Some((self.str(*path)?, lit)))
    }

    /// The model paths the skin reads and the types it expects, ascending by path.
    pub fn used_paths(&self) -> impl Iterator<Item = (&str, &TypeSig)> + '_ {
        self.bindings
            .used_paths
            .iter()
            .filter_map(|(path, ty)| Some((self.str(*path)?, ty)))
    }

    /// The catalogue intents the skin sends with their argument types.
    pub fn intents(&self) -> impl Iterator<Item = (&str, &[TypeSig])> + '_ {
        self.bindings
            .intents
            .iter()
            .filter_map(|(name, args)| Some((self.str(*name)?, args.as_slice())))
    }

    /// The script handlers markup calls, ascending.
    pub fn handlers(&self) -> impl Iterator<Item = &str> + '_ {
        self.bindings.handlers.iter().filter_map(|h| self.str(*h))
    }

    /// The languages of the skin's texts; the first is the fallback.
    pub fn languages(&self) -> &[String] {
        &self.manifest.languages
    }

    /// The text template of a key in a language, `None` when either is unknown.
    pub fn l10n(&self, lang: &str, key: &str) -> Option<&str> {
        self.l10n.get(lang, self.str_id(key)?)
    }

    /// The extension points for mods and their nodes, ascending by name.
    pub fn slots(&self) -> impl Iterator<Item = (&str, NodeRef)> + '_ {
        self.named(&self.extn.slots)
    }

    /// The named parts and their nodes, ascending by name.
    pub fn parts(&self) -> impl Iterator<Item = (&str, NodeRef)> + '_ {
        self.named(&self.extn.parts)
    }

    /// The screens the skin implements and their root nodes, ascending by id.
    pub fn screens(&self) -> impl Iterator<Item = (&str, NodeRef)> + '_ {
        self.named(&self.extn.screens)
    }

    fn named<'a>(
        &'a self,
        list: &'a [(StrId, NodeRef)],
    ) -> impl Iterator<Item = (&'a str, NodeRef)> + 'a {
        list.iter()
            .filter_map(|(n, node)| Some((self.str(*n)?, *node)))
    }

    /// The script islands, ascending by id.
    pub fn islands(&self) -> &[Island] {
        &self.extn.islands
    }

    /// The hotkeys, ascending by `(code, mods, node)`.
    pub fn hotkeys(&self) -> &[Hotkey] {
        &self.extn.hotkeys
    }

    /// The tooltip templates: the node that shows one and the template, ascending by node.
    pub fn tooltips(&self) -> &[(NodeRef, TemplateId)] {
        &self.extn.tooltips
    }

    /// Where a node came from in the skin's source: file, line, column. `None` without a map.
    pub fn source_loc(&self, tpl: TemplateId, node: u32) -> Option<(&str, u32, u16)> {
        let map = self.srcmap.as_ref()?;
        let loc = map.templates.get(tpl.index())?.get(node as usize)?;
        Some((
            map.files.get(usize::from(loc.file))?.as_str(),
            loc.line,
            loc.col,
        ))
    }

    /// Checks that every model path and intent the skin uses exists in the client's schema with
    /// the same type. The host calls this before it replaces the running skin.
    pub fn check_paths(&self, view: &dyn SchemaView) -> Result<(), PackageError> {
        for (path, want) in self.used_paths() {
            if resolve_path(view, path).as_ref() != Some(want) {
                return Err(PackageError::Incompatible {
                    need: format!("model path {path} of type {want}"),
                });
            }
        }
        for (name, want) in self.intents() {
            let have = view
                .intent(name)
                .map(|args| args.into_iter().map(|(_, t)| t).collect::<Vec<_>>());
            if have.as_deref() != Some(want) {
                return Err(PackageError::Incompatible {
                    need: format!("intent {name}"),
                });
            }
        }
        Ok(())
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
