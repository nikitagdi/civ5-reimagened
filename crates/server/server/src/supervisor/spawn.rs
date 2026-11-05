//! Starting a worker process: `current_exe() worker`, stdin and stdout are
//! pipes, stderr goes to `logs/worker-<id>.log` (rotated at every start, the last 5 kept), the
//! working directory is `state_dir/games/<id>/`.

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use protocol::ToWorker;
use tokio::process::Command;
use tokio::sync::mpsc;

use super::ipc::{self, ExitInfo, IpcTx, SendError, WorkerEvent};

/// Log files kept per worker id: the current one and the older ones.
pub const LOGS_KEPT: usize = 5;

/// After the process ended, wait this long for the reader to deliver what is left in the pipe
/// (a grandchild could hold it open).
const DRAIN: Duration = Duration::from_secs(2);

/// How to start one worker.
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    /// The program: `current_exe()` for the real thing.
    pub exe: PathBuf,
    /// Its arguments: `["worker"]` for the real thing.
    pub args: Vec<OsString>,
    /// Working directory; created if missing.
    pub work_dir: PathBuf,
    /// `logs/worker-<id>.log`.
    pub stderr_log: PathBuf,
    /// The stderr file turns over (as at every start) once it reaches this many bytes; 0: it grows as long as
    /// the worker lives.
    pub stderr_max_bytes: u64,
}

/// A running worker process with its IPC.
pub struct WorkerProc {
    pid: Option<u32>,
    tx: IpcTx,
    events: mpsc::Receiver<WorkerEvent>,
    kill: mpsc::UnboundedSender<()>,
}

impl WorkerProc {
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// Queues a frame for the worker.
    pub fn send(&self, msg: ToWorker) -> Result<(), SendError> {
        self.tx.send(msg)
    }

    /// A handle that only sends, for tasks that do not own the process.
    pub fn sender(&self) -> IpcTx {
        self.tx.clone()
    }

    /// Next event; `None` once `Exited` has been delivered and the queue is closed.
    pub async fn next_event(&mut self) -> Option<WorkerEvent> {
        self.events.recv().await
    }

    /// Kills the process (SIGKILL on Unix, `TerminateProcess` on Windows). `Exited` follows.
    /// Dropping the `WorkerProc` kills it too.
    pub fn kill(&self) {
        let _ = self.kill.send(());
    }
}

/// Starts the process and its IPC tasks. Must run inside the tokio runtime.
pub fn spawn(spec: &SpawnSpec) -> io::Result<WorkerProc> {
    fs::create_dir_all(&spec.work_dir)?;
    if let Some(dir) = spec.stderr_log.parent() {
        fs::create_dir_all(dir)?;
    }
    rotate_log(&spec.stderr_log, LOGS_KEPT);
    let mut log = RotatingLog::create(spec.stderr_log.clone(), spec.stderr_max_bytes)?;

    let mut child = Command::new(&spec.exe)
        .args(&spec.args)
        .current_dir(&spec.work_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let pid = child.id();
    let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        return Err(io::Error::other("worker pipes are missing"));
    };

    // The stderr of the worker goes through the gateway so that the file can turn over at its size cap.
    if let Some(mut err) = child.stderr.take() {
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut buf = vec![0u8; 16 * 1024];
            while let Ok(n) = err.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                log.write(&buf[..n]);
            }
        });
    }

    let io = ipc::start(stdout, stdin);
    let (kill, mut kill_rx) = mpsc::unbounded_channel::<()>();
    let (reader, events_tx) = (io.reader, io.events_tx);
    // The process watcher: the only owner of the `Child`. `Exited` goes out last.
    tokio::spawn(async move {
        let mut killed = false;
        let status = loop {
            tokio::select! {
                status = child.wait() => break status,
                // A kill request, or every `WorkerProc` handle gone: both end the process.
                _ = kill_rx.recv(), if !killed => {
                    killed = true;
                    let _ = child.start_kill();
                }
            }
        };
        // Everything the worker wrote before it ended comes first.
        let _ = tokio::time::timeout(DRAIN, reader).await;
        let code = status.ok().and_then(|s| s.code());
        let _ = events_tx
            .send(WorkerEvent::Exited(ExitInfo { code, killed }))
            .await;
    });

    Ok(WorkerProc {
        pid,
        tx: io.tx,
        events: io.events,
        kill,
    })
}

