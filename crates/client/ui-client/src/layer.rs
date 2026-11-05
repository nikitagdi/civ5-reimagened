//! The original UI as one layer of the client: input in, a draw list out, once per frame.

use std::rc::Rc;

use asset_format::{TextureBlob, TextureFormat};
use draw_list::{TextureId, UiDrawList};
use platform::InputEvent;
pub use ui::InputResult;
use ui::{UiConfig, UiCore, UiStats};

use crate::host::{ClientHost, HostCells};
use crate::store::{LazyStats, StoreFiles, UiStore};
use crate::text::ClientText;

#[derive(Debug, thiserror::Error)]
pub enum LayerError {
    #[error("text table {0} is not in the store")]
    NoTexts(String),
    #[error("text table: {0}")]
    Texts(String),
    #[error("ui core: {0}")]
    Core(String),
}

/// What the layer shows.
#[derive(Debug, Clone)]
pub struct LayerConfig {
    pub lang: String,
    /// Root context loaded at start.
    pub root: String,
    /// Lua that stands in for the exe API and the game core (`ui::fixture::M1_STUBS`).
    pub stubs: Option<String>,
    /// An event fired once after the root is loaded (the panels wait for it).
    pub start_event: Option<String>,
    /// Contexts the exe makes root controls of its own (outside the root's XML): `ToolTips`, which registers the
    /// tooltip manager's basic controls. Loaded after the root, so they draw in front.
    pub extra_roots: Vec<String>,
    /// `UiConfig::small_assets`.
    pub small_assets: bool,
}

impl LayerConfig {
    /// The four EUI contexts on the fixture stubs.
    pub fn m1() -> LayerConfig {
        LayerConfig {
            lang: "en_US".to_owned(),
            root: ui::fixture::M1_ROOT_NAME.to_owned(),
            stubs: Some(ui::fixture::M1_STUBS.to_owned()),
            start_event: Some(ui::fixture::M1_START_EVENT.to_owned()),
            extra_roots: Vec::new(),
            small_assets: false,
        }
    }
}

impl LayerConfig {
    /// M2: the real `InGame` root; the exe API and the game come through [`LayerHooks`].
    pub fn m2() -> LayerConfig {
        LayerConfig {
            lang: "en_US".to_owned(),
            root: "InGame".to_owned(),
            stubs: None,
            start_event: None,
            // LeaderHeadRoot is engine-loaded (no XML names it): the leader screen of the MP form (K12, no 3D head).
            extra_roots: vec!["ToolTips".to_owned(), "LeaderHeadRoot".to_owned()],
            small_assets: true,
        }
    }
}

/// What the M2 layer adds to the config: the exe API, the context filter, and what to do before the root loads.
#[derive(Default)]
#[allow(clippy::type_complexity)]
pub struct LayerHooks {
    /// Puts the exe API into the base globals of the Lua state (xe_api::ExeApi::base_setup).
    pub base_setup: Option<Box<dyn Fn(&lua_sys::Lua, &lua_sys::Table<'_>) -> lua_sys::Result<()>>>,
    /// Which contexts may load (SIMP-exe-4); the others stay empty and hidden.
    pub context_filter: Option<Rc<dyn Fn(&str) -> bool>>,
    /// Runs after the core exists and before the root context loads (the game is bound here).
    pub before_root: Option<Box<dyn FnOnce(&UiCore)>>,
    /// The localizer the exe API shares with the layer's text (from [`load_localizer`]); made here when `None`.
    pub localizer: Option<Rc<loc::Localizer>>,
    /// Runs after the root loaded (fire the init event).
    pub after_root: Option<Box<dyn FnOnce(&UiCore)>>,
}

/// The localizer of the texts of `cfg.lang` in the store, to share with the exe API.
pub fn load_localizer(store: &Rc<UiStore>, lang: &str) -> Result<Rc<loc::Localizer>, LayerError> {
    let text_id = assets::UiFiles::text_id(lang);
    let bytes = store
        .bytes(text_id.as_str())
        .ok_or_else(|| LayerError::NoTexts(text_id.as_str().to_owned()))?;
    let texts = assets::decode_texts(&bytes).map_err(|e| LayerError::Texts(e.to_string()))?;
    Ok(Rc::new(crate::text::make_localizer(store, lang, texts)))
}

/// What the layer reports for the debug counters (`ui.*`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LayerStats {
    pub core: UiStats,
    pub draw_cmds: usize,
    pub vertices: usize,
    pub text_layouts: u64,
    pub text_quads: u64,
    pub tex_loaded: usize,
    pub tex_missing: usize,
    /// Background texture loading: requested, arrived, refused, and waiting (icon atlases picked through the database).
    pub tex_lazy: LazyStats,
    pub sounds: u64,
}

