//! Squad layout (RV-8): the original's `cvUnitFormationLib::GenerateUnitFormation` (0x08a290a8).
//! The exe uses the offsets as they come (no squeeze into the hex: a big squad overhangs it); B1's
//! entities still squeeze theirs ([`fit`]).
//!
//! The stream of the jitter is reset for every unit to its seed: the original's seed is the slot of
//! the unit's sim in the pool of `cvUnitSimSystem` (0, 1, 2, .. in creation order, freed slots
//! reused).
// re: cvUnitSimSystem::OnUnitCreated 0x088037da (m_uiFormationSeed = pool slot), UpdateFromHandle
// 0x0880453e (lib m_iRandSeedOverride = the seed; GenerateUnitFormation resets FRandom to it)
//! SIMPLIFIED(SIMP-023): the seed is the unit's id (per player, in creation order), the stable
//! number nearest the sim slot; the slots depend on the creation order of every unit of the game,
//! which the client does not see.

use asset_format::Formation;
use glam::Vec2;

/// Half-extent a squad may take inside its hex when it has the cell alone, and when it shares the
/// cell with the other kind of unit (figures must stay inside the hex, apothem 32).
pub const ALONE_HALF_EXTENT: Vec2 = Vec2::new(24.0, 22.0);
pub const SHARED_HALF_EXTENT: Vec2 = Vec2::new(10.0, 22.0);
/// A squad is never squeezed below this fraction of its natural size.
const MIN_FIT: f32 = 0.5;

/// Modulus the ctor stores in the generator; the reduced value is the new state after every step.
const RAND_MODULUS: u32 = 0xffff_cfef;
/// Scale of the generator, 1 / 0xffffcfef: draw = state * 2^-32 * (1 + 0x3011_09066 / 2^52).
const RAND_SCALE: f64 =
    (1.0 + 0x3_0110_9066_u64 as f64 / 4_503_599_627_370_496.0) / 4_294_967_296.0;

/// `FRandom` step (`Fun_087fbf02`): returns a draw in [0, 1).
struct Rand(u32);

impl Rand {
    /// The stream reset to a seed (`m_iRandSeedOverride`): the state is the seed.
    fn seeded(seed: u32) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(0x343fd).wrapping_add(0x26_9ec3);
        self.0 %= RAND_MODULUS;
        (f64::from(self.0) * RAND_SCALE) as f32
    }
}

/// The fields `GenerateUnitFormation` reads (`UnitFormationData.h` `UnitFormation`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    pub spacing_x: u32,
    pub spacing_y: u32,
    pub spacing_multiplier: f32,
    pub unit_randomness: f32,
    pub per_row_inc: i32,
    pub y_offset_inc: f32,
    pub y_offset_curved: i32,
    pub stagger: i32,
    pub total_width: i32,
    pub x_block_size: i32,
    pub x_block_spacing_count: i32,
    pub y_block_size: i32,
    pub y_block_spacing_count: i32,
    pub center_width: i32,
}

impl Params {
    /// The record "Default" the library builds in code (entry 0), for a unit without a formation
    /// record or with a name the library does not have.
    // re: cvUnitFormationLib::Startup 0x08a28526 (6, 7, 1.0, 0.2, 0.2, 0, 1.2, 1, 0, 4, -1, 0, -1, 0, 0),
    // GetFormation (cvUnitFormationLib.cpp:55: m_pFormations[0] when not found)
    pub const DEFAULT: Self = Self {
        spacing_x: 6,
        spacing_y: 7,
        spacing_multiplier: 1.0,
        unit_randomness: 0.2,
        per_row_inc: 0,
        y_offset_inc: 1.2,
        y_offset_curved: 1,
        stagger: 0,
        total_width: 4,
        x_block_size: -1,
        x_block_spacing_count: 0,
        y_block_size: -1,
        y_block_spacing_count: 0,
        center_width: 0,
    };

    /// `DefaultMelee` of `UnitFormations.xml` (an ordinary named record).
    pub const DEFAULT_MELEE: Self = Self {
        spacing_x: 13,
        spacing_y: 13,
        spacing_multiplier: 1.0,
        unit_randomness: 0.2,
        per_row_inc: 0,
        y_offset_inc: 1.0,
        y_offset_curved: 1,
        stagger: 0,
        total_width: 4,
        x_block_size: 0,
        x_block_spacing_count: 0,
        y_block_size: 0,
        y_block_spacing_count: 0,
        center_width: 0,
    };

