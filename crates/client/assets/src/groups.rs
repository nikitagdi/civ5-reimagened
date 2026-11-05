//! Load groups on top of the [`Streamer`].
//!
//! The group table (`tb:groups-b2`) lists the closure of every group, so the scheduler never walks
//! dependencies: it turns the members of a group (and of the groups it requires) into blob requests
//! in the streamer, and counts what has arrived. It performs no I/O; `client-app` pops the streamer
//! queue, fetches, and calls [`GroupScheduler::complete`].
//!
//! A table that is missing, a group that is unknown and a member that is not in the manifest are
//! errors that are logged and returned, never replaced by a stand-in (AB-D14).

use std::collections::{BTreeMap, BTreeSet};

use asset_format::{GroupTable, PriorityClass};

use crate::{ContentHash, LogicalId, Manifest, PlatformProfile, Priority, Streamer};

#[derive(Debug, thiserror::Error)]
pub enum GroupError {
    #[error("the group table blob is missing")]
    TableMissing,
    #[error("the group table is invalid: {0}")]
    Table(#[from] asset_format::FormatError),
    #[error("unknown load group {0:?}")]
    UnknownGroup(String),
}

/// What one [`GroupScheduler::need`] did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NeedOutcome {
    /// Blobs put in the streamer queue (new, or moved to a higher priority).
    pub queued: usize,
    /// Blobs that were already requested earlier or have arrived.
    pub already: usize,
    /// Members the manifest has no variant of for the device profile. Logged as errors; the load
    /// of the group is not complete without them.
    pub missing: Vec<LogicalId>,
}

pub struct GroupScheduler {
    table: GroupTable,
    manifest: Manifest,
    profile: PlatformProfile,
    streamer: Streamer,
    /// Blobs asked for, by the class of the group that asked.
    wanted: BTreeMap<PriorityClass, BTreeSet<ContentHash>>,
    done: BTreeSet<ContentHash>,
    /// The Low preset: `@low` variants in place of the full textures.
    low: bool,
    /// Objects whose `obj:` group was asked for ([`GroupScheduler::object_appeared`]).
    seen_objects: BTreeSet<String>,
}

/// Suffix of the logical id of the reduced variant of a terrain texture (`tx:grass_d.dds@low`).
const LOW_SUFFIX: &str = "@low";

impl GroupScheduler {
    /// `table` is the bytes of the `tb:groups-b2` blob, `None` when it could not be fetched.
    pub fn from_blob(
        table: Option<&[u8]>,
        manifest: Manifest,
        profile: PlatformProfile,
    ) -> Result<Self, GroupError> {
        let Some(bytes) = table else {
            tracing::error!("the group table (tb:groups-b2) is missing; no group can be loaded");
            return Err(GroupError::TableMissing);
        };
        let table = GroupTable::decode(bytes).inspect_err(|e| {
            tracing::error!("the group table is invalid: {e}");
        })?;
        Ok(Self::new(table, manifest, profile))
    }

    pub fn new(table: GroupTable, manifest: Manifest, profile: PlatformProfile) -> Self {
        Self {
            table,
            manifest,
            profile,
            streamer: Streamer::new(),
            wanted: BTreeMap::new(),
            done: BTreeSet::new(),
            low: false,
            seen_objects: BTreeSet::new(),
        }
    }

    pub fn table(&self) -> &GroupTable {
        &self.table
    }

    /// Asks for group `key` and the groups it requires, at `priority`. A blob asked again at a
    /// higher priority moves up the queue; one that has arrived is not asked for again.
    pub fn need(&mut self, key: &str, priority: Priority) -> Result<NeedOutcome, GroupError> {
        if self.table.get(key).is_none() {
            tracing::error!("unknown load group {key:?}");
            return Err(GroupError::UnknownGroup(key.to_owned()));
        }
        // Requirements first; the table was checked for circles when it was read.
        let mut order: Vec<&str> = Vec::new();
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        self.closure(key, &mut seen, &mut order);

        let mut out = NeedOutcome::default();
        let mut requests: Vec<(PriorityClass, ContentHash)> = Vec::new();
        for id in order {
            let Some(group) = self.table.get(id) else {
                continue;
            };
            let held: BTreeSet<&LogicalId> = group.members.iter().collect();
            for member in &group.members {
                if !self.wants(member, &held) {
                    continue;
                }
                match self.manifest.resolve(member, &self.profile) {
                    Some(v) => requests.push((group.class, v.hash)),
                    None => {
                        tracing::error!(
                            "load group {id:?}: {} has no variant for this device in the manifest",
                            member.as_str()
                        );
                        if !out.missing.contains(member) {
                            out.missing.push(member.clone());
                        }
                    }
                }
            }
        }
        for (class, hash) in requests {
            self.wanted.entry(class).or_default().insert(hash);
            if self.done.contains(&hash) {
                out.already += 1;
            } else if self.streamer.contains(&hash) {
                // Upgrades the priority when it is higher; counts as queued only if it moved.
                self.streamer.request(hash, priority);
                out.already += 1;
            } else {
                self.streamer.request(hash, priority);
                out.queued += 1;
            }
        }
        Ok(out)
    }

