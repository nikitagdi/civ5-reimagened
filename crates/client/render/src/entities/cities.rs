//! Cities: the composed city model of the owner's culture and the era, at the centre
//! of the cell, on the ground.

use asset_format::{ArtTable, LogicalId};
use glam::{Vec2, Vec3};

use super::labels::{CityBanner, LABEL_HEIGHT, LabelData, WorldLabel};
use super::{
    ArtState, CityEntity, Culture, EntityPlan, EntitySnapshot, EraBucket, ModelRef, ModelStatus,
    Owners, PlanContext, fog_of, instance, srgb_tint,
};
use crate::hexgrid::cell_center;
use crate::static_model::StaticInstance;
use crate::terrain::{Fog, terrain_height_at};

/// Culture and era used when the owner's are unknown or have no model.
pub const FALLBACK_CULTURE: Culture = Culture::Euro;
pub const FALLBACK_ERA: EraBucket = EraBucket::Ancient;

/// The city model has its final size (the scale is baked in, SIMP-021).
const CITY_SCALE: f32 = 1.0;

/// Model of a city of `culture` in `era`. Missing culture or era: the culture's Ancient model,
/// then Ancient Europe.
// SIMPLIFIED(SIMP-021): only the Ancient and Renaissance buckets are built; later eras and a
// civilization without an art style use them (the fallback order is ours, not the game's).
pub fn city_model(
    table: &ArtTable,
    culture: Option<Culture>,
    era: EraBucket,
) -> Option<&LogicalId> {
    let culture = culture.unwrap_or(FALLBACK_CULTURE);
    let era = era.b1();
    table
        .city(culture.prefix(), era.key())
        .or_else(|| table.city(culture.prefix(), FALLBACK_ERA.key()))
        .or_else(|| table.city(FALLBACK_CULTURE.prefix(), era.key()))
        .or_else(|| table.city(FALLBACK_CULTURE.prefix(), FALLBACK_ERA.key()))
        .map(|c| &c.model)
}

/// What one city adds to the plan (the counterpart of `UnitPlaced`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CityPlaced {
    pub label: Option<WorldLabel>,
    pub items: Vec<(ModelRef, StaticInstance)>,
    pub wanted: Vec<LogicalId>,
    /// The model whose arrival or failure changes this placement.
    pub deps: Vec<LogicalId>,
}

impl CityPlaced {
    /// Adds this city to `plan`.
    pub fn add_to(&self, plan: &mut EntityPlan) {
        plan.labels.extend(self.label.iter().cloned());
        plan.wanted.extend(self.wanted.iter().cloned());
        plan.city_models.extend(self.deps.iter().cloned());
        for (model, instance) in &self.items {
            plan.push(model.clone(), *instance);
        }
    }
}

/// Where one city stands and what it draws: its banner on a revealed cell, the composed model or a
/// pillar. `with_models` false leaves the banner only (WOB5: the models of `assets-b2` draw the
/// city).
pub fn place_city(
    c: &CityEntity,
    owners: &Owners,
    ctx: &PlanContext<'_>,
    with_models: bool,
) -> CityPlaced {
    let mut out = CityPlaced::default();
    if matches!(ctx.art, ArtState::Loading) {
        return out;
    }
    let Some((cell, fog)) = fog_of(ctx.terrain, c.x, c.y) else {
        return out;
    };
    if fog == Fog::Unrevealed {
        return out;
    }
    let centre: Vec2 = cell_center(c.x, c.y);
    let ground = terrain_height_at(ctx.terrain, centre);
    let at = Vec3::new(centre.x, ground, centre.y);
    out.label = Some(WorldLabel {
        cell: (c.x, c.y),
        position: at + Vec3::Y * LABEL_HEIGHT,
        data: LabelData::CityBanner(CityBanner {
            city: c.id,
            owner: c.owner,
            name: c.name.clone(),
            population: c.population,
            capital: c.capital,
            hp: c.hp,
            max_hp: c.max_hp,
        }),
    });
    if !with_models {
        return out;
    }
    let model = match ctx.art {
        ArtState::Ready(table) => {
            city_model(table, owners.culture_of(c.owner), owners.era_of(c.owner))
        }
        _ => None,
    };
    out.deps = model.into_iter().cloned().collect();
    match model.map(|m| (m, (ctx.status)(m))) {
        Some((m, ModelStatus::Ready)) => {
            out.items
                .push((ModelRef::Asset(m.clone()), instance(at, CITY_SCALE, cell)));
        }
        Some((m, ModelStatus::Pending)) => {
            out.wanted.push(m.clone());
        }
        // No model: a wide pillar in the owner's colour (SIMP-011 as for units).
        Some((_, ModelStatus::Failed)) | None => {
            let mut i = instance(at, CITY_SCALE, cell);
            i.tint = srgb_tint(owners.color_of(c.owner));
            out.items.push((ModelRef::CityMarker, i));
        }
    }
    out
}

pub(super) fn place_cities(
    snapshot: &EntitySnapshot,
    ctx: &PlanContext<'_>,
    plan: &mut EntityPlan,
) {
    if matches!(ctx.art, ArtState::Loading) {
        return;
    }
    let owners = snapshot.owners();
    let mut cities: Vec<&CityEntity> = snapshot.cities.iter().collect();
    cities.sort_by_key(|c| c.id);
    for c in cities {
        place_city(c, &owners, ctx, true).add_to(plan);
    }
}

