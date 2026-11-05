//! Load groups (`C5GP`, manifest id `tb:groups-b2`): which logical ids belong together and when the
//! client asks for them.
//!
//! A group lists its members as a closure: everything the members need is in the list, so the client
//! never walks dependencies. One id may be a member of several groups. The table is separate from the
//! manifest because the manifest's `Variant.prefetch` can only say "boot or not".
//!
//! Framing is the common blob header; the payload is postcard, so equal tables give equal bytes.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{FormatError, LogicalId, blob};

pub const GROUP_TABLE_MAGIC: [u8; 4] = *b"C5GP";

/// Manifest id of the group table.
pub const GROUP_TABLE_ID: &str = "tb:groups-b2";

/// Id of the group that is loaded before the first frame of the map.
pub const BOOT_GROUP: &str = "boot";

/// Which kind of group it is; the loading screen counts progress per class and the streamer
/// priority follows from it. The order is the order of loading. Postcard writes the variant index,
/// so new classes go at the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum PriorityClass {
    /// Light cubes, colour keys, fog, settings, overlay: needed before the first frame.
    Boot,
    /// Terrain styles, decals, water, minimap, trees, roads: by what is on the map.
    Map,
    /// By the civilizations of the game.
    Civ,
    /// By the era of the player.
    Era,
    /// One object that came into view.
    Near,
}

/// What makes the client ask for a group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Trigger {
    /// Always, before the first map frame (`boot`) or when the map is shown.
    Always,
    /// A terrain style is on the map (`europe`, `generic`, ...).
    Style(String),
    /// A civilization of the game (`CIVILIZATION_ROME`).
    Civ(String),
    /// The era of the player, or the next one (`ERA_MEDIEVAL`).
    Era(String),
    /// An object came into view (`md:archer`).
    Object(String),
    /// A quality preset (`low`).
    Preset(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    /// `boot`, `style:europe`, `civ:CIVILIZATION_ROME`, `era:ERA_MEDIEVAL`, `obj:md:archer`.
    pub id: String,
    pub class: PriorityClass,
    pub trigger: Trigger,
    /// The closure: every id the group needs, sorted, no repeats.
    pub members: Vec<LogicalId>,
    /// Bytes of the members as stored (the largest variant of each id), as the pipeline summed them.
    pub bytes: u64,
    /// Groups to ask for together with this one.
    pub requires: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupTable {
    /// Sorted by id.
    pub groups: Vec<Group>,
}

impl GroupTable {
    pub const VERSION: u16 = 1;
    pub const MIN_VERSION: u16 = 1;

    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, id: &str) -> Option<&Group> {
        self.groups
            .binary_search_by(|g| g.id.as_str().cmp(id))
            .ok()
            .map(|i| &self.groups[i])
    }

    /// Adds `group` or replaces the one with its id; keeps the table sorted.
    pub fn upsert(&mut self, group: Group) {
        match self
            .groups
            .binary_search_by(|g| g.id.as_str().cmp(group.id.as_str()))
        {
            Ok(i) => self.groups[i] = group,
            Err(i) => self.groups.insert(i, group),
        }
    }

    /// Deterministic: the same table gives the same bytes. Groups are written sorted by id and the
    /// members of each sorted without repeats, whatever order they were added in.
    pub fn encode(&self) -> Vec<u8> {
        let mut t = self.clone();
        t.groups.sort_by(|a, b| a.id.cmp(&b.id));
        for g in &mut t.groups {
            g.members.sort();
            g.members.dedup();
            g.requires.sort();
            g.requires.dedup();
        }
        blob::encode_v(GROUP_TABLE_MAGIC, Self::VERSION, &t)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let (_, t): (u16, Self) =
            blob::decode_range(GROUP_TABLE_MAGIC, Self::MIN_VERSION, Self::VERSION, bytes)?;
        t.check()?;
        Ok(t)
    }

    /// Structure of the table: ids are unique and in order, the members are sorted and unique,
    /// every `requires` names a group, and no group needs itself, directly or through others.
    pub fn check(&self) -> Result<(), FormatError> {
        for w in self.groups.windows(2) {
            if w[0].id >= w[1].id {
                return Err(FormatError::invalid(format!(
                    "groups are not sorted or repeat: {:?} then {:?}",
                    w[0].id, w[1].id
                )));
            }
        }
        for g in &self.groups {
            if g.id.is_empty() {
                return Err(FormatError::invalid("group without an id"));
            }
            if g.members.windows(2).any(|w| w[0] >= w[1]) {
                return Err(FormatError::invalid(format!(
                    "group {:?}: members are not sorted or repeat",
                    g.id
                )));
            }
            for r in &g.requires {
                if self.get(r).is_none() {
                    return Err(FormatError::invalid(format!(
                        "group {:?} requires {r:?}, which is not in the table",
                        g.id
                    )));
                }
            }
        }
        // A cycle would make `need` loop; depth-first with a path set.
        let edges: BTreeMap<&str, &[String]> = self
            .groups
            .iter()
            .map(|g| (g.id.as_str(), g.requires.as_slice()))
            .collect();
        let mut done: BTreeSet<&str> = BTreeSet::new();
        for g in &self.groups {
            let mut path = Vec::new();
            visit(g.id.as_str(), &edges, &mut done, &mut path)?;
        }
        Ok(())
    }
}

