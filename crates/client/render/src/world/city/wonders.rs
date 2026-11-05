//! The wonders and the buildings with a model of their own in a city (world-objects 4.4, WOB5):
//! the castle, the harbor, the colosseum, the Pyramids...
//!
//! `CityView.buildings` lists the buildings with art in the world; the client turns each into its
//! `Buildings.ArtDefineTag`. The record `building/<tag>` of the art table holds the model and its
//! scale (the wonder art `fScale`) or the variants: by the culture of the owner, by the era, or
//! one of a few at random. A building with no model of its own is part of the catalog (RW-18) and
//! adds nothing here.
//!
//! A wonder stands on a corner of the city hex or of a neighbour ([`find_slot`], a port of
//! `cvCityBuildingSim::FindWonderPlacementSlot` 0x087b8988): the candidate corners of its kind are
//! shuffled with `srand(y * 0xff + x)` and the MSVC `random_shuffle`, the first free usable one is
//! taken; its place is the middle of the three hexes that meet there
//! (`CalculateWonderPositionFromSlot` 0x087b9500).
//!
//! The kind of a building (`Buildings.DisplayPosition`) picks the rule ([`place`]): 1 four fixed
//! offsets round the middle of the hex, 2 and 4 corners, 8 a corner on the water (lighthouse), 16 the
//! coast between the city hex and the sea (harbor, [`harbor_spot`]), 32 the middle of the hex
//! (palace), 64 a mountain within two rings.
//!
//! SIMPLIFIED(SIMP-021): a corner of kind 1, 2 or 4 is usable when its three hexes are dry land and
//! no mountain (the corner flags of `InitHexCorners` are not read through).

use std::collections::BTreeMap;

use asset_format::{ArtTable, BuildingArt, LogicalId};
use glam::{Mat4, Vec2, Vec3};

use super::fields::TRANSLATIONS;
use super::layout::{CITY_SCALE, PlotEdges, plot_of};
use super::msvc::MsvcRand;
use super::walls::WALLS_TAG;
use crate::entities::{Culture, EraBucket};
use crate::hexgrid::cell_center;
use crate::world::landmark::select::mix;

/// `ms_aWonderCornerCandidates` (0x0a4cc088): cube offset and corner, T_A then T_B.
const CANDIDATES: [([i32; 2], u8); 12] = [
    ([0, 0], 0),
    ([-1, 1], 1),
    ([0, -1], 0),
    ([0, 0], 1),
    ([1, -1], 0),
    ([0, 1], 1),
    ([-1, 2], 1),
    ([-1, 0], 0),
    ([-1, 0], 1),
    ([1, -2], 0),
    ([1, 0], 1),
    ([1, 0], 0),
];

/// `kCornerNeighbours`: the two neighbours that span each corner with the hex.
const CORNER_NEIGHBOURS: [[usize; 2]; 2] = [[0, 5], [3, 2]];

/// A wonder slot: a hex (cube x, y) and its corner 0 or 1.
pub type Slot = ((i32, i32), u8);

/// The three hexes of a corner.
pub fn corner_hexes(slot: Slot) -> [(i32, i32); 3] {
    let ((x, y), c) = slot;
    let [a, b] = CORNER_NEIGHBOURS[usize::from(c)];
    let t = |k: usize| (x + TRANSLATIONS[k][0], y + TRANSLATIONS[k][1]);
    [(x, y), t(a), t(b)]
}

/// The place of a slot in our world: the middle of the centres of its three hexes.
pub fn slot_position(slot: Slot) -> Vec2 {
    corner_hexes(slot)
        .iter()
        .map(|&(hx, hy)| {
            let (c, r) = plot_of(hx, hy);
            cell_center(c, r)
        })
        .sum::<Vec2>()
        / 3.0
}

/// `FindWonderPlacementSlot(kind, hex, allowShared)`. `used` holds the taken corners of the hexes
/// (bit per corner); `usable` says whether a corner may hold a wonder.
pub fn find_slot(
    kind: u8,
    hex: (i32, i32),
    used: &mut BTreeMap<(i32, i32), u8>,
    usable: &dyn Fn(Slot) -> bool,
    allow_shared: bool,
) -> Option<Slot> {
    let mut list: Vec<([i32; 2], u8)> = match kind {
        2 => CANDIDATES[..6].to_vec(),
        4 => CANDIDATES[6..].to_vec(),
        8 | 16 => CANDIDATES.to_vec(),
        _ => return None,
    };
    let seed = (hex.1.wrapping_mul(0xff)).wrapping_add(hex.0) as u32;
    MsvcRand::new(seed).shuffle(&mut list);
    for pass in 0..3 {
        if pass == 0 && kind != 2 && kind != 4 {
            continue;
        }
        if pass == 2 && !allow_shared {
            break;
        }
        for (d, corner) in &list {
            let h = (hex.0 + d[0], hex.1 + d[1]);
            let bits = used.get(&h).copied().unwrap_or(0);
            if pass < 2 && bits >> corner & 1 != 0 {
                continue;
            }
            if !usable((h, *corner)) {
                continue;
            }
            used.insert(h, bits | 1 << corner);
            return Some((h, *corner));
        }
    }
    None
}