    /// The formation a unit's art names: the record of the art table, else the shipped record of
    /// that name ([`SHIPPED`]), else the code default (no name, or a name the library lacks).
    /// The B2 art table carries no `formation/` records (only B1's trees set added them), so
    /// without the shipped records every unit fell to the default.
    pub fn of_name<'a>(name: Option<&str>, table: impl Fn(&str) -> Option<&'a Formation>) -> Self {
        let Some(name) = name else {
            return Self::DEFAULT;
        };
        if let Some(f) = table(name) {
            return Self::from_record(Some(f));
        }
        Self::shipped(name).unwrap_or(Self::DEFAULT)
    }

    /// The record `name` of the shipped `UnitFormations.xml` (the name as the library hashes it,
    /// case and spaces kept).
    pub fn shipped(name: &str) -> Option<Self> {
        SHIPPED.iter().find(|(n, _)| *n == name).map(|&(_, p)| p)
    }

    /// Parameters of a formation record (named fields plus `extra`), or the defaults.
    pub fn from_record(record: Option<&Formation>) -> Self {
        let Some(f) = record else {
            return Self::DEFAULT;
        };
        let extra = |key: &str, default: f32| f.extra.get(key).copied().unwrap_or(default);
        Self {
            spacing_x: f.spacing_x.max(0.0) as u32,
            spacing_y: f.spacing_y.max(0.0) as u32,
            spacing_multiplier: extra("SpacingMultiplier", 1.0),
            unit_randomness: f.unit_randomness,
            per_row_inc: extra("PerRowInc", 0.0) as i32,
            y_offset_inc: extra("YOffsetInc", 0.0),
            y_offset_curved: extra("YOffsetCurved", 0.0) as i32,
            stagger: f.stagger as i32,
            total_width: f.total_width as i32,
            x_block_size: extra("XBlockSize", 0.0) as i32,
            x_block_spacing_count: extra("XBlockSpacingCount", 0.0) as i32,
            y_block_size: extra("YBlockSize", 0.0) as i32,
            y_block_spacing_count: extra("YBlockSpacingCount", 0.0) as i32,
            center_width: extra("CenterWidth", 0.0) as i32,
        }
    }
}

/// A record of `UnitFormations.xml`: spacing x, y, multiplier, `UnitRandomness`, `PerRowInc`,
/// `YOffsetInc`, `YOffsetCurved`, `Stagger`, `TotalWidth`, `XBlockSize`, `XBlockSpacingCount`,
/// `YBlockSize`, `YBlockSpacingCount`, `CenterWidth` (`BlockRandomness` is not read).
#[allow(clippy::too_many_arguments)]
const fn rec(
    sx: u32,
    sy: u32,
    mult: f32,
    rnd: f32,
    per_row: i32,
    y_inc: f32,
    curved: i32,
    stagger: i32,
    width: i32,
    xb: i32,
    xbs: i32,
    yb: i32,
    ybs: i32,
    centre: i32,
) -> Params {
    Params {
        spacing_x: sx,
        spacing_y: sy,
        spacing_multiplier: mult,
        unit_randomness: rnd,
        per_row_inc: per_row,
        y_offset_inc: y_inc,
        y_offset_curved: curved,
        stagger,
        total_width: width,
        x_block_size: xb,
        x_block_spacing_count: xbs,
        y_block_size: yb,
        y_block_spacing_count: ybs,
        center_width: centre,
    }
}

