//! Icon textures from the asset server (SIMP-012): the strategic-view
//! icons of the resources, improvements, encampments and ruins that are on the map. The renderer
//! says which textures the current plots need (`icon_textures_wanted`); they are fetched as they
//! become needed and handed over in batches, so the icon array is rebuilt once per batch. Without
//! assets nothing is fetched and no icon is drawn.

use std::collections::{HashMap, HashSet};

use assets::{AssetHandle, AssetLoader, AssetPoll, LogicalId};
use render::Renderer;
use render::asset_format::TextureBlob;

pub(crate) struct IconAssets {
    requested: HashSet<LogicalId>,
    pending: Vec<(LogicalId, AssetHandle)>,
    /// Arrived, not handed over yet: the batch grows until nothing is pending.
    arrived: HashMap<LogicalId, TextureBlob>,
}

impl IconAssets {
    pub(crate) fn new() -> Self {
        Self {
            requested: HashSet::new(),
            pending: Vec::new(),
            arrived: HashMap::new(),
        }
    }

    /// Advances the loading; call once per frame.
    pub(crate) fn update(&mut self, loader: Option<&mut AssetLoader>, renderer: &mut Renderer) {
        let Some(loader) = loader else {
            return;
        };
        let profile = renderer.platform_profile();
        for id in renderer.icon_textures_wanted() {
            if self.requested.insert(id.clone()) {
                let h = loader.request(id.clone(), &profile);
                self.pending.push((id, h));
            }
        }
        let arrived = &mut self.arrived;
        self.pending.retain(|(id, h)| match loader.poll(*h) {
            AssetPoll::Pending => true,
            AssetPoll::Ready(bytes) => {
                match TextureBlob::decode(&bytes) {
                    Ok(t) => {
                        arrived.insert(id.clone(), t);
                    }
                    Err(e) => {
                        tracing::warn!(id = id.as_str(), "bad icon texture blob: {e}");
                        renderer.fail_icon_texture(id.clone());
                    }
                }
                false
            }
            AssetPoll::Failed(e) => {
                tracing::warn!(id = id.as_str(), "icon texture unavailable ({e})");
                renderer.fail_icon_texture(id.clone());
                false
            }
        });
        if self.pending.is_empty() && !self.arrived.is_empty() {
            renderer.add_icon_textures(std::mem::take(&mut self.arrived));
        }
    }
}
