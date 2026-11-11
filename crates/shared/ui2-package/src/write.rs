//! `PackageBuilder`: the only way a package is written.
//!
//! `finish` is a pure function of what was added: strings are sorted, sections are laid out by
//! tag, and no collection with an unspecified order takes part. Adding the same strings in another
//! order gives the same bytes.

use crate::container::{OutSection, Tag, assemble};
use crate::error::PackageError;
use crate::ids::StrId;
use crate::sections::manifest::Manifest;
use crate::sections::strings::StrPool;

/// Collects the parts of a package and writes it. Ids handed out by the `str` method are
/// provisional: they are valid for the data added to this builder and are rewritten when the
/// package is finished.
#[derive(Debug, Default)]
pub struct PackageBuilder {
    pool: StrPool,
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

    /// Writes the package. The manifest is checked first (id grammar, themes and languages).
    pub fn finish(self, manifest: Manifest) -> Result<Vec<u8>, PackageError> {
        manifest.validate(Tag::MANI)?;
        let (strings, _remap) = self.pool.finish();
        let encode = |e: postcard::Error| PackageError::build(format!("encoding failed: {e}"));
        let sections = vec![
            OutSection {
                tag: Tag::MANI,
                required: true,
                payload: postcard::to_allocvec(&manifest).map_err(encode)?,
            },
            OutSection {
                tag: Tag::STRS,
                required: true,
                payload: postcard::to_allocvec(&strings).map_err(encode)?,
            },
        ];
        assemble(sections)
    }
}
