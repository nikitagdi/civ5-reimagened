//! The five levels of a coastline: `GenerateAllLevelSplines` and
//! `GenerateLevels`. For every coastline five parallel lines are made, from the land side to the
//! deep water: the shift of every level comes from the styles of the hexes the line passes, blended
//! along it; each is cut twice, given the character of its profile, and cut once more, so a line of
//! `n` points gives levels of `8n - 14` points.

use glam::Vec2;

use super::bspline;
use super::lines::{CoastCP, CoastlineSpline};
use super::styles::{CoastStyles, LEVELS, STRIPS, STYLE_COUNT, StyleDef, StyleLevel};
use crate::world::terrain::library::TerrainDefs;

/// The level that is the waterline.
// re: TerrainSystem.cpp:55 (kCoastlineLevel)
pub const COASTLINE_LEVEL: usize = 2;

/// The continent whose styles and terrains the coast takes: every line has `m_iContinent = 1`.
// re: ExtractSingleCoastlineFromGrid 0x088ec7a2 (`*(a1 + 0x28) = 1`), CreateCoastMeshJob::Execute 0x088ec056 (continent 1)
pub const COAST_CONTINENT: u8 = 1;

/// A terrain index that means "no terrain for this strip".
pub const NO_TERRAIN: u8 = 0xff;

/// A style with its terrain types turned into rows of the table `TerrainDef`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedStyle {
    pub levels: [StyleLevel; LEVELS],
    /// Index in the table of the terrain of every strip, [`NO_TERRAIN`] for none.
    pub terrain: [u8; STRIPS],
}

/// Row of the table for a terrain type of a continent: the continent's own texture first, then
/// the one of continent 0, [`NO_TERRAIN`] if neither is there.
// re: Fun_088e96b4 0x088e96b4 (`g_aContinentPrefix[uiContinent] | uiType`, then `g_aContinentPrefix[0] | uiType`)
fn resolve_terrain(defs: &TerrainDefs, continent: u8, ty: u16) -> u8 {
    let find = |c: u8| {
        defs.rows()
            .iter()
            .position(|r| r.continent == c && r.id == u32::from(ty))
    };
    if let Some(i) = find(continent)
        && i as u8 != NO_TERRAIN
    {
        return i as u8;
    }
    find(0).map_or(NO_TERRAIN, |i| i as u8)
}

/// All the styles of the coast, resolved against the table of the terrains of the map.
#[derive(Clone, Debug)]
pub struct StyleSet {
    styles: Vec<ResolvedStyle>,
    zero: ResolvedStyle,
    profiles: Vec<Vec<Vec2>>,
}

impl StyleSet {
    /// The styles of `styles` for the continent [`COAST_CONTINENT`] and the table `defs`.
    // re: Fun_088e96b4 0x088e96b4
    pub fn new(styles: &CoastStyles, defs: &TerrainDefs) -> Self {
        let resolve = |s: &StyleDef| ResolvedStyle {
            levels: s.levels,
            terrain: s
                .terrain_type
                .map(|t| t.map_or(NO_TERRAIN, |t| resolve_terrain(defs, COAST_CONTINENT, t))),
        };
        Self {
            styles: (0..STYLE_COUNT as u8)
                .map(|i| resolve(&styles.style_or_zero(i)))
                .collect(),
            zero: resolve(&StyleDef::zero()),
            profiles: styles
                .profiles()
                .iter()
                .map(|p| p.samples.clone())
                .collect(),
        }
    }

    /// The style of a kind byte of a control point; a byte past the thirteen is all zero.
    pub fn get(&self, index: u8) -> &ResolvedStyle {
        self.styles.get(usize::from(index)).unwrap_or(&self.zero)
    }

    /// A sample of a profile: sample `i` modulo the number of samples.
    fn sample(&self, profile: usize, i: usize) -> Option<Vec2> {
        let p = self.profiles.get(profile)?;
        (!p.is_empty()).then(|| p[i % p.len()])
    }
}

/// The five levels of one coastline: the control points of all levels one after the other, and
/// the range of each level in them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FinalLevels {
    pub ranges: [[usize; 2]; LEVELS],
    pub points: Vec<CoastCP>,
    pub line_type: i32,
}

