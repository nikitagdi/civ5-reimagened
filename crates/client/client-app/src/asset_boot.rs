//! Startup asset wiring: derive the HTTP base from the WebSocket URL and load the manifest, then
//! the `boot` group of the light data (`LightBoot`).

use std::rc::Rc;

use assets::{
    AssetHandle, AssetLoader, AssetPoll, ContentHash, HttpSource, LogicalId, Manifest,
    ManifestStatus, PlatformProfile,
};
use platform::HttpClient;
use render::Renderer;
use render::asset_format::{TerrainPieceSet, TextureBlob, WorldSettings};
use render::lighting::{BOOT_GROUP, WORLD_SETTINGS_ID, file_of, required_ids};
use render::world::water::data as water_data;

/// `ws://host:port/ws` → `http://host:port/assets/`; `wss` maps to `https`.
/// `None` if the URL is not a WebSocket URL.
pub(crate) fn assets_base_from_ws_url(ws_url: &str) -> Option<String> {
    let (scheme, rest) = ws_url.split_once("://")?;
    let scheme = match scheme {
        "ws" => "http",
        "wss" => "https",
        _ => return None,
    };
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .filter(|a| !a.is_empty())?;
    Some(format!("{scheme}://{authority}/assets/"))
}

/// Manifest loading in progress, driven once per frame.
pub(crate) struct AssetsBoot {
    loader: AssetLoader,
    logged: bool,
}

impl AssetsBoot {
    /// `None` (with a log line) when the URLs cannot be used; the client then runs without
    /// assets.
    pub(crate) fn start(
        http: Rc<dyn HttpClient>,
        server_url: &str,
        manifest_override: Option<&str>,
    ) -> Option<Self> {
        let Some(base) = assets_base_from_ws_url(server_url) else {
            tracing::warn!(%server_url, "cannot derive the assets URL from the server URL; assets disabled");
            return None;
        };
        Self::start_at(http, &base, manifest_override)
    }

    /// As [`Self::start`], with the base URL of the assets given (`golden` has no server).
    pub(crate) fn start_at(
        http: Rc<dyn HttpClient>,
        base: &str,
        manifest_override: Option<&str>,
    ) -> Option<Self> {
        let hash = match manifest_override.map(str::parse::<ContentHash>).transpose() {
            Ok(h) => h,
            Err(e) => {
                tracing::error!("--manifest/?manifest= is not a valid hash ({e}); assets disabled");
                return None;
            }
        };
        let source = HttpSource::new(Rc::clone(&http), base);
        let mut loader = AssetLoader::new(http, base, Box::new(source));
        tracing::info!(%base, manifest = ?hash.map(|h| h.to_string()), "loading asset manifest");
        loader.load_manifest(hash);
        Some(Self {
            loader,
            logged: false,
        })
    }

    pub(crate) fn loader_mut(&mut self) -> &mut AssetLoader {
        &mut self.loader
    }

    pub(crate) fn update(&mut self) {
        let status = self.loader.poll_manifest();
        if self.logged {
            return;
        }
        match status {
            ManifestStatus::Ready => {
                self.logged = true;
                let n = self.loader.manifest().map_or(0, assets::Manifest::len);
                tracing::info!(entries = n, "asset manifest loaded");
            }
            ManifestStatus::Failed => {
                self.logged = true;
                if let Some(e) = self.loader.manifest_error() {
                    tracing::error!("asset manifest failed to load: {e}");
                }
            }
            ManifestStatus::Idle | ManifestStatus::Loading => {}
        }
    }
}

/// Whether the manifest has the `boot` group of the light data: the world settings blob is in
/// the group (the set `b2-light`). The assets of B1 have none, and the world is drawn at once.
pub(crate) fn has_boot_group(manifest: &Manifest, profile: &PlatformProfile) -> bool {
    manifest
        .resolve(&LogicalId::new(WORLD_SETTINGS_ID), profile)
        .is_some_and(|v| v.prefetch == BOOT_GROUP)
}

