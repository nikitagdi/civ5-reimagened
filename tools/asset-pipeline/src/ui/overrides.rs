//! What a package's `Override` folder would change.

use std::collections::BTreeSet;

use super::text::{self, TextSet};
use super::vfs::{Origin, UiVfs, VfsError, ext_of};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Zero bytes over a file of another layer: blanks it.
    Blanks { layer: String, size: u64 },
    /// Zero bytes, no other file of that name.
    EmptyNew,
    /// Same bytes as the other copy.
    Same { layer: String },
    /// Different bytes over a file of another layer.
    Replaces { layer: String, old: u64, new: u64 },
    /// A name no other layer has.
    Adds { size: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OverrideItem {
    pub name: String,
    pub size: u64,
    pub effect: Effect,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct OverrideReport {
    pub items: Vec<OverrideItem>,
    pub total: usize,
    pub zero_bytes: usize,
    pub with_content: usize,
    pub blanks: usize,
    pub replaces: usize,
    pub same: usize,
    pub adds: usize,
    pub empty_new: usize,
    /// Names that a UI file of the set also has (xml, lua, ggxml, dds).
    pub ui_overlap: Vec<String>,
    /// Text rows of the override files that change a text of the baseline / add a new tag.
    pub text_changed: usize,
    pub text_new: usize,
    pub text_files: usize,
}

/// `baseline`: the merged texts without the override files. `ui_names`: bare names of the UI set.
pub fn analyze(
    vfs: &UiVfs,
    baseline: &TextSet,
    ui_names: &BTreeSet<String>,
) -> Result<OverrideReport, VfsError> {
    let mut rep = OverrideReport::default();
    let override_paths: BTreeSet<_> = vfs.override_files.iter().map(|o| o.path.clone()).collect();
    let mut files: Vec<_> = vfs.override_files.iter().collect();
    files.sort_by(|a, b| a.name.cmp(&b.name).then(a.path.cmp(&b.path)));
    for o in files {
        rep.total += 1;
        if o.size == 0 {
            rep.zero_bytes += 1;
        } else {
            rep.with_content += 1;
        }
        let others: Vec<_> = vfs
            .copies(&o.name)
            .iter()
            .filter(|e| match &e.origin {
                Origin::Loose { path, .. } => !override_paths.contains(path),
                Origin::Packed { .. } => true,
            })
            .collect();
        let effect = match others.last() {
            None if o.size == 0 => {
                rep.empty_new += 1;
                Effect::EmptyNew
            }
            None => {
                rep.adds += 1;
                Effect::Adds { size: o.size }
            }
            Some(other) => {
                let layer = vfs.layer(other).name.clone();
                if o.size == 0 {
                    rep.blanks += 1;
                    Effect::Blanks {
                        layer,
                        size: other.size(),
                    }
                } else {
                    let mine = std::fs::read(&o.path).map_err(|source| VfsError::Io {
                        path: o.path.clone(),
                        source,
                    })?;
                    if other.size() == o.size && vfs.read(other)? == mine {
                        rep.same += 1;
                        Effect::Same { layer }
                    } else {
                        rep.replaces += 1;
                        Effect::Replaces {
                            layer,
                            old: other.size(),
                            new: o.size,
                        }
                    }
                }
            }
        };
        if matches!(ext_of(&o.name), "xml" | "lua" | "ggxml" | "dds") && ui_names.contains(&o.name)
        {
            rep.ui_overlap.push(o.name.clone());
        }
        if o.size > 0 && ext_of(&o.name) == "xml" {
            let bytes = std::fs::read(&o.path).map_err(|source| VfsError::Io {
                path: o.path.clone(),
                source,
            })?;
            if let Ok(x) = text::parse_xml(&String::from_utf8_lossy(&bytes))
                && !x.rows.is_empty()
            {
                rep.text_files += 1;
                for r in x.rows {
                    match baseline.table(&r.lang).and_then(|t| t.get(&r.tag)) {
                        Some(old) if *old != r.text => rep.text_changed += 1,
                        Some(_) => {}
                        None => rep.text_new += 1,
                    }
                }
            }
        }
        rep.items.push(OverrideItem {
            name: o.name.clone(),
            size: o.size,
            effect,
        });
    }
    rep.ui_overlap.sort();
    rep.ui_overlap.dedup();
    Ok(rep)
}
