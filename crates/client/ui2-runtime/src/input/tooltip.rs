//! Tooltips (`data-tooltip`): while a node with a tooltip, or a descendant of it, is hovered, an instance of
//! its tooltip template lives in the `tooltip` layer; it goes when the hover leaves the node or the node goes.
//!
//! The instance is made when the hover arrives (its bindings, text and layout run in the next frame like any
//! new node). After that frame's layout it is placed (and again whenever its size changes): its left edge at the node's left edge, 4 px under the
//! node's bottom edge (or 4 px above its top edge when it fits there and not below), pushed back inside the
//! viewport, by `left`/`top` set on its root (which the skin positions absolutely), and laid out again in the same
//! frame. The instance carries the list key of the
//! node's innermost list item, so its bindings read that item.

use glam::Vec2;
use ui2_package::style::{Len, LenAuto};
use ui2_package::{Layer, PropId, TemplateId};

use crate::Runtime;
use crate::arena::NodeId;
use crate::input::hit;
use crate::instantiate::{self, Spawn};
use crate::style::{self, OverValue};

/// The gap between a node and its tooltip, CSS px.
pub const TOOLTIP_GAP: f32 = 4.0;

/// The top of a tooltip of height `tip` for a node at `y` of height `h` in a viewport of height `vh` (CSS px):
/// under the node when it fits there, above it when it fits there instead, otherwise on the side with more room,
/// pushed back inside the viewport.
// SIMPLIFIED(SIMP-200): a tooltip goes under or above its node (left edges aligned, 4 px gap, pushed back inside the viewport); it is never put beside the node
pub(crate) fn tooltip_top(y: f32, h: f32, tip: f32, vh: f32) -> f32 {
    let below = y + h + TOOLTIP_GAP;
    let above = y - TOOLTIP_GAP - tip;
    let top = if below + tip <= vh || (above < 0.0 && vh - (y + h) >= y) {
        below
    } else {
        above
    };
    top.min(vh - tip).max(0.0)
}

/// A tooltip on screen.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Tooltip {
    /// The node that shows it.
    pub owner: NodeId,
    /// The top-level nodes of the instance.
    pub roots: Vec<NodeId>,
    /// The size of its root when it was last placed (`None` before its first layout). A tooltip whose content
    /// grows or shrinks later (a core reply arriving after the hover) is placed again for its new size.
    pub placed: Option<Vec2>,
}

impl Runtime {
    /// The root of the tooltip on screen.
    pub fn tooltip(&self) -> Option<NodeId> {
        let t = self.input.tooltip.as_ref()?;
        t.roots.first().copied().filter(|&r| self.arena.contains(r))
    }

    /// The tooltip template of a node, if it has one.
    fn tooltip_of(&self, id: NodeId) -> Option<TemplateId> {
        let tmpl = self.arena.get(id)?.tmpl();
        let list = self.pkg.tooltips();
        let at = list.binary_search_by(|(n, _)| n.cmp(&tmpl)).ok()?;
        Some(list[at].1)
    }

    fn remove_tooltip(&mut self) {
        if let Some(t) = self.input.tooltip.take() {
            for r in t.roots {
                self.arena.remove(r);
            }
        }
    }

    /// Shows the tooltip of the nearest hovered node that has one (or none).
    pub(crate) fn update_tooltip(&mut self) {
        if self.pkg.tooltips().is_empty() {
            return;
        }
        let want = self
            .hover
            .iter()
            .find_map(|&id| self.tooltip_of(id).map(|t| (id, t)));
        if let (Some(t), Some((owner, _))) = (&self.input.tooltip, want)
            && t.owner == owner
        {
            return;
        }
        self.remove_tooltip();
        let Some((owner, tpl)) = want else {
            return;
        };
        let parent = hit::layer_roots(&self.pkg, &self.arena)
            .into_iter()
            .find(|(l, _, is_layer)| *l == Layer::Tooltip && *is_layer)
            .map(|(_, r, _)| r);
        let mut key = None;
        let mut at = Some(owner);
        while let Some(n) = at.and_then(|a| self.arena.get(a)) {
            if let Some(k) = n.key() {
                key = Some(k);
                break;
            }
            at = n.parent();
        }
        let pkg = std::sync::Arc::clone(&self.pkg);
        let at = self.arena.siblings(parent).len();
        let roots = instantiate::instantiate(
            &mut self.arena,
            &*pkg,
            &self.style_env,
            &mut self.errors,
            Spawn {
                tpl,
                parent,
                at,
                key,
                owner: None,
                nodes: None,
                own_chunk: false,
            },
        );
        // A first guess under the node (its containing block taken at the page origin), so that the placement
        // after layout only corrects `left`/`top` by the distance still missing.
        if let Some(&root) = roots.first() {
            let mut g = self.geo();
            let guess = g.abs(owner) + Vec2::new(0.0, g.size(owner).y + TOOLTIP_GAP);
            for (prop, v) in [(PropId::Left, guess.x), (PropId::Top, guess.y)] {
                let bits = style::set_override(
                    &mut self.arena,
                    self.pkg.styles(),
                    &self.style_index,
                    root,
                    prop,
                    Some(OverValue::Len(LenAuto::Len(Len::px(v)))),
                );
                self.arena.mark(root, bits);
            }
        }
        self.input.tooltip = Some(Tooltip {
            owner,
            roots,
            placed: None,
        });
    }