#[cfg(test)]
mod tests {
    use asset_format::CityArt;

    use super::*;
    use crate::entities::units::tests::{plan_with, ready, snapshot, terrain};

    fn city(id: i32, owner: i8, x: i32, y: i32) -> CityEntity {
        CityEntity {
            id,
            owner,
            x,
            y,
            name: format!("City {id}"),
            population: 3,
            capital: id == 1,
            hp: 200,
            max_hp: 200,
            ..CityEntity::default()
        }
    }

    fn table() -> ArtTable {
        let mut t = ArtTable::new();
        for (culture, era) in [
            ("EUROPEAN", "Ancient"),
            ("ASIAN", "Ancient"),
            ("ASIAN", "Rennaisance"),
        ] {
            t.insert_city(
                culture,
                era,
                CityArt {
                    model: LogicalId::new(format!("model:city_{culture}_{era}")),
                },
            );
        }
        t
    }

    #[test]
    fn city_stands_in_the_cell_centre_on_the_ground() {
        let mut s = snapshot(vec![]);
        s.cities = vec![city(1, 0, 2, 2)];
        let p = plan_with(&s, &ArtState::Ready(table()), &ready);
        let v = &p.batches[&ModelRef::Asset(LogicalId::new("model:city_EUROPEAN_Ancient"))];
        let c = cell_center(2, 2);
        assert_eq!(v[0].position, [c.x, crate::terrain::LAND_HEIGHT, c.y]);
        assert_eq!(v[0].scale, 1.0);
        assert_eq!(v[0].cell as usize, 2 * 6 + 2);
        let l = &p.labels[0];
        assert_eq!(l.position.y, crate::terrain::LAND_HEIGHT + 35.0);
        assert!(matches!(&l.data, LabelData::CityBanner(b) if b.capital && b.name == "City 1"));
    }

    #[test]
    fn model_follows_culture_and_era_with_europe_as_fallback() {
        let t = table();
        assert_eq!(
            city_model(&t, Some(Culture::Asia), EraBucket::Renaissance)
                .unwrap()
                .as_str(),
            "model:city_ASIAN_Rennaisance"
        );
        // A culture without a Renaissance model keeps its own Ancient one.
        let mut t2 = t.clone();
        t2.insert_city(
            "AFRICAN",
            "Ancient",
            CityArt {
                model: LogicalId::new("model:city_AFRICAN_Ancient"),
            },
        );
        assert_eq!(
            city_model(&t2, Some(Culture::Afri), EraBucket::Renaissance)
                .unwrap()
                .as_str(),
            "model:city_AFRICAN_Ancient"
        );
        // A culture without any model uses Europe, in the same era if there is one.
        assert_eq!(
            city_model(&t, Some(Culture::Afri), EraBucket::Renaissance)
                .unwrap()
                .as_str(),
            "model:city_EUROPEAN_Ancient"
        );
        assert_eq!(
            city_model(&t, None, EraBucket::Ancient).unwrap().as_str(),
            "model:city_EUROPEAN_Ancient"
        );
        assert!(city_model(&ArtTable::new(), None, EraBucket::Ancient).is_none());
        // The owner's culture decides.
        let mut s = snapshot(vec![]);
        s.cities = vec![city(1, 1, 1, 1)];
        let p = plan_with(&s, &ArtState::Ready(t), &ready);
        assert!(
            p.batches
                .contains_key(&ModelRef::Asset(LogicalId::new("model:city_ASIAN_Ancient")))
        );
    }

    #[test]
    fn owner_era_wins_over_the_snapshot_era() {
        let mut s = snapshot(vec![]);
        s.cities = vec![city(1, 1, 1, 1)];
        s.era = EraBucket::Ancient;
        s.players[1].era = Some(EraBucket::Renaissance);
        assert_eq!(s.era_of(1), EraBucket::Renaissance);
        assert_eq!(s.era_of(0), EraBucket::Ancient);
        let p = plan_with(&s, &ArtState::Ready(table()), &ready);
        assert!(p.batches.contains_key(&ModelRef::Asset(LogicalId::new(
            "model:city_ASIAN_Rennaisance"
        ))));
    }

    #[test]
    fn cities_on_hidden_cells_are_drawn_and_unrevealed_ones_are_not() {
        let mut s = snapshot(vec![]);
        s.cities = vec![city(1, 0, 5, 5), city(2, 0, 0, 5), city(3, 0, 1, 1)];
        let t = terrain();
        assert_eq!(t.cell(5, 5).unwrap().fog, Fog::Hidden);
        let p = plan_with(&s, &ArtState::Ready(table()), &ready);
        let v = &p.batches[&ModelRef::Asset(LogicalId::new("model:city_EUROPEAN_Ancient"))];
        assert_eq!(v.len(), 2, "revealed cities only");
        assert_eq!(p.labels.len(), 2);
    }

    #[test]
    fn city_without_a_model_is_a_marker() {
        let mut s = snapshot(vec![]);
        s.cities = vec![city(1, 0, 1, 1)];
        let p = plan_with(&s, &ArtState::Ready(ArtTable::new()), &ready);
        assert_eq!(p.batches[&ModelRef::CityMarker].len(), 1);
    }
}