enum BootStage {
    /// Waiting for the manifest and for the renderer (which knows the texture variants).
    Waiting,
    Loading {
        pending: Vec<(LogicalId, AssetHandle)>,
        total: usize,
        failed: usize,
    },
    /// Every item has been asked for and answered; `complete` if none failed.
    Done { complete: bool },
    /// No boot group (B1 assets, no assets): nothing to wait for.
    NotNeeded,
}

/// Loads the `boot` group into the renderer's `LightingEnv`. The first frame of the map waits
/// for it (`Renderer::set_boot_required`); the loading screen shows the progress.
pub(crate) struct LightBoot {
    stage: BootStage,
}

impl LightBoot {
    pub(crate) fn new() -> Self {
        Self {
            stage: BootStage::Waiting,
        }
    }

    /// The map may be drawn: there is no boot group, or it has been loaded. A failed item keeps
    /// the loading screen, with the error in the log (no stand-ins, AB-D14).
    pub(crate) fn map_may_draw(&self) -> bool {
        matches!(
            self.stage,
            BootStage::NotNeeded | BootStage::Done { complete: true }
        )
    }

    /// Advances the loading; call once per frame.
    pub(crate) fn update(&mut self, loader: Option<&mut AssetLoader>, renderer: &mut Renderer) {
        let Some(loader) = loader else {
            self.stage = BootStage::NotNeeded;
            return;
        };
        match &mut self.stage {
            BootStage::Waiting => {
                let profile = renderer.platform_profile();
                loader.announce_manifest(&profile);
                match loader.manifest() {
                    None if loader.manifest_error().is_some() => {
                        self.stage = BootStage::NotNeeded;
                    }
                    None => {}
                    Some(m) if !has_boot_group(m, &profile) => {
                        tracing::info!("no boot group in the manifest; the world draws at once");
                        self.stage = BootStage::NotNeeded;
                    }
                    Some(_) => {
                        renderer.set_boot_required(true);
                        let pending: Vec<_> = required_ids()
                            .into_iter()
                            .map(|id| {
                                let h = loader.request(id.clone(), &profile);
                                (id, h)
                            })
                            .collect();
                        tracing::info!(items = pending.len(), "loading the boot group");
                        self.stage = BootStage::Loading {
                            total: pending.len(),
                            pending,
                            failed: 0,
                        };
                    }
                }
            }
            BootStage::Loading {
                pending,
                total,
                failed,
            } => {
                pending.retain(|(id, h)| match loader.poll(*h) {
                    AssetPoll::Pending => true,
                    AssetPoll::Ready(bytes) => {
                        if let Err(why) = take(renderer, id, &bytes) {
                            tracing::error!(id = id.as_str(), "boot item unusable: {why}");
                            *failed += 1;
                        }
                        false
                    }
                    AssetPoll::Failed(e) => {
                        tracing::error!(id = id.as_str(), "boot item unavailable: {e}");
                        *failed += 1;
                        false
                    }
                });
                if pending.is_empty() {
                    let complete = *failed == 0 && renderer.lighting().ready();
                    tracing::info!(
                        total = *total,
                        failed = *failed,
                        complete,
                        "boot group done"
                    );
                    self.stage = BootStage::Done { complete };
                }
            }
            BootStage::Done { .. } | BootStage::NotNeeded => {}
        }
    }
}

/// Loads what the terrain needs for the map it has: the pieces of the styles of its continents and
/// the textures of its `TerrainDef` rows (`Renderer::terrain_wanted`). Runs once the boot group is
/// in (the table of rows comes with the world settings). A blob that cannot be loaded is logged once
/// and not asked for again: the terrain then stays on B1's ground.
///
/// The key `TextureQuality` (0 Low) picks the `@low` variant of a `tx:` id where the manifest has
/// one ([`terrain_request_id`]); the blob goes to the renderer under the original id. A change of
/// the key while running asks the textures again in the other variant: `add_terrain_def_texture`
/// replaces the held texture, bumps the library epoch and the terrain bakes its pages again.
pub(crate) struct TerrainBoot {
    asked: std::collections::HashSet<LogicalId>,
    /// `tx:` ids handed to the renderer (original ids), to ask again when the quality changes.
    taken: std::collections::HashSet<LogicalId>,
    /// Original id, handle, and the quality it was asked at.
    pending: Vec<(LogicalId, AssetHandle, bool)>,
    /// The quality last seen; `None` before the first frame.
    low: Option<bool>,
    /// A switch of the quality is on its way: the new variants of the taken textures are held here
    /// and go to the renderer together, so the terrain bakes its pages again once, not per texture.
    switching: bool,
    held: Vec<(LogicalId, Vec<u8>)>,
    /// Decoded piece sets that wait for the others of their batch ([`Renderer::add_terrain_piece_sets`]).
    sets: Vec<TerrainPieceSet>,
}