/// The 24 records of `Assets/Units/UnitFormations.xml` of the install (Lekmod ships none of its
/// own), in file order.
// SIMPLIFIED(SIMP-exe-1272): held by the client because the B2 art table has no formation records.
pub const SHIPPED: [(&str, Params); 24] = [
    (
        "DefaultMelee",
        rec(13, 13, 1.0, 0.2, 0, 1.0, 1, 0, 4, 0, 0, 0, 0, 0),
    ),
    (
        "DefaultCavalry",
        rec(12, 25, 1.0, 0.2, 0, 1.0, 1, 1, 3, 0, 0, 0, 0, 4),
    ),
    (
        "Warrior",
        rec(13, 13, 1.0, 0.4, 0, 1.0, 1, 0, 4, 0, 0, 0, 0, 0),
    ),
    (
        "Archer",
        rec(9, 12, 1.0, 0.5, 0, 1.0, 1, 0, 6, 0, 0, 0, 0, 0),
    ),
    (
        "Scout",
        rec(11, 14, 1.0, 0.6, 0, 1.0, 1, 0, 4, 0, 0, 0, 0, 0),
    ),
    (
        "Phalanx",
        rec(11, 11, 1.0, 0.2, 0, 1.0, 1, 0, 4, 0, 0, 0, 0, 0),
    ),
    (
        "Pikeman",
        rec(9, 12, 1.0, 0.2, 0, 3.0, 30, 0, 6, 0, 0, 0, 0, 0),
    ),
    (
        "HonorableGunpowder",
        rec(8, 13, 1.0, 0.1, 0, 1.0, 1, 0, 7, 0, 0, 0, 0, 0),
    ),
    (
        "ThreeBigGuns",
        rec(20, 36, 1.0, 0.2, 0, 6.0, 1, 0, 3, 0, 0, 0, 0, 0),
    ),
    (
        "TwoBigGuns",
        rec(28, 36, 1.0, 0.2, 0, 1.0, 1, 0, 3, 0, 0, 0, 0, 0),
    ),
    (
        "Vehicle",
        rec(18, 12, 1.0, 0.2, 0, 6.0, 1, 0, 3, 0, 0, 0, 0, 0),
    ),
    (
        "ChariotElephant",
        rec(24, 36, 1.0, 0.2, 0, 1.0, 1, 0, 2, 0, 0, 0, 0, 0),
    ),
    (
        "Samurai",
        rec(8, 13, 1.0, 0.2, 0, 1.0, 1, 0, 6, 2, 1, 3, 1, 2),
    ),
    (
        "UnFormed",
        rec(9, 14, 1.0, 0.6, 0, 1.0, 1, 1, 5, 0, 0, 0, 0, 0),
    ),
    (
        "Barbarian",
        rec(9, 14, 1.0, 0.8, 0, 1.0, 1, 1, 4, 0, 0, 0, 0, 0),
    ),
    (
        "LooseCivilian",
        rec(12, 14, 1.0, 0.8, 0, 1.0, 1, 1, 4, 0, 0, 0, 0, 0),
    ),
    (
        "Melee Front",
        rec(11, 11, 1.0, 0.0, 0, 8.8268, 1, 0, 5, 0, 0, 0, 0, 0),
    ),
    (
        "BomberWing",
        rec(20, 12, 1.0, 0.2, 0, 6.0, 1, 0, 3, 0, 0, 0, 0, 0),
    ),
    (
        "FighterWing",
        rec(20, 12, 1.0, 0.2, 0, 6.0, 1, 0, 3, 0, 0, 0, 0, 0),
    ),
    (
        "EarlyGreatArtist",
        rec(9, 13, 1.0, 0.0, 0, 6.215, 1, 0, 5, 0, 0, 0, 0, 0),
    ),
    (
        "EarlyGreatMerchant",
        rec(9, 5, 1.39252, 0.0, 0, 10.0, 1, 0, 3, 4, 0, 0, 1, 0),
    ),
    (
        "OffsetPikeman",
        rec(9, 12, 1.0, 0.2, 0, 1.0, 30, 1, 6, 3, 1, 0, 0, 0),
    ),
    (
        "ThreeInLine",
        rec(6, 24, 1.0, 0.0, 0, 1.2, 0, 0, 1, -1, 0, 0, 0, 0),
    ),
    (
        "TheeInLineStag",
        rec(22, 23, 1.0, 1.0, 0, 0.0, 0, 0, 1, 0, 0, 0, 0, 0),
    ),
];

