//! Delivery counters (`net.*`, `assets.*`, `storage.*`) and the hygiene of the asset cache: stale
//! blobs are removed after the manifest is in, a full quota is handled once (asset-delivery spec
//! D-6, D-7, D-9). Counters are per thread: the game worker's own downloads are not counted.

use std::cell::RefCell;
use std::collections::HashSet;

use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::JsFuture;

use crate::http::open_cache;

/// Delay between a ready manifest and the removal of stale blobs.
const PURGE_DELAY_MS: i32 = 15_000;
/// Minimum time between two reads of `navigator.storage` and of the cache keys.
const STORAGE_REFRESH_MS: f64 = 10_000.0;

/// Bytes per second over the span since the last read that was at least a second ago.
#[derive(Default)]
struct Rate {
    since_ms: f64,
    since_total: u64,
    bps: f64,
}

impl Rate {
    fn read(&mut self, now_ms: f64, total: u64) -> f64 {
        let span = now_ms - self.since_ms;
        if self.since_ms == 0.0 {
            self.since_ms = now_ms;
            self.since_total = total;
        } else if span >= 1000.0 {
            self.bps = total.saturating_sub(self.since_total) as f64 * 1000.0 / span;
            self.since_ms = now_ms;
            self.since_total = total;
        }
        self.bps
    }
}

/// What to do after a failed `cache.put`.
#[derive(Debug, PartialEq, Eq)]
enum PutAction {
    /// Only count it.
    Skip,
    /// Remove stale entries, then put once more.
    PurgeRetry,
    /// No more puts this session.
    Disable,
}

fn on_put_failure(quota_error: bool, is_retry: bool, purged_before: bool) -> PutAction {
    match (quota_error, is_retry || purged_before) {
        (false, _) => PutAction::Skip,
        (true, false) => PutAction::PurgeRetry,
        (true, true) => PutAction::Disable,
    }
}

/// The 64 hex digits at the end of a `.../assets/<hash>` URL; other shapes are not ours.
fn blob_hash_of(url: &str) -> Option<String> {
    let (rest, hash) = url.rsplit_once('/')?;
    let (_, parent) = rest.rsplit_once('/')?;
    (parent == "assets" && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| hash.to_ascii_lowercase())
}

/// Keys of blobs whose hash is not in `live`. Empty if `live` is empty (no manifest known).
fn stale_keys(keys: &[String], live: &HashSet<String>) -> Vec<String> {
    if live.is_empty() {
        return Vec::new();
    }
    keys.iter()
        .filter(|k| blob_hash_of(k).is_some_and(|h| !live.contains(&h)))
        .cloned()
        .collect()
}

#[derive(Default)]
struct State {
    in_bytes: u64,
    out_bytes: u64,
    msgs_in: u64,
    msgs_out: u64,
    in_rate: Rate,
    out_rate: Rate,

    requests: u64,
    in_flight: u64,
    files_done: u64,
    bytes_done: u64,
    cache_hits: u64,
    cache_misses: u64,
    files_total: u64,
    bytes_total: u64,

    used: f64,
    quota: f64,
    refreshed_ms: f64,
    blobs: u64,
    stale_blobs: u64,
    removed_total: u64,
    put_failed: u64,
    cache_off: bool,
    purged: bool,
    live: HashSet<String>,
    purge_scheduled: bool,
    /// URLs whose next GET must skip the cache (their entry failed verification).
    bypass: HashSet<String>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| f(&mut s.borrow_mut()))
}

fn now_ms() -> f64 {
    js_sys::Date::now()
}

pub(crate) fn note_net_in(bytes: usize) {
    with(|s| {
        s.in_bytes += bytes as u64;
        s.msgs_in += 1;
    });
}

pub(crate) fn note_net_out(bytes: usize) {
    with(|s| {
        s.out_bytes += bytes as u64;
        s.msgs_out += 1;
    });
}

/// What this device will fetch from the manifest (`assets.files_total`, `assets.bytes_total`).
pub fn set_expected(files: u64, bytes: u64) {
    with(|s| {
        s.files_total = files;
        s.bytes_total = bytes;
    });
}

/// The manifest is in: `live` is the hash of every blob it names. Starts the removal of the other
/// blobs from the cache after a delay. An empty set does nothing.
pub fn set_live_hashes(live: HashSet<String>) {
    if live.is_empty() {
        return;
    }
    let schedule = with(|s| {
        s.live = live;
        !std::mem::replace(&mut s.purge_scheduled, true)
    });
    let Some(window) = web_sys::window() else {
        return;
    };
    if schedule {
        let run = Closure::once_into_js(|| {
            wasm_bindgen_futures::spawn_local(async {
                purge_stale().await;
            });
        });
        let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
            run.unchecked_ref(),
            PURGE_DELAY_MS,
        );
    }
}