/// The id to request for a terrain id: `<id>@low` when `low` and the manifest resolves it for the
/// profile (only `tx:` ids have such variants), else the id itself.
pub(crate) fn terrain_request_id(
    id: &LogicalId,
    low: bool,
    manifest: Option<&Manifest>,
    profile: &PlatformProfile,
) -> LogicalId {
    if low && id.as_str().starts_with("tx:") {
        let lowered = LogicalId::new(format!("{}@low", id.as_str()));
        if manifest.is_some_and(|m| m.resolve(&lowered, profile).is_some()) {
            return lowered;
        }
    }
    id.clone()
}

impl TerrainBoot {
    pub(crate) fn new() -> Self {
        Self {
            asked: std::collections::HashSet::new(),
            taken: std::collections::HashSet::new(),
            pending: Vec::new(),
            low: None,
            switching: false,
            held: Vec::new(),
            sets: Vec::new(),
        }
    }

    /// Nothing asked for is still on its way.
    pub(crate) fn idle(&self) -> bool {
        self.pending.is_empty()
    }

    /// Advances the loading; call once per frame, after [`LightBoot::update`].
    pub(crate) fn update(
        &mut self,
        boot: &LightBoot,
        loader: Option<&mut AssetLoader>,
        renderer: &mut Renderer,
    ) {
        let (true, Some(loader)) = (boot.map_may_draw(), loader) else {
            return;
        };
        let profile = renderer.platform_profile();
        let low = renderer
            .settings()
            .get("TextureQuality")
            .is_some_and(|v| v <= 0);
        if self.low.is_some_and(|was| was != low) {
            // Quality switched: answers still on their way are of the other variant, drop them,
            // and ask for every taken texture again.
            self.pending.retain(|(_, _, was)| *was == low);
            for id in &self.taken {
                self.asked.remove(id);
            }
            self.switching = true;
            self.held.clear();
        }
        self.low = Some(low);
        if !matches!(boot.stage, BootStage::NotNeeded) {
            let mut ids = renderer.terrain_wanted();
            ids.extend(self.taken.iter().cloned());
            for id in ids {
                if self.asked.insert(id.clone()) {
                    let req = terrain_request_id(&id, low, loader.manifest(), &profile);
                    let h = loader.request(req, &profile);
                    self.pending.push((id, h, low));
                }
            }
        }
        let taken = &mut self.taken;
        let (switching, held) = (self.switching, &mut self.held);
        let sets = &mut self.sets;
        self.pending.retain(|(id, h, _)| match loader.poll(*h) {
            AssetPoll::Pending => true,
            // Piece sets wait for each other: every set rebuilds the whole map, so they go in together.
            AssetPoll::Ready(bytes) if id.as_str().starts_with("pc:") => {
                match TerrainPieceSet::decode(&bytes) {
                    Ok(set) => sets.push(set),
                    Err(why) => tracing::error!(id = id.as_str(), "terrain item unusable: {why}"),
                }
                false
            }
            AssetPoll::Ready(bytes) if switching && taken.contains(id) => {
                held.push((id.clone(), bytes.to_vec()));
                false
            }
            AssetPoll::Ready(bytes) => {
                match take_terrain(renderer, id, &bytes) {
                    Ok(()) if id.as_str().starts_with("tx:") => {
                        taken.insert(id.clone());
                    }
                    Ok(()) => {}
                    Err(why) => tracing::error!(id = id.as_str(), "terrain item unusable: {why}"),
                }
                false
            }
            AssetPoll::Failed(e) => {
                tracing::error!(id = id.as_str(), "terrain item unavailable: {e}");
                false
            }
        });
        if !self.sets.is_empty()
            && !self
                .pending
                .iter()
                .any(|(id, _, _)| id.as_str().starts_with("pc:"))
        {
            renderer.add_terrain_piece_sets(std::mem::take(&mut self.sets));
        }
        if self.switching
            && !self
                .pending
                .iter()
                .any(|(id, _, _)| self.taken.contains(id))
        {
            for (id, bytes) in self.held.drain(..) {
                if let Err(why) = take_terrain(renderer, &id, &bytes) {
                    tracing::error!(id = id.as_str(), "terrain item unusable: {why}");
                }
            }
            self.switching = false;
        }
    }
}

