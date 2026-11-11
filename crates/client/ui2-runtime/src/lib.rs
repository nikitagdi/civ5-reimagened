//! UI v2 runtime: turns a skin package and the stream of UI model changes into a node tree, styles, layout,
//! input handling and per-layer draw lists.
//!
//! One code path for the browser (wasm) and native tests: no GPU, no browser bindings, no clock and no file
//! access here. The host owns the runtime, feeds it the environment, model deltas and input, and hands the
//! draw lists to the render side.
//!
//! Present state: the node arena ([`arena`]), instantiation from templates with node paths
//! ([`instantiate`]), error reporting ([`error`]), style selection ([`style`]), scalar bindings with their
//! dependency index and the skin's texts, text leaves and inline layout on `ui2-text` ([`text`]), box layout
//! with taffy over the arena ([`layout`]), draw lists per chunk set on the `ui2-draw` scene, the frame
//! ([`frame`]: [`Runtime::apply_delta`], [`Runtime::frame`], [`Runtime::drain_output`]), input
//! ([`input`]: [`Runtime::handle_input`], hit-testing, interaction states, pointer capture, the wheel, modal
//! screens), list and condition bindings (`for` by key with sorting, filtering and virtual lists; `if` bodies
//! that sleep while false), map anchors ([`anchor`]: [`Runtime::set_anchor_screen`]) and rectangle dumps
//! ([`debug`]).
//!
//! Zero-work rule: every pass is driven by the arena's dirty queues; a frame without changes checks a few
//! counters and touches no node.

#![forbid(unsafe_code)]

pub mod anchor;
mod anim;
pub mod arena;
mod bind;
pub mod debug;
pub mod doc;
pub mod error;
pub mod frame;
pub mod input;
pub mod instantiate;
mod island;
pub mod layout;
mod loc;
mod paint;
pub mod panel;
mod stats;
pub mod style;
#[cfg(feature = "testkit")]
pub mod testkit;
pub mod text;

use std::rc::Rc;
use std::sync::Arc;

use ui2_draw::{ChunkId, ChunkItems, LayerId};
use ui2_model::{FrameDelta, Key, ModelStore};
use ui2_package::style::{ColorV, StyleRecord};
use ui2_package::{Package, StrId, TemplateId};

pub use anchor::{AnchorChange, AnchorId, AnchorScreen};
pub use arena::{
    Arena, Dirty, HIDDEN_STYLE, Instance, LayoutSlot, Node, NodeClass, NodeId, PLACEHOLDER_STYLE,
    StateBits,
};
pub use debug::ElementRect;
pub use doc::Doc;
pub use error::{ErrorCode, ErrorSink, NodePath, UiError};
pub use frame::{FrameOutput, UiOutput};
pub use input::{CARET_BLINK_MS, InputOutcome, PointerCapture};
use instantiate::Spawn;
pub use instantiate::{MAX_INSTANCE_NODES, Source};
pub use layout::LayoutStyles;
pub use panel::{PanelId, PanelStatus};
pub use platform::ui2::{TextInputRequest, TextInputState, TextSel};
pub use stats::FrameStats;
use style::select::EnvChange;
pub use style::{Scale, StyleEnv, StyleIndex};
pub use text::{FontBook, load_faces};
pub use ui2_text::TextMeasure;

/// The environment of the UI: window size, pixel density, UI scale and theme.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Env {
    /// The window in physical pixels.
    pub viewport_px: (u32, u32),
    /// Device pixels per CSS pixel of the page (1 to 2).
    pub dpr: f32,
    /// The player's UI scale (0.75 to 1.5).
    pub ui_scale: f32,
    /// The theme index of the skin's manifest (0 is the default).
    pub theme: u16,
}

impl Default for Env {
    fn default() -> Self {
        Self {
            viewport_px: (1920, 1080),
            dpr: 1.0,
            ui_scale: 1.0,
            theme: 0,
        }
    }
}

/// What the host lends the runtime.
#[derive(Clone, Default)]
pub struct Services {
    /// The game's localizer: texts of the game (`Text::Key` values, `loc(...)` of keys the skin does not
    /// have), the language the skin's own texts are shown in, and the composition of those texts. Without
    /// one the skin's texts show in its first language.
    pub loc: Option<Rc<::loc::Localizer>>,
}

