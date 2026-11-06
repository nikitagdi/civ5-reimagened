//! Asset packs of the client mods (T33): fetched from the server by hash, verified, kept per mod.
//!
//! The server names a mod's pack in `ModView::pack` (the sha256 of the whole blob) and serves it at
//! `GET /mods/{hash}` (T32). [`PackFetch`] is one pollable download (the platform's `HttpClient`, no `async`, D8); the
//! blob goes through `mod_assets::load`, which checks the hash of the whole pack, then the size and sha256 of every
//! file. A pack with one byte changed is refused; no half-checked pack ever reaches a mod.
//!
//! [`PackSet`] holds the packs of the running game's mods and implements `mod_client::ModAssets` for the mod the host
//! has selected (`select`). What it reports is one of `Pending`, `Ready`, `Missing` (the server has no pack: the
//! signal of `mod-assets` for "not built") or `Failed` (refused). What to do with the last two is the hook's call:
//! an optional mod is switched off with a log line, a required one makes a readable error (see `ModProblem`).

use std::collections::BTreeMap;
use std::rc::Rc;

use mod_assets::{Pack, PackError, Sha256};
use mod_client::{ModAssets, PackState};
use mod_manifest::ModId;
use platform::{HttpClient, HttpOp};
use protocol::ModSetView;

/// Why a pack download did not give a pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchError {
    /// The server has no pack under this hash (404): not built, not installed.
    Missing,
    /// Network failure, a bad status, or the blob did not pass `mod-assets` (the text names the file or the hash).
    Failed(String),
}

/// One download of a pack: `GET <base><hash>`, checked against the hash.
pub struct PackFetch {
    http: Rc<dyn HttpClient>,
    url: String,
    op: Box<dyn HttpOp>,
    hash: Sha256,
}

impl PackFetch {
    /// `base` ends with `/` (`http://host/mods/`).
    pub fn start(http: Rc<dyn HttpClient>, base: &str, hash: Sha256) -> Self {
        let url = format!("{base}{hash}");
        Self {
            op: http.get(&url),
            http,
            url,
            hash,
        }
    }

    /// `None` while the answer is on its way.
    pub fn poll(&mut self) -> Option<Result<Pack, FetchError>> {
        let result = self.op.poll()?;
        Some(match result {
            Err(e) => Err(FetchError::Failed(e.to_string())),
            Ok(resp) if resp.status == 404 => Err(FetchError::Missing),
            Ok(resp) if !resp.is_success() => {
                Err(FetchError::Failed(format!("HTTP {}", resp.status)))
            }
            Ok(resp) => match mod_assets::load(Some(&resp.body), Some(&self.hash), None) {
                Ok(pack) => Ok(pack),
                Err(PackError::Missing) => Err(FetchError::Missing),
                Err(e) => {
                    // The cached copy is no good: the next try goes to the server.
                    self.http.forget(&self.url);
                    Err(FetchError::Failed(e.to_string()))
                }
            },
        })
    }
}

/// `ws://host:port/ws` -> `http://host:port/mods/` (the route of T32); `wss` maps to `https`.
pub fn mods_base_from_ws_url(ws_url: &str) -> Option<String> {
    let assets = crate::asset_boot::assets_base_from_ws_url(ws_url)?;
    Some(format!("{}mods/", assets.strip_suffix("assets/")?))
}

struct Slot {
    id: ModId,
    required: bool,
    fetch: Option<PackFetch>,
    state: PackState,
    pack: Option<Pack>,
    /// The state changed since `poll` last reported it.
    fresh: bool,
}

/// A pack that reached a final state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackEvent {
    pub mod_idx: u16,
    pub id: ModId,
    /// The game cannot do without the mod on this client (`ModView::client_required`).
    pub required: bool,
    /// `Ready`, `Missing` or `Failed(why)`.
    pub state: PackState,
}

