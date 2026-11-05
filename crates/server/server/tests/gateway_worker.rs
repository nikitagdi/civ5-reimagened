//! The gateway process with the real `worker` subcommand on the test world (no mock): the `Describe` pass
//! runs at start, /healthz turns 200, and a client that logs in gets `Choices` from it.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use protocol::{ClientMsg, Codec, Login, PROTOCOL_VERSION, Postcard, ServerMsg};
use tungstenite::Message;
use wire::{Encoder, Limits};

const BIN: &str = env!("CARGO_BIN_EXE_civ5-server");

struct Gateway {
    child: Child,
    dir: PathBuf,
    listen: String,
}

impl Drop for Gateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn start() -> Gateway {
    let dir = std::env::temp_dir().join(format!("civ5-gw-worker-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let listen = format!("127.0.0.1:{port}");
    let child = Command::new(BIN)
        .args(["--dev", "--core", "fixture", "--listen", &listen])
        .args(["--admin-listen", "127.0.0.1:0", "--state-dir"])
        .arg(&dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    Gateway { child, dir, listen }
}

#[test]
fn login_gets_choices_from_a_describe_run_of_the_real_worker() {
    let gw = start();
    // /healthz is 200 once the `Describe` pass is in.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let ok = Command::new(BIN)
            .args([
                "healthcheck",
                "--dev",
                "--core",
                "fixture",
                "--listen",
                &gw.listen,
            ])
            .arg("--state-dir")
            .arg(&gw.dir)
            .output()
            .unwrap()
            .status
            .success();
        if ok {
            break;
        }
        assert!(Instant::now() < deadline, "/healthz never turned 200");
        std::thread::sleep(Duration::from_millis(100));
    }

    let (mut ws, _) = tungstenite::connect(format!("ws://{}/ws", gw.listen)).unwrap();
    let mut send = |msg: &ClientMsg| {
        let body = Postcard.encode(msg).unwrap();
        ws.send(Message::binary(Encoder::plain(&body))).unwrap();
    };
    send(&ClientMsg::Hello {
        protocol: PROTOCOL_VERSION,
        build: "test".into(),
        nick: "alice".into(),
    });
    send(&ClientMsg::Login(Login {
        password: "dev".into(),
        token: None,
        resume: None,
    }));
    let mut choices = None;
    for _ in 0..4 {
        let Message::Binary(b) = ws.read().unwrap() else {
            panic!("expected a binary message");
        };
        if let ServerMsg::Choices(c) = wire::decode(&b, &Limits::CLIENT).unwrap() {
            choices = Some(c);
            break;
        }
    }
    let choices = choices.expect("no Choices after login");
    assert!(!choices.civs.is_empty(), "the lobby lists are empty");
    assert!(!choices.world_sizes.is_empty());
    // The catalog of the worker is stored under its hash.
    let catalogs: Vec<_> = std::fs::read_dir(gw.dir.join("catalog"))
        .expect("state_dir/catalog")
        .map(|e| e.unwrap().metadata().unwrap().len())
        .collect();
    assert!(catalogs.iter().any(|&n| n > 0), "no catalog on disk");
}