/// Offsets of `count` figures of a squad of `group` (RV-8: `GenerateUnitFormation`), centred on
/// the middle of their bounding box, the jitter from the stream reset to `seed`. Offset x is across, y is the row axis:
/// row 0 has the smallest y and is the front row, so the placement turns y away from the facing
/// (`world/unit/place.rs`). Pass the full squad size as both, so the living figures keep their
/// places as the others die.
pub fn generate(count: usize, group: usize, p: &Params, seed: i32) -> Vec<Vec2> {
    if count == 0 {
        return Vec::new();
    }
    let mut width = p.total_width;
    if !(1..=11).contains(&width) {
        width = (group as f32).sqrt().ceil() as i32 + 1;
    }
    let x_period = if p.x_block_size != 0 {
        p.x_block_size.saturating_mul(2)
    } else {
        0x3fff_fffe
    };
    let y_period = if p.y_block_size != 0 {
        p.y_block_size
    } else {
        0x1fff_ffff
    };
    let mut rng = Rand::seeded(seed as u32);

    let sx = p.spacing_x as f32 * p.spacing_multiplier;
    let sy = p.spacing_y as f32 * p.spacing_multiplier;
    let half_center_gap =
        if p.center_width != 0 { 0.0 } else { 1.0 } * p.x_block_spacing_count as f32 * sx * 0.5;
    let randomness = p.unit_randomness * 0.4;
    let block_gap_x = p.x_block_spacing_count as f32 * sx;
    let block_gap_y = p.y_block_spacing_count as f32 * sy;

    let (mut min, mut max) = (Vec2::ZERO, Vec2::ZERO);
    let (mut sign, mut column, mut row, mut stagger_offset) = (1.0_f32, 0_i32, 0_i32, 0_i32);
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let row_length = (stagger_offset + width + p.per_row_inc * 2 * row).max(1);
        let from_center = column - p.center_width;
        let past_center = from_center >= 0;
        let from_center = from_center.max(0);
        let parity = row_length % 2;
        let next_parity = (row_length + 1) % 2;
        let block = i32::from(past_center && p.center_width != 0) + from_center / x_period;
        let row_origin =
            (next_parity as f32 * 0.5 * sx - half_center_gap) - block as f32 * block_gap_x;
        let mut x = (((column + 2 - parity) / 2) as f32 * sx - row_origin) * sign;

        let y_offset = if p.y_offset_curved == 0 {
            ((parity + column) / 2) as f32 * p.y_offset_inc
        } else {
            let half_width = ((row_length + 1 - parity) / 2) as f32 * sx - row_origin;
            let curve = ((x / half_width) * 1.570_795).cos();
            (((parity + row_length) / 2) as f32 * (1.0 - curve) * p.y_offset_inc).abs()
        };

        x += (rng.next() - 0.5) * 2.0 * sx * randomness;
        let mut y = row as f32 * sy + (rng.next() - 0.5) * 2.0 * sy * randomness + y_offset;
        y += (row / y_period) as f32 * block_gap_y;
        out.push(Vec2::new(x, y));

        column += 1;
        if column == row_length {
            row += 1;
            stagger_offset = (row % 2) * p.stagger;
            column = 0;
        }
        sign = -sign;
        // The bounding box starts at the origin, as in the original.
        if x <= min.x {
            min.x = x;
        }
        if y <= min.y {
            min.y = y;
        }
        if x > max.x {
            max.x = x;
        }
        if y > max.y {
            max.y = y;
        }
    }
    let centre = (max + min) * 0.5;
    out.iter_mut().for_each(|o| *o -= centre);
    out
}

