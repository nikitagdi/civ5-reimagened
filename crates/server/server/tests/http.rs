//! HTTP routes: web and assets, `/healthz`, `/catalog/{hash}`, `/netcheck/ws`,
//! the admin API.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use server::http::catalog;
use server::identity::Secret;
use server::{ServerConfig, Shared};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

struct TestServer {
    addr: SocketAddr,
    admin_addr: SocketAddr,
    shared: Arc<Shared>,
    _rt: tokio::runtime::Runtime,
}

impl TestServer {
    fn start(config: ServerConfig) -> Self {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let bind = || {
            rt.block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
                .unwrap()
        };
        let (listener, admin) = (bind(), bind());
        let addr = listener.local_addr().unwrap();
        let admin_addr = admin.local_addr().unwrap();
        let shared = Shared::new(config).unwrap();
        rt.spawn(server::serve(listener, shared.clone()));
        rt.spawn(server::serve_admin(admin, shared.clone()));
        Self {
            addr,
            admin_addr,
            shared,
            _rt: rt,
        }
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("civ5-http-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Minimal HTTP/1.0 GET with optional extra headers.
fn http_get_h(addr: SocketAddr, path: &str, headers: &[(&str, &str)]) -> (u16, String, Vec<u8>) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(s, "GET {path} HTTP/1.0\r\nHost: localhost\r\n").unwrap();
    for (k, v) in headers {
        write!(s, "{k}: {v}\r\n").unwrap();
    }
    write!(s, "\r\n").unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).unwrap();
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("end of headers");
    let head = String::from_utf8_lossy(&raw[..split]).to_ascii_lowercase();
    let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, head, raw[split + 4..].to_vec())
}

fn http_get(addr: SocketAddr, path: &str) -> (u16, String, Vec<u8>) {
    http_get_h(addr, path, &[])
}

/// Minimal HTTP/1.0 POST of `body`: the status.
fn http_post(addr: SocketAddr, path: &str, body: &[u8]) -> u16 {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        s,
        "POST {path} HTTP/1.0\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .unwrap();
    // A refused body may close the socket before all of it is written.
    let _ = s.write_all(body);
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let head = String::from_utf8_lossy(&raw);
    head.split_whitespace().nth(1).unwrap().parse().unwrap()
}

/// The web client's crash report (live report 2026-10-31): taken (204), capped (413), rate-limited (429).
#[test]
fn client_report_is_taken_capped_and_rate_limited() {
    let srv = TestServer::start(ServerConfig::default());
    let report = br#"{"kind":"wasm-terminated","first":{"message":"unreachable"}}"#;
    assert_eq!(http_post(srv.addr, "/client-report", report), 204);
    let big = vec![b'x'; server::http::client_report::MAX_BODY + 1];
    assert_eq!(http_post(srv.addr, "/client-report", &big), 413);
    // The limit of one address: a few reports a minute.
    let statuses: Vec<u16> = (0..5)
        .map(|_| http_post(srv.addr, "/client-report", report))
        .collect();
    assert!(statuses.contains(&429), "{statuses:?}");
}

