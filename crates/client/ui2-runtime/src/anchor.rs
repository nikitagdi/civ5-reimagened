//! Map anchors: chunks of the `map` layer that follow objects of the world (`data-anchor="city:{{c.id}}"`).
//!
//! A node with an anchor binding is a chunk root (a child of a layer root, usually an instance of a `for`).
//! It is laid out once like any box; its chunk is drawn in its own space, whose origin is the origin of the
//! node's containing block, and placed on the screen by an anchor slot of the scene
//! (`screen = pos + scale * chunk`). The binding gives the anchor its object (kind and key); new anchors,
//! changed objects and removed anchors are handed to the host in [`FrameOutput::anchor_changes`]. The host
//! projects the objects with its camera and calls [`Runtime::set_anchor_screen`], which writes the scene's
//! anchor table and the placement hit-testing uses: no style, layout or draw list work.
//!
//! [`FrameOutput::anchor_changes`]: crate::FrameOutput::anchor_changes

use std::collections::HashMap;

use glam::{Affine2, Vec2};
use ui2_draw::{AnchorSlot, AnchorXform, Scene};
use ui2_model::Value;
use ui2_package::profile::action::AnchorKind;

use crate::Runtime;
use crate::arena::{Arena, Dirty, NodeId};
use crate::error::{ErrorCode, ErrorSink, UiError};
use crate::instantiate;

/// The identity of a map anchor for the host: fixed while its node lives, never used again after.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AnchorId(pub u32);

/// Where the camera puts an anchor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnchorScreen {
    /// The anchor.
    pub id: AnchorId,
    /// The screen point of the anchor's origin, device px.
    pub pos: Vec2,
    /// The scale of its chunk (1: as laid out).
    pub scale: f32,
    /// False: nothing of it is drawn or hit (off screen, hidden by the world).
    pub visible: bool,
}

/// One change for the host: a new anchor or a new object of an anchor (its kind and key), or a removed
/// anchor (`Value::None`).
pub type AnchorChange = (AnchorId, AnchorKind, Value);

/// What an anchor binding asked: its node, the object's kind and its key.
pub(crate) type AnchorUpdate = (NodeId, AnchorKind, Value);

#[derive(Debug, Clone)]
struct Entry {
    id: AnchorId,
    node: NodeId,
    kind: AnchorKind,
    slot: Option<AnchorSlot>,
    /// The last placement: position (device px), scale, visible.
    screen: Option<(Vec2, f32, bool)>,
}

/// The anchors of a runtime.
#[derive(Debug, Default)]
pub(crate) struct Anchors {
    /// By the slot index of the anchor's node (its chunk id).
    by_node: HashMap<u32, Entry>,
    ids: HashMap<AnchorId, u32>,
    next: u32,
    changes: Vec<AnchorChange>,
}

impl Anchors {
    /// Takes the anchor bindings' results and the removed nodes into the table: new anchors get a slot of
    /// the scene (their chunks are built again with it), removed ones give theirs back. Called by the
    /// frame before painting; it does nothing when nothing was asked and no chunk went away.
    pub fn sync(
        &mut self,
        arena: &mut Arena,
        pkg: &ui2_package::Package,
        scene: &mut Scene,
        updates: Vec<AnchorUpdate>,
        errors: &mut ErrorSink,
    ) {
        if updates.is_empty() && !arena.has_removed_chunks() {
            return;
        }
        // Anchors whose node is gone (a removed chunk root is in the removed chunk list until painting).
        let gone: Vec<u32> = arena
            .removed_chunks()
            .iter()
            .map(|c| c.0)
            .filter(|c| self.by_node.get(c).is_some_and(|e| !arena.contains(e.node)))
            .collect();
        for c in gone {
            self.drop_entry(c, scene);
        }
        for (node, kind, value) in updates {
            let Some(n) = arena.get(node) else {
                continue;
            };
            if n.chunk != node.index() {
                let path = instantiate::node_path(arena, pkg, node);
                errors.report(UiError::at(
                    &path,
                    ErrorCode::Bind,
                    "an anchor must be a child of the map layer's root",
                ));
                continue;
            }
            let key = node.index();
            if let Some(e) = self.by_node.get_mut(&key) {
                if e.node == node {
                    e.kind = kind;
                    self.changes.push((e.id, kind, value));
                    continue;
                }
                // The slot index went to another node: the old anchor is gone.
                self.drop_entry(key, scene);
            }
            let slot = match scene.alloc_anchor() {
                Ok(s) => Some(s),
                Err(e) => {
                    let path = instantiate::node_path(arena, pkg, node);
                    errors.report(UiError::at(&path, ErrorCode::Paint, e.to_string()));
                    None
                }
            };
            // SIMPLIFIED(SIMP-229): anchors are drawn in the order they were made (their chunks are appended to the map layer), not sorted by depth
            let id = AnchorId(self.next);
            self.next = self.next.wrapping_add(1);
            self.ids.insert(id, key);
            self.by_node.insert(
                key,
                Entry {
                    id,
                    node,
                    kind,
                    slot,
                    screen: None,
                },
            );
            self.changes.push((id, kind, value));
            arena.mark(node, Dirty::PAINT);
        }
    }

