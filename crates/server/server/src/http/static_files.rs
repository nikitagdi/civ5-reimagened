//! Web client and asset files. File I/O uses `std::fs` in `spawn_blocking`.
//! Behind Caddy (the deploy) Caddy serves these itself (GS-D15); the gateway keeps the routes for
//! development, tests and a server without a reverse proxy.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};

use crate::Shared;

const MAX_SERVED_FILE: u64 = 256 * 1024 * 1024;

/// What a route serves: web files are revalidated on every load, blobs never change.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Web,
    Blob,
}

/// Content codings the client accepts, from `Accept-Encoding`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Accept {
    br: bool,
    zstd: bool,
}

impl Accept {
    fn from_headers(headers: &HeaderMap) -> Self {
        let joined = headers
            .get_all(header::ACCEPT_ENCODING)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .collect::<Vec<_>>()
            .join(",");
        Self::parse(&joined)
    }

    /// Tokens with `q=0` are refused; `*` stands for every coding not named.
    fn parse(value: &str) -> Self {
        let (mut br, mut zstd) = (None, None);
        let mut star = None;
        for item in value.split(',') {
            let mut parts = item.split(';');
            let name = parts.next().unwrap_or("").trim().to_ascii_lowercase();
            let ok = parts
                .filter_map(|p| {
                    p.trim()
                        .to_ascii_lowercase()
                        .strip_prefix("q=")
                        .map(str::to_owned)
                })
                .next_back()
                .is_none_or(|q| q.trim().parse::<f32>().is_ok_and(|q| q > 0.0));
            match name.as_str() {
                "br" => br = Some(ok),
                "zstd" => zstd = Some(ok),
                "*" => star = Some(ok),
                _ => {}
            }
        }
        Self {
            br: br.or(star).unwrap_or(false),
            zstd: zstd.or(star).unwrap_or(false),
        }
    }

    /// Sidecars to try, best first: (file suffix, `Content-Encoding`).
    fn candidates(self) -> impl Iterator<Item = (&'static str, &'static str)> {
        [(self.br, ("br", "br")), (self.zstd, ("zst", "zstd"))]
            .into_iter()
            .filter(|(on, _)| *on)
            .map(|(_, c)| c)
    }
}

pub async fn index(State(shared): State<Arc<Shared>>, headers: HeaderMap) -> Response {
    let resp = serve_file(
        shared.config.web_root.clone(),
        "index.html".into(),
        &headers,
        Kind::Web,
    )
    .await;
    isolate(&shared, resp)
}

/// COOP `same-origin` and COEP `require-corp` (with `cross_origin_isolation`, B3 render-mt): the page
/// is `crossOriginIsolated` (`SharedArrayBuffer`, the threaded build). Every subresource of the page is
/// same-origin, so `require-corp` blocks nothing; CORP `same-origin` says so explicitly.
pub(super) fn isolate(shared: &Shared, mut resp: Response) -> Response {
    if shared.config.cross_origin_isolation {
        let h = resp.headers_mut();
        h.insert(
            HeaderName::from_static("cross-origin-opener-policy"),
            HeaderValue::from_static("same-origin"),
        );
        h.insert(
            HeaderName::from_static("cross-origin-embedder-policy"),
            HeaderValue::from_static("require-corp"),
        );
        h.insert(
            HeaderName::from_static("cross-origin-resource-policy"),
            HeaderValue::from_static("same-origin"),
        );
    }
    resp
}

pub async fn static_file(
    State(shared): State<Arc<Shared>>,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let rel = uri.path().trim_start_matches('/');
    if rel.is_empty() {
        let resp = serve_file(
            shared.config.web_root.clone(),
            "index.html".into(),
            &headers,
            Kind::Web,
        )
        .await;
        return isolate(&shared, resp);
    }
    // Decode first: `%2e%2e`, `%5c`, `%3a` must not bypass safe_relative.
    let Some(decoded) = percent_decode(rel) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(mut rel) = safe_relative(&decoded) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // `/netcheck/` and other directory URLs show their index page.
    if decoded.ends_with('/') {
        rel.push("index.html");
    }
    let worker = rel == Path::new(SCRIPT_WORKER);
    let resp = serve_file(shared.config.web_root.clone(), rel, &headers, Kind::Web).await;
    let mut resp = isolate(&shared, resp);
    if worker {
        resp.headers_mut().insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(SCRIPT_WORKER_CSP),
        );
    }
    resp
}

