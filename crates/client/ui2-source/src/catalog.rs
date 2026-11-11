//! Names and icons of the core's static tables, read once per core and kept by id.

use std::collections::BTreeMap;

use replica::{CatalogEntry, CatalogTable, OrderType, ReadError};
use ui2_model::{AssetRef, Text};

use crate::CountingRead;
use crate::trigger::Roots;

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

/// The tables this source reads, in the order frames load them: the small ones the top bar and the game root name
/// things with first, then the big ones of units, buildings and promotions.
pub const TABLES: [CatalogTable; 8] = [
    CatalogTable::Eras,
    CatalogTable::Technologies,
    CatalogTable::Resources,
    CatalogTable::PolicyBranches,
    CatalogTable::Units,
    CatalogTable::Buildings,
    CatalogTable::Projects,
    CatalogTable::Promotions,
];

const ERAS: usize = 0;
const TECHS: usize = 1;
const RESOURCES: usize = 2;
const BRANCHES: usize = 3;
const UNITS: usize = 4;
const BUILDINGS: usize = 5;
const PROJECTS: usize = 6;
const PROMOTIONS: usize = 7;

const fn bit(i: usize) -> u16 {
    1 << i
}

/// The tables a root names things with: it is built only when they are all loaded.
#[must_use]
pub fn tables_of(root: Roots) -> u16 {
    if root == Roots::TOP {
        bit(RESOURCES) | bit(BRANCHES)
    } else if root == Roots::RESEARCH {
        bit(TECHS)
    } else if root == Roots::GAME {
        bit(ERAS)
    } else if root == Roots::UNIT {
        bit(UNITS) | bit(PROMOTIONS)
    } else if root == Roots::MAP || root == Roots::PRODUCTION {
        bit(UNITS) | bit(BUILDINGS) | bit(PROJECTS)
    } else {
        0
    }
}

/// The tables the roots of this source need, read once per core. Loading is spread over frames by the frame
/// budget (the whole catalog costs more than one frame): a frame loads tables in [`TABLES`] order while they fit,
/// and a root waits until the tables it needs ([`tables_of`]) are there.
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    tables: [BTreeMap<i16, Item>; 8],
    /// The tables read (a table the core does not serve counts as read, empty).
    loaded: u16,
    /// The tables whose read failed since the game last changed: the roots go on without their names, and the
    /// table is asked again after the next change of the game.
    failed: u16,
}

impl Catalog {
    /// Every table is read.
    #[must_use]
    pub fn complete(&self) -> bool {
        self.loaded == bit(TABLES.len()) - 1
    }

    /// The tables `need` ([`tables_of`]) are read, or failed until the game changes.
    #[must_use]
    pub fn ready(&self, need: u16) -> bool {
        (self.loaded | self.failed) & need == need
    }

    /// The game changed: the tables that failed are asked again.
    pub(crate) fn retry_failed(&mut self) {
        self.failed = 0;
    }

    /// Reads the tables still missing, in [`TABLES`] order, while the frame budget admits them; each table is one
    /// piece of work, and the first read of a frame is always admitted. A table the core does not serve stays
    /// empty (its names read as empty text); a table the core fails on is marked failed until the game changes.
    pub(crate) fn load(&mut self, read: &mut CountingRead<'_, '_>) {
        for (i, t) in TABLES.into_iter().enumerate() {
            if (self.loaded | self.failed) & bit(i) != 0 {
                continue;
            }
            read.begin(read.nothing_read());
            let r = read.catalog(t);
            if read.refused() {
                return;
            }
            match r {
                Ok(rows) => {
                    self.tables[i] = rows.iter().map(|e| (e.id, Item::of(e))).collect();
                    self.loaded |= bit(i);
                }
                Err(ReadError::Unsupported) => self.loaded |= bit(i),
                Err(e) => {
                    tracing::debug!(target: "ui2_source", table = ?t, "catalog: {e}");
                    self.failed |= bit(i);
                }
            }
        }
    }

    fn get(&self, table: usize, id: i16) -> Item {
        self.tables[table].get(&id).cloned().unwrap_or_default()
    }

    /// A unit type by `UnitTypes` id.
    #[must_use]
    pub fn unit(&self, id: i16) -> Item {
        self.get(UNITS, id)
    }

    /// A promotion by `PromotionTypes` id.
    #[must_use]
    pub fn promotion(&self, id: i16) -> Item {
        self.get(PROMOTIONS, id)
    }

    /// What a city order builds: a unit, a building (wonders too), a project; a process has no catalog row.
    #[must_use]
    pub fn order_item(&self, order: OrderType, id: i16) -> Item {
        let table = match order {
            OrderType::Train => UNITS,
            OrderType::Construct => BUILDINGS,
            OrderType::Create => PROJECTS,
            OrderType::Maintain => {
                return Item {
                    // SIMPLIFIED(SIMP-243): processes (Wealth, Research) are not in the core's catalog, so they are
                    // named by number until the rules tables of the asset pipeline name them.
                    name: Text::raw(&format!("Process {id}")),
                    icon: AssetRef::default(),
                };
            }
            OrderType::Other(_) => return Item::default(),
        };
        self.get(table, id)
    }

    /// An era by `EraTypes` id.
    #[must_use]
    pub fn era(&self, id: i16) -> Item {
        self.get(ERAS, id)
    }

    /// A technology by `TechTypes` id.
    #[must_use]
    pub fn tech(&self, id: i16) -> Item {
        self.get(TECHS, id)
    }

    /// A resource by `ResourceTypes` id.
    #[must_use]
    pub fn resource(&self, id: i16) -> Item {
        self.get(RESOURCES, id)
    }

    /// A policy branch by `PolicyBranchTypes` id (the table has names only, no icons).
    #[must_use]
    pub fn branch(&self, id: i16) -> Item {
        self.get(BRANCHES, id)
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
