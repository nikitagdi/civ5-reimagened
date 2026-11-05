//! Models of units and cities from the asset server: fetches
//! `table:art-b1`, then, one by one, the models the renderer says the current world needs, and
//! the textures those models name. A model that cannot be had is reported as failed and shows as
//! a marker (SIMP-011). Without assets everything is a marker.
//!
//! Beside it runs the loading of the units of `assets-b2` (world-objects WOB2): `tb:art-b2`, and the
//! models of the figures the renderer asks for with the animation pack, the idle automaton and the
//! textures each names. A pack that has no such table (B1's) leaves the units to the flow above. Before the table
//! the blend graphs of the units (`data:dge-graphs`) are installed into the renderer's animation, so every machine
//! is built with them; a set without them leaves the units on the clips of their actions.

use std::collections::{HashMap, HashSet};

use assets::{AssetHandle, AssetLoader, AssetPoll, LogicalId};
use render::Renderer;
use render::asset_format::{
    AnimPack, ArtTable, DGE_GRAPHS_ID, IdleFsm, ModelBlob, ModelBlobV2, TextureBlob,
};
use render::world::unit::draw::UnitDraw;

use crate::terrain_assets::{ART_TABLE_B2_ID, ART_TABLE_ID};

enum Stage {
    Start,
    /// The unit blend graphs (`data:dge-graphs`), before the figures' machines are built (B2 only).
    Graphs(AssetHandle),
    Table(AssetHandle),
    Running,
    Done,
}

/// A model that has arrived and waits for its textures.
struct Partial {
    id: LogicalId,
    blob: ModelBlob,
    textures: Vec<(LogicalId, AssetHandle)>,
    loaded: HashMap<LogicalId, TextureBlob>,
}

/// A model of `assets-b2` that has arrived and waits for its animation data and textures.
struct PartialB2 {
    id: LogicalId,
    blob: ModelBlobV2,
    pack: Option<AssetHandle>,
    fsm: Option<AssetHandle>,
    pack_blob: Option<AnimPack>,
    fsm_blob: Option<IdleFsm>,
    textures: Vec<(LogicalId, AssetHandle)>,
    loaded: HashMap<LogicalId, TextureBlob>,
}

/// The loading of the units of `assets-b2`.
struct B2 {
    stage: Stage,
    requested: HashSet<LogicalId>,
    models: Vec<(LogicalId, AssetHandle)>,
    partial: Vec<PartialB2>,
    /// The placement masks of the trees (`forest_tile1..N`) being fetched, and those that arrived.
    masks: Vec<(LogicalId, AssetHandle)>,
    mask_ids: Vec<LogicalId>,
    mask_blobs: HashMap<LogicalId, TextureBlob>,
    /// The fields of the city layout system (`tx:df_*`, `tx:tf_*`) being fetched, and those that
    /// arrived (by file name).
    layout: Vec<(String, AssetHandle)>,
    layout_blobs: Vec<(String, TextureBlob)>,
}

pub(crate) struct EntityAssets {
    stage: Stage,
    requested: HashSet<LogicalId>,
    models: Vec<(LogicalId, AssetHandle)>,
    partial: Vec<Partial>,
    b2: B2,
}

impl EntityAssets {
    pub(crate) fn new() -> Self {
        Self {
            stage: Stage::Start,
            requested: HashSet::new(),
            models: Vec::new(),
            partial: Vec::new(),
            b2: B2 {
                stage: Stage::Start,
                requested: HashSet::new(),
                models: Vec::new(),
                partial: Vec::new(),
                masks: Vec::new(),
                mask_ids: Vec::new(),
                mask_blobs: HashMap::new(),
                layout: Vec::new(),
                layout_blobs: Vec::new(),
            },
        }
    }