    /// The Low preset (`TextureQuality` 0, AB-D12): a member that has an `@low` variant in its
    /// group is fetched as that variant; on any other preset the `@low` variants are never
    /// fetched. Set before the first [`need`](Self::need); blobs already asked for stay.
    pub fn set_low(&mut self, low: bool) {
        self.low = low;
    }

    pub fn is_low(&self) -> bool {
        self.low
    }

    /// Whether `member` of a group that holds `held` is fetched on the current preset.
    fn wants(&self, member: &LogicalId, held: &BTreeSet<&LogicalId>) -> bool {
        if member.as_str().ends_with(LOW_SUFFIX) {
            return self.low;
        }
        !(self.low && held.contains(&LogicalId::new(format!("{}{LOW_SUFFIX}", member.as_str()))))
    }

    /// `style:<style>` (class `Map`) with the groups it requires (`boot`, `style:shared`): the style
    /// is on the map. A style of the first frame is asked for at `Boot` priority, the others at
    /// `Visible`.
    pub fn need_style(
        &mut self,
        style: &str,
        first_frame: bool,
    ) -> Result<NeedOutcome, GroupError> {
        let p = if first_frame {
            Priority::Boot
        } else {
            Priority::Visible
        };
        self.need(&format!("style:{style}"), p)
    }

    /// `civ:<type>` (class `Civ`): a civilization of the game; `Adjacent` priority.
    pub fn need_civ(&mut self, civ: &str) -> Result<NeedOutcome, GroupError> {
        self.need(&format!("civ:{civ}"), Priority::Adjacent)
    }

    /// `era:<era>` (class `Era`): the era of the player (`Adjacent`) or the next one
    /// (`prefetch`, `Prefetch` priority).
    pub fn need_era(&mut self, era: &str, prefetch: bool) -> Result<NeedOutcome, GroupError> {
        let p = if prefetch {
            Priority::Prefetch
        } else {
            Priority::Adjacent
        };
        self.need(&format!("era:{era}"), p)
    }

    /// `obj:<key>` (class `Near`): one unit type, landmark or building came into view; `key` is
    /// what the art table names (`md:archer`). An object the table has no group for is an
    /// error that is logged, not a stand-in (the models of an era may already be in its group, so
    /// the caller asks only for objects it has no model for).
    pub fn need_object(&mut self, key: &str) -> Result<NeedOutcome, GroupError> {
        self.need(&format!("obj:{key}"), Priority::Visible)
    }

    /// An object (`model`, such as `md:archer`) shows on the map. The first time, `obj:<model>` is
    /// asked for at `Visible` priority and the outcome is returned; later `None`, so the caller may
    /// call it on every frame the object is seen. A model the table has no group for is an error
    /// (see [`need_object`](Self::need_object)) and is asked again on the next sighting.
    pub fn object_appeared(&mut self, model: &str) -> Result<Option<NeedOutcome>, GroupError> {
        if self.seen_objects.contains(model) {
            return Ok(None);
        }
        let out = self.need_object(model)?;
        self.seen_objects.insert(model.to_owned());
        Ok(Some(out))
    }

