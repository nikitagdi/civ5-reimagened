//! Game data in memory and its loading by manifest.
//!
//! No I/O here (R4): [`load_game_files`] takes the transport as a closure, so the browser worker
//! passes `fetch()` and the tests pass a fake. The steps:
//!
//! 1. `<base>manifest` holds the hash of the current manifest blob (`manifest_hash` skips this);
//! 2. `<base><hash>` is the manifest (`asset-format`);
//! 3. `data:vfs-index` of the manifest is `vfs-index.json`: Lua files with their sizes, ids of
//!    both databases;
//! 4. every Lua file and both databases are fetched by the hash of their manifest entry
//!    (a few requests at a time) and stored in [`GameFiles`] under the keys `rt_vfs` and
//!    `read_file` use: the `path` of the index, `vfs-index.json`, `db/gameplay.db`,
//!    `db/loc-en_us.db`.
//!
//! Every blob is checked against the size the manifest records (and, for Lua files, the size the
//! index records). Checking the content hash (blake3) needs a hashing dependency in the wasm
//! build, which is not approved yet: a blob with the right size and wrong content is not caught.

use std::collections::HashMap;
use std::fmt;
use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;

use asset_format::{ContentHash, LogicalId, Manifest};
use serde::Deserialize;

/// Manifest id of the index.
pub const INDEX_ID: &str = "data:vfs-index";
/// Key of the index in [`GameFiles`].
pub const INDEX_PATH: &str = "vfs-index.json";
/// Keys of the two databases in [`GameFiles`] (the paths of the folder export).
pub const GAMEPLAY_DB_PATH: &str = "db/gameplay.db";
pub const LOC_DB_PATH: &str = "db/loc-en_us.db";
/// `vfs-index.json` format this code reads.
pub const INDEX_FORMAT: u32 = 1;
/// Requests in flight at a time.
pub const DEFAULT_CONCURRENCY: usize = 8;

/// Game data files held in memory, keyed by the paths of the `game-data` index.
#[derive(Debug, Clone, Default)]
pub struct GameFiles(HashMap<String, Arc<[u8]>>);

impl GameFiles {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, path: impl Into<String>, bytes: impl Into<Arc<[u8]>>) {
        self.0.insert(path.into(), bytes.into());
    }

    pub fn get(&self, path: &str) -> Option<&Arc<[u8]>> {
        self.0.get(path)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Sum of the sizes of all files.
    pub fn total_bytes(&self) -> u64 {
        self.0.values().map(|b| b.len() as u64).sum()
    }

    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }
}

/// Why the game data could not be loaded. The text goes to the player as is (AC-9.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    /// Pointer, manifest or index is unreadable or inconsistent.
    Manifest(String),
    /// The manifest has no entry for a file the index lists.
    MissingInManifest { path: String, id: String },
    /// The server does not have the blob.
    NotFound { path: String, hash: String },
    /// The request failed or the server answered with an error.
    Fetch { what: String, message: String },
    /// The blob is not the file the manifest (or index) describes.
    Corrupt {
        path: String,
        expected: u64,
        actual: u64,
    },
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Manifest(m) => write!(f, "game data: {m}"),
            Self::MissingInManifest { path, id } => {
                write!(f, "game data: {path} ({id}) is not in the manifest")
            }
            Self::NotFound { path, hash } => {
                write!(
                    f,
                    "game data: {path} is missing on the server (blob {hash})"
                )
            }
            Self::Fetch { what, message } => write!(f, "game data: {what}: {message}"),
            Self::Corrupt {
                path,
                expected,
                actual,
            } => write!(
                f,
                "game data: {path} is corrupt: {actual} bytes, expected {expected}"
            ),
        }
    }
}

impl std::error::Error for LoadError {}

/// One file of `vfs-index.json`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct IndexFile {
    pub path: String,
    pub id: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct IndexDb {
    gameplay: String,
    localization: String,
}

/// The part of `vfs-index.json` the loader needs; the rest (include roots, contexts, ...) is for
/// `rt_vfs`, which reads the same bytes from [`GameFiles`].
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DataIndex {
    format: u32,
    db: IndexDb,
    pub files: Vec<IndexFile>,
}