/// HTTP/1.0 POST with extra headers: the status and the body.
fn http_post_h(
    addr: SocketAddr,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> (u16, String) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut head = format!(
        "POST {path} HTTP/1.0\r\nHost: localhost\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes()).unwrap();
    let _ = s.write_all(body);
    let mut raw = Vec::new();
    let _ = s.read_to_end(&mut raw);
    let text = String::from_utf8_lossy(&raw).into_owned();
    let status = text.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = text.split("\r\n\r\n").nth(1).unwrap_or_default().to_owned();
    (status, body)
}

fn gzip_bytes(b: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    e.write_all(b).unwrap();
    e.finish().unwrap()
}

/// `POST /client-report/v2`: logged-in players only, capped, rate-limited, stored under
/// `state_dir/reports/` and read through the admin API.
#[test]
fn client_report_v2_is_stored_for_a_logged_in_player_and_read_by_the_admin_api() {
    let dir = temp_dir("report-v2");
    let srv = TestServer::start(ServerConfig {
        state_dir: Some(dir.clone()),
        admin_token: Some(Secret::new("adm")),
        ..ServerConfig::default()
    });
    // An account with one device; its token is what the page would send.
    let grant = srv
        .shared
        .identity
        .register("Ann", "$argon2id$unused".into(), None, "test")
        .unwrap();
    let hex = server::identity::token_to_hex(&grant.token);
    let lines = [
        r#"{"ts":1,"kind":"log","lvl":"error","msg":"lua exploded"}"#,
        r#"{"ts":2,"kind":"span","span":"c.load","dur_us":5}"#,
    ]
    .join("\n");
    let doc = serde_json::json!({
        "v": 2, "comment": "my archer did not move",
        "client": {"build": "x"}, "where": {"game": 936, "turn": 126},
        "sessions": [{"start": 1000, "lines": lines}],
        "perf": {"v": 1, "passes": [{"name": "Main", "gpu_ms": 19.3}]},
    })
    .to_string();
    let body = gzip_bytes(doc.as_bytes());
    let auth = [
        ("X-Civ5-Nick", "Ann"),
        ("X-Civ5-Token", hex.as_str()),
        ("Content-Encoding", "gzip"),
    ];

    // No token, a wrong token: 401.
    let (st, _) = http_post_h(
        srv.addr,
        "/client-report/v2",
        &[("Content-Encoding", "gzip")],
        &body,
    );
    assert_eq!(st, 401);
    let bad = "00".repeat(16);
    let (st, _) = http_post_h(
        srv.addr,
        "/client-report/v2",
        &[
            ("X-Civ5-Nick", "Ann"),
            ("X-Civ5-Token", bad.as_str()),
            ("Content-Encoding", "gzip"),
        ],
        &body,
    );
    assert_eq!(st, 401);
    // Over 4 MiB: 413.
    let big = vec![7u8; server::http::reports::MAX_BODY + 1];
    let (st, _) = http_post_h(srv.addr, "/client-report/v2", &auth, &big);
    assert_eq!(st, 413);
    // A bomb: small body, huge inflated text.
    let bomb = gzip_bytes(&vec![b' '; server::http::reports::MAX_INFLATED + 10]);
    assert!(bomb.len() < server::http::reports::MAX_BODY);
    let (st, _) = http_post_h(srv.addr, "/client-report/v2", &auth, &bomb);
    assert_eq!(st, 413);
    // Taken.
    let (st, answer) = http_post_h(srv.addr, "/client-report/v2", &auth, &body);
    assert_eq!(st, 201, "{answer}");
    let id = serde_json::from_str::<serde_json::Value>(&answer).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        dir.join("reports")
            .join(&id)
            .join("client.jsonl.gz")
            .exists()
    );

    // Read through the admin API.
    let adm = [("x-admin-token", "adm")];
    let (st, _, list) = http_get_h(srv.admin_addr, "/admin/reports", &adm);
    assert_eq!(st, 200);
    let list: serde_json::Value = serde_json::from_slice(&list).unwrap();
    assert_eq!(list["reports"][0]["id"], id.as_str());
    assert_eq!(list["reports"][0]["game"], 936);
    let (st, _, one) = http_get_h(srv.admin_addr, &format!("/admin/reports/{id}"), &adm);
    assert_eq!(st, 200);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&one).unwrap()["comment"],
        "my archer did not move"
    );
    let (st, _, log) = http_get_h(
        srv.admin_addr,
        &format!("/admin/reports/{id}/log?grep=exploded&level=error"),
        &adm,
    );
    assert_eq!(st, 200);
    let log = String::from_utf8(log).unwrap();
    assert!(
        log.contains("lua exploded") && !log.contains("c.load"),
        "{log}"
    );
    // The performance snapshot of the panel's "Send report" comes back whole.
    assert_eq!(list["reports"][0]["perf"], true);
    let (st, _, perf) = http_get_h(srv.admin_addr, &format!("/admin/reports/{id}/perf"), &adm);
    assert_eq!(st, 200);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&perf).unwrap()["passes"][0]["gpu_ms"],
        19.3
    );
    let (st, _, _) = http_get_h(srv.admin_addr, "/admin/reports/..%2Fx/log", &adm);
    assert_eq!(st, 404);
    let (st, _, _) = http_get_h(srv.admin_addr, "/admin/reports", &[]);
    assert_eq!(st, 401, "the admin API needs its token");

    // Plain JSON (no gzip) is taken too.
    let plain = [("X-Civ5-Nick", "Ann"), ("X-Civ5-Token", hex.as_str())];
    let (st, answer) = http_post_h(srv.addr, "/client-report/v2", &plain, doc.as_bytes());
    assert_eq!(st, 201, "{answer}");

    // The nick's limit: 3 an hour (one is taken, one 4xx above did not count).
    let mut statuses = Vec::new();
    for _ in 0..3 {
        statuses.push(http_post_h(srv.addr, "/client-report/v2", &auth, &body).0);
    }
    assert!(statuses.contains(&429), "{statuses:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn static_files_with_content_types() {
    let web = temp_dir("web");
    std::fs::write(web.join("index.html"), "<h1>hi</h1>").unwrap();
    std::fs::write(web.join("app.wasm"), [0, b'a', b's', b'm']).unwrap();
    std::fs::write(web.join("app.js"), "export {}").unwrap();
    std::fs::write(web.join("my file ф.txt"), "spaced").unwrap();
    std::fs::create_dir_all(web.join("netcheck")).unwrap();
    std::fs::write(web.join("netcheck").join("index.html"), "<p>ping</p>").unwrap();
    let srv = TestServer::start(ServerConfig {
        web_root: Some(web),
        ..ServerConfig::default()
    });

    let (status, head, body) = http_get(srv.addr, "/");
    assert_eq!(status, 200);
    assert!(head.contains("content-type: text/html"), "{head}");
    assert_eq!(body, b"<h1>hi</h1>");

    let (status, head, _) = http_get(srv.addr, "/app.wasm");
    assert_eq!(status, 200);
    assert!(head.contains("content-type: application/wasm"), "{head}");

    let (status, head, _) = http_get(srv.addr, "/app.js");
    assert_eq!(status, 200);
    assert!(head.contains("content-type: text/javascript"), "{head}");

    // Spaces and non-ASCII arrive percent-encoded.
    let (status, _, body) = http_get(srv.addr, "/my%20file%20%D1%84.txt");
    assert_eq!(status, 200);
    assert_eq!(body, b"spaced");
    assert_eq!(http_get(srv.addr, "/%2e%2e/Cargo.toml").0, 404);
    assert_eq!(http_get(srv.addr, "/bad%zz.txt").0, 404);

    assert_eq!(http_get(srv.addr, "/missing.js").0, 404);
    assert_eq!(http_get(srv.addr, "/../Cargo.toml").0, 404);

    // A directory URL shows its index page (`/netcheck/`).
    let (status, _, body) = http_get(srv.addr, "/netcheck/");
    assert_eq!((status, body.as_slice()), (200, &b"<p>ping</p>"[..]));
}