    fn closure<'a>(&'a self, id: &'a str, seen: &mut BTreeSet<&'a str>, order: &mut Vec<&'a str>) {
        if !seen.insert(id) {
            return;
        }
        if let Some(g) = self.table.get(id) {
            for r in &g.requires {
                self.closure(r.as_str(), seen, order);
            }
        }
        order.push(id);
    }

    /// The next blob to fetch, by priority and then in request order.
    pub fn next_request(&mut self) -> Option<(ContentHash, Priority)> {
        self.streamer.next()
    }

    /// The blob has arrived (the caller fetched it and put it in the cache).
    pub fn complete(&mut self, hash: ContentHash) {
        self.done.insert(hash);
    }

    /// `(arrived, asked for)` over the groups of `class` that were asked for: the progress bar of
    /// the loading screen. `(0, 0)` while no group of the class has been asked for.
    pub fn progress(&self, class: PriorityClass) -> (usize, usize) {
        match self.wanted.get(&class) {
            Some(set) => (
                set.iter().filter(|h| self.done.contains(h)).count(),
                set.len(),
            ),
            None => (0, 0),
        }
    }

    /// Blobs asked for that have not arrived: waiting in the queue or being fetched.
    pub fn pending(&self) -> usize {
        let all: BTreeSet<&ContentHash> = self.wanted.values().flatten().collect();
        all.iter().filter(|h| !self.done.contains(**h)).count()
    }
}

#[cfg(test)]
mod tests {
    use asset_format::{Group, PrefetchGroup, Trigger, Variant, VariantFormat};

    use super::*;

    fn hash(n: u8) -> ContentHash {
        ContentHash([n; 32])
    }

    fn group(id: &str, class: PriorityClass, members: &[&str], requires: &[&str]) -> Group {
        Group {
            id: id.into(),
            class,
            trigger: Trigger::Always,
            members: members.iter().map(|m| LogicalId::new(*m)).collect(),
            bytes: 0,
            requires: requires.iter().map(|r| (*r).to_owned()).collect(),
        }
    }

    /// boot = {tx:cube, tb:world}; style:europe = {pc:europe, tx:grass} requires boot;
    /// water = {tx:bumps, tx:cube}.
    fn scheduler() -> GroupScheduler {
        let mut m = Manifest::new();
        for (i, id) in ["tx:cube", "tb:world", "pc:europe", "tx:grass", "tx:bumps"]
            .iter()
            .enumerate()
        {
            m.insert(
                LogicalId::new(*id),
                Variant {
                    format: VariantFormat::Generic,
                    hash: hash(i as u8 + 1),
                    size: 10,
                    prefetch: PrefetchGroup(0),
                },
            );
        }
        let mut t = GroupTable::new();
        t.upsert(group(
            "boot",
            PriorityClass::Boot,
            &["tx:cube", "tb:world"],
            &[],
        ));
        t.upsert(group(
            "style:europe",
            PriorityClass::Map,
            &["pc:europe", "tx:grass"],
            &["boot"],
        ));
        t.upsert(group(
            "water",
            PriorityClass::Map,
            &["tx:bumps", "tx:cube"],
            &[],
        ));
        GroupScheduler::new(t, m, PlatformProfile::uncompressed_only())
    }

    fn drain(s: &mut GroupScheduler) -> Vec<(u8, Priority)> {
        std::iter::from_fn(|| s.next_request())
            .map(|(h, p)| (h.0[0], p))
            .collect()
    }

    #[test]
    fn boot_requests_come_before_visible_ones_whatever_the_order_asked() {
        let mut s = scheduler();
        s.need("water", Priority::Visible).unwrap();
        s.need("boot", Priority::Boot).unwrap();
        // tx:cube (1) was asked for as visible first and then as boot: it moved up.
        assert_eq!(
            drain(&mut s),
            [
                (1, Priority::Boot),
                (2, Priority::Boot),
                (5, Priority::Visible)
            ]
        );
    }

    #[test]
    fn a_group_brings_the_groups_it_requires() {
        let mut s = scheduler();
        let out = s.need("style:europe", Priority::Visible).unwrap();
        assert_eq!(out.queued, 4);
        assert!(out.missing.is_empty());
        let got: Vec<u8> = drain(&mut s).into_iter().map(|(h, _)| h).collect();
        // The members of `boot` are asked for first, then the style's own, each in table order.
        assert_eq!(got, [1, 2, 3, 4]);
    }

