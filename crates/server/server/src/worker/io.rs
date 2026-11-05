//! The two I/O threads of the worker shell (game-host-mp 4.16, GH-D3): stdin -> `ToWorker`, `WorkerFrame`
//! -> stdout. stdout carries frames only; logs go to stderr.

use std::io::{self, BufWriter, Read, Write};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use game_host::SaveDone;
use protocol::{Codec, Postcard, ToWorker, TurnPhase, WorkerFrame, WorkerMsg};
use wire::ipc::{FrameReader, write_frame};

/// What the stdin thread hands to the core thread.
#[derive(Debug)]
pub enum Input {
    /// A frame, decoded.
    Msg(Box<ToWorker>),
    /// stdin closed at a frame boundary: the same as `Shutdown { save: true }` (GH-D16).
    Eof,
    /// A frame that is cut, too long or not a `ToWorker`: exit code 2.
    Bad(String),
    /// The save thread wrote a file (or failed to): the answer to an `Out::Save`.
    Saved(Box<SaveDone>),
    /// The pack thread packed a snapshot of the replica stream: the answer to an `Out::Replica`.
    Replica(Box<game_host::ReplicaDone>),
}

/// A helper thread that failed to start is logged loudly (a missing watchdog or start pacer would otherwise
/// just be absent and hang detection silently off); the worker goes on, the gateway's silence rule stays.
pub fn logged_spawn(name: &str, spawned: io::Result<JoinHandle<()>>) -> Option<JoinHandle<()>> {
    match spawned {
        Ok(t) => Some(t),
        Err(e) => {
            tracing::error!(thread = name, error = %e, "a helper thread cannot be started");
            None
        }
    }
}

/// Reads frames from `r` on a thread of its own and sends them into `tx` (the channel the save thread
/// answers into as well). The thread ends at EOF or at the first bad frame; the channel then goes on holding
/// what was read.
pub fn spawn_reader<R: Read + Send + 'static>(r: R, tx: Sender<Input>) -> io::Result<()> {
    std::thread::Builder::new()
        .name("ipc-stdin".into())
        .spawn(move || {
            let mut frames = FrameReader::new(r);
            loop {
                let input = match frames.next() {
                    Ok(Some(body)) => match Postcard.decode::<ToWorker>(&body) {
                        Ok(msg) => Input::Msg(Box::new(msg)),
                        Err(e) => {
                            Input::Bad(format!("a frame from the gateway does not parse: {e}"))
                        }
                    },
                    Ok(None) => Input::Eof,
                    Err(e) => Input::Bad(format!("stdin: {e}")),
                };
                let last = !matches!(input, Input::Msg(_));
                // The core thread is gone: nothing left to read for.
                if tx.send(input).is_err() || last {
                    return;
                }
            }
        })?;
    Ok(())
}

/// The stdout thread: encoded frames in, a `BufWriter`, a flush after every frame.
pub struct Writer {
    tx: Option<Sender<Vec<u8>>>,
    thread: Option<JoinHandle<()>>,
}