/// A picture of a texture the renderer owns: the texture and the rectangle in it, normalized
/// (`[0, 0, 1, 1]` is the whole texture).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExtImage {
    /// The renderer's texture id.
    pub texture: ui2_draw::ExtTexture,
    /// Left, top, right, bottom.
    pub uv: [f32; 4],
}

/// Finds the texture of a game asset reference of the skin (`data-asset`, `data-bind-src`), such as a
/// `ui:tx` texture or a portrait: `None` when the game has no such picture. The minimap (`render:minimap`)
/// is known to the runtime and never asked for.
pub type AssetResolver = Rc<dyn Fn(&str) -> Option<ExtImage>>;

/// The skin cannot be used at all; the host keeps the previous UI.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LoadError {
    /// A fatal problem: no document, a root that cannot be built, a font that does not parse.
    #[error("the skin cannot be used: {0}")]
    Fatal(String),
}

/// The tables the passes read: the skin's package (and the mod fragments mounted into it).
pub(crate) type Pkg = Doc;

/// The runtime of one skin.
pub struct Runtime {
    pkg: Arc<Doc>,
    env: Env,
    style_env: StyleEnv,
    style_index: StyleIndex,
    env_change: EnvChange,
    /// The node under the pointer and its ancestors (they all have `HOVER`).
    hover: Vec<NodeId>,
    svc: Services,
    arena: Arena,
    errors: ErrorSink,
    layout_styles: LayoutStyles,
    fonts: FontBook,
    binder: bind::Binder,
    /// What markup asked since the last [`drain_output`](Self::drain_output).
    output: UiOutput,
    /// Pointer, focus and key state.
    input: input::InputState,
    /// Times the passes of a frame, when the host gives one.
    clock: Option<ui2_draw::Clock>,
    /// Inline roots that fit their text to their box (`data-fit`).
    fit_roots: Vec<NodeId>,
    /// Running transitions.
    anim: anim::Animator,
    /// Where game asset pictures are found.
    assets: Option<AssetResolver>,
    /// Map anchors.
    anchors: anchor::Anchors,
    /// Where the keyed nodes of script islands are.
    islands: island::IslandIndex,
    /// The panels: the skin's top-level parts and the mounted mod fragments.
    panels: panel::Panels,
}

impl Runtime {
    /// Builds the document of a package (template 0) for an environment.
    pub fn new(pkg: Arc<Package>, env: Env, svc: Services) -> Result<Runtime, LoadError> {
        if pkg.templates().is_empty() {
            return Err(LoadError::Fatal(
                "the package has no document template".into(),
            ));
        }
        let (style_env, env_ok) = StyleEnv::of(pkg.styles(), &env);
        let mut rt = Runtime {
            style_index: StyleIndex::new(pkg.styles()),
            layout_styles: LayoutStyles::from_styles(pkg.styles()),
            fonts: FontBook::new(&pkg),
            input: input::InputState::new(&pkg),
            binder: bind::Binder::new(loc::SkinLoc::new(
                &Doc::new(Arc::clone(&pkg)),
                svc.loc.clone(),
            )),
            pkg: Arc::new(Doc::new(pkg)),
            env,
            style_env,
            env_change: EnvChange::default(),
            hover: Vec::new(),
            svc,
            arena: Arena::new(),
            errors: ErrorSink::new(),
            output: UiOutput::default(),
            clock: None,
            fit_roots: Vec::new(),
            anim: anim::Animator::default(),
            assets: None,
            anchors: anchor::Anchors::default(),
            islands: island::IslandIndex::default(),
            panels: panel::Panels::default(),
        };
        if !env_ok {
            rt.report_bad_env(env);
        }
        let roots = rt.instantiate(TemplateId(0), None);
        let all_placeholders = roots.iter().all(|&r| {
            rt.arena
                .get(r)
                .is_some_and(|n| n.class() == NodeClass::Placeholder)
        });
        if roots.is_empty() || all_placeholders {
            return Err(LoadError::Fatal(
                "the document template cannot be built".into(),
            ));
        }
        rt.register_skin_panels();
        Ok(rt)
    }

    fn report_bad_env(&mut self, env: Env) {
        self.errors.report(UiError::global(
            ErrorCode::BadEnv,
            format!(
                "ui_scale {} and dpr {} must be finite and positive; 1 is used instead",
                env.ui_scale, env.dpr
            ),
        ));
    }

