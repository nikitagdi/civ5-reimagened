//! The supervisor: one worker per script context, the frame batch, the watchdog and restarts, and the answers
//! turned into output for the host.

use std::sync::Arc;

use platform::{ScriptHost, ScriptHostEvent, WorkerId};
use ui2_model::wire::{Origin, ScriptCtxId, ScriptEvent, StateOp, WireIntent};
use ui2_model::{FrameDelta, Op, Schema, WatchSet};

use crate::log::LogOnce;
use crate::proto::{self, FromWorker, OutMsg};
use crate::watchdog::{KILL_AFTER_MS, Verdict, Watchdog};
use crate::{
    FrameInput, LogLevel, LogLine, MAX_CONTEXTS, SchemaPart, ScriptBundle, ScriptEnv, ScriptError,
    ScriptInit, ScriptOutput, ScriptStats, ScriptStatus, ctx_name,
};

/// Whether a script may write the state path `path` (as the worker sends it): its own state (`state`,
/// `state.<path>`; a mod's lands in `modstate.<id>`), or a field of the `ui` root marked `js` in the schema.
fn writable(schema: &Schema, path: &str) -> Result<(), String> {
    let own = path
        .strip_prefix("state")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(['.', '[']));
    if own {
        return Ok(());
    }
    if path.starts_with("ui.") {
        return if schema.paths().iter().any(|e| e.js && e.path == path) {
            Ok(())
        } else {
            Err(format!(
                "state write `{path}` dropped: not a field of `ui` scripts may write"
            ))
        };
    }
    Err(format!(
        "state write `{path}` dropped: scripts write their `state` and the `js` fields of `ui` only"
    ))
}

/// What a busy context did not get yet: the frames since it got busy, merged into one.
#[derive(Debug, Default)]
struct Backlog {
    revision: u64,
    ops: Vec<Op>,
    events: Vec<ScriptEvent>,
    env: Option<ScriptEnv>,
}

impl Backlog {
    fn is_empty(&self) -> bool {
        self.ops.is_empty() && self.events.is_empty() && self.env.is_none()
    }

    /// Add a frame. A value set again replaces the earlier set of the same place; list changes stay one by one,
    /// in order.
    fn add(&mut self, delta: Option<FrameDelta>, events: &[&ScriptEvent], env: Option<&ScriptEnv>) {
        if let Some(d) = delta {
            self.revision = d.revision;
            for op in d.ops {
                if let Op::Set { field, keys, .. } = &op {
                    self.ops.retain(|o| {
                        !matches!(o, Op::Set { field: f, keys: k, .. } if f == field && k == keys)
                    });
                }
                self.ops.push(op);
            }
        }
        self.events.extend(events.iter().map(|e| (*e).clone()));
        if let Some(env) = env {
            self.env = Some(env.clone());
        }
    }
}

struct Ctx {
    id: ScriptCtxId,
    /// `None` while a killed worker waits for its successor.
    worker: Option<WorkerId>,
    bundle: ScriptBundle,
    watch: WatchSet,
    parts: Vec<SchemaPart>,
    status: ScriptStatus,
    /// What a restart starts from besides the model: the settings as the scripts last wrote them, the last
    /// environment, the development mode.
    settings: Vec<(String, String)>,
    env: ScriptEnv,
    dev: bool,
    /// The `seq` of the last frame sent, and of the last one answered.
    seq: u64,
    acked: u64,
    /// The highest `out.n` drained: every write up to it reaches the model before the next frame is taken.
    applied: u64,
    wd: Watchdog,
    backlog: Backlog,
    handlers: Vec<String>,
}

