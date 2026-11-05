//! The strips of a border (`GenerateMajorCivMesh`, `GenerateMinorCivMesh`; RW-19).
//!
//! A civilization: along its spline, two strips side by side going inwards (the cross section of S3's
//! draws, `CultureBorders.xml`): the primary colour, a soft edge of `primary_aa_width` fading in, the
//! solid band of `primary_width` at `primary_alpha`, a soft edge of `secondary_aa_width` fading out;
//! and the second colour from the inner edge of the band, `secondary_alpha` fading to nothing over
//! `secondary_width`. All the primary strips come first in the index list, then the fills: the pass
//! draws with a depth test that lets the first triangle at a pixel win, so the fill does not tint the
//! band.
//!
//! A city state: a band of `cs_primary_width` at `cs_primary_alpha` between two black outlines of
//! `cs_outline_width` at `cs_outline_alpha`, with a soft edge of `cs_aa_width` outside each; the band
//! is cut into dashes with no gap between them, of the second colour and the primary in turn
//! (`CalculateDashWidth`: an even number of whole dashes of `cs_dash_width`, the rest shared out).
//!
//! Both cross sections lie inside the territory only, beginning [`DEAD_ZONE`] from the spline.
// re: GenerateMajorCivMesh 0x08a4b742, GenerateMinorCivMesh 0x08a4b080, CalculateDashWidth 0x08a4c36e
// (re-b12-item4b 1.4-1.6, 6.1-6.3; RE-B18 item 3 BRD-2, BRD-3)

use glam::Vec2;

use super::culture::{bspline, culture_lines, shape_points};
use super::smooth::Samples;
use super::{BorderStyle, BorderVertex, OwnerMesh};
use crate::hexgrid::GridShape;
use crate::world::overlay::OwnersInput;

/// One station along a spline: the point, the unit normal pointing inside, the cell.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Section {
    pub(crate) p: Vec2,
    pub(crate) n: Vec2,
    pub(crate) cell: u32,
}

/// One vertex of a cross section: how far along the normal, colour, alpha.
type Rung = (f32, [u8; 3], u8);

/// Nothing is drawn nearer the spline than this, on the inside.
// re: GenerateMajorCivMesh 0x08a4b742 (the primary ramps in from 0.5)
pub const DEAD_ZONE: f32 = 0.5;

/// The outline of a city state's band.
const BLACK: [u8; 3] = [0, 0, 0];

fn byte(alpha: f32) -> u8 {
    alpha.round().clamp(0.0, 255.0) as u8
}

pub(crate) fn sections(samples: &Samples, inward_left: bool) -> Vec<Section> {
    let n = samples.points.len();
    (0..n)
        .map(|i| {
            let (prev, next) = if samples.closed {
                (samples.points[(i + n - 1) % n], samples.points[(i + 1) % n])
            } else {
                (
                    samples.points[i.saturating_sub(1)],
                    samples.points[(i + 1).min(n - 1)],
                )
            };
            let t = (next - prev).normalize_or_zero();
            let normal = if inward_left {
                Vec2::new(-t.y, t.x)
            } else {
                Vec2::new(t.y, -t.x)
            };
            Section {
                p: samples.points[i],
                n: normal,
                cell: samples.cells[i],
            }
        })
        .collect()
}

/// A strip along `secs` with the rungs of `profile` across it, into `mesh`; the triangles go to
/// `indices`.
fn add_strip(
    mesh: &mut OwnerMesh,
    indices: &mut Vec<u32>,
    secs: &[Section],
    closed: bool,
    profile: &[Rung],
) {
    if secs.len() < 2 || profile.len() < 2 {
        return;
    }
    let base = mesh.vertices.len() as u32;
    let rungs = profile.len() as u32;
    for s in secs {
        for &(offset, rgb, alpha) in profile {
            let p = s.p + s.n * offset;
            mesh.vertices.push(BorderVertex {
                position: p.to_array(),
                color: [rgb[0], rgb[1], rgb[2], alpha],
            });
            mesh.cells.push(s.cell);
        }
    }
    let count = secs.len() as u32;
    let spans = if closed { count } else { count - 1 };
    for k in 0..spans {
        let k2 = (k + 1) % count;
        for j in 0..rungs - 1 {
            let a = base + k * rungs + j;
            let b = base + k2 * rungs + j;
            indices.extend_from_slice(&[a, b, b + 1, a, b + 1, a + 1]);
        }
    }
}

