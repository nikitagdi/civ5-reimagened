//! UI v2 scripts: the JS of skins and mods, run in Web Workers and supervised from Rust.
//!
//! A skin's script never runs on the main thread. Each script context (the skin, or one mod) is one worker;
//! [`ScriptSupervisor`] starts it with the script files and a snapshot of the model the scripts may read,
//! then sends it, at most once per frame, the part of the frame's model delta the context watches plus the
//! markup events addressed to it. Nothing changed means nothing is sent. The worker answers with state
//! writes, intents, island commands, setting writes and log lines, which the host takes with
//! [`ScriptSupervisor::drain`] at the start of the next frame and hands to the actions module and the runtime.
//!
//! The messages are JSON texts (see [`proto`]); the worker side lives in the page (`ui2/worker.js`,
//! `ui2/api.js`). Workers are reached through [`platform::ScriptHost`], so this crate knows nothing of the
//! browser and is tested natively with [`testing::MockScriptHost`].

#![forbid(unsafe_code)]

pub mod proto;
mod supervisor;
pub mod testing;

use std::sync::Arc;

use platform::ScriptHostError;
use thiserror::Error;
use ui2_model::wire::{IslandCmd, ScriptCtxId, ScriptEvent, StateOp, WireIntent};
use ui2_model::{FrameDelta, ModelStore, Namespace, Schema, WatchSet};

pub use supervisor::ScriptSupervisor;

/// The version of the `ui` API scripts are written against. A package whose major differs is refused.
pub const UI_SCRIPT_API: &str = "1.0.0";

/// The most script contexts that run at once (the skin and seven mods).
pub const MAX_CONTEXTS: usize = 8;

/// Whose local state a context writes: the skin's `state.*`, or `modstate.<id>.*` of a mod.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum StateNs {
    Skin,
    Mod(Arc<str>),
}

impl StateNs {
    /// The namespace of the model the state lives in.
    #[must_use]
    pub fn namespace(&self) -> Namespace {
        match self {
            StateNs::Skin => Namespace::State,
            StateNs::Mod(id) => Namespace::ModState(id.clone()),
        }
    }
}

/// Whose settings a context reads and writes (the actions module keeps them under a prefix of its own).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SettingsNs {
    /// The settings of a skin, by skin id.
    Skin(Arc<str>),
    /// The settings of a mod, by mod id.
    Mod(Arc<str>),
}

/// What a context runs: its files and what it may see and do.
#[derive(Clone, Debug)]
pub struct ScriptBundle {
    /// `(path in the package, source)`, in execution order.
    pub files: Vec<(String, Arc<str>)>,
    /// The model paths the scripts read (`top`, `production.groups`); their own state and, for the skin, the
    /// `ui` root are added by the supervisor.
    pub reads: Vec<String>,
    pub state_ns: StateNs,
    pub settings_ns: SettingsNs,
    /// `None`: the scripts may raise every intent of the schema's catalog (the skin). `Some("mods.<id>.")`:
    /// only intents with this prefix (a mod); others are dropped by the supervisor.
    pub intent_prefix: Option<String>,
}

impl ScriptBundle {
    /// The prefixes of the model the context watches: `reads`, its own state namespace when it is mounted and,
    /// for the skin, the `ui` root. Duplicates removed, order kept.
    #[must_use]
    pub fn watch_prefixes(&self, store: &ModelStore) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut add = |p: String| {
            if !out.contains(&p) {
                out.push(p);
            }
        };
        for r in &self.reads {
            add(r.clone());
        }
        if self.state_ns == StateNs::Skin {
            add("ui".to_string());
        }
        let ns = self.state_ns.namespace();
        if store.is_mounted(&ns) {
            add(ns.root_name());
        }
        out
    }
}

/// The page's environment as scripts see it (`ui.env`).
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptEnv {
    /// The language of the game texts (`en_US`).
    pub lang: String,
    /// The window in physical pixels.
    pub viewport: (u32, u32),
    /// The player's UI scale.
    pub scale: f32,
    /// The active style cell of the skin (which of its breakpoints apply).
    pub cell: u32,
    /// The id of the active skin.
    pub skin: String,
}

/// One schema the context's model is made of: the schema of the model, or a mounted fragment, with the
/// store-wide ids its local ids are shifted by.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SchemaPart {
    pub schema: &'static Schema,
    /// Store-wide id of the part's field 0.
    pub field_base: u32,
    /// Store-wide id of the part's root 0.
    pub root_base: u16,
}

/// What a context starts from: the model it watches, its settings and the environment. Built from the store
/// with [`ScriptInit::from_store`]; a restarted worker gets a fresh one.
#[derive(Clone, Debug)]
pub struct ScriptInit {
    /// UMD1 snapshot of the roots the context watches.
    pub model_umd1: String,
    /// `(key, JSON text)` of the context's settings, as the actions module keeps them.
    pub settings: Vec<(String, String)>,
    pub env: ScriptEnv,
    /// Development mode (`?ui2dev=`): the model mirror in the worker is frozen, so a script that writes to it
    /// fails loudly.
    pub dev: bool,
    pub(crate) watch: WatchSet,
    pub(crate) prefixes: Vec<String>,
    pub(crate) parts: Vec<SchemaPart>,
}

