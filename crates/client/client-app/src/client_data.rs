//! The data blobs of the asset set the client reads once (set `client-data` of `tools/asset-pipeline`): the mouse
//! cursors (`data:cursors`, handed to the app, which makes the window cursors on the event loop) and the unit blend
//! graphs (`data:dge-graphs`, installed into `render::anim::dge`; the unit loading of `entity_assets` waits for it, so
//! the machines of the figures are built with it).
//!
//! An asset set without them is fine: the platform cursor stays, and the units play the clips of their action.

use assets::{AssetHandle, AssetLoader, AssetPoll, LogicalId, PlatformProfile};
use render::asset_format::{CURSORS_ID, CursorSet};

enum Stage {
    Waiting,
    Loading(AssetHandle),
    Done,
}

pub(crate) struct ClientData {
    cursors: Stage,
}

impl ClientData {
    pub(crate) fn new() -> Self {
        Self {
            cursors: Stage::Waiting,
        }
    }

    /// Advances the loading; returns the cursor set once, when it has arrived.
    pub(crate) fn update(&mut self, loader: &mut AssetLoader) -> Option<CursorSet> {
        let profile = PlatformProfile::uncompressed_only();
        match &self.cursors {
            Stage::Done => None,
            Stage::Waiting => {
                if loader.manifest_error().is_some() {
                    self.cursors = Stage::Done;
                    return None;
                }
                let manifest = loader.manifest()?;
                let id = LogicalId::new(CURSORS_ID);
                if manifest.resolve(&id, &profile).is_none() {
                    tracing::info!("the asset set has no {CURSORS_ID}: the platform cursor stays");
                    self.cursors = Stage::Done;
                    return None;
                }
                self.cursors = Stage::Loading(loader.request(id, &profile));
                None
            }
            Stage::Loading(h) => match loader.poll(*h) {
                AssetPoll::Pending => None,
                AssetPoll::Failed(e) => {
                    tracing::warn!("{CURSORS_ID} unavailable ({e}): the platform cursor stays");
                    self.cursors = Stage::Done;
                    None
                }
                AssetPoll::Ready(bytes) => {
                    self.cursors = Stage::Done;
                    match CursorSet::decode(&bytes) {
                        Ok(set) => Some(set),
                        Err(e) => {
                            tracing::warn!("{CURSORS_ID} is not a cursor set ({e})");
                            None
                        }
                    }
                }
            },
        }
    }
}