/// The section at arc length `s` of `secs` (a closed line closes with its first section).
fn section_at(secs: &[Section], cum: &[f32], closed: bool, s: f32) -> Section {
    let n = secs.len();
    let i = cum
        .partition_point(|&c| c <= s)
        .saturating_sub(1)
        .min(n - 1);
    let j = if closed {
        (i + 1) % n
    } else {
        (i + 1).min(n - 1)
    };
    let (a, b) = (secs[i], secs[j]);
    let span = if closed && i == n - 1 {
        cum[n] - cum[n - 1]
    } else {
        cum[(i + 1).min(n - 1)] - cum[i]
    };
    let t = if span > 1e-6 {
        ((s - cum[i]) / span).clamp(0.0, 1.0)
    } else {
        0.0
    };
    Section {
        p: a.p.lerp(b.p, t),
        n: a.n.lerp(b.n, t).normalize_or_zero(),
        cell: a.cell,
    }
}

/// `CalculateDashWidth`: `n` whole dashes of `dash` in `length`, made even by giving one back, the
/// rest shared among them. `None` when not two fit.
pub fn dash_width(length: f32, dash: f32) -> Option<f32> {
    let mut n = (length / dash).floor();
    let mut r = length - n * dash;
    if n as i64 % 2 == 1 {
        r += dash;
        n -= 1.0;
    }
    (n >= 2.0).then(|| dash + r / n)
}

/// The dashes of a line, end to end with no gap: runs of sections of `dash_width` of arc each.
fn dashes(secs: &[Section], closed: bool, dash: f32) -> Vec<Vec<Section>> {
    let n = secs.len();
    let mut cum = vec![0.0f32; n + 1];
    for i in 0..n {
        let j = (i + 1) % n;
        cum[i + 1] = cum[i] + secs[i].p.distance(secs[j].p);
    }
    let total = if closed { cum[n] } else { cum[n - 1] };
    if total <= 0.0 {
        return Vec::new();
    }
    let Some(d) = dash_width(total, dash) else {
        return vec![secs.to_vec()];
    };
    let count = (total / d).round() as usize;
    (0..count)
        .map(|k| {
            let (s0, s1) = (k as f32 * d, (k as f32 + 1.0) * d);
            let mut run = vec![section_at(secs, &cum, closed, s0)];
            run.extend(
                (0..n)
                    .filter(|&i| cum[i] > s0 && cum[i] < s1)
                    .map(|i| secs[i]),
            );
            run.push(section_at(secs, &cum, closed, s1.min(total - 1e-4)));
            run
        })
        .collect()
}

