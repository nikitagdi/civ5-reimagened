//! The load groups of the world objects: `civ:<type>`, `era:<era>`,
//! `obj:<key>`, the map groups `trees` and `roads`, and the overlays in `boot`.
//!
//! The groups are derived from the art table and the manifest of an output directory, after every
//! set that fills them has run, so the result does not depend on the order of the sets and a second
//! run gives the same table. Every group holds the closure of its models ([`blob_deps`]: the
//! textures, the clip pack, the state machine) minus what `boot` already holds, so the client never
//! walks dependencies (AB-D10).
//!
//! * **`obj:<md id>`** (class `Near`): one model of a unit, a landmark, a building or a wonder (and
//!   any model that no record names, such as the damage models of cities) with its closure; asked for
//!   when the object comes into view.
//! * **`civ:<type>`** (class `Civ`): the unique units of the civilization (`Civilization_
//!   UnitClassOverrides`), the models of its unit style suffix (`ArtStyleSuffix`), the city catalogs
//!   and walls of its culture (the style type's culture when it differs from the prefix, RW-18) and
//!   the ruins.
//! * **`era:<era>`** (class `Era`): the ordinary units, the buildings and wonders and the landmarks
//!   that come in that era. The era of a unit and of a building is the era of its prerequisite
//!   technology (none: the first era); a building art tag takes the first era of its building types;
//!   a landmark is in no era group, whatever its era (only in its own `obj:` group, asked for when it
//!   is first seen: the map shows them by the hundred and they would fill the first era). An era group requires the one before it (what came in earlier is still there).
//! * **`trees`, `roads`** (class `Map`): the tree models with their overlays and masks, and the road
//!   models; asked for with the map.
//! * **The overlays** (the path models, the ring and the highlights) are members of `boot`, and
//!   their variants carry the boot flag.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use asset_format::{
    ArtRecord, ArtTable, BOOT_GROUP, GroupTable, LogicalId, Manifest, PriorityClass, Trigger,
    UnitRecordB2, Variant,
};

use super::art_table::load_table;
use super::groups::{self, BOOT, GroupSpec, blob_deps, closure};
use crate::db::{self, BuildingRow, DbError, UnitRow};
use crate::models::ModelError;
use crate::store::{BlobStore, ManifestBuilder};

/// Prefixes of the groups this step owns; a group of the table with one of them that is not
/// derived again is dropped.
const OWNED_PREFIXES: [&str; 3] = ["civ:", "era:", "obj:"];
/// Ids of the owned groups without a prefix.
const OWNED_IDS: [&str; 2] = ["trees", "roads"];

/// What the snapshot says about the objects.
#[derive(Debug, Clone, Default)]
pub struct WorldInput {
    /// `Eras.Type` in game order.
    pub eras: Vec<String>,
    pub units: Vec<UnitRow>,
    /// `(civilization, unit type)` of the unique units.
    pub unique: Vec<(String, String)>,
    pub buildings: Vec<BuildingRow>,
}

pub fn read_input(snapshot: &Path) -> Result<WorldInput, DbError> {
    let d = db::open_snapshot(snapshot)?;
    Ok(WorldInput {
        eras: db::eras(&d)?,
        units: db::all_units(&d)?,
        unique: db::unit_overrides(&d)?,
        buildings: db::all_buildings(&d)?,
    })
}

/// Closures of single models, computed once.
pub struct Closures<'a> {
    read: &'a dyn Fn(&LogicalId) -> Option<Vec<u8>>,
    memo: BTreeMap<LogicalId, Vec<LogicalId>>,
}

impl<'a> Closures<'a> {
    pub fn new(read: &'a dyn Fn(&LogicalId) -> Option<Vec<u8>>) -> Self {
        Self {
            read,
            memo: BTreeMap::new(),
        }
    }

    /// The closure of one id: itself and what the blobs need, transitively.
    pub fn of(&mut self, id: &LogicalId) -> Result<&Vec<LogicalId>, ModelError> {
        if !self.memo.contains_key(id) {
            let read = self.read;
            let c = closure([id.clone()], |x| {
                if x.as_str().starts_with("md:") {
                    let bytes = read(x).ok_or_else(|| {
                        ModelError::invalid(format!(
                            "{} is named by a record or a model but is not in the manifest",
                            x.as_str()
                        ))
                    })?;
                    blob_deps(x, &bytes)
                } else {
                    Ok(Vec::new())
                }
            })?;
            self.memo.insert(id.clone(), c);
        }
        Ok(&self.memo[id])
    }

    /// The union of the closures of `roots`.
    pub fn union(
        &mut self,
        roots: impl IntoIterator<Item = LogicalId>,
    ) -> Result<BTreeSet<LogicalId>, ModelError> {
        let mut out = BTreeSet::new();
        for r in roots {
            out.extend(self.of(&r)?.iter().cloned());
        }
        Ok(out)
    }
}

