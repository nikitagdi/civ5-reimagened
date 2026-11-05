//! Cell under a click: keeps the pick index of the current terrain mesh.

use render::{Pick, PickIndex, Ray};

use crate::bridge::WorldBridge;

/// Pick index of the bridge's mesh, rebuilt when the bridge builds a new mesh.
#[derive(Debug, Default)]
pub struct Picker {
    index: Option<PickIndex>,
    mesh_builds: u32,
}

impl Picker {
    /// Cell and hit point under `ray`; `None` before the first terrain or if the ray misses.
    pub fn pick(&mut self, bridge: &WorldBridge, ray: &Ray) -> Option<Pick> {
        let (snapshot, mesh) = bridge.terrain()?;
        if self.index.is_none() || self.mesh_builds != bridge.mesh_builds() {
            self.index = Some(PickIndex::new(snapshot.shape(), mesh));
            self.mesh_builds = bridge.mesh_builds();
        }
        self.index.as_ref()?.pick(ray)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use render::glam::Vec3;

    #[test]
    fn nothing_to_pick_before_terrain() {
        let bridge = WorldBridge::new();
        let ray = Ray {
            origin: Vec3::new(0.0, 100.0, 0.0),
            dir: Vec3::NEG_Y,
        };
        assert_eq!(Picker::default().pick(&bridge, &ray), None);
    }
}
