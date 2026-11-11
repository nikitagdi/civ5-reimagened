//! `PackageBuilder`: the only way a package is written.
//!
//! `finish` is a pure function of what was added: strings are sorted, sections are laid out by
//! tag, and no collection with an unspecified order takes part. Adding the same strings in another
//! order gives the same bytes. Things whose order carries meaning (templates, style records and
//! sets, tokens) keep the order the caller adds them in; that order must itself be deterministic
//! (document order).
//!
//! The finished bytes are read back with `Package::read` before they are returned, so the builder
//! never emits a package the reader would refuse.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde::Serialize;

use crate::container::{OutSection, SectionTag, assemble};
use crate::error::PackageError;
use crate::ids::{ARefId, ExprId, ImgId, NodeRef, StrId, StyleId, StyleSetId, TemplateId, TplId};
use crate::package::Package;
use crate::profile::TypeSig;
use crate::profile::expr::Expr;
use crate::remap::{Remap, Remapped};
use crate::sections::bindings::{Binding, Bindings, Lit, Seg};
use crate::sections::blobs::{
    self, ARefs, BlobRef, BlobSection, Face, Fonts, Images, Img, Page, Scripts,
};
use crate::sections::extn::{Extn, Hotkey, Island};
use crate::sections::l10n::{L10n, Lang};
use crate::sections::manifest::Manifest;
use crate::sections::srcmap::{SrcLoc, SrcMap};
use crate::sections::strings::StrPool;
use crate::sections::styles::{Bound, StyleSet, Styles};
use crate::sections::templates::{Template, Templates};
use crate::sections::tokens::{Theme, Tokens};
use crate::style::StyleRecord;

/// An island as the caller gave it: id, root node and keyed nodes.
type IslandDraft = (String, NodeRef, Vec<(String, u32)>);

/// Collects the parts of a package and writes it. Ids handed out by the builder's methods are
/// provisional where the final table is sorted (strings): they are valid for the data added to
/// this builder and are rewritten when the package is finished.
#[derive(Debug, Default)]
pub struct PackageBuilder {
    pool: StrPool,
    templates: Vec<Template>,
    records: Vec<StyleRecord>,
    record_ids: BTreeMap<Vec<u8>, StyleId>,
    sets: Vec<StyleSet>,
    set_ids: BTreeMap<Vec<u8>, StyleSetId>,
    media_w: Vec<Bound>,
    media_h: Vec<Bound>,
    tokens: Tokens,
    fonts: Vec<(Face, Vec<u8>)>,
    pages: Vec<(Page, Vec<u8>)>,
    imgs: Vec<Img>,
    scripts: Vec<(StrId, Vec<u8>)>,
    script_reads: Vec<StrId>,
    arefs: Vec<StrId>,
    exprs: Vec<Expr>,
    expr_ids: BTreeMap<Vec<u8>, ExprId>,
    tpls: Vec<Vec<Seg>>,
    tpl_ids: BTreeMap<Vec<u8>, TplId>,
    bindings: Vec<Binding>,
    used_paths: BTreeMap<String, TypeSig>,
    intents: BTreeMap<String, Vec<TypeSig>>,
    state: BTreeMap<String, Lit>,
    handlers: BTreeSet<String>,
    l10n: BTreeMap<String, BTreeMap<String, String>>,
    slots: Vec<(String, NodeRef)>,
    parts: Vec<(String, NodeRef)>,
    screens: Vec<(String, NodeRef)>,
    islands: Vec<IslandDraft>,
    hotkeys: Vec<(String, u8, NodeRef)>,
    tooltips: Vec<(NodeRef, TemplateId)>,
    srcmap: Option<(Vec<String>, Vec<Vec<SrcLoc>>)>,
    error: Option<String>,
}

impl PackageBuilder {
    /// An empty builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a string to the table (or finds it) and returns its provisional id.
    pub fn str(&mut self, s: &str) -> StrId {
        self.pool.intern(s)
    }

    /// Adds a template and returns its id (insertion order; template 0 is the document).
    pub fn template(&mut self, template: Template) -> TemplateId {
        self.templates.push(template);
        TemplateId((self.templates.len() - 1) as u32)
    }