/// All models of a unit record; `style` and `era` include the variants of that kind.
fn unit_models(u: &UnitRecordB2, style: bool, era: bool) -> Vec<LogicalId> {
    let mut arts = vec![&u.base];
    if style {
        arts.extend(u.style_variants.iter().map(|v| &v.art));
    }
    if era {
        arts.extend(u.era_variants.iter().map(|v| &v.art));
    }
    arts.into_iter()
        .flat_map(|a| a.figures.iter().map(|f| f.model.clone()))
        .collect()
}

/// The place of `era` in `eras`; the first era for none or an unknown one (the second result says
/// it was unknown).
fn era_index(eras: &[String], era: Option<&str>) -> (usize, bool) {
    match era {
        None => (0, false),
        Some(e) => eras
            .iter()
            .position(|x| x == e)
            .map_or((0, true), |i| (i, false)),
    }
}

/// Groups to publish and what changes in `boot`.
#[derive(Debug, Default)]
pub struct Derived {
    pub specs: Vec<GroupSpec>,
    /// Ids that join `boot` (the overlays and what they need).
    pub boot_add: BTreeSet<LogicalId>,
    pub notes: Vec<String>,
}

/// Derives the groups of `table`. `boot_existing` is what `boot` holds now; `manifest_models` is
/// every `md:` id of the manifest.
pub fn derive(
    table: &ArtTable,
    input: &WorldInput,
    boot_existing: &BTreeSet<LogicalId>,
    manifest_models: &BTreeSet<LogicalId>,
    cl: &mut Closures<'_>,
) -> Result<Derived, ModelError> {
    let mut d = Derived::default();

    // The overlays join boot.
    let mut overlay_roots: Vec<LogicalId> = Vec::new();
    let mut tree_roots: Vec<LogicalId> = Vec::new();
    let mut road_roots: Vec<LogicalId> = Vec::new();
    for (_, rec) in table.iter() {
        match rec {
            ArtRecord::Overlay(o) => {
                overlay_roots.extend(o.model.iter().cloned());
                overlay_roots.extend(o.textures.iter().cloned());
            }
            ArtRecord::Tree(t) => {
                tree_roots.push(t.model.clone());
                tree_roots.push(t.overlay.clone());
                tree_roots.extend(t.masks.iter().cloned());
            }
            ArtRecord::Route(r) => road_roots.extend(r.models.iter().map(|m| m.model.clone())),
            _ => {}
        }
    }
    d.boot_add = cl.union(overlay_roots)?;
    let boot: BTreeSet<LogicalId> = boot_existing.union(&d.boot_add).cloned().collect();
    let strip = |s: BTreeSet<LogicalId>| -> Vec<LogicalId> {
        s.into_iter().filter(|i| !boot.contains(i)).collect()
    };

    // Map groups.
    if !tree_roots.is_empty() {
        let m = strip(cl.union(tree_roots)?);
        d.specs.push(GroupSpec::map("trees", Trigger::Always, m));
    }
    if !road_roots.is_empty() {
        let m = strip(cl.union(road_roots)?);
        d.specs.push(GroupSpec::map("roads", Trigger::Always, m));
    }

    // Units, by type.
    let units: BTreeMap<&str, &UnitRecordB2> = table
        .iter()
        .filter_map(|(k, r)| match (k.strip_prefix("unit/"), r) {
            (Some(t), ArtRecord::UnitB2(u)) => Some((t, u)),
            _ => None,
        })
        .collect();
    let unique_types: BTreeSet<&str> = input.unique.iter().map(|(_, u)| u.as_str()).collect();

    // Era groups.
    let mut era_roots: Vec<Vec<LogicalId>> = vec![Vec::new(); input.eras.len().max(1)];
    let last = era_roots.len() - 1;
    let mut unknown_eras: BTreeSet<String> = BTreeSet::new();
    let mut note_unknown = |what: &str, era: Option<&str>, unknown: bool| {
        if unknown {
            unknown_eras.insert(format!(
                "{what}: era {:?} is not in Eras",
                era.unwrap_or("")
            ));
        }
    };
    let mut units_without_record = 0usize;
    for row in &input.units {
        if unique_types.contains(row.unit_type.as_str()) {
            continue;
        }
        let Some(u) = units.get(row.unit_type.as_str()) else {
            units_without_record += 1;
            continue;
        };
        let (i, unknown) = era_index(&input.eras, row.era.as_deref());
        note_unknown(&row.unit_type, row.era.as_deref(), unknown);
        era_roots[i.min(last)].extend(unit_models(u, false, true));
    }
    if units_without_record > 0 {
        d.notes.push(format!(
            "{units_without_record} unit type(s) of the snapshot have no unit record (not in an era group)"
        ));
    }
    // The landmarks (of any era) are in no era group: each is its own `obj:` group.
    let building_era: BTreeMap<&str, Option<&str>> = input
        .buildings
        .iter()
        .map(|b| (b.building_type.as_str(), b.era.as_deref()))
        .collect();
    for (_, rec) in table.iter() {
        let ArtRecord::Building(b) = rec else {
            continue;
        };
        let mut idx = usize::MAX;
        for t in &b.building_types {
            let era = building_era.get(t.as_str()).copied().flatten();
            let (i, unknown) = era_index(&input.eras, era);
            note_unknown(t, era, unknown);
            idx = idx.min(i);
        }
        let i = if idx == usize::MAX { 0 } else { idx };
        let r = &mut era_roots[i.min(last)];
        r.extend(b.model.iter().cloned());
        r.extend(b.variants.iter().map(|v| v.model.clone()));
    }
    d.notes.extend(unknown_eras);
    let mut prev: Option<String> = None;
    for (i, roots) in era_roots.into_iter().enumerate() {
        let Some(name) = input.eras.get(i) else {
            if !roots.is_empty() {
                return Err(ModelError::invalid(
                    "models of an era, but the snapshot has no Eras",
                ));
            }
            continue;
        };
        if roots.is_empty() {
            continue;
        }
        let members = strip(cl.union(roots)?);
        let mut spec = GroupSpec::era(name, members);
        spec.requires.extend(prev.iter().cloned());
        prev = Some(spec.id.clone());
        d.specs.push(spec);
    }

    // Civilization groups.
    let mut city_models_in_civ: BTreeSet<LogicalId> = BTreeSet::new();
    let by_culture = |culture: &str| -> Vec<LogicalId> {
        let (c, w) = (format!("city/{culture}/"), format!("wall/{culture}/"));
        table
            .iter()
            .filter_map(|(k, r)| match r {
                ArtRecord::CityB2(x) if k.starts_with(&c) => Some(x.model.clone()),
                ArtRecord::Wall(x) if k.starts_with(&w) => Some(x.model.clone()),
                _ => None,
            })
            .collect()
    };
    let ruins = by_culture("RUINS");
    for (_, rec) in table.iter() {
        let ArtRecord::Civ(c) = rec else { continue };
        let mut roots: Vec<LogicalId> = Vec::new();
        for (civ, unit) in input.unique.iter().filter(|p| p.0 == c.civ) {
            match units.get(unit.as_str()) {
                Some(u) => roots.extend(unit_models(u, true, true)),
                None => d
                    .notes
                    .push(format!("{civ}: the unique unit {unit} has no unit record")),
            }
        }
        if let Some(suffix) = &c.suffix {
            for u in units.values() {
                for v in u.style_variants.iter().filter(|v| v.suffix == *suffix) {
                    roots.extend(v.art.figures.iter().map(|f| f.model.clone()));
                }
            }
        }
        if let Some(culture) = c.style_culture.as_ref().or(c.prefix.as_ref()) {
            let city = by_culture(culture);
            if city.is_empty() {
                d.notes.push(format!(
                    "{}: no city catalog for the culture {culture}",
                    c.civ
                ));
            }
            city_models_in_civ.extend(city.iter().cloned());
            roots.extend(city);
            city_models_in_civ.extend(ruins.iter().cloned());
            roots.extend(ruins.iter().cloned());
        }
        if roots.is_empty() {
            continue;
        }
        let members = strip(cl.union(roots)?);
        d.specs.push(GroupSpec::civ(&c.civ, members));
    }

    // Objects: every model that a record shows one at a time, and the models no record names.
    let mut by_kind: BTreeMap<&str, BTreeSet<&LogicalId>> = BTreeMap::new();
    for (kind, id) in table.models() {
        by_kind.entry(kind).or_default().insert(id);
    }
    let mut objects: BTreeSet<LogicalId> = BTreeSet::new();
    for kind in ["unit", "landmark", "building", "wonder"] {
        objects.extend(
            by_kind
                .get(kind)
                .into_iter()
                .flatten()
                .map(|i| (*i).clone()),
        );
    }
    let named: BTreeSet<&LogicalId> = by_kind.values().flatten().copied().collect();
    objects.extend(
        manifest_models
            .iter()
            .filter(|m| !named.contains(m))
            .cloned(),
    );
    // A city or a wall that no civilization holds can still be asked for by itself.
    for kind in ["city", "wall"] {
        objects.extend(
            by_kind
                .get(kind)
                .into_iter()
                .flatten()
                .filter(|i| !city_models_in_civ.contains(**i))
                .map(|i| (*i).clone()),
        );
    }
    for id in &objects {
        if boot.contains(id) {
            continue;
        }
        let members = strip(cl.union([id.clone()])?);
        d.specs.push(GroupSpec::near(id.as_str(), members));
    }
    Ok(d)
}