/// The loader of the UI v2 script workers: skin and mod scripts run in it.
const SCRIPT_WORKER: &str = "ui2/worker.js";

/// The policy of a script worker (a dedicated worker takes the policy of its own script's response): no network
/// at all (`default-src 'none'` covers `connect-src`), scripts only from the page's origin (the loader's
/// `importScripts("api.js")`), and `'unsafe-eval'` for the skin files, which arrive as text in the worker's first
/// message and run through `new Function`. The loader also takes the network globals away; the policy holds if
/// that is ever bypassed. The page itself gets no policy here.
const SCRIPT_WORKER_CSP: &str = "default-src 'none'; script-src 'self' 'unsafe-eval'";

/// `None` on malformed or non-UTF-8 input.
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let hi = char::from(hex[0]).to_digit(16)?;
            let lo = char::from(hex[1]).to_digit(16)?;
            // Both digits < 16: fits in u8.
            out.push(u8::try_from(hi * 16 + lo).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

pub async fn asset(
    State(shared): State<Arc<Shared>>,
    UrlPath(hash): UrlPath<String>,
    headers: HeaderMap,
) -> Response {
    // Hex only: blocks path traversal.
    if hash.is_empty() || hash.len() > 128 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let resp = serve_file(
        shared.config.assets_root.clone(),
        PathBuf::from(hash),
        &headers,
        Kind::Blob,
    )
    .await;
    isolate(&shared, resp)
}

/// Hash of the current manifest blob, from `<assets-root>/manifest`. Mutable, unlike blobs.
pub async fn manifest_pointer(State(shared): State<Arc<Shared>>) -> Response {
    let Some(root) = shared.config.assets_root.clone() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let read = tokio::task::spawn_blocking(move || {
        use std::io::Read;
        let mut buf = Vec::new();
        // A pointer is 64 digits; the cap keeps a wrong file from being slurped.
        std::fs::File::open(root.join("manifest"))?
            .take(1024)
            .read_to_end(&mut buf)?;
        std::io::Result::Ok(buf)
    })
    .await;
    let Ok(Ok(bytes)) = read else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(hash) = parse_pointer(&bytes) else {
        tracing::warn!("assets-root/manifest does not hold a 64-digit hex hash");
        return StatusCode::NOT_FOUND.into_response();
    };
    (
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        hash,
    )
        .into_response()
}

/// Trimmed pointer text if it is 64 lowercase hex digits (a `ContentHash`).
fn parse_pointer(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?.trim();
    (text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')))
        .then(|| text.to_owned())
}

/// Rejects `..`, absolute parts, and Windows separators.
fn safe_relative(url_path: &str) -> Option<PathBuf> {
    if url_path.contains(['\\', '\0', ':']) {
        return None;
    }
    let path = Path::new(url_path);
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Normal(part) => out.push(part),
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// A file ready to send: the original or one of its precompressed sidecars.
struct Loaded {
    /// `None` answers `If-None-Match` with 304.
    body: Option<Vec<u8>>,
    content_type: &'static str,
    encoding: Option<&'static str>,
    etag: String,
}

async fn serve_file(
    root: Option<PathBuf>,
    rel: PathBuf,
    headers: &HeaderMap,
    kind: Kind,
) -> Response {
    let Some(root) = root else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let full = root.join(&rel);
    let accept = Accept::from_headers(headers);
    let if_none_match = (kind == Kind::Web)
        .then(|| headers.get(header::IF_NONE_MATCH))
        .flatten()
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let read =
        tokio::task::spawn_blocking(move || load(&full, accept, if_none_match.as_deref())).await;
    let loaded = match read {
        Ok(Ok(l)) => l,
        Ok(Err(err)) if err.kind() == std::io::ErrorKind::FileTooLarge => {
            tracing::warn!("file larger than {MAX_SERVED_FILE} bytes is not served");
            return StatusCode::PAYLOAD_TOO_LARGE.into_response();
        }
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    let mut resp = match loaded.body {
        Some(body) => ([(header::CONTENT_TYPE, loaded.content_type)], body).into_response(),
        None => StatusCode::NOT_MODIFIED.into_response(),
    };
    let h = resp.headers_mut();
    h.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    if let Some(enc) = loaded.encoding {
        h.insert(header::CONTENT_ENCODING, HeaderValue::from_static(enc));
    }
    match kind {
        Kind::Blob => {
            h.insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=31536000, immutable"),
            );
        }
        Kind::Web => {
            h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
            if let Ok(v) = HeaderValue::from_str(&loaded.etag) {
                h.insert(header::ETAG, v);
            }
        }
    }
    resp
}

/// Picks the best sidecar the client accepts, stats it for the ETag and reads it unless
/// `if_none_match` already names it. The original must exist; a lone sidecar is not served.
fn load(full: &Path, accept: Accept, if_none_match: Option<&str>) -> std::io::Result<Loaded> {
    if !std::fs::metadata(full)?.is_file() {
        return Err(std::io::ErrorKind::NotFound.into());
    }
    let mut chosen = (full.to_path_buf(), None, "");
    for (suffix, coding) in accept.candidates() {
        let mut name = full.as_os_str().to_owned();
        name.push(".");
        name.push(suffix);
        let side = PathBuf::from(name);
        if std::fs::metadata(&side).is_ok_and(|m| m.is_file()) {
            chosen = (side, Some(coding), suffix);
            break;
        }
    }
    let (path, encoding, suffix) = chosen;
    let meta = std::fs::metadata(&path)?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    let mut etag = format!("\"{:x}-{mtime:x}", meta.len());
    if !suffix.is_empty() {
        etag.push('-');
        etag.push_str(suffix);
    }
    etag.push('"');
    let body = if if_none_match.is_some_and(|inm| etag_matches(inm, &etag)) {
        None
    } else {
        Some(read_capped(&path)?)
    };
    Ok(Loaded {
        body,
        content_type: content_type(full),
        encoding,
        etag,
    })
}

/// `If-None-Match` weak comparison: `*` or any listed tag equal to `etag`, `W/` ignored.
fn etag_matches(if_none_match: &str, etag: &str) -> bool {
    if_none_match
        .split(',')
        .map(str::trim)
        .any(|t| t == "*" || t.strip_prefix("W/").unwrap_or(t) == etag)
}

fn read_capped(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let file = std::fs::File::open(path)?;
    if file.metadata()?.len() > MAX_SERVED_FILE {
        return Err(std::io::ErrorKind::FileTooLarge.into());
    }
    // `take` catches files grown after the size check.
    let mut buf = Vec::new();
    file.take(MAX_SERVED_FILE + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > MAX_SERVED_FILE {
        return Err(std::io::ErrorKind::FileTooLarge.into());
    }
    Ok(buf)
}

fn content_type(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("webmanifest") => "application/manifest+json",
        Some("txt") => "text/plain; charset=utf-8",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_web_app_manifest_has_its_own_type() {
        assert_eq!(
            content_type(Path::new("manifest.webmanifest")),
            "application/manifest+json"
        );
        assert_eq!(content_type(Path::new("icons/icon-192.png")), "image/png");
    }

    #[test]
    fn safe_paths() {
        assert_eq!(safe_relative("a/b.js"), Some(PathBuf::from("a/b.js")));
        assert_eq!(safe_relative("../x"), None);
        assert_eq!(safe_relative("a/../../x"), None);
        assert_eq!(safe_relative("a\\b"), None);
        assert_eq!(safe_relative("C:/x"), None);
        assert_eq!(safe_relative(""), None);
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("a%20b.js").as_deref(), Some("a b.js"));
        assert_eq!(percent_decode("%D1%84.txt").as_deref(), Some("ф.txt"));
        assert_eq!(percent_decode("plain").as_deref(), Some("plain"));
        assert_eq!(percent_decode("bad%zz"), None);
        assert_eq!(percent_decode("cut%2"), None);
        assert_eq!(percent_decode("%FF"), None);
        // safe_relative must reject these after decoding.
        for evil in ["%2e%2e/x", "a%5Cb", "C%3A/x", "a%00b", "a/%2E%2E/%2e%2e/x"] {
            let decoded = percent_decode(evil).unwrap_or_default();
            assert_eq!(safe_relative(&decoded), None, "{evil}");
        }
    }

    #[test]
    fn read_capped_reads_normal_file() {
        let dir = std::env::temp_dir().join(format!("civ5-gw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a b.txt");
        std::fs::write(&file, b"hi").unwrap();
        assert_eq!(read_capped(&file).unwrap(), b"hi");
        assert!(read_capped(&dir.join("missing")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pointer_parsing() {
        let h = "ab".repeat(32);
        assert_eq!(
            parse_pointer(format!(" {h}\r\n").as_bytes()),
            Some(h.clone())
        );
        assert_eq!(parse_pointer(h.to_uppercase().as_bytes()), None);
        assert_eq!(parse_pointer(b"abc"), None);
        assert_eq!(parse_pointer(format!("{h}00").as_bytes()), None);
        assert_eq!(parse_pointer(b"\xff"), None);
    }

    fn accept(br: bool, zstd: bool) -> Accept {
        Accept { br, zstd }
    }

    #[test]
    fn accept_encoding_parsing() {
        assert_eq!(Accept::parse(""), accept(false, false));
        assert_eq!(Accept::parse("gzip, deflate"), accept(false, false));
        assert_eq!(Accept::parse("gzip, deflate, br, zstd"), accept(true, true));
        assert_eq!(Accept::parse("BR;q=0.8, zstd;q=0.5"), accept(true, true));
        assert_eq!(Accept::parse("br;q=0, zstd"), accept(false, true));
        assert_eq!(Accept::parse("br; q=0.0, zstd;q=0"), accept(false, false));
        assert_eq!(Accept::parse("*"), accept(true, true));
        assert_eq!(Accept::parse("*, br;q=0"), accept(false, true));
        assert_eq!(Accept::parse("*;q=0, zstd"), accept(false, true));
        assert_eq!(Accept::parse("identity"), accept(false, false));
    }

    #[test]
    fn sidecar_preference_is_br_then_zstd() {
        let c = |a: Accept| a.candidates().collect::<Vec<_>>();
        assert_eq!(c(accept(true, true)), [("br", "br"), ("zst", "zstd")]);
        assert_eq!(c(accept(false, true)), [("zst", "zstd")]);
        assert!(c(accept(false, false)).is_empty());
    }

    #[test]
    fn load_picks_sidecar_and_etag_differs_per_coding() {
        let dir = std::env::temp_dir().join(format!("civ5-gw-load-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.wasm");
        std::fs::write(&f, b"original").unwrap();
        std::fs::write(dir.join("a.wasm.br"), b"br").unwrap();
        std::fs::write(dir.join("a.wasm.zst"), b"zst").unwrap();
        // A sidecar without its original is never served.
        std::fs::write(dir.join("b.js.br"), b"br").unwrap();

        let plain = load(&f, accept(false, false), None).unwrap();
        let br = load(&f, accept(true, true), None).unwrap();
        let zst = load(&f, accept(false, true), None).unwrap();
        assert_eq!(plain.body.as_deref(), Some(&b"original"[..]));
        assert_eq!(
            (plain.encoding, br.encoding, zst.encoding),
            (None, Some("br"), Some("zstd"))
        );
        assert_eq!(br.body.as_deref(), Some(&b"br"[..]));
        assert_eq!(zst.body.as_deref(), Some(&b"zst"[..]));
        assert_eq!(plain.content_type, "application/wasm");
        assert_eq!(br.content_type, "application/wasm");
        assert!(br.etag.ends_with("-br\"") && zst.etag.ends_with("-zst\""));
        assert!(!plain.etag.contains("-br") && plain.etag != br.etag);

        let hit = load(&f, accept(true, true), Some(&format!("\"x\", {}", br.etag))).unwrap();
        assert!(hit.body.is_none());
        let miss = load(&f, accept(true, true), Some(&plain.etag)).unwrap();
        assert!(miss.body.is_some());
        assert!(load(&dir.join("b.js"), accept(true, true), None).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn etag_comparison() {
        assert!(etag_matches("\"a\"", "\"a\""));
        assert!(etag_matches("W/\"a\", \"b\"", "\"a\""));
        assert!(etag_matches("*", "\"a\""));
        assert!(!etag_matches("\"b\"", "\"a\""));
    }

    #[test]
    fn content_types() {
        assert_eq!(content_type(Path::new("a.wasm")), "application/wasm");
        assert!(content_type(Path::new("a.JS")).starts_with("text/javascript"));
        assert_eq!(content_type(Path::new("abc")), "application/octet-stream");
    }
}
