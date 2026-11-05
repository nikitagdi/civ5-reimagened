//! Terrain textures from the asset server: fetches `table:art-b1`,
//! then the textures it names, and hands them to the renderer as a texture array. The same table
//! names the art of the features: the tree models with their textures and the decal
//! textures; those are fetched next to the terrain textures and handed over together.
//! Without assets (no server, no manifest, no table) nothing happens and the terrain keeps its
//! flat colours, without trees and decals.

use std::collections::HashMap;

use assets::{AssetHandle, AssetLoader, AssetPoll, LogicalId};
use render::asset_format::{ArtTable, ModelBlob, ModelBlobV2, TextureBlob};
use render::terrain::fog::CLOUD_LAYERS;
use render::terrain::water::WATER_BUMP_TEXTURE;
use render::world::fow::puffs::PUFF_TEXTURE;
use render::world::minimap::TEXTURES as MINIMAP_TEXTURES;
use render::world::overlay::grid::HEX_GRID_TEXTURE;
use render::{FeatureArtSet, LayerPlan, Renderer, TerrainArtSlots};

/// Logical id of the shared art table.
pub(crate) const ART_TABLE_ID: &str = "table:art-b1";
/// The art table of `assets-b2`.
pub(crate) const ART_TABLE_B2_ID: &str = "tb:art-b2";

enum Stage {
    /// Waiting for a renderer, which knows which texture variants the device can sample.
    Start,
    Table(AssetHandle),
    Textures {
        art: TerrainArtSlots,
        pending: Vec<(LogicalId, AssetHandle)>,
        loaded: HashMap<LogicalId, TextureBlob>,
    },
    Done,
}

/// Loading of the feature art: the tree models first (they name their textures), then all
/// textures.
enum FeatureStage {
    /// Waiting for the art table.
    Waiting,
    /// B1's table is not in the pack: the trees come from the table of `assets-b2`.
    B2Table(AssetHandle),
    Fetching(Box<FeatureFetch>),
    Done,
}

struct FeatureFetch {
    art: FeatureArtSet,
    models: Vec<(LogicalId, AssetHandle)>,
    model_blobs: HashMap<LogicalId, ModelBlob>,
    textures: Vec<(LogicalId, AssetHandle)>,
    texture_blobs: HashMap<LogicalId, TextureBlob>,
    /// Every texture asked for, so a texture used twice is fetched once.
    requested: Vec<LogicalId>,
}

/// Loading of the three cloud textures of the fog, the bump map of the water, the texture of the hex
/// grid and the seven of the minimap (they have no art-table record).
enum CloudStage {
    /// Waiting for the art table, which says the assets are there.
    Waiting,
    Fetching {
        pending: Vec<(LogicalId, AssetHandle)>,
        loaded: HashMap<LogicalId, TextureBlob>,
    },
    Done,
}

pub(crate) struct TerrainAssets {
    stage: Stage,
    features: FeatureStage,
    clouds: CloudStage,
}

impl TerrainAssets {
    pub(crate) fn new() -> Self {
        Self {
            stage: Stage::Start,
            features: FeatureStage::Waiting,
            clouds: CloudStage::Waiting,
        }
    }

    /// The terrain textures are in the renderer, or will never come (no assets, a failed table):
    /// what is on screen now is the terrain as it stays.
    pub(crate) fn textures_settled(&self) -> bool {
        matches!(self.stage, Stage::Done)
    }

    /// The textures, the feature art and the cloud textures are in the renderer, or will never come.
    pub(crate) fn idle(&self) -> bool {
        matches!(self.stage, Stage::Done)
            && matches!(self.features, FeatureStage::Done)
            && matches!(self.clouds, CloudStage::Done)
    }