enum WaterStage {
    Waiting,
    Loading(Vec<(LogicalId, AssetHandle)>),
    Done,
}

/// Loads the textures of the group `water` the `Water` subsystem reads (`WaterData::PACK_FILES`).
/// Runs once the boot group is in, like [`TerrainBoot`]. Assets without the group (B1) leave the
/// water to B1; a file that cannot be loaded is logged and the water stays on B1's too, because the
/// subsystem draws only with the whole set.
pub(crate) struct WaterBoot {
    stage: WaterStage,
}

impl WaterBoot {
    pub(crate) fn new() -> Self {
        Self {
            stage: WaterStage::Waiting,
        }
    }

    /// The set is in, or will never come (no group, no assets).
    pub(crate) fn idle(&self) -> bool {
        matches!(self.stage, WaterStage::Done)
    }

    /// Advances the loading; call once per frame, after [`LightBoot::update`].
    pub(crate) fn update(
        &mut self,
        boot: &LightBoot,
        loader: Option<&mut AssetLoader>,
        renderer: &mut Renderer,
    ) {
        let (true, Some(loader)) = (boot.map_may_draw(), loader) else {
            return;
        };
        match &mut self.stage {
            WaterStage::Waiting => {
                let profile = renderer.platform_profile();
                let Some(manifest) = loader.manifest() else {
                    return;
                };
                let ids: Vec<LogicalId> = water_data::PACK_FILES
                    .iter()
                    .map(|file| LogicalId::new(format!("tx:{file}")))
                    .filter(|id| manifest.resolve(id, &profile).is_some())
                    .collect();
                if ids.is_empty() {
                    tracing::info!("no group `water` in the manifest; B1's water is drawn");
                    self.stage = WaterStage::Done;
                    return;
                }
                let pending: Vec<_> = ids
                    .into_iter()
                    .map(|id| {
                        let h = loader.request(id.clone(), &profile);
                        (id, h)
                    })
                    .collect();
                tracing::info!(items = pending.len(), "loading the group water");
                self.stage = WaterStage::Loading(pending);
            }
            WaterStage::Loading(pending) => {
                pending.retain(|(id, h)| match loader.poll(*h) {
                    AssetPoll::Pending => true,
                    AssetPoll::Ready(bytes) => {
                        match TextureBlob::decode(&bytes) {
                            Ok(blob) => {
                                let file = id.as_str().strip_prefix("tx:").unwrap_or(id.as_str());
                                renderer.add_water_texture(file, blob);
                            }
                            Err(e) => {
                                tracing::error!(id = id.as_str(), "water texture unusable: {e}")
                            }
                        }
                        false
                    }
                    AssetPoll::Failed(e) => {
                        tracing::error!(id = id.as_str(), "water texture unavailable: {e}");
                        false
                    }
                });
                if pending.is_empty() {
                    tracing::info!("group water done");
                    self.stage = WaterStage::Done;
                }
            }
            WaterStage::Done => {}
        }
    }
}