    /// Adds a style record, or finds the equal one; ids follow first use.
    pub fn style_record(&mut self, record: StyleRecord) -> StyleId {
        let key = self.key(&record);
        if let Some(&id) = self.record_ids.get(&key) {
            return id;
        }
        let id = StyleId(self.records.len() as u32);
        self.records.push(record);
        self.record_ids.insert(key, id);
        id
    }

    /// Adds a style set, or finds the equal one; ids follow first use.
    pub fn style_set(&mut self, set: StyleSet) -> StyleSetId {
        let key = self.key(&set);
        if let Some(&id) = self.set_ids.get(&key) {
            return id;
        }
        let id = StyleSetId(self.sets.len() as u32);
        self.sets.push(set);
        self.set_ids.insert(key, id);
        id
    }

    /// Sets the viewport breakpoints (ascending, at most 16 cells).
    pub fn media(&mut self, width: Vec<Bound>, height: Vec<Bound>) {
        self.media_w = width;
        self.media_h = height;
    }

    /// Sets the color tokens and themes. When no themes are given, one empty theme is made per
    /// theme of the manifest.
    pub fn tokens(&mut self, tokens: Tokens) {
        self.tokens = tokens;
    }

    /// Adds a font face. `bytes` is the font file as stored: when `face.blob.zstd` it is one zstd
    /// frame of `raw_len` bytes (the converter compresses), otherwise the file itself. The blob
    /// position is filled in by the builder; `face.blake3` is the hash of the decompressed file
    /// and is verified at `finish`.
    pub fn font(&mut self, face: Face, bytes: &[u8], raw_len: u32) {
        self.fonts.push((face, bytes.to_vec()));
        if let Some((face, bytes)) = self.fonts.last_mut() {
            face.blob = stored(BlobSection::Font, face.blob.zstd, bytes.len(), raw_len);
        }
    }

    /// Adds a picture page (same rules as `font`) and returns its index; pages keep the order
    /// they are added in, so the packer must be deterministic.
    pub fn image_page(&mut self, page: Page, bytes: &[u8], raw_len: u32) -> u16 {
        self.pages.push((page, bytes.to_vec()));
        if let Some((page, bytes)) = self.pages.last_mut() {
            page.blob = stored(BlobSection::Imgs, page.blob.zstd, bytes.len(), raw_len);
        }
        (self.pages.len() - 1) as u16
    }

    /// Adds a picture and returns its provisional id; pictures end up sorted by path.
    pub fn image(&mut self, img: Img) -> ImgId {
        self.imgs.push(img);
        ImgId((self.imgs.len() - 1) as u32)
    }

    /// Adds a script. Scripts run in the order they are added; the bytes are stored as they are.
    pub fn script(&mut self, path: &str, bytes: &[u8]) {
        let path = self.pool.intern(path);
        self.scripts.push((path, bytes.to_vec()));
    }

    /// Sets the model roots the scripts read.
    pub fn script_reads(&mut self, roots: &[&str]) {
        self.script_reads = roots.iter().map(|r| self.pool.intern(r)).collect();
    }

    /// Adds a game asset reference (`portrait:...`) and returns its provisional id; the final
    /// table is sorted and has no repeats.
    pub fn asset_ref(&mut self, name: &str) -> ARefId {
        let id = self.pool.intern(name);
        let at = match self.arefs.iter().position(|r| *r == id) {
            Some(at) => at,
            None => {
                self.arefs.push(id);
                self.arefs.len() - 1
            }
        };
        ARefId(at as u32)
    }
    /// Adds an expression tree, or finds the equal one; ids follow first use.
    pub fn expr(&mut self, expr: Expr) -> ExprId {
        let key = self.key(&expr);
        if let Some(&id) = self.expr_ids.get(&key) {
            return id;
        }
        let id = ExprId(self.exprs.len() as u32);
        self.exprs.push(expr);
        self.expr_ids.insert(key, id);
        id
    }

