//! The journal of a game: JSON lines in `logs/game-<id>.jsonl`, written by a thread of its own so
//! that no task waits on the disk. A line has `ts` (calendar milliseconds) and `kind`, then the fields of the
//! kind (`session_open/close`, `login`, `attach`, `detach`, `cmd`, `result`, `batch`, `view_fail`,
//! `seat_change`, `worker_start/exit/hang`, `save`, `timer`, `chat_meta`, `fault`). Chat is logged as
//! metadata only; passwords, tokens and the text of chat lines never get here. The file turns over at
//! 50 MB and the last three are kept.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use serde_json::{Map, Value};

use crate::clock::WallClock;

/// The file turns over at this size.
pub const ROTATE_AT: u64 = 50 * 1024 * 1024;
/// How many turned-over files stay.
pub const KEEP: usize = 3;

/// A handle that writes lines; cheap to clone. A disabled one drops everything.
#[derive(Clone)]
pub struct GameLog {
    tx: Option<Sender<String>>,
    wall: Option<Arc<dyn WallClock>>,
}

impl std::fmt::Debug for GameLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GameLog")
    }
}

impl GameLog {
    pub fn disabled() -> Self {
        Self {
            tx: None,
            wall: None,
        }
    }

    /// Opens `path` for appending; a failure to open leaves a disabled log and a line in the log.
    pub fn open(path: PathBuf, wall: Arc<dyn WallClock>) -> Self {
        let (tx, rx) = mpsc::channel();
        let writer = match Writer::open(path.clone()) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "cannot open the game journal");
                return Self::disabled();
            }
        };
        let spawned = std::thread::Builder::new()
            .name("game-log".into())
            .spawn(move || writer.run(&rx));
        if spawned.is_err() {
            return Self::disabled();
        }
        Self {
            tx: Some(tx),
            wall: Some(wall),
        }
    }

    /// One line: `kind` and the object `fields` (anything else is ignored).
    pub fn event(&self, kind: &str, fields: Value) {
        let (Some(tx), Some(wall)) = (&self.tx, &self.wall) else {
            return;
        };
        let mut m = Map::new();
        m.insert("ts".into(), wall.unix_ms().into());
        m.insert("kind".into(), kind.into());
        if let Value::Object(f) = fields {
            m.extend(f);
        }
        let _ = tx.send(Value::Object(m).to_string());
    }
}

struct Writer {
    path: PathBuf,
    out: BufWriter<File>,
    size: u64,
}

fn numbered(path: &Path, n: usize) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(format!(".{n}"));
    PathBuf::from(s)
}

impl Writer {
    fn open(path: PathBuf) -> std::io::Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let size = file.metadata()?.len();
        Ok(Self {
            path,
            out: BufWriter::new(file),
            size,
        })
    }

    fn run(mut self, rx: &Receiver<String>) {
        while let Ok(line) = rx.recv() {
            self.write(&line);
            // Whatever is queued goes out together, then the file is flushed.
            while let Ok(more) = rx.try_recv() {
                self.write(&more);
            }
            let _ = self.out.flush();
        }
        let _ = self.out.flush();
    }

    fn write(&mut self, line: &str) {
        if self.size >= ROTATE_AT {
            self.rotate();
        }
        if writeln!(self.out, "{line}").is_ok() {
            self.size += line.len() as u64 + 1;
        }
    }

    /// `game-1.jsonl` becomes `.1`, `.1` becomes `.2`, ...; the oldest beyond [`KEEP`] goes.
    fn rotate(&mut self) {
        let _ = self.out.flush();
        let _ = std::fs::remove_file(numbered(&self.path, KEEP));
        for n in (1..KEEP).rev() {
            let _ = std::fs::rename(numbered(&self.path, n), numbered(&self.path, n + 1));
        }
        let _ = std::fs::rename(&self.path, numbered(&self.path, 1));
        if let Ok(file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            self.out = BufWriter::new(file);
            self.size = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::TestWallClock;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("civ5-gamelog-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn lines_are_json_with_a_time_and_a_kind() {
        let dir = scratch("lines");
        let path = dir.join("game-1.jsonl");
        let log = GameLog::open(path.clone(), Arc::new(TestWallClock::new(42)));
        log.event("cmd", serde_json::json!({"id": 7, "player": 0}));
        log.event("save", serde_json::json!({"seq": 1}));
        drop(log);
        // The writer flushes when its queue closes.
        for _ in 0..50 {
            if std::fs::read_to_string(&path).is_ok_and(|t| t.lines().count() == 2) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let first: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(first["ts"], 42);
        assert_eq!(first["kind"], "cmd");
        assert_eq!(first["id"], 7);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_file_turns_over_and_only_the_last_three_stay() {
        let dir = scratch("rotate");
        let path = dir.join("game-2.jsonl");
        let mut w = Writer::open(path.clone()).unwrap();
        for i in 0..5 {
            w.write(&format!("line {i}"));
            w.size = ROTATE_AT;
        }
        let _ = w.out.flush();
        let read = |p: &Path| std::fs::read_to_string(p).unwrap_or_default();
        assert_eq!(read(&path).trim(), "line 4");
        assert_eq!(read(&numbered(&path, 1)).trim(), "line 3");
        assert_eq!(read(&numbered(&path, 2)).trim(), "line 2");
        assert_eq!(read(&numbered(&path, 3)).trim(), "line 1");
        assert!(!numbered(&path, 4).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