    /// No model, texture or table is on its way: what the renderer asked for has arrived (or failed).
    pub(crate) fn idle(&self) -> bool {
        // `Running` stays so while the game goes on; the table is in.
        let done = |s: &Stage| matches!(s, Stage::Running | Stage::Done);
        // (`Graphs` and `Table` are on their way.)
        done(&self.stage)
            && done(&self.b2.stage)
            && self.models.is_empty()
            && self.partial.is_empty()
            && self.b2.models.is_empty()
            && self.b2.partial.is_empty()
            && self.b2.masks.is_empty()
            && self.b2.layout.is_empty()
    }

    /// Advances the loading; call once per frame.
    pub(crate) fn update(&mut self, mut loader: Option<&mut AssetLoader>, renderer: &mut Renderer) {
        self.update_b1(loader.as_deref_mut(), renderer);
        self.update_b2(loader, renderer);
    }

    fn update_b1(&mut self, loader: Option<&mut AssetLoader>, renderer: &mut Renderer) {
        let Some(loader) = loader else {
            if !matches!(self.stage, Stage::Done) {
                tracing::info!("assets are disabled; units and cities are markers");
                renderer.set_entity_art_unavailable();
                self.stage = Stage::Done;
            }
            return;
        };
        let profile = renderer.platform_profile();
        match std::mem::replace(&mut self.stage, Stage::Done) {
            Stage::Start => {
                self.stage = Stage::Table(loader.request(LogicalId::new(ART_TABLE_ID), &profile));
            }
            Stage::Table(h) => {
                self.stage = match loader.poll(h) {
                    AssetPoll::Pending => Stage::Table(h),
                    AssetPoll::Ready(bytes) => match ArtTable::decode(&bytes) {
                        Ok(table) => {
                            renderer.set_entity_art(table);
                            Stage::Running
                        }
                        Err(e) => {
                            tracing::error!("{ART_TABLE_ID} is not a valid art table: {e}");
                            renderer.set_entity_art_unavailable();
                            Stage::Done
                        }
                    },
                    AssetPoll::Failed(e) => {
                        tracing::info!("{ART_TABLE_ID} unavailable ({e}); units are markers");
                        renderer.set_entity_art_unavailable();
                        Stage::Done
                    }
                };
            }
            Stage::Running => {
                self.stage = Stage::Running;
                self.run(loader, renderer);
            }
            // The graphs are a stage of the B2 flow only.
            Stage::Graphs(_) | Stage::Done => {}
        }
    }

    fn run(&mut self, loader: &mut AssetLoader, renderer: &mut Renderer) {
        let profile = renderer.platform_profile();
        for id in renderer.entity_models_wanted().to_vec() {
            if self.requested.insert(id.clone()) {
                let h = loader.request(id.clone(), &profile);
                self.models.push((id, h));
            }
        }

        let mut arrived = Vec::new();
        let mut failed = Vec::new();
        self.models.retain(|(id, h)| match loader.poll(*h) {
            AssetPoll::Pending => true,
            AssetPoll::Ready(bytes) => {
                match ModelBlob::decode(&bytes) {
                    Ok(blob) => arrived.push((id.clone(), blob)),
                    Err(e) => {
                        tracing::warn!(id = id.as_str(), "bad model blob: {e}");
                        failed.push(id.clone());
                    }
                }
                false
            }
            AssetPoll::Failed(e) => {
                tracing::warn!(id = id.as_str(), "model unavailable ({e})");
                failed.push(id.clone());
                false
            }
        });
        for (id, blob) in arrived {
            let mut wanted: Vec<LogicalId> = blob
                .materials
                .iter()
                // The specular map is the `SREFMap` of the ported `UnitShader`.
                .flat_map(|m| [m.diffuse.clone(), m.specular.clone()])
                .flatten()
                .collect();
            wanted.sort();
            wanted.dedup();
            let textures = wanted
                .into_iter()
                .map(|t| {
                    let h = loader.request(t.clone(), &profile);
                    (t, h)
                })
                .collect();
            self.partial.push(Partial {
                id,
                blob,
                textures,
                loaded: HashMap::new(),
            });
        }
        for id in failed {
            renderer.fail_entity_model(id);
        }

        let mut i = 0;
        while i < self.partial.len() {
            let p = &mut self.partial[i];
            p.textures.retain(|(id, h)| match loader.poll(*h) {
                AssetPoll::Pending => true,
                AssetPoll::Ready(bytes) => {
                    match TextureBlob::decode(&bytes) {
                        Ok(t) => {
                            p.loaded.insert(id.clone(), t);
                        }
                        Err(e) => tracing::warn!(id = id.as_str(), "bad texture blob: {e}"),
                    }
                    false
                }
                AssetPoll::Failed(e) => {
                    tracing::warn!(
                        id = id.as_str(),
                        "texture unavailable ({e}), model is white"
                    );
                    false
                }
            });
            if p.textures.is_empty() {
                let p = self.partial.swap_remove(i);
                renderer.add_entity_model(p.id, &p.blob, &p.loaded);
            } else {
                i += 1;
            }
        }
    }