/// The packs of the mods of the running game, by `mod_idx`.
pub struct PackSet {
    http: Rc<dyn HttpClient>,
    /// `None`: the server address has no HTTP form (no pack can be fetched).
    base: Option<String>,
    slots: BTreeMap<u16, Slot>,
    cur: u16,
}

impl PackSet {
    pub fn new(http: Rc<dyn HttpClient>, base: Option<String>) -> Self {
        Self {
            http,
            base,
            slots: BTreeMap::new(),
            cur: 0,
        }
    }

    /// Forgets the packs of the previous set and starts the downloads of the new one: every mod the client has
    /// (`available`) that names a pack. A mod that names none has no pack (`Missing`), which only matters to a mod
    /// that asks for one.
    pub fn start(&mut self, set: &ModSetView) {
        self.slots.clear();
        for m in set.mods.iter().filter(|m| m.available) {
            let Ok(id) = ModId::new(&m.id) else { continue };
            let (fetch, state) = match (&m.pack, &self.base) {
                (None, _) => (None, PackState::Missing),
                (Some(_), None) => (
                    None,
                    PackState::Failed("the server address cannot be reached over HTTP".into()),
                ),
                (Some(h), Some(base)) => match h.parse::<Sha256>() {
                    Ok(hash) => (
                        Some(PackFetch::start(Rc::clone(&self.http), base, hash)),
                        PackState::Pending,
                    ),
                    Err(_) => (
                        None,
                        PackState::Failed(format!("the pack hash `{h}` is not a sha256")),
                    ),
                },
            };
            self.slots.insert(
                m.load_idx,
                Slot {
                    id,
                    required: m.client_required,
                    fetch,
                    fresh: state != PackState::Pending,
                    state,
                    pack: None,
                },
            );
        }
    }

    /// Drops every pack (the game ended).
    pub fn clear(&mut self) {
        self.slots.clear();
    }

    /// Advances the downloads; returns the packs that reached a final state since the last call.
    pub fn poll(&mut self) -> Vec<PackEvent> {
        let mut out = Vec::new();
        for (&mod_idx, s) in &mut self.slots {
            if let Some(f) = s.fetch.as_mut()
                && let Some(r) = f.poll()
            {
                s.fetch = None;
                s.fresh = true;
                match r {
                    Ok(pack) => {
                        s.pack = Some(pack);
                        s.state = PackState::Ready;
                    }
                    Err(FetchError::Missing) => s.state = PackState::Missing,
                    Err(FetchError::Failed(why)) => s.state = PackState::Failed(why),
                }
            }
            if std::mem::take(&mut s.fresh) {
                out.push(PackEvent {
                    mod_idx,
                    id: s.id.clone(),
                    required: s.required,
                    state: s.state.clone(),
                });
            }
        }
        out
    }

    /// The mod the next `ModAssets` calls are about.
    pub fn select(&mut self, mod_idx: u16) {
        self.cur = mod_idx;
    }

    /// State of the pack of `mod_idx` (`Missing` for a mod with no slot).
    pub fn state_of(&self, mod_idx: u16) -> PackState {
        self.slots
            .get(&mod_idx)
            .map_or(PackState::Missing, |s| s.state.clone())
    }
}

impl ModAssets for PackSet {
    fn state(&self) -> PackState {
        self.state_of(self.cur)
    }