    #[test]
    fn progress_counts_by_class_and_pending_drains() {
        let mut s = scheduler();
        assert_eq!(s.progress(PriorityClass::Boot), (0, 0));
        assert_eq!(s.pending(), 0);
        s.need("boot", Priority::Boot).unwrap();
        s.need("water", Priority::Visible).unwrap();
        assert_eq!(s.progress(PriorityClass::Boot), (0, 2));
        assert_eq!(s.progress(PriorityClass::Map), (0, 2));
        assert_eq!(s.pending(), 3, "tx:cube is in both groups, counted once");
        s.complete(hash(1));
        s.complete(hash(2));
        assert_eq!(s.progress(PriorityClass::Boot), (2, 2));
        // tx:cube is also a member of the water group.
        assert_eq!(s.progress(PriorityClass::Map), (1, 2));
        assert_eq!(s.pending(), 1);
        s.complete(hash(5));
        assert_eq!(s.pending(), 0);
    }

    #[test]
    fn a_blob_that_has_arrived_is_not_asked_for_again() {
        let mut s = scheduler();
        s.need("boot", Priority::Boot).unwrap();
        s.complete(hash(1));
        s.complete(hash(2));
        while s.next_request().is_some() {}
        let out = s.need("boot", Priority::Boot).unwrap();
        assert_eq!((out.queued, out.already), (0, 2));
        assert_eq!(s.next_request(), None);
    }

    #[test]
    fn unknown_group_missing_member_and_missing_table_are_errors_not_stand_ins() {
        let mut s = scheduler();
        assert!(matches!(
            s.need("civ:nobody", Priority::Visible),
            Err(GroupError::UnknownGroup(_))
        ));
        assert_eq!(s.pending(), 0);

        // A member the manifest does not have: reported, the rest still loads.
        let mut t = GroupTable::new();
        t.upsert(group(
            "g",
            PriorityClass::Near,
            &["tx:cube", "tx:gone"],
            &[],
        ));
        let mut s = GroupScheduler::new(
            t,
            scheduler().manifest,
            PlatformProfile::uncompressed_only(),
        );
        let out = s.need("g", Priority::Visible).unwrap();
        assert_eq!(out.queued, 1);
        assert_eq!(out.missing, [LogicalId::new("tx:gone")]);

        let none =
            GroupScheduler::from_blob(None, Manifest::new(), PlatformProfile::uncompressed_only());
        assert!(matches!(none, Err(GroupError::TableMissing)));
        let bad = GroupScheduler::from_blob(
            Some(b"junk"),
            Manifest::new(),
            PlatformProfile::uncompressed_only(),
        );
        assert!(matches!(bad, Err(GroupError::Table(_))));
    }

    /// A scheduler over `ids` (hash n+1 for the n-th id) and the groups `(id, class, members, requires)`.
    fn scheduler_of(
        ids: &[&str],
        groups: &[(&str, PriorityClass, &[&str], &[&str])],
    ) -> GroupScheduler {
        let mut m = Manifest::new();
        for (i, id) in ids.iter().enumerate() {
            m.insert(
                LogicalId::new(*id),
                Variant {
                    format: VariantFormat::Generic,
                    hash: hash(i as u8 + 1),
                    size: 10,
                    prefetch: PrefetchGroup(0),
                },
            );
        }
        let mut t = GroupTable::new();
        for (id, class, members, requires) in groups {
            t.upsert(group(id, *class, members, requires));
        }
        GroupScheduler::new(t, m, PlatformProfile::uncompressed_only())
    }

    #[test]
    fn a_style_brings_boot_and_the_shared_group_and_first_frame_styles_go_first() {
        let mut s = scheduler_of(
            &["tx:cube", "tx:big_h", "pc:europe", "pc:asia", "tx:waves"],
            &[
                ("boot", PriorityClass::Boot, &["tx:cube"], &[]),
                ("style:shared", PriorityClass::Map, &["tx:big_h"], &["boot"]),
                (
                    "style:europe",
                    PriorityClass::Map,
                    &["pc:europe"],
                    &["boot", "style:shared"],
                ),
                (
                    "style:asia",
                    PriorityClass::Map,
                    &["pc:asia"],
                    &["boot", "style:shared"],
                ),
                ("water", PriorityClass::Map, &["tx:waves"], &["boot"]),
            ],
        );
        s.need_style("asia", false).unwrap();
        let out = s.need_style("europe", true).unwrap();
        // The shared group was already asked for with asia: only the style's own blob is new.
        assert_eq!((out.queued, out.already), (1, 2));
        let got = drain(&mut s);
        // Europe is the first frame: its blob and the shared and boot ones it moved up come first.
        assert_eq!(
            got[..3],
            [
                (1, Priority::Boot),
                (2, Priority::Boot),
                (3, Priority::Boot)
            ]
        );
        assert_eq!(got[3], (4, Priority::Visible));
        assert_eq!(
            s.progress(PriorityClass::Map),
            (0, 3),
            "shared, europe, asia"
        );
    }

