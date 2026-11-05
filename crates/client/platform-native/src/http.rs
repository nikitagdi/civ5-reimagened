//! Blocking HTTP/1.1 GET over `std::net::TcpStream`, one worker thread per request.
//! Plain `http://` only (no TLS); `Connection: close`, so the body ends at EOF unless
//! `Content-Length` or chunked framing says otherwise.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::Duration;

use platform::{HttpClient, HttpError, HttpOp, HttpResponse};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Per read/write, not per request: large blobs may take longer overall.
const IO_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_HEADER_LINE: u64 = 16 * 1024;
const MAX_HEADERS: usize = 100;
/// Matches the server's served-file cap.
const MAX_BODY: u64 = 256 * 1024 * 1024;

#[derive(Debug, Default, Clone, Copy)]
pub struct StdHttp;

impl HttpClient for StdHttp {
    fn get(&self, url: &str) -> Box<dyn HttpOp> {
        let (tx, rx) = mpsc::channel();
        let url = url.to_owned();
        let spawned = thread::Builder::new()
            .name("civ5-http".into())
            .spawn(move || {
                // The receiver may be gone (request dropped); nothing to report then.
                let _ = tx.send(fetch(&url));
            });
        if let Err(e) = spawned {
            // The channel closes with the sender; `poll` reports it.
            tracing::error!("failed to start HTTP thread: {e}");
        }
        Box::new(ThreadOp {
            rx,
            finished: false,
        })
    }
}

struct ThreadOp {
    rx: Receiver<Result<HttpResponse, HttpError>>,
    finished: bool,
}

impl HttpOp for ThreadOp {
    fn poll(&mut self) -> Option<Result<HttpResponse, HttpError>> {
        if self.finished {
            return None;
        }
        match self.rx.try_recv() {
            Ok(r) => {
                self.finished = true;
                Some(r)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.finished = true;
                Some(Err(HttpError::Network("HTTP thread terminated".into())))
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ParsedUrl {
    host: String,
    port: u16,
    path: String,
}

fn parse_url(url: &str) -> Result<ParsedUrl, HttpError> {
    let bad = |msg: &str| HttpError::InvalidUrl(format!("{msg}: {url}"));
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| bad("only http:// is supported"))?;
    let (authority, path) = match rest.find(['/', '?']) {
        Some(i) if rest.as_bytes()[i] == b'/' => (&rest[..i], rest[i..].to_owned()),
        Some(i) => (&rest[..i], format!("/{}", &rest[i..])),
        None => (rest, "/".to_owned()),
    };
    if authority.is_empty() || authority.contains('@') {
        return Err(bad("bad host"));
    }
    // Header injection guard: the path goes into the request line verbatim.
    if path.bytes().any(|b| b <= b' ' || b == 0x7f) {
        return Err(bad("path contains whitespace or control characters"));
    }
    let (host, port) = match authority.rsplit_once(':') {
        // `[::1]` has colons but no port part after the bracket.
        Some((h, p)) if !p.contains(']') => (h, p.parse::<u16>().map_err(|_| bad("bad port"))?),
        _ => (authority, 80),
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.is_empty() {
        return Err(bad("bad host"));
    }
    Ok(ParsedUrl {
        host: host.to_owned(),
        port,
        path,
    })
}

fn net_err(what: &str, e: impl std::fmt::Display) -> HttpError {
    HttpError::Network(format!("{what}: {e}"))
}

fn fetch(url: &str) -> Result<HttpResponse, HttpError> {
    let u = parse_url(url)?;
    let addrs = (u.host.as_str(), u.port)
        .to_socket_addrs()
        .map_err(|e| net_err(&format!("resolving {}", u.host), e))?;
    let mut last = None;
    let mut stream = None;
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => last = Some(e),
        }
    }
    let mut stream = stream.ok_or_else(|| match last {
        Some(e) => net_err(&format!("connecting to {}:{}", u.host, u.port), e),
        None => HttpError::Network(format!("{} resolved to no addresses", u.host)),
    })?;
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(IO_TIMEOUT)))
        .map_err(|e| net_err("configuring socket", e))?;
    let _ = stream.set_nodelay(true);

