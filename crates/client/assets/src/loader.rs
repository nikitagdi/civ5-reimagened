//! Manifest loading and logical-id requests on top of a [`BlobSource`].
//!
//! Separate from [`Streamer`](crate::Streamer): that one is a pure priority queue without I/O,
//! while this owns the in-flight fetches and the manifest. `client-app` can feed it from the
//! `Streamer` later.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use platform::{HttpClient, HttpOp};

use crate::http::{http_error, with_trailing_slash};
use crate::{AssetError, BlobSource, ContentHash, FetchOp, LogicalId, Manifest, PlatformProfile};

/// Path of the current-manifest pointer, relative to the assets base URL.
const MANIFEST_POINTER: &str = "manifest";

/// Opaque id of a [`AssetLoader::request`]. Consumed by the `poll` that delivers its result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AssetHandle(u64);

#[derive(Debug)]
pub enum AssetPoll {
    Pending,
    Ready(Vec<u8>),
    Failed(AssetError),
}

/// Progress of [`AssetLoader::load_manifest`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestStatus {
    /// `load_manifest` was not called.
    Idle,
    Loading,
    Ready,
    /// See [`AssetLoader::manifest_error`].
    Failed,
}

enum ManifestState {
    Idle,
    /// Fetching the pointer file (hash of the current manifest blob).
    Pointer(Box<dyn HttpOp>),
    Blob(Box<dyn FetchOp>, ContentHash),
    Ready(Manifest),
    Failed(AssetError),
}

enum Stage {
    /// Waiting for the manifest to resolve the id.
    AwaitManifest(PlatformProfile),
    Fetch(Box<dyn FetchOp>, ContentHash),
}

struct Request {
    id: LogicalId,
    stage: Stage,
}

/// Single-threaded and polled: call [`poll`](Self::poll) or
/// [`poll_manifest`](Self::poll_manifest) every frame.
pub struct AssetLoader {
    http: Rc<dyn HttpClient>,
    /// Ends with `/`.
    base: String,
    source: Box<dyn BlobSource>,
    manifest: ManifestState,
    /// Hash of the loaded manifest blob.
    manifest_hash: Option<ContentHash>,
    announced: bool,
    requests: HashMap<u64, Request>,
    next_handle: u64,
}

impl AssetLoader {
    /// `base_url` is the assets root (`http://host:port/assets/`), used for the manifest
    /// pointer; blobs come from `source`, typically a [`Layered`](crate::Layered) ending in an
    /// [`HttpSource`](crate::HttpSource) on the same base.
    pub fn new(http: Rc<dyn HttpClient>, base_url: &str, source: Box<dyn BlobSource>) -> Self {
        Self {
            http,
            base: with_trailing_slash(base_url),
            source,
            manifest: ManifestState::Idle,
            manifest_hash: None,
            announced: false,
            requests: HashMap::new(),
            next_handle: 0,
        }
    }

    /// Starts loading; restarts if already running. With `hash`, skips the pointer fetch
    /// (`?manifest=<hash>` override).
    pub fn load_manifest(&mut self, hash: Option<ContentHash>) {
        self.manifest = match hash {
            Some(h) => ManifestState::Blob(self.source.get(&h), h),
            None => {
                ManifestState::Pointer(self.http.get(&format!("{}{MANIFEST_POINTER}", self.base)))
            }
        };
    }

    pub fn manifest(&self) -> Option<&Manifest> {
        match &self.manifest {
            ManifestState::Ready(m) => Some(m),
            _ => None,
        }
    }

    /// Once per loaded manifest: tells the HTTP client which blobs are live (every variant and the
    /// manifest blob itself) and how much `profile` will fetch from it. No-op until the manifest is
    /// ready.
    pub fn announce_manifest(&mut self, profile: &PlatformProfile) {
        let ManifestState::Ready(m) = &self.manifest else {
            return;
        };
        if self.announced {
            return;
        }
        self.announced = true;
        let mut live: HashSet<String> =
            self.manifest_hash.iter().map(ToString::to_string).collect();
        let (mut files, mut bytes) = (0u64, 0u64);
        for (id, variants) in m.iter() {
            live.extend(variants.iter().map(|v| v.hash.to_string()));
            if let Some(v) = m.resolve(id, profile) {
                files += 1;
                bytes += v.size;
            }
        }
        self.http.manifest_ready(live, files, bytes);
    }

    pub fn manifest_error(&self) -> Option<&AssetError> {
        match &self.manifest {
            ManifestState::Failed(e) => Some(e),
            _ => None,
        }
    }

