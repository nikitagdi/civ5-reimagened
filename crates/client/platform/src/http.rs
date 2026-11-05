//! HTTP GET: `fetch()` on the web, a worker thread natively (architecture §6.5).

use std::fmt;

/// Pending request. Yields its result once, then `None`.
pub trait HttpOp {
    fn poll(&mut self) -> Option<Result<HttpResponse, HttpError>>;
}

/// Plain GET client. `url` is absolute (`http://host:port/path`) unless the implementation
/// documents otherwise.
pub trait HttpClient {
    fn get(&self, url: &str) -> Box<dyn HttpOp>;

    /// Drops any cached copy of `url` (its body failed verification). Default: no-op.
    fn forget(&self, _url: &str) {}

    /// The asset manifest is loaded: `live` holds the hex hash of every blob it names, and
    /// `files`/`bytes` what this device will fetch from it. Default: no-op.
    fn manifest_ready(&self, _live: std::collections::HashSet<String>, _files: u64, _bytes: u64) {}
}

/// Any status the server answered with: 404 is a response, not an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// No usable response: bad URL, connection failure, or a malformed reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpError {
    InvalidUrl(String),
    Network(String),
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HttpError::InvalidUrl(msg) => write!(f, "invalid URL: {msg}"),
            HttpError::Network(msg) => write!(f, "HTTP request failed: {msg}"),
        }
    }
}

impl std::error::Error for HttpError {}