/// Gives a piece set or a texture of the terrain to the renderer.
fn take_terrain(renderer: &mut Renderer, id: &LogicalId, bytes: &[u8]) -> Result<(), String> {
    if id.as_str().starts_with("pc:") {
        let set = TerrainPieceSet::decode(bytes).map_err(|e| e.to_string())?;
        renderer.add_terrain_piece_set(set);
    } else {
        let blob = TextureBlob::decode(bytes).map_err(|e| e.to_string())?;
        renderer.add_terrain_def_texture(id.clone(), blob);
    }
    Ok(())
}

/// Gives one blob of the boot group to the renderer.
fn take(renderer: &mut Renderer, id: &LogicalId, bytes: &[u8]) -> Result<(), String> {
    if id.as_str() == WORLD_SETTINGS_ID {
        let settings = WorldSettings::decode(bytes).map_err(|e| e.to_string())?;
        renderer.set_world_settings(settings);
        return Ok(());
    }
    let file = file_of(id).ok_or("not a texture id")?;
    let blob = TextureBlob::decode(bytes).map_err(|e| e.to_string())?;
    renderer
        .add_lighting_texture(file, &blob)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use assets::{PrefetchGroup, Variant, VariantFormat};

    fn variant(group: u16) -> Variant {
        Variant {
            format: VariantFormat::Generic,
            hash: ContentHash([7; 32]),
            size: 1,
            prefetch: PrefetchGroup(group),
        }
    }

    #[test]
    fn the_boot_group_is_there_when_the_settings_blob_is_in_group_one() {
        let profile = PlatformProfile::uncompressed_only();
        let mut b2 = Manifest::new();
        b2.insert(LogicalId::new(WORLD_SETTINGS_ID), variant(1));
        assert!(has_boot_group(&b2, &profile));
        // Assets of B1: no such blob; or the blob is not in the boot group.
        assert!(!has_boot_group(&Manifest::new(), &profile));
        let mut other = Manifest::new();
        other.insert(LogicalId::new(WORLD_SETTINGS_ID), variant(0));
        assert!(!has_boot_group(&other, &profile));
    }

    #[test]
    fn low_quality_asks_the_low_variant_where_the_manifest_has_one() {
        let profile = PlatformProfile::uncompressed_only();
        let mut m = Manifest::new();
        m.insert(LogicalId::new("tx:grass_d.dds"), variant(0));
        m.insert(LogicalId::new("tx:grass_d.dds@low"), variant(0));
        m.insert(LogicalId::new("tx:snow_d.dds"), variant(0));
        m.insert(LogicalId::new("pc:fantasy"), variant(0));
        m.insert(LogicalId::new("pc:fantasy@low"), variant(0));
        let ask = |id: &str, low: bool| {
            terrain_request_id(&LogicalId::new(id), low, Some(&m), &profile)
                .as_str()
                .to_owned()
        };
        assert_eq!(ask("tx:grass_d.dds", true), "tx:grass_d.dds@low");
        assert_eq!(ask("tx:grass_d.dds", false), "tx:grass_d.dds");
        assert_eq!(ask("tx:snow_d.dds", true), "tx:snow_d.dds", "no variant");
        assert_eq!(ask("pc:fantasy", true), "pc:fantasy", "pieces never");
        let none = terrain_request_id(&LogicalId::new("tx:grass_d.dds"), true, None, &profile);
        assert_eq!(none.as_str(), "tx:grass_d.dds");
    }

    #[test]
    fn the_map_waits_until_the_manifest_is_known() {
        let boot = LightBoot::new();
        assert!(!boot.map_may_draw(), "until the manifest is known");
    }

    #[test]
    fn base_from_ws_url() {
        let f = assets_base_from_ws_url;
        assert_eq!(
            f("ws://127.0.0.1:8080/ws").as_deref(),
            Some("http://127.0.0.1:8080/assets/")
        );
        assert_eq!(
            f("wss://game.example/ws?x=1").as_deref(),
            Some("https://game.example/assets/")
        );
        assert_eq!(f("ws://h").as_deref(), Some("http://h/assets/"));
        assert_eq!(f("http://h/ws"), None);
        assert_eq!(f("ws:///ws"), None);
        assert_eq!(f("nonsense"), None);
    }
}