    /// Advances manifest loading.
    pub fn poll_manifest(&mut self) -> ManifestStatus {
        // Steps can chain within one call (the pointer arrives, the blob is already cached).
        loop {
            match &mut self.manifest {
                ManifestState::Idle => return ManifestStatus::Idle,
                ManifestState::Ready(_) => return ManifestStatus::Ready,
                ManifestState::Failed(_) => return ManifestStatus::Failed,
                ManifestState::Pointer(op) => {
                    let Some(result) = op.poll() else {
                        return ManifestStatus::Loading;
                    };
                    self.manifest = match parse_pointer(result) {
                        Ok(h) => ManifestState::Blob(self.source.get(&h), h),
                        Err(e) => ManifestState::Failed(e),
                    };
                }
                ManifestState::Blob(op, hash) => {
                    let Some(result) = op.poll() else {
                        return ManifestStatus::Loading;
                    };
                    let hash = *hash;
                    self.manifest = match decode_manifest(result, hash) {
                        Ok(m) => {
                            self.manifest_hash = Some(hash);
                            self.announced = false;
                            ManifestState::Ready(m)
                        }
                        Err(e) => ManifestState::Failed(e),
                    };
                }
            }
        }
    }

    /// Resolves `id` against the manifest for `profile` and fetches the chosen variant's blob.
    /// May be called before the manifest is ready: resolution waits for it.
    pub fn request(&mut self, id: LogicalId, profile: &PlatformProfile) -> AssetHandle {
        let handle = self.next_handle;
        self.next_handle += 1;
        self.requests.insert(
            handle,
            Request {
                id,
                stage: Stage::AwaitManifest(profile.clone()),
            },
        );
        AssetHandle(handle)
    }

    /// Delivers the result once; afterwards the handle is `Failed(UnknownHandle)`.
    pub fn poll(&mut self, handle: AssetHandle) -> AssetPoll {
        self.poll_manifest();
        let Some(req) = self.requests.get_mut(&handle.0) else {
            return AssetPoll::Failed(AssetError::UnknownHandle);
        };
        if let Stage::AwaitManifest(profile) = &req.stage {
            let resolved = match &self.manifest {
                ManifestState::Ready(m) => resolve(m, &req.id, profile),
                ManifestState::Failed(_) => Err(AssetError::ManifestNotLoaded),
                ManifestState::Idle => Err(AssetError::ManifestNotLoaded),
                ManifestState::Pointer(_) | ManifestState::Blob(..) => {
                    return AssetPoll::Pending;
                }
            };
            match resolved {
                Ok(hash) => req.stage = Stage::Fetch(self.source.get(&hash), hash),
                Err(e) => {
                    self.requests.remove(&handle.0);
                    return AssetPoll::Failed(e);
                }
            }
        }
        let Stage::Fetch(op, hash) = &mut req.stage else {
            return AssetPoll::Pending;
        };
        let Some(result) = op.poll() else {
            return AssetPoll::Pending;
        };
        let hash = *hash;
        self.requests.remove(&handle.0);
        match result {
            Ok(Some(bytes)) => AssetPoll::Ready(bytes),
            Ok(None) => AssetPoll::Failed(AssetError::BlobNotFound(hash)),
            Err(e) => AssetPoll::Failed(e),
        }
    }

    /// Forgets the request; its fetch is dropped.
    pub fn cancel(&mut self, handle: AssetHandle) {
        self.requests.remove(&handle.0);
    }

    /// The URL of the blob `id` resolves to for `profile` (`base` + hash), for a file the platform streams itself (the
    /// music: an `<audio>` element downloads while it plays). `None` before the manifest is loaded or for an unknown id.
    pub fn url_of(&self, id: &LogicalId, profile: &PlatformProfile) -> Option<String> {
        let ManifestState::Ready(m) = &self.manifest else {
            return None;
        };
        let hash = resolve(m, id, profile).ok()?;
        Some(format!("{}{hash}", self.base))
    }
}

fn resolve(
    manifest: &Manifest,
    id: &LogicalId,
    profile: &PlatformProfile,
) -> Result<ContentHash, AssetError> {
    if manifest.variants(id).is_empty() {
        return Err(AssetError::UnknownAsset(id.clone()));
    }
    manifest
        .resolve(id, profile)
        .map(|v| v.hash)
        .ok_or_else(|| AssetError::NoVariant(id.clone()))
}

fn parse_pointer(
    result: Result<platform::HttpResponse, platform::HttpError>,
) -> Result<ContentHash, AssetError> {
    let resp = result.map_err(http_error)?;
    if !resp.is_success() {
        return Err(AssetError::ManifestUnavailable(resp.status));
    }
    let text = String::from_utf8_lossy(&resp.body);
    text.trim().parse().map_err(AssetError::ManifestPointer)
}

