//! Text in the tree: font selection, inline formatting roots, line boxes.
//!
//! An *inline formatting root* is an element whose children are all inline-level: text leaves and inline
//! elements (`span`, `b`, ... with `display: inline`) holding text. It is one leaf for taffy; its content is
//! laid out here, on `ui2-text`. A text leaf that has block siblings (an anonymous block in CSS) is its own
//! root. Each root keeps an [`Ifc`]: one shaped run per text leaf (runs are never shaped across an element
//! boundary), the vertical metrics of every inline box, and the lines at the width layout gave it.
//!
//! Line boxes, markup pieces and ellipsis are in [`ifc`]; `data-fit` in [`fit`].
//!
//! Shaping happens in the text pass (TEXT dirty nodes only); layout only breaks shaped runs into lines;
//! fitting runs after layout, for the roots whose room or text changed.

pub mod fit;
pub mod ifc;

use std::sync::Arc;

use ui2_package::style::{
    Display, FontStyle, LenNormal, LineHeight as CssLineHeight, Overflow, StyleRecord,
    TextOverflow, TextWrapMode,
};
use ui2_package::{BindKind, Package};
use ui2_text::{
    FaceId, Lines, NumericFigure, NumericSpacing, NumericVariant, RunStyle, ShapedRun, TextEngine,
    TextMeasure,
};