/// B3 render-mt: COOP/COEP by default (threads by default); `cross_origin_isolation = false` gives the
/// answers without them.
#[test]
fn cross_origin_isolation_headers_by_default_and_off_when_asked() {
    assert!(ServerConfig::default().cross_origin_isolation);
    assert!(server::config::Config::default().cross_origin_isolation);
    let web = temp_dir("web-coi");
    let assets = temp_dir("assets-coi");
    std::fs::write(web.join("index.html"), "<h1>hi</h1>").unwrap();
    std::fs::write(web.join("app.js"), "export {}").unwrap();
    std::fs::write(assets.join("abc123"), b"blob").unwrap();
    for on in [false, true] {
        let srv = TestServer::start(ServerConfig {
            web_root: Some(web.clone()),
            assets_root: Some(assets.clone()),
            cross_origin_isolation: on,
            ..ServerConfig::default()
        });
        for path in ["/", "/index.html", "/app.js", "/assets/abc123"] {
            let (status, head, _) = http_get(srv.addr, path);
            assert_eq!(status, 200, "{path}");
            for h in [
                "cross-origin-opener-policy: same-origin",
                "cross-origin-embedder-policy: require-corp",
                "cross-origin-resource-policy: same-origin",
            ] {
                assert_eq!(head.contains(h), on, "{path} {h}: {head}");
            }
        }
    }
}