/// The mesh of one player.
pub fn owner_mesh(
    input: &OwnersInput,
    shape: &GridShape,
    owner: i8,
    style: &BorderStyle,
) -> OwnerMesh {
    let primary = input.primary_color(owner);
    let secondary = input.secondary_color(owner);
    let city_state = input.is_city_state(owner);
    let mut mesh = OwnerMesh::default();
    let mut bands: Vec<u32> = Vec::new();
    let mut fills: Vec<u32> = Vec::new();
    for line in culture_lines(input, shape, owner, style.corner_bias) {
        let shaped = shape_points(&line, style.expansion, style.randomness);
        let samples = bspline(&shaped, line.line.closed);
        if samples.points.len() < 2 {
            continue;
        }
        let secs = sections(&samples, line.line.inward_left);
        if city_state {
            let a = style.cs_aa_width;
            let rim = style.cs_outline_width;
            let (pa, oa) = (byte(style.cs_primary_alpha), byte(style.cs_outline_alpha));
            let (b0, b1) = (
                DEAD_ZONE + a + rim,
                DEAD_ZONE + a + rim + style.cs_primary_width,
            );
            let profile = |dash: [u8; 3]| -> [Rung; 8] {
                [
                    (DEAD_ZONE, BLACK, 0),
                    (b0 - rim, BLACK, oa),
                    (b0, BLACK, oa),
                    (b0, dash, pa),
                    (b1, dash, pa),
                    (b1, BLACK, oa),
                    (b1 + rim, BLACK, oa),
                    (b1 + rim + a, BLACK, 0),
                ]
            };
            // Dash 0 in the second colour, dash 1 in the primary, and so on.
            for (k, run) in dashes(&secs, samples.closed, style.cs_dash_width)
                .iter()
                .enumerate()
            {
                let dash = if k % 2 == 0 { secondary } else { primary };
                add_strip(&mut mesh, &mut bands, run, false, &profile(dash));
            }
        } else {
            // The middle of the solid band, measured inwards from the spline.
            let half = style.primary_width * 0.5;
            let mid = DEAD_ZONE + style.primary_aa_width + half;
            let band: [Rung; 4] = [
                (mid - half - style.primary_aa_width, primary, 0),
                (mid - half, primary, byte(style.primary_alpha)),
                (mid + half, primary, byte(style.primary_alpha)),
                (mid + half + style.secondary_aa_width, primary, 0),
            ];
            let fill: [Rung; 3] = [
                (mid + half - style.secondary_aa_width, secondary, 0),
                (mid + half, secondary, byte(style.secondary_alpha)),
                (mid + half + style.secondary_width, secondary, 0),
            ];
            add_strip(&mut mesh, &mut bands, &secs, samples.closed, &band);
            add_strip(&mut mesh, &mut fills, &secs, samples.closed, &fill);
        }
    }
    mesh.indices = bands;
    mesh.indices.extend(fills);
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blob(minor: bool) -> (OwnersInput, GridShape) {
        let owners = (0..7 * 7)
            .map(|i| {
                let (x, y) = (i % 7, i / 7);
                if (2..5).contains(&x) && (2..5).contains(&y) {
                    0
                } else {
                    -1
                }
            })
            .collect();
        let mut o = OwnersInput::new(7, 7, owners, vec![[176, 7, 3]]).unwrap();
        o = o.with_secondary_colors(vec![[245, 230, 55]]);
        if minor {
            o = o.with_city_states(vec![true]);
        }
        (
            o,
            GridShape {
                width: 7,
                height: 7,
                wrap_x: false,
            },
        )
    }

    #[test]
    fn a_civilization_has_the_cross_section_of_the_capture() {
        let (inp, shape) = blob(false);
        let m = owner_mesh(&inp, &shape, 0, &BorderStyle::default());
        assert!(!m.is_empty());
        assert!(m.indices.len().is_multiple_of(3));
        assert_eq!(m.cells.len(), m.vertices.len());
        assert!(m.indices.iter().all(|&i| (i as usize) < m.vertices.len()));
        // The alphas of a section: 0, 200, 200, 0 in the player's colour and 0, 90, 0 in the second.
        let primary: Vec<u8> = m
            .vertices
            .iter()
            .filter(|v| v.color[..3] == [176, 7, 3])
            .map(|v| v.color[3])
            .collect();
        assert!(primary.iter().all(|&a| matches!(a, 0 | 200)));
        assert_eq!(
            primary.iter().filter(|&&a| a == 200).count(),
            primary.len() / 2
        );
        let fill: Vec<u8> = m
            .vertices
            .iter()
            .filter(|v| v.color[..3] == [245, 230, 55])
            .map(|v| v.color[3])
            .collect();
        assert!(fill.iter().all(|&a| matches!(a, 0 | 90)));
        assert_eq!(fill.iter().filter(|&&a| a == 90).count(), fill.len() / 3);
    }

    #[test]
    fn the_widths_across_a_section_are_the_files() {
        let (inp, shape) = blob(false);
        let m = owner_mesh(&inp, &shape, 0, &BorderStyle::default());
        // The first section of the primary strip: four vertices in a row across the line.
        let v: Vec<Vec2> = m.vertices[..4]
            .iter()
            .map(|v| Vec2::from(v.position))
            .collect();
        let d = |a: usize, b: usize| v[a].distance(v[b]);
        assert!((d(0, 1) - 0.5).abs() < 1e-3, "soft edge");
        assert!((d(1, 2) - 1.4).abs() < 1e-3, "solid band");
        assert!((d(2, 3) - 0.3).abs() < 1e-3, "inner soft edge");
        // The fill's far end is `secondary_width` from its start, along the same direction.
        let fill_start = m
            .vertices
            .iter()
            .position(|v| v.color[..3] == [245, 230, 55])
            .unwrap();
        let f0 = Vec2::from(m.vertices[fill_start + 1].position);
        let f1 = Vec2::from(m.vertices[fill_start + 2].position);
        assert!((f0.distance(f1) - 20.0).abs() < 1e-2);
    }

    #[test]
    fn the_bands_come_before_the_fills_in_the_index_list() {
        let (inp, shape) = blob(false);
        let m = owner_mesh(&inp, &shape, 0, &BorderStyle::default());
        let is_fill = |i: u32| m.vertices[i as usize].color[..3] == [245, 230, 55];
        let first_fill = m
            .indices
            .iter()
            .position(|&i| is_fill(i))
            .expect("there is a fill");
        assert!(m.indices[..first_fill].iter().all(|&i| !is_fill(i)));
        assert!(m.indices[first_fill..].iter().all(|&i| is_fill(i)));
    }

    #[test]
    fn the_line_lies_inside_the_owners_hexes() {
        let (inp, shape) = blob(false);
        let m = owner_mesh(&inp, &shape, 0, &BorderStyle::default());
        // The solid band's vertices are at most a few units from the blob's outline: inside the
        // hexes of the owner (the soft fill goes 20 further in).
        for (i, v) in m.vertices.iter().enumerate() {
            if v.color[..3] != [176, 7, 3] || v.color[3] != 200 {
                continue;
            }
            let cell = m.cells[i];
            let c = crate::hexgrid::cell_center(cell as i32 % 7, cell as i32 / 7);
            let d = (Vec2::from(v.position) - c).length();
            assert!(d < 1.2 * crate::hexgrid::HEX_WIDTH, "{d}");
        }
    }

    #[test]
    fn a_city_state_is_a_band_of_dashes_of_both_colours_between_black_outlines() {
        let (inp, shape) = blob(true);
        let minor = owner_mesh(&inp, &shape, 0, &BorderStyle::default());
        assert!(!minor.is_empty());
        // No fill, no alpha over the outline's.
        assert!(minor.vertices.iter().all(|v| v.color[3] <= 160));
        let alphas: std::collections::BTreeSet<u8> =
            minor.vertices.iter().map(|v| v.color[3]).collect();
        assert_eq!(alphas.into_iter().collect::<Vec<_>>(), vec![0, 130, 160]);
        // The outline is black; the dashes are of the primary and the second colour, both.
        assert!(
            minor
                .vertices
                .iter()
                .filter(|v| v.color[3] == 160)
                .all(|v| v.color[..3] == BLACK)
        );
        let dash_colours: std::collections::BTreeSet<[u8; 3]> = minor
            .vertices
            .iter()
            .filter(|v| v.color[3] == 130)
            .map(|v| [v.color[0], v.color[1], v.color[2]])
            .collect();
        assert_eq!(dash_colours.len(), 2);
    }

    #[test]
    fn the_dashes_are_an_even_number_of_whole_dashes_with_the_rest_shared() {
        // 50 = 6 dashes of 8 and 2 left: 6 dashes of 8 + 2/6.
        assert!((dash_width(50.0, 8.0).unwrap() - (8.0 + 2.0 / 6.0)).abs() < 1e-5);
        // 133 = 16 of 8 and 5 left.
        assert!((dash_width(133.0, 8.0).unwrap() - (8.0 + 5.0 / 16.0)).abs() < 1e-5);
        // 60 = 7 of 8: one is given back, 6 of 8 + 12/6.
        assert!((dash_width(60.0, 8.0).unwrap() - 10.0).abs() < 1e-5);
        assert_eq!(dash_width(12.0, 8.0), None);
    }

    #[test]
    fn the_band_begins_half_a_unit_inside_the_spline() {
        let (inp, shape) = blob(false);
        let style = BorderStyle::default();
        let m = owner_mesh(&inp, &shape, 0, &style);
        // The first section: its outer rung is `DEAD_ZONE` along the normal from the spline point,
        // so the four rungs lie 0.5, 1.0, 2.4 and 2.7 from the curve.
        let v: Vec<Vec2> = m.vertices[..4]
            .iter()
            .map(|v| Vec2::from(v.position))
            .collect();
        let line = &culture_lines(&inp, &shape, 0, style.corner_bias)[0];
        let shaped = shape_points(line, style.expansion, style.randomness);
        let s = bspline(&shaped, line.line.closed);
        assert!((v[0].distance(s.points[0]) - DEAD_ZONE).abs() < 1e-3);
        assert!((v[3].distance(s.points[0]) - 2.7).abs() < 1e-3);
    }
    #[test]
    fn the_same_cells_give_the_same_mesh() {
        let (inp, shape) = blob(false);
        let a = owner_mesh(&inp, &shape, 0, &BorderStyle::default());
        let b = owner_mesh(&inp, &shape, 0, &BorderStyle::default());
        assert_eq!(a, b);
    }
}
