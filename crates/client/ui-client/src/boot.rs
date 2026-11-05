//! Fetches the `ui` set through the asset loader: the index, then the groups the first screen needs,
//! then the texts. The result is a [`UiStore`] with everything in it, so layout never waits for a texture.

use std::collections::BTreeSet;
use std::rc::Rc;

use assets::{
    AssetHandle, AssetLoader, AssetPoll, LogicalId, PlatformProfile, UiFiles, VariantFormat,
};

use crate::store::{MemoryFile, UiStore};

/// How many blobs are in flight at once.
const IN_FLIGHT: usize = 32;

/// How many background texture fetches (`UiStore::take_wanted_fetches`) are in flight at once.
const LAZY_IN_FLIGHT: usize = 24;

/// What to fetch.
#[derive(Debug, Clone)]
pub struct BootPlan {
    /// `en_US`: the text table `ui:text:en_us`.
    pub lang: String,
    /// Contexts whose window groups (`ui:win:<name>`) are loaded with `ui:boot`.
    pub contexts: Vec<String>,
    /// Extra groups (`ui:hud`).
    pub groups: Vec<String>,
    /// Files that are not in the set.
    pub memory: Vec<MemoryFile>,
    /// A context without a window group is skipped (the M2 list names script libraries too).
    pub lenient_groups: bool,
}

impl BootPlan {
    /// M2: the contexts of the milestone (`exe_api::m2_contexts`) and the real `InGame` root.
    pub fn m2(contexts: Vec<String>) -> BootPlan {
        BootPlan {
            lang: "en_US".to_owned(),
            contexts,
            groups: Vec::new(),
            memory: Vec::new(),
            lenient_groups: true,
        }
    }
}

impl BootPlan {
    /// The four contexts of M1.
    pub fn m1() -> BootPlan {
        BootPlan {
            lang: "en_US".to_owned(),
            contexts: ["ToolTips", "EUI_context", "TopPanel", "NotificationPanel"]
                .map(str::to_owned)
                .to_vec(),
            groups: Vec::new(),
            lenient_groups: false,
            memory: vec![
                MemoryFile {
                    path: "fixture/M1Root.xml".to_owned(),
                    kind: asset_format::UiFileKind::Xml,
                    bytes: ui::fixture::M1_ROOT_XML.as_bytes().to_vec(),
                },
                MemoryFile {
                    path: "fixture/M1Root.lua".to_owned(),
                    kind: asset_format::UiFileKind::Lua,
                    bytes: Vec::new(),
                },
            ],
        }
    }
}