/// The placement kind of a building (`Buildings.DisplayPosition`, bit value) by its `Type`: the
/// table of RB13 item 4b. A building with a model that is not in it takes kind 2.
// SIMPLIFIED(SIMP-exe-1170): only for an art table built before the `building_type/` records (the
// `DisplayPosition` column, re-b17 5.1): the kinds are the list of the answer (the names of the base
// game and Expansion 2), and an unlisted building with a model of its own takes the corner kind 2.
pub fn kind_of(art: &BuildingArt) -> u8 {
    const TABLE: [(u8, &[&str]); 7] = [
        (
            1,
            &[
                "CASTLE",
                "GREAT_LIBRARY",
                "NOTRE_DAME",
                "SISTINE_CHAPEL",
                "KREMLIN",
                "FORBIDDEN_PALACE",
                "BIG_BEN",
                "EIFFEL_TOWER",
                "UNITED_NATIONS",
                "UFFIZI",
                "BROADWAY",
            ],
        ),
        (2, &[]),
        (
            4,
            &[
                "STONEHENGE",
                "PYRAMID",
                "ORACLE",
                "CHICHEN_ITZA",
                "CRISTO_REDENTOR",
                "RED_FORT",
                "PRORA_RESORT",
                "BOROBUDUR",
            ],
        ),
        (8, &["LIGHTHOUSE", "GREAT_LIGHTHOUSE", "COLOSSUS"]),
        (16, &["HARBOR", "SYDNEY_OPERA_HOUSE"]),
        (32, &["PALACE"]),
        (64, &["MACHU_PICHU", "MACHU_PICCHU"]),
    ];
    art.building_types
        .iter()
        .find_map(|t| {
            let name = t.strip_prefix("BUILDING_").unwrap_or(t);
            TABLE
                .iter()
                .find(|(_, names)| names.contains(&name))
                .map(|(k, _)| *k)
        })
        .unwrap_or(2)
}

/// A model that stands in the city.
#[derive(Debug, Clone, PartialEq)]
pub struct Special {
    pub tag: String,
    pub model: LogicalId,
    pub scale: f32,
    /// The placement kind ([`kind_of`]).
    pub kind: u8,
}

/// Where a special stands: the place in our world from the middle of the city hex, and its yaw
/// (the angle of its heading in the original's plane, which is the rotation about our up axis).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spot {
    pub at: Vec2,
    pub yaw: f32,
}

/// What the placement reads of the map.
pub struct Ground<'a> {
    /// The city hex (cube x, y) and the middle of it in our world.
    pub hex: (i32, i32),
    pub centre: Vec2,
    /// Water and mountain of a hex; `None` outside the map.
    pub plot: &'a dyn Fn((i32, i32)) -> Option<PlotEdges>,
    /// Land at a point of our world: the raw height above the sea (12999).
    pub land: &'a dyn Fn(Vec2) -> bool,
}

impl Ground<'_> {
    fn water(&self, h: (i32, i32)) -> bool {
        (self.plot)(h).is_some_and(|p| p.water)
    }

    fn mountain(&self, h: (i32, i32)) -> bool {
        (self.plot)(h).is_some_and(|p| p.mountain)
    }

    fn hex_centre(&self, h: (i32, i32)) -> Vec2 {
        let (c, r) = plot_of(h.0, h.1);
        cell_center(c, r)
    }
}

fn neighbour(h: (i32, i32), k: usize) -> (i32, i32) {
    (h.0 + TRANSLATIONS[k][0], h.1 + TRANSLATIONS[k][1])
}

fn hex_distance(a: (i32, i32), b: (i32, i32)) -> i32 {
    let (dx, dy) = (a.0 - b.0, a.1 - b.1);
    dx.abs().max(dy.abs()).max((dx + dy).abs())
}

/// `ms_aInCitySlotOffsets` (Startup 0x087b32a2): the four fixed offsets of kind 1, in the original's
/// plane.
const IN_CITY_SLOTS: [[f32; 2]; 4] = [[16.0, -16.0], [-16.0, -16.0], [-16.0, 16.0], [16.0, 16.0]];

