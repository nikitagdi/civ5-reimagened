//! Style selection: a node's record is `StyleSet::get(cell, mask)` of its template's set.

use ui2_package::Styles;

use crate::arena::{Arena, NodeId};

/// What style selection depends on besides the node: the viewport cell.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StyleEnv {
    /// The viewport cell (`Styles::cell` of the logical viewport).
    pub cell: u8,
}

/// Selects the record of a new node.
pub(crate) fn attach(arena: &mut Arena, styles: &Styles, env: &StyleEnv, id: NodeId) {
    let Some(node) = arena.get_mut(id) else {
        return;
    };
    if let Some(set) = node.set.and_then(|s| styles.sets.get(s.index())) {
        node.style = set.get(env.cell, 0);
    }
}