impl ScriptInit {
    /// The init of a context over the current model.
    ///
    /// # Errors
    /// A path of `bundle.reads` that is neither in the schema nor in a mounted fragment.
    pub fn from_store(
        store: &ModelStore,
        bundle: &ScriptBundle,
        settings: Vec<(String, String)>,
        env: ScriptEnv,
    ) -> Result<ScriptInit, ScriptError> {
        let prefixes = bundle.watch_prefixes(store);
        let refs: Vec<&str> = prefixes.iter().map(String::as_str).collect();
        let watch =
            WatchSet::from_store(store, &refs).map_err(|e| ScriptError::Reads(e.to_string()))?;
        let mut parts = vec![SchemaPart {
            schema: store.schema(),
            field_base: 0,
            root_base: 0,
        }];
        for p in &prefixes {
            let Some(ns) = Namespace::of_path(p) else {
                continue;
            };
            if let (Some(schema), Some((root, field))) =
                (store.mount_schema(&ns), store.mount_ids(&ns))
                && !parts.iter().any(|x| x.field_base == field.0)
            {
                parts.push(SchemaPart {
                    schema,
                    field_base: field.0,
                    root_base: root.0,
                });
            }
        }
        Ok(ScriptInit {
            model_umd1: store.snapshot_umd1(&watch),
            settings,
            env,
            dev: false,
            watch,
            prefixes,
            parts,
        })
    }

    /// The model prefixes the context watches (and may read).
    #[must_use]
    pub fn prefixes(&self) -> &[String] {
        &self.prefixes
    }
}

/// What one frame gives the scripts. The supervisor filters the delta by each context's watch set and picks
/// the events addressed to it.
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameInput<'a> {
    pub delta: Option<&'a FrameDelta>,
    pub events: &'a [ScriptEvent],
    /// A new environment (window size, scale, language), if it changed this frame.
    pub env: Option<&'a ScriptEnv>,
}

/// How bad a log line is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// A line for the client log from a script or about one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogLine {
    pub level: LogLevel,
    pub text: String,
    /// The script file and line, when known.
    pub file: Option<String>,
    pub line: Option<u32>,
}

/// What the workers sent since the last drain, for the host to apply.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScriptOutput {
    /// State writes, in the order the contexts made them.
    pub state_ops: Vec<(ScriptCtxId, StateOp)>,
    /// Intents (origin `Script`); a mod's are already limited to its own prefix.
    pub intents: Vec<(ScriptCtxId, WireIntent)>,
    pub island_cmds: Vec<(ScriptCtxId, IslandCmd)>,
    /// `(context, key, JSON text)` writes; `None` removes the key.
    pub settings: Vec<(ScriptCtxId, String, Option<String>)>,
    /// Contexts that were switched off since the last drain (their panels show a stub).
    pub failed: Vec<ScriptCtxId>,
    /// Log lines of the scripts and of the supervisor about them.
    pub log: Vec<(ScriptCtxId, LogLine)>,
}

impl ScriptOutput {
    /// Whether there is nothing to apply.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.state_ops.is_empty()
            && self.intents.is_empty()
            && self.island_cmds.is_empty()
            && self.settings.is_empty()
            && self.failed.is_empty()
            && self.log.is_empty()
    }
}

/// Where a context is.
#[derive(Clone, Debug, PartialEq)]
pub enum ScriptStatus {
    /// The worker was given its files and has not said `ready` yet.
    Starting,
    Running,
    /// A batch has waited for its answer this long.
    Busy {
        ms: f64,
    },
    /// The worker was killed and a new one is starting.
    Restarting,
    /// Switched off until the skin changes; the reason is for the log.
    Disabled {
        reason: String,
    },
}

/// Counters for the stats overlay.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScriptStats {
    /// Contexts that exist (any status).
    pub contexts: u32,
    /// Contexts that said `ready` and are not switched off.
    pub running: u32,
    /// Messages sent to workers and received from them since the start.
    pub posted: u64,
    pub received: u64,
    /// Error lines and failed contexts since the start.
    pub errors: u64,
    /// Workers restarted since the start.
    pub restarts: u64,
    /// The `busy_ms` the workers reported in the last drain, summed.
    pub worker_ms: f64,
}

/// Why a context could not start.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ScriptError {
    #[error("too many script contexts: at most {MAX_CONTEXTS} run at once")]
    TooMany,
    #[error(transparent)]
    Host(#[from] ScriptHostError),
    #[error("scripts read a path the model does not have: {0}")]
    Reads(String),
    #[error("the scripts are written for API {found}, this client has {UI_SCRIPT_API}")]
    Api { found: String },
}

/// Check the API version a package declares: the major must be this client's.
///
/// # Errors
/// Another major, or a text that is not a version.
pub fn check_api(declared: &str) -> Result<(), ScriptError> {
    let major = |v: &str| v.split('.').next().and_then(|m| m.parse::<u32>().ok());
    match (major(declared), major(UI_SCRIPT_API)) {
        (Some(a), Some(b)) if a == b => Ok(()),
        _ => Err(ScriptError::Api {
            found: declared.to_string(),
        }),
    }
}

/// The context id as the worker sees it: `skin`, `mod:<id>`.
#[must_use]
pub fn ctx_name(ctx: &ScriptCtxId) -> String {
    match ctx {
        ScriptCtxId::Skin => "skin".to_string(),
        ScriptCtxId::Mod(id) => format!("mod:{id}"),
    }
}
