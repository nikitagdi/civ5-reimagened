//! Load groups (`C5GP`, `tb:groups-b2`, 9 "Группы").
//!
//! Each set of the B2 pipeline describes its groups with [`GroupSpec`] and calls [`publish`] once its
//! blobs are in the manifest. The table in the manifest is read, the groups of the same id are
//! replaced, the byte sums of all groups are recomputed from the manifest (a set that rebuilds a
//! blob changes the sums of every group that holds it) and the table is stored again.
//!
//! The members of a group are its closure: nothing a member needs is left out, and a member that is
//! not in the manifest is an error (there are no stand-ins, AB-D14).

use std::collections::BTreeSet;

use asset_format::{
    BOOT_GROUP, FormatError, GROUP_TABLE_ID, Group, GroupTable, LogicalId, Manifest, ModelBlobV2,
    PrefetchGroup, PriorityClass, Trigger, Variant, VariantFormat,
};

use crate::models::ModelError;
use crate::store::{BlobStore, ManifestBuilder};

/// `Variant.prefetch` of the members of `boot` (AB-D10): 1 is `Boot`, 0 is "by the group table".
pub const BOOT: PrefetchGroup = PrefetchGroup(1);

/// A group as a set describes it, before the byte sum is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupSpec {
    pub id: String,
    pub class: PriorityClass,
    pub trigger: Trigger,
    pub members: Vec<LogicalId>,
    pub requires: Vec<String>,
}

impl GroupSpec {
    /// The `boot` group: always loaded before the first frame of the map.
    pub fn boot(members: Vec<LogicalId>) -> Self {
        Self {
            id: BOOT_GROUP.to_owned(),
            class: PriorityClass::Boot,
            trigger: Trigger::Always,
            members,
            requires: Vec::new(),
        }
    }

    /// A group of the `Map` class that waits for `trigger`. [`publish`] makes it require `boot`
    /// once the table has that group, whichever set is built first.
    pub fn map(id: impl Into<String>, trigger: Trigger, members: Vec<LogicalId>) -> Self {
        Self {
            id: id.into(),
            class: PriorityClass::Map,
            trigger,
            members,
            requires: Vec::new(),
        }
    }
}

impl GroupSpec {
    /// `civ:<type>` (4.13, class `Civ`): the unique units, city models and suffix variants of a
    /// civilization; asked for when the civilization is in the game.
    pub fn civ(civ_type: &str, members: Vec<LogicalId>) -> Self {
        Self::of(
            format!("civ:{civ_type}"),
            PriorityClass::Civ,
            Trigger::Civ(civ_type.to_owned()),
            members,
        )
    }

    /// `era:<era>` (class `Era`): the ordinary units, buildings, wonders and landmarks of an era;
    /// asked for when it is the player's era or the next one.
    pub fn era(era: &str, members: Vec<LogicalId>) -> Self {
        Self::of(
            format!("era:{era}"),
            PriorityClass::Era,
            Trigger::Era(era.to_owned()),
            members,
        )
    }

    /// `obj:<key>` (class `Near`): one unit type, landmark or building, by the key the client
    /// asks with (`md:archer`) when it comes into view.
    pub fn near(key: &str, members: Vec<LogicalId>) -> Self {
        Self::of(
            format!("obj:{key}"),
            PriorityClass::Near,
            Trigger::Object(key.to_owned()),
            members,
        )
    }

    fn of(id: String, class: PriorityClass, trigger: Trigger, members: Vec<LogicalId>) -> Self {
        Self {
            id,
            class,
            trigger,
            members,
            requires: Vec::new(),
        }
    }
}