    /// Advances the loading; call once per frame with the loader (if assets are on).
    /// `boot_ready`: the boot group is in (or there is none), so the world settings are.
    pub(crate) fn update(
        &mut self,
        loader: Option<&mut AssetLoader>,
        renderer: &mut Renderer,
        boot_ready: bool,
    ) {
        let Some(loader) = loader else {
            if !matches!(self.stage, Stage::Done) {
                tracing::info!("assets are disabled; the terrain stays flat colours");
                self.stage = Stage::Done;
                self.features = FeatureStage::Done;
                self.clouds = CloudStage::Done;
            }
            return;
        };
        let stage = std::mem::replace(&mut self.stage, Stage::Done);
        self.stage = match stage {
            Stage::Start => {
                let profile = renderer.platform_profile();
                Stage::Table(loader.request(LogicalId::new(ART_TABLE_ID), &profile))
            }
            Stage::Table(h) => {
                let (stage, art) = poll_table(loader, h, renderer);
                self.features = match art {
                    TableOutcome::Pending => FeatureStage::Waiting,
                    TableOutcome::Art(art) => start_features(loader, art, renderer),
                    // A pack of B2 has no `table:art-b1`; its trees are in `tb:art-b2`.
                    TableOutcome::Missing => FeatureStage::B2Table(loader.request(
                        LogicalId::new(ART_TABLE_B2_ID),
                        &renderer.platform_profile(),
                    )),
                    TableOutcome::Bad => FeatureStage::Done,
                };
                stage
            }
            Stage::Textures {
                art,
                pending,
                loaded,
            } => poll_textures(loader, art, pending, loaded, renderer),
            Stage::Done => Stage::Done,
        };
        self.features = match std::mem::replace(&mut self.features, FeatureStage::Done) {
            FeatureStage::Fetching(fetch) => poll_features(loader, fetch, renderer),
            FeatureStage::B2Table(h) => poll_b2_table(loader, h, renderer),
            other => other,
        };
        // The loose textures need the manifest and the world settings (the highlight styles), not
        // the art table of B1: the B2 sets have none.
        if matches!(self.clouds, CloudStage::Waiting) && boot_ready {
            match loader.manifest() {
                Some(_) => self.clouds = start_clouds(loader, renderer),
                None if loader.manifest_error().is_some() => self.clouds = CloudStage::Done,
                None => {}
            }
        }
        self.clouds = poll_clouds(
            loader,
            std::mem::replace(&mut self.clouds, CloudStage::Done),
            renderer,
        );
    }
}

/// The id of the B1 blobs of a texture (`tex:` where the B2 sets have `tx:`).
fn legacy_id(id: &LogicalId) -> Option<LogicalId> {
    id.as_str()
        .strip_prefix("tx:")
        .map(|file| LogicalId::new(format!("tex:{file}")))
}

/// Requests the cloud textures of the fog and the rest of the loose textures. The textures are kept
/// under their `tx:` ids; a set that has only the `tex:` one (B1) is asked for that.
fn start_clouds(loader: &mut AssetLoader, renderer: &Renderer) -> CloudStage {
    let profile = renderer.platform_profile();
    let ids: Vec<LogicalId> = CLOUD_LAYERS
        .iter()
        .map(|l| l.texture)
        .chain([PUFF_TEXTURE, WATER_BUMP_TEXTURE, HEX_GRID_TEXTURE])
        .chain(MINIMAP_TEXTURES.iter().map(|(id, _)| *id))
        .map(LogicalId::new)
        // The textures of the highlight styles and of the selection ring (WOV5).
        .chain(renderer.highlight_texture_ids())
        // The arrows of the trade routes (R5.9).
        .chain(renderer.trade_texture_ids())
        .collect();
    let pending = ids
        .into_iter()
        .map(|id| {
            let on_offer = |i: &LogicalId| {
                loader
                    .manifest()
                    .is_some_and(|m| m.resolve(i, &profile).is_some())
            };
            let ask = match legacy_id(&id) {
                Some(old) if !on_offer(&id) && on_offer(&old) => old,
                _ => id.clone(),
            };
            let h = loader.request(ask, &profile);
            (id, h)
        })
        .collect();
    CloudStage::Fetching {
        pending,
        loaded: HashMap::new(),
    }
}

