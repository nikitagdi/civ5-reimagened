//! Unit flags and city banners (play-ui §3.3, AC-4.3, AC-4.4) as the interim UI's view: the
//! renderer's `WorldLabel`s projected with the game camera, with the owners' colours, HP, the
//! selection, production of own cities and whether a city can shoot now.
//!
//! "Can shoot now" is the view's `CityFlags::CAN_RANGE_ATTACK` of the city (the core's
//! `CanRangeStrikeNow`): a banner of an own city with it set, in the human's turn, gets the "Strike"
//! button. The B1 `CityStrikes` that asked `Q_RANGE_TARGETS` for every city is gone (NC5).
//!
//! SIMPLIFIED(SIMP-014): a flag shows two letters of the unit type ([`type_mark`]) instead of the
//! unit's flag icon; original: `UnitFlagManager` with the unit flag atlases.

use interim_ui::labels::{BannerView, FlagView, MapLabelsView};
use protocol::{Catalog, CityFlags, TurnStatus, WorldView};
use render::TerrainSnapshot;
use render::entities::{LabelData, WorldLabel, project_wrapped_label};
use render::glam::{Mat4, Vec2, Vec3};

/// The camera the labels are projected with.
#[derive(Debug, Clone, Copy)]
pub struct LabelCamera {
    pub view_proj: Mat4,
    pub eye: Vec3,
    /// The surface in physical pixels.
    pub viewport: Vec2,
    /// Physical pixels per egui point.
    pub scale: f32,
    /// The world width of a map that wraps along X.
    pub wrap_width: Option<f32>,
}

/// "UNIT_WARRIOR" -> "Wa", "UNIT_WORK_BOAT" -> "WB".
pub fn type_mark(type_name: &str) -> String {
    let t = type_name.strip_prefix("UNIT_").unwrap_or(type_name);
    let words: Vec<&str> = t.split('_').filter(|w| !w.is_empty()).collect();
    let first = |w: &str| w.chars().next().map(|c| c.to_ascii_uppercase());
    match words.as_slice() {
        [] => "?".to_owned(),
        [one] => first(one)
            .into_iter()
            .chain(one.chars().nth(1).map(|c| c.to_ascii_lowercase()))
            .collect(),
        [a, b, ..] => first(a).into_iter().chain(first(b)).collect(),
    }
}

fn item_name(catalog: Option<&Catalog>, order: i8, item: i16) -> Option<String> {
    let table = catalog.and_then(|c| match order {
        0 => Some(&c.units),
        1 => Some(&c.buildings),
        2 => Some(&c.projects),
        _ => None,
    })?;
    table.iter().find(|e| e.id == item).map(|e| e.name.clone())
}