    /// Adds a text template, or finds the equal one; ids follow first use. Literal pieces hold
    /// provisional string ids from `str`.
    pub fn tpl(&mut self, segs: Vec<Seg>) -> TplId {
        let key = self.key(&segs);
        if let Some(&id) = self.tpl_ids.get(&key) {
            return id;
        }
        let id = TplId(self.tpls.len() as u32);
        self.tpls.push(segs);
        self.tpl_ids.insert(key, id);
        id
    }

    /// Adds a binding. Bindings end up ordered by node; several of one node keep the order they
    /// were added in.
    pub fn binding(&mut self, binding: Binding) {
        self.bindings.push(binding);
    }

    /// Records a model path the skin reads and the type it expects (`top.units[].hp`).
    pub fn used_path(&mut self, path: &str, ty: TypeSig) {
        match self.used_paths.get(path) {
            Some(old) if *old != ty => self.fail(format!("the path {path} is used with two types")),
            _ => {
                self.used_paths.insert(path.to_string(), ty);
            }
        }
    }

    /// Records a catalogue intent the skin sends, with its argument types.
    pub fn intent(&mut self, name: &str, args: Vec<TypeSig>) {
        match self.intents.get(name) {
            Some(old) if *old != args => {
                self.fail(format!("the intent {name} is used with two signatures"))
            }
            _ => {
                self.intents.insert(name.to_string(), args);
            }
        }
    }

    /// Records a skin state path (without `state.`) and its default value.
    pub fn state_default(&mut self, path: &str, value: Lit) {
        match self.state.get(path) {
            Some(old) if *old != value => {
                self.fail(format!("the state path {path} has two defaults"))
            }
            _ => {
                self.state.insert(path.to_string(), value);
            }
        }
    }

    /// Records a script handler markup calls.
    pub fn handler(&mut self, name: &str) {
        self.handlers.insert(name.to_string());
    }

    /// Adds a text of a language. The language must be one of the manifest's.
    pub fn l10n(&mut self, lang: &str, key: &str, template: &str) {
        let texts = self.l10n.entry(lang.to_string()).or_default();
        match texts.get(key) {
            Some(old) if old != template => {
                let msg = format!("the text {key} of {lang} is given twice");
                self.fail(msg);
            }
            _ => {
                texts.insert(key.to_string(), template.to_string());
            }
        }
    }

    /// Adds an extension point for mods.
    pub fn slot(&mut self, name: &str, node: NodeRef) {
        self.slots.push((name.to_string(), node));
    }

    /// Adds a named part.
    pub fn part(&mut self, name: &str, node: NodeRef) {
        self.parts.push((name.to_string(), node));
    }

    /// Adds a screen the skin implements.
    pub fn screen(&mut self, id: &str, node: NodeRef) {
        self.screens.push((id.to_string(), node));
    }

    /// Adds a script island with its keyed nodes (key and node index in the root's template).
    pub fn island(&mut self, id: &str, node: NodeRef, keys: &[(&str, u32)]) {
        let keys = keys.iter().map(|(k, n)| ((*k).to_string(), *n)).collect();
        self.islands.push((id.to_string(), node, keys));
    }

    /// Adds a hotkey (`code` is a `KeyboardEvent.code`, `mods` are `hotkey_mods` bits).
    pub fn hotkey(&mut self, code: &str, mods: u8, node: NodeRef) {
        self.hotkeys.push((code.to_string(), mods, node));
    }

    /// Adds a tooltip: the node that shows the template.
    pub fn tooltip(&mut self, node: NodeRef, template: TemplateId) {
        self.tooltips.push((node, template));
    }

    /// Sets the source map: the source files and, per template, one place per node (`file`
    /// indexes `files`). The builder sorts the files and rewrites the indices, so the order the
    /// caller names them in does not reach the bytes.
    pub fn source_map(&mut self, files: &[&str], places: Vec<Vec<SrcLoc>>) {
        let files = files.iter().map(|f| (*f).to_string()).collect();
        self.srcmap = Some((files, places));
    }

    fn fail(&mut self, message: String) {
        self.error.get_or_insert(message);
    }

