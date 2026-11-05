//! The wall of a city (world-objects 4.4, WOB5).
//!
//! The wall model of a culture has three parts: a gatehouse (the widest and tallest), a short
//! stretch of wall, and a tower. A city with a wall (`ART_DEF_BUILDING_WALLS`, or the Walls of
//! Babylon, which have a wall model of their own) is ringed by them along the round wall of
//! [`super::wall_gen`]: a tower at each tower of the plan, and along the spline of each
//! `AddWallSegment` chunk the stretch as `AddSplineModel` (0x08a165a6, re-b17 2W) lays it: as many
//! whole copies as fit, `n = max(1, floor(L / length))`, stretched to fill the spline exactly, each
//! at its share of the arc length and turned along the spline there. No gatehouse: the original
//! sends no `AddWallGate` either. The pieces are drawn at the wall scale 0.05 of `SetWallScale`.
//!
//! SIMPLIFIED(SIMP-exe-1252): each copy is a rigid piece along the chord of its share of the spline,
//! lifted to the ground at its middle; the original bends every vertex along the spline and puts
//! it on the terrain under it (the pieces are instances of one mesh here, not a mesh of its own).

use asset_format::{ArtTable, WallArt};
use glam::{Mat4, Vec2, Vec3};

use super::compose::ModelMeta;
use super::layout::from_orig;
use super::wall_gen::{self, WallPlan};
use crate::entities::{Culture, EraBucket};

pub use super::wall_gen::WALL_SCALE;

/// The art tag of the wall every culture has.
pub const WALLS_TAG: &str = "ART_DEF_BUILDING_WALLS";

/// The wall record of a city that has a wall: the Walls of Babylon, else the wall of the culture
/// and the era (an older era of the same culture when the era has none, then Europe).
pub fn wall_of<'a>(
    table: &'a ArtTable,
    tags: &[String],
    culture: Culture,
    era: EraBucket,
) -> Option<&'a WallArt> {
    if let Some(w) = tags.iter().find_map(|t| table.wall(t, "Any")) {
        return Some(w);
    }
    if !tags.iter().any(|t| t == WALLS_TAG) {
        return None;
    }
    let eras = [
        EraBucket::Modern,
        EraBucket::Industrial,
        EraBucket::Renaissance,
        EraBucket::Ancient,
    ];
    let from = eras.iter().position(|e| *e == era).unwrap_or(0);
    [culture, Culture::Euro]
        .into_iter()
        .flat_map(|c| eras[from..].iter().map(move |e| (c, *e)))
        .find_map(|(c, e)| table.wall(c.prefix(), e.key()))
}

/// One piece of the ring.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WallPiece {
    /// The mesh of the piece among those of the (split) model.
    pub mesh: u16,
    /// Model space to world space, relative to the middle of the hex (the caller adds the
    /// middle and the ground).
    pub matrix: Mat4,
}

