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
//! with taffy over the arena ([`layout`]) and rectangle dumps ([`debug`]). List bindings, input and painting
//! build on these.
//!
//! Zero-work rule: every pass is driven by the arena's dirty queues; a frame without changes checks a few
//! counters and touches no node.

#![forbid(unsafe_code)]

pub mod arena;
mod bind;
pub mod debug;
pub mod error;
pub mod instantiate;
pub mod layout;
mod loc;
pub mod style;
#[cfg(feature = "testkit")]
pub mod testkit;
pub mod text;

use std::rc::Rc;
use std::sync::Arc;

use ui2_model::{FrameDelta, Key, ModelStore};
use ui2_package::style::{ColorV, StyleRecord};
use ui2_package::{Package, StrId, TemplateId};

pub use arena::{
    Arena, Dirty, HIDDEN_STYLE, Instance, LayoutSlot, Node, NodeClass, NodeId, PLACEHOLDER_STYLE,
    StateBits,
};
pub use debug::ElementRect;
pub use error::{ErrorCode, ErrorSink, NodePath, UiError};
use instantiate::Spawn;
pub use instantiate::{MAX_INSTANCE_NODES, Source};
pub use layout::LayoutStyles;
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

/// The skin cannot be used at all; the host keeps the previous UI.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LoadError {
    /// A fatal problem: no document, a root that cannot be built, a font that does not parse.
    #[error("the skin cannot be used: {0}")]
    Fatal(String),
}

/// Work counters of one frame. A frame without changes has every counter at zero except `nodes`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameStats {
    /// Nodes whose style record was selected again.
    pub restyled: u32,
    /// Layout runs (one per chunk root laid out).
    pub layout_runs: u32,
    /// Text runs shaped (cache misses included).
    pub shaped_runs: u32,
    /// Bindings evaluated.
    pub binds_evaluated: u32,
    /// Chunks whose draw list was rebuilt.
    pub chunks_rebuilt: u32,
    /// Live nodes.
    pub nodes: u32,
    /// Nodes any pass looked at (zero on an idle frame).
    pub visited: u32,
}

/// The runtime of one skin.
pub struct Runtime {
    pkg: Arc<Package>,
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
            binder: bind::Binder::new(loc::SkinLoc::new(&pkg, svc.loc.clone())),
            pkg,
            env,
            style_env,
            env_change: EnvChange::default(),
            hover: Vec::new(),
            svc,
            arena: Arena::new(),
            errors: ErrorSink::new(),
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

    /// The package.
    pub fn package(&self) -> &Arc<Package> {
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

    /// Errors reported for the first time since the last call.
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

    /// The number of live binding rows (one per scalar binding of a live node).
    pub fn binding_rows(&self) -> usize {
        self.binder.len()
    }

    /// Runs the passes that exist so far (bindings, styles, text, layout) and returns the frame's counters. A
    /// frame without changes only checks the queues.
    pub fn update(&mut self, m: &ModelStore, text: &mut dyn TextMeasure) -> FrameStats {
        let binds = self.binder.pass(
            &mut self.arena,
            &bind::Ctx {
                pkg: &self.pkg,
                index: &self.style_index,
                store: m,
            },
            &mut self.errors,
        );
        let change = std::mem::take(&mut self.env_change);
        let styles = style::select::pass(
            &mut self.arena,
            self.pkg.styles(),
            &self.style_index,
            &self.style_env,
            change,
        );
        let ctx = text::TextCtx {
            pkg: &self.pkg,
            fonts: &self.fonts,
            index: &self.style_index,
        };
        let texts = text::pass(&ctx, &mut self.arena, text, &mut self.errors);
        let layout = layout::pass(
            &mut self.arena,
            &self.layout_styles,
            text,
            self.style_env.scale.viewport_css,
        );
        FrameStats {
            restyled: styles.restyled,
            shaped_runs: texts.shaped,
            layout_runs: layout.runs,
            binds_evaluated: binds.evaluated,
            visited: binds.visited + styles.visited + texts.visited + layout.visited,
            nodes: self.arena.len() as u32,
            ..FrameStats::default()
        }
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
