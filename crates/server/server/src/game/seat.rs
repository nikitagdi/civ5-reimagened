//! The seat pipeline: one task per human seat. It owns the
//! [`SeatStream`], takes what the game actor sends, and is the only writer into the queue of the
//! attached session. Packing of big batches runs on the blocking pool, so a slow seat or a big
//! keyframe never holds the game actor or the other seats.

use std::sync::Arc;

use protocol::{
    BasisClaim, CatalogRef, CmdId, DetachReason, KeyframeReason, PlayerId, Resume, ServerMsg,
};
use tokio::sync::mpsc;
use wire::Encoder;

use super::actor::ActorMsg;
use super::gamelog::GameLog;
use super::recording::Recorder;
use super::stats::SeatStats;
use super::stream::{AttachArgs, BLOCKING_FROM, Parts, SeatStream};
use crate::clock::Clock;
use crate::lobby::{LobbyMsg, LobbyTx};
use crate::session::outbox::Outbox;

/// A connection that wants the seat.
#[derive(Debug)]
pub struct AttachReq {
    pub session: u64,
    pub out: Outbox,
    pub resume: Option<Resume>,
    pub claim: Option<BasisClaim>,
    pub catalog: CatalogRef,
    pub last_cmd_id: CmdId,
}

/// A message for the lobby that the pipeline sends right after it queued `Detached`, so that the
/// lobby view of the player follows the `Detached` on the same socket (NP 4.3).
#[derive(Default)]
pub struct Then(Option<(LobbyTx, LobbyMsg)>);

impl Then {
    pub fn lobby(tx: LobbyTx, msg: LobbyMsg) -> Self {
        Self(Some((tx, msg)))
    }
}

impl std::fmt::Debug for Then {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() {
            "Then(lobby)"
        } else {
            "Then(-)"
        })
    }
}

/// Test hooks on one seat (`--test-hooks`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeatFault {
    /// The next diff of the seat leaves with a wrong view hash.
    CorruptHash,
    /// The next batch is built (its `seq` is used up) but not sent.
    DropBatch,
    /// The socket of the seat is closed.
    CloseSocket,
    /// The pipeline of the seat stands still for this many milliseconds.
    Hold(u64),
}

/// What the actor tells a seat pipeline.
#[derive(Debug)]
pub enum SeatInput {
    /// A test hook.
    Fault(SeatFault),
    /// One step of the game for this seat: at most one batch.
    Step(Box<Parts>),
    /// The worker died: what the stream knows of it (contexts, the view) waits for the next `Full`.
    WorkerLost,
    Attach(AttachReq),
    /// The session ended or was replaced.
    Detach {
        session: u64,
    },
    /// Send `Detached { reason }` to the session, then forget it.
    Release {
        session: u64,
        reason: DetachReason,
        then: Then,
    },
    NeedKeyframe {
        session: u64,
        reason: KeyframeReason,
    },
    /// A control message (`QueryResult`) for the session.
    Control {
        session: u64,
        msg: ServerMsg,
    },
}

/// The actor's end of a pipeline.
#[derive(Clone)]
pub struct SeatHandle {
    tx: mpsc::UnboundedSender<SeatInput>,
    pub stats: Arc<SeatStats>,
}

impl SeatHandle {
    pub fn send(&self, input: SeatInput) {
        // The task ends only when the actor drops its handles.
        let _ = self.tx.send(input);
    }
}

struct Attachment {
    session: u64,
    out: Outbox,
}

struct SeatTask {
    stream: Option<SeatStream>,
    att: Option<Attachment>,
    pending: Option<AttachReq>,
    player: PlayerId,
    notify: mpsc::UnboundedSender<ActorMsg>,
    clock: Arc<dyn Clock>,
    encoder: Encoder,
    stats: Arc<SeatStats>,
    log: GameLog,
    rec: Recorder,
    drop_next: bool,
}

