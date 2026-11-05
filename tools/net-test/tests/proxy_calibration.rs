//! `netproxy` gives what it promises (`calibrate`): the delay, the bandwidth, the order after a
//! blackhole, a reset as RST. The upstream is a local echo server; the same checks run in the `calibrate` scenario
//! of the runner against the real binary.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use net_test::proxy::{self, Proxy};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// The tolerance of a measured RTT: the timers of Windows step by about 15 ms.
const RTT_TOLERANCE_MS: f64 = if cfg!(windows) { 16.0 } else { 5.0 };

async fn echo_server() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = vec![0u8; 16384];
                loop {
                    match s.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => {
                            if s.write_all(&buf[..n]).await.is_err() {
                                return;
                            }
                        }
                    }
                }
            });
        }
    });
    addr
}

async fn start() -> Proxy {
    let up = echo_server().await;
    let any: SocketAddr = "127.0.0.1:0".parse().unwrap();
    proxy::start(any, up, any, 7).await.unwrap()
}

/// A bare HTTP/1.1 call to the control port.
async fn control(p: &Proxy, method: &str, path: &str, body: Value) -> Value {
    let mut s = TcpStream::connect(p.control).await.unwrap();
    let body = body.to_string();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).await.unwrap();
    let text = String::from_utf8(out).unwrap();
    let (head, json_body) = text.split_once("\r\n\r\n").unwrap();
    assert!(head.starts_with("HTTP/1.1 200"), "{path}: {head}");
    // axum may send the body chunked: take from the first `{` to the last `}`.
    let a = json_body.find('{').unwrap();
    let b = json_body.rfind('}').unwrap();
    serde_json::from_str(&json_body[a..=b]).unwrap()
}

async fn ping(s: &mut TcpStream) -> f64 {
    let t = Instant::now();
    s.write_all(b"ping").await.unwrap();
    let mut b = [0u8; 4];
    s.read_exact(&mut b).await.unwrap();
    t.elapsed().as_secs_f64() * 1000.0
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rtt_is_twice_the_delay() {
    let p = start().await;
    for rtt in [100u32, 200, 400] {
        control(
            &p,
            "POST",
            "/profile",
            json!({ "reset": true, "delay_ms": rtt / 2, "connect_delay_ms": 0 }),
        )
        .await;
        let mut s = TcpStream::connect(p.listen).await.unwrap();
        s.set_nodelay(true).unwrap();
        ping(&mut s).await;
        let mut xs = Vec::new();
        for _ in 0..(if rtt == 400 { 10 } else { 30 }) {
            xs.push(ping(&mut s).await);
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median = xs[xs.len() / 2];
        assert!(
            (median - f64::from(rtt)).abs() <= RTT_TOLERANCE_MS,
            "RTT {rtt}: measured median {median:.1} ms"
        );
    }
    p.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bandwidth_is_within_ten_percent() {
    let p = start().await;
    control(
        &p,
        "POST",
        "/profile",
        json!({ "reset": true, "rate_kbit": 1000, "connect_delay_ms": 0 }),
    )
    .await;
    let s = TcpStream::connect(p.listen).await.unwrap();
    s.set_nodelay(true).unwrap();
    let total = 100_000usize;
    let (mut r, mut w) = s.into_split();
    tokio::spawn(async move {
        w.write_all(&vec![7u8; total]).await.unwrap();
        w.flush().await.unwrap();
        // The write half lives on until the test ends.
        tokio::time::sleep(Duration::from_secs(30)).await;
    });
    let mut got = 0usize;
    let mut first = None;
    let mut buf = vec![0u8; 16384];
    let mut after_first = 0usize;
    while got < total {
        let n = r.read(&mut buf).await.unwrap();
        assert!(n > 0, "closed after {got}");
        got += n;
        match first {
            None => first = Some(Instant::now()),
            Some(_) => after_first += n,
        }
    }
    let secs = first.unwrap().elapsed().as_secs_f64();
    let kbit = after_first as f64 * 8.0 / 1000.0 / secs;
    assert!(
        (kbit - 1000.0).abs() <= 100.0,
        "throughput {kbit:.0} kbit/s"
    );
    p.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn blackhole_delivers_the_queue_in_order() {
    let p = start().await;
    control(
        &p,
        "POST",
        "/profile",
        json!({ "reset": true, "connect_delay_ms": 0 }),
    )
    .await;
    let mut s = TcpStream::connect(p.listen).await.unwrap();
    s.set_nodelay(true).unwrap();
    ping(&mut s).await;
    control(&p, "POST", "/blackhole", json!({ "ms": 400 })).await;
    let t0 = Instant::now();
    for tag in [b"aaaa", b"bbbb", b"cccc"] {
        s.write_all(tag).await.unwrap();
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    let mut got = Vec::new();
    let mut buf = [0u8; 64];
    while got.len() < 12 {
        let n = s.read(&mut buf).await.unwrap();
        assert!(
            t0.elapsed() >= Duration::from_millis(380),
            "something came through the blackhole"
        );
        got.extend_from_slice(&buf[..n]);
    }
    assert_eq!(got, b"aaaabbbbcccc");
    p.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reset_closes_with_rst() {
    let p = start().await;
    control(
        &p,
        "POST",
        "/profile",
        json!({ "reset": true, "connect_delay_ms": 0 }),
    )
    .await;
    let mut s = TcpStream::connect(p.listen).await.unwrap();
    ping(&mut s).await;
    let r = control(&p, "POST", "/reset", json!({ "side": "client" })).await;
    assert_eq!(r["reset"], 1);
    let mut buf = [0u8; 8];
    let err = s
        .read(&mut buf)
        .await
        .expect_err("a reset is an error, not a clean EOF");
    assert!(
        matches!(
            err.kind(),
            std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
        ),
        "{err:?}"
    );
    let stats = control(&p, "GET", "/stats", json!({})).await;
    assert_eq!(stats["conns"]["resets"], 1);
    p.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn half_open_silences_one_direction() {
    let p = start().await;
    control(
        &p,
        "POST",
        "/profile",
        json!({ "reset": true, "connect_delay_ms": 0 }),
    )
    .await;
    let mut s = TcpStream::connect(p.listen).await.unwrap();
    ping(&mut s).await;
    control(
        &p,
        "POST",
        "/half-open",
        json!({ "dir": "down", "ms": 300 }),
    )
    .await;
    let t = ping(&mut s).await;
    assert!(t >= 250.0, "the answer came after {t:.0} ms");
    p.stop();
}
