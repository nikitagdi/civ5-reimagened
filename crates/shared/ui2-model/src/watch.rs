//! Subscriptions: a set of fields a consumer reads, and the delta filtered to it. The store gives the
//! mechanism only; the runtime and the script host decide who watches what.

use crate::delta::{FrameDelta, Op};
use crate::schema::{FieldId, PathEntry, Schema, SchemaError};
use crate::store::ModelStore;

/// The fields a consumer is interested in: the listed paths and everything below them.
#[derive(Clone, Debug)]
pub struct WatchSet {
    /// Indexed by field id: the field is in the set, below one that is, or above one that is (a change of a
    /// whole struct touches the fields inside it).
    hit: Vec<bool>,
    /// The path slot of every root, for `Reset` (`None` for a root that is gone).
    root_fields: Vec<Option<FieldId>>,
}

/// Mark the node `id` of a path table whose first slot has the store-wide id `base`, everything below it and
/// everything above it.
fn mark(paths: &[PathEntry], base: u32, id: usize, hit: &mut [bool]) {
    let end = paths[id].end as usize;
    let first = base as usize;
    for h in &mut hit[first + id..first + end] {
        *h = true;
    }
    // The fields above it see a change of their whole value.
    for (j, a) in paths[..id].iter().enumerate() {
        if a.end as usize > id {
            hit[first + j] = true;
        }
    }
}

impl WatchSet {
    /// The set of the given path prefixes (`top`, `top.resources`, `production.groups[].options`): each prefix
    /// and all fields below it.
    ///
    /// # Errors
    /// A prefix that is not a path of the schema.
    pub fn from_prefixes(schema: &Schema, prefixes: &[&str]) -> Result<WatchSet, SchemaError> {
        let paths = schema.paths();
        let mut hit = vec![false; paths.len()];
        for p in prefixes {
            mark(paths, 0, schema.resolve(p)?.id.0 as usize, &mut hit);
        }
        Ok(WatchSet {
            hit,
            root_fields: schema.roots().iter().map(|r| Some(r.field)).collect(),
        })
    }

    /// Like [`from_prefixes`](Self::from_prefixes) for the paths of the store: the schema's and those of the
    /// mounted fragments (`state`, `mods.hello.count`). The set holds the ids as they are now: build it
    /// again after a fragment is mounted or unmounted.
    ///
    /// # Errors
    /// A prefix that is not a path of the schema or of a mounted fragment.
    pub fn from_store(store: &ModelStore, prefixes: &[&str]) -> Result<WatchSet, SchemaError> {
        let mut hit = vec![false; store.field_ids() as usize];
        for p in prefixes {
            let id = store.resolve(p)?.id;
            if let Some((schema, base)) = store.locate(id) {
                mark(schema.paths(), base, (id.0 - base) as usize, &mut hit);
            }
        }
        Ok(WatchSet {
            hit,
            root_fields: store.root_fields(),
        })
    }

    /// Whether a change at this field concerns the set.
    #[must_use]
    pub fn contains(&self, field: FieldId) -> bool {
        self.hit.get(field.0 as usize).copied().unwrap_or(false)
    }

    fn keeps(&self, op: &Op) -> bool {
        match op {
            Op::Set { field, .. } => self.contains(*field),
            Op::Insert { list, .. } | Op::Remove { list, .. } | Op::Move { list, .. } => {
                self.contains(*list)
            }
            Op::Reset { root, .. } => self
                .root_fields
                .get(usize::from(root.0))
                .copied()
                .flatten()
                .is_some_and(|f| self.contains(f)),
        }
    }
}

impl FrameDelta {
    /// The operations that concern a watch set, with the same revision; `None` if there are none.
    #[must_use]
    pub fn filter(&self, watch: &WatchSet) -> Option<FrameDelta> {
        let ops: Vec<Op> = self
            .ops
            .iter()
            .filter(|o| watch.keeps(o))
            .cloned()
            .collect();
        (!ops.is_empty()).then_some(FrameDelta {
            revision: self.revision,
            ops,
        })
    }
}