/// Squeezes `offsets` uniformly into `half_extent` (not below half their size).
pub fn fit(offsets: &mut [Vec2], half_extent: Vec2) {
    let need = offsets.iter().fold(Vec2::ZERO, |m, o| m.max(o.abs()));
    let k = (half_extent.x / need.x.max(1e-3))
        .min(half_extent.y / need.y.max(1e-3))
        .clamp(MIN_FIT, 1.0);
    if k < 1.0 {
        offsets.iter_mut().for_each(|o| *o *= k);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn warrior() -> Params {
        Params {
            unit_randomness: 0.4,
            ..Params::DEFAULT_MELEE
        }
    }

    #[test]
    fn rand_is_the_msvc_lcg_in_unit_range() {
        let mut r = Rand(1);
        let a = r.next();
        assert_eq!(r.0, 0x343fd + 0x26_9ec3);
        assert!((0.0..1.0).contains(&a));
        let draws: Vec<f32> = (0..1000).map(|_| r.next()).collect();
        assert!(draws.iter().all(|d| (0.0..1.0).contains(d)));
        assert!(draws.iter().any(|d| *d > 0.9) && draws.iter().any(|d| *d < 0.1));
    }

    #[test]
    fn warrior_has_twelve_distinct_places_in_four_columns() {
        let g = generate(12, 12, &warrior(), 7);
        assert_eq!(g.len(), 12);
        for (i, a) in g.iter().enumerate() {
            assert!(
                g[i + 1..].iter().all(|b| a.distance(*b) > 3.0),
                "{i}: {g:?}"
            );
        }
        // Rows of four: the second row sits about one SpacingY behind the first.
        let mean_y = |r: std::ops::Range<usize>| {
            g[r.clone()].iter().map(|o| o.y).sum::<f32>() / r.len() as f32
        };
        let (r0, r1, r2) = (mean_y(0..4), mean_y(4..8), mean_y(8..12));
        assert!(
            (r1 - r0 - 13.0).abs() < 6.0 && (r2 - r1 - 13.0).abs() < 6.0,
            "{r0} {r1} {r2}"
        );
        // Centred on the middle of the bounding box.
        let (lo, hi) = g
            .iter()
            .fold((Vec2::MAX, Vec2::MIN), |(l, h), o| (l.min(*o), h.max(*o)));
        assert!(lo.x < 0.0 && hi.x > 0.0 && lo.y < 0.0 && hi.y > 0.0);
    }

    #[test]
    fn without_randomness_it_is_a_symmetric_pattern() {
        let p = Params {
            unit_randomness: 0.0,
            y_offset_curved: 0,
            y_offset_inc: 0.0,
            ..Params::DEFAULT_MELEE
        };
        let g = generate(4, 4, &p, 1);
        // One row of four, alternating sides: x = +-6.5, +-19.5.
        let mut xs: Vec<f32> = g.iter().map(|o| o.x).collect();
        xs.sort_by(f32::total_cmp);
        let want = [-19.5, -6.5, 6.5, 19.5];
        let sum: f32 = xs.iter().sum();
        assert!(sum.abs() < 1e-3, "{xs:?}");
        for (x, w) in xs.iter().zip(want) {
            assert!((x - w).abs() < 1e-3, "{xs:?}");
        }
        assert!(g.iter().all(|o| o.y.abs() < 1e-3));
    }

    #[test]
    fn one_figure_is_a_single_place_near_the_centre() {
        let g = generate(1, 1, &Params::DEFAULT, 5);
        assert_eq!(g.len(), 1);
        assert!(g[0].length() < 13.0, "{:?}", g[0]);
        assert!(generate(0, 0, &Params::DEFAULT, 5).is_empty());
    }

    #[test]
    fn positions_are_stable_and_depend_on_the_unit() {
        let a = generate(12, 12, &warrior(), 3);
        assert_eq!(a, generate(12, 12, &warrior(), 3));
        assert_ne!(a, generate(12, 12, &warrior(), 4));
    }

    #[test]
    fn record_fields_reach_the_parameters() {
        let mut f = Formation {
            spacing_x: 9.0,
            spacing_y: 12.0,
            total_width: 5.0,
            unit_randomness: 0.3,
            stagger: 1.0,
            extra: Default::default(),
        };
        f.extra.insert("CenterWidth".into(), 4.0);
        f.extra.insert("YOffsetCurved".into(), 1.0);
        let p = Params::from_record(Some(&f));
        assert_eq!(
            (p.spacing_x, p.spacing_y, p.total_width, p.stagger),
            (9, 12, 5, 1)
        );
        assert_eq!(
            (p.center_width, p.y_offset_curved, p.spacing_multiplier),
            (4, 1, 1.0)
        );
        assert_eq!(Params::from_record(None), Params::DEFAULT);
    }

    #[test]
    fn staggered_and_wide_formations_stay_finite() {
        let cav = Params {
            spacing_x: 12,
            spacing_y: 25,
            stagger: 1,
            total_width: 3,
            center_width: 4,
            unit_randomness: 0.2,
            ..Params::DEFAULT
        };
        for n in [1, 2, 5, 12, 40] {
            for p in [
                cav,
                Params {
                    total_width: 0,
                    ..cav
                },
                Params {
                    total_width: 11,
                    ..cav
                },
            ] {
                let g = generate(n, n, &p, 9);
                assert_eq!(g.len(), n);
                assert!(g.iter().all(|o| o.is_finite()), "{n} {p:?}");
            }
        }
    }

    /// The B2 art table has no `formation/` records: a unit naming `DefaultCavalry` or
    /// `DefaultMelee` must still get the spacing of the shipped record, not the 6 x 7 of the code
    /// default (live report 2026-11-03: figures crowded).
    #[test]
    fn the_standard_formations_keep_the_spacing_of_the_original_records() {
        let none = |_: &str| None;
        let cav = Params::of_name(Some("DefaultCavalry"), none);
        assert_eq!(
            (
                cav.spacing_x,
                cav.spacing_y,
                cav.stagger,
                cav.total_width,
                cav.center_width
            ),
            (12, 25, 1, 3, 4)
        );
        assert_eq!((cav.unit_randomness, cav.y_offset_inc), (0.2, 1.0));
        let melee = Params::of_name(Some("DefaultMelee"), none);
        assert_eq!(melee, Params::DEFAULT_MELEE);
        assert_eq!(Params::of_name(Some("Pikeman"), none).y_offset_curved, 30);
        assert_eq!(Params::of_name(Some("Melee Front"), none).total_width, 5);
        // No name, or a name the library lacks: the code default.
        assert_eq!(Params::of_name(None, none), Params::DEFAULT);
        assert_eq!(Params::of_name(Some("Nope"), none), Params::DEFAULT);
        // A record of the art table wins over the shipped one.
        let f = Formation {
            spacing_x: 30.0,
            spacing_y: 30.0,
            total_width: 2.0,
            unit_randomness: 0.0,
            stagger: 0.0,
            extra: Default::default(),
        };
        assert_eq!(
            Params::of_name(Some("DefaultCavalry"), |_| Some(&f)).spacing_x,
            30
        );
        // Cavalry of five without jitter: rows 25 apart and nothing squeezed (the exe uses the
        // offsets as they come).
        let g = generate(
            5,
            5,
            &Params {
                unit_randomness: 0.0,
                ..cav
            },
            0,
        );
        let (lo, hi) = g
            .iter()
            .fold((Vec2::MAX, Vec2::MIN), |(l, h), o| (l.min(*o), h.max(*o)));
        assert!(hi.y - lo.y >= 25.0, "{g:?}");
        let closest = g
            .iter()
            .enumerate()
            .flat_map(|(i, a)| g[i + 1..].iter().map(move |b| a.distance(*b)))
            .fold(f32::MAX, f32::min);
        assert!(closest >= 12.0, "{closest} {g:?}");
    }

    #[test]
    fn a_unit_without_a_record_takes_the_code_default_of_six_by_seven() {
        let p = Params::from_record(None);
        assert_eq!((p.spacing_x, p.spacing_y, p.y_offset_inc), (6, 7, 1.2));
        let g = generate(
            8,
            8,
            &Params {
                unit_randomness: 0.0,
                ..p
            },
            0,
        );
        // Two rows of four, 7 apart (plus the curve of 1.2 a step).
        let ys: Vec<f32> = g.iter().map(|o| o.y).collect();
        let (r0, r1) = (
            ys[..4].iter().sum::<f32>() / 4.0,
            ys[4..].iter().sum::<f32>() / 4.0,
        );
        assert!((r1 - r0 - 7.0).abs() < 1e-3, "{ys:?}");
        assert!(g.iter().all(|o| o.is_finite()));
    }

    #[test]
    fn the_jitter_stream_starts_at_the_seed() {
        let mut r = Rand::seeded(5);
        r.next();
        assert_eq!(r.0, (5u32.wrapping_mul(0x343fd) + 0x26_9ec3) % RAND_MODULUS);
    }

    #[test]
    fn fit_squeezes_but_not_below_half() {
        let mut g = generate(12, 12, &warrior(), 1);
        fit(&mut g, ALONE_HALF_EXTENT);
        assert!(
            g.iter()
                .all(|o| o.x.abs() <= 24.0 + 1e-3 && o.y.abs() <= 22.0 + 1e-3)
        );
        let mut s = generate(12, 12, &warrior(), 1);
        let before = s.iter().fold(0.0_f32, |m, o| m.max(o.x.abs()));
        fit(&mut s, SHARED_HALF_EXTENT);
        let after = s.iter().fold(0.0_f32, |m, o| m.max(o.x.abs()));
        assert!(
            after <= 10.0 + 1e-3 || (after / before - 0.5).abs() < 1e-3,
            "{before} {after}"
        );
    }
}