fn visit<'a>(
    id: &'a str,
    edges: &BTreeMap<&'a str, &'a [String]>,
    done: &mut BTreeSet<&'a str>,
    path: &mut Vec<&'a str>,
) -> Result<(), FormatError> {
    if done.contains(id) {
        return Ok(());
    }
    if path.contains(&id) {
        return Err(FormatError::invalid(format!(
            "groups require each other in a circle: {} -> {id}",
            path.join(" -> ")
        )));
    }
    path.push(id);
    for r in edges.get(id).copied().unwrap_or_default() {
        visit(r.as_str(), edges, done, path)?;
    }
    path.pop();
    done.insert(id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(id: &str, members: &[&str], requires: &[&str]) -> Group {
        Group {
            id: id.into(),
            class: PriorityClass::Map,
            trigger: Trigger::Style(id.into()),
            members: members.iter().map(|m| LogicalId::new(*m)).collect(),
            bytes: 7,
            requires: requires.iter().map(|r| (*r).to_owned()).collect(),
        }
    }

    fn table() -> GroupTable {
        let mut t = GroupTable::new();
        t.upsert(group(
            "style:europe",
            &["tx:b", "pc:europe", "tx:a"],
            &["boot"],
        ));
        t.upsert(Group {
            class: PriorityClass::Boot,
            trigger: Trigger::Always,
            ..group("boot", &["tx:sun"], &[])
        });
        t
    }

    #[test]
    fn round_trips_sorted_and_deterministic() {
        let t = table();
        let bytes = t.encode();
        assert_eq!(&bytes[..4], b"C5GP");
        let back = GroupTable::decode(&bytes).unwrap();
        assert_eq!(back.groups[0].id, "boot");
        // The members come back sorted whatever order they went in.
        let europe = back.get("style:europe").unwrap();
        let names: Vec<_> = europe.members.iter().map(LogicalId::as_str).collect();
        assert_eq!(names, ["pc:europe", "tx:a", "tx:b"]);
        // Adding in the other order gives the same bytes.
        let mut other = GroupTable::new();
        other.upsert(table().groups[0].clone());
        other.upsert(table().groups[1].clone());
        assert_eq!(other.encode(), bytes);
        assert_eq!(back.encode(), bytes);
    }

    #[test]
    fn upsert_replaces_by_id() {
        let mut t = table();
        t.upsert(group("boot", &["tx:new"], &[]));
        assert_eq!(t.groups.len(), 2);
        assert_eq!(t.get("boot").unwrap().members, [LogicalId::new("tx:new")]);
    }

    #[test]
    fn a_requirement_that_does_not_exist_or_loops_is_refused() {
        let mut t = table();
        t.upsert(group("style:asia", &[], &["nope"]));
        assert!(t.check().unwrap_err().to_string().contains("nope"));

        let mut t = GroupTable::new();
        t.upsert(group("a", &[], &["b"]));
        t.upsert(group("b", &[], &["a"]));
        assert!(t.check().unwrap_err().to_string().contains("circle"));
        let mut t = GroupTable::new();
        t.upsert(group("a", &[], &["a"]));
        assert!(t.check().is_err());
    }

    #[test]
    fn unsorted_or_repeated_input_is_refused_on_decode() {
        // A hand-written blob with the groups in the wrong order is not a valid table.
        let t = GroupTable {
            groups: vec![group("b", &[], &[]), group("a", &[], &[])],
        };
        let bytes = blob::encode_v(GROUP_TABLE_MAGIC, GroupTable::VERSION, &t);
        assert!(GroupTable::decode(&bytes).is_err());
        let t = GroupTable {
            groups: vec![group("a", &["tx:x", "tx:x"], &[])],
        };
        let bytes = blob::encode_v(GROUP_TABLE_MAGIC, GroupTable::VERSION, &t);
        assert!(GroupTable::decode(&bytes).is_err());
    }

    #[test]
    fn wrong_magic_version_and_tail_are_refused() {
        let good = table().encode();
        let mut bad = good.clone();
        bad[0] = b'X';
        assert!(matches!(
            GroupTable::decode(&bad),
            Err(FormatError::BadMagic { .. })
        ));
        let mut v2 = good.clone();
        v2[4] = 2;
        assert!(matches!(
            GroupTable::decode(&v2),
            Err(FormatError::UnsupportedVersion { .. })
        ));
        let mut tail = good;
        tail.push(0);
        assert!(matches!(
            GroupTable::decode(&tail),
            Err(FormatError::TrailingBytes(1))
        ));
    }
}