/// The UI v2 script worker gets a policy without network and with `eval` for the skin files; every other file
/// keeps its headers (no policy), with and without cross-origin isolation.
#[test]
fn the_script_worker_has_its_csp_and_nothing_else_does() {
    let web = temp_dir("web-csp");
    std::fs::create_dir_all(web.join("ui2")).unwrap();
    std::fs::write(web.join("index.html"), "<h1>hi</h1>").unwrap();
    std::fs::write(web.join("app.js"), "export {}").unwrap();
    std::fs::write(web.join("worker.js"), "// not the script worker").unwrap();
    std::fs::write(
        web.join("ui2").join("worker.js"),
        "importScripts('api.js');",
    )
    .unwrap();
    std::fs::write(web.join("ui2").join("api.js"), "self.x = 1;").unwrap();
    let csp = "default-src 'none'; script-src 'self' 'unsafe-eval'";
    for on in [false, true] {
        let srv = TestServer::start(ServerConfig {
            web_root: Some(web.clone()),
            cross_origin_isolation: on,
            ..ServerConfig::default()
        });
        let (status, head, body) = http_get(srv.addr, "/ui2/worker.js");
        assert_eq!(
            (status, body.as_slice()),
            (200, &b"importScripts('api.js');"[..])
        );
        assert_eq!(
            header_value(&head, "content-security-policy"),
            Some(csp),
            "{head}"
        );
        assert!(head.contains("content-type: text/javascript"), "{head}");
        assert!(head.contains("cache-control: no-cache"), "{head}");
        assert_eq!(
            head.contains("cross-origin-embedder-policy: require-corp"),
            on,
            "{head}"
        );
        // An encoded path is the same file.
        let (_, head, _) = http_get(srv.addr, "/ui2/worker%2Ejs");
        assert_eq!(
            header_value(&head, "content-security-policy"),
            Some(csp),
            "{head}"
        );
        // A revalidation keeps the policy.
        let etag = header_value(&head, "etag").unwrap().to_string();
        let (status, head, _) = http_get_h(srv.addr, "/ui2/worker.js", &[("If-None-Match", &etag)]);
        assert_eq!(status, 304);
        assert_eq!(
            header_value(&head, "content-security-policy"),
            Some(csp),
            "{head}"
        );
        for path in ["/", "/index.html", "/app.js", "/worker.js", "/ui2/api.js"] {
            let (status, head, _) = http_get(srv.addr, path);
            assert_eq!(status, 200, "{path}");
            assert!(!head.contains("content-security-policy"), "{path}: {head}");
        }
    }
}

#[test]
fn assets_immutable_and_no_traversal() {
    let web = temp_dir("web2");
    let assets = temp_dir("assets");
    std::fs::write(assets.join("abc123"), b"blob").unwrap();
    // Traversal must not reach this file.
    std::fs::write(
        assets.parent().unwrap().join("secret-from-test.txt"),
        b"secret",
    )
    .unwrap();
    let srv = TestServer::start(ServerConfig {
        web_root: Some(web),
        assets_root: Some(assets),
        ..ServerConfig::default()
    });

    let (status, head, body) = http_get(srv.addr, "/assets/abc123");
    assert_eq!(status, 200);
    assert!(
        head.contains("cache-control: public, max-age=31536000, immutable"),
        "{head}"
    );
    assert_eq!(body, b"blob");

    assert_eq!(http_get(srv.addr, "/assets/ffff").0, 404);
    for path in [
        "/assets/..",
        "/assets/../secret-from-test.txt",
        "/assets/..%2fsecret-from-test.txt",
        "/assets/%2e%2e%2fsecret-from-test.txt",
        "/assets/abc123.txt",
        "/assets/zz",
    ] {
        let (status, head, _) = http_get(srv.addr, path);
        assert_eq!(status, 404, "{path}");
        assert!(!head.contains("immutable"), "{path}");
    }
}

fn header_value<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines()
        .find_map(|l| l.strip_prefix(&format!("{name}: ")))
}