/// The flags and banners of `labels` on the screen.
pub fn labels_view(
    labels: &[WorldLabel],
    terrain: Option<&TerrainSnapshot>,
    cam: &LabelCamera,
    world: &WorldView,
    catalog: Option<&Catalog>,
    selected: Option<i32>,
) -> MapLabelsView {
    let mut out = MapLabelsView::default();
    let colors = |owner: i8| {
        world
            .players
            .iter()
            .find(|p| p.id == owner)
            .map_or((0x80_80_80_FF, 0xFF_FF_FF_FF), |p| {
                (p.color_primary, p.color_secondary)
            })
    };
    let civ = |owner: i8| {
        world
            .players
            .iter()
            .find(|p| p.id == owner)
            .map(|p| p.civ_name.clone())
            .unwrap_or_default()
    };
    let human_turn = world.status == TurnStatus::HumanTurn;
    for l in labels {
        let s = project_wrapped_label(
            l.position,
            cam.wrap_width,
            &cam.view_proj,
            cam.viewport,
            cam.eye,
            terrain,
        );
        if !s.visible {
            continue;
        }
        let pos = [s.pos.x / cam.scale, s.pos.y / cam.scale];
        let health = |hp: i32, max: i32| (max > 0 && hp < max).then(|| hp as f32 / max as f32);
        match &l.data {
            LabelData::UnitFlag(f) => {
                let (primary, secondary) = colors(f.owner);
                let own = f.owner == world.human;
                let name = world
                    .units
                    .iter()
                    .find(|u| u.id == f.unit && u.owner == f.owner)
                    .map_or_else(|| f.type_name.clone(), |u| u.name.clone());
                let whose = if own { "Your".to_owned() } else { civ(f.owner) };
                out.flags.push(FlagView {
                    unit: f.unit,
                    cell: l.cell,
                    pos,
                    depth: s.depth,
                    primary,
                    secondary,
                    mark: type_mark(&f.type_name),
                    health: health(f.hp, f.max_hp),
                    needs_orders: own && f.needs_orders,
                    selected: own && selected == Some(f.unit),
                    civilian: f.civilian,
                    tooltip: format!("{whose} {name}, HP {}/{}", f.hp, f.max_hp),
                });
            }
            LabelData::CityBanner(b) => {
                let (primary, secondary) = colors(b.owner);
                let own = b.owner == world.human;
                let city = world
                    .cities
                    .iter()
                    .find(|c| c.id == b.city && c.owner == b.owner);
                let production = city.filter(|_| own).map(|c| {
                    match item_name(catalog, c.production_order, c.production_item) {
                        Some(n) if c.production_turns >= 0 => {
                            format!("{n} ({})", c.production_turns)
                        }
                        Some(n) => n,
                        None => "Choose production".to_owned(),
                    }
                });
                let owner = if own {
                    String::new()
                } else {
                    format!(" ({})", civ(b.owner))
                };
                out.banners.push(BannerView {
                    city: b.city,
                    cell: l.cell,
                    pos,
                    depth: s.depth,
                    primary,
                    secondary,
                    name: b.name.clone(),
                    population: b.population,
                    capital: b.capital,
                    tooltip: format!(
                        "{}{owner}: population {}, HP {}/{}{}",
                        b.name,
                        b.population,
                        b.hp,
                        b.max_hp,
                        production
                            .as_ref()
                            .map(|p| format!(", producing {p}"))
                            .unwrap_or_default()
                    ),
                    production,
                    health: health(b.hp, b.max_hp),
                    strike: own
                        && human_turn
                        && city.is_some_and(|c| c.flags.contains(CityFlags::CAN_RANGE_ATTACK)),
                });
            }
            LabelData::PathTurn(_) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use render::entities::{CityBanner, UnitFlag};

    fn world() -> WorldView {
        crate::test_world::world()
    }

    /// Looking straight down at the origin from 100 up: a point at the origin is the screen middle.
    fn camera() -> LabelCamera {
        let eye = Vec3::new(0.0, 100.0, 0.0);
        let view = render::glam::camera::rh::view::look_at_mat4(eye, Vec3::ZERO, Vec3::NEG_Z);
        let proj = render::glam::camera::rh::proj::directx::perspective(1.0, 1.0, 1.0, 1000.0);
        LabelCamera {
            view_proj: proj * view,
            eye,
            viewport: Vec2::new(800.0, 800.0),
            scale: 2.0,
            wrap_width: None,
        }
    }

    fn flag(unit: i32, owner: i8, at: Vec3) -> WorldLabel {
        WorldLabel {
            cell: (8, 11),
            position: at,
            data: LabelData::UnitFlag(UnitFlag {
                unit,
                owner,
                type_name: "UNIT_ARCHER".into(),
                hp: 6,
                max_hp: 10,
                needs_orders: true,
                civilian: false,
            }),
        }
    }

    #[test]
    fn flags_and_banners_on_screen_in_points() {
        let w = world();
        let labels = [
            flag(4, 0, Vec3::ZERO),
            flag(9, 1, Vec3::new(5000.0, 0.0, 0.0)), // off screen
            WorldLabel {
                cell: (6, 12),
                position: Vec3::new(10.0, 0.0, 0.0),
                data: LabelData::CityBanner(CityBanner {
                    city: 1,
                    owner: 0,
                    name: "Roma".into(),
                    population: 3,
                    capital: true,
                    hp: 20,
                    max_hp: 20,
                }),
            },
        ];
        let v = labels_view(
            &labels,
            None,
            &camera(),
            &w,
            Some(&crate::test_world::catalog()),
            Some(4),
        );
        assert_eq!(v.flags.len(), 1, "the off-screen flag is left out");
        let f = &v.flags[0];
        assert!(
            (f.pos[0] - 200.0).abs() < 0.5 && (f.pos[1] - 200.0).abs() < 0.5,
            "{:?}",
            f.pos
        );
        assert_eq!((f.primary, f.mark.as_str()), (0xB0_20_20_FF, "Ar"));
        assert!(f.selected && f.needs_orders);
        assert_eq!(f.health, Some(0.6));
        assert_eq!(f.tooltip, "Your Archer, HP 6/10");
        let b = &v.banners[0];
        assert!(b.capital && b.health.is_none() && !b.strike);
        // Fixture Roma builds unit 2 (Warrior) in 4 turns.
        assert_eq!(b.production.as_deref(), Some("Warrior (4)"));

        // The core says Roma can shoot: the banner gets its "Strike", in the human's turn only.
        let mut shooting = w.clone();
        shooting.cities[0].flags =
            protocol::CityFlags(shooting.cities[0].flags.0 | CityFlags::CAN_RANGE_ATTACK);
        let v = labels_view(&labels, None, &camera(), &shooting, None, None);
        assert!(v.banners[0].strike);
        assert!(!v.flags[0].selected);
        shooting.status = TurnStatus::AiTurn { active: 1 };
        let v = labels_view(&labels, None, &camera(), &shooting, None, None);
        assert!(!v.banners[0].strike, "not in the rivals' turn");
    }

    #[test]
    fn type_marks() {
        assert_eq!(type_mark("UNIT_WARRIOR"), "Wa");
        assert_eq!(type_mark("UNIT_WORK_BOAT"), "WB");
        assert_eq!(type_mark("UNIT_GREAT_GENERAL"), "GG");
        assert_eq!(type_mark(""), "?");
    }
}