/// The size limit of a group of `class` in bytes, for the classes this step owns
/// (`Civ` up to 15 MB, `Era` up to 40 MB, `Near` up to 1 MB).
pub fn class_limit(class: PriorityClass) -> Option<u64> {
    match class {
        PriorityClass::Civ => Some(15_000_000),
        PriorityClass::Era => Some(40_000_000),
        PriorityClass::Near => Some(1_000_000),
        _ => None,
    }
}

/// Cuts every group over the limit of its class into parts of at most that many bytes. The group
/// keeps its id, its trigger and its first part, and requires the others (`<id>#2`, `<id>#3`, ...,
/// groups of the same class); the client's `need` brings required groups along, so the closure is
/// the same and no member is dropped. Members go first-fit by size, biggest first; the part with the
/// first model (`md:`) is the one the group keeps. A single blob over the limit is a part of its
/// own and is noted: a blob is not cut.
fn split_oversized(
    specs: Vec<GroupSpec>,
    size: &dyn Fn(&LogicalId) -> u64,
    notes: &mut Vec<String>,
) -> Vec<GroupSpec> {
    let mut out = Vec::with_capacity(specs.len());
    for spec in specs {
        let limit = match class_limit(spec.class) {
            Some(l) if spec.members.iter().map(size).sum::<u64>() > l => l,
            _ => {
                out.push(spec);
                continue;
            }
        };
        let mut members: Vec<(u64, LogicalId)> =
            spec.members.iter().map(|m| (size(m), m.clone())).collect();
        members.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        let mut bins: Vec<(u64, Vec<LogicalId>)> = Vec::new();
        for (n, m) in members {
            if n > limit {
                notes.push(format!(
                    "{}: {} alone is {:.2} MB, over the {:.0} MB limit; it is not cut",
                    spec.id,
                    m.as_str(),
                    n as f64 / 1e6,
                    limit as f64 / 1e6
                ));
            }
            match bins.iter_mut().find(|b| b.0 + n <= limit) {
                Some(b) => {
                    b.0 += n;
                    b.1.push(m);
                }
                None => bins.push((n, vec![m])),
            }
        }
        // The part with the first model is the head of the group.
        let first_model = spec.members.iter().find(|m| m.as_str().starts_with("md:"));
        let head = first_model
            .and_then(|f| bins.iter().position(|b| b.1.contains(f)))
            .unwrap_or(0);
        let head_bin = bins.remove(head);
        let mut parts = Vec::with_capacity(bins.len());
        for (i, (_, members)) in bins.into_iter().enumerate() {
            let mut part = spec.clone();
            part.id = format!("{}#{}", spec.id, i + 2);
            part.members = members;
            part.requires = Vec::new();
            parts.push(part);
        }
        let mut head_spec = spec;
        head_spec.members = head_bin.1;
        head_spec
            .requires
            .extend(parts.iter().map(|p| p.id.clone()));
        out.push(head_spec);
        out.extend(parts);
    }
    out
}

