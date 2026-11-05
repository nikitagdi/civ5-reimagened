//! M1 in miniature: two bots on missions play 5 turns of a real `civ5-server` on the test
//! world, and what they hold equals what the gateway holds. The full run (20 turns, the report) is
//! `cargo xtask nettest --scenario m1`. The server is built by this test if it is not there.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use net_test::bot::mission::{Mission, play_with};
use serde_json::{Value, json};

struct Server {
    child: Child,
    dir: PathBuf,
    listen: String,
    admin: String,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// `target/<profile>/civ5-server`, built here when missing.
fn server_exe() -> PathBuf {
    let me = std::env::current_exe().unwrap();
    let profile_dir = me.parent().and_then(Path::parent).unwrap().to_path_buf();
    let exe = profile_dir.join(format!("civ5-server{}", std::env::consts::EXE_SUFFIX));
    if !exe.is_file() {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let status = Command::new(cargo)
            .args(["build", "-p", "server"])
            .status()
            .expect("cargo build -p server");
        assert!(status.success(), "cargo build -p server failed");
    }
    assert!(exe.is_file(), "{} was not built", exe.display());
    exe
}

fn start() -> Server {
    let dir = std::env::temp_dir().join(format!("civ5-nettest-m1-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let listen = format!("127.0.0.1:{}", free_port());
    let admin = format!("127.0.0.1:{}", free_port());
    let exe = server_exe();
    let child = Command::new(&exe)
        .args(["--dev", "--core", "fixture", "--listen", &listen])
        .args(["--admin-listen", &admin, "--state-dir"])
        .arg(&dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let server = Server {
        child,
        dir,
        listen,
        admin,
    };
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let ok = Command::new(&exe)
            .args(["healthcheck", "--dev", "--core", "fixture", "--listen"])
            .arg(&server.listen)
            .arg("--state-dir")
            .arg(&server.dir)
            .output()
            .unwrap()
            .status
            .success();
        if ok {
            return server;
        }
        assert!(Instant::now() < deadline, "/healthz never turned 200");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn admin_get(s: &Server, path: &str) -> Value {
    let token = std::fs::read_to_string(s.dir.join("admin.token")).unwrap();
    let mut c = TcpStream::connect(&s.admin).unwrap();
    c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        c,
        "GET {path} HTTP/1.0\r\nHost: localhost\r\nX-Admin-Token: {}\r\n\r\n",
        token.trim()
    )
    .unwrap();
    let mut raw = String::new();
    c.read_to_string(&mut raw).unwrap();
    serde_json::from_str(raw.split("\r\n\r\n").nth(1).unwrap()).unwrap()
}

fn mission(url: &str, nick: &str, lobby: Value) -> Mission {
    serde_json::from_value(json!({
        "url": url,
        "nick": nick,
        "lobby": lobby,
        "turns": 5,
        "seed": 3,
        "lobby_timeout_ms": 60000,
        "turn_timeout_ms": 60000,
    }))
    .unwrap()
}

#[test]
fn two_bots_play_five_turns_and_their_views_equal_the_gateways() {
    let server = start();
    let url = format!("ws://{}/ws", server.listen);
    let host = mission(
        &url,
        "ta",
        json!({ "role": "host", "humans": 2, "ai_slots": [2] }),
    );
    let guest = mission(&url, "tb", json!({ "role": "guest", "host": "ta" }));
    // `--dev` has the public password `dev`.
    let play_it = |m: Mission| std::thread::spawn(move || play_with(&m, "dev", None));
    let results: Vec<Value> = {
        let a = play_it(host);
        let b = play_it(guest);
        vec![a.join().unwrap(), b.join().unwrap()]
    };
    for r in &results {
        assert_eq!(r["errors"], json!([]), "{}", r["nick"]);
        assert_eq!(r["play"]["done"], "turns", "{}", r["nick"]);
        assert_eq!(r["turns_played"], 5);
        assert_eq!(r["hash"]["mismatch"], 0);
        assert_eq!(r["fog"]["violations"], 0);
        assert_eq!(r["need_keyframe"], json!({}));
        assert_eq!(r["ledger"]["open"], 0);
    }
    // The gateway publishes a seat's view a moment after it sends it.
    let mut m = Value::Null;
    for _ in 0..50 {
        m = admin_get(&server, "/admin/metrics");
        let seats = &m["games"]["1"]["seats"];
        if results.iter().all(|r| {
            seats[r["player"].as_i64().unwrap().to_string()]["view_hash"] == r["final"]["view_hash"]
        }) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    for r in &results {
        let p = r["player"].as_i64().unwrap().to_string();
        assert_eq!(
            m["games"]["1"]["seats"][&p]["view_hash"], r["final"]["view_hash"],
            "{}'s copy is the gateway's view",
            r["nick"]
        );
    }
    let g = &m["games"]["1"];
    assert_eq!(g["cmds"]["lost"], 0);
    assert_eq!(g["cmds"]["expired"], 0);
    assert_eq!(g["fog_violations"], 0);
    assert_eq!(g["views"]["need_keyframe"]["HashMismatch"], 0);
}
