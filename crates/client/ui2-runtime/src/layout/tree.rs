//! taffy's tree traits over the arena.
//!
//! A layout run borrows the arena mutably together with the taffy styles of the records ([`LayoutStyles`]) and
//! the text measurer. taffy addresses nodes by slot index ([`NodeId::taffy`]); every id it receives came from
//! the arena during the same run, so a missing node only happens on a broken invariant and is answered with a
//! hidden box instead of a panic.
//!
//! Inline formatting roots (nodes with an [`Ifc`](crate::text::Ifc)) are leaves for taffy: their children are
//! laid out by the text module. A leaf measures its lines at the width taffy offers and reports its first and
//! last baselines (for `align-items: baseline`); the full layout pass also keeps the lines for paint and dumps.

use std::cell::Cell;

use taffy::{
    AvailableSpace, BlockContext, CacheTree, Display, LayoutBlockContainer, LayoutContainingBlock,
    LayoutFlexboxContainer, LayoutGridContainer, LayoutInput, LayoutOutput, LayoutPartialTree,
    ResolveOrZero, RunMode, Size, Style, TraversePartialTree, compute_block_layout,
    compute_cached_layout, compute_flexbox_layout, compute_grid_layout, compute_hidden_layout,
    compute_leaf_layout, compute_oof_layout, compute_root_layout,
};
use ui2_package::{StyleId, Styles};
use ui2_text::TextMeasure;

use crate::arena::{Arena, HIDDEN_STYLE, LayoutSlot, NodeClass, NodeId, PLACEHOLDER_STYLE};
use crate::text::ifc::Extent;
use crate::text::{AnonItem, Ifc};

/// The taffy style of each style record, plus the built-in styles of service nodes, placeholders and text
/// leaves that are their own inline root (anonymous blocks).
#[derive(Debug, Clone)]
pub struct LayoutStyles {
    by_record: Vec<Style>,
    hidden: Style,
    placeholder: Style,
    anonymous: Style,
}

impl LayoutStyles {
    /// Styles indexed by `StyleId`.
    pub fn new(by_record: Vec<Style>) -> Self {
        let block = Style {
            display: Display::Block,
            ..Style::DEFAULT
        };
        Self {
            by_record,
            hidden: Style {
                display: Display::None,
                ..Style::DEFAULT
            },
            placeholder: block.clone(),
            anonymous: block,
        }
    }

    /// The styles of every record of a package (computed once when the skin loads).
    pub fn from_styles(styles: &Styles) -> Self {
        Self::new(
            styles
                .records
                .iter()
                .map(super::style::taffy_style)
                .collect(),
        )
    }

    /// The taffy style of a record; an unknown record is hidden.
    pub fn get(&self, id: StyleId) -> &Style {
        match id {
            HIDDEN_STYLE => &self.hidden,
            PLACEHOLDER_STYLE => &self.placeholder,
            _ => self.by_record.get(id.index()).unwrap_or(&self.hidden),
        }
    }
}

/// Lays out the subtree of `root` in the available space. Results are in each node's `LayoutSlot`, relative to
/// the parent's border box (out-of-flow boxes: to their containing block), in CSS px, unrounded. Returns the
/// nodes whose layout changed.
pub fn compute_root(
    arena: &mut Arena,
    styles: &LayoutStyles,
    text: &dyn TextMeasure,
    root: NodeId,
    available: Size<AvailableSpace>,
) -> Vec<NodeId> {
    let mut view = View {
        arena,
        styles,
        text,
        scratch: LayoutSlot::default(),
        changed: Vec::new(),
    };
    compute_root_layout(&mut view, root.taffy(), available);
    view.changed
}

struct View<'a> {
    arena: &'a mut Arena,
    styles: &'a LayoutStyles,
    text: &'a dyn TextMeasure,
    /// Absorbs writes for ids that are not live (never happens while the arena is consistent).
    scratch: LayoutSlot,
    changed: Vec<NodeId>,
}

