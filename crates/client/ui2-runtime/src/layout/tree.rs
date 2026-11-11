//! taffy's tree traits over the arena.
//!
//! A layout run borrows the arena mutably together with the taffy styles of the records ([`LayoutStyles`]).
//! taffy addresses nodes by slot index ([`NodeId::taffy`]); every id it receives came from the arena during the
//! same run, so a missing node only happens on a broken invariant and is answered with a hidden box instead of
//! a panic.
//!
//! Present state: the tree, the cache and the dispatch to block, flex and grid are complete; leaves measure as
//! empty boxes. The text leaf measurement (with first baselines), the conversion of style records to taffy
//! styles and the dirty-flag driven cache invalidation come with the layout pass.

use taffy::{
    AvailableSpace, BlockContext, CacheTree, Display, LayoutBlockContainer, LayoutContainingBlock,
    LayoutFlexboxContainer, LayoutGridContainer, LayoutInput, LayoutOutput, LayoutPartialTree,
    RunMode, Size, Style, TraversePartialTree, compute_block_layout, compute_cached_layout,
    compute_flexbox_layout, compute_grid_layout, compute_hidden_layout, compute_leaf_layout,
    compute_oof_layout, compute_root_layout,
};
use ui2_package::StyleId;

use crate::arena::{Arena, HIDDEN_STYLE, LayoutSlot, NodeId, PLACEHOLDER_STYLE};

/// The taffy style of each style record, plus the built-in styles of service nodes and placeholders.
#[derive(Debug, Clone)]
pub struct LayoutStyles {
    by_record: Vec<Style>,
    hidden: Style,
    placeholder: Style,
}

impl LayoutStyles {
    /// Styles indexed by `StyleId`.
    pub fn new(by_record: Vec<Style>) -> Self {
        Self {
            by_record,
            hidden: Style {
                display: Display::None,
                ..Style::DEFAULT
            },
            placeholder: Style {
                display: Display::Block,
                ..Style::DEFAULT
            },
        }
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
/// the parent's border box, in CSS px, unrounded.
pub fn compute_root(
    arena: &mut Arena,
    styles: &LayoutStyles,
    root: NodeId,
    available: Size<AvailableSpace>,
) {
    let mut view = View {
        arena,
        styles,
        scratch: LayoutSlot::default(),
    };
    compute_root_layout(&mut view, root.taffy(), available);
}

struct View<'a> {
    arena: &'a mut Arena,
    styles: &'a LayoutStyles,
    /// Absorbs writes for ids that are not live (never happens while the arena is consistent).
    scratch: LayoutSlot,
}

fn to_taffy(id: &NodeId) -> taffy::NodeId {
    id.taffy()
}

impl View<'_> {
    fn style(&self, id: taffy::NodeId) -> &Style {
        match self.arena.at(usize::from(id)) {
            Some(node) => self.styles.get(node.style()),
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
        self.arena
            .at(usize::from(id))
            .map_or(&[][..], |n| n.children())
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
                // Leaves: text and pictures measure here (text with its first baseline).
                (_, false) => {
                    compute_leaf_layout(inputs, tree.style(node), |_, _| 0.0, |_, _| Size::ZERO)
                }
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
        self.slot_mut(node).unrounded = *layout;
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
        compute_root(
            &mut arena,
            &styles,
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