    /// The units of `assets-b2`: the table, then the models of the figures.
    fn update_b2(&mut self, loader: Option<&mut AssetLoader>, renderer: &mut Renderer) {
        let Some(loader) = loader else { return };
        let profile = renderer.platform_profile();
        let b2 = &mut self.b2;
        match std::mem::replace(&mut b2.stage, Stage::Done) {
            Stage::Start => {
                let id = LogicalId::new(DGE_GRAPHS_ID);
                if loader.manifest().is_none() && loader.manifest_error().is_none() {
                    // Whether the set has the graphs is known with the manifest.
                    b2.stage = Stage::Start;
                    return;
                }
                b2.stage = if loader
                    .manifest()
                    .is_some_and(|m| m.resolve(&id, &profile).is_some())
                {
                    Stage::Graphs(loader.request(id, &profile))
                } else {
                    tracing::info!(
                        "the asset set has no {DGE_GRAPHS_ID}: the units play the clips of their actions"
                    );
                    Stage::Table(loader.request(LogicalId::new(ART_TABLE_B2_ID), &profile))
                };
            }
            Stage::Graphs(h) => {
                b2.stage = match loader.poll(h) {
                    AssetPoll::Pending => Stage::Graphs(h),
                    done => {
                        match done {
                            AssetPoll::Ready(bytes) => {
                                match std::str::from_utf8(&bytes)
                                    .map_err(|e| e.to_string())
                                    .and_then(render::anim::dge::install)
                                {
                                    Ok(n) => tracing::info!("{n} unit blend graph(s) installed"),
                                    Err(e) => tracing::warn!("{DGE_GRAPHS_ID} does not read: {e}"),
                                }
                            }
                            AssetPoll::Failed(e) => {
                                tracing::warn!("{DGE_GRAPHS_ID} unavailable ({e})")
                            }
                            AssetPoll::Pending => {}
                        }
                        Stage::Table(loader.request(LogicalId::new(ART_TABLE_B2_ID), &profile))
                    }
                };
            }
            Stage::Table(h) => {
                b2.stage = match loader.poll(h) {
                    AssetPoll::Pending => Stage::Table(h),
                    AssetPoll::Ready(bytes) => match ArtTable::decode(&bytes) {
                        Ok(table) => {
                            // The masks are the same in every tree record (WOB3).
                            let masks = ["FEATURE_FOREST", "FEATURE_JUNGLE"]
                                .into_iter()
                                .flat_map(|f| (0..6u8).map(move |s| (f, s)))
                                .find_map(|(f, s)| table.tree(f, s))
                                .map(|t| t.masks.clone())
                                .unwrap_or_default();
                            for id in &masks {
                                let h = loader.request(id.clone(), &profile);
                                b2.masks.push((id.clone(), h));
                            }
                            b2.mask_ids = masks;
                            for name in render::world::city::fields::file_names() {
                                let h =
                                    loader.request(LogicalId::new(format!("tx:{name}")), &profile);
                                b2.layout.push((name, h));
                            }
                            renderer.set_unit_art_b2(table);
                            Stage::Running
                        }
                        Err(e) => {
                            tracing::error!("{ART_TABLE_B2_ID} is not a valid art table: {e}");
                            Stage::Done
                        }
                    },
                    // B1's pack has none: the units stay as the flow of B1 draws them.
                    AssetPoll::Failed(e) => {
                        tracing::info!(
                            "{ART_TABLE_B2_ID} unavailable ({e}); the units stay on B1's models"
                        );
                        Stage::Done
                    }
                };
            }
            Stage::Running => {
                b2.stage = Stage::Running;
                run_b2(b2, loader, renderer);
            }
            Stage::Done => {}
        }
    }
}

