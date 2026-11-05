//! The draw groups of the `Main` pass (the performance panel's "Main by group") and their debug
//! switches. WebGPU times whole passes only, so what the pass costs inside comes from counts (draws,
//! triangles, instances per group) and from drawing a group no more and looking at the pass time.
//!
//! A group is what `Main` records together: the subsystems of table 4.2 that draw into its slots, one
//! group for each, except that the water, the fogs and the models of the units share one:
//!
//! | group | subsystems | what is drawn |
//! |---|---|---|
//! | `terrain` | `Terrain` | the ground |
//! | `water` | `Water`, `Waves`, `Ice` | the sea, coast waves, ice |
//! | `rivers` | `River` | rivers |
//! | `units` | `Unit`, `MovementPath` | units, ships, the path of a move |
//! | `trees` | `Forest` | forests, jungle |
//! | `cities` | `City` | the buildings of cities |
//! | `landmarks` | `Landmark` | wonders, improvements, resources, roads |
//! | `decals` | `Decal`, `YieldIcons` | feature decals, resource and improvement icons |
//! | `overlays` | `Overlay`, `Arrows`, `Trade` | borders, grid, highlights, arrows, trade routes |
//! | `fog` | `FowClassic`, `FluidFow`, `DistanceFog` | fog of war and the haze |
//! | `effects` | `Particle` | particles |
//! | `other` | the rest | what `Main` does outside the records (the clear, the test grid) |
//!
//! The counts are the command counters of [`super::draw`] read around each record step of `Main`: a
//! few relaxed loads for each of ~30 steps a frame, always on. The switches are one mask (a field of the
//! renderer, never a global) that is 0 unless the operator unticks a group in the panel; the renderer skips the record of a group whose
//! bit is set, in `Main` only (the class mask of the picking and the other passes are unaffected).

use super::draw::DrawCounts;
use crate::frame::SystemId;

// SIMPLIFIED(SIMP-exe-1121): the groups are the subsystems that record into `Main`, not the finer kinds of
// object (resources, improvements and wonders are drawn together by `Landmark` and `City`).
/// A draw group of `Main`. The discriminant is the bit of the switch mask.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    Terrain,
    Water,
    Rivers,
    Units,
    Trees,
    Cities,
    Landmarks,
    Decals,
    Overlays,
    Fog,
    Effects,
    Other,
}

/// Number of groups.
pub const N: usize = 12;

impl Group {
    /// Every group, in the order of the bits.
    pub const ALL: [Group; N] = [
        Group::Terrain,
        Group::Water,
        Group::Rivers,
        Group::Units,
        Group::Trees,
        Group::Cities,
        Group::Landmarks,
        Group::Decals,
        Group::Overlays,
        Group::Fog,
        Group::Effects,
        Group::Other,
    ];

    /// The group a subsystem's records in `Main` belong to.
    pub fn of(system: SystemId) -> Group {
        match system {
            SystemId::Terrain => Group::Terrain,
            SystemId::Water | SystemId::Waves | SystemId::Ice => Group::Water,
            SystemId::River => Group::Rivers,
            SystemId::Unit | SystemId::MovementPath => Group::Units,
            SystemId::Forest => Group::Trees,
            SystemId::City => Group::Cities,
            SystemId::Landmark => Group::Landmarks,
            SystemId::Decal | SystemId::YieldIcons => Group::Decals,
            SystemId::Overlay | SystemId::Arrows | SystemId::Trade => Group::Overlays,
            SystemId::FowClassic | SystemId::FluidFow | SystemId::DistanceFog => Group::Fog,
            SystemId::Particle => Group::Effects,
            SystemId::Minimap | SystemId::Shadows => Group::Other,
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }

    /// The bit of the group in the switch mask.
    pub fn bit(self) -> u32 {
        1 << self as u32
    }

    /// The name as the panel and the report show it.
    pub fn name(self) -> &'static str {
        match self {
            Group::Terrain => "terrain",
            Group::Water => "water/coast",
            Group::Rivers => "rivers",
            Group::Units => "units",
            Group::Trees => "trees/forests",
            Group::Cities => "cities",
            Group::Landmarks => "landmarks/wonders",
            Group::Decals => "decals/icons",
            Group::Overlays => "overlays/borders",
            Group::Fog => "fog",
            Group::Effects => "effects/particles",
            Group::Other => "other",
        }
    }
}

/// Groups that cannot be switched off: `other` has no record to skip.
const FIXED: u32 = 1 << Group::Other as u32;

/// The mask a renderer keeps for `mask` asked for: only the bits of groups that can be switched off.
/// (The mask is the renderer's, not a global: a debug switch must not reach another renderer.)
pub fn sanitize(mask: u32) -> u32 {
    mask & !FIXED & ((1 << N) - 1)
}

/// True if `mask` (bit = [`Group::bit`]) has `group` switched off.
pub fn is_off(mask: u32, group: Group) -> bool {
    mask & group.bit() != 0
}
/// What the record steps of `Main` did in one frame, by group.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GroupTally {
    rows: [DrawCounts; N],
}

