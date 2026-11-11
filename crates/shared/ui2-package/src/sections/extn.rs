//! `EXTN`: extension points, named parts, screens, script islands, hotkeys and tooltips.

use serde::{Deserialize, Serialize};

use crate::container::SectionTag;
use crate::error::PackageError;
use crate::ids::{NodeRef, StrId, TemplateId};
use crate::sections::templates::Templates;

/// Modifier bits of [`Hotkey::mods`].
pub mod hotkey_mods {
    /// Ctrl.
    pub const CTRL: u8 = 1;
    /// Alt.
    pub const ALT: u8 = 2;
    /// Shift.
    pub const SHIFT: u8 = 4;
    /// Every bit this version defines.
    pub const ALL: u8 = CTRL | ALT | SHIFT;
}

/// A script island: a subtree the script drives by key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Island {
    /// The island id.
    pub id: StrId,
    /// Its root node.
    pub node: NodeRef,
    /// The keyed nodes (`data-ui2-key`): key and node index in the root's template; ascending by
    /// key.
    pub keys: Vec<(StrId, u32)>,
}

/// A key bound to a node's click.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hotkey {
    /// The `KeyboardEvent.code` (`KeyB`).
    pub code: StrId,
    /// `hotkey_mods` bits.
    pub mods: u8,
    /// The node.
    pub node: NodeRef,
}

/// The section. Every list is sorted (names by string, then node) so that lookup is a binary
/// search and the bytes do not depend on the order things were added in.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Extn {
    /// Extension points for mods, by name.
    pub slots: Vec<(StrId, NodeRef)>,
    /// Stable part names, by name.
    pub parts: Vec<(StrId, NodeRef)>,
    /// Screens the skin implements, by id.
    pub screens: Vec<(StrId, NodeRef)>,
    /// Script islands, by id.
    pub islands: Vec<Island>,
    /// Hotkeys, by `(code, mods, node)`.
    pub hotkeys: Vec<Hotkey>,
    /// Tooltips: the node that shows the tooltip template, by node.
    pub tooltips: Vec<(NodeRef, TemplateId)>,
}

fn node_ok(templates: &Templates, n: NodeRef) -> bool {
    templates
        .items
        .get(n.0.index())
        .is_some_and(|t| (n.1 as usize) < t.nodes.len())
}

impl Extn {
    pub(crate) fn validate(
        &self,
        at: SectionTag,
        strs: usize,
        templates: &Templates,
    ) -> Result<(), PackageError> {
        let bad = |what: &str| Err(PackageError::invalid(at, what));
        for (what, list) in [
            ("slots", &self.slots),
            ("parts", &self.parts),
            ("screens", &self.screens),
        ] {
            if list.iter().any(|(n, _)| n.index() >= strs)
                || list.windows(2).any(|w| w[0].0 >= w[1].0)
            {
                return bad(&format!(
                    "{what} are not ascending unique names of the string table"
                ));
            }
            if list.iter().any(|(_, n)| !node_ok(templates, *n)) {
                return bad(&format!("a node of {what} does not exist"));
            }
        }
        if self.islands.iter().any(|i| i.id.index() >= strs)
            || self.islands.windows(2).any(|w| w[0].id >= w[1].id)
        {
            return bad("islands are not ascending unique names of the string table");
        }
        for island in &self.islands {
            if !node_ok(templates, island.node) {
                return bad("the node of an island does not exist");
            }
            let nodes = templates.items[island.node.0.index()].nodes.len();
            let keys = &island.keys;
            if keys
                .iter()
                .any(|(k, n)| k.index() >= strs || *n as usize >= nodes)
                || keys.windows(2).any(|w| w[0].0 >= w[1].0)
            {
                return bad(
                    "the keys of an island are not ascending unique names of existing nodes",
                );
            }
        }
        if self.hotkeys.iter().any(|h| {
            h.code.index() >= strs || h.mods & !hotkey_mods::ALL != 0 || !node_ok(templates, h.node)
        }) {
            return bad("a hotkey has an unknown name, modifier bit or node");
        }
        let key = |h: &Hotkey| (h.code, h.mods, h.node);
        if self.hotkeys.windows(2).any(|w| key(&w[0]) >= key(&w[1])) {
            return bad("hotkeys are not ascending and unique");
        }
        if self
            .tooltips
            .iter()
            .any(|(n, t)| !node_ok(templates, *n) || t.index() >= templates.items.len())
            || self.tooltips.windows(2).any(|w| w[0].0 >= w[1].0)
        {
            return bad("tooltips are not ascending unique existing nodes with existing templates");
        }
        Ok(())
    }
}
