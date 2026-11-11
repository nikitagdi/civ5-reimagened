//! The inline layout of one inline formatting root: its runs, its inline boxes, line boxes, `text-align`,
//! `text-overflow: ellipsis` and the rectangles of its pieces.
//!
//! Line boxes follow CSS: every inline box on a line (the root's strut, each inline element, each run) sits
//! on the baseline with its half-leading (`floor((L - (ascent + descent)) / 2)` above the content area, the
//! rule of the text spike); the line box spans the highest top to the lowest bottom.
//!
//! A text leaf gives one run per piece of its text: the game's markup (`[COLOR_X]...[ENDCOLOR]`) splits a
//! string into pieces of different colors, `[NEWLINE]` is a line feed inside a piece. Runs are never shaped
//! across a piece or an element boundary (a ligature or kerning pair across a `span` edge is lost, as the
//! profile accepts).

use ui2_package::style::{ColorV, TextAlign};
use ui2_text::{
    BreakOptions, Line, LineHeight, LineMetrics, LineSegment, Lines, ShapedRun, TextMeasure,
};

use crate::arena::NodeId;

/// Vertical metrics of an inline box: the font's metrics and the used line height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VMetrics {
    /// Ascent, descent and gap of the box's first available font.
    pub metrics: LineMetrics,
    /// `line-height` in px.
    pub line_height: f32,
}

impl VMetrics {
    pub(crate) const ZERO: VMetrics = VMetrics {
        metrics: LineMetrics {
            ascent: 0.0,
            descent: 0.0,
            line_gap: 0.0,
        },
        line_height: 0.0,
    };

    /// Space above the baseline: the top half-leading (floored) plus the ascent.
    pub fn above(self) -> f32 {
        self.metrics.baseline_in(self.line_height)
    }

    /// Space below the baseline.
    pub fn below(self) -> f32 {
        self.line_height - self.above()
    }
}

/// An inline element inside a root.
#[derive(Debug, Clone)]
pub struct InlineBox {
    /// The element.
    pub node: NodeId,
    /// Its metrics.
    pub vm: VMetrics,
}

/// One piece of a text leaf.
#[derive(Debug, Clone)]
pub struct Run {
    /// The text leaf.
    pub node: NodeId,
    /// The index of the piece in the leaf's text (markup splits a text into pieces).
    pub piece: u16,
    /// The color of the piece when markup gives one (otherwise the color of the leaf's element).
    pub color: Option<ColorV>,
    /// The shaped text.
    pub shaped: ShapedRun,
    /// The metrics of the leaf (its parent's font and line height).
    pub vm: VMetrics,
    /// The inline elements around the leaf, innermost first (indices into [`Ifc::boxes`]).
    pub boxes: Vec<usize>,
}

/// The text of a root shrunk to fit its box (`data-fit="shrink"`).
#[derive(Debug, Clone)]
pub struct Fitted {
    /// The runs at the smaller size.
    pub runs: Vec<Run>,
    /// Their lines in the box.
    pub lines: Lines,
    /// The factor applied to every font size.
    pub scale: f32,
}

/// The inline layout of a root.
#[derive(Debug, Clone)]
pub struct Ifc {
    /// Text pieces in document order.
    pub runs: Vec<Run>,
    /// Inline elements in document order.
    pub boxes: Vec<InlineBox>,
    /// The root's own font and line height (the strut every line starts with).
    pub strut: VMetrics,
    /// `white-space` allows wrapping.
    pub wrap: bool,
    /// `text-align` of the root.
    pub align: TextAlign,
    /// `text-overflow: ellipsis` applies (one line, clipped): the shaped `…` in the style of the last run.
    pub ellipsis: Option<Run>,
    /// `data-fit="shrink"`: the smallest font size in px (`None`: no limit).
    pub fit: Option<Option<f32>>,
    /// The root's font size (fitting scales it down to `fit`).
    pub font_size: f32,
    /// The lines at the width of the last full layout.
    pub lines: Lines,
    /// The content width of the last full layout (the width `lines` were broken at).
    pub width: f32,
    /// The text shrunk to fit, when it did not fit at its size.
    pub fitted: Option<Box<Fitted>>,
    /// The content width and natural width the fitting was done for.
    pub(crate) fit_key: Option<(f32, f32)>,
    /// A flex container whose only content is text: the box of the anonymous flex item that holds the text
    /// (as Chrome makes one), placed by the container's alignment. `None` for other roots: the lines fill
    /// the content box.
    pub item: Option<AnonItem>,
}