impl DataIndex {
    pub fn parse(bytes: &[u8]) -> Result<Self, LoadError> {
        let bad = |m: String| LoadError::Manifest(format!("{INDEX_PATH}: {m}"));
        let index: Self = serde_json::from_slice(bytes).map_err(|e| bad(e.to_string()))?;
        if index.format != INDEX_FORMAT {
            return Err(bad(format!(
                "format {} (expected {INDEX_FORMAT})",
                index.format
            )));
        }
        let mut seen = std::collections::HashSet::new();
        for f in &index.files {
            if f.path.is_empty() || !seen.insert(f.path.as_str()) {
                return Err(bad(format!("empty or duplicate path {:?}", f.path)));
            }
        }
        for reserved in [INDEX_PATH, GAMEPLAY_DB_PATH, LOC_DB_PATH] {
            if seen.contains(reserved) {
                return Err(bad(format!("{reserved} is a reserved path")));
            }
        }
        Ok(index)
    }
}

/// A file to fetch: where it goes, which blob it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileJob {
    /// Key in [`GameFiles`].
    pub path: String,
    pub hash: ContentHash,
    pub size: u64,
}

fn job(
    manifest: &Manifest,
    path: &str,
    id: &str,
    index_size: Option<u64>,
) -> Result<FileJob, LoadError> {
    let variant = manifest
        .variants(&LogicalId::new(id))
        .first()
        .ok_or_else(|| LoadError::MissingInManifest {
            path: path.to_owned(),
            id: id.to_owned(),
        })?;
    if let Some(size) = index_size
        && size != variant.size
    {
        return Err(LoadError::Manifest(format!(
            "{path} has size {} in the manifest, {size} in {INDEX_PATH}",
            variant.size
        )));
    }
    Ok(FileJob {
        path: path.to_owned(),
        hash: variant.hash,
        size: variant.size,
    })
}

/// Everything the index lists besides the index itself: both databases, then the Lua files in
/// index order. Fails on the first file the manifest lacks or describes with another size.
pub fn plan(index: &DataIndex, manifest: &Manifest) -> Result<Vec<FileJob>, LoadError> {
    let mut jobs = Vec::with_capacity(index.files.len() + 2);
    jobs.push(job(manifest, GAMEPLAY_DB_PATH, &index.db.gameplay, None)?);
    jobs.push(job(manifest, LOC_DB_PATH, &index.db.localization, None)?);
    for f in &index.files {
        jobs.push(job(manifest, &f.path, &f.id, Some(f.size))?);
    }
    Ok(jobs)
}

/// The blob must be exactly as long as the manifest says.
pub fn check_blob(job: &FileJob, bytes: &[u8]) -> Result<(), LoadError> {
    if bytes.len() as u64 == job.size {
        Ok(())
    } else {
        Err(LoadError::Corrupt {
            path: job.path.clone(),
            expected: job.size,
            actual: bytes.len() as u64,
        })
    }
}

/// An HTTP answer, as far as the loader cares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    pub status: u16,
    pub body: Vec<u8>,
}

/// After every file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub files_done: usize,
    pub files_total: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

/// What was loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadReport {
    pub files: usize,
    pub bytes: u64,
}

/// Runs `make(item)` for all items, at most `limit` at a time, and hands every output to `done`
/// in completion order. Stops at the first `Err` of `done`; unfinished futures are dropped.
async fn for_each_windowed<T, Fut, E>(
    items: Vec<T>,
    limit: usize,
    make: impl Fn(T) -> Fut,
    mut done: impl FnMut(Fut::Output) -> Result<(), E>,
) -> Result<(), E>
where
    Fut: Future,
{
    let limit = limit.max(1);
    let mut items = items.into_iter();
    let mut active: Vec<Pin<Box<Fut>>> = Vec::new();
    poll_fn(|cx| {
        loop {
            while active.len() < limit {
                match items.next() {
                    Some(item) => active.push(Box::pin(make(item))),
                    None => break,
                }
            }
            if active.is_empty() {
                return Poll::Ready(Ok(()));
            }
            let mut finished = false;
            let mut i = 0;
            while i < active.len() {
                if let Poll::Ready(out) = active[i].as_mut().poll(cx) {
                    drop(active.swap_remove(i));
                    if let Err(e) = done(out) {
                        return Poll::Ready(Err(e));
                    }
                    finished = true;
                } else {
                    i += 1;
                }
            }
            if !finished {
                return Poll::Pending;
            }
        }
    })
    .await
}

