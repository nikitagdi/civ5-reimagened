//! IPC with one worker (net-protocol 4.14, NP-D15): `u32 LE` length + postcard on the worker's
//! stdin (`ToWorker`) and stdout (`WorkerFrame`). Two tasks per worker: the reader turns stdout
//! into [`WorkerEvent`]s, the writer drains a bounded queue into stdin. The frame layout is the
//! one of `wire::ipc` (blocking, for the worker); these are its async twins for the gateway.

use std::io;

use protocol::{Codec, Postcard, ToWorker, WorkerFrame};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use wire::ipc::MAX_FRAME;

/// Frames from the gateway waiting for the pipe. More than this means the worker does not read.
pub const TO_WORKER_QUEUE: usize = 1024;

/// Decoded frames waiting for the gateway task. A full queue stops the reader, and with it the
/// worker, once its pipe is full: the gateway is the slow one then.
const EVENT_QUEUE: usize = 256;

/// What the worker (or its process) did.
#[derive(Debug)]
pub enum WorkerEvent {
    /// A frame from stdout.
    Frame(WorkerFrame),
    /// stdout carried something that is not a frame (bad length, bad postcard, read error).
    /// The stream cannot be trusted any more; no further frames follow.
    Garbage(String),
    /// stdout closed at a frame boundary.
    Eof,
    /// The process ended. Always the last event, after every frame it wrote.
    Exited(ExitInfo),
}

/// How a worker process ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitInfo {
    /// Exit code; `None` if a signal ended it (Unix).
    pub code: Option<i32>,
    /// The gateway killed it (hang, shutdown) before it ended by itself.
    pub killed: bool,
}

/// Why a frame could not be queued for the worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendError {
    /// [`TO_WORKER_QUEUE`] frames are waiting: the worker does not read its stdin.
    Full,
    /// The pipe is closed.
    Closed,
}

/// Sends frames to the worker. Never waits; cheap to clone.
#[derive(Clone)]
pub struct IpcTx {
    tx: mpsc::Sender<ToWorker>,
}

impl IpcTx {
    /// Queues one frame; the writer task sends them in order.
    pub fn send(&self, msg: ToWorker) -> Result<(), SendError> {
        self.tx.try_send(msg).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => SendError::Full,
            mpsc::error::TrySendError::Closed(_) => SendError::Closed,
        })
    }
}

/// The two tasks of a worker and their ends.
pub struct Io {
    pub tx: IpcTx,
    pub events: mpsc::Receiver<WorkerEvent>,
    /// Sender of the same event queue, for the process watcher (`Exited`).
    pub events_tx: mpsc::Sender<WorkerEvent>,
    /// Ends when stdout is read to its end or broke.
    pub reader: JoinHandle<()>,
    pub writer: JoinHandle<()>,
}

/// Starts the reader and the writer over the worker's pipes.
pub fn start<R, W>(stdout: R, stdin: W) -> Io
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (ev_tx, events) = mpsc::channel(EVENT_QUEUE);
    let (tx, rx) = mpsc::channel(TO_WORKER_QUEUE);
    let reader = tokio::spawn(read_loop(stdout, ev_tx.clone()));
    let writer = tokio::spawn(write_loop(stdin, rx));
    Io {
        tx: IpcTx { tx },
        events,
        events_tx: ev_tx,
        reader,
        writer,
    }
}

async fn read_loop<R: AsyncRead + Unpin>(mut r: R, ev: mpsc::Sender<WorkerEvent>) {
    loop {
        let event = match read_frame(&mut r).await {
            Ok(Some(body)) => match Postcard.decode::<WorkerFrame>(&body) {
                Ok(frame) => WorkerEvent::Frame(frame),
                Err(e) => WorkerEvent::Garbage(format!("bad frame body: {e}")),
            },
            Ok(None) => WorkerEvent::Eof,
            Err(e) => WorkerEvent::Garbage(format!("bad frame: {e}")),
        };
        let last = !matches!(event, WorkerEvent::Frame(_));
        if ev.send(event).await.is_err() || last {
            return;
        }
    }
}

async fn write_loop<W: AsyncWrite + Unpin>(mut w: W, mut rx: mpsc::Receiver<ToWorker>) {
    while let Some(msg) = rx.recv().await {
        let body = match Postcard.encode(&msg) {
            Ok(body) => body,
            Err(e) => {
                tracing::error!(error = %e, "cannot encode a frame for the worker");
                continue;
            }
        };
        if let Err(e) = write_frame(&mut w, &body).await {
            // The worker is gone or stuck shut; the reader side reports it. Closing `rx` makes
            // the next `send` fail.
            tracing::debug!(error = %e, "worker stdin closed");
            return;
        }
    }
    // The sender is gone: close stdin, which tells the worker to shut down (EOF).
    let _ = w.shutdown().await;
}