    /// Changes the environment. Nothing is recomputed here; the next frame reselects the nodes whose set
    /// varies by viewport cell (when the cell changed), repaints nodes with theme tokens (when the theme
    /// changed), lays the roots out again (when the logical viewport changed) and repaints every chunk (when
    /// the device scale changed).
    pub fn set_env(&mut self, env: Env) {
        let (new, ok) = StyleEnv::of(self.pkg.styles(), &env);
        if !ok {
            self.report_bad_env(env);
        }
        let old = self.style_env;
        self.env_change.cell |= new.cell != old.cell;
        self.env_change.theme |= new.theme != old.theme;
        self.env_change.viewport |= new.scale.viewport_css != old.scale.viewport_css;
        self.env_change.k |= new.scale.k != old.scale.k;
        self.env = env;
        self.style_env = new;
    }

    /// The style environment: viewport cell, theme and scale.
    pub fn style_env(&self) -> StyleEnv {
        self.style_env
    }

    /// The selected style record of a node.
    pub fn style(&self, id: NodeId) -> Option<&StyleRecord> {
        let node = self.arena.get(id)?;
        Some(style::node_record(
            &self.style_index,
            self.pkg.styles(),
            node,
        ))
    }

    /// A record color under the current theme.
    pub fn color(&self, c: ColorV) -> [u8; 4] {
        style::tokens::resolve(self.pkg.tokens(), self.style_env.theme, c)
    }

    /// Sets or clears interaction states on a node (`HOVER` is better set with [`set_hover`](Self::set_hover),
    /// which handles the ancestor chain). Nodes whose conditions test those states restyle in the next frame.
    pub fn set_state(&mut self, id: NodeId, bits: StateBits, on: bool) {
        style::select::set_state(&mut self.arena, id, bits, on);
    }

    /// Moves the pointer to a node (or off the UI): as in CSS, the node and all its ancestors are hovered.
    /// Only nodes that some condition tests for hover restyle.
    pub fn set_hover(&mut self, target: Option<NodeId>) {
        let mut chain = Vec::new();
        let mut at = target.filter(|&t| self.arena.contains(t));
        while let Some(id) = at {
            chain.push(id);
            at = self.arena.get(id).and_then(|n| n.parent());
        }
        let old = std::mem::take(&mut self.hover);
        for &id in old.iter().filter(|id| !chain.contains(id)) {
            style::select::set_state(&mut self.arena, id, StateBits::HOVER, false);
        }
        for &id in &chain {
            style::select::set_state(&mut self.arena, id, StateBits::HOVER, true);
        }
        self.hover = chain;
    }

    /// Switches a run-time class on a node (`class-toggle` bindings, script islands).
    pub fn set_class(&mut self, id: NodeId, class: StrId, on: bool) {
        style::select::set_class(&mut self.arena, id, class, on);
    }

    /// The skin's package.
    pub fn package(&self) -> &Arc<Package> {
        self.pkg.skin()
    }

    /// The tables the runtime reads: the skin's, with the mounted mod fragments.
    pub fn doc(&self) -> &Doc {
        &self.pkg
    }

    /// The services the host lent.
    pub fn services(&self) -> &Services {
        &self.svc
    }

    /// The current environment.
    pub fn env(&self) -> Env {
        self.env
    }

    /// The nodes.
    pub fn arena(&self) -> &Arena {
        &self.arena
    }