/// The anonymous flex item around the text of a flex container, relative to its content box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnonItem {
    /// Left.
    pub x: f32,
    /// Top.
    pub y: f32,
    /// Width (`text-align` works inside it).
    pub w: f32,
}

/// What paint and dumps show of a root: the runs and their lines (shrunk ones when the text was fitted).
#[derive(Debug, Clone, Copy)]
pub struct Shown<'a> {
    /// The runs.
    pub runs: &'a [Run],
    /// The ellipsis run, index `runs.len()` in line segments.
    pub ellipsis: Option<&'a Run>,
    /// The lines.
    pub lines: &'a Lines,
}

impl<'a> Shown<'a> {
    /// The run a line segment refers to.
    pub fn run(&self, i: usize) -> Option<&'a Run> {
        if i == self.runs.len() {
            return self.ellipsis;
        }
        self.runs.get(i)
    }
}

/// Builds line boxes over broken lines: heights and baselines from the strut, the runs and their boxes.
fn line_boxes(
    lines: &mut Lines,
    strut: VMetrics,
    runs: &[Run],
    boxes: &[InlineBox],
    ell: Option<&Run>,
) {
    let mut top = 0.0;
    for line in &mut lines.lines {
        let (mut above, mut below) = (strut.above(), strut.below());
        let mut take = |vm: VMetrics| {
            above = above.max(vm.above());
            below = below.max(vm.below());
        };
        for seg in &line.segments {
            let run = if seg.run == runs.len() {
                ell
            } else {
                runs.get(seg.run)
            };
            if let Some(run) = run {
                take(run.vm);
                for &b in &run.boxes {
                    if let Some(b) = boxes.get(b) {
                        take(b.vm);
                    }
                }
            }
        }
        line.top = top;
        line.height = above + below;
        line.baseline = above;
        top += line.height;
    }
    lines.height = top;
}

/// Cuts a line that is wider than `width` so that it ends with the ellipsis: the longest prefix (by
/// clusters) that leaves room for `…`.
fn ellipsize(line: &mut Line, runs: &[Run], ell: &Run, width: f32) {
    let budget = width - ell.shaped.width;
    let mut keep: Vec<LineSegment> = Vec::new();
    let mut end_x = 0.0f32;
    for seg in &line.segments {
        if seg.x + seg.width <= budget {
            keep.push(*seg);
            end_x = seg.x + seg.width;
            continue;
        }
        if let Some(run) = runs.get(seg.run) {
            let shaped = &run.shaped;
            let mut best = seg.start;
            let mut best_w = 0.0;
            for b in shaped.cluster_starts() {
                if b <= seg.start || b > seg.end {
                    continue;
                }
                let w = shaped.range_width(seg.start, b);
                if seg.x + w > budget {
                    break;
                }
                best = b;
                best_w = w;
            }
            if best > seg.start {
                keep.push(LineSegment {
                    end: best,
                    width: best_w,
                    ..*seg
                });
                end_x = seg.x + best_w;
            }
        }
        break;
    }
    keep.push(LineSegment {
        run: runs.len(),
        start: 0,
        end: ell.shaped.text.len(),
        x: end_x,
        run_x: 0.0,
        width: ell.shaped.width,
    });
    line.segments = keep;
    line.width = end_x + ell.shaped.width;
    line.hanging = 0.0;
}