    let host = if u.host.contains(':') {
        format!("[{}]", u.host)
    } else {
        u.host.clone()
    };
    let host_header = if u.port == 80 {
        host
    } else {
        format!("{host}:{}", u.port)
    };
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {host_header}\r\nUser-Agent: civ5-reimagened\r\nAccept: */*\r\nConnection: close\r\n\r\n",
        u.path
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| net_err("sending request", e))?;
    read_response(BufReader::new(stream)).map_err(|e| net_err("reading response", e))
}

fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

/// Line without its CRLF; `None` at EOF.
fn read_line(r: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut buf = Vec::new();
    // `take` bounds a hostile server that never sends a newline.
    let n = r
        .by_ref()
        .take(MAX_HEADER_LINE)
        .read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Ok(None);
    }
    if buf.last() != Some(&b'\n') {
        return Err(invalid("header line too long or truncated"));
    }
    while matches!(buf.last(), Some(b'\n' | b'\r')) {
        buf.pop();
    }
    String::from_utf8(buf)
        .map(Some)
        .map_err(|_| invalid("non-UTF-8 header"))
}

fn read_response(mut r: impl BufRead) -> io::Result<HttpResponse> {
    let status_line = read_line(&mut r)?.ok_or_else(|| invalid("empty response"))?;
    let mut parts = status_line.splitn(3, ' ');
    let version = parts.next().unwrap_or_default();
    if !version.starts_with("HTTP/1.") {
        return Err(invalid(format!("bad status line {status_line:?}")));
    }
    let status: u16 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| invalid(format!("bad status line {status_line:?}")))?;

    let mut content_length: Option<u64> = None;
    let mut chunked = false;
    for _ in 0..=MAX_HEADERS {
        let line = read_line(&mut r)?.ok_or_else(|| invalid("truncated headers"))?;
        if line.is_empty() {
            return finish(r, status, content_length, chunked);
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(invalid(format!("bad header {line:?}")));
        };
        let value = value.trim();
        if name.eq_ignore_ascii_case("content-length") {
            content_length = Some(value.parse().map_err(|_| invalid("bad Content-Length"))?);
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            chunked = value.to_ascii_lowercase().contains("chunked");
        }
    }
    Err(invalid("too many headers"))
}

fn finish(
    mut r: impl BufRead,
    status: u16,
    content_length: Option<u64>,
    chunked: bool,
) -> io::Result<HttpResponse> {
    // 1xx/204/304 carry no body whatever the headers say.
    let body = if status < 200 || status == 204 || status == 304 {
        Vec::new()
    } else if chunked {
        read_chunked(&mut r)?
    } else if let Some(len) = content_length {
        if len > MAX_BODY {
            return Err(invalid(format!("body of {len} bytes exceeds the limit")));
        }
        let mut body = Vec::new();
        r.take(len).read_to_end(&mut body)?;
        if body.len() as u64 != len {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        body
    } else {
        let mut body = Vec::new();
        r.take(MAX_BODY + 1).read_to_end(&mut body)?;
        if body.len() as u64 > MAX_BODY {
            return Err(invalid("body exceeds the limit"));
        }
        body
    };
    Ok(HttpResponse { status, body })
}

fn read_chunked(r: &mut impl BufRead) -> io::Result<Vec<u8>> {
    let mut body = Vec::new();
    loop {
        let line = read_line(r)?.ok_or(io::ErrorKind::UnexpectedEof)?;
        // Chunk extensions follow `;`.
        let size_text = line.split(';').next().unwrap_or_default().trim();
        let size = u64::from_str_radix(size_text, 16)
            .map_err(|_| invalid(format!("bad chunk size {size_text:?}")))?;
        if size == 0 {
            // Trailers until the empty line.
            while !read_line(r)?
                .ok_or(io::ErrorKind::UnexpectedEof)?
                .is_empty()
            {}
            return Ok(body);
        }
        if body.len() as u64 + size > MAX_BODY {
            return Err(invalid("body exceeds the limit"));
        }
        let start = body.len();
        r.by_ref().take(size).read_to_end(&mut body)?;
        if (body.len() - start) as u64 != size {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let end = read_line(r)?.ok_or(io::ErrorKind::UnexpectedEof)?;
        if !end.is_empty() {
            return Err(invalid("missing CRLF after chunk"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::time::Instant;

    fn parse(raw: &[u8]) -> io::Result<HttpResponse> {
        read_response(io::Cursor::new(raw.to_vec()))
    }

    #[test]
    fn urls() {
        assert_eq!(
            parse_url("http://127.0.0.1:8080/assets/ab").unwrap(),
            ParsedUrl {
                host: "127.0.0.1".into(),
                port: 8080,
                path: "/assets/ab".into()
            }
        );
        let u = parse_url("http://example.test").unwrap();
        assert_eq!((u.port, u.path.as_str()), (80, "/"));
        assert_eq!(parse_url("http://h?x=1").unwrap().path, "/?x=1");
        assert_eq!(parse_url("http://[::1]:9/p").unwrap().host, "::1");
        assert_eq!(parse_url("http://[::1]/p").unwrap().port, 80);
        assert!(parse_url("https://h/").is_err());
        assert!(parse_url("http:///x").is_err());
        assert!(parse_url("http://h:99999/").is_err());
        assert!(parse_url("http://h/a b").is_err());
        assert!(parse_url("http://h/a\r\nX: y").is_err());
    }

    #[test]
    fn content_length_body() {
        let r = parse(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nX: y\r\n\r\nabcEXTRA").unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.body, b"abc");
    }

    #[test]
    fn body_until_eof_and_not_found() {
        let r = parse(b"HTTP/1.0 404 Not Found\r\n\r\nnope").unwrap();
        assert_eq!((r.status, r.body.as_slice()), (404, &b"nope"[..]));
    }

    #[test]
    fn chunked_body_with_extension_and_trailer() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: Chunked\r\n\r\n3;ext=1\r\nabc\r\n2\r\nde\r\n0\r\nTrailer: t\r\n\r\n";
        assert_eq!(parse(raw).unwrap().body, b"abcde");
    }

    #[test]
    fn no_body_statuses() {
        let r = parse(b"HTTP/1.1 304 Not Modified\r\nContent-Length: 5\r\n\r\n").unwrap();
        assert!(r.body.is_empty());
    }

    #[test]
    fn malformed_and_truncated_replies_are_errors() {
        assert!(parse(b"").is_err());
        assert!(parse(b"garbage\r\n\r\n").is_err());
        assert!(parse(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc").is_err());
        assert!(parse(b"HTTP/1.1 200 OK\r\nContent-Length: x\r\n\r\n").is_err());
        assert!(parse(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n").is_err());
        assert!(parse(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nab").is_err());
        assert!(parse(b"HTTP/1.1 200 OK\r\nHost").is_err());
    }

    fn poll_until(op: &mut dyn HttpOp) -> Result<HttpResponse, HttpError> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Some(r) = op.poll() {
                return r;
            }
            thread::sleep(Duration::from_millis(2));
        }
        panic!("request did not complete");
    }

    #[test]
    fn get_over_loopback() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            let mut request = String::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).expect("read");
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                request.push_str(&line);
            }
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nhi\r\n0\r\n\r\n",
                )
                .expect("write");
            request
        });

        let mut op = StdHttp.get(&format!("http://{addr}/assets/manifest"));
        let resp = poll_until(op.as_mut()).expect("response");
        assert_eq!((resp.status, resp.body.as_slice()), (200, &b"hi"[..]));
        assert!(op.poll().is_none(), "result is delivered once");
        let request = server.join().expect("server");
        assert!(
            request.starts_with("GET /assets/manifest HTTP/1.1\r\n"),
            "{request}"
        );
        assert!(request.contains(&format!("Host: {addr}")), "{request}");
    }

    #[test]
    fn connection_refused_is_network_error() {
        let port = TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .expect("port")
            .port();
        let mut op = StdHttp.get(&format!("http://127.0.0.1:{port}/"));
        assert!(matches!(
            poll_until(op.as_mut()),
            Err(HttpError::Network(_))
        ));
        let mut op = StdHttp.get("ftp://x/");
        assert!(matches!(
            poll_until(op.as_mut()),
            Err(HttpError::InvalidUrl(_))
        ));
    }
}