    /// A live node.
    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.arena.get(id)
    }

    /// The path of a node, for messages and dumps.
    pub fn path(&self, id: NodeId) -> NodePath {
        instantiate::node_path(&self.arena, &*self.pkg, id)
    }

    /// The first node (document order) with a `data-part` name.
    pub fn find_part(&self, name: &str) -> Option<NodeId> {
        instantiate::find_part(&self.arena, &*self.pkg, name)
    }

    /// Makes an instance of a template as the last children of `parent` (or at the end of the document).
    /// Returns its top-level nodes; a bad template gives a placeholder and an error.
    pub fn instantiate(&mut self, tpl: TemplateId, parent: Option<NodeId>) -> Vec<NodeId> {
        let pkg = Arc::clone(&self.pkg);
        let at = self.arena.siblings(parent).len();
        instantiate::instantiate(
            &mut self.arena,
            &*pkg,
            &self.style_env,
            &mut self.errors,
            Spawn {
                tpl,
                parent,
                at,
                key: None,
                owner: None,
            },
        )
    }

    /// Makes one more instance of the body of a `for` or `if` node, after its existing instances.
    pub fn instantiate_body(&mut self, service: NodeId, key: Option<Key>) -> Vec<NodeId> {
        let pkg = Arc::clone(&self.pkg);
        instantiate::instantiate_body(
            &mut self.arena,
            &*pkg,
            &self.style_env,
            &mut self.errors,
            service,
            key,
        )
    }

    /// Removes a node and its subtree; returns the number of nodes removed.
    pub fn remove(&mut self, id: NodeId) -> usize {
        self.arena.remove(id)
    }

    /// The nodes queued for one pass, with the flag cleared (see [`Arena::take_dirty`]).
    pub fn take_dirty(&mut self, bit: Dirty) -> Vec<NodeId> {
        self.arena.take_dirty(bit)
    }

    /// Errors reported for the first time since the last call or frame (a frame hands its new errors over in
    /// [`FrameOutput::errors`]; this returns those reported between frames, by calls such as
    /// [`instantiate`](Self::instantiate)).
    pub fn take_errors(&mut self) -> Vec<UiError> {
        self.errors.take()
    }

    /// Takes the changes of a model frame: queues the bindings that read a changed field (nothing is
    /// evaluated before the next frame).
    pub fn apply_delta(&mut self, d: &FrameDelta, m: &ModelStore) {
        self.binder.apply_delta(&mut self.arena, &self.pkg, d, m);
    }

    /// Changes the game's localizer (and the language). Every binding whose text depends on the language is
    /// evaluated again in the next frame.
    pub fn set_localizer(&mut self, loc: Rc<::loc::Localizer>) {
        self.binder.loc.set_game(&self.pkg, Some(Rc::clone(&loc)));
        self.svc.loc = Some(loc);
        self.binder.queue_localized();
    }

    /// Sets where game asset pictures are found (the renderer's texture ids). Every game asset picture is
    /// painted again in the next frame: the host calls this again when pictures that were missing became
    /// known. Without a resolver such pictures are reported once and not drawn (the minimap is drawn).
    pub fn set_asset_resolver(&mut self, resolver: AssetResolver) {
        self.assets = Some(resolver);
        let pkg = Arc::clone(&self.pkg);
        let pictures: Vec<NodeId> = self
            .arena
            .ids()
            .filter(|&id| {
                self.arena.get(id).is_some_and(|n| {
                    instantiate::template_node(&*pkg, n.tmpl()).is_some_and(|t| {
                        matches!(
                            t.kind,
                            ui2_package::NodeKind::Image(
                                ui2_package::ImageSrc::Asset(_) | ui2_package::ImageSrc::Bound(_)
                            )
                        )
                    })
                })
            })
            .collect();
        for id in pictures {
            self.arena.mark(id, Dirty::PAINT);
        }
    }

    /// The number of live binding rows (one per scalar binding of a live node).
    pub fn binding_rows(&self) -> usize {
        self.binder.len()
    }

    /// The draw list of a chunk as the paint pass builds it from the current state, with its layer (for tests
    /// and tools; the pass itself sets chunks on the scene). `None` when no live node roots that chunk.
    pub fn chunk_items(&mut self, chunk: ChunkId) -> Option<(LayerId, ChunkItems)> {
        let id = self.arena.id_at(chunk.0 as usize)?;
        if self.arena.get(id)?.chunk() != chunk {
            return None;
        }
        let cx = paint::PaintCtx {
            pkg: &self.pkg,
            index: &self.style_index,
            env: &self.style_env,
            anim: &self.anim,
            srcs: self.binder.srcs(),
            assets: self.assets.as_ref(),
            anchors: &self.anchors,
            fields: &self.input.fields,
        };
        let layer = paint::emit::layer_of(&self.arena, &self.pkg, id);
        let (items, _) = paint::build(&self.arena, &cx, &mut self.errors, id);
        Some((layer, items))
    }

    /// The rectangles of the laid-out boxes and text leaves, in the converter's rectangle dump format: the
    /// whole document, or the subtree of the first node with a `data-part` name (empty when there is none).
    pub fn dump_rects(&self, part: Option<&str>) -> Vec<ElementRect> {
        let under = match part {
            Some(name) => match self.find_part(name) {
                Some(id) => Some(id),
                None => return Vec::new(),
            },
            None => None,
        };
        debug::dump(&self.arena, &*self.pkg, &self.style_index, under)
    }
}
