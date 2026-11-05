//! Blob source over HTTP: `GET <base>/<hex hash>`, the body checked against the hash.

use std::cell::Cell;
use std::rc::Rc;

use platform::{HttpClient, HttpError, HttpOp, HttpResponse};

use crate::{AssetError, BlobSource, ContentHash, FetchOp};

/// Bytes hashed per `poll`, so a large blob does not stall a frame.
const VERIFY_CHUNK: usize = 512 * 1024;

thread_local! {
    static VERIFIED_BYTES: Cell<u64> = const { Cell::new(0) };
    static FAILURES: Cell<u64> = const { Cell::new(0) };
}

/// `(verified_bytes, failures)` of this thread since start: bytes hashed (also of bodies that
/// failed) and bodies whose hash did not match (each retry counts).
pub fn verify_stats() -> (u64, u64) {
    (VERIFIED_BYTES.get(), FAILURES.get())
}

/// Immutable blobs served at `<base><hash>` (the server's `/assets/`).
pub struct HttpSource {
    http: Rc<dyn HttpClient>,
    /// Always ends with `/`.
    base: String,
}

impl HttpSource {
    pub fn new(http: Rc<dyn HttpClient>, base_url: &str) -> Self {
        Self {
            http,
            base: with_trailing_slash(base_url),
        }
    }
}

pub(crate) fn with_trailing_slash(url: &str) -> String {
    if url.ends_with('/') {
        url.to_owned()
    } else {
        format!("{url}/")
    }
}

impl BlobSource for HttpSource {
    fn get(&self, hash: &ContentHash) -> Box<dyn FetchOp> {
        let url = format!("{}{hash}", self.base);
        Box::new(HttpFetch {
            http: Rc::clone(&self.http),
            op: self.http.get(&url),
            url,
            hash: *hash,
            stage: Stage::Fetching,
            retried: false,
        })
    }
}

enum Stage {
    Fetching,
    /// The body is here; `pos` bytes of it are hashed.
    Hashing {
        body: Vec<u8>,
        pos: usize,
        hasher: Box<blake3::Hasher>,
    },
}

struct HttpFetch {
    http: Rc<dyn HttpClient>,
    op: Box<dyn HttpOp>,
    url: String,
    hash: ContentHash,
    stage: Stage,
    retried: bool,
}

impl HttpFetch {
    fn on_response(
        &mut self,
        result: Result<HttpResponse, HttpError>,
    ) -> Option<Result<Option<Vec<u8>>, AssetError>> {
        match result {
            Ok(resp) if resp.is_success() => {
                self.stage = Stage::Hashing {
                    body: resp.body,
                    pos: 0,
                    hasher: Box::new(blake3::Hasher::new()),
                };
                self.hash_step()
            }
            // "Not in this source": `Layered` moves on.
            Ok(resp) if resp.status == 404 => Some(Ok(None)),
            Ok(resp) => Some(Err(AssetError::Network(format!(
                "GET {}: HTTP {}",
                self.hash, resp.status
            )))),
            Err(e) => Some(Err(http_error(e))),
        }
    }

    /// Hashes the next chunk; `None` while there is more.
    fn hash_step(&mut self) -> Option<Result<Option<Vec<u8>>, AssetError>> {
        let Stage::Hashing { body, pos, hasher } = &mut self.stage else {
            return None;
        };
        let end = (*pos + VERIFY_CHUNK).min(body.len());
        hasher.update(&body[*pos..end]);
        VERIFIED_BYTES.set(VERIFIED_BYTES.get() + (end - *pos) as u64);
        *pos = end;
        if end < body.len() {
            return None;
        }
        let ok = hasher.finalize().as_bytes() == self.hash.as_bytes();
        let Stage::Hashing { body, .. } = std::mem::replace(&mut self.stage, Stage::Fetching)
        else {
            return None;
        };
        if ok {
            return Some(Ok(Some(body)));
        }
        FAILURES.set(FAILURES.get() + 1);
        if self.retried {
            return Some(Err(AssetError::Corrupt(self.hash)));
        }
        tracing::warn!(hash = %self.hash, "blob does not match its hash; downloading it again");
        self.retried = true;
        self.http.forget(&self.url);
        self.op = self.http.get(&self.url);
        None
    }
}

impl FetchOp for HttpFetch {
    fn poll(&mut self) -> Option<Result<Option<Vec<u8>>, AssetError>> {
        match self.stage {
            Stage::Fetching => {
                let result = self.op.poll()?;
                self.on_response(result)
            }
            Stage::Hashing { .. } => self.hash_step(),
        }
    }
}

pub(crate) fn http_error(e: HttpError) -> AssetError {
    AssetError::Network(e.to_string())
}

#[cfg(test)]
pub(crate) mod fake {
    use std::cell::RefCell;
    use std::collections::{HashMap, VecDeque};

    use platform::HttpResponse;

    use super::*;

    /// Canned responses by URL; unknown URLs answer 404. Responses arrive after `delay` polls.
    #[derive(Default)]
    pub struct FakeHttp {
        pub routes: RefCell<HashMap<String, Result<HttpResponse, HttpError>>>,
        /// Bodies served in turn, one per GET; the last one repeats. Checked before `routes`.
        pub sequences: RefCell<HashMap<String, VecDeque<Vec<u8>>>>,
        pub requests: RefCell<Vec<String>>,
        pub forgotten: RefCell<Vec<String>>,
        pub delay: u32,
    }

    impl FakeHttp {
        pub fn ok(&self, url: &str, body: &[u8]) {
            self.routes.borrow_mut().insert(
                url.to_owned(),
                Ok(HttpResponse {
                    status: 200,
                    body: body.to_vec(),
                }),
            );
        }