/// The coast point of a harbor (`OnWonderCreated` 0x087b4368, kind 16) and its heading: the edge
/// between the city hex and a water neighbour T0..T2, or else any edge near the city with water on
/// one side only; ten bisections of the line from the land centre to the water centre on the
/// height; the yaw is the direction land to water. `used` holds the edges taken by harbors (two at
/// most).
///
/// The "edge taken" test of pass 1 reads the bit of the water neighbour while the bit is set on
/// the city hex (0x087b45a2 against 0x087b4c82), so it never sees the edge of the same city: a
/// harbor and the Sydney Opera House of one city take the same edge, as in the original.
pub fn harbor_spot(g: &Ground<'_>, used: &mut Vec<((i32, i32), usize)>) -> Option<Spot> {
    if used.len() >= 2 {
        return None;
    }
    let h = g.hex;
    let mut edge = (0..3)
        .find(|&k| {
            let n = neighbour(h, k);
            g.water(n) && !used.contains(&(n, k))
        })
        .map(|k| (h, k));
    if edge.is_none() {
        'outer: for d in 0..6 {
            let n = neighbour(h, d);
            if (g.plot)(n).is_none() {
                continue;
            }
            for e in 0..3 {
                let m = neighbour(n, e);
                if hex_distance(m, h) <= 1 && (g.plot)(m).is_some() && g.water(n) != g.water(m) {
                    edge = Some((n, e));
                    break 'outer;
                }
            }
        }
    }
    let (e_hex, e) = edge?;
    used.push((e_hex, e));
    let (mut a, mut b) = (g.hex_centre(e_hex), g.hex_centre(neighbour(e_hex, e)));
    if g.water(e_hex) {
        std::mem::swap(&mut a, &mut b);
    }
    let d = b - a;
    let mut at = a;
    for _ in 0..10 {
        at = (a + b) * 0.5;
        if (g.land)(at) {
            a = at;
        } else {
            b = at;
        }
    }
    // Our Z is the original's -y.
    Some(Spot {
        at: at - g.centre,
        yaw: (-d.y).atan2(d.x),
    })
}

/// The places of the specials of a city, in their order (`OnWonderCreated` by kind). `usable` says
/// whether a corner may hold a wonder of kind 1, 2 or 4.
// SIMPLIFIED(SIMP-exe-1171): a lighthouse takes a corner that touches water (any of its three hexes,
// as the corner flag of `InitHexCorners` is set by any edge with water, re-b17 1.4; the flags are not
// read through); the heading of corner models and of the kind-1 slots (street tensor and noise) is
// not read, the yaw is 0.
// SIMPLIFIED(SIMP-exe-1172): the forward axis of the harbor model is not known; its +x is turned to
// the water, the way the palace's (rotation 0, heading (1, 0)) reads in the same code.
pub fn place(
    specials: &[Special],
    g: &Ground<'_>,
    usable: &dyn Fn(Slot) -> bool,
) -> Vec<Option<Spot>> {
    let touches_water = |slot: Slot| corner_hexes(slot).iter().any(|&h| g.water(h));
    let mut corners = BTreeMap::new();
    let mut coast = Vec::new();
    let mut slots = 0;
    let mut machu = false;
    specials
        .iter()
        .map(|s| match s.kind {
            1 => {
                let o = *IN_CITY_SLOTS.get(slots)?;
                slots += 1;
                Some(Spot {
                    at: Vec2::new(o[0], -o[1]),
                    yaw: 0.0,
                })
            }
            2 | 4 => find_slot(s.kind, g.hex, &mut corners, usable, true).map(|c| Spot {
                at: slot_position(c) - g.centre,
                yaw: 0.0,
            }),
            8 => find_slot(8, g.hex, &mut corners, &touches_water, false).map(|c| Spot {
                at: slot_position(c) - g.centre,
                yaw: 0.0,
            }),
            16 => harbor_spot(g, &mut coast),
            32 => Some(Spot {
                at: Vec2::ZERO,
                yaw: 0.0,
            }),
            64 if !machu => {
                let found = (0..6)
                    .flat_map(|a| (0..6).map(move |b| (a, b)))
                    .map(|(a, b)| neighbour(neighbour(g.hex, a), b))
                    .find(|&h| g.mountain(h));
                machu = found.is_some();
                found.map(|h| Spot {
                    at: g.hex_centre(h) - g.centre,
                    yaw: 0.0,
                })
            }
            _ => None,
        })
        .collect()
}

fn rank(era: &str) -> Option<u8> {
    ["ANCIENT", "MEDIEVAL", "INDUSTRIAL", "MODERN"]
        .iter()
        .position(|e| *e == era)
        .map(|i| i as u8)
}

fn era_rank(era: EraBucket) -> u8 {
    rank(era.building_key()).unwrap_or(0)
}

/// The model of a building for this owner, or `None` when it has none of its own.
pub fn choose(
    art: &BuildingArt,
    culture: Culture,
    era: EraBucket,
    seed: u32,
) -> Option<(&LogicalId, f32)> {
    if let Some(m) = &art.model {
        return Some((m, art.scale));
    }
    let kind = art.variants.first()?.kind.as_str();
    let of_kind = || art.variants.iter().filter(move |v| v.kind == kind);
    let pick = match kind {
        "culture" => of_kind()
            .find(|v| v.key == culture.prefix())
            .or_else(|| of_kind().find(|v| v.key == Culture::Euro.prefix()))
            .or_else(|| of_kind().next()),
        "era" => {
            // The latest variant that is not later than the era; the earliest when all are.
            let now = era_rank(era);
            of_kind()
                .filter(|v| rank(&v.key).is_some_and(|r| r <= now))
                .max_by_key(|v| rank(&v.key))
                .or_else(|| of_kind().min_by_key(|v| rank(&v.key)))
        }
        _ => {
            let all: Vec<_> = of_kind().collect();
            let tag_seed = art.art_tag.bytes().fold(seed, |h, b| mix(h, u32::from(b)));
            all.get(tag_seed as usize % all.len().max(1)).copied()
        }
    }?;
    Some((&pick.model, pick.scale))
}