#[test]
fn precompressed_sidecars_by_accept_encoding() {
    let web = temp_dir("web-sidecar");
    let assets = temp_dir("assets-sidecar");
    std::fs::write(web.join("app.wasm"), b"wasm original bytes").unwrap();
    std::fs::write(web.join("app.wasm.br"), b"wasm-br").unwrap();
    std::fs::write(web.join("app.wasm.zst"), b"wasm-zst").unwrap();
    std::fs::write(web.join("app.js"), b"js original").unwrap();
    std::fs::write(web.join("app.js.zst"), b"js-zst").unwrap();
    let hash = "ab".repeat(32);
    std::fs::write(assets.join(&hash), b"blob original").unwrap();
    std::fs::write(assets.join(format!("{hash}.br")), b"blob-br").unwrap();
    let srv = TestServer::start(ServerConfig {
        web_root: Some(web),
        assets_root: Some(assets),
        ..ServerConfig::default()
    });

    // br wins over zstd; the type is the original's.
    let ae = |v| [("Accept-Encoding", v)];
    let (status, head, body) = http_get_h(srv.addr, "/app.wasm", &ae("gzip, br, zstd"));
    assert_eq!((status, body.as_slice()), (200, &b"wasm-br"[..]));
    assert_eq!(header_value(&head, "content-encoding"), Some("br"));
    assert_eq!(
        header_value(&head, "content-type"),
        Some("application/wasm")
    );
    assert_eq!(header_value(&head, "content-length"), Some("7"));
    assert_eq!(header_value(&head, "vary"), Some("accept-encoding"));
    assert_eq!(header_value(&head, "cache-control"), Some("no-cache"));
    let br_etag = header_value(&head, "etag").unwrap().to_owned();

    // zstd when br is refused or missing.
    let (_, head, body) = http_get_h(srv.addr, "/app.wasm", &ae("br;q=0, zstd"));
    assert_eq!(body, b"wasm-zst");
    assert_eq!(header_value(&head, "content-encoding"), Some("zstd"));
    let (_, head, body) = http_get_h(srv.addr, "/app.js", &ae("br, zstd"));
    assert_eq!(body, b"js-zst");
    assert_eq!(header_value(&head, "content-encoding"), Some("zstd"));
    assert!(
        header_value(&head, "content-type")
            .unwrap()
            .starts_with("text/javascript")
    );

    // No Accept-Encoding, or only gzip: the original, still with Vary and an ETag.
    for h in [&[][..], &ae("gzip")[..]] {
        let (status, head, body) = http_get_h(srv.addr, "/app.wasm", h);
        assert_eq!(
            (status, body.as_slice()),
            (200, &b"wasm original bytes"[..])
        );
        assert_eq!(header_value(&head, "content-encoding"), None);
        assert_eq!(header_value(&head, "vary"), Some("accept-encoding"));
        let plain_etag = header_value(&head, "etag").unwrap();
        assert_ne!(plain_etag, br_etag);
        assert!(!plain_etag.contains("-br") && !plain_etag.contains("-zst"));
    }

    // 304 on a matching tag; another coding's tag does not match.
    let (status, head, body) = http_get_h(
        srv.addr,
        "/app.wasm",
        &[("Accept-Encoding", "br"), ("If-None-Match", &br_etag)],
    );
    assert_eq!((status, body.len()), (304, 0));
    assert_eq!(header_value(&head, "etag"), Some(br_etag.as_str()));
    assert_eq!(header_value(&head, "cache-control"), Some("no-cache"));
    assert_eq!(header_value(&head, "vary"), Some("accept-encoding"));
    let (status, _, body) = http_get_h(
        srv.addr,
        "/app.wasm",
        &[("Accept-Encoding", "zstd"), ("If-None-Match", &br_etag)],
    );
    assert_eq!((status, body.as_slice()), (200, &b"wasm-zst"[..]));

    // A sidecar next to nothing is not a file of the site.
    std::fs::write(
        srv.shared.config.web_root.as_ref().unwrap().join("x.js.br"),
        b"x",
    )
    .unwrap();
    assert_eq!(http_get_h(srv.addr, "/x.js", &ae("br")).0, 404);

    // Blobs: encoded, and still immutable (no ETag dance).
    let (status, head, body) = http_get_h(srv.addr, &format!("/assets/{hash}"), &ae("br"));
    assert_eq!((status, body.as_slice()), (200, &b"blob-br"[..]));
    assert_eq!(header_value(&head, "content-encoding"), Some("br"));
    assert_eq!(header_value(&head, "vary"), Some("accept-encoding"));
    assert_eq!(
        header_value(&head, "cache-control"),
        Some("public, max-age=31536000, immutable")
    );
    assert_eq!(
        header_value(&head, "content-type"),
        Some("application/octet-stream")
    );
    // zstd client, only a .br sidecar: the original.
    let (_, head, body) = http_get_h(srv.addr, &format!("/assets/{hash}"), &ae("zstd"));
    assert_eq!(body, b"blob original");
    assert_eq!(header_value(&head, "content-encoding"), None);
    assert!(head.contains("immutable"));
    // The sidecar name is not a route of its own.
    assert_eq!(http_get(srv.addr, &format!("/assets/{hash}.br")).0, 404);
}