/// Loads the whole game data package from `base` (ends with `/`, e.g. `/assets/`).
///
/// `fetch` does one GET of an absolute or page-relative URL; `Err` is a failed request (text for
/// the player), a 404 is `Ok` with `status: 404`. `manifest_hash` (64 hex digits) skips the
/// pointer. `progress` is called once the index is read and after every file.
pub async fn load_game_files<F, Fut>(
    fetch: F,
    base: &str,
    manifest_hash: Option<&str>,
    concurrency: usize,
    mut progress: impl FnMut(Progress),
) -> Result<(GameFiles, LoadReport), LoadError>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<Fetched, String>>,
{
    let base = if base.ends_with('/') {
        base.to_owned()
    } else {
        format!("{base}/")
    };
    let get = |what: String, url: String| {
        let fetch = &fetch;
        async move {
            fetch(url).await.map_err(|message| LoadError::Fetch {
                what: what.clone(),
                message,
            })
        }
    };

    let hash: ContentHash = match manifest_hash {
        Some(h) => h.trim().parse(),
        None => {
            let r = get("manifest pointer".into(), format!("{base}manifest")).await?;
            if r.status != 200 {
                return Err(LoadError::Manifest(format!(
                    "no manifest on the server (HTTP {})",
                    r.status
                )));
            }
            String::from_utf8_lossy(&r.body).trim().parse()
        }
    }
    .map_err(|e| LoadError::Manifest(format!("manifest pointer is not a hash: {e}")))?;

    let r = get("manifest".into(), format!("{base}{hash}")).await?;
    if r.status != 200 {
        return Err(LoadError::Manifest(format!(
            "manifest blob {hash} not available (HTTP {})",
            r.status
        )));
    }
    let manifest = Manifest::decode(&r.body)
        .map_err(|e| LoadError::Manifest(format!("manifest {hash}: {e}")))?;

    let index_job = job(&manifest, INDEX_PATH, INDEX_ID, None)?;
    let r = get(INDEX_PATH.into(), format!("{base}{}", index_job.hash)).await?;
    match r.status {
        200 => {}
        404 => {
            return Err(LoadError::NotFound {
                path: INDEX_PATH.into(),
                hash: index_job.hash.to_string(),
            });
        }
        s => {
            return Err(LoadError::Fetch {
                what: INDEX_PATH.into(),
                message: format!("HTTP {s}"),
            });
        }
    }
    check_blob(&index_job, &r.body)?;
    let index = DataIndex::parse(&r.body)?;
    let jobs = plan(&index, &manifest)?;

    let mut files = GameFiles::new();
    let mut p = Progress {
        files_done: 1,
        files_total: jobs.len() + 1,
        bytes_done: r.body.len() as u64,
        bytes_total: jobs.iter().map(|j| j.size).sum::<u64>() + index_job.size,
    };
    files.insert(INDEX_PATH, r.body);
    progress(p);

    for_each_windowed(
        jobs,
        concurrency,
        |job: FileJob| {
            let url = format!("{base}{}", job.hash);
            let fetched = get(job.path.clone(), url);
            async move { fetched.await.map(|r| (job, r)) }
        },
        |out| {
            let (job, r) = out?;
            match r.status {
                200 => {}
                404 => {
                    return Err(LoadError::NotFound {
                        path: job.path,
                        hash: job.hash.to_string(),
                    });
                }
                s => {
                    return Err(LoadError::Fetch {
                        what: job.path,
                        message: format!("HTTP {s}"),
                    });
                }
            }
            check_blob(&job, &r.body)?;
            p.files_done += 1;
            p.bytes_done += job.size;
            files.insert(job.path, r.body);
            progress(p);
            Ok(())
        },
    )
    .await?;

    let report = LoadReport {
        files: files.len(),
        bytes: files.total_bytes(),
    };
    Ok((files, report))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::task::{Context, Waker};

    use asset_format::{PrefetchGroup, Variant, VariantFormat};

    use super::*;

    fn block_on<T>(fut: impl Future<Output = T>) -> T {
        let mut fut = Box::pin(fut);
        let mut cx = Context::from_waker(Waker::noop());
        for _ in 0..10_000 {
            if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
                return v;
            }
        }
        panic!("future did not finish");
    }

    /// Pending for `n` polls, like a network round trip.
    struct Delay(u32);
    impl Future for Delay {
        type Output = ();
        fn poll(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
            if self.0 == 0 {
                Poll::Ready(())
            } else {
                self.0 -= 1;
                Poll::Pending
            }
        }
    }

    fn hash(n: u8) -> ContentHash {
        ContentHash([n; 32])
    }

    fn variant(n: u8, size: u64) -> Variant {
        Variant {
            format: VariantFormat::Generic,
            hash: hash(n),
            size,
            prefetch: PrefetchGroup(0),
        }
    }

    fn index_json(files: &[(&str, u64)]) -> Vec<u8> {
        let list: Vec<String> = files
            .iter()
            .map(|(p, s)| format!(r#"{{"path":"{p}","id":"lua:{p}","size":{s}}}"#))
            .collect();
        format!(
            r#"{{"format":1,"snapshot":{{}},"db":{{"gameplay":"data:gameplay-db","localization":"data:loc-en_us"}},"include_roots":[],"files":[{}]}}"#,
            list.join(",")
        )
        .into_bytes()
    }

    /// A fake server: blobs by URL, requests logged.
    struct Server {
        routes: RefCell<HashMap<String, Result<Fetched, String>>>,
        requests: RefCell<Vec<String>>,
        in_flight: RefCell<(u32, u32)>,
    }

    impl Server {
        fn put(&self, url: &str, body: &[u8]) {
            self.routes.borrow_mut().insert(
                url.to_owned(),
                Ok(Fetched {
                    status: 200,
                    body: body.to_vec(),
                }),
            );
        }

        fn fetch(self: &Rc<Self>, url: String) -> impl Future<Output = Result<Fetched, String>> {
            let me = Rc::clone(self);
            async move {
                me.requests.borrow_mut().push(url.clone());
                {
                    let mut f = me.in_flight.borrow_mut();
                    f.0 += 1;
                    f.1 = f.1.max(f.0);
                }
                Delay(2).await;
                me.in_flight.borrow_mut().0 -= 1;
                me.routes.borrow().get(&url).cloned().unwrap_or(Ok(Fetched {
                    status: 404,
                    body: vec![],
                }))
            }
        }
    }

    const LUA: [(&str, &[u8]); 3] = [
        ("assets/a.lua", b"print(1)"),
        ("assets/b/c.lua", b"x = 2\n"),
        ("assets/d.lua", b""),
    ];

    /// Manifest at hash(1), index at hash(2), DBs hash(3), hash(4), Lua files from hash(10).
    fn server() -> Rc<Server> {
        let srv = Rc::new(Server {
            routes: RefCell::default(),
            requests: RefCell::default(),
            in_flight: RefCell::default(),
        });
        let listing: Vec<(&str, u64)> = LUA.iter().map(|(p, b)| (*p, b.len() as u64)).collect();
        let index = index_json(&listing);
        let mut m = Manifest::new();
        m.insert(LogicalId::new(INDEX_ID), variant(2, index.len() as u64));
        m.insert(LogicalId::new("data:gameplay-db"), variant(3, 5));
        m.insert(LogicalId::new("data:loc-en_us"), variant(4, 3));
        for (i, (p, b)) in LUA.iter().enumerate() {
            m.insert(
                LogicalId::new(format!("lua:{p}")),
                variant(10 + i as u8, b.len() as u64),
            );
            srv.put(&format!("/assets/{}", hash(10 + i as u8)), b);
        }
        srv.put("/assets/manifest", format!("{}\n", hash(1)).as_bytes());
        srv.put(&format!("/assets/{}", hash(1)), &m.encode());
        srv.put(&format!("/assets/{}", hash(2)), &index);
        srv.put(&format!("/assets/{}", hash(3)), b"gamep");
        srv.put(&format!("/assets/{}", hash(4)), b"loc");
        srv
    }

    fn run(
        srv: &Rc<Server>,
        hash: Option<&str>,
        limit: usize,
    ) -> (Result<(GameFiles, LoadReport), LoadError>, Vec<Progress>) {
        let mut seen = Vec::new();
        let r = block_on(load_game_files(
            |u| srv.fetch(u),
            "/assets",
            hash,
            limit,
            |p| seen.push(p),
        ));
        (r, seen)
    }

    #[test]
    fn loads_every_file_under_the_keys_of_the_index() {
        let srv = server();
        let (r, seen) = run(&srv, None, DEFAULT_CONCURRENCY);
        let (files, report) = r.unwrap();
        for (p, b) in LUA {
            assert_eq!(&files.get(p).unwrap()[..], b, "{p}");
        }
        assert_eq!(&files.get(GAMEPLAY_DB_PATH).unwrap()[..], b"gamep");
        assert_eq!(&files.get(LOC_DB_PATH).unwrap()[..], b"loc");
        assert!(files.get(INDEX_PATH).is_some());
        assert_eq!(report.files, 6);
        assert_eq!(report.bytes, files.total_bytes());
        assert_eq!(files.paths().count(), 6);

        // progress: after the index, then once per file, monotonic, ending complete
        assert_eq!(seen.len(), 6);
        assert_eq!(seen[0].files_done, 1);
        assert!(
            seen.windows(2)
                .all(|w| w[0].files_done + 1 == w[1].files_done)
        );
        let last = seen.last().unwrap();
        assert_eq!((last.files_done, last.files_total), (6, 6));
        assert_eq!(last.bytes_done, last.bytes_total);
        assert_eq!(last.bytes_done, report.bytes);
    }

    #[test]
    fn manifest_hash_skips_the_pointer() {
        let srv = server();
        let h = hash(1).to_string();
        let (r, _) = run(&srv, Some(&h), 4);
        r.unwrap();
        assert!(
            !srv.requests
                .borrow()
                .iter()
                .any(|u| u.ends_with("manifest"))
        );
    }

    #[test]
    fn requests_are_limited_but_overlap() {
        let srv = server();
        run(&srv, None, 2).0.unwrap();
        assert_eq!(srv.in_flight.borrow().1, 2);
        let srv = server();
        run(&srv, None, 1).0.unwrap();
        assert_eq!(srv.in_flight.borrow().1, 1);
        // zero means one, not a hang
        run(&server(), None, 0).0.unwrap();
    }

    #[test]
    fn missing_blob_names_the_file() {
        let srv = server();
        srv.routes
            .borrow_mut()
            .remove(&format!("/assets/{}", hash(11)));
        let err = run(&srv, None, 4).0.unwrap_err();
        match &err {
            LoadError::NotFound { path, hash: h } => {
                assert_eq!(path, "assets/b/c.lua");
                assert_eq!(*h, hash(11).to_string());
            }
            other => panic!("{other:?}"),
        }
        assert!(err.to_string().contains("assets/b/c.lua"));
    }

    #[test]
    fn wrong_size_is_corrupt() {
        let srv = server();
        srv.put(&format!("/assets/{}", hash(10)), b"print(12345)");
        let err = run(&srv, None, 4).0.unwrap_err();
        assert_eq!(
            err,
            LoadError::Corrupt {
                path: "assets/a.lua".into(),
                expected: 8,
                actual: 12
            }
        );
        assert!(err.to_string().contains("corrupt"));
        // the database is checked too
        let srv = server();
        srv.put(&format!("/assets/{}", hash(3)), b"x");
        assert!(matches!(
            run(&srv, None, 4).0,
            Err(LoadError::Corrupt { path, .. }) if path == GAMEPLAY_DB_PATH
        ));
    }

    #[test]
    fn server_errors_and_network_failures_are_reported() {
        let srv = server();
        srv.routes.borrow_mut().insert(
            format!("/assets/{}", hash(12)),
            Ok(Fetched {
                status: 500,
                body: vec![],
            }),
        );
        assert_eq!(
            run(&srv, None, 4).0.unwrap_err(),
            LoadError::Fetch {
                what: "assets/d.lua".into(),
                message: "HTTP 500".into()
            }
        );
        let srv = server();
        srv.routes.borrow_mut().insert(
            format!("/assets/{}", hash(4)),
            Err("connection reset".into()),
        );
        let err = run(&srv, None, 4).0.unwrap_err();
        assert_eq!(
            err,
            LoadError::Fetch {
                what: LOC_DB_PATH.into(),
                message: "connection reset".into()
            }
        );
    }

    #[test]
    fn broken_pointer_manifest_and_index_are_clear_errors() {
        let srv = server();
        srv.put("/assets/manifest", b"not a hash");
        assert!(
            matches!(run(&srv, None, 4).0, Err(LoadError::Manifest(m)) if m.contains("pointer"))
        );

        let srv = Rc::new(Server {
            routes: RefCell::default(),
            requests: RefCell::default(),
            in_flight: RefCell::default(),
        });
        assert!(matches!(run(&srv, None, 4).0, Err(LoadError::Manifest(m)) if m.contains("404")));

        let srv = server();
        srv.put(&format!("/assets/{}", hash(1)), b"garbage");
        assert!(
            matches!(run(&srv, None, 4).0, Err(LoadError::Manifest(m)) if m.contains("manifest"))
        );

        let srv = server();
        srv.routes
            .borrow_mut()
            .remove(&format!("/assets/{}", hash(2)));
        assert!(matches!(
            run(&srv, None, 4).0,
            Err(LoadError::NotFound { path, .. }) if path == INDEX_PATH
        ));
    }

    #[test]
    fn index_entry_without_manifest_entry_or_with_other_size_fails() {
        let mut m = Manifest::new();
        m.insert(LogicalId::new("data:gameplay-db"), variant(3, 5));
        m.insert(LogicalId::new("data:loc-en_us"), variant(4, 3));
        let idx = DataIndex::parse(&index_json(&[("assets/a.lua", 8)])).unwrap();
        assert_eq!(
            plan(&idx, &m).unwrap_err(),
            LoadError::MissingInManifest {
                path: "assets/a.lua".into(),
                id: "lua:assets/a.lua".into()
            }
        );
        m.insert(LogicalId::new("lua:assets/a.lua"), variant(9, 7));
        assert!(matches!(plan(&idx, &m), Err(LoadError::Manifest(t)) if t.contains("size")));
        let mut m2 = Manifest::new();
        m2.insert(LogicalId::new("data:loc-en_us"), variant(4, 3));
        assert!(matches!(
            plan(&idx, &m2),
            Err(LoadError::MissingInManifest { path, .. }) if path == GAMEPLAY_DB_PATH
        ));
    }

    #[test]
    fn plan_lists_databases_then_lua_in_index_order() {
        let srv = server();
        let routes = srv.routes.borrow();
        let m = Manifest::decode(
            &routes[&format!("/assets/{}", hash(1))]
                .clone()
                .unwrap()
                .body,
        )
        .unwrap();
        let idx =
            DataIndex::parse(&index_json(&[("assets/a.lua", 8), ("assets/b/c.lua", 6)])).unwrap();
        let paths: Vec<_> = plan(&idx, &m)
            .unwrap()
            .into_iter()
            .map(|j| j.path)
            .collect();
        assert_eq!(
            paths,
            [
                GAMEPLAY_DB_PATH,
                LOC_DB_PATH,
                "assets/a.lua",
                "assets/b/c.lua"
            ]
        );
    }

    #[test]
    fn index_parsing_rejects_bad_input() {
        let err = |b: &[u8]| DataIndex::parse(b).unwrap_err().to_string();
        assert!(err(b"{").contains(INDEX_PATH));
        assert!(err(b"[]").contains(INDEX_PATH));
        let v2 = String::from_utf8(index_json(&[]))
            .unwrap()
            .replace(r#""format":1"#, r#""format":2"#);
        assert!(err(v2.as_bytes()).contains("format 2"));
        assert!(err(&index_json(&[("a.lua", 1), ("a.lua", 2)])).contains("duplicate"));
        assert!(err(&index_json(&[("", 1)])).contains("empty"));
        assert!(err(&index_json(&[("db/gameplay.db", 1)])).contains("reserved"));
        // unknown fields (include_roots, contexts, ...) are fine; the real file has plenty
        let ok = DataIndex::parse(&index_json(&[("a.lua", 1)])).unwrap();
        assert_eq!(ok.files.len(), 1);
    }

    #[test]
    fn check_blob_compares_length() {
        let j = FileJob {
            path: "p".into(),
            hash: hash(0),
            size: 3,
        };
        assert!(check_blob(&j, b"abc").is_ok());
        assert!(check_blob(&j, b"ab").is_err());
        assert!(check_blob(&j, b"abcd").is_err());
    }

    #[test]
    fn game_files_basics() {
        let mut f = GameFiles::new();
        assert!(f.is_empty());
        f.insert("a", vec![1u8, 2, 3]);
        f.insert("b", Vec::<u8>::new());
        assert_eq!((f.len(), f.total_bytes()), (2, 3));
        assert!(f.get("c").is_none());
    }
}