impl FinalLevels {
    /// The control points of a level.
    pub fn level(&self, l: usize) -> &[CoastCP] {
        &self.points[self.ranges[l][0]..self.ranges[l][1]]
    }

    /// Points of every level.
    pub fn level_len(&self) -> usize {
        self.ranges[0][1] - self.ranges[0][0]
    }
}

/// The style state of a walk along a line: the styles of the point in hand and the parity of the
/// runs of one style. The blend weight is turned over on every other run.
pub(super) struct StyleWalk<'a> {
    set: &'a StyleSet,
    pub(super) a: ResolvedStyle,
    pub(super) b: ResolvedStyle,
    flip: bool,
    last: Option<u8>,
}

impl<'a> StyleWalk<'a> {
    pub(super) fn new(set: &'a StyleSet) -> Self {
        Self {
            set,
            a: set.zero,
            b: set.zero,
            flip: true,
            last: None,
        }
    }

    /// Takes the styles of a point and returns its blend weight with the parity applied.
    // re: GenerateLevels 0x088e7918 (`if (uiStyle != uiLastStyle) { ...; bFlip = !bFlip; }`)
    pub(super) fn at(&mut self, style_a: u8, style_b: u8, blend: f32) -> f32 {
        if self.last != Some(style_a) {
            self.a = *self.set.get(style_a);
            self.b = *self.set.get(style_b);
            self.flip = !self.flip;
            self.last = Some(style_a);
        }
        if self.flip { 1.0 - blend } else { blend }
    }
}

/// `GenerateLevels`.
// re: cvCoastMeshEngine::GenerateLevels 0x088e7918 (cvCoastMeshEngine.cpp:79-264)
pub fn generate_levels(spline: &CoastlineSpline, set: &StyleSet) -> FinalLevels {
    let src = &spline.points;
    let n = src.len();
    debug_assert!(n >= 4);
    let mut levels: [Vec<CoastCP>; LEVELS] = std::array::from_fn(|_| vec![CoastCP::default(); n]);

    // The level points: the point of the line moved along its normal by the blend of the
    // offsets of the two styles.
    let mut walk = StyleWalk::new(set);
    let shifted = |walk: &mut StyleWalk<'_>, base: &CoastCP, g: Vec2, value: &CoastCP| {
        let inv_len = 1.0 / (g.x * g.x + g.y * g.y).sqrt();
        let blend = walk.at(value.style_a, value.style_b, value.blend);
        let mut out = [CoastCP::default(); LEVELS];
        for (l, o) in out.iter_mut().enumerate() {
            let offset = walk.b.levels[l].offset * (1.0 - blend) + walk.a.levels[l].offset * blend;
            *o = CoastCP {
                pos: Vec2::new(
                    base.pos.x + g.y * inv_len * offset,
                    base.pos.y - g.x * inv_len * offset,
                ),
                blend: base.blend,
                style_a: base.style_a,
                style_b: base.style_b,
            };
        }
        out
    };
    if spline.closed {
        // Points 0 and n-1 of each level are the wrap copies written after the loop.
        let mut t = 0.5f32;
        for i in 0..n - 1 {
            let g = bspline::gradient_cp(src, t);
            let value = bspline::value_cp(src, t);
            t += 1.0;
            let out = shifted(&mut walk, &src[i + 1], g, &value);
            for (l, o) in out.into_iter().enumerate() {
                levels[l][i + 1] = o;
            }
        }
        for level in &mut levels {
            wrap_copies(level, spline.wrap_offset, n);
        }
    } else {
        let last_t = (n - 2) as f32 + -0.01;
        let mut t = 0.5f32;
        for i in 0..n {
            let (g, value) = if i == 0 {
                (bspline::gradient_cp(src, 0.0), bspline::value_cp(src, 0.0))
            } else if i == n - 1 {
                (
                    bspline::gradient_cp(src, last_t),
                    bspline::value_cp(src, last_t),
                )
            } else {
                let r = (bspline::gradient_cp(src, t), bspline::value_cp(src, t));
                t += 1.0;
                r
            };
            let out = shifted(&mut walk, &src[i], g, &value);
            for (l, o) in out.into_iter().enumerate() {
                levels[l][i] = o;
            }
        }
    }

    // Two cuts: n -> 2n-2 -> 4n-6.
    for level in &mut levels {
        for _ in 0..2 {
            *level = bspline::reconstruct_cp(level);
        }
    }
    let count = levels[0].len();

    // The character of the profile of the style, in the frame of the tangent, added to the point.
    let mut walk = StyleWalk::new(set);
    for (l, level) in levels.iter_mut().enumerate() {
        for i in 0..count - 1 {
            let g = bspline::gradient_cp(level, i as f32 + 0.5);
            let inv_len = 1.0 / (g.x * g.x + g.y * g.y).sqrt();
            let (style_a, style_b, point_blend) = {
                let p = &level[i];
                (p.style_a, p.style_b, p.blend)
            };
            let blend = walk.at(style_a, style_b, point_blend);
            if i == 0 {
                continue;
            }
            let (tx, ty) = (g.x * inv_len, g.y * inv_len);
            let character = |style: &ResolvedStyle| -> Vec2 {
                style.levels[l]
                    .character
                    .and_then(|c| set.sample(c, i))
                    .map_or(Vec2::ZERO, |s| {
                        Vec2::new(s.x * tx - s.y * ty, s.x * ty + s.y * tx)
                    })
            };
            let (a, b) = (character(&walk.a), character(&walk.b));
            level[i].pos.x += b.x * (1.0 - blend) + a.x * blend;
            level[i].pos.y += b.y * (1.0 - blend) + a.y * blend;
        }
        if spline.closed {
            wrap_copies(level, spline.wrap_offset, count);
        }
    }

    // The last cut: 4n-6 -> 8n-14.
    for level in &mut levels {
        *level = bspline::reconstruct_cp(level);
    }

    let size = levels[0].len();
    debug_assert_eq!(size, n * 8 - 14);
    let mut ranges = [[0; 2]; LEVELS];
    let mut points = Vec::with_capacity(size * LEVELS);
    for (l, level) in levels.iter().enumerate() {
        ranges[l] = [points.len(), points.len() + size];
        points.extend_from_slice(level);
    }
    FinalLevels {
        ranges,
        points,
        line_type: spline.line_type,
    }
}