impl Ctx {
    fn schema(&self) -> &'static Schema {
        self.parts[0].schema
    }

    fn line(&self, level: LogLevel, text: String) -> (ScriptCtxId, LogLine) {
        (
            self.id.clone(),
            LogLine {
                level,
                text: format!("ui2.js: {} {text}", ctx_name(&self.id)),
                file: None,
                line: None,
            },
        )
    }

    fn off(&self) -> bool {
        matches!(self.status, ScriptStatus::Disabled { .. })
    }

    /// The worker answers frames (it said `ready`).
    fn live(&self) -> bool {
        matches!(
            self.status,
            ScriptStatus::Running | ScriptStatus::Busy { .. }
        )
    }

    /// Remember a settings write for a restart.
    fn remember_setting(&mut self, key: &str, value: Option<&String>) {
        self.settings.retain(|(k, _)| k != key);
        if let Some(v) = value {
            self.settings.push((key.to_string(), v.clone()));
        }
    }
}

/// Runs the script contexts of the active skin and mods. Owned by the host; called twice per frame:
/// [`drain`](Self::drain) at the start, [`post`](Self::post) after the model delta was taken.
pub struct ScriptSupervisor {
    host: Box<dyn ScriptHost>,
    ctxs: Vec<Ctx>,
    /// Output produced outside `drain` (a failed post, a kill, a restart), handed over with the next drain.
    pending: ScriptOutput,
    fatal: Option<(ScriptCtxId, String)>,
    stats: ScriptStats,
    events: Vec<(WorkerId, ScriptHostEvent)>,
    once: LogOnce,
}

impl ScriptSupervisor {
    /// A supervisor over a worker host (`NullScriptHost` on a platform without workers).
    #[must_use]
    pub fn new(host: Box<dyn ScriptHost>) -> ScriptSupervisor {
        ScriptSupervisor {
            host,
            ctxs: Vec::new(),
            pending: ScriptOutput::default(),
            fatal: None,
            stats: ScriptStats::default(),
            events: Vec::new(),
            once: LogOnce::default(),
        }
    }

    /// Start a context: a new worker gets the files and the snapshot. A context of the same id is stopped
    /// first, and the errors it logged may be logged again.
    ///
    /// # Errors
    /// [`MAX_CONTEXTS`] are running, or the host could not start a worker (on the native client it never can:
    /// the skin then runs on its markup alone).
    pub fn start(
        &mut self,
        ctx: ScriptCtxId,
        bundle: ScriptBundle,
        init: ScriptInit,
    ) -> Result<(), ScriptError> {
        self.stop(&ctx);
        self.once.clear(&ctx);
        if self.ctxs.len() >= MAX_CONTEXTS {
            return Err(ScriptError::TooMany);
        }
        let worker = self.spawn(&ctx, &bundle, &init)?;
        let mut wd = Watchdog::default();
        wd.started();
        self.ctxs.push(Ctx {
            id: ctx,
            worker: Some(worker),
            bundle,
            watch: init.watch,
            parts: init.parts,
            status: ScriptStatus::Starting,
            settings: init.settings,
            env: init.env,
            dev: init.dev,
            seq: 0,
            acked: 0,
            applied: 0,
            wd,
            backlog: Backlog::default(),
            handlers: Vec::new(),
        });
        Ok(())
    }

    /// A worker with the context's files and snapshot.
    fn spawn(
        &mut self,
        ctx: &ScriptCtxId,
        bundle: &ScriptBundle,
        init: &ScriptInit,
    ) -> Result<WorkerId, ScriptError> {
        let worker = self.host.spawn()?;
        let text = proto::encode_init(&ctx_name(ctx), bundle, init, self.host.carries_bytes());
        if let Err(e) = self.host.post(worker, &text) {
            self.host.terminate(worker);
            return Err(e.into());
        }
        self.stats.posted += 1;
        Ok(worker)
    }

    /// Stop a context and its worker. Unknown contexts are ignored.
    pub fn stop(&mut self, ctx: &ScriptCtxId) {
        if let Some(i) = self.ctxs.iter().position(|c| c.id == *ctx) {
            let c = self.ctxs.remove(i);
            if let Some(w) = c.worker {
                // `stop` lets a worker that is still alive close itself; terminating makes sure it is gone, and
                // nothing it sends is read any more.
                if self.host.post(w, proto::STOP).is_ok() {
                    self.stats.posted += 1;
                }
                self.host.terminate(w);
            }
        }
    }