    /// Stores a parsed value in `record`, interning font family names in this builder.
    #[cfg(feature = "profile")]
    pub fn set_value(
        &mut self,
        record: &mut StyleRecord,
        prop: crate::profile::PropId,
        value: &crate::profile::Value,
    ) -> Result<(), crate::style::StyleError> {
        record.set_value(prop, value, &mut |s| self.pool.intern(s))
    }

    fn key<T: Serialize>(&mut self, value: &T) -> Vec<u8> {
        match postcard::to_allocvec(value) {
            Ok(bytes) => bytes,
            Err(e) => {
                self.error
                    .get_or_insert_with(|| format!("encoding failed: {e}"));
                Vec::new()
            }
        }
    }

    /// Writes the package. The manifest is checked first (id grammar, themes and languages); the
    /// result is read back and checked like any package.
    pub fn finish(mut self, manifest: Manifest) -> Result<Vec<u8>, PackageError> {
        if let Some(e) = self.error.take() {
            return Err(PackageError::build(e));
        }
        manifest.validate(SectionTag::MANI)?;
        if self.tokens.themes.is_empty() {
            for name in &manifest.themes {
                let name = self.pool.intern(name);
                self.tokens.themes.push(Theme {
                    name,
                    set: Vec::new(),
                });
            }
        }
        if let Some(lang) = self.l10n.keys().find(|l| !manifest.languages.contains(l)) {
            return Err(PackageError::build(format!(
                "texts are given for {lang}, which the manifest does not list"
            )));
        }

        // The strings of the sorted tables enter the pool before it is sealed. A final string id
        // follows the byte order of the string, so each table is sorted by its strings here.
        let mut used_paths: Vec<(StrId, TypeSig)> = Vec::new();
        for (path, ty) in std::mem::take(&mut self.used_paths) {
            used_paths.push((self.pool.intern(&path), ty));
        }
        let mut intents: Vec<(StrId, Vec<TypeSig>)> = Vec::new();
        for (name, args) in std::mem::take(&mut self.intents) {
            intents.push((self.pool.intern(&name), args));
        }
        let mut state: Vec<(StrId, Lit)> = Vec::new();
        for (path, lit) in std::mem::take(&mut self.state) {
            state.push((self.pool.intern(&path), lit));
        }
        let mut handlers: Vec<StrId> = Vec::new();
        for name in std::mem::take(&mut self.handlers) {
            handlers.push(self.pool.intern(&name));
        }
        let mut langs: Vec<Lang> = Vec::new();
        for tag in &manifest.languages {
            let mut entries = Vec::new();
            for (key, text) in self.l10n.remove(tag).unwrap_or_default() {
                entries.push((self.pool.intern(&key), text));
            }
            langs.push(Lang {
                tag: tag.clone(),
                entries,
            });
        }
        let mut named = |list: &mut Vec<(String, NodeRef)>| -> Vec<(StrId, NodeRef)> {
            list.sort();
            list.iter()
                .map(|(n, node)| (self.pool.intern(n), *node))
                .collect()
        };
        let mut extn = Extn {
            slots: named(&mut self.slots),
            parts: named(&mut self.parts),
            screens: named(&mut self.screens),
            ..Extn::default()
        };
        self.islands.sort();
        for (id, node, mut keys) in std::mem::take(&mut self.islands) {
            keys.sort();
            extn.islands.push(Island {
                id: self.pool.intern(&id),
                node,
                keys: keys
                    .into_iter()
                    .map(|(k, n)| (self.pool.intern(&k), n))
                    .collect(),
            });
        }
        self.hotkeys.sort();
        for (code, mods, node) in std::mem::take(&mut self.hotkeys) {
            extn.hotkeys.push(Hotkey {
                code: self.pool.intern(&code),
                mods,
                node,
            });
        }
        self.tooltips.sort();
        extn.tooltips = std::mem::take(&mut self.tooltips);

        let (strings, strs) = self.pool.finish();
        let mut remap = Remap {
            strs,
            ..Remap::default()
        };

        // Sorted tables take their final order from the final string ids.
        let mut imgs = self.imgs;
        for img in &mut imgs {
            img.remap(&remap);
        }
        remap.imgs = sorted_positions(imgs.iter().map(|i| i.path));
        let mut sorted_imgs = imgs.clone();
        for (provisional, img) in imgs.into_iter().enumerate() {
            sorted_imgs[remap.imgs[provisional] as usize] = img;
        }
        let mut arefs = self.arefs;
        for a in &mut arefs {
            remap.str(a);
        }
        remap.arefs = sorted_positions(arefs.iter().copied());
        let mut sorted_arefs = arefs.clone();
        for (provisional, a) in arefs.into_iter().enumerate() {
            sorted_arefs[remap.arefs[provisional] as usize] = a;
        }

        let mut templates = Templates {
            items: self.templates,
        };
        for t in &mut templates.items {
            t.remap(&remap);
        }
        let mut styles = Styles {
            media_w: self.media_w,
            media_h: self.media_h,
            records: self.records,
            sets: self.sets,
        };
        for r in &mut styles.records {
            r.remap(&remap);
        }
        for s in &mut styles.sets {
            s.remap(&remap);
        }
        let mut tokens = self.tokens;
        tokens.remap(&remap);

        for (id, _) in &mut used_paths {
            remap.str(id);
        }
        for (id, _) in &mut intents {
            remap.str(id);
        }
        for (id, _) in &mut state {
            remap.str(id);
        }
        for id in &mut handlers {
            remap.str(id);
        }
        for lang in &mut langs {
            for (key, _) in &mut lang.entries {
                remap.str(key);
            }
        }
        for (name, _) in extn
            .slots
            .iter_mut()
            .chain(&mut extn.parts)
            .chain(&mut extn.screens)
        {
            remap.str(name);
        }
        for island in &mut extn.islands {
            remap.str(&mut island.id);
            for (key, _) in &mut island.keys {
                remap.str(key);
            }
        }
        for h in &mut extn.hotkeys {
            remap.str(&mut h.code);
        }
        let mut tpls = self.tpls;
        for tpl in &mut tpls {
            for seg in tpl {
                seg.remap(&remap);
            }
        }
        let mut items = self.bindings;
        for b in &mut items {
            b.remap(&remap);
        }
        // Bindings of one node keep the order they were added in.
        items.sort_by_key(|b| (b.tpl, b.node));
        let bindings = Bindings {
            exprs: self.exprs,
            tpls,
            items,
            used_paths,
            intents,
            state,
            handlers,
        };
        let l10n = L10n { langs };
        let srcmap = self
            .srcmap
            .map(|(files, places)| sort_source_map(files, places));

        // Fonts sorted by family, weight, italic; their blobs follow in that order.
        let mut faces = self.fonts;
        for (face, _) in &mut faces {
            face.remap(&remap);
        }
        faces.sort_by_key(|(f, _)| (f.family, f.weight, f.italic));
        let mut font_area = Vec::new();
        let mut fonts = Fonts::default();
        for (mut face, bytes) in faces {
            face.blob.offset = font_area.len() as u32;
            font_area.extend_from_slice(&bytes);
            fonts.faces.push(face);
        }

        let mut image_area = Vec::new();
        let mut images = Images {
            pages: Vec::new(),
            imgs: sorted_imgs,
        };
        for (mut page, bytes) in self.pages {
            page.blob.offset = image_area.len() as u32;
            image_area.extend_from_slice(&bytes);
            images.pages.push(page);
        }

        let mut script_area = Vec::new();
        let mut scripts = Scripts {
            files: Vec::new(),
            reads: self.script_reads,
        };
        scripts.remap(&remap);
        for (path, bytes) in self.scripts {
            let mut path = path;
            remap.str(&mut path);
            let mut blob = stored(BlobSection::Scrp, false, bytes.len(), bytes.len() as u32);
            blob.offset = script_area.len() as u32;
            script_area.extend_from_slice(&bytes);
            scripts.files.push((path, blob));
        }
        let arefs = ARefs {
            items: sorted_arefs,
        };
        let encode = |e: postcard::Error| PackageError::build(format!("encoding failed: {e}"));
        let section = |tag: SectionTag, payload: Result<Vec<u8>, postcard::Error>| {
            Ok(OutSection {
                tag,
                required: true,
                payload: payload.map_err(encode)?,
            })
        };
        let mut sections = vec![
            section(SectionTag::MANI, postcard::to_allocvec(&manifest))?,
            section(SectionTag::STRS, postcard::to_allocvec(&strings))?,
            section(SectionTag::TMPL, postcard::to_allocvec(&templates))?,
            section(SectionTag::STYL, postcard::to_allocvec(&styles))?,
            section(SectionTag::TOKN, postcard::to_allocvec(&tokens))?,
            section(SectionTag::BIND, postcard::to_allocvec(&bindings))?,
            section(SectionTag::L10N, postcard::to_allocvec(&l10n))?,
            section(SectionTag::EXTN, postcard::to_allocvec(&extn))?,
        ];
        if let Some(map) = &srcmap {
            sections.push(OutSection {
                tag: SectionTag::SRCM,
                required: false,
                payload: postcard::to_allocvec(map).map_err(encode)?,
            });
        }
        let blob_section =
            |tag: SectionTag, index: Result<Vec<u8>, postcard::Error>, area: &[u8]| {
                section(tag, index.map(|i| blobs::join(&i, area)))
            };
        sections.push(blob_section(
            SectionTag::FONT,
            postcard::to_allocvec(&fonts),
            &font_area,
        )?);
        if !images.pages.is_empty() || !images.imgs.is_empty() {
            sections.push(blob_section(
                SectionTag::IMGS,
                postcard::to_allocvec(&images),
                &image_area,
            )?);
        }
        if !scripts.files.is_empty() || !scripts.reads.is_empty() {
            sections.push(blob_section(
                SectionTag::SCRP,
                postcard::to_allocvec(&scripts),
                &script_area,
            )?);
        }
        if !arefs.items.is_empty() {
            sections.push(section(SectionTag::AREF, postcard::to_allocvec(&arefs))?);
        }
        let bytes = assemble(sections)?;
        // The writer never emits what the reader would refuse, nor a blob with a wrong hash or
        // a damaged frame.
        Package::read(Arc::from(bytes.as_slice()))?.verify_blobs()?;
        Ok(bytes)
    }
}