        pub fn ok_in_turn(&self, url: &str, bodies: &[&[u8]]) {
            self.sequences
                .borrow_mut()
                .insert(url.to_owned(), bodies.iter().map(|b| b.to_vec()).collect());
        }

        pub fn status(&self, url: &str, status: u16) {
            self.routes.borrow_mut().insert(
                url.to_owned(),
                Ok(HttpResponse {
                    status,
                    body: Vec::new(),
                }),
            );
        }
    }

    struct Op {
        delay: u32,
        result: Option<Result<HttpResponse, HttpError>>,
    }

    impl HttpOp for Op {
        fn poll(&mut self) -> Option<Result<HttpResponse, HttpError>> {
            if self.delay > 0 {
                self.delay -= 1;
                return None;
            }
            self.result.take()
        }
    }

    impl HttpClient for FakeHttp {
        fn get(&self, url: &str) -> Box<dyn HttpOp> {
            self.requests.borrow_mut().push(url.to_owned());
            let turn = self.sequences.borrow_mut().get_mut(url).map(|q| {
                if q.len() > 1 {
                    q.pop_front().unwrap_or_default()
                } else {
                    q.front().cloned().unwrap_or_default()
                }
            });
            let result = match turn {
                Some(body) => Ok(HttpResponse { status: 200, body }),
                None => self
                    .routes
                    .borrow()
                    .get(url)
                    .cloned()
                    .unwrap_or(Ok(HttpResponse {
                        status: 404,
                        body: Vec::new(),
                    })),
            };
            Box::new(Op {
                delay: self.delay,
                result: Some(result),
            })
        }

        fn forget(&self, url: &str) {
            self.forgotten.borrow_mut().push(url.to_owned());
        }
    }
}

#[cfg(test)]
pub(crate) fn hash_of(bytes: &[u8]) -> ContentHash {
    ContentHash(*blake3::hash(bytes).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::fake::FakeHttp;
    use super::*;

    fn run(op: &mut dyn FetchOp) -> Result<Option<Vec<u8>>, AssetError> {
        for _ in 0..100 {
            if let Some(r) = op.poll() {
                return r;
            }
        }
        panic!("operation did not complete");
    }

    #[test]
    fn maps_status_to_result() {
        let hash = hash_of(b"blob");
        let url = format!("http://h/assets/{hash}");
        let http = Rc::new(FakeHttp::default());
        let src = HttpSource::new(http.clone(), "http://h/assets");

        http.ok(&url, b"blob");
        assert_eq!(run(&mut *src.get(&hash)).unwrap(), Some(b"blob".to_vec()));
        assert_eq!(*http.requests.borrow(), std::slice::from_ref(&url));

        http.status(&url, 404);
        assert_eq!(run(&mut *src.get(&hash)).unwrap(), None);

        http.status(&url, 500);
        assert!(matches!(
            run(&mut *src.get(&hash)),
            Err(AssetError::Network(m)) if m.contains("500")
        ));

        http.routes
            .borrow_mut()
            .insert(url, Err(HttpError::Network("boom".into())));
        assert!(matches!(
            run(&mut *src.get(&hash)),
            Err(AssetError::Network(_))
        ));
    }

    #[test]
    fn corrupt_body_is_forgotten_and_downloaded_once_more_then_fails() {
        let hash = hash_of(b"good");
        let url = format!("http://h/assets/{hash}");
        let http = Rc::new(FakeHttp::default());
        let src = HttpSource::new(http.clone(), "http://h/assets");
        http.ok(&url, b"bad!");
        let (_, failures) = verify_stats();
        assert!(matches!(
            run(&mut *src.get(&hash)),
            Err(AssetError::Corrupt(h)) if h == hash
        ));
        assert_eq!(http.requests.borrow().len(), 2, "one retry, no more");
        assert_eq!(*http.forgotten.borrow(), std::slice::from_ref(&url));
        assert_eq!(verify_stats().1, failures + 2);
    }

    #[test]
    fn corrupt_then_good_succeeds() {
        let hash = hash_of(b"good");
        let url = format!("http://h/assets/{hash}");
        let http = Rc::new(FakeHttp {
            delay: 1,
            ..FakeHttp::default()
        });
        let src = HttpSource::new(http.clone(), "http://h/assets");
        http.ok_in_turn(&url, &[b"bad!", b"good"]);
        assert_eq!(run(&mut *src.get(&hash)).unwrap(), Some(b"good".to_vec()));
        assert_eq!(http.requests.borrow().len(), 2);
        assert_eq!(http.forgotten.borrow().len(), 1);
    }

    #[test]
    fn a_large_body_is_hashed_over_several_polls() {
        let body: Vec<u8> = (0..VERIFY_CHUNK * 2 + 10)
            .map(|i| (i % 251) as u8)
            .collect();
        let hash = hash_of(&body);
        let url = format!("http://h/assets/{hash}");
        let http = Rc::new(FakeHttp::default());
        let src = HttpSource::new(http.clone(), "http://h/assets");
        http.ok(&url, &body);
        let before = verify_stats().0;
        let mut op = src.get(&hash);
        let mut polls = 0;
        let got = loop {
            polls += 1;
            if let Some(r) = op.poll() {
                break r;
            }
            assert!(polls < 10, "did not finish");
        };
        assert_eq!(got.unwrap(), Some(body.clone()));
        assert_eq!(polls, 3, "three chunks, one per poll");
        assert_eq!(verify_stats().0 - before, body.len() as u64);
    }

    #[test]
    fn base_slash_is_normalized() {
        assert_eq!(with_trailing_slash("http://h/assets"), "http://h/assets/");
        assert_eq!(with_trailing_slash("http://h/assets/"), "http://h/assets/");
    }
}