pub use ifc::{AnonItem, Fitted, Ifc, InlineBox, InlineRect, Run, Shown, VMetrics};

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
fn pick_weight(want: u16, have: impl Iterator<Item = u16> + Clone) -> Option<u16> {
    if have.clone().any(|w| w == want) {
        return Some(want);
    }
    let below = || have.clone().filter(|&w| w < want).max();
    let above = |limit: u16| have.clone().filter(|&w| w > want && w <= limit).min();
    match want {
        400..=500 => above(500)
            .or_else(below)
            .or_else(|| have.clone().filter(|&w| w > 500).min()),
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
            let (mut any, mut styled) = (false, false);
            for f in self.faces.iter().filter(|f| f.0 == family) {
                any = true;
                styled |= f.2 == italic;
            }
            if !any {
                continue;
            }
            // The faces of the style asked for, or of the family when it has none of that style.
            let pool = |f: &&(String, u16, bool)| f.0 == family && (!styled || f.2 == italic);
            let w = pick_weight(weight, self.faces.iter().filter(pool).map(|f| f.1))?;
            return self
                .faces
                .iter()
                .position(|f| pool(&f) && f.1 == w)
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

/// What the text pass reads.
pub(crate) struct TextCtx<'a> {
    pub pkg: &'a crate::Pkg,
    pub fonts: &'a FontBook,
    pub index: &'a StyleIndex,
}

impl TextCtx<'_> {
    fn record<'a>(&'a self, arena: &'a Arena, id: NodeId) -> Option<&'a StyleRecord> {
        let n = arena.get(id)?;
        Some(crate::style::node_record(self.index, self.pkg.styles(), n))
    }

    /// An element with `display: inline` that is not a flex or grid item: CSS blockifies the children of a
    /// flex or grid container, so text beside such an element is not in a line with it but in its own
    /// anonymous item (Chrome's computed records already say `block` for them; a record written by hand may
    /// not).
    fn is_inline_element(&self, arena: &Arena, id: NodeId) -> bool {
        let Some(n) = arena.get(id).filter(|n| n.class() == NodeClass::Element) else {
            return false;
        };
        let item = n
            .parent()
            .and_then(|p| self.record(arena, p))
            .is_some_and(|r| matches!(r.display, Display::Flex | Display::Grid));
        !item
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

    pub(crate) fn vmetrics(
        &self,
        tm: &dyn TextMeasure,
        r: &StyleRecord,
    ) -> Option<(FaceId, RunStyle, VMetrics)> {
        let names = r.font_family.iter().filter_map(|&s| self.pkg.str(s));
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

/// The widths a line breaker sees in a run: the width of every piece between spaces and of every space, in
/// order. Two runs with the same profile break into lines of the same widths at any available width.
fn break_profile(run: &ShapedRun) -> Vec<f32> {
    let bytes = run.text.as_bytes();
    let mut out = Vec::new();
    let mut acc = 0.0f32;
    let mut in_space = None;
    for g in run.glyphs.iter() {
        let space = matches!(bytes.get(g.cluster as usize), Some(b' '));
        if in_space != Some(space) {
            if in_space.is_some() {
                out.push(acc);
            }
            acc = 0.0;
            in_space = Some(space);
        }
        acc += g.advance;
    }
    out.push(acc);
    out
}

/// Whether a new inline layout takes exactly the room of the old one, so the box needs no layout: the same
/// runs with the same widths and vertical metrics (and, when lines wrap, the same widths between spaces).
fn same_extent(old: &Ifc, new: &Ifc) -> bool {
    let same_boxes = old.boxes.len() == new.boxes.len()
        && old.boxes.iter().zip(&new.boxes).all(|(a, b)| a.vm == b.vm);
    same_boxes
        && old.strut == new.strut
        && old.wrap == new.wrap
        && old.runs.len() == new.runs.len()
        && old.runs.iter().zip(&new.runs).all(|(a, b)| {
            a.node == b.node
                && a.piece == b.piece
                && a.vm == b.vm
                && a.boxes == b.boxes
                && a.shaped.width == b.shaped.width
                && (!new.wrap
                    || Arc::ptr_eq(&a.shaped.glyphs, &b.shaped.glyphs)
                    || break_profile(&a.shaped) == break_profile(&b.shaped))
        })
}

/// What rebuilding one root did.
#[derive(Debug, Clone, Copy, Default)]
struct Built {
    /// Runs shaped (runs whose text and style did not change are kept).
    shaped: u32,
    /// The root's box must be laid out again (its content takes another room).
    layout: bool,
    /// The root fits its text to its box (`data-fit`).
    fits: bool,
}

/// Builds the inline layout of a root (or removes it when the node is not a root any more).
fn build(
    ctx: &TextCtx<'_>,
    arena: &mut Arena,
    tm: &mut dyn TextMeasure,
    errors: &mut ErrorSink,
    root: NodeId,
) -> Built {
    let Some(class) = arena.get(root).map(|n| n.class()) else {
        return Built::default();
    };
    let mut texts = Vec::new();
    let mut boxes = Vec::new();
    match class {
        NodeClass::Text => texts.push((root, Vec::new())),
        _ if ctx.inline_content(arena, root) => {
            collect(arena, root, &mut Vec::new(), &mut texts, &mut boxes)
        }
        _ => {
            let had = arena.get_mut(root).and_then(|n| n.ifc.take()).is_some();
            return Built {
                shaped: 0,
                layout: had,
                fits: false,
            };
        }
    }
    let old = arena.get_mut(root).and_then(|n| n.ifc.take());
    let mut report = |arena: &Arena, id: NodeId, code: ErrorCode, msg: &str| {
        let path = instantiate::node_path(arena, ctx.pkg, id);
        errors.report(UiError::at(&path, code, msg));
    };
    let Some(root_rec) = ctx.record(arena, root) else {
        return Built::default();
    };
    // What the root's own record decides (read once: the record stays borrowed from the arena).
    let wrap = root_rec.text_wrap_mode == TextWrapMode::Wrap;
    // `text-overflow: ellipsis` cuts a single line that overflows a clipping box.
    let ellipsize = root_rec.overflow_x != Overflow::Visible
        && !wrap
        && root_rec.text_overflow == TextOverflow::Ellipsis;
    let (align, font_size) = (root_rec.text_align, root_rec.font_size.px);
    let strut = match ctx.vmetrics(tm, root_rec) {
        Some((_, _, vm)) => vm,
        None => {
            report(
                arena,
                root,
                ErrorCode::Font,
                "no font face of the package matches",
            );
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
        let Some(rec) = ctx.record(arena, node) else {
            continue;
        };
        let Some((_, style, vm)) = ctx.vmetrics(tm, rec) else {
            report(
                arena,
                node,
                ErrorCode::Font,
                "no font face of the package matches",
            );
            continue;
        };
        let (pieces, icons) = crate::loc::split_markup(&text);
        if icons {
            report(
                arena,
                node,
                ErrorCode::L10n,
                "icons in a text ([ICON_*]) are not shown",
            );
        }
        for (i, p) in pieces.into_iter().enumerate() {
            let piece = i.min(usize::from(u16::MAX - 1)) as u16;
            let color = p
                .color
                .as_deref()
                .and_then(|c| crate::loc::markup_color(ctx.pkg, c));
            // A piece whose text and style did not change keeps its shaped run.
            let kept = old
                .as_ref()
                .and_then(|o| o.runs.iter().find(|r| r.node == node && r.piece == piece))
                .filter(|r| r.shaped.style == style && *r.shaped.text == *p.text);
            let run = match kept {
                Some(prev) => Ok(prev.shaped.clone()),
                None => {
                    shaped += 1;
                    tm.shape(&style, &p.text)
                }
            };
            match run {
                Ok(run) => runs.push(Run {
                    node,
                    piece,
                    color,
                    shaped: run,
                    vm,
                    boxes: bxs.clone(),
                }),
                Err(e) => report(
                    arena,
                    node,
                    ErrorCode::Font,
                    &format!("text cannot be shaped: {e}"),
                ),
            }
        }
    }
    let ellipsis = match runs.last() {
        Some(last) if ellipsize => {
            let kept = old
                .as_ref()
                .and_then(|o| o.ellipsis.as_ref())
                .filter(|e| e.shaped.style == last.shaped.style);
            let shaped_ell = match kept {
                Some(e) => Ok(e.shaped.clone()),
                None => {
                    shaped += 1;
                    tm.shape(&last.shaped.style, "\u{2026}")
                }
            };
            shaped_ell.ok().map(|s| Run {
                piece: u16::MAX,
                shaped: s,
                ..last.clone()
            })
        }
        _ => None,
    };
    let fit = arena.get(root).and_then(|n| {
        let r = n.tmpl();
        ctx.pkg
            .bindings_of(r.0, r.1)
            .iter()
            .find_map(|b| match b.kind {
                BindKind::Fit { min } => Some(min),
                _ => None,
            })
    });
    let mut ifc = Ifc {
        runs,
        boxes,
        strut,
        wrap,
        align,
        ellipsis,
        fit: if class == NodeClass::Text { None } else { fit },
        font_size,
        lines: Lines::default(),
        width: 0.0,
        fitted: None,
        fit_key: None,
        item: old.as_ref().and_then(|o| o.item),
        button: false,
        memo: ifc::LineMemo::new(),
    };
    let layout = match &old {
        Some(o) if same_extent(o, &ifc) => {
            // Same room: break again at the width layout gave, for paint, and leave the box alone.
            ifc.width = o.width;
            ifc.lines = ifc.lines_at(tm, o.width);
            false
        }
        _ => true,
    };
    ifc.button = class != NodeClass::Text
        && arena.get(root).is_some_and(|n| {
            instantiate::template_node(ctx.pkg, n.tmpl()).is_some_and(|t| {
                matches!(
                    t.kind,
                    ui2_package::NodeKind::Box {
                        tag: ui2_package::Tag::Button
                    }
                )
            })
        });
    let fits = ifc.fit.is_some();
    if let Some(n) = arena.get_mut(root) {
        n.ifc = Some(Box::new(ifc));
    }
    Built {
        shaped,
        layout,
        fits,
    }
}

/// What one text pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PassStats {
    pub shaped: u32,
    pub visited: u32,
}

/// The text pass: rebuilds the inline layout of every root with a TEXT-dirty node and queues it for paint,
/// and for layout only when its content takes another room. Text fields in the queue go to `fields` (the
/// input module shapes them).
pub(crate) fn pass(
    ctx: &TextCtx<'_>,
    arena: &mut Arena,
    tm: &mut dyn TextMeasure,
    errors: &mut ErrorSink,
    fit_roots: &mut Vec<NodeId>,
    fields: &mut Vec<NodeId>,
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
        if instantiate::template_node(ctx.pkg, n.tmpl())
            .is_some_and(|t| matches!(t.kind, ui2_package::NodeKind::Input { .. }))
        {
            fields.push(id);
            continue;
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
        let built = build(ctx, arena, tm, errors, root);
        stats.shaped += built.shaped;
        let bits = if built.layout {
            Dirty::LAYOUT | Dirty::PAINT
        } else {
            Dirty::PAINT
        };
        arena.mark(root, bits);
        if built.fits && !fit_roots.contains(&root) {
            fit_roots.push(root);
        }
    }
    stats
}

/// Fits the texts of the `data-fit` roots whose room or text changed since they were last fitted (after
/// layout). Roots that were removed or stopped fitting leave the list.
pub(crate) fn fit_pass(
    arena: &mut Arena,
    tm: &mut dyn TextMeasure,
    fit_roots: &mut Vec<NodeId>,
) -> PassStats {
    let mut stats = PassStats::default();
    fit_roots.retain(|&id| {
        let Some(ifc) = arena
            .get_mut(id)
            .and_then(|n| n.ifc.as_deref_mut())
            .filter(|i| i.fit.is_some())
        else {
            return false;
        };
        stats.visited += 1;
        let before = ifc.fitted.as_ref().map(|f| f.scale);
        let shaped = fit::fit(ifc, tm);
        let after = ifc.fitted.as_ref().map(|f| f.scale);
        stats.shaped += shaped;
        if shaped > 0 || before != after {
            arena.mark(id, Dirty::PAINT);
        }
        true
    });
    stats
}

#[cfg(test)]
mod tests {
    #[test]
    fn weights_follow_css_matching() {
        let pick_weight = |want: u16, have: &[u16]| super::pick_weight(want, have.iter().copied());
        assert_eq!(pick_weight(400, &[400, 500]), Some(400));
        assert_eq!(pick_weight(450, &[400, 500]), Some(500));
        assert_eq!(pick_weight(500, &[400, 700]), Some(400));
        assert_eq!(pick_weight(600, &[400, 500]), Some(500));
        assert_eq!(pick_weight(300, &[400, 500]), Some(400));
        assert_eq!(pick_weight(700, &[400, 900]), Some(900));
        assert_eq!(pick_weight(400, &[]), None);
    }
}