#[test]
fn manifest_pointer_is_not_cached() {
    let hash = "0123456789abcdef".repeat(4);
    let assets = temp_dir("manifest-ptr");
    std::fs::write(assets.join(&hash), b"manifest-blob").unwrap();
    std::fs::write(assets.join("manifest"), format!("{hash}\n")).unwrap();
    let srv = TestServer::start(ServerConfig {
        assets_root: Some(assets.clone()),
        ..ServerConfig::default()
    });

    let (status, head, body) = http_get(srv.addr, "/assets/manifest");
    assert_eq!(status, 200);
    assert!(head.contains("cache-control: no-cache"), "{head}");
    assert!(!head.contains("immutable"), "{head}");
    assert_eq!(body, hash.as_bytes(), "whitespace is trimmed");
    // The pointed-to blob is still an immutable asset.
    let (status, head, body) = http_get(srv.addr, &format!("/assets/{hash}"));
    assert_eq!((status, body.as_slice()), (200, &b"manifest-blob"[..]));
    assert!(head.contains("immutable"), "{head}");

    // A pointer update is visible at once.
    let next = "f".repeat(64);
    std::fs::write(assets.join("manifest"), &next).unwrap();
    assert_eq!(http_get(srv.addr, "/assets/manifest").2, next.as_bytes());

    // Not a hash: refused rather than relayed.
    std::fs::write(assets.join("manifest"), "../../etc/passwd").unwrap();
    assert_eq!(http_get(srv.addr, "/assets/manifest").0, 404);
    std::fs::write(assets.join("manifest"), hash.to_uppercase()).unwrap();
    assert_eq!(http_get(srv.addr, "/assets/manifest").0, 404);
}

#[test]
fn manifest_pointer_absent_is_404() {
    let assets = temp_dir("manifest-none");
    let srv = TestServer::start(ServerConfig {
        assets_root: Some(assets),
        ..ServerConfig::default()
    });
    assert_eq!(http_get(srv.addr, "/assets/manifest").0, 404);

    let srv = TestServer::start(ServerConfig::default());
    assert_eq!(http_get(srv.addr, "/assets/manifest").0, 404);
}

#[test]
fn healthz_is_503_until_describe_then_200() {
    let srv = TestServer::start(ServerConfig::default());
    let (status, head, body) = http_get(srv.addr, "/healthz");
    assert_eq!(status, 503);
    assert!(head.contains("application/json"), "{head}");
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "starting");
    assert!(json["core_build"].is_null());

    srv.shared.health.set_ready("core-abc");
    let (status, _, body) = http_get(srv.addr, "/healthz");
    assert_eq!(status, 200);
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "ok");
    assert_eq!(json["core_build"], "core-abc");
    assert_eq!(json["protocol"], protocol::PROTOCOL_VERSION);
    assert_eq!(json["build"], env!("CARGO_PKG_VERSION"));
    assert_eq!(json["games"], 0);
    assert_eq!(json["restoring"], 0);
}