/// The models of the buildings of a city. Walls are not among them (see [`super::walls`]).
///
/// With the `building_type/` records of the art table, one per building `Type` of `types` (in the
/// order of the building ids, the order `setupWonderGraphics` sends them, re-b17 5.7) whose
/// `DisplayPosition` is not 0, of that kind (`GameplayWonderCreated` per type, re-b17 5.1): an
/// Amphitheater shares the Colosseum's tag but has no model in the city, and a Harbor and a
/// Feitoria of one city are two harbors. A table built before those records falls back to the
/// tags and [`kind_of`].
pub fn specials(
    table: &ArtTable,
    tags: &[String],
    types: &[String],
    culture: Culture,
    era: EraBucket,
    seed: u32,
) -> Vec<Special> {
    let scaled = |s: f32| if s.is_finite() && s > 0.0 { s } else { 1.0 };
    if table.has_building_types() && !types.is_empty() {
        return types
            .iter()
            .filter_map(|t| {
                let row = table.building_type(t)?;
                if row.display_position == 0 || table.wall(&row.art_tag, "Any").is_some() {
                    return None;
                }
                let art = table.building(&row.art_tag)?;
                let (model, scale) = choose(art, culture, era, seed)?;
                Some(Special {
                    tag: row.art_tag.clone(),
                    kind: row.display_position,
                    model: model.clone(),
                    scale: scaled(scale),
                })
            })
            .collect();
    }
    tags.iter()
        .filter(|t| t.as_str() != WALLS_TAG && table.wall(t, "Any").is_none())
        .filter_map(|tag| {
            let art = table.building(tag)?;
            let (model, scale) = choose(art, culture, era, seed)?;
            Some(Special {
                tag: tag.clone(),
                kind: kind_of(art),
                model: model.clone(),
                scale: scaled(scale),
            })
        })
        .collect()
}

/// The matrix of a special on the place `at` (catalog units from the middle of the hex), relative
/// to the middle of the hex: the origin of the model on the place (the record position of
/// `UpdateFromHandle`, not the middle of its box: re-b17 1.3), turned by `yaw` about the place.
pub fn matrix(at: Vec2, scale: f32, yaw: f32) -> Mat4 {
    let w = at * CITY_SCALE;
    Mat4::from_translation(Vec3::new(w.x, 0.0, w.y))
        * Mat4::from_rotation_y(yaw)
        * Mat4::from_scale(Vec3::splat(scale))
}

#[cfg(test)]
mod tests {
    use asset_format::BuildingVariant;

    use super::*;

    fn variant(kind: &str, key: &str, model: &str) -> BuildingVariant {
        BuildingVariant {
            kind: kind.into(),
            key: key.into(),
            art_type: format!("{key} X"),
            model: LogicalId::new(model),
            scale: 0.2,
        }
    }

    fn art(tag: &str, model: Option<&str>, variants: Vec<BuildingVariant>) -> BuildingArt {
        BuildingArt {
            art_tag: tag.into(),
            building_types: vec![],
            cultural_variation: false,
            era_variation: false,
            random_variation: false,
            model: model.map(LogicalId::new),
            scale: 0.05,
            variants,
            note: None,
        }
    }

    #[test]
    fn a_wonder_has_one_model() {
        let a = art("THE PYRAMIDS", Some("md:pyramids"), vec![]);
        let (m, s) = choose(&a, Culture::Asia, EraBucket::Modern, 1).unwrap();
        assert_eq!((m.as_str(), s), ("md:pyramids", 0.05));
    }

    #[test]
    fn a_building_follows_the_culture_of_the_owner() {
        let a = art(
            "CASTLE",
            None,
            vec![
                variant("culture", "AFRICAN", "md:afri"),
                variant("culture", "EUROPEAN", "md:euro"),
                variant("culture", "ASIAN", "md:asia"),
            ],
        );
        let pick = |c| {
            choose(&a, c, EraBucket::Ancient, 1)
                .unwrap()
                .0
                .as_str()
                .to_owned()
        };
        assert_eq!(pick(Culture::Asia), "md:asia");
        assert_eq!(pick(Culture::Afri), "md:afri");
        // A culture it has no variant for: Europe.
        assert_eq!(pick(Culture::Amer), "md:euro");
    }