/// The pieces of the wall of `plan` (the original's plane) around a hex whose middle is `centre`:
/// a tower at each tower of the plan, the stretches end to end along the spline of each segment.
/// The matrices are relative to the middle of the hex. The wall model needs its stretch and tower
/// parts in `model` (the gatehouse is the first); fewer give none.
pub fn ring(model: &ModelMeta, plan: &WallPlan, centre: Vec2) -> Vec<WallPiece> {
    let part = |i: usize| model.lots.get(i).and_then(|l| l.building.map(|m| (m, l)));
    let (Some(_gate), Some(stretch), Some(tower)) = (part(0), part(1), part(2)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let put = |out: &mut Vec<WallPiece>,
               (mesh, lot): (u16, &super::compose::Lot),
               at: Vec2,
               along: Vec2,
               fill: f32| {
        // The longer side of the piece runs along the wall; it is stretched by `fill` along it.
        let run_z = lot.size.y > lot.size.x;
        let (yaw, stretch) = if run_z {
            (along.x.atan2(along.y), Vec3::new(1.0, 1.0, fill))
        } else {
            ((-along.y).atan2(along.x), Vec3::new(fill, 1.0, 1.0))
        };
        let at = from_orig(at - centre);
        out.push(WallPiece {
            mesh,
            matrix: Mat4::from_translation(Vec3::new(at.x, 0.0, at.y))
                * Mat4::from_rotation_y(yaw)
                * Mat4::from_scale(Vec3::splat(WALL_SCALE))
                * Mat4::from_scale(stretch)
                * lot.placement,
        });
    };
    for t in &plan.towers {
        let along = from_orig(Vec2::new(t.heading.cos(), t.heading.sin()));
        put(&mut out, tower, t.at, along, 1.0);
    }
    // The length of the stretch along the wall, world units.
    let seg_len = (stretch.1.size.max_element() * WALL_SCALE).max(0.05);
    for seg in &plan.segments {
        // `AddSplineModel` draws nothing for fewer than four points.
        if seg.len() < 4 {
            continue;
        }
        let table = arc_table(seg);
        let length = table.last().map_or(0.0, |(_, s)| *s);
        if length <= 0.0 {
            continue;
        }
        let n = ((length / seg_len).floor() as usize).max(1);
        let fill = length / (n as f32 * seg_len);
        for k in 0..n {
            let (a, b) = (
                param_at(&table, length * k as f32 / n as f32),
                param_at(&table, length * (k + 1) as f32 / n as f32),
            );
            let (pa, pb) = (wall_gen::eval(seg, a), wall_gen::eval(seg, b));
            let along = from_orig((pb - pa).normalize_or_zero());
            put(&mut out, stretch, (pa + pb) * 0.5, along, fill);
        }
    }
    out
}

/// The arc length of the spline of `seg` at even steps of its parameter: `(t, length to t)`.
fn arc_table(seg: &[Vec2]) -> Vec<(f32, f32)> {
    let end = (seg.len() - 2) as f32;
    let steps = (seg.len() - 2) * 16;
    let mut out = vec![(0.0, 0.0)];
    let mut prev = wall_gen::eval(seg, 0.0);
    let mut s = 0.0;
    for k in 1..=steps {
        let t = end * k as f32 / steps as f32;
        let p = wall_gen::eval(seg, t);
        s += prev.distance(p);
        prev = p;
        out.push((t, s));
    }
    out
}

/// The parameter at the arc length `s` of an [`arc_table`].
fn param_at(table: &[(f32, f32)], s: f32) -> f32 {
    let i = table
        .partition_point(|(_, l)| *l < s)
        .clamp(1, table.len() - 1);
    let ((t0, l0), (t1, l1)) = (table[i - 1], table[i]);
    if l1 - l0 <= f32::EPSILON {
        return t1;
    }
    t0 + (t1 - t0) * ((s - l0) / (l1 - l0)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use asset_format::{ArtTable, WallArt};
    use glam::Vec2;

    use super::*;
    use crate::world::city::compose::{Lot, ModelMeta};

    fn lot(mesh: u16, size: Vec2) -> Lot {
        Lot {
            ground: None,
            building: Some(mesh),
            placement: Mat4::IDENTITY,
            radius: 0.5 * size.max_element(),
            height: 100.0,
            base_side: 0.0,
            size,
        }
    }

    fn wall_model() -> ModelMeta {
        ModelMeta {
            lots: vec![
                lot(0, Vec2::new(112.0, 138.0)),
                lot(1, Vec2::new(68.0, 84.0)),
                lot(2, Vec2::new(88.0, 88.0)),
            ],
            ..ModelMeta::default()
        }
    }

    fn table() -> ArtTable {
        let mut t = ArtTable::new();
        for (culture, era, model) in [
            ("EUROPEAN", "Ancient", "md:euro_walls"),
            ("EUROPEAN", "Rennaisance", "md:euro_walls"),
            ("ASIAN", "Ancient", "md:asia_walls"),
        ] {
            t.insert_wall(
                culture,
                era,
                WallArt {
                    model: asset_format::LogicalId::new(model),
                    parts: 3,
                },
            );
        }
        t.insert_wall(
            "ART_DEF_BUILDING_WALLS_OF_BABYLON",
            "Any",
            WallArt {
                model: asset_format::LogicalId::new("md:wall_of_babylon"),
                parts: 3,
            },
        );
        t
    }

    fn tags(t: &[&str]) -> Vec<String> {
        t.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn only_a_city_with_walls_has_a_wall() {
        let t = table();
        assert!(wall_of(&t, &tags(&["TEMPLE"]), Culture::Asia, EraBucket::Ancient).is_none());
        let w = wall_of(&t, &tags(&[WALLS_TAG]), Culture::Asia, EraBucket::Ancient).unwrap();
        assert_eq!(w.model.as_str(), "md:asia_walls");
        // The era has none: an older one of the culture, then Europe.
        let w = wall_of(&t, &tags(&[WALLS_TAG]), Culture::Asia, EraBucket::Modern).unwrap();
        assert_eq!(w.model.as_str(), "md:asia_walls");
        let w = wall_of(
            &t,
            &tags(&[WALLS_TAG]),
            Culture::Afri,
            EraBucket::Renaissance,
        )
        .unwrap();
        assert_eq!(w.model.as_str(), "md:euro_walls");
    }

    #[test]
    fn babylon_has_a_wall_of_its_own() {
        let t = table();
        let w = wall_of(
            &t,
            &tags(&["ART_DEF_BUILDING_WALLS_OF_BABYLON"]),
            Culture::Euro,
            EraBucket::Ancient,
        )
        .unwrap();
        assert_eq!(w.model.as_str(), "md:wall_of_babylon");
    }

    fn round_plan() -> WallPlan {
        let ring: Vec<Vec2> = (0..20)
            .map(|k| {
                let a = k as f32 / 20.0 * std::f32::consts::TAU;
                Vec2::new(500.0, 200.0) + Vec2::new(a.cos(), a.sin()) * 30.0
            })
            .collect();
        wall_gen::plan(ring)
    }

    #[test]
    fn the_ring_has_the_towers_of_the_plan_and_stretches_round_it() {
        let plan = round_plan();
        let r = ring(&wall_model(), &plan, Vec2::new(500.0, 200.0));
        let count = |mesh| r.iter().filter(|p| p.mesh == mesh).count();
        assert_eq!(count(2), plan.towers.len());
        assert_eq!(count(0), 0, "no gatehouse");
        // A stretch of 84 is 4.2 long: some 45 round a ring of 188.
        assert!((40..=60).contains(&count(1)), "{}", count(1));
        for p in &r {
            let at = p.matrix.transform_point3(Vec3::ZERO);
            let d = Vec2::new(at.x, at.z).length();
            assert!((d - 30.0).abs() < 1.5, "{d}");
        }
        // The pieces are small: a tower of 88 is 4.4 wide.
        let t = r.iter().find(|p| p.mesh == 2).unwrap();
        assert!((t.matrix.transform_vector3(Vec3::X).length() - 0.05).abs() < 1e-5);
    }

    #[test]
    fn the_stretches_of_a_segment_are_whole_copies_stretched_to_meet_end_to_end() {
        let plan = round_plan();
        let centre = Vec2::new(500.0, 200.0);
        let r = ring(&wall_model(), &plan, centre);
        let stretches: Vec<&WallPiece> = r.iter().filter(|p| p.mesh == 1).collect();
        // The stretch of 84 is 4.2 long; each copy is stretched along z by length / (n * 4.2).
        let mut i = 0;
        for seg in plan.segments.iter().filter(|s| s.len() >= 4) {
            let length = wall_gen::spline_length(seg);
            let n = ((length / 4.2).floor() as usize).max(1);
            let pieces = &stretches[i..i + n];
            i += n;
            let mut total = 0.0;
            for p in pieces {
                let z = p.matrix.transform_vector3(Vec3::Z).length() * 84.0;
                assert!((4.2 - 1e-4..8.4).contains(&z), "{z}");
                total += z;
            }
            // The copies fill the spline (their chords are a little shorter than the arc).
            assert!((total - length).abs() < 0.02 * length, "{total} {length}");
            // Each copy ends where the next begins.
            let end = |p: &WallPiece, s: f32| p.matrix.transform_point3(Vec3::new(0.0, 0.0, s));
            for w in pieces.windows(2) {
                let gap = end(w[0], 42.0).distance(end(w[1], -42.0));
                let gap2 = end(w[0], -42.0).distance(end(w[1], 42.0));
                assert!(gap.min(gap2) < 0.2, "{gap} {gap2}");
            }
        }
        assert!(i > 10);
        assert_eq!(i, stretches.len());
    }

    #[test]
    fn a_wall_model_without_its_three_parts_gives_no_ring() {
        let mut m = wall_model();
        m.lots.pop();
        assert!(ring(&m, &round_plan(), Vec2::new(500.0, 200.0)).is_empty());
    }

    #[test]
    fn the_pieces_run_along_the_wall() {
        let r = ring(&wall_model(), &round_plan(), Vec2::new(500.0, 200.0));
        for p in r.iter().filter(|p| p.mesh == 1) {
            // A stretch whose long side is z: its z axis runs at right angles to the radius.
            let z = p.matrix.transform_vector3(Vec3::Z).normalize();
            let at = p.matrix.transform_point3(Vec3::ZERO);
            let outward = Vec2::new(at.x, at.z).normalize();
            assert!(
                Vec2::new(z.x, z.z).dot(outward).abs() < 0.3,
                "{z:?} {outward:?}"
            );
        }
    }
}
