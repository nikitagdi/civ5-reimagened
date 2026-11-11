//! The supervisor: one worker per script context, the frame batch, and the answers turned into output for
//! the host.

use std::sync::Arc;

use platform::{ScriptHost, ScriptHostEvent, WorkerId};
use ui2_model::wire::{Origin, ScriptCtxId, StateOp, WireIntent};
use ui2_model::{Schema, WatchSet};

use crate::proto::{self, FromWorker, OutMsg};
use crate::{
    FrameInput, LogLevel, LogLine, MAX_CONTEXTS, SchemaPart, ScriptBundle, ScriptError, ScriptInit,
    ScriptOutput, ScriptStats, ScriptStatus, ctx_name,
};

struct Ctx {
    id: ScriptCtxId,
    worker: WorkerId,
    bundle: ScriptBundle,
    watch: WatchSet,
    parts: Vec<SchemaPart>,
    status: ScriptStatus,
    /// The `seq` of the last frame sent, and of the last one answered.
    seq: u64,
    acked: u64,
    /// The highest `out.n` drained: every write up to it reaches the model before the next frame is taken.
    applied: u64,
    /// When the oldest unanswered frame was sent.
    sent_ms: Option<f64>,
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
}

/// Runs the script contexts of the active skin and mods. Owned by the host; called twice per frame:
/// [`drain`](Self::drain) at the start, [`post`](Self::post) after the model delta was taken.
pub struct ScriptSupervisor {
    host: Box<dyn ScriptHost>,
    ctxs: Vec<Ctx>,
    /// Output produced outside `drain` (a failed post), handed over with the next drain.
    pending: ScriptOutput,
    fatal: Option<(ScriptCtxId, String)>,
    stats: ScriptStats,
    events: Vec<(WorkerId, ScriptHostEvent)>,
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
        }
    }

    /// Start a context: a new worker gets the files and the snapshot. A context of the same id is stopped
    /// first.
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
        if self.ctxs.len() >= MAX_CONTEXTS {
            return Err(ScriptError::TooMany);
        }
        let worker = self.host.spawn()?;
        let text = proto::encode_init(&ctx_name(&ctx), &bundle, &init);
        if let Err(e) = self.host.post(worker, &text) {
            self.host.terminate(worker);
            return Err(e.into());
        }
        self.stats.posted += 1;
        self.ctxs.push(Ctx {
            id: ctx,
            worker,
            bundle,
            watch: init.watch,
            parts: init.parts,
            status: ScriptStatus::Starting,
            seq: 0,
            acked: 0,
            applied: 0,
            sent_ms: None,
            handlers: Vec::new(),
        });
        Ok(())
    }

    /// Stop a context and its worker. Unknown contexts are ignored.
    pub fn stop(&mut self, ctx: &ScriptCtxId) {
        if let Some(i) = self.ctxs.iter().position(|c| c.id == *ctx) {
            let c = self.ctxs.remove(i);
            // `stop` lets a worker that is still alive close itself; terminating makes sure it is gone, and
            // nothing it sends is read any more.
            if self.host.post(c.worker, proto::STOP).is_ok() {
                self.stats.posted += 1;
            }
            self.host.terminate(c.worker);
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

    /// Counters for the stats overlay.
    #[must_use]
    pub fn stats(&self) -> ScriptStats {
        let mut s = self.stats.clone();
        s.contexts = self.ctxs.len() as u32;
        s.running = self
            .ctxs
            .iter()
            .filter(|c| matches!(c.status, ScriptStatus::Running | ScriptStatus::Busy { .. }))
            .count() as u32;
        s
    }

    /// Send each context its part of the frame: the delta filtered by its watch set, its markup events and a
    /// changed environment. A context with nothing to receive gets no message.
    pub fn post(&mut self, now_ms: f64, input: &FrameInput<'_>) {
        for c in &mut self.ctxs {
            if matches!(c.status, ScriptStatus::Disabled { .. }) {
                continue;
            }
            let model = input
                .delta
                .and_then(|d| d.filter(&c.watch))
                .map(|d| d.to_umd1(c.parts[0].schema));
            let events: Vec<_> = input.events.iter().filter(|e| e.ctx == c.id).collect();
            if model.is_none() && events.is_empty() && input.env.is_none() {
                continue;
            }
            c.seq += 1;
            let text = proto::encode_frame(c.seq, c.applied, model.as_deref(), &events, input.env);
            match self.host.post(c.worker, &text) {
                Ok(()) => {
                    self.stats.posted += 1;
                    c.sent_ms.get_or_insert(now_ms);
                }
                Err(e) => {
                    let reason = e.to_string();
                    self.pending
                        .log
                        .push(c.line(LogLevel::Error, format!("stopped: {reason}")));
                    self.pending.failed.push(c.id.clone());
                    self.stats.errors += 1;
                    c.status = ScriptStatus::Disabled { reason };
                }
            }
        }
    }

    /// Take what the workers sent since the last call.
    pub fn drain(&mut self, now_ms: f64) -> ScriptOutput {
        let _ = now_ms;
        let mut out = std::mem::take(&mut self.pending);
        self.stats.worker_ms = 0.0;
        self.events.clear();
        self.host.drain(&mut self.events);
        let events = std::mem::take(&mut self.events);
        for (worker, ev) in &events {
            let Some(i) = self.ctxs.iter().position(|c| c.worker == *worker) else {
                continue;
            };
            self.stats.received += 1;
            match ev {
                ScriptHostEvent::Message(text) => match proto::decode(text) {
                    Ok(msg) => self.on_message(i, msg, &mut out),
                    Err(e) => {
                        let c = &self.ctxs[i];
                        out.log.push(c.line(
                            LogLevel::Error,
                            format!("sent a message it should not: {e}"),
                        ));
                        self.stats.errors += 1;
                    }
                },
                ScriptHostEvent::Error(message) => {
                    self.fail(i, format!("worker error: {message}"), &mut out);
                }
            }
        }
        self.events = events;
        out
    }

    fn on_message(&mut self, i: usize, msg: FromWorker, out: &mut ScriptOutput) {
        match msg {
            FromWorker::Ready { handlers, ms } => {
                let c = &mut self.ctxs[i];
                if c.status == ScriptStatus::Starting {
                    c.status = ScriptStatus::Running;
                }
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
                self.fail(i, format!("failed to load: {}{at}", error.message), out);
            }
            FromWorker::Out(o) => self.on_out(i, o, out),
        }
    }

    /// Switch a context off: its panels get a stub; the skin's failing while it starts is fatal.
    fn fail(&mut self, i: usize, reason: String, out: &mut ScriptOutput) {
        let c = &mut self.ctxs[i];
        if matches!(c.status, ScriptStatus::Disabled { .. }) {
            return;
        }
        if c.id == ScriptCtxId::Skin && c.status == ScriptStatus::Starting {
            self.fatal = Some((c.id.clone(), reason.clone()));
        }
        out.log.push(c.line(LogLevel::Error, reason.clone()));
        out.failed.push(c.id.clone());
        self.stats.errors += 1;
        c.status = ScriptStatus::Disabled { reason };
        self.host.terminate(c.worker);
    }

    fn on_out(&mut self, i: usize, o: OutMsg, out: &mut ScriptOutput) {
        let c = &mut self.ctxs[i];
        if matches!(c.status, ScriptStatus::Disabled { .. }) {
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
                if c.acked == c.seq {
                    c.sent_ms = None;
                }
            }
        }
        c.applied = c.applied.max(o.n);
        let id = c.id.clone();
        for s in o.state {
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
            match m.to_cmd() {
                Ok(cmd) => out.island_cmds.push((id.clone(), cmd)),
                Err(e) => out.log.push(c.line(LogLevel::Warn, e)),
            }
        }
        for s in o.settings {
            let value = (!s.value.is_null()).then(|| s.value.to_string());
            out.settings.push((id.clone(), s.key, value));
        }
        for l in &o.log {
            let line = l.to_line();
            if line.level == LogLevel::Error {
                self.stats.errors += 1;
            }
            out.log.push((id.clone(), line));
        }
    }
}
