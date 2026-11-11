//! Names and icons of the core's static tables, read once per core and kept by id.

use std::collections::BTreeMap;

use replica::{CatalogEntry, CatalogTable, ReadError};
use ui2_model::{AssetRef, Text};

use crate::CountingRead;

/// A catalog row as the model shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Item {
    pub name: Text,
    pub icon: AssetRef,
}

impl Item {
    fn of(e: &CatalogEntry) -> Item {
        Item {
            // SIMPLIFIED(SIMP-230): the core's catalog gives names already localized to en_US, not `TXT_KEY_*`
            // keys, so they go into the model as raw text and a language switch does not translate them.
            name: Text::raw(&e.name),
            icon: icon(e.icon_atlas.as_deref(), e.icon_index),
        }
    }
}

/// `icon:<atlas>:<index>`, or no asset when the row has no icon.
#[must_use]
pub fn icon(atlas: Option<&str>, index: Option<i16>) -> AssetRef {
    match (atlas, index) {
        (Some(a), Some(i)) if !a.is_empty() && i >= 0 => AssetRef::new(&format!("icon:{a}:{i}")),
        _ => AssetRef::default(),
    }
}

/// The tables the first-slice roots need.
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    eras: BTreeMap<i16, Item>,
    techs: BTreeMap<i16, Item>,
}

impl Catalog {
    /// Reads the tables. A table the core does not serve stays empty (names then read as empty text); any other
    /// failure is an error, so the caller tries again on the next rebuild.
    ///
    /// # Errors
    /// The core failed on a table it serves.
    pub(crate) fn load(read: &mut CountingRead<'_, '_>) -> Result<Catalog, ReadError> {
        Ok(Catalog {
            eras: table(read, CatalogTable::Eras)?,
            techs: table(read, CatalogTable::Technologies)?,
        })
    }

    /// An era by `EraTypes` id.
    #[must_use]
    pub fn era(&self, id: i16) -> Item {
        self.eras.get(&id).cloned().unwrap_or_default()
    }

    /// A technology by `TechTypes` id.
    #[must_use]
    pub fn tech(&self, id: i16) -> Item {
        self.techs.get(&id).cloned().unwrap_or_default()
    }
}

fn table(
    read: &mut CountingRead<'_, '_>,
    t: CatalogTable,
) -> Result<BTreeMap<i16, Item>, ReadError> {
    match read.catalog(t) {
        Ok(rows) => Ok(rows.iter().map(|e| (e.id, Item::of(e))).collect()),
        Err(ReadError::Unsupported) => Ok(BTreeMap::new()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::icon;

    #[test]
    fn icon_ref_needs_atlas_and_index() {
        assert_eq!(
            &*icon(Some("TECH_ATLAS_1"), Some(12)).0,
            "icon:TECH_ATLAS_1:12"
        );
        assert_eq!(&*icon(None, Some(1)).0, "");
        assert_eq!(&*icon(Some("A"), None).0, "");
        assert_eq!(&*icon(Some("A"), Some(-1)).0, "");
    }
}