    fn drop_entry(&mut self, key: u32, scene: &mut Scene) {
        if let Some(e) = self.by_node.remove(&key) {
            self.ids.remove(&e.id);
            if let Some(slot) = e.slot {
                scene.free_anchor(slot);
            }
            self.changes.push((e.id, e.kind, Value::None));
        }
    }

    /// The changes since the last call.
    pub fn take_changes(&mut self) -> Vec<AnchorChange> {
        std::mem::take(&mut self.changes)
    }

    fn entry(&self, node: NodeId) -> Option<&Entry> {
        self.by_node.get(&node.index()).filter(|e| e.node == node)
    }

    /// For a chunk root: `None` when it is no anchor, `Some(slot)` when it is (`Some(None)` while it has
    /// no slot: its chunk draws nothing).
    pub fn slot(&self, node: NodeId) -> Option<Option<AnchorSlot>> {
        self.entry(node).map(|e| e.slot)
    }

    /// The transform from an anchor's layout space (page CSS px, `origin` being the origin of its
    /// containing block) to the screen in CSS px, while it is placed and visible.
    pub fn placement(&self, node: NodeId, origin: Vec2, k: f32) -> Option<Affine2> {
        let (pos, scale, visible) = self.entry(node)?.screen?;
        if !visible || k <= 0.0 || !scale.is_finite() || scale <= 0.0 {
            return None;
        }
        Some(
            Affine2::from_translation(pos / k)
                * Affine2::from_scale(Vec2::splat(scale))
                * Affine2::from_translation(-origin),
        )
    }

    /// Writes placements: the scene's anchor table and the placement hit-testing uses.
    pub fn set_screen(&mut self, upd: &[AnchorScreen], scene: &mut Scene) {
        for u in upd {
            let Some(e) = self
                .ids
                .get(&u.id)
                .and_then(|key| self.by_node.get_mut(key))
            else {
                continue;
            };
            e.screen = Some((u.pos, u.scale, u.visible));
            if let Some(slot) = e.slot {
                scene.set_anchor(
                    slot,
                    AnchorXform {
                        pos: u.pos,
                        scale: u.scale,
                        visible: u.visible,
                    },
                );
            }
        }
    }

    /// The number of live anchors.
    pub fn len(&self) -> usize {
        self.by_node.len()
    }
}

impl Runtime {
    /// Places map anchors on the screen after the host projected them with its camera: writes the scene's
    /// anchor table (one record per anchor) and the placement hit-testing uses. Nothing is restyled, laid
    /// out or painted again. Unknown ids (anchors removed meanwhile) are ignored.
    pub fn set_anchor_screen(&mut self, upd: &[AnchorScreen], scene: &mut Scene) {
        self.anchors.set_screen(upd, scene);
    }

    /// The number of live map anchors.
    pub fn anchor_count(&self) -> usize {
        self.anchors.len()
    }
}
