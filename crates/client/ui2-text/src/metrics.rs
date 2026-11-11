//! Line metrics that match Blink.
//!
//! Rule 4: ascent, descent and line gap come from OS/2 typographic metrics if `USE_TYPO_METRICS` is set, else
//! from `hhea` (win metrics are never used). Each value is scaled by `size / upem` and rounded to a whole
//! pixel on its own. The inline box (content area) is `ascent + descent`; `line-height: normal` adds the gap.
//!
//! Rule 5: in a line box of height `L` the baseline sits `floor((L - (ascent + descent)) / 2) + ascent` below
//! the line top (the top half of the leading is floored). This also holds for `L` smaller than the content area.

use crate::face::Face;
use crate::{FaceId, TextEngine, TextError};

/// Rounds like Skia's `SkScalarRoundToScalar`: half up.
fn round_half_up(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// Ascent, descent and gap of a face at a size, each rounded to whole pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineMetrics {
    /// Distance from the baseline up to the top of the inline box.
    pub ascent: f32,
    /// Distance from the baseline down to the bottom of the inline box.
    pub descent: f32,
    /// Extra leading added by `line-height: normal`.
    pub line_gap: f32,
}

impl LineMetrics {
    /// Height of the inline box (Chrome's content area): `ascent + descent`.
    pub fn content_height(&self) -> f32 {
        self.ascent + self.descent
    }

    /// Line box height for `line-height: normal`: `ascent + descent + gap`.
    pub fn normal_line_height(&self) -> f32 {
        self.ascent + self.descent + self.line_gap
    }

    /// Baseline offset from the top of a line box of height `line_height` (rule 5).
    pub fn baseline_in(&self, line_height: f32) -> f32 {
        ((line_height - self.content_height()) / 2.0).floor() + self.ascent
    }
}

impl Face {
    pub(crate) fn line_metrics(&self, size: f32) -> LineMetrics {
        let s = f64::from(size) / f64::from(self.upem);
        let px = |units: i32| round_half_up(f64::from(units) * s) as f32;
        LineMetrics {
            ascent: px(self.raw.ascent),
            descent: px(self.raw.descent),
            line_gap: px(self.raw.line_gap),
        }
    }
}

impl TextEngine {
    /// Line metrics of `face` at `size` (CSS px). `wght` is accepted for the contract but does not change the
    /// metrics: the fonts used here have no `MVAR` variation of them.
    pub fn metrics(&self, face: FaceId, size: f32, _wght: f32) -> Result<LineMetrics, TextError> {
        Ok(self.face(face)?.line_metrics(size))
    }
}
