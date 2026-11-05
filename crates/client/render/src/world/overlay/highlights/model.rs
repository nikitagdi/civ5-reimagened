//! `HexModel`: a model on each cell of the set (`SampleHexModel` is `SelectedAniOneTurn.fxsxml`),
//! animated through `AnimService` (`world-anim`): every placement gets an `AnimTarget::Fx`, the
//! service steps its clip, and the bone reference of the frame goes with the placement to whoever
//! draws models (`world-objects`' path, which `Overlay` does not duplicate).
//!
//! The service is not reachable from a subsystem's `prepare` (it holds the services by shared
//! reference), so [`HexModels::sync`] is called by the owner of both, after the set changes.

use std::collections::HashMap;

use asset_format::LogicalId;

use super::{HighlightMesh, ModelPlacement};
use crate::anim::{AnimHandle, AnimService, AnimTarget, BoneRef};

/// The id of the model a style names: `md:` and the file name without its extension, lowercase.
pub fn model_id(file: &str) -> LogicalId {
    let stem = file.rsplit(['/', '\\']).next().unwrap_or(file);
    let stem = stem.rsplit_once('.').map_or(stem, |(s, _)| s);
    LogicalId::new(format!("md:{}", stem.to_lowercase()))
}

/// Adds the placements of `cells` to the mesh's list.
pub fn place(mesh: &mut HighlightMesh, cells: &[(i32, i32)], model: &str) {
    mesh.models
        .extend(cells.iter().map(|&(x, y)| ModelPlacement {
            model: model.to_string(),
            x,
            y,
        }));
}

/// A placement with its pose of this frame.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelInstance {
    pub model: LogicalId,
    pub x: i32,
    pub y: i32,
    pub bones: BoneRef,
    /// The clip is baked: a model that is not ready is not drawn in the pose of the bind.
    pub ready: bool,
}

/// The handles of the placements that have an animation.
#[derive(Debug, Default)]
pub struct HexModels {
    handles: HashMap<(LogicalId, i32, i32), AnimHandle>,
}

fn fx_id(model: &LogicalId, x: i32, y: i32) -> u32 {
    // FNV-1a over the model id and the cell: stable across runs.
    let mut h = 0x811C_9DC5u32;
    for b in model
        .as_str()
        .bytes()
        .chain(x.to_le_bytes())
        .chain(y.to_le_bytes())
    {
        h = (h ^ u32::from(b)).wrapping_mul(0x0100_0193);
    }
    h
}

impl HexModels {
    /// Attaches an animation to every placement whose model the service knows and lets go of the
    /// handles of the placements that are gone. Returns how many are attached.
    pub fn sync(&mut self, anim: &mut AnimService, placements: &[ModelPlacement]) -> usize {
        let want: Vec<(LogicalId, i32, i32)> = placements
            .iter()
            .map(|p| (model_id(&p.model), p.x, p.y))
            .collect();
        let gone: Vec<_> = self
            .handles
            .keys()
            .filter(|k| !want.contains(k))
            .cloned()
            .collect();
        for k in gone {
            if let Some(h) = self.handles.remove(&k) {
                anim.release(h);
            }
        }
        for key in want {
            if self.handles.contains_key(&key) {
                continue;
            }
            let target = AnimTarget::Fx {
                id: fx_id(&key.0, key.1, key.2),
            };
            if let Some(h) = anim.attach(&key.0, target) {
                self.handles.insert(key, h);
            }
        }
        self.handles.len()
    }

    /// The poses of this frame.
    pub fn instances(&self, anim: &AnimService) -> Vec<ModelInstance> {
        let mut out: Vec<ModelInstance> = self
            .handles
            .iter()
            .map(|((model, x, y), &h)| ModelInstance {
                model: model.clone(),
                x: *x,
                y: *y,
                bones: anim.bone_ref(h),
                ready: anim.is_ready(h),
            })
            .collect();
        out.sort_by(|a, b| (a.model.as_str(), a.x, a.y).cmp(&(b.model.as_str(), b.x, b.y)));
        out
    }

    pub fn len(&self) -> usize {
        self.handles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.handles.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::{ModelAnim, data::fixtures};

    fn service() -> AnimService {
        let mut s = AnimService::default();
        let (m, pack) = fixtures::model("selectedanioneturn", 4);
        let a = ModelAnim::new(&m, Some(pack), None).unwrap();
        s.register_model(LogicalId::new("md:selectedanioneturn"), a);
        s
    }

    fn at(x: i32, y: i32) -> ModelPlacement {
        ModelPlacement {
            model: "SelectedAniOneTurn.fxsxml".into(),
            x,
            y,
        }
    }

    #[test]
    fn the_id_is_the_file_name_in_lower_case_without_its_extension() {
        assert_eq!(
            model_id("SelectedAniOneTurn.fxsxml").as_str(),
            "md:selectedanioneturn"
        );
        assert_eq!(
            model_id("Assets/UI/Overlays/CenterAttack.fxsxml").as_str(),
            "md:centerattack"
        );
    }

    #[test]
    fn a_placement_gets_an_animation_and_loses_it_when_it_goes() {
        let mut anim = service();
        let mut models = HexModels::default();
        assert_eq!(models.sync(&mut anim, &[at(1, 1), at(2, 1)]), 2);
        assert_eq!(anim.stats().attached, 2);
        // The same placements again attach nothing new.
        assert_eq!(models.sync(&mut anim, &[at(1, 1), at(2, 1)]), 2);
        assert_eq!(anim.stats().attached, 2);
        assert_eq!(models.sync(&mut anim, &[at(2, 1)]), 1);
        assert_eq!(anim.stats().attached, 1);
        anim.step(0.1);
        let inst = models.instances(&anim);
        assert_eq!(inst.len(), 1);
        assert_eq!((inst[0].x, inst[0].y), (2, 1));
        assert!(inst[0].ready);
        models.sync(&mut anim, &[]);
        assert!(models.is_empty());
        assert_eq!(anim.stats().attached, 0);
    }

    #[test]
    fn a_model_the_service_does_not_know_is_not_attached() {
        let mut anim = AnimService::default();
        let mut models = HexModels::default();
        assert_eq!(models.sync(&mut anim, &[at(0, 0)]), 0);
    }

    #[test]
    fn two_cells_have_two_targets() {
        let m = model_id("a.fxsxml");
        assert_ne!(fx_id(&m, 1, 2), fx_id(&m, 2, 1));
    }
}
