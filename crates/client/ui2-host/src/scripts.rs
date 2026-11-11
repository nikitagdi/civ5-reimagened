//! The skin's scripts in the UI v2 frame: the supervisor of their worker, what its answers do to the actions
//! module, and the environment they see.
//!
//! A skin without scripts, or a platform without workers, has no [`Scripts`] at all: its frames do no script work.

use std::sync::Arc;

use platform::ScriptHost;
use ui2_actions::UiActions;
use ui2_model::ModelStore;
use ui2_model::wire::{ScriptCtxId, WireIntent};
use ui2_package::Package;
use ui2_runtime::Runtime;
use ui2_script::{
    LogLevel, ScriptBundle, ScriptEnv, ScriptError, ScriptInit, ScriptOutput, ScriptSupervisor,
    SettingsNs, StateNs,
};

use crate::error::LoadFailure;

/// The language of the game texts scripts see in `ui.env.lang`: the session's one language.
pub const SCRIPT_LANG: &str = "en_US";

/// The scripts of the running skin: their supervisor and the environment they were last told.
pub(crate) struct Scripts {
    pub(crate) sup: ScriptSupervisor,
    env: ScriptEnv,
}

/// The skin context's files and rights from the package; `None` when the package has no scripts.
fn skin_bundle(pkg: &Package) -> Result<Option<ScriptBundle>, LoadFailure> {
    let mut files = Vec::new();
    for (path, bytes) in pkg.scripts() {
        let code = std::str::from_utf8(bytes)
            .map_err(|_| LoadFailure::Scripts(format!("{path} is not UTF-8 text")))?;
        files.push((path.to_string(), Arc::from(code)));
    }
    if files.is_empty() {
        return Ok(None);
    }
    Ok(Some(ScriptBundle {
        files,
        reads: pkg.script_reads().map(str::to_string).collect(),
        state_ns: StateNs::Skin,
        settings_ns: SettingsNs::Skin(Arc::from(pkg.manifest().id.as_str())),
        intent_prefix: None,
    }))
}

/// What the scripts see of the window and the skin.
pub(crate) fn script_env(runtime: &Runtime, skin: &str) -> ScriptEnv {
    let env = runtime.env();
    ScriptEnv {
        lang: SCRIPT_LANG.to_string(),
        viewport: env.viewport_px,
        scale: env.ui_scale,
        cell: u32::from(runtime.style_env().cell),
        skin: skin.to_string(),
    }
}

/// The actions module's name of a context's state.
fn state_ns(ctx: &ScriptCtxId) -> ui2_actions::StateNs {
    match ctx {
        ScriptCtxId::Skin => ui2_actions::StateNs::Skin,
        ScriptCtxId::Mod(id) => ui2_actions::StateNs::Mod(id.clone()),
    }
}

/// The actions module's name of a context's settings.
fn settings_ns(ctx: &ScriptCtxId, skin: &str) -> ui2_actions::SettingsNs {
    match ctx {
        ScriptCtxId::Skin => ui2_actions::SettingsNs::Skin(Arc::from(skin)),
        ScriptCtxId::Mod(id) => ui2_actions::SettingsNs::Mod(id.clone()),
    }
}

impl Scripts {
    /// Starts the skin's context on the model as it is. `Ok(None)`: nothing runs, because the package has no
    /// scripts or the platform has no workers (the skin then works on its markup alone).
    ///
    /// # Errors
    /// The scripts are not text or read model paths this client does not have: the skin cannot be used.
    pub(crate) fn start(
        host: Box<dyn ScriptHost>,
        pkg: &Package,
        runtime: &Runtime,
        store: &ModelStore,
        actions: &mut UiActions,
    ) -> Result<Option<Scripts>, LoadFailure> {
        let Some(bundle) = skin_bundle(pkg)? else {
            return Ok(None);
        };
        let skin = pkg.manifest().id.as_str();
        let env = script_env(runtime, skin);
        let settings = actions.settings_snapshot(&settings_ns(&ScriptCtxId::Skin, skin));
        let init = ScriptInit::from_store(store, &bundle, settings, env.clone())
            .map_err(|e| LoadFailure::Scripts(e.to_string()))?;
        let mut sup = ScriptSupervisor::new(host);
        match sup.start(ScriptCtxId::Skin, bundle, init) {
            Ok(()) => Ok(Some(Scripts { sup, env })),
            Err(ScriptError::Host(e)) => {
                tracing::info!(target: "ui2_host", "skin {skin}: its scripts do not run here ({e}); markup only");
                Ok(None)
            }
            Err(e) => Err(LoadFailure::Scripts(e.to_string())),
        }
    }

    /// A supervisor with no context yet, for the mods' scripts (each mod's context is started on its own).
    pub(crate) fn for_mods(host: Box<dyn ScriptHost>, runtime: &Runtime, skin: &str) -> Scripts {
        Scripts {
            sup: ScriptSupervisor::new(host),
            env: script_env(runtime, skin),
        }
    }

    /// The environment the scripts were last told.
    pub(crate) fn env(&self) -> &ScriptEnv {
        &self.env
    }

    /// The scripts' environment, when the window, the scale or the style cell changed since it was last sent.
    pub(crate) fn env_change(&mut self, runtime: &Runtime) -> Option<ScriptEnv> {
        let env = runtime.env();
        let cell = u32::from(runtime.style_env().cell);
        if self.env.viewport == env.viewport_px
            && self.env.scale == env.ui_scale
            && self.env.cell == cell
        {
            return None;
        }
        self.env.viewport = env.viewport_px;
        self.env.scale = env.ui_scale;
        self.env.cell = cell;
        Some(self.env.clone())
    }
}

/// What applying the scripts' answers did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Applied {
    /// State and settings writes the actions module refused.
    pub(crate) refused: u32,
}

/// The scripts' state writes (in their order, after the markup's) and settings writes go to the actions module;
/// the log lines go to the client log. Intents are left in `out` for [`UiActions::apply`].
pub(crate) fn apply_writes(
    out: &ScriptOutput,
    actions: &mut UiActions,
    store: &mut ModelStore,
    skin: &str,
) -> Applied {
    let mut applied = Applied::default();
    for (ctx, op) in &out.state_ops {
        for e in actions.state_ops(state_ns(ctx), std::slice::from_ref(op), store) {
            tracing::warn!(target: "ui2_host", "script state write refused: {e}");
            applied.refused += 1;
        }
    }
    for (ctx, key, value) in &out.settings {
        if let Err(e) = actions.settings_set(&settings_ns(ctx, skin), key, value.as_deref()) {
            tracing::warn!(target: "ui2_host", "script setting `{key}` refused: {e}");
            applied.refused += 1;
        }
    }
    for (ctx, line) in &out.log {
        let at = match (&line.file, line.line) {
            (Some(f), Some(l)) => format!(" ({f}:{l})"),
            _ => String::new(),
        };
        let ctx = ui2_script::ctx_name(ctx);
        match line.level {
            LogLevel::Info => tracing::info!(target: "ui2_js", "[{ctx}] {}{at}", line.text),
            LogLevel::Warn => tracing::warn!(target: "ui2_js", "[{ctx}] {}{at}", line.text),
            LogLevel::Error => tracing::error!(target: "ui2_js", "[{ctx}] {}{at}", line.text),
        }
    }
    applied
}

/// The scripts' intents, in their order.
pub(crate) fn take_intents(out: &mut ScriptOutput) -> impl Iterator<Item = WireIntent> + '_ {
    out.intents.drain(..).map(|(_, i)| i)
}
