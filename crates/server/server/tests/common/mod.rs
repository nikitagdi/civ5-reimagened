//! Helpers shared by the integration tests. The mock worker itself is the `mock-worker` binary
//! (`mock_worker.rs`); this module only starts it.

#![allow(dead_code)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use protocol::{CoreKind, WorkerConfig};
use server::supervisor::{SpawnSpec, Supervisor, SupervisorConfig};

/// Path of the compiled mock worker.
pub const MOCK_WORKER: &str = env!("CARGO_BIN_EXE_mock-worker");

/// A fresh, empty scratch directory.
pub fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("civ5-sup-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A [`SpawnSpec`] for the mock worker in `mode` (`"echo"`, `"crash-after n=3"`, ...), logging
/// under `dir`.
pub fn mock_spec(dir: &Path, mode: &str) -> SpawnSpec {
    SpawnSpec {
        exe: MOCK_WORKER.into(),
        args: mode.split_whitespace().map(OsString::from).collect(),
        work_dir: dir.join("work"),
        stderr_log: dir.join("logs").join("worker-test.log"),
        stderr_max_bytes: 0,
    }
}

/// A supervisor whose workers are the mock worker in `mode`; `state_dir` is `dir`.
pub fn mock_supervisor(dir: &Path, mode: &str, hang: Duration) -> Supervisor {
    Supervisor::new(SupervisorConfig {
        exe: MOCK_WORKER.into(),
        args: mode.split_whitespace().map(OsString::from).collect(),
        state_dir: dir.to_path_buf(),
        core: CoreKind::Fixture {
            huge: false,
            wrap_x: false,
        },
        worker: WorkerConfig::default(),
        hang,
        stderr_max_bytes: 0,
    })
}

/// A [`ServerConfig`](server::ServerConfig) whose workers are the mock worker in `mode`, with quick restarts
/// (`delays` before the first, second and third restart).
pub fn mock_server_config(
    dir: &Path,
    mode: &str,
    hang: Duration,
    delays: [Duration; 3],
) -> server::ServerConfig {
    let sup = mock_supervisor(dir, mode, hang);
    server::ServerConfig {
        state_dir: Some(dir.to_path_buf()),
        supervisor: Some(sup.config().clone()),
        restart_policy: server::supervisor::policy::PolicyConfig {
            delays,
            ..Default::default()
        },
        ..server::ServerConfig::default()
    }
}
