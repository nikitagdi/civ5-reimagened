//! UI v2 runtime: turns a skin package and the stream of UI model changes into a node tree, styles, layout,
//! input handling and per-layer draw lists.
//!
//! One code path for the browser (wasm) and native tests: no GPU, no browser bindings, no clock and no file
//! access here. The host owns the runtime, feeds it the environment, model deltas and input, and hands the
//! draw lists to the render side.
//!
//! Present state: the node arena ([`arena`]), instantiation from templates with node paths
//! ([`instantiate`]), error reporting ([`error`]), style selection ([`style`]) and taffy's tree traits over the
//! arena ([`layout`]). Text leaves, bindings, input and painting build on these.
//!
//! Zero-work rule: every pass is driven by the arena's dirty queues; a frame without changes checks a few
//! counters and touches no node.

#![forbid(unsafe_code)]

pub mod arena;
pub mod error;
pub mod instantiate;
pub mod layout;
pub mod style;
#[cfg(feature = "testkit")]
pub mod testkit;

use std::rc::Rc;
use std::sync::Arc;

use ui2_model::Key;
use ui2_package::{Package, TemplateId};

pub use arena::{
    Arena, Dirty, HIDDEN_STYLE, Instance, LayoutSlot, Node, NodeClass, NodeId, PLACEHOLDER_STYLE,
    StateBits,
};
pub use error::{ErrorCode, ErrorSink, NodePath, UiError};
use instantiate::Spawn;
pub use instantiate::{MAX_INSTANCE_NODES, Source};
use style::StyleEnv;

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
    /// The game's localizer (texts of the game, `Text::Key` values). Localization is wired with the bindings;
    /// until then it may be absent.
    pub loc: Option<Rc<loc::Localizer>>,
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
    svc: Services,
    arena: Arena,
    errors: ErrorSink,
}

impl Runtime {
    /// Builds the document of a package (template 0) for an environment.
    pub fn new(pkg: Arc<Package>, env: Env, svc: Services) -> Result<Runtime, LoadError> {
        if pkg.templates().is_empty() {
            return Err(LoadError::Fatal(
                "the package has no document template".into(),
            ));
        }
        let mut rt = Runtime {
            pkg,
            env,
            style_env: StyleEnv::default(),
            svc,
            arena: Arena::new(),
            errors: ErrorSink::new(),
        };
        rt.style_env = rt.style_env_for(env);
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

    fn style_env_for(&self, env: Env) -> StyleEnv {
        let (w, h) = env.viewport_px;
        let k = env.dpr * env.ui_scale;
        let k = if k.is_finite() && k > 0.0 { k } else { 1.0 };
        StyleEnv {
            cell: self.pkg.styles().cell([w as f32 / k, h as f32 / k]),
        }
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

    /// Runs the passes that exist so far and returns the frame's counters. A frame without changes only checks
    /// the queues.
    pub fn update(&mut self) -> FrameStats {
        FrameStats {
            nodes: self.arena.len() as u32,
            ..FrameStats::default()
        }
    }
}
