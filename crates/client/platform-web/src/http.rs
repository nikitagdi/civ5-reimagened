//! HTTP GET on `fetch()`. The promise chain runs on the microtask queue and fills a slot
//! that `poll` reads.

use std::cell::RefCell;
use std::rc::Rc;

use platform::{HttpClient, HttpError, HttpOp, HttpResponse};
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

use crate::delivery;
use crate::transport::js_error_text;

/// `url` may be relative to the page (`/assets/...`).
#[derive(Debug, Default, Clone, Copy)]
pub struct FetchHttp;

type Slot = Rc<RefCell<Option<Result<HttpResponse, HttpError>>>>;

impl HttpClient for FetchHttp {
    fn get(&self, url: &str) -> Box<dyn HttpOp> {
        let slot: Slot = Rc::default();
        let out = Rc::clone(&slot);
        let url = url.to_owned();
        wasm_bindgen_futures::spawn_local(async move {
            let result = fetch_get(&url).await;
            *out.borrow_mut() = Some(result);
        });
        Box::new(FetchOp {
            slot,
            finished: false,
        })
    }

    fn forget(&self, url: &str) {
        delivery::forget(url);
    }

    fn manifest_ready(&self, live: std::collections::HashSet<String>, files: u64, bytes: u64) {
        delivery::set_expected(files, bytes);
        delivery::set_live_hashes(live);
    }
}

/// One GET from a page or from a worker. Any HTTP status is an `Ok` response.
pub async fn fetch_get(url: &str) -> Result<HttpResponse, HttpError> {
    let counted = delivery::Request::begin(url);
    let result = fetch_get_inner(url, &counted).await;
    if let Ok(resp) = &result
        && resp.is_success()
    {
        counted.done(resp.body.len());
    }
    result
}

async fn fetch_get_inner(
    url: &str,
    counted: &delivery::Request,
) -> Result<HttpResponse, HttpError> {
    let net = |what: &str, e: wasm_bindgen::JsValue| {
        HttpError::Network(format!("{what}: {}", js_error_text(&e)))
    };
    // Cache Storage first: the browser's HTTP cache does not keep multi-MB blobs, so a warm start
    // would download them all again. `None` without `caches` (insecure context).
    let cache = open_cache().await;
    // An entry that failed verification is skipped once (`HttpClient::forget`).
    let skip = delivery::take_bypass(url);
    if let Some(cache) = &cache
        && !skip
        && let Some(hit) = cache_match(cache, url).await
    {
        counted.cache_hit(true);
        return read_body(hit, &net).await.map(|(r, _)| r);
    }
    counted.cache_hit(false);
    // `fetch` of the page, or of the worker (the game host downloads its data itself).
    let promise = if let Some(window) = web_sys::window() {
        window.fetch_with_str(url)
    } else {
        js_sys::global()
            .dyn_into::<web_sys::WorkerGlobalScope>()
            .map_err(|_| HttpError::Network("no window or worker scope".into()))?
            .fetch_with_str(url)
    };
    let resp = JsFuture::from(promise)
        .await
        .map_err(|e| net(&format!("fetch {url}"), e))?
        .dyn_into::<web_sys::Response>()
        .map_err(|_| HttpError::Network("fetch did not return a Response".into()))?;
    // Only `immutable` answers are kept (content-addressed blobs); the manifest pointer is
    // `no-cache` and always goes to the server. A failed put never fails the GET.
    let keep = cache
        .filter(|_| resp.status() == 200 && !delivery::cache_off())
        .filter(|_| {
            resp.headers()
                .get("cache-control")
                .ok()
                .flatten()
                .is_some_and(|v| v.contains("immutable"))
        });
    let (out, buf) = read_body(resp, &net).await?;
    // The put runs after the body is read, from the same buffer (a clone of the response held
    // the two branches of the body against each other and some GETs never finished).
    if let Some(cache) = keep {
        let url = url.to_owned();
        wasm_bindgen_futures::spawn_local(async move {
            delivery::put_blob(&cache, &url, &buf).await;
        });
    }
    Ok(out)
}

/// The asset cache of the global scope (window or worker), if there is one.
pub(crate) async fn open_cache() -> Option<web_sys::Cache> {
    // An insecure context (plain HTTP to a LAN address) has no `caches` at all: the getter of
    // web-sys would hand back `undefined` and `open` on it would throw.
    let has = js_sys::Reflect::get(&js_sys::global(), &"caches".into())
        .is_ok_and(|v| !v.is_undefined() && !v.is_null());
    if !has {
        return None;
    }
    let storage = if let Some(window) = web_sys::window() {
        window.caches().ok()?
    } else {
        js_sys::global()
            .dyn_into::<web_sys::WorkerGlobalScope>()
            .ok()?
            .caches()
            .ok()?
    };
    JsFuture::from(storage.open(CACHE_NAME))
        .await
        .ok()?
        .dyn_into::<web_sys::Cache>()
        .ok()
}

/// A stored response for `url`, if any.
async fn cache_match(cache: &web_sys::Cache, url: &str) -> Option<web_sys::Response> {
    JsFuture::from(cache.match_with_str(url))
        .await
        .ok()?
        .dyn_into::<web_sys::Response>()
        .ok()
}

/// Name of the cache of content-addressed asset blobs; bump the suffix to drop old contents.
pub(crate) const CACHE_NAME: &str = "civ5-assets-v1";

/// The response with its body, and the buffer the body came in (for the cache).
async fn read_body(
    resp: web_sys::Response,
    net: &impl Fn(&str, wasm_bindgen::JsValue) -> HttpError,
) -> Result<(HttpResponse, js_sys::ArrayBuffer), HttpError> {
    let status = resp.status();
    let buf = JsFuture::from(resp.array_buffer().map_err(|e| net("reading body", e))?)
        .await
        .map_err(|e| net("reading body", e))?;
    let body = js_sys::Uint8Array::new(&buf).to_vec();
    Ok((HttpResponse { status, body }, buf.unchecked_into()))
}

struct FetchOp {
    slot: Slot,
    finished: bool,
}

impl HttpOp for FetchOp {
    fn poll(&mut self) -> Option<Result<HttpResponse, HttpError>> {
        if self.finished {
            return None;
        }
        let r = self.slot.borrow_mut().take()?;
        self.finished = true;
        Some(r)
    }
}