/// `forget` of the HTTP client: the next GET of `url` goes to the network, and the entry goes.
pub(crate) fn forget(url: &str) {
    with(|s| s.bypass.insert(url.to_owned()));
    let url = url.to_owned();
    wasm_bindgen_futures::spawn_local(async move {
        if let Some(cache) = open_cache().await
            && let Ok(p) = JsFuture::from(cache.delete_with_str(&url)).await
            && p.as_bool() == Some(true)
        {
            with(|s| s.removed_total += 1);
        }
    });
}

pub(crate) fn take_bypass(url: &str) -> bool {
    with(|s| s.bypass.remove(url))
}

pub(crate) fn cache_off() -> bool {
    with(|s| s.cache_off)
}

/// Counts one GET from `begin` to the end of its future (also when dropped).
pub(crate) struct Request {
    blob: bool,
}

impl Request {
    pub(crate) fn begin(url: &str) -> Self {
        with(|s| {
            s.requests += 1;
            s.in_flight += 1;
        });
        Self {
            blob: blob_hash_of(url).is_some(),
        }
    }

    pub(crate) fn cache_hit(&self, hit: bool) {
        if self.blob {
            with(|s| {
                if hit {
                    s.cache_hits += 1;
                } else {
                    s.cache_misses += 1;
                }
            });
        }
    }

    pub(crate) fn done(&self, body_len: usize) {
        if self.blob {
            with(|s| {
                s.files_done += 1;
                s.bytes_done += body_len as u64;
            });
        }
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        with(|s| s.in_flight = s.in_flight.saturating_sub(1));
    }
}

fn is_quota_error(e: &wasm_bindgen::JsValue) -> bool {
    js_sys::Reflect::get(e, &"name".into())
        .ok()
        .and_then(|n| n.as_string())
        .is_some_and(|n| n == "QuotaExceededError")
}

async fn put_once(
    cache: &web_sys::Cache,
    url: &str,
    body: &js_sys::Object,
) -> Result<(), wasm_bindgen::JsValue> {
    let resp = web_sys::Response::new_with_opt_buffer_source(Some(body))?;
    JsFuture::from(cache.put_with_str(url, &resp)).await?;
    Ok(())
}

/// Stores a verified-by-hash-address blob. Never fails the caller: a quota error purges stale
/// entries once and retries once; a second failure turns the cache off for the session.
pub(crate) async fn put_blob(cache: &web_sys::Cache, url: &str, body: &js_sys::Object) {
    if cache_off() {
        return;
    }
    let Err(e) = put_once(cache, url, body).await else {
        return;
    };
    with(|s| s.put_failed += 1);
    tracing::debug!(
        "asset cache put {url}: {}",
        crate::transport::js_error_text(&e)
    );
    let purged = with(|s| s.purged);
    match on_put_failure(is_quota_error(&e), false, purged) {
        PutAction::Skip => {}
        PutAction::Disable => disable_cache(),
        PutAction::PurgeRetry => {
            purge_stale().await;
            if put_once(cache, url, body).await.is_err() {
                with(|s| s.put_failed += 1);
                disable_cache();
            }
        }
    }
}

fn disable_cache() {
    tracing::warn!("asset cache is full: blobs are not cached until the page is reloaded");
    with(|s| s.cache_off = true);
}

async fn cache_keys(cache: &web_sys::Cache) -> Vec<String> {
    let Ok(keys) = JsFuture::from(cache.keys()).await else {
        return Vec::new();
    };
    js_sys::Array::from(&keys)
        .iter()
        .filter_map(|k| k.dyn_into::<web_sys::Request>().ok())
        .map(|r| r.url())
        .collect()
}

/// Deletes the entries of the cache whose hash is not live. Safe to call at any time.
async fn purge_stale() {
    with(|s| s.purged = true);
    let Some(cache) = open_cache().await else {
        return;
    };
    let live = with(|s| s.live.clone());
    let keys = cache_keys(&cache).await;
    let mut removed = 0u64;
    for key in stale_keys(&keys, &live) {
        if let Ok(v) = JsFuture::from(cache.delete_with_str(&key)).await
            && v.as_bool() == Some(true)
        {
            removed += 1;
        }
    }
    let blobs = (keys.len() as u64).saturating_sub(removed);
    with(|s| {
        s.removed_total += removed;
        s.blobs = blobs;
        s.stale_blobs = 0;
    });
}