/// What a run did.
#[derive(Debug, Default)]
pub struct Summary {
    pub groups: BTreeMap<String, usize>,
    pub boot_added: usize,
    pub group_table_bytes: u64,
    pub notes: Vec<String>,
    /// `(id, bytes)` of the largest group of each class.
    pub largest: BTreeMap<String, (String, u64)>,
    /// Groups that are over their size estimate by the report's rule are for the report; here the
    /// ones of no member.
    pub empty: Vec<String>,
}

fn read_from<'a>(
    store: &'a BlobStore,
    manifest: &'a Manifest,
) -> impl Fn(&LogicalId) -> Option<Vec<u8>> + 'a {
    move |id: &LogicalId| {
        let v = manifest.variants(id).first()?;
        std::fs::read(store.blob_path(&v.hash)).ok()
    }
}

/// Whether the art table has records of the objects this step makes groups for.
fn has_world_records(table: &ArtTable) -> bool {
    table.iter().any(|(_, r)| {
        matches!(
            r,
            ArtRecord::UnitB2(_)
                | ArtRecord::Landmark(_)
                | ArtRecord::Building(_)
                | ArtRecord::Wonder(_)
                | ArtRecord::CityB2(_)
                | ArtRecord::Tree(_)
                | ArtRecord::Route(_)
                | ArtRecord::Overlay(_)
        )
    })
}