    #[test]
    fn a_building_follows_the_era_of_the_owner() {
        let a = art(
            "HARBOR",
            None,
            vec![
                variant("era", "ANCIENT", "md:anc"),
                variant("era", "MEDIEVAL", "md:mid"),
                variant("era", "INDUSTRIAL", "md:ind"),
                variant("era", "MODERN", "md:mod"),
            ],
        );
        let pick = |e| {
            choose(&a, Culture::Euro, e, 1)
                .unwrap()
                .0
                .as_str()
                .to_owned()
        };
        assert_eq!(pick(EraBucket::Ancient), "md:anc");
        assert_eq!(pick(EraBucket::Renaissance), "md:mid");
        assert_eq!(pick(EraBucket::Industrial), "md:ind");
        assert_eq!(pick(EraBucket::Modern), "md:mod");
        // A lighthouse has two eras: the latest that is not later than the owner's.
        let l = art(
            "LIGHTHOUSE",
            None,
            vec![
                variant("era", "ANCIENT", "md:a"),
                variant("era", "INDUSTRIAL", "md:i"),
            ],
        );
        assert_eq!(
            choose(&l, Culture::Euro, EraBucket::Renaissance, 1)
                .unwrap()
                .0
                .as_str(),
            "md:a"
        );
        assert_eq!(
            choose(&l, Culture::Euro, EraBucket::Modern, 1)
                .unwrap()
                .0
                .as_str(),
            "md:i"
        );
    }

    #[test]
    fn a_random_building_is_the_same_for_a_city() {
        let a = art(
            "STADIUM",
            None,
            vec![
                variant("random", "ONE", "md:one"),
                variant("random", "TWO", "md:two"),
            ],
        );
        let pick = |seed| {
            choose(&a, Culture::Euro, EraBucket::Ancient, seed)
                .unwrap()
                .0
                .clone()
        };
        assert_eq!(pick(7), pick(7));
        let seen: std::collections::BTreeSet<_> = (0..40).map(pick).collect();
        assert_eq!(seen.len(), 2, "both occur");
    }