impl GroupTally {
    pub fn add(&mut self, group: Group, counts: DrawCounts) {
        self.rows[group.index()].add(counts);
    }

    pub fn get(&self, group: Group) -> DrawCounts {
        self.rows[group.index()]
    }

    /// The rows of the panel: one per group, the largest in triangles first. `main` is the row of the
    /// `Main` pass; what it did beyond the records (the clear, the test grid) is added to `other`.
    pub fn rows(&self, main: DrawCounts) -> Vec<GroupRow> {
        let mut total = DrawCounts::default();
        for g in Group::ALL {
            total.add(self.get(g));
        }
        let mut rows: Vec<GroupRow> = Group::ALL
            .into_iter()
            .map(|g| {
                let mut c = self.get(g);
                if g == Group::Other {
                    c.draw_calls += main.draw_calls.saturating_sub(total.draw_calls);
                    c.triangles += main.triangles.saturating_sub(total.triangles);
                    c.instances += main.instances.saturating_sub(total.instances);
                }
                GroupRow {
                    name: g.name(),
                    bit: g.bit(),
                    draw_calls: c.draw_calls,
                    triangles: c.triangles,
                    instances: c.instances,
                }
            })
            .collect();
        // Stable: groups with equal triangles stay in the order of the bits.
        rows.sort_by_key(|r| std::cmp::Reverse(r.triangles));
        rows
    }
}

/// One row of "Main by group".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GroupRow {
    pub name: &'static str,
    /// The bit of the group in the switch mask ([`Group::bit`]).
    pub bit: u32,
    pub draw_calls: u32,
    pub triangles: u64,
    pub instances: u64,
}

impl GroupRow {
    /// `other` has nothing to switch off.
    pub fn switchable(&self) -> bool {
        self.bit & FIXED == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(draws: u32, tris: u64, inst: u64) -> DrawCounts {
        DrawCounts {
            draw_calls: draws,
            triangles: tris,
            instances: inst,
            ..Default::default()
        }
    }

    #[test]
    fn every_subsystem_belongs_to_one_group_and_the_bits_are_distinct() {
        for id in SystemId::ALL {
            assert!(Group::ALL.contains(&Group::of(id)), "{id:?}");
        }
        let all = Group::ALL.iter().fold(0, |m, g| m | g.bit());
        assert_eq!(all, (1 << N) - 1, "one bit for each group");
        assert_eq!(Group::of(SystemId::Waves), Group::Water);
        assert_eq!(Group::of(SystemId::Forest), Group::Trees);
        assert_eq!(Group::of(SystemId::MovementPath), Group::Units);
    }

    #[test]
    fn the_rows_are_sorted_by_triangles_and_other_gets_what_the_records_did_not() {
        let mut t = GroupTally::default();
        t.add(Group::Terrain, c(10, 1000, 10));
        t.add(Group::Units, c(5, 3000, 40));
        t.add(Group::Terrain, c(2, 200, 2));
        // The Main row has the records and 3 draws of its own (the test grid).
        let main = c(20, 4300, 55);
        let rows = t.rows(main);
        assert_eq!(rows.len(), N, "every group has a row, also an empty one");
        assert_eq!(rows[0].name, "units");
        assert_eq!(rows[1].name, "terrain");
        assert_eq!(rows[1].draw_calls, 12);
        assert_eq!(rows[1].triangles, 1200);
        assert_eq!(rows[1].instances, 12);
        let other = rows.iter().find(|r| r.name == "other").unwrap();
        assert_eq!(
            (other.draw_calls, other.triangles, other.instances),
            (3, 100, 3)
        );
        // The sum of the rows is the Main row.
        assert_eq!(rows.iter().map(|r| r.draw_calls).sum::<u32>(), 20);
        assert_eq!(rows.iter().map(|r| r.triangles).sum::<u64>(), 4300);
        assert_eq!(rows.iter().map(|r| r.instances).sum::<u64>(), 55);
    }

    #[test]
    fn a_main_row_smaller_than_the_records_adds_nothing_to_other() {
        let mut t = GroupTally::default();
        t.add(Group::Fog, c(4, 400, 4));
        let rows = t.rows(c(1, 1, 1));
        let other = rows.iter().find(|r| r.name == "other").unwrap();
        assert_eq!((other.draw_calls, other.triangles), (0, 0));
    }

    #[test]
    fn the_mask_switches_groups_but_never_other() {
        let all = sanitize(u32::MAX);
        assert!(is_off(all, Group::Terrain) && is_off(all, Group::Effects));
        assert!(!is_off(all, Group::Other), "`other` has no record to skip");
        assert_eq!(all, (1 << N) - 1 - FIXED);
        let units = sanitize(Group::Units.bit());
        assert!(is_off(units, Group::Units) && !is_off(units, Group::Terrain));
        assert_eq!(sanitize(0), 0);
        let rows = GroupTally::default().rows(DrawCounts::default());
        assert_eq!(rows.iter().filter(|r| !r.switchable()).count(), 1);
    }
}