fn decode_manifest(
    result: Result<Option<Vec<u8>>, AssetError>,
    hash: ContentHash,
) -> Result<Manifest, AssetError> {
    match result? {
        Some(bytes) => Ok(Manifest::decode(&bytes)?),
        None => Err(AssetError::BlobNotFound(hash)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::fake::FakeHttp;
    use crate::http::hash_of;
    use crate::{HttpSource, PrefetchGroup, Variant, VariantFormat};

    const BASE: &str = "http://h/assets/";

    fn hash(n: u8) -> ContentHash {
        ContentHash([n; 32])
    }

    /// Blobs are checked against their hash: the hash of the fake bodies below.
    fn bc() -> ContentHash {
        hash_of(b"bc")
    }

    fn rgba() -> ContentHash {
        hash_of(b"rgba")
    }

    fn variant(format: VariantFormat, n: u8) -> Variant {
        Variant {
            format,
            hash: hash(n),
            size: 10,
            prefetch: PrefetchGroup(0),
        }
    }

    fn manifest() -> Manifest {
        let mut m = Manifest::new();
        let a = LogicalId::new("tex:a");
        let with = |format, h| Variant {
            hash: h,
            ..variant(format, 0)
        };
        m.insert(a.clone(), with(VariantFormat::Bc, bc()));
        m.insert(a, with(VariantFormat::Uncompressed, rgba()));
        m.insert(LogicalId::new("tex:bc-only"), variant(VariantFormat::Bc, 3));
        m
    }

    /// Manifest blob, pointer to it, and blobs for `tex:a` (Bc, RGBA).
    fn server(delay: u32) -> Rc<FakeHttp> {
        let m = manifest();
        let mhash = hash_of(&m.encode());
        let http = Rc::new(FakeHttp {
            delay,
            ..FakeHttp::default()
        });
        http.ok(&format!("{BASE}manifest"), format!("{mhash}\n").as_bytes());
        http.ok(&format!("{BASE}{mhash}"), &m.encode());
        http.ok(&format!("{BASE}{}", bc()), b"bc");
        http.ok(&format!("{BASE}{}", rgba()), b"rgba");
        http
    }

    fn loader(http: &Rc<FakeHttp>) -> AssetLoader {
        let src = HttpSource::new(http.clone(), BASE);
        AssetLoader::new(http.clone(), BASE, Box::new(src))
    }

    fn wait_manifest(l: &mut AssetLoader) -> ManifestStatus {
        for _ in 0..100 {
            let s = l.poll_manifest();
            if s != ManifestStatus::Loading {
                return s;
            }
        }
        panic!("manifest did not settle");
    }

    fn wait(l: &mut AssetLoader, h: AssetHandle) -> AssetPoll {
        for _ in 0..100 {
            match l.poll(h) {
                AssetPoll::Pending => {}
                done => return done,
            }
        }
        panic!("request did not settle");
    }

    #[test]
    fn the_url_of_a_blob_is_the_base_and_its_hash_once_the_manifest_is_in() {
        let http = server(0);
        let mut l = loader(&http);
        let p = PlatformProfile::new(vec![VariantFormat::Bc]);
        let id = LogicalId::new("tex:bc-only");
        assert_eq!(l.url_of(&id, &p), None, "no manifest yet");
        l.load_manifest(None);
        assert_eq!(wait_manifest(&mut l), ManifestStatus::Ready);
        assert_eq!(l.url_of(&id, &p), Some(format!("{BASE}{}", hash(3))));
        assert_eq!(l.url_of(&LogicalId::new("tex:nope"), &p), None);
    }

    #[test]
    fn loads_manifest_via_pointer() {
        let http = server(2);
        let mut l = loader(&http);
        assert_eq!(l.poll_manifest(), ManifestStatus::Idle);
        l.load_manifest(None);
        assert_eq!(l.poll_manifest(), ManifestStatus::Loading);
        assert_eq!(wait_manifest(&mut l), ManifestStatus::Ready);
        assert_eq!(l.manifest().unwrap().len(), 2);
        assert_eq!(http.requests.borrow().len(), 2);
    }

    #[test]
    fn hash_override_skips_pointer() {
        let http = server(0);
        let mut l = loader(&http);
        l.load_manifest(Some(hash_of(&manifest().encode())));
        assert_eq!(wait_manifest(&mut l), ManifestStatus::Ready);
        assert!(
            !http
                .requests
                .borrow()
                .iter()
                .any(|u| u.ends_with("manifest"))
        );
    }

    #[test]
    fn manifest_failures() {
        let http = Rc::new(FakeHttp::default());
        let mut l = loader(&http);
        l.load_manifest(None);
        assert_eq!(wait_manifest(&mut l), ManifestStatus::Failed);
        assert!(matches!(
            l.manifest_error(),
            Some(AssetError::ManifestUnavailable(404))
        ));

        http.ok(&format!("{BASE}manifest"), b"not a hash");
        l.load_manifest(None);
        wait_manifest(&mut l);
        assert!(matches!(
            l.manifest_error(),
            Some(AssetError::ManifestPointer(_))
        ));

        let garbage = hash_of(b"garbage");
        http.ok(&format!("{BASE}manifest"), garbage.to_string().as_bytes());
        l.load_manifest(None);
        wait_manifest(&mut l);
        assert!(matches!(
            l.manifest_error(),
            Some(AssetError::BlobNotFound(h)) if *h == garbage
        ));

        http.ok(&format!("{BASE}{garbage}"), b"garbage");
        l.load_manifest(None);
        wait_manifest(&mut l);
        assert!(matches!(
            l.manifest_error(),
            Some(AssetError::ManifestFormat(_))
        ));
    }

    #[test]
    fn request_waits_for_manifest_then_picks_variant_by_profile() {
        let http = server(1);
        let mut l = loader(&http);
        l.load_manifest(None);
        let bc = PlatformProfile::new(vec![VariantFormat::Bc]);
        let h1 = l.request(LogicalId::new("tex:a"), &bc);
        let h2 = l.request(
            LogicalId::new("tex:a"),
            &PlatformProfile::uncompressed_only(),
        );
        assert!(matches!(l.poll(h1), AssetPoll::Pending));
        assert!(matches!(wait(&mut l, h1), AssetPoll::Ready(b) if b == b"bc"));
        assert!(matches!(wait(&mut l, h2), AssetPoll::Ready(b) if b == b"rgba"));
        // Delivered once.
        assert!(matches!(
            l.poll(h1),
            AssetPoll::Failed(AssetError::UnknownHandle)
        ));
    }

    #[test]
    fn request_errors() {
        let http = server(0);
        let mut l = loader(&http);
        let p = PlatformProfile::uncompressed_only();

        let early = l.request(LogicalId::new("tex:a"), &p);
        assert!(matches!(
            l.poll(early),
            AssetPoll::Failed(AssetError::ManifestNotLoaded)
        ));

        l.load_manifest(None);
        assert_eq!(wait_manifest(&mut l), ManifestStatus::Ready);
        let unknown = l.request(LogicalId::new("tex:nope"), &p);
        assert!(matches!(
            wait(&mut l, unknown),
            AssetPoll::Failed(AssetError::UnknownAsset(_))
        ));
        let no_variant = l.request(LogicalId::new("tex:bc-only"), &p);
        assert!(matches!(
            wait(&mut l, no_variant),
            AssetPoll::Failed(AssetError::NoVariant(_))
        ));

        // Listed in the manifest, missing on the server.
        let missing = l.request(
            LogicalId::new("tex:bc-only"),
            &PlatformProfile::new(vec![VariantFormat::Bc]),
        );
        assert!(matches!(
            wait(&mut l, missing),
            AssetPoll::Failed(AssetError::BlobNotFound(h)) if h == hash(3)
        ));
    }

    #[test]
    fn cancel_forgets_request() {
        let http = server(0);
        let mut l = loader(&http);
        l.load_manifest(None);
        wait_manifest(&mut l);
        let h = l.request(
            LogicalId::new("tex:a"),
            &PlatformProfile::uncompressed_only(),
        );
        l.cancel(h);
        assert!(matches!(
            l.poll(h),
            AssetPoll::Failed(AssetError::UnknownHandle)
        ));
    }

    #[test]
    fn announce_lists_every_variant_once_per_manifest() {
        use std::cell::RefCell;
        type Seen = Vec<(usize, u64, u64, bool)>;
        struct Spy(Rc<FakeHttp>, Rc<RefCell<Seen>>, ContentHash);
        impl HttpClient for Spy {
            fn get(&self, url: &str) -> Box<dyn HttpOp> {
                self.0.get(url)
            }
            fn manifest_ready(&self, live: HashSet<String>, files: u64, bytes: u64) {
                let has_manifest = live.contains(&self.2.to_string());
                self.1
                    .borrow_mut()
                    .push((live.len(), files, bytes, has_manifest));
            }
        }
        let http = server(0);
        let seen = Rc::default();
        let mhash = hash_of(&manifest().encode());
        let spy = Rc::new(Spy(http, Rc::clone(&seen), mhash));
        let mut l = AssetLoader::new(spy.clone(), BASE, Box::new(HttpSource::new(spy, BASE)));
        let p = PlatformProfile::uncompressed_only();
        l.announce_manifest(&p);
        assert!(seen.borrow().is_empty(), "nothing before the manifest");
        l.load_manifest(None);
        wait_manifest(&mut l);
        l.announce_manifest(&p);
        l.announce_manifest(&p);
        // Live: bc, rgba, bc-only, the manifest. This profile fetches `tex:a` only.
        assert_eq!(*seen.borrow(), [(4, 1, 10, true)]);
    }
}