/// Derives and publishes the groups of the output directory `out`; `None` when the art table has
/// nothing for them.
pub fn run(out: &Path, input: &WorldInput) -> Result<Option<Summary>, ModelError> {
    let store = BlobStore::open(out)?;
    let mut manifest = ManifestBuilder::open(&store)?;
    let table = load_table(&store, &manifest)?;
    if !has_world_records(&table) {
        return Ok(None);
    }
    let old: GroupTable = groups::load(&store, &manifest)?;
    let boot_existing: BTreeSet<LogicalId> = old
        .get(BOOT_GROUP)
        .map(|g| g.members.iter().cloned().collect())
        .unwrap_or_default();
    let manifest_models: BTreeSet<LogicalId> = manifest
        .manifest()
        .iter()
        .map(|(id, _)| id.clone())
        .filter(|id| id.as_str().starts_with("md:"))
        .collect();
    let mut derived = {
        let read = read_from(&store, manifest.manifest());
        let mut cl = Closures::new(&read);
        derive(&table, input, &boot_existing, &manifest_models, &mut cl)?
    };

    // A group over the limit of its class is cut into parts the group requires (RP21).
    let sizes = |m: &LogicalId| groups::stored_size(manifest.manifest(), m).unwrap_or(0);
    derived.specs = split_oversized(
        std::mem::take(&mut derived.specs),
        &sizes,
        &mut derived.notes,
    );

    // Every member of boot carries the boot flag: the overlays are new members, and a texture that
    // a model set rebuilt after `b2-light` (the shared lighting maps) lost it.
    let mut boot: BTreeSet<LogicalId> = boot_existing;
    boot.extend(derived.boot_add.iter().cloned());
    // The art table is read first, so it is boot's.
    let table_id = LogicalId::new(super::art_table::ART_TABLE_B2_ID);
    if !manifest.manifest().variants(&table_id).is_empty() {
        boot.insert(table_id);
    }
    for id in &boot {
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
    if !boot.is_empty() {
        derived
            .specs
            .push(GroupSpec::boot(boot.into_iter().collect()));
    }
    let keep: BTreeSet<String> = derived.specs.iter().map(|s| s.id.clone()).collect();
    let group_table_bytes = groups::publish_with(&store, &mut manifest, derived.specs, &|g| {
        (OWNED_PREFIXES.iter().any(|p| g.id.starts_with(p)) || OWNED_IDS.contains(&g.id.as_str()))
            && !keep.contains(&g.id)
    })?;
    manifest.finish(&store)?;

    let table = groups::load(&store, &manifest)?;
    let mut s = Summary {
        boot_added: derived.boot_add.len(),
        group_table_bytes,
        notes: derived.notes,
        ..Summary::default()
    };
    for g in &table.groups {
        let class = format!("{:?}", g.class);
        *s.groups.entry(class.clone()).or_default() += 1;
        let e = s.largest.entry(class).or_insert_with(|| (g.id.clone(), 0));
        if g.bytes > e.1 {
            *e = (g.id.clone(), g.bytes);
        }
        if g.members.is_empty() {
            s.empty.push(g.id.clone());
        }
    }
    Ok(Some(s))
}

/// The budget article of every id the objects own: models with their clips, state machines and
/// textures by what shows them (units, landmarks, city, building and wonder art, trees, roads,
/// overlays). An id several objects use goes to the first article in that order.
pub fn owners(
    out: &Path,
    manifest: &Manifest,
) -> Result<BTreeMap<LogicalId, &'static str>, ModelError> {
    let store = BlobStore::open(out)?;
    let Some(v) = manifest
        .variants(&LogicalId::new(super::art_table::ART_TABLE_B2_ID))
        .first()
    else {
        return Ok(BTreeMap::new());
    };
    let bytes = std::fs::read(store.blob_path(&v.hash))
        .map_err(|e| ModelError::invalid(format!("reading the art table: {e}")))?;
    let table =
        ArtTable::decode(&bytes).map_err(|e| ModelError::invalid(format!("art table: {e}")))?;
    let read = read_from(&store, manifest);
    let mut cl = Closures::new(&read);
    let mut owners: BTreeMap<LogicalId, &'static str> = BTreeMap::new();
    let mut take = |ids: BTreeSet<LogicalId>, article: &'static str| {
        for id in ids {
            owners.entry(id).or_insert(article);
        }
    };
    let mut roots: BTreeMap<&'static str, Vec<LogicalId>> = BTreeMap::new();
    for (kind, id) in table.models() {
        let article = match kind {
            "unit" => "units",
            "landmark" => "landmarks",
            "building" | "wonder" | "city" | "wall" => "city-bldg-wonder",
            "tree" => "trees",
            "route" => "roads",
            _ => "fow-overlay",
        };
        roots.entry(article).or_default().push(id.clone());
    }
    for (_, rec) in table.iter() {
        match rec {
            ArtRecord::Tree(t) => {
                let r = roots.entry("trees").or_default();
                r.push(t.overlay.clone());
                r.extend(t.masks.iter().cloned());
            }
            ArtRecord::Overlay(o) => roots
                .entry("fow-overlay")
                .or_default()
                .extend(o.textures.iter().cloned()),
            _ => {}
        }
    }
    // Models no record names (the damage models of cities) belong with the city art.
    let named: BTreeSet<LogicalId> = table.models().into_iter().map(|(_, i)| i.clone()).collect();
    let orphans: Vec<LogicalId> = manifest
        .iter()
        .map(|(id, _)| id.clone())
        .filter(|id| id.as_str().starts_with("md:") && !named.contains(id))
        .collect();
    roots.entry("city-bldg-wonder").or_default().extend(orphans);
    for article in [
        "units",
        "landmarks",
        "city-bldg-wonder",
        "trees",
        "roads",
        "fow-overlay",
    ] {
        if let Some(r) = roots.remove(article) {
            take(cl.union(r)?, article);
        }
    }
    Ok(owners)
}