#[test]
fn catalog_is_checked_against_its_hash_and_immutable() {
    let state = temp_dir("catalog-state");
    std::fs::create_dir_all(state.join("catalog")).unwrap();
    let body = b"catalog bytes of a game".to_vec();
    let hash = catalog::store(&state.join("catalog"), &body).unwrap();
    let srv = TestServer::start(ServerConfig {
        state_dir: Some(state.clone()),
        ..ServerConfig::default()
    });

    let (status, head, got) = http_get(srv.addr, &format!("/catalog/{hash:016x}"));
    assert_eq!(status, 200);
    assert!(
        head.contains("cache-control: public, max-age=31536000, immutable"),
        "{head}"
    );
    // The page is COEP `require-corp` (threads): the catalog says it is same-origin.
    assert!(
        head.contains("cross-origin-resource-policy: same-origin"),
        "{head}"
    );
    assert_eq!(got, body);

    // Unknown hash, malformed hash, traversal.
    assert_eq!(http_get(srv.addr, "/catalog/0000000000000001").0, 404);
    assert_eq!(http_get(srv.addr, "/catalog/ff").0, 404);
    assert_eq!(http_get(srv.addr, &format!("/catalog/{hash:016X}")).0, 404);
    assert_eq!(http_get(srv.addr, "/catalog/..%2f..%2fidentities").0, 404);

    // A file whose content does not match its name is never served.
    std::fs::write(
        state.join("catalog").join(format!("{hash:016x}.bin")),
        b"tampered",
    )
    .unwrap();
    assert_eq!(http_get(srv.addr, &format!("/catalog/{hash:016x}")).0, 404);

    // Without a state_dir there are no catalogs.
    let bare = TestServer::start(ServerConfig::default());
    assert_eq!(http_get(bare.addr, &format!("/catalog/{hash:016x}")).0, 404);
}

type Ws = WebSocket<MaybeTlsStream<TcpStream>>;

fn netcheck(srv: &TestServer) -> Result<Ws, tungstenite::Error> {
    let (mut ws, _) = tungstenite::connect(format!("ws://{}/netcheck/ws", srv.addr))?;
    if let MaybeTlsStream::Plain(s) = ws.get_mut() {
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    }
    Ok(ws)
}

fn assert_closed(ws: &mut Ws) {
    loop {
        match ws.read() {
            Ok(Message::Close(_)) | Err(_) => return,
            Ok(_) => {}
        }
    }
}

#[test]
fn netcheck_echoes_and_allows_one_connection_per_address() {
    let srv = TestServer::start(ServerConfig::default());
    let mut a = netcheck(&srv).unwrap();
    a.send(Message::text("t=123")).unwrap();
    assert_eq!(a.read().unwrap(), Message::text("t=123"));

    // The same address again: refused at the upgrade.
    assert!(netcheck(&srv).is_err());

    // After the first one is gone, a new one is accepted (the guard is released).
    a.close(None).unwrap();
    assert_closed(&mut a);
    let mut accepted = None;
    for _ in 0..50 {
        if let Ok(ws) = netcheck(&srv) {
            accepted = Some(ws);
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(accepted.is_some(), "address was not released");
}

#[test]
fn netcheck_closes_on_too_many_messages_and_after_its_time() {
    let srv = TestServer::start(ServerConfig {
        netcheck_max_per_second: 2,
        netcheck_max_duration: Duration::from_millis(600),
        ..ServerConfig::default()
    });
    let mut ws = netcheck(&srv).unwrap();
    for i in 0..3 {
        // Ignore a send error: the server may already have closed.
        let _ = ws.send(Message::text(format!("{i}")));
    }
    // Two echoes at most, then the policy close.
    let mut echoes = 0;
    loop {
        match ws.read() {
            Ok(Message::Text(_)) => echoes += 1,
            Ok(Message::Close(_)) | Err(_) => break,
            Ok(_) => {}
        }
    }
    assert!(echoes <= 2, "{echoes} echoes");

    // The time limit.
    std::thread::sleep(Duration::from_millis(100));
    let mut ws = loop {
        if let Ok(ws) = netcheck(&srv) {
            break ws;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_closed(&mut ws);
}

#[test]
fn admin_api_needs_the_token() {
    let srv = TestServer::start(ServerConfig {
        admin_token: Some(Secret::new("admin-secret")),
        ..ServerConfig::default()
    });
    let a = srv.admin_addr;
    assert_eq!(http_get(a, "/admin/metrics").0, 401);
    assert_eq!(
        http_get_h(a, "/admin/metrics", &[("X-Admin-Token", "wrong")]).0,
        401
    );
    // Unknown paths do not leak their existence without a token.
    assert_eq!(http_get(a, "/admin/nothing").0, 401);
    assert_eq!(
        http_get_h(a, "/admin/nothing", &[("X-Admin-Token", "admin-secret")]).0,
        404
    );

    let (status, _, body) = http_get_h(a, "/admin/metrics", &[("X-Admin-Token", "admin-secret")]);
    assert_eq!(status, 200);
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["schema"], 1);
    assert_eq!(json["protocol"], protocol::PROTOCOL_VERSION);
    assert_eq!(json["server"]["sessions"]["open"], 0);
    assert!(json["server"]["sessions"]["login_failed_total"].is_u64());

    let (status, _, body) = http_get_h(a, "/admin/games", &[("X-Admin-Token", "admin-secret")]);
    assert_eq!(status, 200);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()["games"],
        serde_json::json!([])
    );
}

#[test]
fn admin_api_without_a_configured_token_is_unavailable() {
    let srv = TestServer::start(ServerConfig::default());
    assert_eq!(http_get(srv.admin_addr, "/admin/metrics").0, 503);
}

#[test]
fn admin_listener_refuses_non_loopback() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let l = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
        let shared = Shared::new(ServerConfig::default()).unwrap();
        assert!(server::serve_admin(l, shared).await.is_err());
    });
}