/// The closure of `roots`: the roots and everything `deps` says they need, transitively, sorted
/// without repeats. The set builders of models and landmarks give `deps` (see [`blob_deps`]); a
/// group made from this list is the closure the client never has to walk (AB-D10).
pub fn closure<E>(
    roots: impl IntoIterator<Item = LogicalId>,
    mut deps: impl FnMut(&LogicalId) -> Result<Vec<LogicalId>, E>,
) -> Result<Vec<LogicalId>, E> {
    let mut seen: BTreeSet<LogicalId> = BTreeSet::new();
    let mut stack: Vec<LogicalId> = roots.into_iter().collect();
    while let Some(id) = stack.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        stack.extend(deps(&id)?);
    }
    Ok(seen.into_iter().collect())
}

/// What the blob of `id` needs, read from its bytes: for a model its textures, its clip pack
/// (`an:`, or the `C5AN` blobs of a v1 model) and its state machine (`fsm:`). Other
/// ids need nothing here. This is the one place that knows how a blob names other blobs, so a new
/// model version changes this function and nothing else.
pub fn blob_deps(id: &LogicalId, bytes: &[u8]) -> Result<Vec<LogicalId>, ModelError> {
    match id.as_str().split_once(':').map(|(p, _)| p) {
        Some("md") => {
            // `ModelBlobV2::decode` reads v1 too (its clips then name `C5AN` blobs).
            let m = ModelBlobV2::decode(bytes)
                .map_err(|e| ModelError::invalid(format!("{}: {e}", id.as_str())))?;
            let mut deps: Vec<LogicalId> = Vec::new();
            for mat in &m.materials {
                deps.extend(mat.texture_files().cloned());
            }
            deps.extend(m.anim_pack.iter().cloned());
            deps.extend(m.clips.iter().filter_map(|c| c.legacy_blob.clone()));
            deps.extend(m.fsm.iter().cloned());
            Ok(deps)
        }
        _ => Ok(Vec::new()),
    }
}

/// Problems with the closure of the groups: a model that is a member of a group needs a blob that
/// neither the group nor the groups it requires (transitively) hold. `read` gives the bytes of a
/// member (`None`: the blob is missing, which `verify` reports on its own).
pub fn closure_gaps(
    table: &GroupTable,
    read: &dyn Fn(&LogicalId) -> Option<Vec<u8>>,
) -> Vec<String> {
    let mut problems = Vec::new();
    for g in &table.groups {
        let mut available: BTreeSet<&LogicalId> = BTreeSet::new();
        let mut todo = vec![g.id.as_str()];
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        while let Some(id) = todo.pop() {
            if !seen.insert(id) {
                continue;
            }
            if let Some(h) = table.get(id) {
                available.extend(h.members.iter());
                todo.extend(h.requires.iter().map(String::as_str));
            }
        }
        for m in &g.members {
            if !m.as_str().starts_with("md:") {
                continue;
            }
            let Some(bytes) = read(m) else { continue };
            match blob_deps(m, &bytes) {
                Err(e) => problems.push(format!("group {}: {e}", g.id)),
                Ok(deps) => {
                    for d in deps.iter().filter(|d| !available.contains(d)) {
                        problems.push(format!(
                            "group {}: {} needs {}, which is not in the group or in the groups it requires",
                            g.id,
                            m.as_str(),
                            d.as_str()
                        ));
                    }
                }
            }
        }
    }
    problems
}

/// Stored size of an id: the largest of its variants, the worst case for the device that gets that
/// one. B2 ids have a single variant, so this is that variant's size. `None`: not in the manifest.
pub fn stored_size(manifest: &Manifest, id: &LogicalId) -> Option<u64> {
    manifest.variants(id).iter().map(|v| v.size).max()
}

/// The table in the manifest; empty when there is none.
pub fn load(store: &BlobStore, manifest: &ManifestBuilder) -> Result<GroupTable, ModelError> {
    let id = LogicalId::new(GROUP_TABLE_ID);
    let Some(v) = manifest.manifest().variants(&id).first() else {
        return Ok(GroupTable::new());
    };
    let path = store.blob_path(&v.hash);
    let bytes = std::fs::read(&path).map_err(|e| {
        ModelError::invalid(format!("reading the group table {}: {e}", path.display()))
    })?;
    GroupTable::decode(&bytes)
        .map_err(|e| ModelError::invalid(format!("group table in the manifest: {e}")))
}