    /// Where a context is; `None` for one that was never started or was stopped.
    #[must_use]
    pub fn status(&self, ctx: &ScriptCtxId) -> Option<ScriptStatus> {
        self.ctxs
            .iter()
            .find(|c| c.id == *ctx)
            .map(|c| c.status.clone())
    }

    /// Every context and where it is, in the order they were started.
    pub fn contexts(&self) -> impl Iterator<Item = (ScriptCtxId, ScriptStatus)> + '_ {
        self.ctxs.iter().map(|c| (c.id.clone(), c.status.clone()))
    }

    /// The names a context's scripts gave to `ui.on`, as reported in `ready`.
    #[must_use]
    pub fn handlers(&self, ctx: &ScriptCtxId) -> &[String] {
        self.ctxs
            .iter()
            .find(|c| c.id == *ctx)
            .map_or(&[], |c| c.handlers.as_slice())
    }

    /// A context whose failure the host cannot carry on with: the skin's scripts failed while loading. The
    /// host falls back to another skin. Taken once.
    pub fn take_fatal(&mut self) -> Option<(ScriptCtxId, String)> {
        self.fatal.take()
    }

    /// Whether no context exists (none was started, or all were stopped): [`drain`](Self::drain) and
    /// [`post`](Self::post) have nothing to do.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ctxs.is_empty()
    }

    /// Counters for the stats overlay.
    #[must_use]
    pub fn stats(&self) -> ScriptStats {
        let mut s = self.stats.clone();
        s.contexts = self.ctxs.len() as u32;
        s.running = self.ctxs.iter().filter(|c| c.live()).count() as u32;
        s
    }

    /// Send each context its part of the frame: the delta filtered by its watch set, its markup events and a
    /// changed environment. A context with nothing to receive gets no message; a busy one keeps it for later,
    /// merged with what follows. A context whose worker was killed starts again here, from `input.store`.
    pub fn post(&mut self, now_ms: f64, input: &FrameInput<'_>) {
        let mut out = std::mem::take(&mut self.pending);
        self.watch(now_ms, &mut out);
        for i in 0..self.ctxs.len() {
            if self.ctxs[i].off() {
                continue;
            }
            if self.ctxs[i].worker.is_none() {
                // The snapshot holds this frame's delta; the events were meant for the worker that hung.
                if let Some(store) = input.store {
                    self.restart(i, store, &mut out);
                }
                continue;
            }
            let c = &mut self.ctxs[i];
            if let Some(env) = input.env {
                c.env = env.clone();
            }
            let delta = input.delta.and_then(|d| d.filter(&c.watch));
            let events: Vec<&ScriptEvent> = input.events.iter().filter(|e| e.ctx == c.id).collect();
            if matches!(c.status, ScriptStatus::Busy { .. }) {
                c.backlog.add(delta, &events, input.env);
                continue;
            }
            let (model, events, env) = if c.backlog.is_empty() {
                let model = delta.map(|d| d.to_umd1(c.parts[0].schema));
                (
                    model,
                    events.into_iter().cloned().collect(),
                    input.env.cloned(),
                )
            } else {
                c.backlog.add(delta, &events, input.env);
                let b = std::mem::take(&mut c.backlog);
                let model = (!b.ops.is_empty()).then(|| {
                    FrameDelta {
                        revision: b.revision,
                        ops: b.ops,
                    }
                    .to_umd1(c.parts[0].schema)
                });
                (model, b.events, b.env)
            };
            if model.is_none() && events.is_empty() && env.is_none() {
                continue;
            }
            c.seq += 1;
            let refs: Vec<&ScriptEvent> = events.iter().collect();
            let text = proto::encode_frame(c.seq, c.applied, model.as_deref(), &refs, env.as_ref());
            let Some(worker) = c.worker else {
                continue;
            };
            match self.host.post(worker, &text) {
                Ok(()) => {
                    self.stats.posted += 1;
                    c.wd.sent(c.seq, now_ms);
                }
                Err(e) => {
                    let reason = e.to_string();
                    out.log
                        .push(c.line(LogLevel::Error, format!("stopped: {reason}")));
                    out.failed.push(c.id.clone());
                    self.stats.errors += 1;
                    c.status = ScriptStatus::Disabled { reason };
                    self.host.terminate(worker);
                    c.worker = None;
                }
            }
        }
        self.pending = out;
    }

    /// Take what the workers sent since the last call.
    pub fn drain(&mut self, now_ms: f64) -> ScriptOutput {
        let mut out = std::mem::take(&mut self.pending);
        self.stats.worker_ms = 0.0;
        self.events.clear();
        self.host.drain(&mut self.events);
        let events = std::mem::take(&mut self.events);
        for (worker, ev) in &events {
            let Some(i) = self.ctxs.iter().position(|c| c.worker == Some(*worker)) else {
                continue;
            };
            self.stats.received += 1;
            let msg = match ev {
                ScriptHostEvent::Message(text) => proto::decode(text),
                // An out of wire 2.
                ScriptHostEvent::Bytes(bytes) => proto::decode_bin(bytes),
                ScriptHostEvent::Error(message) => {
                    self.fail(i, now_ms, format!("worker error: {message}"), &mut out);
                    continue;
                }
            };
            match msg {
                Ok(msg) => self.on_message(i, now_ms, msg, &mut out),
                Err(e) => {
                    self.stats.errors += 1;
                    let c = &self.ctxs[i];
                    if self.once.first(&c.id, "garbage") {
                        out.log.push(c.line(
                            LogLevel::Error,
                            format!("sent a message it should not: {e}"),
                        ));
                    }
                }
            }
        }
        self.events = events;
        self.watch(now_ms, &mut out);
        out
    }

    /// Ask the watchdog about every context: mark the busy ones, kill the hung ones.
    ///
    /// The waits count up to the time the page has taken in the workers' messages
    /// ([`ScriptHost::delivered_until`]), not up to now: while the page holds the messages back (input and long
    /// frames come first in a browser) an answer may be posted and not drained yet, and a worker is judged by
    /// what it could have answered.
    fn watch(&mut self, now_ms: f64, out: &mut ScriptOutput) {
        let seen = self
            .host
            .delivered_until()
            .map_or(now_ms, |t| t.min(now_ms));
        for i in 0..self.ctxs.len() {
            let c = &mut self.ctxs[i];
            if c.off() || c.worker.is_none() {
                continue;
            }
            match c.wd.check(seen) {
                Verdict::Fine => {
                    if matches!(c.status, ScriptStatus::Busy { .. }) {
                        c.status = ScriptStatus::Running;
                    }
                }
                Verdict::Busy(ms) => {
                    if c.live() {
                        c.status = ScriptStatus::Busy { ms };
                    }
                }
                Verdict::Kill { waited_ms, loading } => {
                    let text = if loading {
                        format!("killed: not ready after {waited_ms:.0} ms")
                    } else {
                        format!("killed after {KILL_AFTER_MS:.0} ms")
                    };
                    out.log.push(c.line(LogLevel::Error, text.clone()));
                    self.stats.errors += 1;
                    self.lost(i, now_ms, text, out);
                }
            }
        }
    }

    /// The worker of a context is gone (killed or failed): start it again on the next frame with a store, or
    /// switch the context off after too many failures.
    fn lost(&mut self, i: usize, now_ms: f64, reason: String, out: &mut ScriptOutput) {
        let c = &mut self.ctxs[i];
        if let Some(w) = c.worker.take() {
            self.host.terminate(w);
        }
        c.backlog = Backlog::default();
        c.handlers.clear();
        if c.wd.failure(now_ms) {
            out.log.push(c.line(
                LogLevel::Error,
                format!(
                    "switched off: failed {} times in a minute",
                    crate::MAX_FAILURES
                ),
            ));
            out.failed.push(c.id.clone());
            c.status = ScriptStatus::Disabled { reason };
        } else {
            c.status = ScriptStatus::Restarting;
        }
    }

    /// A new worker for a context whose worker was lost, from the model as it is now.
    fn restart(&mut self, i: usize, store: &ui2_model::ModelStore, out: &mut ScriptOutput) {
        let c = &self.ctxs[i];
        let init = ScriptInit::from_store(store, &c.bundle, c.settings.clone(), c.env.clone()).map(
            |mut init| {
                init.dev = c.dev;
                init
            },
        );
        let (id, bundle) = (c.id.clone(), c.bundle.clone());
        let started = init.and_then(|init| {
            let w = self.spawn(&id, &bundle, &init)?;
            Ok((w, init))
        });
        let c = &mut self.ctxs[i];
        match started {
            Ok((w, init)) => {
                c.worker = Some(w);
                c.watch = init.watch;
                c.parts = init.parts;
                c.seq = 0;
                c.acked = 0;
                c.applied = 0;
                c.wd.started();
                c.status = ScriptStatus::Restarting;
                self.stats.restarts += 1;
            }
            Err(e) => {
                let reason = format!("cannot start again: {e}");
                out.log.push(c.line(LogLevel::Error, reason.clone()));
                out.failed.push(c.id.clone());
                self.stats.errors += 1;
                c.status = ScriptStatus::Disabled { reason };
            }
        }
    }

    fn on_message(&mut self, i: usize, now_ms: f64, msg: FromWorker, out: &mut ScriptOutput) {
        match msg {
            FromWorker::Ready { handlers, ms } => {
                let c = &mut self.ctxs[i];
                if matches!(c.status, ScriptStatus::Starting | ScriptStatus::Restarting) {
                    c.status = ScriptStatus::Running;
                }
                c.wd.ready(now_ms);
                c.handlers = handlers;
                out.log
                    .push(c.line(LogLevel::Info, format!("ready in {ms:.1} ms")));
            }
            FromWorker::Fail { error } => {
                let at = match (&error.file, error.line) {
                    (Some(f), Some(l)) => format!(" at {f}:{l}"),
                    (Some(f), None) => format!(" at {f}"),
                    _ => String::new(),
                };
                self.fail(
                    i,
                    now_ms,
                    format!("failed to load: {}{at}", error.message),
                    out,
                );
            }
            FromWorker::Out(o) => self.on_out(i, o, out),
        }
    }

    /// A worker failed (its files threw while loading, or the worker itself broke). The skin failing at its
    /// first start is fatal; otherwise the context starts again, or is switched off after too many failures.
    fn fail(&mut self, i: usize, now_ms: f64, reason: String, out: &mut ScriptOutput) {
        let c = &mut self.ctxs[i];
        if c.off() {
            return;
        }
        self.stats.errors += 1;
        if self.once.first(&c.id, &format!("fail|{reason}")) {
            out.log.push(c.line(LogLevel::Error, reason.clone()));
        }
        if c.id == ScriptCtxId::Skin && c.status == ScriptStatus::Starting {
            self.fatal = Some((c.id.clone(), reason.clone()));
            out.failed.push(c.id.clone());
            c.status = ScriptStatus::Disabled { reason };
            if let Some(w) = c.worker.take() {
                self.host.terminate(w);
            }
            return;
        }
        self.lost(i, now_ms, reason, out);
    }

    fn on_out(&mut self, i: usize, o: OutMsg, out: &mut ScriptOutput) {
        let c = &mut self.ctxs[i];
        if c.off() {
            return;
        }
        self.stats.worker_ms += o.busy_ms;
        if let Some(ack) = o.ack {
            if ack <= c.acked || ack > c.seq {
                out.log.push(c.line(
                    LogLevel::Warn,
                    format!(
                        "answered frame {ack} out of order (sent {}, answered {})",
                        c.seq, c.acked
                    ),
                ));
            } else {
                c.acked = ack;
                c.wd.acked(ack);
                if !c.wd.waiting() && matches!(c.status, ScriptStatus::Busy { .. }) {
                    c.status = ScriptStatus::Running;
                }
            }
        }
        c.applied = c.applied.max(o.n);
        let id = c.id.clone();
        for s in o.state {
            if let Err(why) = writable(c.schema(), &s.p) {
                out.log.push(c.line(LogLevel::Warn, why));
                continue;
            }
            let ty = proto::state_type(&c.parts, &c.bundle.state_ns, &s.p);
            let value = proto::json_to_value(&s.v, ty.as_ref().map(|(s, t)| (*s, t)));
            out.state_ops.push((
                id.clone(),
                StateOp {
                    path: Arc::from(s.p.as_str()),
                    value,
                },
            ));
        }
        for m in o.intents {
            if let Some(prefix) = &c.bundle.intent_prefix
                && !m.name.starts_with(prefix.as_str())
            {
                out.log.push(c.line(
                    LogLevel::Warn,
                    format!(
                        "intent `{}` dropped: a mod raises only `{prefix}*` intents",
                        m.name
                    ),
                ));
                continue;
            }
            let schema = c.schema();
            let def = schema.intents().iter().find(|d| d.name == m.name);
            let args = m
                .args
                .iter()
                .map(|(k, v)| {
                    let ty = def.and_then(|d| d.args.iter().find(|a| a.name == *k));
                    let value = proto::json_to_value(v, ty.map(|a| (schema, &a.ty)));
                    (Arc::from(k.as_str()), value)
                })
                .collect();
            out.intents.push((
                id.clone(),
                WireIntent {
                    name: Arc::from(m.name.as_str()),
                    args,
                    origin: Origin::Script,
                },
            ));
        }
        for m in o.cmds {
            if m.island.is_empty() || m.key.is_empty() {
                out.log.push(c.line(
                    LogLevel::Warn,
                    format!(
                        "island command `{}` dropped: it needs an island and a key",
                        m.op
                    ),
                ));
                continue;
            }
            match m.to_cmd() {
                Ok(cmd) => out.island_cmds.push((id.clone(), cmd)),
                Err(e) => out.log.push(c.line(LogLevel::Warn, e)),
            }
        }
        // An out of wire 2 brings its commands read already.
        out.island_cmds.reserve(o.island_cmds.len());
        for cmd in o.island_cmds {
            if cmd.island.is_empty() || cmd.key.is_empty() {
                out.log.push(c.line(
                    LogLevel::Warn,
                    format!(
                        "island command `{}` dropped: it needs an island and a key",
                        proto::op_name(&cmd.op)
                    ),
                ));
                continue;
            }
            out.island_cmds.push((id.clone(), cmd));
        }
        for s in o.settings {
            let value = (!s.value.is_null()).then(|| s.value.to_string());
            c.remember_setting(&s.key, value.as_ref());
            out.settings.push((id.clone(), s.key, value));
        }
        for l in &o.log {
            let line = l.to_line();
            if line.level == LogLevel::Error {
                self.stats.errors += 1;
            }
            if let Some(key) = &l.key
                && !self.once.first(&id, key)
            {
                continue;
            }
            out.log.push((id.clone(), line));
        }
    }
}

impl Drop for ScriptSupervisor {
    /// A supervisor that goes away (the skin is replaced, the UI falls back) takes its workers with it.
    fn drop(&mut self) {
        for c in &mut self.ctxs {
            if let Some(w) = c.worker.take() {
                let _ = self.host.post(w, proto::STOP);
                self.host.terminate(w);
            }
        }
    }
}
