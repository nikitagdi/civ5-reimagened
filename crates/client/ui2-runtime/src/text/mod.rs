//! Text in the tree: font selection, inline formatting roots, line boxes.
//!
//! An *inline formatting root* is an element whose children are all inline-level: text leaves and inline
//! elements (`span`, `b`, ... with `display: inline`) holding text. It is one leaf for taffy; its content is
//! laid out here, on `ui2-text`. A text leaf that has block siblings (an anonymous block in CSS) is its own
//! root. Each root keeps an [`Ifc`]: one shaped run per text leaf (runs are never shaped across an element
//! boundary), the vertical metrics of every inline box, and the lines at the width layout gave it.
//!
//! Line boxes follow CSS: every inline box on a line (the root's strut, each inline element, each run) sits
//! on the baseline with its half-leading (`floor((L - (ascent + descent)) / 2)` above the content area, the
//! rule of the text spike); the line box spans the highest top to the lowest bottom.
//!
//! Shaping happens in the text pass (TEXT dirty nodes only); layout only breaks shaped runs into lines.

use std::sync::Arc;

use ui2_package::Package;
use ui2_package::style::{
    Display, FontStyle, LenNormal, LineHeight as CssLineHeight, StyleRecord, TextAlign,
    TextWrapMode,
};
use ui2_text::{
    BreakOptions, FaceId, Line, LineHeight, LineMetrics, Lines, NumericFigure, NumericSpacing,
    NumericVariant, RunStyle, ShapedRun, TextEngine, TextMeasure,
};

use crate::LoadError;
use crate::arena::{Arena, Dirty, NodeClass, NodeId};
use crate::error::{ErrorCode, ErrorSink, UiError};
use crate::instantiate;
use crate::style::StyleIndex;

/// The package's font faces, for selection by family, weight and style. `FaceId(i)` is face `i` of the
/// package (the ids [`load_faces`] registers).
#[derive(Debug, Clone, Default)]
pub struct FontBook {
    faces: Vec<(String, u16, bool)>,
}

/// CSS font matching for weights: the face of `want` if there is one, else the nearest by the CSS rule.
fn pick_weight(want: u16, have: &[u16]) -> Option<u16> {
    if have.contains(&want) {
        return Some(want);
    }
    let below = || have.iter().copied().filter(|&w| w < want).max();
    let above = |limit: u16| {
        have.iter()
            .copied()
            .filter(|&w| w > want && w <= limit)
            .min()
    };
    match want {
        400..=500 => above(500)
            .or_else(below)
            .or_else(|| have.iter().copied().filter(|&w| w > 500).min()),
        0..=399 => below().or_else(|| above(u16::MAX)),
        _ => above(u16::MAX).or_else(below),
    }
}

impl FontBook {
    /// The faces of a package.
    pub fn new(pkg: &Package) -> Self {
        Self {
            faces: pkg
                .fonts()
                .iter()
                .map(|f| {
                    (
                        pkg.str(f.family).unwrap_or("").to_string(),
                        f.weight,
                        f.italic,
                    )
                })
                .collect(),
        }
    }

    /// The face for a family list (first family the package has), a weight and a style.
    pub fn select<'a>(
        &self,
        families: impl IntoIterator<Item = &'a str>,
        weight: u16,
        italic: bool,
    ) -> Option<FaceId> {
        for family in families {
            let of_family: Vec<usize> = (0..self.faces.len())
                .filter(|&i| self.faces[i].0 == family)
                .collect();
            if of_family.is_empty() {
                continue;
            }
            let styled: Vec<usize> = of_family
                .iter()
                .copied()
                .filter(|&i| self.faces[i].2 == italic)
                .collect();
            let pool = if styled.is_empty() { of_family } else { styled };
            let weights: Vec<u16> = pool.iter().map(|&i| self.faces[i].1).collect();
            let w = pick_weight(weight, &weights)?;
            return pool
                .into_iter()
                .find(|&i| self.faces[i].1 == w)
                .map(|i| FaceId(i as u32));
        }
        None
    }
}