fn describe(e: FormatError) -> ModelError {
    ModelError::invalid(format!("group table: {e}"))
}

/// Puts `specs` in the table (a group with the same id is replaced), recomputes the byte sums of
/// every group from the manifest, stores the table and adds it to the manifest in the boot
/// prefetch group. Returns the size of the table blob.
///
/// The table itself is not a member of `boot` (its size would be part of its own sum); the
/// manifest entry has `prefetch = BOOT`, so the client fetches it first.
pub fn publish(
    store: &BlobStore,
    manifest: &mut ManifestBuilder,
    specs: Vec<GroupSpec>,
) -> Result<u64, ModelError> {
    publish_with(store, manifest, specs, &|_| false)
}

/// [`publish`], first dropping the groups of the table that `stale` says are out of date (a step
/// that derives a whole family of groups, such as `obj:*`, drops the ones it no longer derives).
pub fn publish_with(
    store: &BlobStore,
    manifest: &mut ManifestBuilder,
    specs: Vec<GroupSpec>,
    stale: &dyn Fn(&Group) -> bool,
) -> Result<u64, ModelError> {
    let mut table = load(store, manifest)?;
    table.groups.retain(|g| !stale(g));
    for s in specs {
        let mut members = s.members;
        // `boot` is shared by several sets (the light maps, the overlays, the selection ring): a
        // set adds its members to the ones in the table and never takes the others out, so the
        // group does not depend on which set ran last.
        if s.class == PriorityClass::Boot
            && let Some(old) = table.get(&s.id)
        {
            members.extend(old.members.iter().cloned());
        }
        members.sort();
        members.dedup();
        let mut requires = s.requires;
        requires.sort();
        requires.dedup();
        table.upsert(Group {
            id: s.id,
            class: s.class,
            trigger: s.trigger,
            members,
            bytes: 0,
            requires,
        });
    }
    // Everything that is not boot waits for boot (when the table has it), so a set built before
    // `b2-light` gets the requirement as soon as the boot group exists.
    if table.get(BOOT_GROUP).is_some() {
        for g in &mut table.groups {
            if g.class != PriorityClass::Boot && !g.requires.iter().any(|r| r == BOOT_GROUP) {
                g.requires.push(BOOT_GROUP.to_owned());
                g.requires.sort();
            }
        }
    }
    // Every member of boot carries the boot flag (a set that named the texture later may have
    // added it with another one; `ManifestBuilder::add` merges the flag too).
    let boot_members: Vec<LogicalId> = table
        .get(BOOT_GROUP)
        .map(|g| g.members.clone())
        .unwrap_or_default();
    for id in &boot_members {
        let variants: Vec<Variant> = manifest.manifest().variants(id).to_vec();
        if variants.iter().all(|v| v.prefetch == BOOT) {
            continue;
        }
        for v in variants {
            manifest.add(
                id.clone(),
                Variant {
                    prefetch: BOOT,
                    ..v
                },
            );
        }
    }
    // Bytes of every group, from the manifest as it is now.
    let mut missing: BTreeSet<String> = BTreeSet::new();
    for g in &mut table.groups {
        let mut bytes = 0;
        for m in &g.members {
            match stored_size(manifest.manifest(), m) {
                Some(n) => bytes += n,
                None => {
                    missing.insert(format!("{} (group {})", m.as_str(), g.id));
                }
            }
        }
        g.bytes = bytes;
    }
    if !missing.is_empty() {
        return Err(ModelError::invalid(format!(
            "group members that are not in the manifest: {}",
            missing.into_iter().collect::<Vec<_>>().join(", ")
        )));
    }
    table.check().map_err(describe)?;
    let (hash, size) = store.put(&table.encode())?;
    manifest.add(
        LogicalId::new(GROUP_TABLE_ID),
        Variant {
            format: VariantFormat::Generic,
            hash,
            size,
            prefetch: BOOT,
        },
    );
    Ok(size)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use asset_format::ModelBlob;

    use super::*;

    fn temp_store(tag: &str) -> (BlobStore, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("civ5-groups-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        (BlobStore::open(&dir).unwrap(), dir)
    }

    fn add_blob(
        store: &BlobStore,
        m: &mut ManifestBuilder,
        id: &str,
        bytes: &[u8],
        prefetch: PrefetchGroup,
    ) {
        let (hash, size) = store.put(bytes).unwrap();
        m.add(
            LogicalId::new(id),
            Variant {
                format: VariantFormat::Generic,
                hash,
                size,
                prefetch,
            },
        );
    }

    /// A new build run: the manifest as the last run saved it.
    fn next_run(store: &BlobStore, m: ManifestBuilder) -> ManifestBuilder {
        m.finish(store).unwrap();
        ManifestBuilder::open(store).unwrap()
    }

    #[test]
    fn publish_sums_bytes_replaces_by_id_and_keeps_other_groups() {
        let (store, dir) = temp_store("publish");
        let mut m = ManifestBuilder::new();
        add_blob(&store, &mut m, "tx:a", &[1; 100], BOOT);
        add_blob(&store, &mut m, "tx:b", &[2; 50], PrefetchGroup(0));
        publish(
            &store,
            &mut m,
            vec![
                GroupSpec::boot(vec![LogicalId::new("tx:a")]),
                GroupSpec::map(
                    "style:europe",
                    Trigger::Style("europe".into()),
                    vec![LogicalId::new("tx:b"), LogicalId::new("tx:a")],
                ),
            ],
        )
        .unwrap();
        let t = load(&store, &m).unwrap();
        assert_eq!(t.get("boot").unwrap().bytes, 100);
        assert_eq!(t.get("style:europe").unwrap().bytes, 150);
        assert_eq!(t.get("style:europe").unwrap().requires, ["boot"]);
        let id = LogicalId::new(GROUP_TABLE_ID);
        assert_eq!(m.manifest().variants(&id)[0].prefetch, BOOT);

        // A blob is rebuilt with another size: the next publish fixes every sum that holds it.
        let mut m = next_run(&store, m);
        add_blob(&store, &mut m, "tx:a", &[3; 120], BOOT);
        publish(&store, &mut m, vec![]).unwrap();
        let t = load(&store, &m).unwrap();
        assert_eq!(t.get("boot").unwrap().bytes, 120);
        assert_eq!(t.get("style:europe").unwrap().bytes, 170);

        // Same id replaces, others stay.
        let mut m = next_run(&store, m);
        publish(
            &store,
            &mut m,
            vec![GroupSpec::boot(vec![
                LogicalId::new("tx:a"),
                LogicalId::new("tx:b"),
            ])],
        )
        .unwrap();
        let t = load(&store, &m).unwrap();
        assert_eq!(t.groups.len(), 2);
        assert_eq!(t.get("boot").unwrap().bytes, 170);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn publish_with_drops_the_stale_groups_it_is_told_to() {
        let (store, dir) = temp_store("stale");
        let mut m = ManifestBuilder::new();
        add_blob(&store, &mut m, "tx:a", &[1; 10], PrefetchGroup(0));
        let spec = |id: &str| GroupSpec::near(id, vec![LogicalId::new("tx:a")]);
        publish(&store, &mut m, vec![spec("md:x"), spec("md:y")]).unwrap();
        let mut m = next_run(&store, m);
        publish_with(&store, &mut m, vec![spec("md:y")], &|g| {
            g.id.starts_with("obj:") && g.id != "obj:md:y"
        })
        .unwrap();
        let t = load(&store, &m).unwrap();
        assert!(t.get("obj:md:x").is_none());
        assert!(t.get("obj:md:y").is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// What the boot flags and the boot group look like after the sets ran in `order`:
    /// `light` (boot: ring, cube), `misc` (adds the ring "by the table" and a map group) and
    /// `overlays` (the object-group step: puts `tx:overlay` in boot).
    fn run_sets(tag: &str, order: &[&str]) -> (Vec<(String, u16)>, Vec<String>) {
        let (store, dir) = temp_store(tag);
        let mut m = ManifestBuilder::new();
        for set in order {
            m = next_run(&store, m);
            match *set {
                "light" => {
                    add_blob(&store, &mut m, "tx:ring", &[1; 10], BOOT);
                    add_blob(&store, &mut m, "tx:cube", &[2; 10], BOOT);
                    publish(
                        &store,
                        &mut m,
                        vec![GroupSpec::boot(vec![
                            LogicalId::new("tx:ring"),
                            LogicalId::new("tx:cube"),
                        ])],
                    )
                    .unwrap();
                }
                "misc" => {
                    add_blob(&store, &mut m, "tx:ring", &[1; 10], PrefetchGroup(0));
                    add_blob(&store, &mut m, "tx:tree", &[3; 10], PrefetchGroup(0));
                    publish(
                        &store,
                        &mut m,
                        vec![GroupSpec::map(
                            "trees",
                            Trigger::Always,
                            vec![LogicalId::new("tx:tree"), LogicalId::new("tx:ring")],
                        )],
                    )
                    .unwrap();
                }
                "overlays" => {
                    add_blob(&store, &mut m, "tx:overlay", &[4; 10], BOOT);
                    publish(
                        &store,
                        &mut m,
                        vec![GroupSpec::boot(vec![LogicalId::new("tx:overlay")])],
                    )
                    .unwrap();
                }
                other => panic!("{other}"),
            }
        }
        let m = next_run(&store, m);
        let flags: Vec<(String, u16)> = m
            .manifest()
            .iter()
            .flat_map(|(id, vs)| {
                vs.iter()
                    .map(move |v| (id.as_str().to_owned(), v.prefetch.0))
            })
            .filter(|(id, _)| id != GROUP_TABLE_ID)
            .collect();
        let boot: Vec<String> = load(&store, &m)
            .unwrap()
            .get("boot")
            .unwrap()
            .members
            .iter()
            .map(|i| i.as_str().to_owned())
            .collect();
        let _ = std::fs::remove_dir_all(dir);
        (flags, boot)
    }

    #[test]
    fn boot_group_and_flags_do_not_depend_on_the_order_of_the_sets() {
        let sets = ["light", "misc", "overlays"];
        let mut perms: Vec<Vec<&str>> = Vec::new();
        for a in sets {
            for b in sets {
                for c in sets {
                    if a != b && b != c && a != c {
                        perms.push(vec![a, b, c]);
                    }
                }
            }
        }
        // Each order also runs `light` again at the end: a rebuild of one set after the others.
        let mut results = Vec::new();
        for (i, p) in perms.iter().enumerate() {
            let mut order = p.clone();
            order.push("light");
            results.push(run_sets(&format!("perm{i}"), &order));
        }
        let (flags, boot) = &results[0];
        assert_eq!(boot, &["tx:cube", "tx:overlay", "tx:ring"]);
        // The ring is in boot with the flag, the map-only texture is not.
        assert!(flags.contains(&("tx:ring".to_owned(), 1)));
        assert!(flags.contains(&("tx:tree".to_owned(), 0)));
        for r in &results[1..] {
            assert_eq!(r, &results[0]);
        }
    }

    #[test]
    fn groups_built_before_boot_get_the_requirement_when_boot_arrives() {
        let (store, dir) = temp_store("order");
        let mut m = ManifestBuilder::new();
        add_blob(&store, &mut m, "tx:a", &[1; 10], BOOT);
        add_blob(&store, &mut m, "tx:b", &[2; 10], PrefetchGroup(0));
        publish(
            &store,
            &mut m,
            vec![GroupSpec::map(
                "decals",
                Trigger::Always,
                vec![LogicalId::new("tx:b")],
            )],
        )
        .unwrap();
        assert!(
            load(&store, &m)
                .unwrap()
                .get("decals")
                .unwrap()
                .requires
                .is_empty()
        );

        let mut m = next_run(&store, m);
        publish(
            &store,
            &mut m,
            vec![GroupSpec::boot(vec![LogicalId::new("tx:a")])],
        )
        .unwrap();
        let t = load(&store, &m).unwrap();
        assert_eq!(t.get("decals").unwrap().requires, ["boot"]);
        assert!(t.get("boot").unwrap().requires.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_member_that_is_not_in_the_manifest_is_an_error() {
        let (store, dir) = temp_store("missing");
        let mut m = ManifestBuilder::new();
        let err = publish(
            &store,
            &mut m,
            vec![GroupSpec::boot(vec![LogicalId::new("tx:gone")])],
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("tx:gone") && err.contains("boot"), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }

    fn model(textures: &[&str], clips: &[&str]) -> Vec<u8> {
        use asset_format::{AnimRef, Material};
        ModelBlob {
            materials: vec![Material {
                diffuse: textures.first().map(|t| LogicalId::new(*t)),
                specular: textures.get(1).map(|t| LogicalId::new(*t)),
            }],
            animations: clips
                .iter()
                .map(|c| AnimRef {
                    name: (*c).to_owned(),
                    blob: LogicalId::new(*c),
                    events: vec![],
                })
                .collect(),
            ..ModelBlob::default()
        }
        .encode()
    }

    #[test]
    fn class_constructors_name_the_group_and_its_trigger() {
        let civ = GroupSpec::civ("CIVILIZATION_ROME", vec![]);
        assert_eq!(
            (civ.id.as_str(), civ.class, civ.trigger),
            (
                "civ:CIVILIZATION_ROME",
                PriorityClass::Civ,
                Trigger::Civ("CIVILIZATION_ROME".into())
            )
        );
        let era = GroupSpec::era("ERA_MEDIEVAL", vec![]);
        assert_eq!(
            (era.id.as_str(), era.class, era.trigger),
            (
                "era:ERA_MEDIEVAL",
                PriorityClass::Era,
                Trigger::Era("ERA_MEDIEVAL".into())
            )
        );
        let near = GroupSpec::near("md:archer", vec![]);
        assert_eq!(
            (near.id.as_str(), near.class, near.trigger),
            (
                "obj:md:archer",
                PriorityClass::Near,
                Trigger::Object("md:archer".into())
            )
        );
    }

    #[test]
    fn closure_follows_dependencies_transitively_once_and_through_cycles() {
        let deps = |id: &LogicalId| -> Result<Vec<LogicalId>, ()> {
            Ok(match id.as_str() {
                "md:a" => vec!["tx:x".into(), "an:i".into()],
                "an:i" => vec!["md:a".into()],
                _ => vec![],
            }
            .into_iter()
            .map(|s: String| LogicalId::new(s))
            .collect())
        };
        let c = closure([LogicalId::new("md:a"), LogicalId::new("tx:x")], deps).unwrap();
        let names: Vec<&str> = c.iter().map(LogicalId::as_str).collect();
        assert_eq!(names, ["an:i", "md:a", "tx:x"]);
        assert!(
            closure([LogicalId::new("md:a")], |_| Err::<Vec<LogicalId>, _>(
                "boom"
            ))
            .is_err()
        );
    }

    #[test]
    fn blob_deps_of_a_model_are_its_textures_and_clips() {
        let m = model(&["tx:d", "tx:s"], &["an:idle"]);
        let d = blob_deps(&LogicalId::new("md:archer"), &m).unwrap();
        let names: Vec<&str> = d.iter().map(LogicalId::as_str).collect();
        assert_eq!(names, ["tx:d", "tx:s", "an:idle"]);
        // Other ids need nothing; a model that does not read is an error.
        assert!(blob_deps(&LogicalId::new("tx:d"), b"x").unwrap().is_empty());
        assert!(blob_deps(&LogicalId::new("md:bad"), b"junk").is_err());
    }

    #[test]
    fn blob_deps_of_a_v2_model_are_textures_pack_and_machine() {
        use asset_format::{MaterialDesc, ParamSet, TexKind, TexParam, TexSource};
        let mut mat = MaterialDesc::default();
        mat.sets.push(ParamSet {
            textures: vec![TexParam {
                name: "DiffuseMap".into(),
                kind: TexKind::Tex2D,
                src: TexSource::File(LogicalId::new("tx:d")),
                semantic: 0,
                reg: 0,
                usage: 0,
                flags: 0,
                active: true,
            }],
            blocks: vec![],
            constants: vec![],
        });
        let m = ModelBlobV2 {
            materials: vec![mat],
            anim_pack: Some(LogicalId::new("an:archer")),
            fsm: Some(LogicalId::new("fsm:omni")),
            ..ModelBlobV2::default()
        };
        let d = blob_deps(&LogicalId::new("md:archer"), &m.encode()).unwrap();
        let names: Vec<&str> = d.iter().map(LogicalId::as_str).collect();
        assert_eq!(names, ["tx:d", "an:archer", "fsm:omni"]);
    }

    #[test]
    fn a_group_that_misses_what_its_model_needs_is_a_gap_unless_a_required_group_has_it() {
        let blobs: BTreeMap<&str, Vec<u8>> =
            BTreeMap::from([("md:archer", model(&["tx:d", "tx:s"], &["an:idle"]))]);
        let read = |id: &LogicalId| blobs.get(id.as_str()).cloned();
        let mut near = GroupSpec::near("md:archer", vec![]);
        near.members = vec![LogicalId::new("md:archer"), LogicalId::new("tx:d")];
        let mut era = GroupSpec::era("ERA_ANCIENT", vec![]);
        era.members = vec![LogicalId::new("tx:s")];
        let table = |near_requires: &[&str]| {
            let mut t = GroupTable::new();
            let mut n = near.clone();
            n.requires = near_requires.iter().map(|s| (*s).to_owned()).collect();
            for s in [n, era.clone()] {
                t.upsert(Group {
                    id: s.id,
                    class: s.class,
                    trigger: s.trigger,
                    members: s.members,
                    bytes: 0,
                    requires: s.requires,
                });
            }
            t
        };
        let gaps = closure_gaps(&table(&[]), &read);
        assert_eq!(gaps.len(), 2, "{gaps:?}");
        assert!(gaps.iter().any(|g| g.contains("tx:s")), "{gaps:?}");
        assert!(gaps.iter().any(|g| g.contains("an:idle")), "{gaps:?}");
        // `era:ERA_ANCIENT` held `tx:s`: with it required, only the clip is left.
        let gaps = closure_gaps(&table(&["era:ERA_ANCIENT"]), &read);
        assert_eq!(gaps.len(), 1, "{gaps:?}");
        assert!(gaps[0].contains("an:idle"), "{gaps:?}");
    }

    #[test]
    fn two_runs_give_the_same_table_blob() {
        let build = |tag: &str, order: bool| {
            let (store, dir) = temp_store(tag);
            let mut m = ManifestBuilder::new();
            add_blob(&store, &mut m, "tx:a", &[1; 10], BOOT);
            add_blob(&store, &mut m, "tx:b", &[2; 10], BOOT);
            let ids = if order {
                vec![LogicalId::new("tx:a"), LogicalId::new("tx:b")]
            } else {
                vec![LogicalId::new("tx:b"), LogicalId::new("tx:a")]
            };
            publish(&store, &mut m, vec![GroupSpec::boot(ids)]).unwrap();
            let v = m.manifest().variants(&LogicalId::new(GROUP_TABLE_ID))[0].hash;
            let _ = std::fs::remove_dir_all(dir);
            v
        };
        assert_eq!(build("a1", true), build("a2", false));
    }
}