/// Hands the cloud textures to the renderer once all have arrived; any that fails leaves the
/// unrevealed cells the flat fog colour.
fn poll_clouds(loader: &mut AssetLoader, stage: CloudStage, renderer: &mut Renderer) -> CloudStage {
    let CloudStage::Fetching {
        mut pending,
        mut loaded,
    } = stage
    else {
        return stage;
    };
    pending.retain(|(id, h)| match loader.poll(*h) {
        AssetPoll::Pending => true,
        AssetPoll::Ready(bytes) => {
            match TextureBlob::decode(&bytes) {
                Ok(t) => {
                    loaded.insert(id.clone(), t);
                }
                Err(e) => tracing::warn!(id = id.as_str(), "bad cloud or water blob: {e}"),
            }
            false
        }
        AssetPoll::Failed(e) => {
            tracing::info!(id = id.as_str(), "cloud or water texture unavailable ({e})");
            false
        }
    });
    if !pending.is_empty() {
        return CloudStage::Fetching { pending, loaded };
    }
    let drawn = renderer.set_fog_clouds(&loaded);
    let ripples = renderer.set_water_texture(&loaded);
    let grid = renderer.set_hex_grid_texture(&loaded);
    let minimap = renderer.set_minimap_textures(&loaded);
    let highlights = renderer.set_highlight_textures(&loaded);
    let ring = renderer.set_ring_textures(&loaded);
    let trade = renderer.set_trade_textures(&loaded);
    tracing::info!(
        clouds = drawn,
        water_bump = ripples,
        hex_grid = grid,
        minimap_textures = minimap,
        highlight_textures = highlights,
        ring_textures = ring,
        trade_textures = trade,
        "fog clouds, water, grid, minimap, highlights, ring and trade arrows"
    );
    CloudStage::Done
}

/// What the poll of B1's art table gave the feature art.
enum TableOutcome {
    Pending,
    Art(FeatureArtSet),
    /// The pack has no such table (a pack of B2).
    Missing,
    Bad,
}

fn poll_table(
    loader: &mut AssetLoader,
    handle: AssetHandle,
    renderer: &Renderer,
) -> (Stage, TableOutcome) {
    let bytes = match loader.poll(handle) {
        AssetPoll::Pending => return (Stage::Table(handle), TableOutcome::Pending),
        AssetPoll::Ready(b) => b,
        AssetPoll::Failed(e) => {
            tracing::info!("{ART_TABLE_ID} unavailable ({e}); the terrain stays flat colours");
            return (Stage::Done, TableOutcome::Missing);
        }
    };
    let table = match ArtTable::decode(&bytes) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("{ART_TABLE_ID} is not a valid art table: {e}");
            return (Stage::Done, TableOutcome::Bad);
        }
    };
    let art = TerrainArtSlots::from_table(&table);
    let features = FeatureArtSet::from_table(&table);
    let profile = renderer.platform_profile();
    let pending: Vec<_> = art
        .textures()
        .into_iter()
        .map(|id| {
            let h = loader.request(id.clone(), &profile);
            (id, h)
        })
        .collect();
    tracing::info!(
        textures = pending.len(),
        "art table loaded, fetching terrain textures"
    );
    let stage = Stage::Textures {
        art,
        pending,
        loaded: HashMap::new(),
    };
    (stage, TableOutcome::Art(features))
}

/// The trees of `tb:art-b2` when the pack has no B1 table: their models and overlays go through the
/// same fetch as B1's feature art.
fn poll_b2_table(
    loader: &mut AssetLoader,
    handle: AssetHandle,
    renderer: &Renderer,
) -> FeatureStage {
    let bytes = match loader.poll(handle) {
        AssetPoll::Pending => return FeatureStage::B2Table(handle),
        AssetPoll::Ready(b) => b,
        AssetPoll::Failed(e) => {
            tracing::info!("{ART_TABLE_B2_ID} unavailable ({e}); no trees");
            return FeatureStage::Done;
        }
    };
    match ArtTable::decode(&bytes) {
        Ok(table) => {
            let art = FeatureArtSet::from_b2_table(&table);
            if art.is_empty() {
                tracing::info!("{ART_TABLE_B2_ID} has no trees");
                return FeatureStage::Done;
            }
            start_features(loader, art, renderer)
        }
        Err(e) => {
            tracing::error!("{ART_TABLE_B2_ID} is not a valid art table: {e}");
            FeatureStage::Done
        }
    }
}

/// A tree model as B1's forest takes it: a v1 blob as it is, a v2 blob (`assets-b2`) made rigid.
fn tree_model(bytes: &[u8]) -> Result<ModelBlob, String> {
    match ModelBlob::decode(bytes) {
        Ok(m) => Ok(m),
        Err(v1) => ModelBlobV2::decode(bytes)
            .and_then(|m| m.to_static_v1())
            .map_err(|v2| format!("{v1}; as v2: {v2}")),
    }
}