/// Starts the pipeline of a seat. `notify` is the actor's queue: the pipeline asks it for a
/// `WantFull` and tells it when a session's queue overflowed.
pub fn spawn(
    game: protocol::GameId,
    player: PlayerId,
    stats: Arc<SeatStats>,
    notify: mpsc::UnboundedSender<ActorMsg>,
    clock: Arc<dyn Clock>,
    log: GameLog,
    rec: Recorder,
) -> SeatHandle {
    let (tx, rx) = mpsc::unbounded_channel();
    let task = SeatTask {
        stream: Some(SeatStream::new(game, player, Arc::clone(&stats))),
        att: None,
        pending: None,
        player,
        notify,
        clock,
        encoder: Encoder::default(),
        stats: Arc::clone(&stats),
        log,
        rec,
        drop_next: false,
    };
    tokio::spawn(task.run(rx));
    SeatHandle { tx, stats }
}

/// The number a part name of a batch gets in `BatchMeta.tags` (the `GameMsg` tag it travels as; `full` and `delta` are
/// the view message, tags 0 and 1).
pub fn kind_tag(kind: &str) -> u8 {
    match kind {
        "full" => 0,
        "delta" => 1,
        "results" => 2,
        "journal" => 3,
        "turn" => 4,
        "seats" => 5,
        "context" => 6,
        "chat" => 7,
        "basis" => 8,
        "gc" => 9,
        _ => 255,
    }
}