/// Where Chrome puts the anonymous flex item that wraps the text of a flex container: its size is the
/// text's (stretched across in a column), its place follows `justify-content` on the main axis and
/// `align-items` on the cross axis.
fn anon_item(style: &Style, content: Size<f32>, lines: Extent) -> AnonItem {
    use taffy::{AlignItems, FlexDirection, JustifyContent};
    let row = matches!(
        style.flex_direction,
        FlexDirection::Row | FlexDirection::RowReverse
    );
    let reversed = matches!(
        style.flex_direction,
        FlexDirection::RowReverse | FlexDirection::ColumnReverse
    );
    let iw = lines.width.min(content.width);
    let ih = lines.height;
    let jc = style.justify_content;
    let main = |free: f32| {
        if jc == JustifyContent::CENTER
            || jc == JustifyContent::SPACE_AROUND
            || jc == JustifyContent::SPACE_EVENLY
        {
            free / 2.0
        } else if jc == JustifyContent::END {
            free
        } else if jc == JustifyContent::START {
            0.0
        } else if (jc == JustifyContent::FLEX_END) != reversed {
            free
        } else {
            0.0
        }
    };
    let ai = style.align_items;
    let cross = |free: f32| {
        if ai == AlignItems::CENTER {
            free / 2.0
        } else if ai == AlignItems::END || ai == AlignItems::FLEX_END {
            free
        } else {
            0.0
        }
    };
    let stretched = ai == AlignItems::NORMAL || ai == AlignItems::STRETCH;
    if row {
        AnonItem {
            x: main(content.width - iw),
            y: cross(content.height - ih),
            w: iw,
        }
    } else {
        let w = if stretched { content.width } else { iw };
        AnonItem {
            x: cross(content.width - w),
            y: main(content.height - ih),
            w,
        }
    }
}

fn to_taffy(id: &NodeId) -> taffy::NodeId {
    id.taffy()
}

impl View<'_> {
    fn style(&self, id: taffy::NodeId) -> &Style {
        match self.arena.at(usize::from(id)) {
            Some(node) if node.class() == NodeClass::Text => &self.styles.anonymous,
            Some(node) => match node.overrides() {
                Some(o) => &o.taffy,
                None => self.styles.get(node.style()),
            },
            None => &self.styles.hidden,
        }
    }

    fn slot_mut(&mut self, id: taffy::NodeId) -> &mut LayoutSlot {
        match self.arena.at_mut(usize::from(id)) {
            Some(node) => &mut node.layout,
            None => &mut self.scratch,
        }
    }

    fn children(&self, id: taffy::NodeId) -> &[NodeId] {
        match self.arena.at(usize::from(id)) {
            // The content of an inline root is laid out by the text module.
            Some(n) if n.ifc.is_some() => &[],
            Some(n) => n.children(),
            None => &[],
        }
    }

    /// A leaf: an inline root measures its lines, any other leaf is an empty box sized by its style.
    fn leaf(&mut self, node: taffy::NodeId, inputs: LayoutInput) -> LayoutOutput {
        let idx = usize::from(node);
        let ifc: Option<&Ifc> = self.arena.at(idx).and_then(|n| n.ifc.as_deref());
        let style = self.style(node);
        let text = self.text;
        // The extent of the lines measured, and the width they were broken at.
        let kept: Cell<Option<(Extent, f32)>> = Cell::new(None);
        let mut output = compute_leaf_layout(
            inputs,
            style,
            |_, _| 0.0,
            |known, avail| {
                let Some(ifc) = ifc else {
                    return Size::ZERO;
                };
                let width = known.width.unwrap_or(match avail.width {
                    AvailableSpace::Definite(w) => w,
                    AvailableSpace::MinContent => 0.0,
                    AvailableSpace::MaxContent => f32::INFINITY,
                });
                let extent = ifc.extent_at(text, width);
                kept.set(Some((extent, width)));
                Size {
                    width: known.width.unwrap_or(extent.width),
                    height: known.height.unwrap_or(extent.height),
                }
            },
        );
        let kept = kept.get();
        let mut item = None;
        let button = ifc.is_some_and(|i| i.button);
        if ifc.is_some() {
            let inset = style
                .padding
                .resolve_or_zero(inputs.parent_size.width, |_, _| 0.0)
                + style
                    .border
                    .resolve_or_zero(inputs.parent_size.width, |_, _| 0.0);
            let content = Size {
                width: (output.size.width - inset.left - inset.right).max(0.0),
                height: (output.size.height - inset.top - inset.bottom).max(0.0),
            };
            if button
                && style.display == Display::Block
                && let Some((extent, _)) = kept
            {
                // Chrome centres a button's content down its content box (to the top when it overflows).
                item = Some(AnonItem {
                    x: 0.0,
                    y: ((content.height - extent.height) / 2.0).max(0.0),
                    w: content.width,
                });
            } else if style.display == Display::Flex
                && let Some((extent, _)) = kept
                && self.arena.at(idx).is_some_and(|n| {
                    n.children().iter().all(|&c| {
                        self.arena
                            .get(c)
                            .is_some_and(|c| c.class() == NodeClass::Text)
                    })
                })
            {
                item = Some(anon_item(style, content, extent));
            }
            let dy = item.map_or(0.0, |a: AnonItem| a.y);
            let (first, last) = kept.map_or((None, None), |(e, _)| (e.first, e.last));
            output.baselines.first = first.map(|b| b + inset.top + dy);
            output.baselines.last = last.map(|b| b + inset.top + dy);
        }
        if inputs.run_mode == RunMode::PerformLayout
            && let Some((_, width)) = kept
            && let Some(ifc) = self.arena.at_mut(idx).and_then(|n| n.ifc.as_deref_mut())
        {
            // The lines for paint and dumps, from the memo of the measurement above.
            ifc.lines = ifc.lines_at(text, width);
            ifc.width = width;
            ifc.item = item;
        }
        output
    }

    fn compute(
        &mut self,
        node: taffy::NodeId,
        inputs: LayoutInput,
        block_ctx: Option<&mut BlockContext<'_>>,
    ) -> LayoutOutput {
        if inputs.run_mode == RunMode::PerformHiddenLayout {
            return compute_hidden_layout(self, node);
        }
        compute_cached_layout(self, node, inputs, |tree, node, inputs| {
            let display = tree.style(node).display;
            let has_children = tree.child_count(node) > 0;
            let mut output = match (display, has_children) {
                (Display::None, _) => compute_hidden_layout(tree, node),
                (Display::Block, true) => compute_block_layout(tree, node, inputs, block_ctx),
                (Display::FlowRoot, true) => compute_block_layout(tree, node, inputs, None),
                (Display::Flex, true) => compute_flexbox_layout(tree, node, inputs),
                (Display::Grid, true) => compute_grid_layout(tree, node, inputs),
                (_, false) => tree.leaf(node, inputs),
            };
            if inputs.run_mode == RunMode::PerformLayout {
                compute_oof_layout(tree, node, &mut output);
            }
            output
        })
    }
}