/// Refreshes `storage.*` in the background, at most every 10 s.
fn refresh_storage() {
    let due = with(|s| {
        let now = now_ms();
        let due = now - s.refreshed_ms >= STORAGE_REFRESH_MS;
        if due {
            s.refreshed_ms = now;
        }
        due
    });
    if !due {
        return;
    }
    wasm_bindgen_futures::spawn_local(async {
        let estimate = web_sys::window().and_then(|w| w.navigator().storage().estimate().ok());
        if let Some(p) = estimate
            && let Ok(est) = JsFuture::from(p).await
        {
            let num = |k: &str| {
                js_sys::Reflect::get(&est, &k.into())
                    .ok()
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0)
            };
            with(|s| {
                s.used = num("usage");
                s.quota = num("quota");
            });
        }
        if let Some(cache) = open_cache().await {
            let keys = cache_keys(&cache).await;
            let live = with(|s| s.live.clone());
            let stale = stale_keys(&keys, &live).len() as u64;
            with(|s| {
                s.blobs = keys.len() as u64;
                s.stale_blobs = stale;
            });
        }
    });
}

/// All delivery counters except `assets.hash_fail` and `assets.verify_bytes` (the `assets` crate
/// keeps those; the app adds them), for `window.__civ5stats.counters`. Also starts a refresh of `storage.*`
/// when the last one is older than 10 s, so the first reads may show zeros.
pub fn counters() -> Vec<(&'static str, f64)> {
    refresh_storage();
    let now = now_ms();
    with(|s| {
        let in_bps = s.in_rate.read(now, s.in_bytes);
        let out_bps = s.out_rate.read(now, s.out_bytes);
        vec![
            ("net.in_bytes", s.in_bytes as f64),
            ("net.out_bytes", s.out_bytes as f64),
            ("net.in_bps", in_bps),
            ("net.out_bps", out_bps),
            ("net.msgs_in", s.msgs_in as f64),
            ("net.msgs_out", s.msgs_out as f64),
            ("assets.requests", s.requests as f64),
            ("assets.in_flight", s.in_flight as f64),
            ("assets.files_done", s.files_done as f64),
            ("assets.bytes_done", s.bytes_done as f64),
            ("assets.files_total", s.files_total as f64),
            ("assets.bytes_total", s.bytes_total as f64),
            ("assets.cache_hits", s.cache_hits as f64),
            ("assets.cache_misses", s.cache_misses as f64),
            ("storage.used", s.used),
            ("storage.quota", s.quota),
            ("storage.blobs", s.blobs as f64),
            ("storage.stale_blobs", s.stale_blobs as f64),
            ("storage.removed_total", s.removed_total as f64),
            ("storage.put_failed", s.put_failed as f64),
            ("storage.cache_off", f64::from(u8::from(s.cache_off))),
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    fn url(h: &str) -> String {
        format!("https://civ5.example/assets/{h}")
    }

    #[test]
    fn only_blobs_of_other_manifests_are_stale() {
        let live: HashSet<String> = [A.to_owned()].into();
        let keys = [
            url(A),
            url(B),
            "https://civ5.example/assets/manifest".to_owned(),
            "https://civ5.example/other/".to_owned() + B,
            url(&B[..63]),
            url(B) + "?x=1",
            "https://civ5.example/assets/".to_owned() + &"g".repeat(64),
        ];
        assert_eq!(stale_keys(&keys, &live), [url(B)]);
        assert_eq!(
            stale_keys(&[url(&B.to_uppercase())], &live),
            [url(&B.to_uppercase())],
            "case does not matter"
        );
    }

    #[test]
    fn an_empty_live_set_removes_nothing() {
        assert!(stale_keys(&[url(A), url(B)], &HashSet::new()).is_empty());
    }

    #[test]
    fn quota_errors_purge_once_then_give_up() {
        assert_eq!(on_put_failure(false, false, false), PutAction::Skip);
        assert_eq!(on_put_failure(false, true, true), PutAction::Skip);
        assert_eq!(on_put_failure(true, false, false), PutAction::PurgeRetry);
        assert_eq!(on_put_failure(true, true, true), PutAction::Disable);
        assert_eq!(on_put_failure(true, false, true), PutAction::Disable);
    }

    #[test]
    fn rate_is_bytes_per_second_over_the_last_window() {
        let mut r = Rate::default();
        assert_eq!(r.read(10_000.0, 500), 0.0, "first read opens the window");
        assert_eq!(r.read(10_500.0, 900), 0.0, "window not full yet");
        assert_eq!(r.read(11_000.0, 2_500), 2_000.0 * 1000.0 / 1000.0);
        assert_eq!(r.read(11_200.0, 9_000), 2_000.0, "keeps the last value");
        assert_eq!(r.read(13_000.0, 3_500), 500.0);
    }
}