    fn get(&self, id: &str) -> Option<&[u8]> {
        self.slots.get(&self.cur)?.pack.as_ref()?.get(id)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use mod_assets::{AssetKind, NewFile};
    use platform::{HttpError, HttpResponse};
    use protocol::{ModSync, ModView};
    use std::cell::RefCell;
    use std::collections::HashMap;

    /// Canned answers by URL (unknown URLs are 404), answered after `delay` polls.
    #[derive(Default)]
    pub(crate) struct FakeHttp {
        pub routes: RefCell<HashMap<String, Result<HttpResponse, HttpError>>>,
        pub requests: RefCell<Vec<String>>,
        pub forgotten: RefCell<Vec<String>>,
        pub delay: u32,
    }

    struct Op(u32, Option<Result<HttpResponse, HttpError>>);

    impl HttpOp for Op {
        fn poll(&mut self) -> Option<Result<HttpResponse, HttpError>> {
            if self.0 > 0 {
                self.0 -= 1;
                return None;
            }
            self.1.take()
        }
    }

    impl HttpClient for FakeHttp {
        fn get(&self, url: &str) -> Box<dyn HttpOp> {
            self.requests.borrow_mut().push(url.to_owned());
            let r = self
                .routes
                .borrow()
                .get(url)
                .cloned()
                .unwrap_or(Ok(HttpResponse {
                    status: 404,
                    body: Vec::new(),
                }));
            Box::new(Op(self.delay, Some(r)))
        }

        fn forget(&self, url: &str) {
            self.forgotten.borrow_mut().push(url.to_owned());
        }
    }

    impl FakeHttp {
        pub(crate) fn serve(&self, base: &str, bytes: &[u8]) -> Sha256 {
            let hash = Sha256::of(bytes);
            self.routes.borrow_mut().insert(
                format!("{base}{hash}"),
                Ok(HttpResponse {
                    status: 200,
                    body: bytes.to_vec(),
                }),
            );
            hash
        }
    }

    pub(crate) fn pack_bytes(mod_id: &str) -> Vec<u8> {
        let files = vec![
            NewFile {
                id: "data/blocks".into(),
                kind: AssetKind::Raw,
                bytes: b"dirt".to_vec(),
            },
            NewFile {
                id: "sounds/dig".into(),
                kind: AssetKind::Sound,
                bytes: vec![9; 40],
            },
        ];
        Pack::build(ModId::new(mod_id).unwrap(), Vec::new(), files)
            .unwrap()
            .to_bytes()
    }

    const BASE: &str = "http://h/mods/";

    pub(crate) fn view(idx: u16, id: &str, pack: Option<&Sha256>, required: bool) -> ModView {
        ModView {
            id: id.into(),
            version: "1.0.0".into(),
            load_idx: idx,
            sync: ModSync::Cosmetic,
            client_required: required,
            available: true,
            pack: pack.map(ToString::to_string),
        }
    }

    fn set(mods: Vec<ModView>) -> ModSetView {
        ModSetView {
            modset_hash: 1,
            mods,
        }
    }

    fn run(p: &mut PackSet) -> Vec<PackEvent> {
        for _ in 0..8 {
            let ev = p.poll();
            if !ev.is_empty() {
                return ev;
            }
        }
        panic!("no pack settled");
    }

    #[test]
    fn the_base_is_the_mods_route_of_the_server() {
        assert_eq!(
            mods_base_from_ws_url("ws://localhost:8080/ws").as_deref(),
            Some("http://localhost:8080/mods/")
        );
        assert_eq!(
            mods_base_from_ws_url("wss://play.example.org/ws?x=1").as_deref(),
            Some("https://play.example.org/mods/")
        );
        assert_eq!(mods_base_from_ws_url("ftp://x"), None);
    }

    #[test]
    fn a_pack_is_fetched_by_hash_and_read_by_id() {
        let http = Rc::new(FakeHttp {
            delay: 2,
            ..Default::default()
        });
        let hash = http.serve(BASE, &pack_bytes("hello"));
        let mut p = PackSet::new(http.clone(), Some(BASE.into()));
        p.start(&set(vec![view(3, "hello", Some(&hash), false)]));
        p.select(3);
        assert_eq!(p.state(), PackState::Pending);
        assert_eq!(p.get("data/blocks"), None, "nothing before Ready");
        assert!(p.poll().is_empty() && p.poll().is_empty());
        let ev = run(&mut p);
        assert_eq!(ev.len(), 1);
        assert_eq!((ev[0].mod_idx, &ev[0].state), (3, &PackState::Ready));
        assert_eq!(p.state(), PackState::Ready);
        assert_eq!(p.get("data/blocks"), Some(&b"dirt"[..]));
        assert_eq!(p.get("nope"), None);
        assert_eq!(*http.requests.borrow(), [format!("{BASE}{hash}")]);
        // Reported once.
        assert!(p.poll().is_empty());
        // Another mod's slot does not leak.
        p.select(0);
        assert_eq!(p.get("data/blocks"), None);
    }

    #[test]
    fn a_changed_byte_refuses_the_pack() {
        let mut bytes = pack_bytes("hello");
        let hash = Sha256::of(&bytes);
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        let http = Rc::new(FakeHttp::default());
        // The server answers the hash with a blob that no longer has it.
        http.routes.borrow_mut().insert(
            format!("{BASE}{hash}"),
            Ok(HttpResponse {
                status: 200,
                body: bytes,
            }),
        );
        let mut p = PackSet::new(http.clone(), Some(BASE.into()));
        p.start(&set(vec![view(0, "hello", Some(&hash), true)]));
        let ev = run(&mut p);
        assert!(ev[0].required);
        assert!(
            matches!(&ev[0].state, PackState::Failed(why) if why.contains("pack hash")),
            "{:?}",
            ev[0].state
        );
        assert_eq!(p.get("data/blocks"), None);
        assert_eq!(http.forgotten.borrow().len(), 1, "the bad copy is dropped");
    }

    #[test]
    fn a_file_changed_inside_a_pack_with_the_right_hash_still_fails() {
        // A server that lies about the hash of the whole blob cannot get a pack past the per-file check either.
        let mut bytes = pack_bytes("hello");
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert!(matches!(
            Pack::parse(&bytes),
            Err(PackError::Corrupt { .. })
        ));
    }

    #[test]
    fn no_pack_on_the_server_is_missing_not_an_error() {
        let http = Rc::new(FakeHttp::default());
        let ghost = Sha256::of(b"never served");
        let mut p = PackSet::new(http, Some(BASE.into()));
        p.start(&set(vec![
            view(0, "alpha", Some(&ghost), false),
            view(1, "bravo", None, false),
        ]));
        let events = run(&mut p);
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|e| e.state == PackState::Missing));
        p.select(1);
        assert_eq!(p.state(), PackState::Missing);
    }

    #[test]
    fn network_failures_and_server_errors_fail_the_pack() {
        let http = Rc::new(FakeHttp::default());
        let a = Sha256::of(b"a");
        let b = Sha256::of(b"b");
        http.routes.borrow_mut().insert(
            format!("{BASE}{a}"),
            Err(HttpError::Network("reset".into())),
        );
        http.routes.borrow_mut().insert(
            format!("{BASE}{b}"),
            Ok(HttpResponse {
                status: 500,
                body: Vec::new(),
            }),
        );
        let mut p = PackSet::new(http, Some(BASE.into()));
        p.start(&set(vec![
            view(0, "alpha", Some(&a), false),
            view(1, "bravo", Some(&b), false),
        ]));
        let ev = run(&mut p);
        assert!(ev.iter().all(|e| matches!(e.state, PackState::Failed(_))));
    }

    #[test]
    fn mods_the_client_lacks_get_no_slot_and_a_new_set_forgets_the_old() {
        let http = Rc::new(FakeHttp::default());
        let h = http.serve(BASE, &pack_bytes("hello"));
        let mut p = PackSet::new(http, Some(BASE.into()));
        let mut absent = view(1, "other", Some(&h), false);
        absent.available = false;
        p.start(&set(vec![view(0, "hello", Some(&h), false), absent]));
        run(&mut p);
        p.select(0);
        assert_eq!(p.state(), PackState::Ready);
        p.select(1);
        assert_eq!(p.state(), PackState::Missing);
        p.start(&set(Vec::new()));
        p.select(0);
        assert_eq!(p.state(), PackState::Missing);
        assert_eq!(p.get("data/blocks"), None);
    }
}