    #[test]
    fn low_fetches_the_low_variant_in_place_of_the_texture_and_normal_never_fetches_it() {
        let ids = ["pc:europe", "tx:grass_d", "tx:grass_d@low", "tx:rock_d"];
        let groups: [(&str, PriorityClass, &[&str], &[&str]); 1] = [(
            "style:europe",
            PriorityClass::Map,
            &["pc:europe", "tx:grass_d", "tx:grass_d@low", "tx:rock_d"],
            &[],
        )];
        let mut normal = scheduler_of(&ids, &groups);
        assert!(!normal.is_low());
        let out = normal.need("style:europe", Priority::Visible).unwrap();
        assert_eq!(out.queued, 3);
        // pc:europe (1), tx:grass_d (2), tx:rock_d (4); never tx:grass_d@low (3).
        let got: Vec<u8> = drain(&mut normal).into_iter().map(|(h, _)| h).collect();
        assert_eq!(got, [1, 2, 4]);

        let mut low = scheduler_of(&ids, &groups);
        low.set_low(true);
        let out = low.need("style:europe", Priority::Visible).unwrap();
        assert_eq!(out.queued, 3);
        // The texture without a variant (rock) is fetched as it is.
        let got: Vec<u8> = drain(&mut low).into_iter().map(|(h, _)| h).collect();
        assert_eq!(got, [1, 3, 4]);
    }

    #[test]
    fn civ_era_and_object_groups_have_their_own_priorities_and_an_unknown_object_is_an_error() {
        let mut s = scheduler_of(
            &["md:legion", "md:knight", "md:archer", "tx:a"],
            &[
                (
                    "civ:CIVILIZATION_ROME",
                    PriorityClass::Civ,
                    &["md:legion"],
                    &[],
                ),
                ("era:ERA_CLASSICAL", PriorityClass::Era, &["md:knight"], &[]),
                ("era:ERA_MEDIEVAL", PriorityClass::Era, &["tx:a"], &[]),
                ("obj:md:archer", PriorityClass::Near, &["md:archer"], &[]),
            ],
        );
        s.need_civ("CIVILIZATION_ROME").unwrap();
        s.need_era("ERA_MEDIEVAL", true).unwrap();
        s.need_era("ERA_CLASSICAL", false).unwrap();
        s.need_object("md:archer").unwrap();
        assert_eq!(
            drain(&mut s),
            [
                (3, Priority::Visible),
                (1, Priority::Adjacent),
                (2, Priority::Adjacent),
                (4, Priority::Prefetch)
            ]
        );
        assert_eq!(s.progress(PriorityClass::Near), (0, 1));
        assert!(matches!(
            s.need_object("md:nothing"),
            Err(GroupError::UnknownGroup(_))
        ));
        assert!(matches!(
            s.need_civ("CIVILIZATION_NOBODY"),
            Err(GroupError::UnknownGroup(_))
        ));
    }

    #[test]
    fn an_object_is_asked_for_once_when_it_first_appears() {
        let mut s = scheduler_of(
            &["md:archer", "tx:a"],
            &[(
                "obj:md:archer",
                PriorityClass::Near,
                &["md:archer", "tx:a"],
                &[],
            )],
        );
        let first = s.object_appeared("md:archer").unwrap().unwrap();
        assert_eq!(first.queued, 2);
        assert_eq!(s.object_appeared("md:archer").unwrap(), None);
        assert_eq!(s.pending(), 2);
        assert_eq!(
            drain(&mut s),
            [(1, Priority::Visible), (2, Priority::Visible)]
        );
        // No group: an error, and the next sighting tries again.
        assert!(s.object_appeared("md:ghost").is_err());
        assert!(s.object_appeared("md:ghost").is_err());
    }

    #[test]
    fn table_from_a_blob_works() {
        let s0 = scheduler();
        let bytes = s0.table().encode();
        let mut s = GroupScheduler::from_blob(
            Some(&bytes),
            s0.manifest.clone(),
            PlatformProfile::uncompressed_only(),
        )
        .unwrap();
        assert_eq!(s.need("boot", Priority::Boot).unwrap().queued, 2);
    }
}
