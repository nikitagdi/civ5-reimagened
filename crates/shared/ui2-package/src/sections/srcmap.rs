//! `SRCM`: where each node came from in the skin's source, for error reports. Optional.

use serde::{Deserialize, Serialize};

use crate::container::SectionTag;
use crate::error::PackageError;
use crate::sections::templates::Templates;

/// A place in a source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SrcLoc {
    /// Index into [`SrcMap::files`].
    pub file: u16,
    /// Line, from 1.
    pub line: u32,
    /// Column, from 1.
    pub col: u16,
}

/// The section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SrcMap {
    /// Source file paths from the skin's root, `/` separated; ascending, no repeats.
    pub files: Vec<String>,
    /// Per template, one place per node, in node order.
    pub templates: Vec<Vec<SrcLoc>>,
}

impl SrcMap {
    pub(crate) fn validate(
        &self,
        at: SectionTag,
        templates: &Templates,
    ) -> Result<(), PackageError> {
        let bad = |what: &str| Err(PackageError::invalid(at, what));
        if self.files.windows(2).any(|w| w[0] >= w[1]) {
            return bad("the files are not ascending and unique");
        }
        // The package never names a path of the machine that built it.
        let local = |p: &String| {
            p.is_empty()
                || p.starts_with('/')
                || p.contains('\\')
                || p.contains(':')
                || p.split('/').any(|s| s == "..")
        };
        if self.files.iter().any(local) {
            return bad("a file path is not relative to the skin");
        }
        if self.templates.len() != templates.items.len() {
            return bad("the map does not cover every template");
        }
        for (locs, tpl) in self.templates.iter().zip(&templates.items) {
            if locs.len() != tpl.nodes.len() {
                return bad("a template's map does not cover every node");
            }
            if locs.iter().any(|l| l.file as usize >= self.files.len()) {
                return bad("a place names a file that is not in the list");
            }
        }
        Ok(())
    }
}