    #[test]
    fn walls_and_buildings_without_a_model_are_not_specials() {
        let mut t = ArtTable::new();
        t.insert_building(art("THE PYRAMIDS", Some("md:pyramids"), vec![]));
        let mut plain = art("ART_DEF_BUILDING_BANK", None, vec![]);
        plain.note = Some("part of the catalog".into());
        t.insert_building(plain);
        t.insert_building(art(WALLS_TAG, None, vec![]));
        let tags: Vec<String> = [
            "ART_DEF_BUILDING_BANK",
            "THE PYRAMIDS",
            WALLS_TAG,
            "MISSING",
        ]
        .map(String::from)
        .into();
        let s = specials(&t, &tags, &[], Culture::Euro, EraBucket::Ancient, 1);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].model.as_str(), "md:pyramids");
    }

    #[test]
    fn each_building_type_draws_by_its_own_display_position_not_by_its_tag() {
        use asset_format::BuildingTypeArt;
        let mut t = ArtTable::new();
        t.insert_building(art(
            "COLESSEUM",
            None,
            vec![variant("culture", "EUROPEAN", "md:colosseum")],
        ));
        t.insert_building(art("TERRACOTTA ARMY", Some("md:terracotta"), vec![]));
        t.insert_building(art(
            "HARBOR",
            None,
            vec![variant("era", "ANCIENT", "md:harbor_anc")],
        ));
        // A table without the records: the tags, as before (a Colosseum for the Amphitheater).
        let tags = vec!["COLESSEUM".to_owned()];
        let amph = vec!["BUILDING_AMPHITHEATER".to_owned()];
        let old = specials(&t, &tags, &amph, Culture::Euro, EraBucket::Ancient, 1);
        assert_eq!(old.len(), 1);
        for (ty, tag, d) in [
            ("BUILDING_COLOSSEUM", "COLESSEUM", 2),
            ("BUILDING_AMPHITHEATER", "COLESSEUM", 0),
            ("BUILDING_POLICE_STATION", "COLESSEUM", 0),
            ("BUILDING_TERRACOTTA_ARMY", "TERRACOTTA ARMY", 4),
            ("BUILDING_GREAT_FIREWALL", "TERRACOTTA ARMY", 0),
            ("BUILDING_HARBOR", "HARBOR", 16),
            ("BUILDING_FEITORIA", "HARBOR", 16),
        ] {
            t.insert_building_type(BuildingTypeArt {
                building_type: ty.into(),
                art_tag: tag.into(),
                display_position: d,
            });
        }
        let s = |types: &[&str]| {
            let types: Vec<String> = types.iter().map(|t| t.to_string()).collect();
            specials(&t, &tags, &types, Culture::Euro, EraBucket::Ancient, 1)
                .into_iter()
                .map(|s| (s.model.as_str().to_owned(), s.kind))
                .collect::<Vec<_>>()
        };
        // An Amphitheater, a Police Station, the Great Firewall: no model in the city.
        assert!(s(&["BUILDING_AMPHITHEATER", "BUILDING_POLICE_STATION"]).is_empty());
        assert!(s(&["BUILDING_GREAT_FIREWALL"]).is_empty());
        assert_eq!(
            s(&["BUILDING_AMPHITHEATER", "BUILDING_COLOSSEUM"]),
            [("md:colosseum".to_owned(), 2)]
        );
        assert_eq!(
            s(&["BUILDING_TERRACOTTA_ARMY"]),
            [("md:terracotta".to_owned(), 4)]
        );
        // Two buildings of one tag are two models, in the order given.
        assert_eq!(
            s(&["BUILDING_HARBOR", "BUILDING_FEITORIA"]),
            [
                ("md:harbor_anc".to_owned(), 16),
                ("md:harbor_anc".to_owned(), 16)
            ]
        );
    }

    #[test]
    fn the_wonder_slots_follow_the_hex_and_do_not_repeat() {
        let all = |_: Slot| true;
        let mut used = BTreeMap::new();
        let a = find_slot(2, (3, 4), &mut used, &all, false).unwrap();
        let b = find_slot(2, (3, 4), &mut used, &all, false).unwrap();
        assert_ne!(a, b);
        // The same hex shuffles the same way.
        assert_eq!(
            find_slot(2, (3, 4), &mut BTreeMap::new(), &all, false),
            Some(a)
        );
        // Six corners of kind 2; the seventh only when shared.
        for _ in 0..4 {
            assert!(find_slot(2, (3, 4), &mut used, &all, false).is_some());
        }
        assert_eq!(find_slot(2, (3, 4), &mut used, &all, false), None);
        assert!(find_slot(2, (3, 4), &mut used, &all, true).is_some());
        assert_eq!(find_slot(3, (3, 4), &mut used, &all, true), None);
        // A corner is the middle of its three hexes: a hex corner, 36.95 from each centre.
        let p = slot_position(((0, 4), 0));
        let c = cell_center(2, 4);
        assert!((p.distance(c) - crate::hexgrid::CORNER_RADIUS).abs() < 0.01);
    }

    #[test]
    fn the_origin_of_the_model_stands_on_the_place_whatever_its_box() {
        let m = matrix(Vec2::new(100.0, 0.0), 0.2, 0.0);
        let at = m.transform_point3(Vec3::new(0.0, 3.0, 0.0));
        assert!((at.x - 100.0 * CITY_SCALE).abs() < 1e-4 && at.z.abs() < 1e-4);
        assert!((at.y - 0.6).abs() < 1e-4, "height is scaled");
        // Turned, the origin stays on the place and the rest turns about it (the Sydney Opera
        // House, whose box is 96 units off its origin, is not pulled inland).
        let m = matrix(Vec2::new(100.0, 0.0), 0.08, 1.0);
        let at = m.transform_point3(Vec3::ZERO);
        assert!((at.x - 100.0 * CITY_SCALE).abs() < 1e-4 && at.z.abs() < 1e-4);
        let off = m.transform_point3(Vec3::new(96.2, 0.0, 0.0)) - at;
        assert!((off.length() - 96.2 * 0.08).abs() < 1e-3);
    }

    #[test]
    fn a_harbor_and_the_opera_house_of_one_city_share_the_first_water_edge() {
        coast(0, |g| {
            let mut used = Vec::new();
            let a = harbor_spot(g, &mut used).unwrap();
            let b = harbor_spot(g, &mut used).unwrap();
            assert_eq!(a, b);
            assert!(harbor_spot(g, &mut used).is_none(), "two at most");
        });
    }

    #[test]
    fn a_lighthouse_may_take_a_corner_with_water_on_all_three_hexes() {
        // A city on an islet: every neighbour is sea.
        let hex = crate::world::city::layout::hex_of(4, 4);
        let plot = move |h: (i32, i32)| {
            Some(PlotEdges {
                water: h != hex,
                ..PlotEdges::default()
            })
        };
        let land = |_: Vec2| true;
        let g = Ground {
            hex,
            centre: cell_center(4, 4),
            plot: &plot,
            land: &land,
        };
        let spots = place(&[8; 12].map(special), &g, &|_| false);
        // Every candidate corner of ring 0 and 1 touches water, also those away from the city hex.
        assert!(spots.iter().all(Option::is_some), "{spots:?}");
        let off_shore = spots
            .iter()
            .flatten()
            .filter(|s| s.at.length() > crate::hexgrid::CORNER_RADIUS + 1.0)
            .count();
        assert!(off_shore > 0, "{spots:?}");
    }

    fn bld(types: &[&str]) -> BuildingArt {
        let mut a = art("X", Some("md:x"), vec![]);
        a.building_types = types.iter().map(|t| t.to_string()).collect();
        a
    }

    fn special(kind: u8) -> Special {
        Special {
            tag: "X".into(),
            model: LogicalId::new("md:x"),
            scale: 0.1,
            kind,
        }
    }

    #[test]
    fn the_kind_of_a_building_follows_its_display_position() {
        let kind = |t: &str| kind_of(&bld(&[t]));
        assert_eq!(kind("BUILDING_HARBOR"), 16);
        assert_eq!(kind("BUILDING_SYDNEY_OPERA_HOUSE"), 16);
        assert_eq!(kind("BUILDING_LIGHTHOUSE"), 8);
        assert_eq!(kind("BUILDING_PALACE"), 32);
        assert_eq!(kind("BUILDING_CASTLE"), 1);
        assert_eq!(kind("BUILDING_PYRAMID"), 4);
        assert_eq!(kind("BUILDING_MACHU_PICHU"), 64);
        assert_eq!(kind("BUILDING_COLOSSEUM"), 2);
        // Not listed: a corner, as before.
        assert_eq!(kind("BUILDING_SOMETHING_NEW"), 2);
    }

    /// A city on (4, 4) with one water hex, its neighbour `k`, the sea being all that is nearer
    /// to the middle of that hex than to the middle of the city.
    fn coast(k: usize, run: impl FnOnce(&Ground<'_>)) {
        let hex = crate::world::city::layout::hex_of(4, 4);
        let sea = neighbour(hex, k);
        let plot = move |h: (i32, i32)| {
            Some(PlotEdges {
                water: h == sea,
                ..PlotEdges::default()
            })
        };
        let centre = cell_center(4, 4);
        let (c, r) = plot_of(sea.0, sea.1);
        let sea_centre = cell_center(c, r);
        let land = move |p: Vec2| p.distance(centre) < p.distance(sea_centre);
        run(&Ground {
            hex,
            centre,
            plot: &plot,
            land: &land,
        });
    }

    #[test]
    fn a_harbor_stands_on_the_coast_facing_the_water() {
        for k in 0..3 {
            coast(k, |g| {
                let spot = harbor_spot(g, &mut Vec::new()).unwrap();
                let (c, r) = plot_of(neighbour(g.hex, k).0, neighbour(g.hex, k).1);
                let to_sea = cell_center(c, r) - g.centre;
                // Half way to the sea hex, not at the middle of the city.
                assert!(
                    (spot.at.length() - to_sea.length() * 0.5).abs() < 0.2,
                    "{k}: {:?}",
                    spot.at
                );
                // The heading (our plane: cos, -sin) points at the water, the land is behind.
                let heading = Vec2::new(spot.yaw.cos(), -spot.yaw.sin());
                assert!(heading.dot(to_sea.normalize()) > 0.999, "{k}");
                assert!(!(g.land)(g.centre + spot.at + heading * 6.0));
                assert!((g.land)(g.centre + spot.at - heading * 6.0));
            });
        }
    }

    #[test]
    fn a_harbor_finds_water_that_is_not_on_its_own_edges_and_two_are_the_most() {
        coast(4, |g| {
            let mut used = Vec::new();
            let spot = harbor_spot(g, &mut used).expect("a coast of a neighbour");
            assert!(spot.at.length() < crate::hexgrid::HEX_WIDTH * 1.5);
            assert!(harbor_spot(g, &mut used).is_some());
            assert!(harbor_spot(g, &mut used).is_none(), "two at most");
        });
        // An inland city has no coast and no harbor.
        let hex = crate::world::city::layout::hex_of(4, 4);
        let plot = |_: (i32, i32)| Some(PlotEdges::default());
        let land = |_: Vec2| true;
        let g = Ground {
            hex,
            centre: cell_center(4, 4),
            plot: &plot,
            land: &land,
        };
        assert_eq!(harbor_spot(&g, &mut Vec::new()), None);
    }

    /// The art table `tb:art-b2` of a built pack: the folder of `CIV5_B2_ASSETS`, else the b13 pack
    /// of the workspace (`assets-out/b13` in a folder above the crate); `None` (the test says it
    /// skipped) when neither is there.
    pub(crate) fn real_art_table() -> Option<ArtTable> {
        use asset_format::Manifest;
        let dir = std::env::var_os("CIV5_B2_ASSETS")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .ancestors()
                    .map(|a| a.join("assets-out").join("b13"))
                    .find(|d| d.is_dir())
            })
            .unwrap_or_default();
        let Ok(line) = std::fs::read_to_string(dir.join("manifest")) else {
            eprintln!("no pack at {}; skipped", dir.display());
            return None;
        };
        let hash = line.lines().next()?.trim().to_owned();
        let manifest = Manifest::decode(&std::fs::read(dir.join(hash)).ok()?).expect("manifest");
        let v = manifest.variants(&LogicalId::new("tb:art-b2")).first()?;
        let bytes = std::fs::read(dir.join(v.hash.to_string())).expect("art table blob");
        Some(ArtTable::decode(&bytes).expect("art table"))
    }

    /// Hai Phong (user report, re-b17 item 1): a coastal city whose NE and E neighbours are sea.
    /// The client turns `BUILDING_HARBOR` into the tag `HARBOR`; with the record of the real art
    /// table the harbor is of kind 16 and stands on the NE shore, facing the water.
    #[test]
    fn the_harbor_of_the_real_art_table_stands_on_the_shore_facing_the_sea() {
        let Some(table) = real_art_table() else {
            return;
        };
        let rec = table.building("HARBOR").expect("building/HARBOR");
        assert!(
            rec.building_types.iter().any(|t| t == "BUILDING_HARBOR"),
            "{:?}",
            rec.building_types
        );
        let tags = vec!["HARBOR".to_owned()];
        // The types are read when the table has their records (a pack built after re-b17 5.1).
        let types = vec!["BUILDING_HARBOR".to_owned()];
        let list = specials(
            &table,
            &tags,
            &types,
            Culture::Asia,
            EraBucket::Renaissance,
            7,
        );
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].kind, 16, "{list:?}");
        assert!(list[0].model.as_str().starts_with("md:harbor_"), "{list:?}");

        let hex = crate::world::city::layout::hex_of(4, 4);
        // T0 is NE and T4 is E.
        let sea = [neighbour(hex, 0), neighbour(hex, 4)];
        let plot = move |h: (i32, i32)| {
            Some(PlotEdges {
                water: sea.contains(&h),
                ..PlotEdges::default()
            })
        };
        let centre_of = |h: (i32, i32)| {
            let (c, r) = plot_of(h.0, h.1);
            cell_center(c, r)
        };
        // A point is sea when the nearest hex centre is a sea hex.
        let land = move |p: Vec2| {
            let mut best = (f32::MAX, false);
            for d in 0..6 {
                for h in [neighbour(hex, d), hex] {
                    let dist = p.distance(centre_of(h));
                    if dist < best.0 {
                        best = (dist, !sea.contains(&h));
                    }
                }
            }
            best.1
        };
        let g = Ground {
            hex,
            centre: cell_center(4, 4),
            plot: &plot,
            land: &land,
        };
        let spots = place(&list, &g, &|_| true);
        let spot = spots[0].expect("a harbor spot");
        let to_ne = centre_of(sea[0]) - g.centre;
        assert!(
            (spot.at.length() - to_ne.length() * 0.5).abs() < 0.3,
            "on the NE shore: {:?}",
            spot.at
        );
        assert!(spot.at.normalize().dot(to_ne.normalize()) > 0.999);
        let heading = Vec2::new(spot.yaw.cos(), -spot.yaw.sin());
        assert!(heading.dot(to_ne.normalize()) > 0.999, "faces the sea");
        assert!(!land(g.centre + spot.at + heading * 6.0));
        assert!(land(g.centre + spot.at - heading * 6.0));
    }

    #[test]
    fn each_kind_has_its_place_and_none_is_on_a_water_hex() {
        coast(1, |g| {
            let list: Vec<Special> = [16, 8, 32, 1, 1, 1, 1, 1, 2, 4].map(special).into();
            let usable = |slot: Slot| {
                corner_hexes(slot)
                    .iter()
                    .all(|&h| !(g.plot)(h).is_some_and(|p| p.water))
            };
            let spots = place(&list, g, &usable);
            // The palace is the middle of the city.
            assert_eq!(spots[2].unwrap().at, Vec2::ZERO);
            // The castle takes the four fixed offsets, the fifth has no slot.
            let offsets: Vec<Vec2> = spots[3..7].iter().map(|s| s.unwrap().at).collect();
            let mut expect = vec![
                Vec2::new(16.0, 16.0),
                Vec2::new(-16.0, 16.0),
                Vec2::new(-16.0, -16.0),
                Vec2::new(16.0, -16.0),
            ];
            expect.sort_by(|a, b| a.to_array().partial_cmp(&b.to_array()).unwrap());
            let mut got = offsets.clone();
            got.sort_by(|a, b| a.to_array().partial_cmp(&b.to_array()).unwrap());
            assert_eq!(got, expect);
            assert_eq!(spots[7], None);
            // The lighthouse is on a corner that touches the sea and land.
            let lh = spots[1].unwrap().at + g.centre;
            let sea_centre = {
                let s = neighbour(g.hex, 1);
                let (c, r) = plot_of(s.0, s.1);
                cell_center(c, r)
            };
            assert!(lh.distance(sea_centre) < crate::hexgrid::CORNER_RADIUS + 0.1);
            // No centre but the harbor's is on the water hex (the coast is its edge).
            for (i, s) in spots.iter().enumerate() {
                let Some(s) = s else { continue };
                if i == 0 {
                    continue;
                }
                assert!(
                    (g.land)(g.centre + s.at) || i == 1,
                    "{i}: {:?} is nearer to the sea",
                    s.at
                );
            }
            assert!(spots[8].is_some() && spots[9].is_some());
        });
    }
}