impl TraversePartialTree for View<'_> {
    type ChildIter<'b>
        = std::iter::Map<std::slice::Iter<'b, NodeId>, fn(&NodeId) -> taffy::NodeId>
    where
        Self: 'b;

    fn child_ids(&self, parent: taffy::NodeId) -> Self::ChildIter<'_> {
        self.children(parent)
            .iter()
            .map(to_taffy as fn(&NodeId) -> taffy::NodeId)
    }

    fn child_count(&self, parent: taffy::NodeId) -> usize {
        self.children(parent).len()
    }

    fn get_child_id(&self, parent: taffy::NodeId, index: usize) -> taffy::NodeId {
        self.children(parent)
            .get(index)
            .map_or(taffy::NodeId::from(usize::MAX), to_taffy)
    }
}

impl LayoutPartialTree for View<'_> {
    type CoreContainerStyle<'b>
        = &'b Style
    where
        Self: 'b;
    type CustomIdent = String;

    fn get_core_container_style(&self, node: taffy::NodeId) -> Self::CoreContainerStyle<'_> {
        self.style(node)
    }

    fn set_unrounded_layout(&mut self, node: taffy::NodeId, layout: &taffy::Layout) {
        let slot = self.slot_mut(node);
        if slot.unrounded != *layout {
            slot.unrounded = *layout;
            if let Some(id) = self.arena.id_at(usize::from(node)) {
                self.changed.push(id);
            }
        }
    }

    fn compute_child_layout(&mut self, node: taffy::NodeId, inputs: LayoutInput) -> LayoutOutput {
        self.compute(node, inputs, None)
    }
}

impl LayoutContainingBlock for View<'_> {
    type OofItemStyle<'b>
        = &'b Style
    where
        Self: 'b;

    fn get_oof_item_style(&self, node: taffy::NodeId) -> Self::OofItemStyle<'_> {
        self.style(node)
    }

    fn clear_hoisted_children(&mut self, node: taffy::NodeId) {
        self.slot_mut(node).hoisted.clear();
    }

    fn add_hoisted_children(&mut self, node: taffy::NodeId, hoisted: &[taffy::NodeId]) {
        self.slot_mut(node).hoisted.extend_from_slice(hoisted);
    }
}

impl CacheTree for View<'_> {
    fn cache_get(&mut self, node: taffy::NodeId, input: &LayoutInput) -> Option<LayoutOutput> {
        self.slot_mut(node).cache.get(input)
    }

    fn cache_store(&mut self, node: taffy::NodeId, input: &LayoutInput, output: LayoutOutput) {
        self.slot_mut(node).cache.store(input, output);
    }

    fn cache_clear(&mut self, node: taffy::NodeId) {
        self.slot_mut(node).cache.clear();
    }
}