/// Points 0 and `count - 1` of a closed line are the copies of `count - 2` and 1 one wrap away.
// re: GenerateLevels 0x088e7918 (`pLevel[0] = pLevel[uiCount - 2] - m_vWrapOffset` ...)
fn wrap_copies(level: &mut [CoastCP], wrap: Vec2, count: usize) {
    level[0].pos = level[count - 2].pos - wrap;
    level[0].blend = level[count - 2].blend;
    level[count - 1].pos = level[1].pos + wrap;
    level[count - 1].blend = level[1].blend;
}

#[cfg(test)]
pub(crate) mod tests {
    use asset_format::TerrainDefRow;

    use super::super::lines::CoastLines;
    use super::super::styles::synthetic_settings;
    use super::super::testkit::{frame_of, island};
    use super::*;

    pub fn defs_with_ids(ids: &[u32]) -> TerrainDefs {
        let mut rows = Vec::new();
        for continent in 0..5u8 {
            for &id in ids {
                rows.push(TerrainDefRow {
                    continent,
                    diffuse: None,
                    heightmap: None,
                    specmap: None,
                    base_dominance: 0,
                    texture_scale: 1.0,
                    projection: 0,
                    id,
                });
            }
        }
        TerrainDefs::new(rows)
    }

    pub fn island_levels(characters: bool) -> (Vec<CoastlineSpline>, Vec<FinalLevels>, StyleSet) {
        let b = island(20, 20, false, 2);
        let mut g = super::super::grid::CoastGrid::new(&b, &frame_of(&b));
        let lines = super::super::testkit::inner_lines(CoastLines::walk(&mut g), &b);
        let styles = CoastStyles::from_settings(&synthetic_settings(characters));
        let set = StyleSet::new(&styles, &defs_with_ids(&[5, 16, 53]));
        let levels = lines
            .splines
            .iter()
            .map(|s| generate_levels(s, &set))
            .collect();
        (lines.splines, levels, set)
    }