#[cfg(test)]
mod tests {
    use asset_format::{
        CityArtB2, CivArt, FigureB2, LandmarkArt, ModelBlobV2, OverlayArt, RouteArt, RouteModel,
        TreeArt, UnitArtB2, UnitVariantB2, WallArt,
    };

    use super::*;

    fn id(s: &str) -> LogicalId {
        LogicalId::new(s)
    }

    fn art(model: &str) -> UnitArtB2 {
        UnitArtB2 {
            art_info: "ART".into(),
            figures: vec![FigureB2 {
                member: "M".into(),
                model: id(model),
                count: 1,
                scale: None,
                z_offset: None,
                domain: None,
            }],
            damage_states: None,
            formation: None,
        }
    }

    fn unit_row(t: &str, era: Option<&str>) -> UnitRow {
        UnitRow {
            unit_type: t.into(),
            art_info: String::new(),
            prereq_tech: None,
            era: era.map(str::to_owned),
            cultural_variation: false,
            era_variation: false,
        }
    }

    fn landmark(era: &str, model: &str) -> LandmarkArt {
        LandmarkArt {
            improvement: "I".into(),
            resource: "R".into(),
            era: era.into(),
            state: "Any".into(),
            layout: "SNAPSHOT".into(),
            scale: 1.0,
            terrain_contour: 0,
            tech: None,
            model: Some(id(model)),
        }
    }

    fn civ(t: &str, prefix: Option<&str>, suffix: Option<&str>, style: Option<&str>) -> CivArt {
        CivArt {
            civ: t.into(),
            art_define_tag: None,
            art_style: None,
            prefix: prefix.map(str::to_owned),
            suffix: suffix.map(str::to_owned),
            default_color: None,
            style_culture: style.map(str::to_owned),
        }
    }