impl SeatTask {
    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<SeatInput>) {
        while let Some(input) = rx.recv().await {
            let alive = match input {
                SeatInput::Step(parts) => self.step(*parts).await,
                SeatInput::Fault(f) => self.fault(f).await,
                SeatInput::WorkerLost => self
                    .with_stream(false, SeatStream::worker_lost)
                    .await
                    .is_some(),
                SeatInput::Attach(req) => {
                    // A newer connection replaces the one the seat has.
                    self.att = None;
                    self.pending = Some(req);
                    self.try_attach().await
                }
                SeatInput::Detach { session } => {
                    if self.att.as_ref().is_some_and(|a| a.session == session) {
                        self.att = None;
                    }
                    if self.pending.as_ref().is_some_and(|p| p.session == session) {
                        self.pending = None;
                    }
                    true
                }
                SeatInput::Release {
                    session,
                    reason,
                    then,
                } => {
                    if self.att.as_ref().is_some_and(|a| a.session == session) {
                        self.send_msg(&ServerMsg::Detached { reason });
                        self.att = None;
                    }
                    if let Some((tx, msg)) = then.0 {
                        let _ = tx.send(msg);
                    }
                    if self.pending.as_ref().is_some_and(|p| p.session == session) {
                        self.pending = None;
                    }
                    true
                }
                SeatInput::NeedKeyframe { session, reason } => {
                    if self.att.as_ref().is_some_and(|a| a.session == session) {
                        match self
                            .with_stream(true, move |s| s.need_keyframe(reason))
                            .await
                        {
                            Some(frame) => {
                                if let Some(f) = frame {
                                    self.push(f);
                                }
                                true
                            }
                            None => false,
                        }
                    } else {
                        true
                    }
                }
                SeatInput::Control { session, msg } => {
                    if self.att.as_ref().is_some_and(|a| a.session == session) {
                        self.send_msg(&msg);
                    }
                    true
                }
            };
            if !alive {
                return;
            }
        }
    }

    /// Runs `f` on the stream, on the blocking pool when `heavy`. `None`: the pool is gone (the
    /// runtime is shutting down).
    async fn with_stream<R: Send + 'static>(
        &mut self,
        heavy: bool,
        f: impl FnOnce(&mut SeatStream) -> R + Send + 'static,
    ) -> Option<R> {
        let mut stream = self.stream.take()?;
        if !heavy {
            let r = f(&mut stream);
            self.stream = Some(stream);
            return Some(r);
        }
        match tokio::task::spawn_blocking(move || {
            let r = f(&mut stream);
            (stream, r)
        })
        .await
        {
            Ok((stream, r)) => {
                self.stream = Some(stream);
                Some(r)
            }
            Err(e) => {
                if e.is_panic() {
                    std::panic::resume_unwind(e.into_panic());
                }
                None
            }
        }
    }

    /// A test hook.
    async fn fault(&mut self, f: SeatFault) -> bool {
        self.log.event(
            "fault",
            serde_json::json!({"player": self.player, "fault": format!("{f:?}")}),
        );
        match f {
            SeatFault::CorruptHash => self
                .with_stream(false, SeatStream::corrupt_next_diff)
                .await
                .is_some(),
            SeatFault::DropBatch => {
                self.drop_next = true;
                true
            }
            SeatFault::CloseSocket => {
                if let Some(att) = &self.att {
                    att.out.close();
                }
                true
            }
            SeatFault::Hold(ms) => {
                tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
                true
            }
        }
    }

    async fn step(&mut self, parts: Parts) -> bool {
        let heavy = parts.weight() >= BLOCKING_FROM;
        let now = self.clock.now_ms();
        let kinds = parts.kinds();
        let raw_before = self
            .stats
            .tx_raw_bytes
            .load(std::sync::atomic::Ordering::Relaxed);
        let started = std::time::Instant::now();
        let Some(out) = self.with_stream(heavy, move |s| s.step(parts, now)).await else {
            return false;
        };
        if let Some(basis) = out.want_full {
            self.log.event(
                "view_fail",
                serde_json::json!({"player": self.player, "basis": basis}),
            );
            let _ = self.notify.send(ActorMsg::WantFull {
                player: self.player,
                basis,
            });
        }
        if let Some(frame) = out.frame {
            self.stats.batch_build.record(started.elapsed());
            let raw = self
                .stats
                .tx_raw_bytes
                .load(std::sync::atomic::Ordering::Relaxed)
                .saturating_sub(raw_before);
            self.log.event(
                "batch",
                serde_json::json!({
                    "player": self.player,
                    "seq": self.stats.snapshot().seq,
                    "parts": kinds,
                    "raw": raw,
                    "body": frame.len(),
                }),
            );
            self.rec.batch_meta(
                self.player,
                self.stats.snapshot().seq,
                u32::try_from(frame.len()).unwrap_or(u32::MAX),
                kinds.iter().map(|k| kind_tag(k)).collect(),
            );
            if std::mem::take(&mut self.drop_next) {
                // Test hook `drop_batch`: the number is used up and the ring has the batch.
            } else {
                self.push(frame);
            }
        }
        // A view may have come that an attach waits for.
        self.try_attach().await
    }

    /// Answers the waiting attach when the stream has a view.
    async fn try_attach(&mut self) -> bool {
        if self.pending.is_none() || !self.stream.as_ref().is_some_and(SeatStream::has_view) {
            return true;
        }
        let Some(req) = self.pending.take() else {
            return true;
        };
        let args = AttachArgs {
            resume: req.resume,
            claim: req.claim,
            catalog: req.catalog,
            last_cmd_id: req.last_cmd_id,
        };
        let Some(out) = self.with_stream(true, move |s| s.attach(args)).await else {
            return false;
        };
        let Some(out) = out else {
            // No view after all: ask again with the next step.
            self.pending = Some(req);
            return true;
        };
        self.att = Some(Attachment {
            session: req.session,
            out: req.out,
        });
        self.send_msg(&out.attached);
        for frame in out.frames {
            self.push(frame);
        }
        true
    }

    fn send_msg(&mut self, msg: &ServerMsg) {
        match self.encoder.frame_msg(msg) {
            Ok(frame) => self.push(frame),
            Err(e) => tracing::error!(seat = self.player, error = %e, "cannot encode a message"),
        }
    }

    /// Queues a frame for the session. A queue that is over its limit or closed ends the
    /// session's hold on the seat: the seat goes to `Lost`, the ring goes on.
    fn push(&mut self, frame: Vec<u8>) {
        let Some(att) = &self.att else { return };
        if let Err(e) = att.out.push(frame) {
            tracing::warn!(
                seat = self.player,
                session = att.session,
                ?e,
                "the session queue failed"
            );
            let session = att.session;
            self.att = None;
            let _ = self.notify.send(ActorMsg::SessionLost {
                player: self.player,
                session,
            });
        }
    }
}