    /// Places a new tooltip after layout, or drops one whose node is gone. Returns whether layout must run
    /// again. An idle frame checks one field.
    pub(crate) fn place_tooltip(&mut self) -> bool {
        let Some(t) = &self.input.tooltip else {
            return false;
        };
        if !self.arena.contains(t.owner) || !self.geo().shown(t.owner) {
            self.remove_tooltip();
            return false;
        }
        let (owner, root, placed) = (t.owner, t.roots.first().copied(), t.placed);
        let Some(root) = root.filter(|&r| self.arena.contains(r)) else {
            return false;
        };
        // Placed for the size it has now: nothing to do (an idle frame reads one size).
        let size = self.geo().size(root);
        if placed == Some(size) {
            return false;
        }
        if let Some(t) = self.input.tooltip.as_mut() {
            t.placed = Some(size);
        }
        let mut g = self.geo();
        let (mut opos, mut osize) = (g.abs(owner), g.size(owner));
        // A node of a map anchor is where the host put the anchor, not where the layout has it.
        if let Some(xf) = g.anchor_xf(owner) {
            let (from, to) = (xf.transform_point2(opos), xf.transform_point2(opos + osize));
            (opos, osize) = (from, to - from);
        }
        let (tpos, tsize) = (g.abs(root), g.size(root));
        let vp = Vec2::from(self.style_env.scale.viewport_css);
        let want = Vec2::new(
            opos.x.min(vp.x - tsize.x).max(0.0),
            tooltip_top(opos.y, osize.y, tsize.y, vp.y),
        );
        // `left`/`top` move the box by as much as they change (its containing block stays).
        let current = |prop: PropId| {
            self.arena
                .get(root)
                .and_then(|n| n.overrides())
                .and_then(|o| o.props.iter().find(|(p, _)| *p == prop))
                .and_then(|(_, v)| match v {
                    OverValue::Len(LenAuto::Len(l)) => Some(l.px),
                    _ => None,
                })
        };
        let left = current(PropId::Left).unwrap_or(0.0) + want.x - tpos.x;
        let top = current(PropId::Top).unwrap_or(0.0) + want.y - tpos.y;
        let mut bits = crate::arena::Dirty::NONE;
        for (prop, v) in [(PropId::Left, left), (PropId::Top, top)] {
            bits |= style::set_override(
                &mut self.arena,
                self.pkg.styles(),
                &self.style_index,
                root,
                prop,
                Some(OverValue::Len(LenAuto::Len(Len::px(v)))),
            );
        }
        self.arena.mark(root, bits);
        bits.contains(crate::arena::Dirty::LAYOUT)
    }
}

#[cfg(test)]
mod tests {
    use super::tooltip_top;

    #[test]
    fn a_tooltip_goes_under_its_node_or_above_it_when_it_does_not_fit_under() {
        // Room under the node.
        assert_eq!(tooltip_top(100.0, 20.0, 200.0, 900.0), 124.0);
        // No room under, room above: above, its bottom 4 px over the node.
        assert_eq!(tooltip_top(768.0, 68.0, 500.0, 900.0), 264.0);
        // Room on neither side: the side with more room, pushed inside the viewport.
        assert_eq!(tooltip_top(600.0, 68.0, 800.0, 900.0), 0.0);
        assert_eq!(tooltip_top(200.0, 68.0, 800.0, 900.0), 100.0);
    }
}
