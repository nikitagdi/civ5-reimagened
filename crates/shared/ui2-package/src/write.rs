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

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Serialize;

use crate::container::{OutSection, SectionTag, assemble};
use crate::error::PackageError;
use crate::ids::{StrId, StyleId, StyleSetId, TemplateId};
use crate::package::Package;
use crate::remap::{Remap, Remapped};
use crate::sections::manifest::Manifest;
use crate::sections::strings::StrPool;
use crate::sections::styles::{Bound, StyleSet, Styles};
use crate::sections::templates::{Template, Templates};
use crate::sections::tokens::{Theme, Tokens};
use crate::style::StyleRecord;

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
        let (strings, strs) = self.pool.finish();
        let remap = Remap {
            strs,
            ..Remap::default()
        };
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

        let encode = |e: postcard::Error| PackageError::build(format!("encoding failed: {e}"));
        let section = |tag: SectionTag, payload: Result<Vec<u8>, postcard::Error>| {
            Ok(OutSection {
                tag,
                required: true,
                payload: payload.map_err(encode)?,
            })
        };
        let sections = vec![
            section(SectionTag::MANI, postcard::to_allocvec(&manifest))?,
            section(SectionTag::STRS, postcard::to_allocvec(&strings))?,
            section(SectionTag::TMPL, postcard::to_allocvec(&templates))?,
            section(SectionTag::STYL, postcard::to_allocvec(&styles))?,
            section(SectionTag::TOKN, postcard::to_allocvec(&tokens))?,
        ];
        let bytes = assemble(sections)?;
        Package::read(Arc::from(bytes.as_slice()))?;
        Ok(bytes)
    }
}
