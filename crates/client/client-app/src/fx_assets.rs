//! The blobs of the particles (R5.12): `fx:index`, the `fx:` effects and their `tx:` textures,
//! loaded when the `Particle` subsystem names them ([`render::world::particle::Particle::wanted`]).
//! Without the set `b2-fx` in the manifest the requests fail once and nothing is drawn.

use std::collections::HashSet;

use assets::{AssetHandle, AssetLoader, AssetPoll, LogicalId};
use render::Renderer;
use render::asset_format::{FX_INDEX_ID, FxEffect, FxIndex, TextureBlob};

#[derive(Default)]
pub struct FxAssets {
    requested: HashSet<LogicalId>,
    pending: Vec<(LogicalId, AssetHandle)>,
}

impl FxAssets {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, loader: Option<&mut AssetLoader>, renderer: &mut Renderer) {
        let Some(loader) = loader else { return };
        let profile = renderer.platform_profile();
        let particles = renderer.particles_mut();
        let wanted = particles.wanted();
        let mut asked = Vec::new();
        for id in wanted {
            if self.requested.insert(id.clone()) {
                let h = loader.request(id.clone(), &profile);
                asked.push(id.clone());
                self.pending.push((id, h));
            }
        }
        particles.mark_requested(&asked);
        self.pending.retain(|(id, h)| match loader.poll(*h) {
            AssetPoll::Pending => true,
            AssetPoll::Ready(bytes) => {
                let s = id.as_str();
                if s == FX_INDEX_ID {
                    match FxIndex::decode(&bytes) {
                        Ok(i) => particles.set_index(i),
                        Err(e) => tracing::warn!(id = s, "bad fx index: {e}"),
                    }
                } else if s.starts_with("fx:") {
                    match FxEffect::decode(&bytes) {
                        Ok(fx) => particles.add_effect(id.clone(), fx),
                        Err(e) => tracing::warn!(id = s, "bad fx blob: {e}"),
                    }
                } else {
                    match TextureBlob::decode(&bytes) {
                        Ok(t) => particles.add_texture(id.clone(), t),
                        Err(e) => tracing::warn!(id = s, "bad texture blob: {e}"),
                    }
                }
                false
            }
            AssetPoll::Failed(e) => {
                tracing::info!(id = id.as_str(), "particle blob unavailable ({e})");
                false
            }
        });
    }
}
