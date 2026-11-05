//! The `civ5-server` binary: subcommands, loopback guard, state_dir lock.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_civ5-server");

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("civ5-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn kill(mut c: Child) {
    let _ = c.kill();
    let _ = c.wait();
}

#[test]
fn worker_without_init_exits_with_code_2() {
    // `output()` gives the child a null stdin: EOF before `Init` is an IPC error (game-host-mp 4.12).
    let out = Command::new(BIN).arg("worker").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn non_loopback_listen_is_refused_without_permission() {
    let out = Command::new(BIN)
        .args(["--core", "fixture", "--listen", "0.0.0.0:0"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("allow_plain_nonlocal"), "{err}");
}

#[test]
fn unknown_subcommand_is_refused() {
    let out = Command::new(BIN).arg("frobnicate").output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn second_gateway_on_the_same_state_dir_does_not_start() {
    let dir = scratch("lock");
    let args = |d: &PathBuf| {
        vec![
            "--dev".to_string(),
            "--core".into(),
            "fixture".into(),
            "--listen".into(),
            "127.0.0.1:0".into(),
            "--admin-listen".into(),
            "127.0.0.1:0".into(),
            "--state-dir".into(),
            d.to_str().unwrap().into(),
        ]
    };
    let first = Command::new(BIN)
        .args(args(&dir))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // The first gateway has the lock once admin.token exists (written right after the lock).
    let deadline = Instant::now() + Duration::from_secs(20);
    while !dir.join("admin.token").exists() {
        assert!(Instant::now() < deadline, "first gateway did not start");
        std::thread::sleep(Duration::from_millis(50));
    }
    let second = Command::new(BIN).args(args(&dir)).output().unwrap();
    kill(first);
    assert_eq!(second.status.code(), Some(2));
    let err = String::from_utf8_lossy(&second.stderr);
    assert!(err.contains("in use by another civ5-server"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn healthcheck_succeeds_against_a_running_gateway() {
    let dir = scratch("health");
    // A fixed free port: healthcheck probes the configured listen address.
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let listen = format!("127.0.0.1:{port}");
    let args = vec![
        "--dev".to_string(),
        "--core".into(),
        "fixture".into(),
        "--listen".into(),
        listen.clone(),
        "--admin-listen".into(),
        "127.0.0.1:0".into(),
        "--state-dir".into(),
        dir.to_str().unwrap().into(),
    ];
    let hc = |extra: &[&str]| {
        Command::new(BIN)
            .arg("healthcheck")
            .args(&args[..])
            .args(extra)
            .output()
            .unwrap()
            .status
            .code()
    };
    // Nothing listens yet.
    assert_eq!(hc(&[]), Some(1));
    let gateway = Command::new(BIN)
        .args(&args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let code = loop {
        let code = hc(&[]);
        if code == Some(0) || Instant::now() > deadline {
            break code;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    kill(gateway);
    assert_eq!(code, Some(0), "healthcheck never succeeded");
    let _ = std::fs::remove_dir_all(&dir);
}