/// Polls the placement masks of the trees; when all have come (or failed) the renderer scatters its
/// tree lists with them.
fn poll_masks(b2: &mut B2, loader: &mut AssetLoader, renderer: &mut Renderer) {
    if b2.mask_ids.is_empty() {
        return;
    }
    let blobs = &mut b2.mask_blobs;
    b2.masks.retain(|(id, h)| match loader.poll(*h) {
        AssetPoll::Pending => true,
        AssetPoll::Ready(bytes) => {
            match TextureBlob::decode(&bytes) {
                Ok(t) => {
                    blobs.insert(id.clone(), t);
                }
                Err(e) => tracing::warn!(id = id.as_str(), "bad forest mask blob: {e}"),
            }
            false
        }
        AssetPoll::Failed(e) => {
            tracing::warn!(id = id.as_str(), "forest mask unavailable ({e})");
            false
        }
    });
    if b2.masks.is_empty() {
        let ids = std::mem::take(&mut b2.mask_ids);
        let ordered: Vec<TextureBlob> = ids.iter().filter_map(|id| blobs.remove(id)).collect();
        // A mask missing would shift the lists (list `i` takes mask `i % N`): all or none.
        if ordered.len() == ids.len() {
            renderer.set_forest_masks(&ordered);
        } else {
            tracing::warn!("a forest mask is missing; the trees keep the hex outline");
        }
    }
}

/// Polls the fields of the city layout; when all have come (or failed) the renderer takes those that
/// came (an incomplete set keeps its stand-in).
fn poll_layout(b2: &mut B2, loader: &mut AssetLoader, renderer: &mut Renderer) {
    if b2.layout.is_empty() {
        return;
    }
    let blobs = &mut b2.layout_blobs;
    b2.layout.retain(|(name, h)| match loader.poll(*h) {
        AssetPoll::Pending => true,
        AssetPoll::Ready(bytes) => {
            match TextureBlob::decode(&bytes) {
                Ok(t) => blobs.push((name.clone(), t)),
                Err(e) => tracing::warn!(name, "bad city layout field blob: {e}"),
            }
            false
        }
        AssetPoll::Failed(e) => {
            tracing::info!(name, "city layout field unavailable ({e})");
            false
        }
    });
    if b2.layout.is_empty() {
        renderer.set_city_layout_fields(&std::mem::take(blobs));
    }
}

/// The time one frame spends registering the models that arrived (each builds GPU buffers and
/// re-places what uses it), and the time while a backlog waits (hundreds of models arrive together
/// at the load; a count of two a frame kept them coming for many seconds). At least one is
/// registered a frame.
const MODEL_BUDGET_MS: f64 = 4.0;
const MODEL_BUDGET_BACKLOG_MS: f64 = 12.0;
/// After the first full view (a new unit type in a game must not make a slow frame).
const MODEL_BUDGET_PLAYING_MS: f64 = 2.0;
/// More waiting than this is a backlog.
const MODEL_BACKLOG: usize = 24;

