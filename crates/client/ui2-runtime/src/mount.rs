//! Mod panels in the skin's extension points.
//!
//! A mod fragment (a package of kind `Fragment`) is linked into the document ([`crate::Doc`]) and one of its
//! panels — a top-level node of its document template — is instantiated in an extension point of the skin
//! (`data-slot`): after the point's own children and the panels mounted before it (`Append`, ordered by the
//! host's order), or in place of the point's node, which is hidden meanwhile (`Replace`). A mod panel is a
//! chunk of its own: a change inside it repaints that chunk only, and an idle frame does not look at it.
//! Unmounting removes the panel (and shows a replaced node again); the linked tables stay until the skin is
//! loaded again, so mounting the same fragment again costs only the instance.

use std::sync::Arc;

use ui2_package::style::Display;
use ui2_package::{NodeRef, Package, PropId, TemplateId};

use crate::LoadError;
use crate::Runtime;
use crate::arena::NodeId;
use crate::error::{ErrorCode, UiError};
use crate::instantiate::{self, Spawn};
use crate::layout::LayoutStyles;
use crate::panel::{MountMode, Panel, PanelId};
use crate::style::{OverValue, StyleIndex};

impl Runtime {
    /// The live node of an extension point of the skin.
    fn slot_node(&self, slot: &str) -> Option<NodeId> {
        let (_, r) = self.pkg.skin().slots().find(|(n, _)| *n == slot)?;
        self.arena
            .preorder()
            .into_iter()
            .find(|&id| self.arena.get(id).is_some_and(|n| n.tmpl() == r))
    }

    /// The template nodes `start..end` of a fragment's panel: the top-level node of the fragment's document
    /// template whose `data-part` is `panel`, or the only top-level node.
    fn panel_nodes(&self, tpl: TemplateId, panel: &str) -> Option<(u32, u32)> {
        let template = self.pkg.templates().get(tpl.index())?;
        let mut tops = Vec::new();
        let mut i = 0usize;
        while i < template.nodes.len() {
            let n = &template.nodes[i];
            let end = i + n.subtree.max(1) as usize;
            tops.push((i as u32, end as u32, n.part));
            i = end;
        }
        let named = tops
            .iter()
            .find(|(_, _, p)| p.and_then(|p| self.pkg.str(p)) == Some(panel));
        match (named, tops.as_slice()) {
            (Some(&(a, b, _)), _) => Some((a, b)),
            (None, [(a, b, _)]) => Some((*a, *b)),
            _ => None,
        }
    }

    /// Mounts panel `panel` of a mod fragment in the extension point `slot` of the skin; `order` places it
    /// among the mod panels of an `Append` point (the host passes the mod's load order). The panel's nodes
    /// are made now; their bindings, styles, text and layout run in the next frame. A part of the fragment
    /// this skin cannot use as it is (a color the skin lacks, pictures) is reported once and the rest is
    /// mounted. `Err` when the package is not a fragment, the skin has no such point, or the fragment no
    /// such panel: nothing is mounted then.
    pub fn mount_fragment(
        &mut self,
        frag: Arc<Package>,
        panel: &str,
        slot: &str,
        mode: MountMode,
        order: u32,
    ) -> Result<PanelId, LoadError> {
        let mod_id: Arc<str> = Arc::from(frag.manifest().id.as_str());
        let point = self.slot_node(slot).ok_or_else(|| {
            LoadError::Fatal(format!(
                "the skin has no extension point {slot} for panel {panel} of mod {mod_id}"
            ))
        })?;
        let before = self.pkg.templates().len();
        let linked = std::sync::Arc::make_mut(&mut self.pkg)
            .link(&frag)
            .map_err(LoadError::Fatal)?;
        if self.pkg.templates().len() != before {
            self.style_index = StyleIndex::new(self.pkg.styles());
            self.layout_styles = LayoutStyles::from_styles(self.pkg.styles());
        }
        let doc_tpl = self
            .pkg
            .frag(linked.frag)
            .map(|f| TemplateId(f.tpls.start))
            .ok_or_else(|| LoadError::Fatal("the fragment was not linked".into()))?;
        let nodes = self
            .panel_nodes(doc_tpl, panel)
            .ok_or_else(|| LoadError::Fatal(format!("mod {mod_id} has no panel {panel}")))?;
        for w in &linked.warnings {
            self.errors.report(UiError {
                path: format!("mod {mod_id} panel {panel}: {w}"),
                panel: Some(panel.to_string()),
                code: ErrorCode::Mount,
                msg: w.clone(),
            });
        }

        let (parent, at, hides) = match mode {
            MountMode::Append => {
                let children = self.arena.siblings(Some(point));
                // After the skin's children and the panels of a lower or equal order.
                let later = |c: &NodeId| {
                    self.panels
                        .list
                        .iter()
                        .flatten()
                        .any(|p| p.root == Some(*c) && p.owner.is_some() && p.order > order)
                };
                let at = children
                    .iter()
                    .rposition(|c| !later(c))
                    .map_or(0, |i| i + 1);
                (Some(point), at, None)
            }
            MountMode::Replace => {
                let parent = self.arena.get(point).and_then(|n| n.parent());
                let at = self
                    .arena
                    .siblings(parent)
                    .iter()
                    .position(|&c| c == point)
                    .map_or(usize::MAX, |i| i + 1);
                (parent, at, Some(point))
            }
        };
        let pkg = Arc::clone(&self.pkg);
        let roots = instantiate::instantiate(
            &mut self.arena,
            &*pkg,
            &self.style_env,
            &mut self.errors,
            Spawn {
                tpl: doc_tpl,
                parent,
                at,
                key: None,
                owner: None,
                nodes: Some(nodes),
                own_chunk: true,
            },
        );
        if let Some(h) = hides {
            self.override_prop(h, PropId::Display, Some(OverValue::Display(Display::None)));
        }
        Ok(self.panels.add(Panel {
            name: panel.to_string(),
            owner: Some(mod_id),
            slot: Some(slot.to_string()),
            mode: Some(mode),
            root: roots.first().copied(),
            failed: None,
            order,
            hides,
        }))
    }

    /// Removes a mod panel: its nodes and bindings go, a node it replaced is shown again. Returns whether
    /// there was such a mounted panel (skin panels cannot be unmounted).
    pub fn unmount(&mut self, id: PanelId) -> bool {
        let Some(slot) = self.panels.list.get_mut(id.0 as usize) else {
            return false;
        };
        if slot.as_ref().is_none_or(|p| p.owner.is_none()) {
            return false;
        }
        let Some(p) = slot.take() else {
            return false;
        };
        if let Some(root) = p.root {
            self.arena.remove(root);
        }
        if let Some(h) = p.hides {
            let still = self
                .panels
                .list
                .iter()
                .flatten()
                .any(|o| o.hides == Some(h));
            if !still {
                self.override_prop(h, PropId::Display, None);
            }
        }
        true
    }

    /// The template node of a panel's root, for tests and tools.
    pub fn panel_root(&self, id: PanelId) -> Option<(NodeId, NodeRef)> {
        let root = self.panels.list.get(id.0 as usize)?.as_ref()?.root?;
        Some((root, self.arena.get(root)?.tmpl()))
    }
}