impl Writer {
    pub fn spawn<W: Write + Send + 'static>(w: W) -> io::Result<Self> {
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let thread = std::thread::Builder::new()
            .name("ipc-stdout".into())
            .spawn(move || {
                let mut w = BufWriter::new(w);
                while let Ok(body) = rx.recv() {
                    // `write_frame` flushes. A write error means the gateway is gone: stop writing, the
                    // core thread notices at the next send.
                    if let Err(e) = write_frame(&mut w, &body) {
                        tracing::warn!("stdout: {e}");
                        return;
                    }
                }
            })?;
        Ok(Writer {
            tx: Some(tx),
            thread: Some(thread),
        })
    }

    /// Queues a frame. `false`: the writer is gone (the gateway closed the pipe).
    pub fn send(&self, frame: &WorkerFrame) -> bool {
        let body = match Postcard.encode(frame) {
            Ok(b) => b,
            Err(e) => {
                tracing::error!("cannot encode a frame: {e}");
                return true;
            }
        };
        self.tx.as_ref().is_some_and(|tx| tx.send(body).is_ok())
    }

    /// `Fatal` before an exit with a code other than 0 (best effort).
    pub fn fatal(&self, message: impl Into<String>, code: u8) {
        let _ = self.send(&WorkerFrame {
            msgs: vec![WorkerMsg::Fatal {
                message: message.into(),
                code,
            }],
        });
    }

    /// Starts a thread that queues `Heartbeat { Starting }` every `every` while the core thread builds the game
    /// (`core.start` is one long call: a Huge map script that searches for fair starts takes longer than the
    /// gateway's silence limit, GH-D16). It stops on [`StartPacer::stop`] or after `budget`: a start
    /// that takes longer than that is a hang, and the gateway's silence rule ends it.
    pub fn start_pacer(&self, every: Duration, budget: Duration) -> StartPacer {
        self.start_pacer_with(every, budget, Instant::now)
    }

    /// [`Writer::start_pacer`] with the clock of the budget given (tests drive it by hand).
    fn start_pacer_with(
        &self,
        every: Duration,
        budget: Duration,
        now: impl Fn() -> Instant + Send + 'static,
    ) -> StartPacer {
        let (stop, rx) = mpsc::channel::<()>();
        let body = Postcard
            .encode(&WorkerFrame {
                msgs: vec![WorkerMsg::Heartbeat {
                    phase: TurnPhase::Starting,
                    queued: 0,
                }],
            })
            .ok();
        let tx = self.tx.clone();
        let thread = logged_spawn(
            "start-pacer",
            std::thread::Builder::new()
                .name("start-pacer".into())
                .spawn(move || {
                    let (Some(body), Some(tx)) = (body, tx) else {
                        return;
                    };
                    let until = now() + budget;
                    loop {
                        match rx.recv_timeout(every) {
                            Err(RecvTimeoutError::Timeout) => {}
                            // Stopped, or the core thread is gone.
                            _ => return,
                        }
                        if now() >= until {
                            tracing::warn!(
                                budget_s = budget.as_secs(),
                                "the game is still not built: no more start heartbeats"
                            );
                            return;
                        }
                        if tx.send(body.clone()).is_err() {
                            return;
                        }
                    }
                }),
        );
        StartPacer {
            stop: Some(stop),
            thread,
        }
    }

    /// The queue of encoded frames, for a thread that sends frames of its own (`watch.rs`).
    pub fn queue(&self) -> Option<Sender<Vec<u8>>> {
        self.tx.clone()
    }

    /// Waits until everything queued is written.
    pub fn finish(mut self) {
        self.tx = None;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The heartbeat thread of the start ([`Writer::start_pacer`]).
pub struct StartPacer {
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl StartPacer {
    /// Stops the heartbeats and waits for the thread: whatever it queued is before the next frame of the core
    /// thread (`Catalog`, `Ready`).
    pub fn stop(mut self) {
        self.halt();
    }

    fn halt(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for StartPacer {
    fn drop(&mut self) {
        self.halt();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_thread_that_did_not_start_is_reported_as_absent() {
        let failed = Err(io::Error::other("no threads"));
        assert!(super::logged_spawn("x", failed).is_none());
        let ok = std::thread::Builder::new().spawn(|| {});
        let t = super::logged_spawn("x", ok).expect("started");
        t.join().unwrap();
    }

    use std::sync::{Arc, Mutex};

    use super::*;

    /// A `Write` whose bytes the test reads afterwards.
    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);

    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// The frames written, decoded.
    fn frames(sink: &Sink) -> Vec<WorkerFrame> {
        let bytes = sink.0.lock().unwrap().clone();
        let mut r = FrameReader::new(&bytes[..]);
        let mut out = Vec::new();
        while let Some(body) = r.next().unwrap() {
            out.push(Postcard.decode(&body).unwrap());
        }
        out
    }

    fn is_start_heartbeat(f: &WorkerFrame) -> bool {
        matches!(
            f.msgs.as_slice(),
            [WorkerMsg::Heartbeat {
                phase: TurnPhase::Starting,
                ..
            }]
        )
    }

    /// Waits (up to 30 s, so a loaded machine only slows it) until the sink holds `n` frames.
    fn wait_for_frames(sink: &Sink, n: usize) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while frames(sink).len() < n {
            assert!(
                Instant::now() < deadline,
                "the pacer never reached {n} frames"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn the_start_pacer_beats_until_stopped_and_nothing_after() {
        let sink = Sink::default();
        let out = Writer::spawn(sink.clone()).unwrap();
        let pacer = out.start_pacer(Duration::from_millis(1), Duration::from_secs(60));
        // No sleep of a fixed length: wait until it has beaten three times.
        wait_for_frames(&sink, 3);
        pacer.stop();
        out.fatal("after the pacer", 1);
        out.finish();
        let f = frames(&sink);
        let (last, beats) = f.split_last().unwrap();
        assert!(beats.len() >= 3, "{} heartbeats", beats.len());
        assert!(beats.iter().all(is_start_heartbeat));
        assert!(matches!(last.msgs.as_slice(), [WorkerMsg::Fatal { .. }]));
    }

    #[test]
    fn the_start_pacer_falls_silent_after_its_budget() {
        let sink = Sink::default();
        let out = Writer::spawn(sink.clone()).unwrap();
        // A clock that moves 10 ms per reading, whatever the machine does: the first reading sets the end
        // of the budget (35 ms), the ticks read 10, 20, 30 (a beat each) and 40 (silent, the thread ends).
        let base = Instant::now();
        let reads = std::sync::atomic::AtomicU64::new(0);
        let clock = move || {
            let n = reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            base + Duration::from_millis(10 * n)
        };
        let pacer =
            out.start_pacer_with(Duration::from_millis(1), Duration::from_millis(35), clock);
        wait_for_frames(&sink, 3);
        // `stop` joins the thread, which ends by itself at the budget; a fourth beat would be a bug.
        pacer.stop();
        out.finish();
        let n = frames(&sink).len();
        assert_eq!(n, 3, "{n} heartbeats within a budget of 35 ms");
    }
}