impl LayoutBlockContainer for View<'_> {
    type BlockContainerStyle<'b>
        = &'b Style
    where
        Self: 'b;
    type BlockItemStyle<'b>
        = &'b Style
    where
        Self: 'b;

    fn get_block_container_style(&self, node: taffy::NodeId) -> Self::BlockContainerStyle<'_> {
        self.style(node)
    }

    fn get_block_child_style(&self, child: taffy::NodeId) -> Self::BlockItemStyle<'_> {
        self.style(child)
    }

    fn compute_block_child_layout(
        &mut self,
        node: taffy::NodeId,
        inputs: LayoutInput,
        block_ctx: Option<&mut BlockContext<'_>>,
    ) -> LayoutOutput {
        self.compute(node, inputs, block_ctx)
    }
}

impl LayoutFlexboxContainer for View<'_> {
    type FlexboxContainerStyle<'b>
        = &'b Style
    where
        Self: 'b;
    type FlexboxItemStyle<'b>
        = &'b Style
    where
        Self: 'b;

    fn get_flexbox_container_style(&self, node: taffy::NodeId) -> Self::FlexboxContainerStyle<'_> {
        self.style(node)
    }

    fn get_flexbox_child_style(&self, child: taffy::NodeId) -> Self::FlexboxItemStyle<'_> {
        self.style(child)
    }
}

impl LayoutGridContainer for View<'_> {
    type GridContainerStyle<'b>
        = &'b Style
    where
        Self: 'b;
    type GridItemStyle<'b>
        = &'b Style
    where
        Self: 'b;

    fn get_grid_container_style(&self, node: taffy::NodeId) -> Self::GridContainerStyle<'_> {
        self.style(node)
    }

    fn get_grid_child_style(&self, child: taffy::NodeId) -> Self::GridItemStyle<'_> {
        self.style(child)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::{Node, NodeClass};
    use taffy::{Dimension, LengthPercentageAuto, Position};
    use ui2_package::{NodeRef, TemplateId};

    fn node(arena: &mut Arena, parent: Option<NodeId>, style: u32) -> NodeId {
        let mut n = Node::new(NodeRef(TemplateId(0), 0), NodeClass::Element, parent);
        n.style = StyleId(style);
        let id = arena.alloc(n);
        arena.attach(parent, usize::MAX, id);
        id
    }

    #[test]
    fn a_flex_row_lays_out_over_the_arena_without_rounding() {
        let row = Style {
            display: Display::Flex,
            size: Size {
                width: Dimension::length(200.5),
                height: Dimension::length(50.0),
            },
            ..Style::DEFAULT
        };
        let fixed = Style {
            size: Size {
                width: Dimension::length(50.25),
                height: Dimension::auto(),
            },
            ..Style::DEFAULT
        };
        let grow = Style {
            flex_grow: 1.0,
            ..Style::DEFAULT
        };
        let abs = Style {
            position: Position::Absolute,
            inset: taffy::Rect {
                left: LengthPercentageAuto::length(10.0),
                top: LengthPercentageAuto::length(5.0),
                right: LengthPercentageAuto::auto(),
                bottom: LengthPercentageAuto::auto(),
            },
            size: Size {
                width: Dimension::length(7.0),
                height: Dimension::length(3.0),
            },
            ..Style::DEFAULT
        };
        let styles = LayoutStyles::new(vec![row, fixed, grow, abs]);
        let mut arena = Arena::new();
        let root = node(&mut arena, None, 0);
        let a = node(&mut arena, Some(root), 1);
        let b = node(&mut arena, Some(root), 2);
        let hidden = node(&mut arena, Some(root), HIDDEN_STYLE.0);
        let c = node(&mut arena, Some(root), 3);
        let max = AvailableSpace::MaxContent;
        let engine = ui2_text::TextEngine::new();
        compute_root(
            &mut arena,
            &styles,
            &engine,
            root,
            Size {
                width: max,
                height: max,
            },
        );

        let rect = |id: NodeId| {
            let l = arena.get(id).expect("node").layout().unrounded;
            (l.location.x, l.location.y, l.size.width, l.size.height)
        };
        assert_eq!(rect(root), (0.0, 0.0, 200.5, 50.0));
        assert_eq!(rect(a), (0.0, 0.0, 50.25, 50.0));
        assert_eq!(rect(b), (50.25, 0.0, 150.25, 50.0));
        assert_eq!(rect(hidden), (0.0, 0.0, 0.0, 0.0));
        assert_eq!(rect(c), (10.0, 5.0, 7.0, 3.0));
        let root_slot = arena.get(root).expect("root").layout();
        assert_eq!(root_slot.hoisted, vec![c.taffy()]);
        assert!(!root_slot.cache.is_empty());
    }
}