    #[test]
    fn a_line_of_n_points_gives_five_levels_of_8n_minus_14() {
        let (splines, levels, _) = island_levels(false);
        for (s, l) in splines.iter().zip(&levels) {
            let n = s.points.len();
            assert_eq!(l.level_len(), 8 * n - 14);
            assert_eq!(l.points.len(), 5 * (8 * n - 14));
            for k in 0..5 {
                assert_eq!(l.ranges[k][1] - l.ranges[k][0], 8 * n - 14);
            }
            assert_eq!(l.ranges[0][0], 0);
            assert_eq!(l.ranges[4][1], l.points.len());
        }
    }

    #[test]
    fn the_levels_do_not_cross_and_keep_their_order_from_land_to_sea() {
        let (_, levels, _) = island_levels(false);
        for l in &levels {
            let n = l.level_len();
            // At every point the five levels are in the order of their offsets along the normal
            // of the waterline: -20, -1.5, 0, 1.5, 20.
            for i in 2..n - 2 {
                let c = l.level(COASTLINE_LEVEL);
                let g = c[i + 1].pos - c[i - 1].pos;
                let normal = Vec2::new(g.y, -g.x).normalize();
                let along = |k: usize| (l.level(k)[i].pos - c[i].pos).dot(normal);
                let d: Vec<f32> = (0..5).map(along).collect();
                assert!(
                    d.windows(2).all(|w| w[0] < w[1]),
                    "point {i}: levels cross, offsets {d:?}"
                );
            }
        }
    }

    #[test]
    fn the_offsets_of_the_styles_are_the_distances_between_the_levels() {
        let (_, levels, _) = island_levels(false);
        let l = &levels[0];
        let c = l.level(COASTLINE_LEVEL);
        let i = l.level_len() / 3;
        let g = c[i + 1].pos - c[i - 1].pos;
        let normal = Vec2::new(g.y, -g.x).normalize();
        let along = |k: usize| (l.level(k)[i].pos - c[i].pos).dot(normal);
        // The styles of grass and shallows have the same offsets in the synthetic data.
        for (k, want) in [(0, -20.0), (1, -1.5), (2, 0.0), (3, 1.5), (4, 20.0)] {
            assert!(
                (along(k) - want).abs() < 0.5,
                "level {k}: {} vs {want}",
                along(k)
            );
        }
    }

    #[test]
    fn a_closed_line_has_its_wrap_copies_on_every_level() {
        let (splines, levels, _) = island_levels(false);
        for (s, l) in splines.iter().zip(&levels) {
            assert!(s.closed);
            for k in 0..5 {
                let p = l.level(k);
                // After the last cut the first and the last points still sit where the wrap puts them.
                let n = p.len();
                assert!((p[0].pos - p[n - 1].pos).length() < 5.0, "level {k}");
            }
        }
    }

    #[test]
    fn the_character_of_the_profile_moves_the_points() {
        let (_, plain, _) = island_levels(false);
        let (_, rough, _) = island_levels(true);
        let moved: f32 = plain[0]
            .points
            .iter()
            .zip(&rough[0].points)
            .map(|(a, b)| (a.pos - b.pos).length())
            .fold(0.0, f32::max);
        assert!(moved > 0.01, "{moved}");
        assert!(moved < 5.0, "{moved}");
    }

    #[test]
    fn terrains_are_looked_up_in_the_continent_first_then_in_the_generic_one() {
        // Continent 1 has the id 5, continent 0 has 16 only; the type 77 is nowhere.
        let mut rows = defs_with_ids(&[5]).rows().to_vec();
        rows.extend(
            defs_with_ids(&[16])
                .rows()
                .iter()
                .filter(|r| r.continent == 0)
                .cloned(),
        );
        let defs = TerrainDefs::new(rows);
        // Rows: 5 per continent 0..4 in this order, then the one extra.
        assert_eq!(resolve_terrain(&defs, 1, 5), 1);
        assert_eq!(resolve_terrain(&defs, 1, 16), 5);
        assert_eq!(resolve_terrain(&defs, 1, 77), NO_TERRAIN);
    }
}