    /// A table with ordinary and unique units, a style variant, landmarks, a building, two
    /// cultures with a city and a wall each, ruins, trees, a road and an overlay.
    fn world() -> (ArtTable, WorldInput, BTreeSet<LogicalId>) {
        let mut t = ArtTable::new();
        let mut archer = UnitRecordB2 {
            base: art("md:archer"),
            style_variants: vec![UnitVariantB2 {
                suffix: "_EURO".into(),
                art: art("md:archer_euro"),
            }],
            era_variation: false,
            era_variants: vec![],
        };
        t.insert_unit_b2("UNIT_ARCHER", archer.clone());
        archer.style_variants.clear();
        archer.base = art("md:tank");
        t.insert_unit_b2("UNIT_TANK", archer.clone());
        archer.base = art("md:legion");
        t.insert_unit_b2("UNIT_LEGION", archer);
        t.insert_landmark(0, landmark("Any", "md:farm"));
        t.insert_landmark(1, landmark("Industrial", "md:factory_land"));
        t.insert_city_b2(
            "EUROPEAN",
            "Ancient",
            CityArtB2 {
                model: id("md:euro_city"),
                parts: 3,
                lighting: None,
            },
        );
        t.insert_wall(
            "EUROPEAN",
            "Ancient",
            WallArt {
                model: id("md:euro_wall"),
                parts: 1,
            },
        );
        t.insert_city_b2(
            "RUINS",
            "Ancient",
            CityArtB2 {
                model: id("md:ruins"),
                parts: 1,
                lighting: None,
            },
        );
        t.insert_city_b2(
            "ASIAN",
            "Ancient",
            CityArtB2 {
                model: id("md:asia_city"),
                parts: 1,
                lighting: None,
            },
        );
        t.insert_civ(civ("CIV_ROME", Some("EUROPEAN"), Some("_EURO"), None));
        t.insert_civ(civ("CIV_JAPAN", Some("ASIAN"), Some("_ASIA"), None));
        t.insert_civ(civ("CIV_MINOR", None, None, None));
        t.insert_tree(TreeArt {
            feature: "FEATURE_FOREST".into(),
            style: 4,
            model: id("md:forest"),
            overlay: id("tx:overlay"),
            masks: vec![id("tx:tile1")],
        });
        t.insert_route(RouteArt {
            route: "ROUTE_ROAD".into(),
            models: vec![RouteModel {
                class: "road".into(),
                model: id("md:road"),
            }],
        });
        t.insert_overlay(OverlayArt {
            name: "path".into(),
            kind: "path_center".into(),
            variant: String::new(),
            model: Some(id("md:path")),
            textures: vec![],
        });
        let input = WorldInput {
            eras: vec![
                "ERA_ANCIENT".into(),
                "ERA_INDUSTRIAL".into(),
                "ERA_MODERN".into(),
            ],
            units: vec![
                unit_row("UNIT_ARCHER", None),
                unit_row("UNIT_TANK", Some("ERA_MODERN")),
                unit_row("UNIT_LEGION", Some("ERA_ANCIENT")),
            ],
            unique: vec![("CIV_ROME".into(), "UNIT_LEGION".into())],
            buildings: vec![],
        };
        let models: BTreeSet<LogicalId> = [
            "md:archer",
            "md:archer_euro",
            "md:tank",
            "md:legion",
            "md:farm",
            "md:factory_land",
            "md:euro_city",
            "md:euro_wall",
            "md:ruins",
            "md:asia_city",
            "md:forest",
            "md:road",
            "md:path",
            "md:fxm_damage",
        ]
        .iter()
        .map(|s| id(s))
        .collect();
        (t, input, models)
    }

    fn derive_world() -> (Derived, BTreeMap<String, Vec<String>>) {
        let (t, input, models) = world();
        // Every model is a plain blob with no dependencies; `tx:` ids need no blob.
        let blob = ModelBlobV2::default().encode();
        let read = |i: &LogicalId| models.contains(i).then(|| blob.clone());
        let mut cl = Closures::new(&read);
        let d = derive(&t, &input, &BTreeSet::new(), &models, &mut cl).unwrap();
        let by: BTreeMap<String, Vec<String>> = d
            .specs
            .iter()
            .map(|s| {
                (
                    s.id.clone(),
                    s.members.iter().map(|m| m.as_str().to_owned()).collect(),
                )
            })
            .collect();
        (d, by)
    }

    #[test]
    fn era_groups_hold_the_ordinary_units_and_the_landmarks_of_the_era_and_chain() {
        let (d, by) = derive_world();
        // Archer: no tech, the first era; the landmark `Any` is in no era (only `obj:`). The unique
        // legion is not here.
        assert_eq!(by["era:ERA_ANCIENT"], ["md:archer"]);
        // The industrial landmark is not in an era group either.
        assert!(!by.contains_key("era:ERA_INDUSTRIAL"));
        assert_eq!(by["era:ERA_MODERN"], ["md:tank"]);
        let req = |id: &str| {
            d.specs
                .iter()
                .find(|s| s.id == id)
                .unwrap()
                .requires
                .clone()
        };
        assert!(req("era:ERA_ANCIENT").is_empty());
        assert_eq!(req("era:ERA_MODERN"), ["era:ERA_ANCIENT"]);
    }

    #[test]
    fn a_civ_group_has_its_unique_units_suffix_variants_culture_and_ruins() {
        let (_, by) = derive_world();
        assert_eq!(
            by["civ:CIV_ROME"],
            [
                "md:archer_euro",
                "md:euro_city",
                "md:euro_wall",
                "md:legion",
                "md:ruins"
            ]
        );
        assert_eq!(by["civ:CIV_JAPAN"], ["md:asia_city", "md:ruins"]);
        // A civilization with nothing has no group.
        assert!(!by.contains_key("civ:CIV_MINOR"));
    }

    #[test]
    fn the_style_culture_wins_over_the_prefix() {
        let (mut t, input, models) = world();
        t.insert_civ(civ("CIV_POLY", Some("EUROPEAN"), None, Some("ASIAN")));
        let blob = ModelBlobV2::default().encode();
        let read = |i: &LogicalId| models.contains(i).then(|| blob.clone());
        let mut cl = Closures::new(&read);
        let d = derive(&t, &input, &BTreeSet::new(), &models, &mut cl).unwrap();
        let g = d.specs.iter().find(|s| s.id == "civ:CIV_POLY").unwrap();
        let names: Vec<&str> = g.members.iter().map(LogicalId::as_str).collect();
        assert_eq!(names, ["md:asia_city", "md:ruins"]);
    }

