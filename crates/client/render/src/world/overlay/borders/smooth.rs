//! The Catmull-Rom spline of the highlight borders (the culture borders take the B-spline of
//! [`super::culture`]).
//!
//! The control points go through a Catmull-Rom spline, cut into steps of about [`SAMPLE_STEP`]
//! (the capture S3 has the samples 3.4 to 4.9 apart, ten to an edge of a hex). The result is a line
//! of samples, each with the cell it belongs to.

use glam::Vec2;

use super::points::ControlPoint;

/// Length of a step of the spline, world units.
// re: the draws of S3 (`VSOverlayCountry`): the sections of a border lie 3.4 to 4.9 apart
pub const SAMPLE_STEP: f32 = 3.7;

/// The samples of a border line.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Samples {
    pub points: Vec<Vec2>,
    pub cells: Vec<u32>,
    pub closed: bool,
}

impl Samples {
    /// Length of the line (the closing step included for a closed one).
    pub fn length(&self) -> f32 {
        let n = self.points.len();
        if n < 2 {
            return 0.0;
        }
        let steps = if self.closed { n } else { n - 1 };
        (0..steps)
            .map(|i| self.points[i].distance(self.points[(i + 1) % n]))
            .sum()
    }
}

fn catmull_rom(p0: Vec2, p1: Vec2, p2: Vec2, p3: Vec2, t: f32) -> Vec2 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * ((2.0 * p1)
        + (p2 - p0) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
        + (3.0 * p1 - 3.0 * p2 + p3 - p0) * t3)
}

/// The spline through the points of a line.
pub fn smooth(points: &[ControlPoint], closed: bool) -> Samples {
    let n = points.len();
    let mut out = Samples {
        closed,
        ..Samples::default()
    };
    if n == 0 {
        return out;
    }
    if n == 1 {
        out.points.push(points[0].position);
        out.cells.push(points[0].cell);
        return out;
    }
    let at = |i: isize| -> Vec2 {
        if closed {
            points[i.rem_euclid(n as isize) as usize].position
        } else {
            points[i.clamp(0, n as isize - 1) as usize].position
        }
    };
    let spans = if closed { n } else { n - 1 };
    for i in 0..spans as isize {
        let (p0, p1, p2, p3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
        let steps = ((p1.distance(p2) / SAMPLE_STEP).round() as usize).max(1);
        let cell = points[i as usize].cell;
        for s in 0..steps {
            out.points
                .push(catmull_rom(p0, p1, p2, p3, s as f32 / steps as f32));
            out.cells.push(cell);
        }
    }
    if !closed {
        out.points.push(points[n - 1].position);
        out.cells.push(points[n - 1].cell);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cp(x: f32, y: f32, key: u32) -> ControlPoint {
        ControlPoint {
            position: Vec2::new(x, y),
            cell: key,
            key,
        }
    }

    #[test]
    fn the_spline_passes_through_the_control_points_in_steps_of_about_the_step() {
        let pts: Vec<ControlPoint> = [(0.0, 0.0), (20.0, 0.0), (40.0, 5.0), (60.0, 5.0)]
            .iter()
            .enumerate()
            .map(|(i, &(x, y))| cp(x, y, i as u32))
            .collect();
        let s = smooth(&pts, false);
        assert_eq!(s.points.first(), Some(&Vec2::new(0.0, 0.0)));
        assert_eq!(s.points.last(), Some(&Vec2::new(60.0, 5.0)));
        for p in &pts {
            assert!(s.points.iter().any(|q| q.distance(p.position) < 1e-3));
        }
        for w in s.points.windows(2) {
            let d = w[0].distance(w[1]);
            assert!(d > 1.5 && d < 6.0, "{d}");
        }
        assert_eq!(s.points.len(), s.cells.len());
    }

    #[test]
    fn a_closed_line_has_no_repeated_end_and_a_length() {
        let pts: Vec<ControlPoint> = [(0.0, 0.0), (30.0, 0.0), (30.0, 30.0), (0.0, 30.0)]
            .iter()
            .enumerate()
            .map(|(i, &(x, y))| cp(x, y, i as u32))
            .collect();
        let s = smooth(&pts, true);
        assert_ne!(s.points.first(), s.points.last());
        let l = s.length();
        assert!(l > 100.0 && l < 140.0, "{l}");
    }
}