pub struct UiLayer {
    core: UiCore,
    store: Rc<UiStore>,
    cells: Rc<HostCells>,
    text_stats: Rc<std::cell::Cell<ui_text::TextStats>>,
    last_ms: Option<u64>,
    draw_cmds: usize,
    vertices: usize,
    /// Ids the renderer asked for that were not in the store yet (they are on their way): tried again at each call of
    /// `resolve_textures`, because the renderer asks for an id only once.
    waiting: std::cell::RefCell<std::collections::BTreeSet<TextureId>>,
}

impl UiLayer {
    /// Builds the core over the store, loads the root context and fires the start event.
    /// `size` is the physical size of the window.
    pub fn new(
        store: Rc<UiStore>,
        cfg: &LayerConfig,
        size: (u32, u32),
    ) -> Result<UiLayer, LayerError> {
        Self::with_hooks(store, cfg, size, LayerHooks::default())
    }

    /// [`UiLayer::new`] with the hooks of the M2 mode.
    pub fn with_hooks(
        store: Rc<UiStore>,
        cfg: &LayerConfig,
        size: (u32, u32),
        hooks: LayerHooks,
    ) -> Result<UiLayer, LayerError> {
        let LayerHooks {
            base_setup,
            context_filter,
            before_root,
            localizer,
            after_root,
        } = hooks;
        let text_id = assets::UiFiles::text_id(&cfg.lang);
        let bytes = store
            .bytes(text_id.as_str())
            .ok_or_else(|| LayerError::NoTexts(text_id.as_str().to_owned()))?;
        let text_stats = Rc::<std::cell::Cell<ui_text::TextStats>>::default();
        let text = match localizer {
            Some(l) => ClientText::with_localizer(Rc::clone(&store), l, Rc::clone(&text_stats)),
            None => {
                let texts =
                    assets::decode_texts(&bytes).map_err(|e| LayerError::Texts(e.to_string()))?;
                ClientText::new(Rc::clone(&store), &cfg.lang, texts, Rc::clone(&text_stats))
            }
        };
        let cells = Rc::new(HostCells::default());
        let stubs = cfg.stubs.clone();
        let mut core = UiCore::new(
            UiConfig {
                small_assets: cfg.small_assets,
                ..UiConfig::default()
            },
            store.files().clone(),
            Rc::new(StoreFiles(Rc::clone(&store))),
            Box::new(text),
            Box::new(ClientHost::new(Rc::clone(&store), Rc::clone(&cells))),
            move |lua, g| {
                if let Some(s) = &stubs {
                    lua.load(s)
                        .set_name("=ui_stubs")
                        .set_environment(g.clone())
                        .exec()?;
                }
                match &base_setup {
                    Some(f) => f(lua, g),
                    None => Ok(()),
                }
            },
        )
        .map_err(|e| LayerError::Core(e.to_string()))?;
        core.resize(size.0.max(1), size.1.max(1));
        core.set_context_filter(context_filter);
        // SIMPLIFIED(SIMP-exe-955): the save window's name field, its delete button and the game menu's quick save do
        // nothing here (the server names its saves; UI.QuickSave and UI.DeleteSavedGame are stubs): they stay hidden.
        core.set_removed_controls(
            [
                ("SaveMenu", "NameBoxFrame"),
                ("SaveMenu", "NameBox"),
                ("SaveMenu", "Delete"),
                ("GameMenu", "QuickSaveButton"),
            ]
            .map(|(c, i)| (c.to_owned(), i.to_owned()))
            .to_vec(),
        );
        if let Some(f) = before_root {
            f(&core);
        }
        core.load_root(&cfg.root);
        for r in &cfg.extra_roots {
            let id = core.load_root(r);
            // The engine shows the leader screen only when an AI speaks (Events.AILeaderMessage shows it through its own script).
            if r == "LeaderHeadRoot" {
                core.shared().set_hide(id, true);
            }
        }
        if let Some(f) = after_root {
            f(&core);
        }
        if let Some(ev) = &cfg.start_event {
            let driver = core
                .runtime()
                .create_context("ui_driver", |_, _| Ok(()))
                .map_err(|e| LayerError::Core(e.to_string()))?;
            if let Err(e) = driver.exec(&format!("__fire_event('{ev}')"), "=ui_driver") {
                tracing::warn!("start event {ev}: {e}");
            }
        }
        Ok(UiLayer {
            core,
            store,
            cells,
            text_stats,
            last_ms: None,
            draw_cmds: 0,
            vertices: 0,
            waiting: Default::default(),
        })
    }