fn poll_textures(
    loader: &mut AssetLoader,
    art: TerrainArtSlots,
    mut pending: Vec<(LogicalId, AssetHandle)>,
    mut loaded: HashMap<LogicalId, TextureBlob>,
    renderer: &mut Renderer,
) -> Stage {
    pending.retain(|(id, h)| match loader.poll(*h) {
        AssetPoll::Pending => true,
        AssetPoll::Ready(bytes) => {
            match TextureBlob::decode(&bytes) {
                Ok(t) => {
                    loaded.insert(id.clone(), t);
                }
                Err(e) => tracing::warn!(id = id.as_str(), "bad terrain texture blob: {e}"),
            }
            false
        }
        AssetPoll::Failed(e) => {
            tracing::warn!(id = id.as_str(), "terrain texture unavailable ({e})");
            false
        }
    });
    if !pending.is_empty() {
        return Stage::Textures {
            art,
            pending,
            loaded,
        };
    }
    let plan = LayerPlan::build(&art, &loaded);
    let layers = renderer.set_terrain_textures(&plan, &loaded);
    tracing::info!(
        layers,
        mips = plan.mip_count,
        left_out = plan.rejected.len(),
        "terrain textures in use"
    );
    Stage::Done
}

/// Requests the tree models and the decal textures.
fn start_features(
    loader: &mut AssetLoader,
    art: FeatureArtSet,
    renderer: &Renderer,
) -> FeatureStage {
    let profile = renderer.platform_profile();
    let models: Vec<_> = art
        .tree_models()
        .into_iter()
        .map(|id| {
            let h = loader.request(id.clone(), &profile);
            (id, h)
        })
        .collect();
    let requested = art.decal_textures();
    let textures = requested
        .iter()
        .map(|id| (id.clone(), loader.request(id.clone(), &profile)))
        .collect();
    tracing::info!(
        models = models.len(),
        decals = requested.len(),
        "fetching feature art"
    );
    FeatureStage::Fetching(Box::new(FeatureFetch {
        art,
        models,
        model_blobs: HashMap::new(),
        textures,
        texture_blobs: HashMap::new(),
        requested,
    }))
}

/// Polls the models and textures; when everything has arrived (or failed) hands the lot to the
/// renderer. A model that arrives asks for its own textures, so the fetch grows once.
fn poll_features(
    loader: &mut AssetLoader,
    mut fetch: Box<FeatureFetch>,
    renderer: &mut Renderer,
) -> FeatureStage {
    let profile = renderer.platform_profile();
    let f = &mut *fetch;
    let mut wanted: Vec<LogicalId> = Vec::new();
    f.models.retain(|(id, h)| match loader.poll(*h) {
        AssetPoll::Pending => true,
        AssetPoll::Ready(bytes) => {
            match tree_model(&bytes) {
                Ok(m) => {
                    wanted.extend(m.materials.iter().filter_map(|mat| mat.diffuse.clone()));
                    f.model_blobs.insert(id.clone(), m);
                }
                Err(e) => tracing::warn!(id = id.as_str(), "bad tree model blob: {e}"),
            }
            false
        }
        AssetPoll::Failed(e) => {
            tracing::warn!(id = id.as_str(), "tree model unavailable ({e})");
            false
        }
    });
    for id in wanted {
        if !f.requested.contains(&id) {
            f.requested.push(id.clone());
            let h = loader.request(id.clone(), &profile);
            f.textures.push((id, h));
        }
    }
    f.textures.retain(|(id, h)| match loader.poll(*h) {
        AssetPoll::Pending => true,
        AssetPoll::Ready(bytes) => {
            match TextureBlob::decode(&bytes) {
                Ok(t) => {
                    f.texture_blobs.insert(id.clone(), t);
                }
                Err(e) => tracing::warn!(id = id.as_str(), "bad feature texture blob: {e}"),
            }
            false
        }
        AssetPoll::Failed(e) => {
            tracing::warn!(id = id.as_str(), "feature texture unavailable ({e})");
            false
        }
    });
    if !f.models.is_empty() || !f.textures.is_empty() {
        return FeatureStage::Fetching(fetch);
    }
    renderer.set_feature_art(fetch.art, &fetch.model_blobs, &fetch.texture_blobs);
    FeatureStage::Done
}