    #[test]
    fn map_groups_and_boot_overlays() {
        let (d, by) = derive_world();
        assert_eq!(by["trees"], ["md:forest", "tx:overlay", "tx:tile1"]);
        assert_eq!(by["roads"], ["md:road"]);
        assert_eq!(
            d.boot_add.iter().map(LogicalId::as_str).collect::<Vec<_>>(),
            ["md:path"]
        );
        // An overlay is not an object of its own.
        assert!(!by.contains_key("obj:md:path"));
    }

    #[test]
    fn objects_are_models_shown_one_at_a_time_and_models_no_record_names() {
        let (_, by) = derive_world();
        for m in [
            "md:archer",
            "md:archer_euro",
            "md:tank",
            "md:legion",
            "md:farm",
            "md:factory_land",
            "md:fxm_damage",
        ] {
            assert_eq!(by[&format!("obj:{m}")], [m], "{m}");
        }
        // A city that a civilization holds is not an object; trees and roads are not either.
        for m in ["md:euro_city", "md:forest", "md:road", "md:ruins"] {
            assert!(!by.contains_key(&format!("obj:{m}")), "{m}");
        }
    }

    #[test]
    fn what_boot_holds_is_left_out_of_the_groups() {
        let (t, input, models) = world();
        let blob = ModelBlobV2::default().encode();
        let read = |i: &LogicalId| models.contains(i).then(|| blob.clone());
        let mut cl = Closures::new(&read);
        let boot: BTreeSet<LogicalId> = [id("md:ruins"), id("md:archer")].into();
        let d = derive(&t, &input, &boot, &models, &mut cl).unwrap();
        let g = d.specs.iter().find(|s| s.id == "civ:CIV_JAPAN").unwrap();
        assert_eq!(g.members, [id("md:asia_city")]);
        assert!(d.specs.iter().all(|s| s.id != "obj:md:archer"));
    }

    #[test]
    fn a_model_that_is_not_in_the_manifest_is_an_error() {
        let (t, input, models) = world();
        let read = |_: &LogicalId| None;
        let mut cl = Closures::new(&read);
        let e = derive(&t, &input, &BTreeSet::new(), &models, &mut cl).unwrap_err();
        assert!(e.to_string().contains("not in the manifest"), "{e}");
    }

    #[test]
    fn a_group_over_its_limit_is_cut_into_required_parts_without_losing_a_member() {
        let sz = |m: &LogicalId| match m.as_str() {
            "md:a" => 100_000,
            "tx:big1" | "tx:big2" => 600_000,
            "tx:small" => 300_000,
            "tx:huge" => 1_500_000,
            _ => 0,
        };
        let spec = GroupSpec::near(
            "md:a",
            ["md:a", "tx:big1", "tx:big2", "tx:small"].map(id).to_vec(),
        );
        let small = GroupSpec::near("md:b", vec![id("md:b")]);
        let mut notes = Vec::new();
        let out = split_oversized(vec![spec, small.clone()], &sz, &mut notes);
        assert!(notes.is_empty());
        let head = out.iter().find(|s| s.id == "obj:md:a").unwrap();
        assert!(head.members.contains(&id("md:a")));
        assert_eq!(head.requires, ["obj:md:a#2"]);
        let mut all: Vec<&LogicalId> = out
            .iter()
            .filter(|s| s.id.starts_with("obj:md:a"))
            .flat_map(|s| &s.members)
            .collect();
        all.sort();
        let want = ["md:a", "tx:big1", "tx:big2", "tx:small"].map(id);
        assert_eq!(all, want.iter().collect::<Vec<_>>());
        for s in out.iter().filter(|s| s.id.starts_with("obj:md:a")) {
            assert!(
                s.members.iter().map(&sz).sum::<u64>() <= 1_000_000,
                "{}",
                s.id
            );
        }
        assert!(
            out.contains(&small),
            "a group under its limit is kept as it is"
        );

        // A blob over the limit is not cut: a part of its own, with a note.
        let big = GroupSpec::near("md:c", vec![id("md:c"), id("tx:huge")]);
        let out = split_oversized(vec![big], &sz, &mut notes);
        assert_eq!(out.len(), 2);
        assert!(notes[0].contains("tx:huge"), "{notes:?}");
    }

    #[test]
    fn landmark_and_unit_eras_map_to_the_game_eras() {
        let eras: Vec<String> = ["ERA_ANCIENT", "ERA_INDUSTRIAL"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        assert_eq!(era_index(&eras, None), (0, false));
        assert_eq!(era_index(&eras, Some("ERA_INDUSTRIAL")), (1, false));
    }
}
