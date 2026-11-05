//! The original UI (EUI) as a layer of the app, behind a switch (`?ui=m1`: the M1 fixture).
//!
//! `OriginalUi` fetches the `ui` asset set through the app's loader, builds the [`ui_client::UiLayer`] when
//! it has arrived, and each frame turns input into UI messages, runs the contexts and fills the draw list
//! the renderer's `UiGame` pass draws. The textures the pass asks for go up after the frame.

use assets::AssetLoader;
use draw_list::UiDrawList;
use platform::InputEvent;
use render::{Renderer, UiTextureData};
use ui_client::{BootPlan, BootState, LayerConfig, UiBoot, UiLayer};

/// `ui` is a dev switch for now: only the M1 fixture (four EUI contexts on stubs) is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// The `ui` set is being fetched.
    Loading,
    Running,
    /// Boot or layer failed; the app goes on without the original UI.
    Failed,
}

enum State {
    Boot(Box<UiBoot>),
    Running(Box<UiLayer>),
    Failed,
}

pub struct OriginalUi {
    state: State,
    config: LayerConfig,
    size: (u32, u32),
    logged_loading: bool,
    /// Wanted textures the store could not give, reported once each.
    unresolved: std::collections::BTreeSet<u64>,
}

/// What a frame did, for the counters and the tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UiFrame {
    pub uploaded: usize,
    pub unresolved: usize,
}

impl OriginalUi {
    /// M1: the four EUI contexts on the fixture stubs.
    pub fn m1() -> OriginalUi {
        OriginalUi {
            state: State::Boot(Box::new(UiBoot::new(BootPlan::m1()))),
            config: LayerConfig::m1(),
            size: (0, 0),
            logged_loading: false,
            unresolved: Default::default(),
        }
    }

    /// The variants the device takes, for the textures the boot fetches from now on.
    pub fn set_texture_profile(&mut self, profile: &assets::PlatformProfile) {
        if let State::Boot(boot) = &mut self.state
            && boot.profile() != profile
        {
            boot.set_profile(profile.clone());
        }
    }

    pub fn phase(&self) -> Phase {
        match self.state {
            State::Boot(_) => Phase::Loading,
            State::Running(_) => Phase::Running,
            State::Failed => Phase::Failed,
        }
    }

    pub fn layer(&self) -> Option<&UiLayer> {
        match &self.state {
            State::Running(l) => Some(l),
            _ => None,
        }
    }

    /// One frame before the renderer draws: fetches the set, runs the layer and fills `out`. `size` is the
    /// physical size of the window. Returns false while there is nothing to draw (`out` is left empty).
    pub fn begin_frame(
        &mut self,
        now_ms: u64,
        loader: Option<&mut AssetLoader>,
        size: (u32, u32),
        out: &mut UiDrawList,
    ) -> bool {
        if let State::Boot(boot) = &mut self.state {
            let Some(loader) = loader else {
                tracing::warn!("the original UI needs the assets; there are none");
                self.state = State::Failed;
                return false;
            };
            if size.0 == 0 || size.1 == 0 {
                return false;
            }
            match boot.update(loader) {
                BootState::Loading { done, total } => {
                    if !self.logged_loading && total > 0 {
                        self.logged_loading = true;
                        tracing::info!(total, done, "loading the ui set");
                    }
                    return false;
                }
                BootState::Failed(msg) => {
                    tracing::error!("the original UI is off: {msg}");
                    self.state = State::Failed;
                    return false;
                }
                BootState::Ready => {
                    let store = boot.store().expect("a ready boot has its store");
                    // The fixture stubs lack the enum tables the real contexts read (`PopupPriority` in TopPanel).
                    let hooks = ui_client::LayerHooks {
                        base_setup: Some(Box::new(|lua, g| {
                            let t = lua.create_table();
                            for (i, name) in exe_api::enums::POPUP_PRIORITY.iter().enumerate() {
                                t.raw_set(*name, i as i32)?;
                            }
                            g.raw_set("PopupPriority", t)
                        })),
                        ..ui_client::LayerHooks::default()
                    };
                    match UiLayer::with_hooks(store, &self.config, size, hooks) {
                        Ok(layer) => {
                            tracing::info!(
                                controls = layer.stats().core.controls,
                                "the original UI is running"
                            );
                            self.size = size;
                            self.state = State::Running(Box::new(layer));
                        }
                        Err(e) => {
                            tracing::error!("the original UI is off: {e}");
                            self.state = State::Failed;
                            return false;
                        }
                    }
                }
            }
        }
        let State::Running(layer) = &mut self.state else {
            return false;
        };
        if size != self.size && size.0 > 0 && size.1 > 0 {
            self.size = size;
            layer.resize(size.0, size.1);
        }
        layer.frame(now_ms, out);
        true
    }

    /// After the renderer drew: uploads the textures the frame asked for, so the next frame has them.
    pub fn end_frame(&mut self, renderer: &mut Renderer) -> UiFrame {
        let State::Running(layer) = &self.state else {
            return UiFrame::default();
        };
        let wanted = renderer.ui_textures_mut().take_wanted();
        let mut frame = UiFrame::default();
        let resolved = layer.resolve_textures(&wanted);
        for tex in &resolved {
            let data = UiTextureData {
                width: tex.width,
                height: tex.height,
                format: tex.format,
                mips: vec![tex.bytes.clone()],
                content: Some(tex.content),
            };
            match renderer.ui_textures_mut().upload(tex.id, &data) {
                Ok(()) => frame.uploaded += 1,
                Err(e) => tracing::warn!(texture = tex.id.0, "ui texture upload: {e}"),
            }
        }
        for id in wanted
            .iter()
            .filter(|w| resolved.iter().all(|r| r.id != **w))
        {
            frame.unresolved += 1;
            if self.unresolved.insert(id.0) {
                tracing::warn!(
                    texture = id.0,
                    "the renderer wants a ui texture the set did not give"
                );
            }
        }
        frame
    }

    /// Mouse, wheel, keys and text for the UI. `true`: the UI used the event and the world must not see it.
    pub fn handle_input(&mut self, ev: &InputEvent) -> bool {
        match &mut self.state {
            State::Running(layer) => layer.handle_input(ev) == ui_client::InputResult::Consumed,
            _ => false,
        }
    }
}