/// The current log file of a worker, turned over when it reaches its size cap: `rotate_log` with [LOGS_KEPT]
/// files in all.
pub struct RotatingLog {
    path: PathBuf,
    file: File,
    size: u64,
    max: u64,
}

impl RotatingLog {
    /// Creates (truncates) path; max 0: no cap.
    pub fn create(path: PathBuf, max: u64) -> io::Result<Self> {
        let file = File::create(&path)?;
        Ok(Self {
            path,
            file,
            size: 0,
            max,
        })
    }

    /// Appends ytes. A file at its cap is turned over first. Best effort: a write error drops the text.
    pub fn write(&mut self, bytes: &[u8]) {
        if self.max > 0 && self.size >= self.max {
            rotate_log(&self.path, LOGS_KEPT);
            match File::create(&self.path) {
                Ok(f) => {
                    self.file = f;
                    self.size = 0;
                }
                Err(_) => return,
            }
        }
        if self.file.write_all(bytes).is_ok() {
            self.size += bytes.len() as u64;
        }
    }
}

/// `worker-3.log` -> `worker-3.log.1` -> ... keeping `keep` files in all (the current one
/// included). Best effort: a log that cannot be rotated is overwritten.
pub fn rotate_log(path: &Path, keep: usize) {
    if keep == 0 || !path.exists() {
        return;
    }
    let numbered = |n: usize| {
        let mut name = path.as_os_str().to_owned();
        name.push(format!(".{n}"));
        PathBuf::from(name)
    };
    let _ = fs::remove_file(numbered(keep - 1));
    for n in (1..keep.saturating_sub(1)).rev() {
        let _ = fs::rename(numbered(n), numbered(n + 1));
    }
    if keep > 1 {
        let _ = fs::rename(path, numbered(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logs_rotate_and_the_oldest_goes() {
        let dir = std::env::temp_dir().join(format!("civ5-rotate-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let log = dir.join("worker-1.log");
        for run in 0..8 {
            rotate_log(&log, LOGS_KEPT);
            fs::write(&log, format!("run {run}")).unwrap();
        }
        let read = |name: &str| fs::read_to_string(dir.join(name)).ok();
        assert_eq!(read("worker-1.log").as_deref(), Some("run 7"));
        assert_eq!(read("worker-1.log.1").as_deref(), Some("run 6"));
        assert_eq!(read("worker-1.log.3").as_deref(), Some("run 4"));
        // Five files in all: the current and four older.
        assert_eq!(read("worker-1.log.4").as_deref(), Some("run 3"));
        assert_eq!(read("worker-1.log.5"), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_worker_log_turns_over_at_its_cap_and_stays_bounded() {
        let dir = std::env::temp_dir().join(format!("civ5-rotating-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("worker-2.log");
        let mut log = RotatingLog::create(path.clone(), 100).unwrap();
        for i in 0..100u8 {
            log.write(&[b'a' + i % 26; 30]);
        }
        let mut total = 0;
        let mut files = 0;
        for e in fs::read_dir(&dir).unwrap().flatten() {
            let n = e.metadata().unwrap().len();
            // The cap is checked before a write: a file holds at most the cap and one chunk.
            assert!(n <= 100 + 30, "{:?} is {n} bytes", e.file_name());
            total += n;
            files += 1;
        }
        assert_eq!(
            files, LOGS_KEPT,
            "the current file and the older ones, no more"
        );
        assert!(total <= (LOGS_KEPT as u64) * 130);
        // Without a cap it only grows.
        let mut open = RotatingLog::create(dir.join("open.log"), 0).unwrap();
        for _ in 0..10 {
            open.write(&[1u8; 1000]);
        }
        assert_eq!(fs::metadata(dir.join("open.log")).unwrap().len(), 10_000);
        let _ = fs::remove_dir_all(&dir);
    }
}