/// T32: `GET /mods/{hash}` serves the pack of an installed mod by the SHA-256 of the file, immutable, and nothing else.
#[test]
fn mod_pack_is_served_by_hash() {
    let mods = temp_dir("mods");
    let dir = mods.join("hello");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("mod.toml"),
        "[mod]\nid=\"hello\"\nname=\"Hello\"\nversion=\"1.0.0\"\napi=\"^1\"\nlicense=\"MIT\"\nsync=\"gameplay\"\nparts=[\"data\"]\n",
    )
    .unwrap();
    let pack = mod_assets::Pack::build(
        "hello".parse().unwrap(),
        vec![],
        vec![mod_assets::NewFile {
            id: "data/blob".into(),
            kind: mod_assets::AssetKind::Raw,
            bytes: b"payload".to_vec(),
        }],
    )
    .unwrap();
    let bytes = pack.to_bytes();
    std::fs::write(dir.join("hello.pack"), &bytes).unwrap();
    // Something outside the store that traversal must not reach.
    std::fs::write(mods.join("secret.txt"), b"secret").unwrap();
    let hash = pack.hash().to_string();

    let srv = TestServer::start(ServerConfig {
        mods_dir: Some(mods),
        ..ServerConfig::default()
    });
    let (status, head, body) = http_get(srv.addr, &format!("/mods/{hash}"));
    assert_eq!(status, 200);
    assert!(
        head.contains("cache-control: public, max-age=31536000, immutable"),
        "{head}"
    );
    assert_eq!(
        header_value(&head, "content-type"),
        Some("application/octet-stream")
    );
    assert_eq!(body, bytes);
    // The client's own check passes on what was served.
    mod_assets::Pack::parse_expecting(&body, &pack.hash()).unwrap();

    for path in [
        format!("/mods/{}", "0".repeat(64)),
        "/mods/hello".to_string(),
        "/mods/hello.pack".to_string(),
        "/mods/..".to_string(),
        "/mods/../secret.txt".to_string(),
        "/mods/..%2fsecret.txt".to_string(),
        format!("/mods/{}x", &hash[..63]),
        format!("/mods/{}0", hash),
        "/mods/".to_string(),
    ] {
        let (status, head, _) = http_get(srv.addr, &path);
        assert_eq!(status, 404, "{path}");
        assert!(!head.contains("immutable"), "{path}");
    }
}

/// A server without mods answers 404 on the route (no mods directory at all).
#[test]
fn mod_pack_route_without_mods_is_404() {
    let srv = TestServer::start(ServerConfig::default());
    assert_eq!(
        http_get(srv.addr, &format!("/mods/{}", "ab".repeat(32))).0,
        404
    );
}