/// Registers every font face of a package with a text engine under the ids [`FontBook`] uses.
pub fn load_faces(pkg: &Package, engine: &mut TextEngine) -> Result<(), LoadError> {
    for (i, face) in pkg.fonts().iter().enumerate() {
        let data = pkg
            .font_data(face)
            .map_err(|e| LoadError::Fatal(format!("font {i}: {e}")))?;
        engine
            .load_face(FaceId(i as u32), data)
            .map_err(|e| LoadError::Fatal(format!("font {i}: {e}")))?;
    }
    Ok(())
}

/// Vertical metrics of an inline box: the font's metrics and the used line height.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VMetrics {
    /// Ascent, descent and gap of the box's first available font.
    pub metrics: LineMetrics,
    /// `line-height` in px.
    pub line_height: f32,
}

impl VMetrics {
    const ZERO: VMetrics = VMetrics {
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

/// One text leaf of a root.
#[derive(Debug, Clone)]
pub struct Run {
    /// The text leaf.
    pub node: NodeId,
    /// The shaped text.
    pub shaped: ShapedRun,
    /// The metrics of the leaf (its parent's font and line height).
    pub vm: VMetrics,
    /// The inline elements around the leaf, innermost first (indices into [`Ifc::boxes`]).
    pub boxes: Vec<usize>,
}

/// The inline layout of a root.
#[derive(Debug, Clone)]
pub struct Ifc {
    /// Text leaves in document order.
    pub runs: Vec<Run>,
    /// Inline elements in document order.
    pub boxes: Vec<InlineBox>,
    /// The root's own font and line height (the strut every line starts with).
    pub strut: VMetrics,
    /// `white-space` allows wrapping.
    pub wrap: bool,
    /// `text-align` of the root.
    pub align: TextAlign,
    /// The lines at the width of the last full layout.
    pub lines: Lines,
}

impl Ifc {
    /// Breaks the runs at a width and builds the line boxes.
    pub fn lines_at(&self, tm: &dyn TextMeasure, width: f32) -> Lines {
        if self.runs.is_empty() {
            return Lines::default();
        }
        let shaped: Vec<ShapedRun> = self.runs.iter().map(|r| r.shaped.clone()).collect();
        let opts = BreakOptions {
            max_width: width,
            wrap: self.wrap,
            line_height: LineHeight::Px(0.0),
        };
        let mut lines = tm.break_lines(&shaped, &opts);
        let mut top = 0.0;
        for line in &mut lines.lines {
            let (mut above, mut below) = (self.strut.above(), self.strut.below());
            let mut take = |vm: VMetrics| {
                above = above.max(vm.above());
                below = below.max(vm.below());
            };
            for seg in &line.segments {
                if let Some(run) = self.runs.get(seg.run) {
                    take(run.vm);
                    for &b in &run.boxes {
                        if let Some(b) = self.boxes.get(b) {
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
        lines
    }

    /// The x of a line inside a content box of `width` (`text-align`).
    pub fn line_x(&self, line: &Line, width: f32) -> f32 {
        let free = (width - line.width).max(0.0);
        match self.align {
            TextAlign::Start | TextAlign::Left => 0.0,
            TextAlign::Center => free / 2.0,
            TextAlign::End | TextAlign::Right => free,
        }
    }

    /// The first baseline from the top of the content box.
    pub fn first_baseline(lines: &Lines) -> Option<f32> {
        lines.lines.first().map(|l| l.top + l.baseline)
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

impl Ifc {
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
        for line in &self.lines.lines {
            let lx = self.line_x(line, width);
            let base = line.top + line.baseline;
            for seg in &line.segments {
                let Some(run) = self.runs.get(seg.run) else {
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

/// What the text pass reads.
pub(crate) struct TextCtx<'a> {
    pub pkg: &'a Package,
    pub fonts: &'a FontBook,
    pub index: &'a StyleIndex,
}

impl TextCtx<'_> {
    fn record(&self, arena: &Arena, id: NodeId) -> Option<&StyleRecord> {
        let n = arena.get(id)?;
        Some(self.index.record(self.pkg.styles(), n.style()))
    }

    fn is_inline_element(&self, arena: &Arena, id: NodeId) -> bool {
        arena
            .get(id)
            .is_some_and(|n| n.class() == NodeClass::Element)
            && self
                .record(arena, id)
                .is_some_and(|r| r.display == Display::Inline)
    }

    /// Whether every child is inline-level content (text leaves, inline elements holding only that).
    fn inline_content(&self, arena: &Arena, id: NodeId) -> bool {
        let Some(n) = arena.get(id) else {
            return false;
        };
        !n.children().is_empty()
            && n.children()
                .iter()
                .all(|&c| match arena.get(c).map(|c| c.class()) {
                    Some(NodeClass::Text) => true,
                    Some(NodeClass::Element) => {
                        self.is_inline_element(arena, c) && self.inline_content(arena, c)
                    }
                    _ => false,
                })
    }

    /// The root a text leaf belongs to.
    fn root_of(&self, arena: &Arena, text: NodeId) -> NodeId {
        let mut at = arena.get(text).and_then(|n| n.parent());
        while let Some(p) = at {
            if !self.is_inline_element(arena, p) {
                break;
            }
            at = arena.get(p).and_then(|n| n.parent());
        }
        match at {
            Some(c) if arena.get(c).is_some_and(|n| n.class().is_element()) => {
                if self.inline_content(arena, c) {
                    c
                } else {
                    text
                }
            }
            _ => text,
        }
    }

    fn vmetrics(
        &self,
        tm: &dyn TextMeasure,
        r: &StyleRecord,
    ) -> Option<(FaceId, RunStyle, VMetrics)> {
        let names: Vec<&str> = r
            .font_family
            .iter()
            .filter_map(|&s| self.pkg.str(s))
            .collect();
        let face = self
            .fonts
            .select(names, r.font_weight, r.font_style == FontStyle::Italic)?;
        let size = r.font_size.px;
        let numeric = NumericVariant {
            figure: if r.font_variant_numeric.lining {
                NumericFigure::Lining
            } else if r.font_variant_numeric.oldstyle {
                NumericFigure::Oldstyle
            } else {
                NumericFigure::Normal
            },
            spacing: if r.font_variant_numeric.tabular {
                NumericSpacing::Tabular
            } else if r.font_variant_numeric.proportional {
                NumericSpacing::Proportional
            } else {
                NumericSpacing::Normal
            },
        };
        let letter_spacing = match r.letter_spacing {
            LenNormal::Normal => 0.0,
            LenNormal::Len(l) => l.px,
        };
        let style = RunStyle::new(face, size)
            .with_wght(f32::from(r.font_weight))
            .with_letter_spacing(letter_spacing)
            .with_numeric(numeric);
        let metrics = tm.metrics(face, size, f32::from(r.font_weight)).ok()?;
        let line_height = match r.line_height {
            CssLineHeight::Normal => metrics.normal_line_height(),
            CssLineHeight::Px(v) => v,
            CssLineHeight::Factor(f) => f * size,
        };
        Some((
            face,
            style,
            VMetrics {
                metrics,
                line_height,
            },
        ))
    }
}

/// Collects the inline content of `id` into runs and boxes.
fn collect(
    arena: &Arena,
    id: NodeId,
    stack: &mut Vec<usize>,
    texts: &mut Vec<(NodeId, Vec<usize>)>,
    boxes: &mut Vec<InlineBox>,
) {
    let Some(n) = arena.get(id) else {
        return;
    };
    for &c in n.children() {
        match arena.get(c).map(|c| c.class()) {
            Some(NodeClass::Text) => texts.push((c, stack.iter().rev().copied().collect())),
            Some(NodeClass::Element) => {
                boxes.push(InlineBox {
                    node: c,
                    vm: VMetrics::ZERO,
                });
                stack.push(boxes.len() - 1);
                collect(arena, c, stack, texts, boxes);
                stack.pop();
            }
            _ => {}
        }
    }
}

/// Builds the inline layout of a root (or removes it when the node is not a root any more). Returns the
/// number of runs shaped.
fn build(
    ctx: &TextCtx<'_>,
    arena: &mut Arena,
    tm: &mut dyn TextMeasure,
    errors: &mut ErrorSink,
    root: NodeId,
) -> u32 {
    let Some(class) = arena.get(root).map(|n| n.class()) else {
        return 0;
    };
    let mut texts = Vec::new();
    let mut boxes = Vec::new();
    match class {
        NodeClass::Text => texts.push((root, Vec::new())),
        _ if ctx.inline_content(arena, root) => {
            collect(arena, root, &mut Vec::new(), &mut texts, &mut boxes)
        }
        _ => {
            if let Some(n) = arena.get_mut(root) {
                n.ifc = None;
            }
            return 0;
        }
    }
    let mut report = |arena: &Arena, id: NodeId, msg: &str| {
        let path = instantiate::node_path(arena, ctx.pkg, id);
        errors.report(UiError::at(&path, ErrorCode::Font, msg));
    };
    let Some(root_rec) = ctx.record(arena, root).cloned() else {
        return 0;
    };
    let strut = match ctx.vmetrics(tm, &root_rec) {
        Some((_, _, vm)) => vm,
        None => {
            report(arena, root, "no font face of the package matches");
            VMetrics::ZERO
        }
    };
    for b in &mut boxes {
        if let Some(rec) = ctx.record(arena, b.node)
            && let Some((_, _, vm)) = ctx.vmetrics(tm, rec)
        {
            b.vm = vm;
        }
    }
    let mut runs = Vec::new();
    let mut shaped = 0;
    for (node, bxs) in texts {
        let text: Arc<str> = arena
            .get(node)
            .and_then(|n| n.text.clone())
            .unwrap_or_else(|| Arc::from(""));
        let Some(rec) = ctx.record(arena, node).cloned() else {
            continue;
        };
        let Some((_, style, vm)) = ctx.vmetrics(tm, &rec) else {
            report(arena, node, "no font face of the package matches");
            continue;
        };
        shaped += 1;
        match tm.shape(&style, &text) {
            Ok(run) => runs.push(Run {
                node,
                shaped: run,
                vm,
                boxes: bxs,
            }),
            Err(e) => report(arena, node, &format!("text cannot be shaped: {e}")),
        }
    }
    let ifc = Ifc {
        runs,
        boxes,
        strut,
        wrap: root_rec.text_wrap_mode == TextWrapMode::Wrap,
        align: root_rec.text_align,
        lines: Lines::default(),
    };
    if let Some(n) = arena.get_mut(root) {
        n.ifc = Some(Box::new(ifc));
    }
    shaped
}

/// What one text pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PassStats {
    pub shaped: u32,
    pub visited: u32,
}

/// The text pass: rebuilds the inline layout of every root with a TEXT-dirty node and queues it for layout.
pub(crate) fn pass(
    ctx: &TextCtx<'_>,
    arena: &mut Arena,
    tm: &mut dyn TextMeasure,
    errors: &mut ErrorSink,
) -> PassStats {
    let mut stats = PassStats::default();
    if arena.pending(Dirty::TEXT) == 0 {
        return stats;
    }
    let mut roots = Vec::new();
    for id in arena.take_dirty(Dirty::TEXT) {
        stats.visited += 1;
        let Some(n) = arena.get(id) else {
            continue;
        };
        if n.class() == NodeClass::Text {
            roots.push(ctx.root_of(arena, id));
            continue;
        }
        if n.ifc.is_some() {
            roots.push(id);
        }
        // Text leaves below this element, through inline elements.
        let mut stack: Vec<NodeId> = n.children().to_vec();
        while let Some(c) = stack.pop() {
            match arena.get(c).map(|c| c.class()) {
                Some(NodeClass::Text) => roots.push(ctx.root_of(arena, c)),
                Some(NodeClass::Element) if ctx.is_inline_element(arena, c) => {
                    stack.extend(arena.get(c).map_or(&[][..], |c| c.children()).iter())
                }
                _ => {}
            }
        }
    }
    roots.sort();
    roots.dedup();
    for root in roots {
        stats.shaped += build(ctx, arena, tm, errors, root);
        arena.mark(root, Dirty::LAYOUT | Dirty::PAINT);
    }
    stats
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_follow_css_matching() {
        assert_eq!(pick_weight(400, &[400, 500]), Some(400));
        assert_eq!(pick_weight(450, &[400, 500]), Some(500));
        assert_eq!(pick_weight(500, &[400, 700]), Some(400));
        assert_eq!(pick_weight(600, &[400, 500]), Some(500));
        assert_eq!(pick_weight(300, &[400, 500]), Some(400));
        assert_eq!(pick_weight(700, &[400, 900]), Some(900));
        assert_eq!(pick_weight(400, &[]), None);
    }

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
