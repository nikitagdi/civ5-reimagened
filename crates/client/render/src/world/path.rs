//! `MovementPath` subsystem (world-overlays 4.5).
//!
//! The input is [`PathInput`] (`set_path`). The path is drawn by the models of the original
//! (`Center*`, `Connector*`, `SelectedAni*`, `pieces.rs`): the pieces are given to the unit draw as
//! instances (`Unit::set_path_pieces`); B1's strips and diamonds are gone (SIMP-024). The turn
//! numbers are labels for the UI (`path_labels`, R-D5).

pub mod pieces;

use std::collections::BTreeMap;
use std::rc::Rc;

use asset_format::LogicalId;
use glam::Vec2;

use crate::anim::{AnimHandle, AnimService, AnimTarget, BoneRef};
use crate::entities::WorldLabel;
use crate::entities::highlight::{self, PathStep};
use crate::frame::{DrawContext, InitContext, PassSlot, PrepareContext, RenderSystem, SystemId};
use crate::static_model::StaticInstance;
use crate::terrain::TerrainSnapshot;
use crate::world::objects::instance::ObjectInstance;

/// The path of the shown unit, start first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PathInput {
    pub steps: Vec<PathStep>,
}

impl PathInput {
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

/// Counters of `?stats=1`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PathCounters {
    /// Times the labels were made.
    pub builds: u64,
    /// Steps of the shown path.
    pub steps: u32,
}

/// The path over the map.
#[derive(Default)]
pub struct MovementPath {
    ground: Option<Rc<TerrainSnapshot>>,
    path: PathInput,
    dirty: bool,
    /// Turn numbers on the shown path, for the UI.
    labels: Vec<WorldLabel>,
    counters: PathCounters,
    /// The animation of the pulsing marks (`SelectedAni*`), by piece.
    handles: Vec<(LogicalId, AnimHandle)>,
}

impl MovementPath {
    /// The relief the path lies on: a new map, the path is built again.
    pub fn set_ground(&mut self, ground: Rc<TerrainSnapshot>) {
        self.ground = Some(ground);
        self.dirty = true;
    }

    /// The path to show; an empty one clears it. The same path again does not build anything.
    pub fn set_path(&mut self, path: PathInput) {
        if self.path == path {
            return;
        }
        self.path = path;
        self.dirty = true;
    }

    pub fn path(&self) -> &PathInput {
        &self.path
    }

    /// Turn numbers on the shown path, for the UI (like `Renderer::entity_labels`).
    pub fn labels(&self) -> &[WorldLabel] {
        &self.labels
    }

    /// The pieces of the shown path on the ground, `ground` giving the relief's height at `(x, z)`.
    pub fn pieces(&self, ground: &dyn Fn(Vec2) -> f32) -> Vec<pieces::Piece> {
        match &self.ground {
            Some(g) => pieces::pieces(&self.path, &g.shape(), ground),
            None => Vec::new(),
        }
    }

    /// The models the shown path needs, sorted, once each.
    pub fn wanted_models(&self) -> Vec<LogicalId> {
        let flat = |_: Vec2| 0.0;
        let mut ids: Vec<LogicalId> = self.pieces(&flat).iter().map(|p| p.model()).collect();
        ids.sort();
        ids.dedup();
        ids
    }

    /// Attaches the animation of the marks to `anim` (a mark whose model the service does not know
    /// stands still) and gives back the handles of the marks that are gone.
    pub fn sync_models(&mut self, anim: &mut AnimService) {
        let flat = |_: Vec2| 0.0;
        let marks: Vec<LogicalId> = self
            .pieces(&flat)
            .iter()
            .filter(|p| p.kind == pieces::PieceKind::Selected)
            .map(|p| p.model())
            .collect();
        for (_, h) in self.handles.drain(..) {
            anim.release(h);
        }
        for (i, id) in marks.into_iter().enumerate() {
            if let Some(h) = anim.attach(&id, AnimTarget::PathNode { index: i as u16 }) {
                self.handles.push((id, h));
            }
        }
    }

    /// The instances of the pieces by model, for the unit draw. A mark whose clip is not baked yet is
    /// drawn in the pose of the bind, as a figure is.
    pub fn model_batches(
        &self,
        anim: &AnimService,
        ground: &dyn Fn(Vec2) -> f32,
    ) -> BTreeMap<LogicalId, Vec<ObjectInstance>> {
        let mut out: BTreeMap<LogicalId, Vec<ObjectInstance>> = BTreeMap::new();
        for p in self.pieces(ground) {
            let id = p.model();
            let mut o = ObjectInstance::from_static(&StaticInstance::new(
                p.position.to_array(),
                p.yaw,
                p.scale,
            ));
            if p.kind == pieces::PieceKind::Selected {
                match self.handles.iter().find(|(m, _)| *m == id) {
                    Some((_, h)) => o.bones = anim.bone_ref(*h),
                    None => o.bones = BoneRef::default(),
                }
            }
            out.entry(id).or_default().push(o);
        }
        out
    }

    /// Back to no map.
    pub fn clear(&mut self) {
        self.ground = None;
        self.labels.clear();
        self.dirty = false;
    }

    pub fn stats(&self) -> PathCounters {
        self.counters
    }

    fn rebuild(&mut self) {
        self.counters.steps = self.path.steps.len() as u32;
        self.counters.builds += 1;
        self.labels = match &self.ground {
            Some(ground) => highlight::path_labels(&self.path.steps, ground),
            None => Vec::new(),
        };
    }
}

impl RenderSystem for MovementPath {
    fn id(&self) -> SystemId {
        SystemId::MovementPath
    }

    /// The path draws as models through the unit draw, so it has no pass of its own.
    fn passes(&self) -> &'static [PassSlot] {
        &[]
    }

    fn init(&mut self, _cx: &mut InitContext<'_>) {}

    fn prepare(&mut self, _cx: &mut PrepareContext<'_>) {
        if std::mem::take(&mut self.dirty) {
            self.rebuild();
        }
    }

    fn record(&self, _slot: PassSlot, _cx: &mut DrawContext<'_, '_>) {}

    fn counters(&self) -> Vec<(&'static str, u64)> {
        let c = self.counters;
        vec![
            ("path.builds", c.builds),
            ("path.steps", u64::from(c.steps)),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(x: i32, y: i32, turn: i32) -> PathStep {
        PathStep {
            x,
            y,
            turn,
            flags: 0,
        }
    }

    #[test]
    fn the_same_path_again_builds_nothing() {
        let mut p = MovementPath::default();
        let path = PathInput {
            steps: vec![step(0, 0, 0), step(1, 0, 1)],
        };
        p.set_path(path.clone());
        assert!(p.dirty);
        p.dirty = false;
        p.set_path(path);
        assert!(!p.dirty);
        p.set_path(PathInput::default());
        assert!(p.dirty, "clearing is a change");
        assert!(p.path().is_empty());
    }
}
