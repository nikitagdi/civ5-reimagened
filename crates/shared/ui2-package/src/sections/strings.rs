//! `STRS`: the string table. Sorted by bytes, no repeats; a `StrId` is an index.

use serde::{Deserialize, Serialize};

use crate::container::SectionTag;
use crate::error::PackageError;

/// The section: identifiers, classes, keys and paths.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StringTable {
    /// Strictly ascending by bytes.
    pub items: Vec<String>,
}

impl StringTable {
    pub(crate) fn validate(&self, at: SectionTag) -> Result<(), PackageError> {
        if self.items.windows(2).any(|w| w[0] >= w[1]) {
            return Err(PackageError::invalid(
                at,
                "the strings are not sorted and unique",
            ));
        }
        if u32::try_from(self.items.len()).is_err() {
            return Err(PackageError::invalid(at, "too many strings"));
        }
        Ok(())
    }
}

/// The builder's string pool. `intern` hands out provisional ids in insertion order; `finish`
/// sorts the strings and returns the table with the map from provisional to final ids, which the
/// builder applies to every section, so the insertion order never reaches the bytes.
#[cfg(feature = "write")]
#[derive(Debug, Default)]
pub(crate) struct StrPool {
    by_text: std::collections::BTreeMap<String, u32>,
    count: u32,
}

#[cfg(feature = "write")]
impl StrPool {
    pub fn intern(&mut self, s: &str) -> crate::ids::StrId {
        if let Some(&id) = self.by_text.get(s) {
            return crate::ids::StrId(id);
        }
        let id = self.count;
        self.count += 1;
        self.by_text.insert(s.to_string(), id);
        crate::ids::StrId(id)
    }

    /// The sorted table and `map[provisional] = final`.
    pub fn finish(self) -> (StringTable, Vec<u32>) {
        let mut map = vec![0u32; self.count as usize];
        let mut items = Vec::with_capacity(self.by_text.len());
        // `BTreeMap` iterates in key order: the final id is the position.
        for (final_id, (text, provisional)) in self.by_text.into_iter().enumerate() {
            map[provisional as usize] = final_id as u32;
            items.push(text);
        }
        (StringTable { items }, map)
    }
}

#[cfg(all(test, feature = "write"))]
mod tests {
    use super::*;

    #[test]
    fn pool_sorts_and_maps() {
        let mut pool = StrPool::default();
        let b = pool.intern("b");
        let a = pool.intern("a");
        assert_eq!(pool.intern("b"), b);
        let (table, map) = pool.finish();
        assert_eq!(table.items, ["a", "b"]);
        assert_eq!(map[b.index()], 1);
        assert_eq!(map[a.index()], 0);
    }
}