    /// The app's projection of world points (for WorldAnchor), set each frame.
    pub fn set_projector(&self, p: Option<crate::host::Projector>) {
        *self.cells.projector.borrow_mut() = p;
    }

    pub fn core(&self) -> &UiCore {
        &self.core
    }

    pub fn store(&self) -> &Rc<UiStore> {
        &self.store
    }

    /// Mouse, wheel, keys and text; `Resized` changes the UI scale.
    pub fn handle_input(&mut self, ev: &InputEvent) -> InputResult {
        self.core.handle_input(ev)
    }

    /// The window changed size (physical pixels): the UI scale and the full-screen anchors follow.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.core.resize(width.max(1), height.max(1));
    }

    /// Runs the timers and animations and draws into `out` (cleared first).
    pub fn frame(&mut self, now_ms: u64, out: &mut UiDrawList) {
        let dt = match self.last_ms {
            Some(last) => (now_ms.saturating_sub(last) as f32 / 1000.0).min(0.1),
            None => 0.0,
        };
        self.last_ms = Some(now_ms);
        self.cells.now.set(now_ms as f64 / 1000.0);
        self.core.update(dt);
        self.core.draw(out);
        self.draw_cmds = out.commands().len();
        self.vertices = out.vertices().len();
    }

    /// The textures the renderer asked for (`UiTextures::take_wanted`), as blobs ready to upload.
    /// An id that is not in the store is left out; one that is on its way (`UiStore::is_loading`) is kept and
    /// answered by a later call, when it has arrived.
    pub fn resolve_textures(&self, wanted: &[TextureId]) -> Vec<ReadyTexture> {
        let mut waiting = self.waiting.borrow_mut();
        let mut ids: std::collections::BTreeSet<TextureId> = std::mem::take(&mut *waiting);
        ids.extend(wanted.iter().copied());
        ids.into_iter()
            .filter_map(|id| {
                let Some(blob) = self.store.texture_blob(id) else {
                    if self.store.is_loading(id) {
                        waiting.insert(id);
                    }
                    return None;
                };
                let (format, bytes) = upload_form(&blob, self.store.is_font_texture(id));
                Some(ReadyTexture {
                    id,
                    width: blob.width,
                    height: blob.height,
                    content: self.store.texture_content(id, &blob),
                    format,
                    bytes,
                })
            })
            .collect()
    }

    /// The button sounds asked for since the last call, oldest first. The app feeds them to
    /// `audio::UiAudio::on_button`.
    pub fn take_button_sounds(&self) -> Vec<(audio::ButtonEvent, audio::ButtonFlags)> {
        self.cells.pending_sounds.take()
    }

    /// The row of the `Cursors` table the scripts last asked for with `UIManager:SetUICursor` (0, the arrow, before any).
    pub fn cursor(&self) -> i32 {
        self.cells.cursor.get()
    }

    pub fn stats(&self) -> LayerStats {
        let t = self.text_stats.get();
        LayerStats {
            core: self.core.stats(),
            draw_cmds: self.draw_cmds,
            vertices: self.vertices,
            text_layouts: t.layouts,
            text_quads: t.quads,
            tex_loaded: self.store.texture_count(),
            tex_missing: self.store.missing_textures().len(),
            tex_lazy: self.store.lazy_stats(),
            sounds: self.cells.sounds.get(),
        }
    }
}

/// A texture ready for `UiTextures::upload`: level 0 only, in a format the renderer takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadyTexture {
    pub id: TextureId,
    /// Size of the stored texture (padded to whole blocks for a compressed variant).
    pub width: u32,
    pub height: u32,
    /// The part of it that holds the image (`UiTextureData::content`); `width` x `height` unless
    /// the variant is padded.
    pub content: (u32, u32),
    pub format: TextureFormat,
    pub bytes: Vec<u8>,
}

/// Level 0 of a blob in the form the GPU takes. `R8` is L8 in the original: luminance with alpha 1, which
/// the renderer's `R8` would sample as red only, so it goes up as `Rgba8`. A glyph atlas is the same bytes
/// read as coverage: white with the byte as alpha.
pub fn upload_form(blob: &TextureBlob, font: bool) -> (TextureFormat, Vec<u8>) {
    let level0 = blob.mips.first().cloned().unwrap_or_default();
    match blob.format {
        TextureFormat::R8 if font => (
            TextureFormat::Rgba8,
            level0.iter().flat_map(|&a| [255, 255, 255, a]).collect(),
        ),
        TextureFormat::R8 => (
            TextureFormat::Rgba8,
            level0.iter().flat_map(|&l| [l, l, l, 255]).collect(),
        ),
        f => (f, level0),
    }
}