/// The blob reference of bytes as the caller hands them in; the offset is set when placed.
fn stored(section: BlobSection, zstd: bool, len: usize, raw_len: u32) -> BlobRef {
    BlobRef {
        section,
        offset: 0,
        len: len as u32,
        raw_len: if zstd { raw_len } else { len as u32 },
        zstd,
    }
}

/// For ids in insertion order, the position each takes when sorted ascending (ties are not
/// possible: the callers pass distinct ids).
fn sorted_positions(ids: impl Iterator<Item = StrId>) -> Vec<u32> {
    let ids: Vec<StrId> = ids.collect();
    let mut order: Vec<usize> = (0..ids.len()).collect();
    order.sort_by_key(|&i| ids[i]);
    let mut position = vec![0u32; ids.len()];
    for (pos, &i) in order.iter().enumerate() {
        position[i] = pos as u32;
    }
    position
}

/// The source map with its files sorted and its places rewritten to the new indices.
fn sort_source_map(files: Vec<String>, places: Vec<Vec<SrcLoc>>) -> SrcMap {
    let mut order: Vec<usize> = (0..files.len()).collect();
    order.sort_by(|&a, &b| files[a].cmp(&files[b]));
    let mut sorted: Vec<String> = Vec::new();
    let mut new_index = vec![0u16; files.len()];
    for &old in &order {
        if sorted.last() != Some(&files[old]) {
            sorted.push(files[old].clone());
        }
        new_index[old] = (sorted.len() - 1) as u16;
    }
    let templates = places
        .into_iter()
        .map(|locs| {
            locs.into_iter()
                .map(|l| SrcLoc {
                    // An index outside the list stays outside it, and the reader refuses it.
                    file: new_index
                        .get(usize::from(l.file))
                        .copied()
                        .unwrap_or(u16::MAX),
                    ..l
                })
                .collect()
        })
        .collect();
    SrcMap {
        files: sorted,
        templates,
    }
}