impl Ifc {
    /// Breaks runs at a width and builds the line boxes (ellipsis included).
    pub(crate) fn break_runs(
        &self,
        tm: &dyn TextMeasure,
        runs: &[Run],
        ell: Option<&Run>,
        width: f32,
    ) -> Lines {
        if runs.is_empty() {
            return Lines::default();
        }
        let shaped: Vec<ShapedRun> = runs.iter().map(|r| r.shaped.clone()).collect();
        let opts = BreakOptions {
            max_width: width,
            wrap: self.wrap,
            line_height: LineHeight::Px(0.0),
        };
        let mut lines = tm.break_lines(&shaped, &opts);
        if let Some(e) = ell {
            for line in &mut lines.lines {
                if line.width > width + 1e-3 {
                    ellipsize(line, runs, e, width);
                }
            }
            lines.width = lines.lines.iter().map(|l| l.width).fold(0.0, f32::max);
        }
        line_boxes(&mut lines, self.strut, runs, &self.boxes, ell);
        lines
    }

    /// Breaks the runs at a width and builds the line boxes.
    pub fn lines_at(&self, tm: &dyn TextMeasure, width: f32) -> Lines {
        self.break_runs(tm, &self.runs, self.ellipsis.as_ref(), width)
    }

    /// What is shown: the fitted text when there is one.
    pub fn shown(&self) -> Shown<'_> {
        match &self.fitted {
            Some(f) => Shown {
                runs: &f.runs,
                ellipsis: None,
                lines: &f.lines,
            },
            None => Shown {
                runs: &self.runs,
                ellipsis: self.ellipsis.as_ref(),
                lines: &self.lines,
            },
        }
    }

    /// The x of a line inside a content box of `width` (`text-align`).
    pub fn line_x(&self, line: &Line, width: f32) -> f32 {
        let (x0, width) = match self.item {
            Some(a) => (a.x, a.w),
            None => (0.0, width),
        };
        let free = (width - line.width).max(0.0);
        x0 + match self.align {
            TextAlign::Start | TextAlign::Left => 0.0,
            TextAlign::Center => free / 2.0,
            TextAlign::End | TextAlign::Right => free,
        }
    }

    /// The top of the lines in the content box (below the top when an anonymous flex item is aligned down).
    pub fn lines_y(&self) -> f32 {
        self.item.map_or(0.0, |a| a.y)
    }

    /// The first baseline from the top of the content box.
    pub fn first_baseline(lines: &Lines) -> Option<f32> {
        lines.lines.first().map(|l| l.top + l.baseline)
    }

    /// The rectangles of the text leaves and inline elements (the content areas Chrome reports for a text
    /// range and an inline element), in a content box of `width`.
    pub fn rects(&self, width: f32) -> Vec<InlineRect> {
        let mut out: Vec<InlineRect> = Vec::new();
        let mut union = |node: NodeId, x: f32, y: f32, w: f32, h: f32| {
            if let Some(r) = out.iter_mut().find(|r| r.node == node) {
                let x1 = (r.x + r.w).max(x + w);
                let y1 = (r.y + r.h).max(y + h);
                r.x = r.x.min(x);
                r.y = r.y.min(y);
                r.w = x1 - r.x;
                r.h = y1 - r.y;
            } else {
                out.push(InlineRect { node, x, y, w, h });
            }
        };
        let shown = self.shown();
        for line in &shown.lines.lines {
            let lx = self.line_x(line, width);
            let base = self.lines_y() + line.top + line.baseline;
            for seg in &line.segments {
                let Some(run) = shown.run(seg.run) else {
                    continue;
                };
                let x = lx + seg.x;
                let m = run.vm.metrics;
                union(run.node, x, base - m.ascent, seg.width, m.content_height());
                for &b in &run.boxes {
                    if let Some(b) = self.boxes.get(b) {
                        let m = b.vm.metrics;
                        union(b.node, x, base - m.ascent, seg.width, m.content_height());
                    }
                }
            }
        }
        out
    }
}

/// A rectangle relative to the content box of a root.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InlineRect {
    /// The text leaf or inline element.
    pub node: NodeId,
    /// Left.
    pub x: f32,
    /// Top.
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn half_leading_floors_the_top() {
        let vm = VMetrics {
            metrics: LineMetrics {
                ascent: 12.0,
                descent: 3.0,
                line_gap: 0.0,
            },
            line_height: 18.0,
        };
        assert_eq!(vm.above(), 13.0);
        assert_eq!(vm.below(), 5.0);
    }
}