fn run_b2(b2: &mut B2, loader: &mut AssetLoader, renderer: &mut Renderer) {
    poll_masks(b2, loader, renderer);
    poll_layout(b2, loader, renderer);
    let profile = renderer.platform_profile();
    for id in renderer.unit_models_b2_wanted() {
        if b2.requested.insert(id.clone()) {
            let h = loader.request(id.clone(), &profile);
            b2.models.push((id, h));
        }
    }
    let mut arrived = Vec::new();
    b2.models.retain(|(id, h)| match loader.poll(*h) {
        AssetPoll::Pending => true,
        AssetPoll::Ready(bytes) => {
            match ModelBlobV2::decode(&bytes) {
                Ok(blob) => arrived.push((id.clone(), blob)),
                Err(e) => tracing::warn!(id = id.as_str(), "bad model blob: {e}"),
            }
            false
        }
        AssetPoll::Failed(e) => {
            tracing::warn!(
                id = id.as_str(),
                "model unavailable ({e}); its figures are not drawn"
            );
            false
        }
    });
    for (id, blob) in arrived {
        let textures = UnitDraw::textures_of(&blob)
            .into_iter()
            .map(|t| {
                let h = loader.request(t.clone(), &profile);
                (t, h)
            })
            .collect();
        let pack = blob.anim_pack.clone().map(|p| loader.request(p, &profile));
        let fsm = blob.fsm.clone().map(|f| loader.request(f, &profile));
        b2.partial.push(PartialB2 {
            id,
            blob,
            pack,
            fsm,
            pack_blob: None,
            fsm_blob: None,
            textures,
            loaded: HashMap::new(),
        });
    }
    let mut i = 0;
    let mut registered = 0;
    let started = renderer.clock_ms();
    let budget = if !renderer.map_loading() {
        MODEL_BUDGET_PLAYING_MS
    } else if b2.partial.len() > MODEL_BACKLOG {
        MODEL_BUDGET_BACKLOG_MS
    } else {
        MODEL_BUDGET_MS
    };
    while i < b2.partial.len() {
        let p = &mut b2.partial[i];
        p.textures.retain(|(id, h)| match loader.poll(*h) {
            AssetPoll::Pending => true,
            AssetPoll::Ready(bytes) => {
                match TextureBlob::decode(&bytes) {
                    Ok(t) => {
                        p.loaded.insert(id.clone(), t);
                    }
                    Err(e) => tracing::warn!(id = id.as_str(), "bad texture blob: {e}"),
                }
                false
            }
            AssetPoll::Failed(e) => {
                tracing::warn!(
                    id = id.as_str(),
                    "texture unavailable ({e}), the material is white"
                );
                false
            }
        });
        if let Some(h) = p.pack {
            match loader.poll(h) {
                AssetPoll::Pending => {}
                AssetPoll::Ready(bytes) => {
                    match AnimPack::decode(&bytes) {
                        Ok(a) => p.pack_blob = Some(a),
                        Err(e) => tracing::warn!(id = p.id.as_str(), "bad animation pack: {e}"),
                    }
                    p.pack = None;
                }
                AssetPoll::Failed(e) => {
                    tracing::warn!(id = p.id.as_str(), "animation pack unavailable ({e})");
                    p.pack = None;
                }
            }
        }
        if let Some(h) = p.fsm {
            match loader.poll(h) {
                AssetPoll::Pending => {}
                AssetPoll::Ready(bytes) => {
                    match IdleFsm::decode(&bytes) {
                        Ok(f) => p.fsm_blob = Some(f),
                        Err(e) => tracing::warn!(id = p.id.as_str(), "bad idle automaton: {e}"),
                    }
                    p.fsm = None;
                }
                AssetPoll::Failed(e) => {
                    tracing::warn!(id = p.id.as_str(), "idle automaton unavailable ({e})");
                    p.fsm = None;
                }
            }
        }
        if p.textures.is_empty() && p.pack.is_none() && p.fsm.is_none() {
            // A few models a frame: each one builds GPU buffers and re-places what uses it.
            if registered >= 1 && renderer.clock_ms() - started >= budget {
                i += 1;
                continue;
            }
            registered += 1;
            let p = b2.partial.swap_remove(i);
            renderer.add_unit_model_b2(p.id, &p.blob, p.pack_blob, p.fsm_blob, &p.loaded);
        } else {
            i += 1;
        }
    }
}