/// Reads one frame. `Ok(None)` is EOF at a frame boundary, EOF inside a frame is
/// `UnexpectedEof`, a length above `MAX_FRAME` is `InvalidData` (checked before allocating).
pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut head = [0u8; 4];
    let mut have = 0;
    while have < head.len() {
        match r.read(&mut head[have..]).await? {
            0 if have == 0 => return Ok(None),
            0 => return Err(io::ErrorKind::UnexpectedEof.into()),
            n => have += n,
        }
    }
    let len = u32::from_le_bytes(head) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("ipc frame length {len} exceeds {MAX_FRAME}"),
        ));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await?;
    Ok(Some(body))
}

/// Writes one frame and flushes.
pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, body: &[u8]) -> io::Result<()> {
    let len = u32::try_from(body.len())
        .ok()
        .filter(|n| *n as usize <= MAX_FRAME)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("ipc frame of {} bytes exceeds {MAX_FRAME}", body.len()),
            )
        })?;
    w.write_all(&len.to_le_bytes()).await?;
    w.write_all(body).await?;
    w.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::WorkerMsg;

    fn frame_bytes(frame: &WorkerFrame) -> Vec<u8> {
        let body = Postcard.encode(frame).unwrap();
        let mut out = Vec::new();
        wire::ipc::write_frame(&mut out, &body).unwrap();
        out
    }

    #[tokio::test]
    async fn reads_what_wire_writes() {
        let frame = WorkerFrame {
            msgs: vec![WorkerMsg::Fatal {
                message: "x".into(),
                code: 2,
            }],
        };
        let mut bytes = frame_bytes(&frame);
        bytes.extend(frame_bytes(&frame));
        let (mut a, b) = tokio::io::duplex(64);
        tokio::spawn(async move {
            // One byte at a time: the reader must cope with any chunking.
            for byte in bytes {
                a.write_all(&[byte]).await.unwrap();
            }
        });
        let mut io = start(b, tokio::io::sink());
        for _ in 0..2 {
            match io.events.recv().await {
                Some(WorkerEvent::Frame(f)) => assert_eq!(f, frame),
                other => panic!("{other:?}"),
            }
        }
        assert!(matches!(io.events.recv().await, Some(WorkerEvent::Eof)));
    }

    #[tokio::test]
    async fn wire_reads_what_the_writer_writes() {
        let (to_worker, from_gateway) = tokio::io::duplex(1 << 16);
        let io = start(tokio::io::empty(), to_worker);
        io.tx.send(ToWorker::Shutdown { save: true }).unwrap();
        io.tx
            .send(ToWorker::SaveNow {
                reason: protocol::SaveReason::Admin,
            })
            .unwrap();
        drop(io.tx);
        // Read the std side from a blocking task.
        let mut raw = Vec::new();
        let mut from_gateway = from_gateway;
        from_gateway.read_to_end(&mut raw).await.unwrap();
        let mut reader = wire::ipc::FrameReader::new(raw.as_slice());
        let first = reader.next().unwrap().unwrap();
        assert_eq!(
            Postcard.decode::<ToWorker>(&first).unwrap(),
            ToWorker::Shutdown { save: true }
        );
        let second = reader.next().unwrap().unwrap();
        assert!(matches!(
            Postcard.decode::<ToWorker>(&second).unwrap(),
            ToWorker::SaveNow { .. }
        ));
        assert!(reader.next().unwrap().is_none());
    }

    #[tokio::test]
    async fn garbage_ends_the_stream() {
        // A body that is not a `WorkerFrame`.
        let mut bytes = Vec::new();
        wire::ipc::write_frame(&mut bytes, &[0xff; 9]).unwrap();
        let mut io = start(std::io::Cursor::new(bytes), tokio::io::sink());
        assert!(matches!(
            io.events.recv().await,
            Some(WorkerEvent::Garbage(_))
        ));
    }

    #[tokio::test]
    async fn too_long_a_frame_is_refused_before_allocation() {
        let bytes = u32::MAX.to_le_bytes().to_vec();
        let mut r = bytes.as_slice();
        let err = read_frame(&mut r).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn eof_inside_a_frame_is_an_error() {
        let mut bytes = 10u32.to_le_bytes().to_vec();
        bytes.extend([1, 2, 3]);
        let mut r = bytes.as_slice();
        let err = read_frame(&mut r).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
        let mut r: &[u8] = &[1, 2];
        let err = read_frame(&mut r).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[tokio::test]
    async fn a_full_queue_is_reported() {
        let (w, _keep_open) = tokio::io::duplex(8);
        let io = start(tokio::io::empty(), w);
        // The pipe holds 8 bytes, so the writer stalls on the first frame and the queue fills.
        let mut result = Ok(());
        for _ in 0..(TO_WORKER_QUEUE + 8) {
            result = io.tx.send(ToWorker::Shutdown { save: false });
            if result.is_err() {
                break;
            }
        }
        assert_eq!(result, Err(SendError::Full));
    }
}