/// The profile before the app hands over the device's ([`UiBoot::set_profile`]): `Bc`, then
/// `Uncompressed`; the renderer decodes `Bc` itself on a device without it.
pub fn ui_profile() -> PlatformProfile {
    PlatformProfile::new(vec![VariantFormat::Bc, VariantFormat::Uncompressed])
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootState {
    Loading { done: usize, total: usize },
    Ready,
    Failed(String),
}

enum Stage {
    Index(AssetHandle),
    Files {
        store: Rc<UiStore>,
        queue: Vec<LogicalId>,
        pending: Vec<(LogicalId, AssetHandle)>,
        total: usize,
        done: usize,
    },
    Ready(Rc<UiStore>),
    Failed(String),
}

pub struct UiBoot {
    plan: BootPlan,
    profile: PlatformProfile,
    stage: Option<Stage>,
    /// Textures the UI named after the boot (icon atlases of the game database): fetched as they are asked for.
    lazy: Vec<(LogicalId, AssetHandle)>,
}

impl UiBoot {
    pub fn new(plan: BootPlan) -> UiBoot {
        UiBoot {
            plan,
            profile: ui_profile(),
            stage: None,
            lazy: Vec::new(),
        }
    }

    /// The variants the device takes (`Renderer::platform_profile`): a phone gets the `Astc`
    /// variants of the textures, like the world loaders. Requests made
    /// after this call use it.
    pub fn set_profile(&mut self, profile: PlatformProfile) {
        self.profile = profile;
    }

    pub fn profile(&self) -> &PlatformProfile {
        &self.profile
    }

    /// Advances the fetches; call once per frame with the loader of the app.
    pub fn update(&mut self, loader: &mut AssetLoader) -> BootState {
        let stage = match self.stage.take() {
            None => Stage::Index(loader.request(LogicalId::new(UiFiles::INDEX_ID), &self.profile)),
            Some(s) => s,
        };
        let stage = self.step(stage, loader);
        let state = match &stage {
            Stage::Index(_) => BootState::Loading { done: 0, total: 0 },
            Stage::Files { done, total, .. } => BootState::Loading {
                done: *done,
                total: *total,
            },
            Stage::Ready(_) => BootState::Ready,
            Stage::Failed(m) => BootState::Failed(m.clone()),
        };
        self.stage = Some(stage);
        if let Some(Stage::Ready(store)) = &self.stage {
            let store = Rc::clone(store);
            self.pump_lazy(&store, loader);
        }
        state
    }

    /// Textures the UI asked for by name that the boot did not bring (an icon atlas picked through the game
    /// database): requested a few at a time, stored when they arrive. The layer shows them from the next frame.
    fn pump_lazy(&mut self, store: &UiStore, loader: &mut AssetLoader) {
        self.lazy.retain(|(id, h)| match loader.poll(*h) {
            AssetPoll::Pending => true,
            AssetPoll::Ready(bytes) => {
                tracing::debug!(target: "ui.lazy", texture = id.as_str(), bytes = bytes.len(), "arrived");
                match store.put(id.as_str(), bytes) {
                    Ok(()) => store.fetch_done(id.as_str()),
                    Err(e) => {
                        tracing::warn!(texture = id.as_str(), "ui texture: {e}");
                        store.fetch_failed(id.as_str());
                    }
                }
                false
            }
            AssetPoll::Failed(e) => {
                tracing::warn!(texture = id.as_str(), "ui texture: {e}");
                store.fetch_failed(id.as_str());
                false
            }
        });
        let room = LAZY_IN_FLIGHT.saturating_sub(self.lazy.len());
        if room == 0 {
            return;
        }
        for id in store.take_wanted_fetches(room) {
            let id = LogicalId::new(id);
            tracing::debug!(target: "ui.lazy", texture = id.as_str(), "requested");
            let h = loader.request(id.clone(), &self.profile);
            self.lazy.push((id, h));
        }
    }

    /// The store, once the state is `Ready`.
    pub fn store(&self) -> Option<Rc<UiStore>> {
        match &self.stage {
            Some(Stage::Ready(s)) => Some(Rc::clone(s)),
            _ => None,
        }
    }

    fn step(&self, stage: Stage, loader: &mut AssetLoader) -> Stage {
        match stage {
            Stage::Index(h) => match loader.poll(h) {
                AssetPoll::Pending => Stage::Index(h),
                AssetPoll::Failed(e) => Stage::Failed(format!("ui:index: {e}")),
                AssetPoll::Ready(bytes) => match self.begin_files(&bytes) {
                    Ok(s) => self.step(s, loader),
                    Err(m) => Stage::Failed(m),
                },
            },
            Stage::Files {
                store,
                mut queue,
                mut pending,
                total,
                mut done,
            } => {
                let mut failed = None;
                pending.retain(|(id, h)| match loader.poll(*h) {
                    AssetPoll::Pending => true,
                    AssetPoll::Ready(bytes) => {
                        if let Err(e) = store.put(id.as_str(), bytes) {
                            failed.get_or_insert(format!("{}: {e}", id.as_str()));
                        }
                        done += 1;
                        false
                    }
                    AssetPoll::Failed(e) => {
                        failed.get_or_insert(format!("{}: {e}", id.as_str()));
                        false
                    }
                });
                if let Some(m) = failed {
                    return Stage::Failed(m);
                }
                while pending.len() < IN_FLIGHT {
                    let Some(id) = queue.pop() else { break };
                    let h = loader.request(id.clone(), &self.profile);
                    pending.push((id, h));
                }
                if pending.is_empty() && queue.is_empty() {
                    tracing::info!(files = total, "ui set loaded");
                    return Stage::Ready(store);
                }
                Stage::Files {
                    store,
                    queue,
                    pending,
                    total,
                    done,
                }
            }
            s @ (Stage::Ready(_) | Stage::Failed(_)) => s,
        }
    }

    /// The index has arrived: works out what the plan needs and opens the store.
    fn begin_files(&self, index_bytes: &[u8]) -> Result<Stage, String> {
        let index =
            asset_format::UiIndex::decode(index_bytes).map_err(|e| format!("ui:index: {e}"))?;
        let probe = UiFiles::open(index_bytes).map_err(|e| format!("ui:index: {e}"))?;
        let mut ids: BTreeSet<String> = BTreeSet::new();
        let mut groups = vec![UiFiles::BOOT_GROUP.to_owned()];
        groups.extend(self.plan.contexts.iter().map(|c| UiFiles::window_group(c)));
        groups.extend(self.plan.groups.iter().cloned());
        for g in &groups {
            let Some(closure) = probe.group_closure(g) else {
                if self.plan.lenient_groups && g != UiFiles::BOOT_GROUP {
                    tracing::debug!(group = %g, "no window group for an M2 context");
                    continue;
                }
                return Err(format!("the ui set has no group {g}"));
            };
            ids.extend(closure.iter().map(|i| i.as_str().to_owned()));
        }
        ids.insert(UiFiles::text_id(&self.plan.lang).as_str().to_owned());
        // Only the language in use; the sound table and sound files are `ui-audio`'s.
        let text = UiFiles::text_id(&self.plan.lang).as_str().to_owned();
        ids.retain(|i| {
            *i != UiFiles::INDEX_ID
                && !(i.starts_with("ui:text:") && *i != text)
                && !i.starts_with("ui:au:")
                && i != "ui:snd"
        });
        let store =
            UiStore::new(index, self.plan.memory.clone()).map_err(|e| format!("ui:index: {e}"))?;
        let queue: Vec<LogicalId> = ids.into_iter().rev().map(LogicalId::new).collect();
        let total = queue.len();
        Ok(Stage::Files {
            store,
            queue,
            pending: Vec::new(),
            total,
            done: 0,
        })
    }
}
