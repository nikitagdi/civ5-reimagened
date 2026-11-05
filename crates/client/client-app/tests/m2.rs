//! M2 on the core stub (native, no GPU): the mode boots from a built asset set that holds the `ui` set and the
//! `gc:replica` game data, makes the replica core, and waits for the server's snapshot (there is no server here, so
//! it stays in `Joining` and draws nothing); the context filter keeps the refused contexts empty and hidden.
//! Skipped, with a message, without the built set (`CIV5_UI_ASSETS`: a folder `civ5-assets build --set ui --set
//! game-data --out <dir>` wrote).

use std::path::{Path, PathBuf};
use std::rc::Rc;

use assets::{AssetLoader, HttpSource};
use client_app::m2::{M2Env, M2Phase, M2Ui};
use draw_list::UiDrawList;
use platform::{Clock, HttpClient, HttpError, HttpOp, HttpResponse};
use ui_client::{BootPlan, BootState, LayerConfig, LayerHooks, UiBoot, UiLayer};

const SIZE: (u32, u32) = (1920, 1080);

fn set_dir() -> Option<PathBuf> {
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let candidates = [
        std::env::var_os("CIV5_UI_ASSETS").map(PathBuf::from),
        Some(here.join("../../../../b4-ui-assets")),
    ];
    for dir in candidates.into_iter().flatten() {
        if dir.join("manifest").is_file() {
            return Some(dir);
        }
    }
    eprintln!("SKIPPED: built ui set not found (set CIV5_UI_ASSETS)");
    None
}

struct DirHttp(PathBuf);

struct Done(Option<Result<HttpResponse, HttpError>>);

impl HttpOp for Done {
    fn poll(&mut self) -> Option<Result<HttpResponse, HttpError>> {
        self.0.take()
    }
}

impl HttpClient for DirHttp {
    fn get(&self, url: &str) -> Box<dyn HttpOp> {
        let name = url.rsplit('/').next().unwrap_or("");
        let resp = match std::fs::read(self.0.join(name)) {
            Ok(body) => HttpResponse { status: 200, body },
            Err(_) => HttpResponse {
                status: 404,
                body: Vec::new(),
            },
        };
        Box::new(Done(Some(Ok(resp))))
    }
}

struct Ticks(std::cell::Cell<u64>);

impl Clock for Ticks {
    fn now_ms(&self) -> u64 {
        self.0.get()
    }
}

fn loader(dir: PathBuf) -> AssetLoader {
    let http: Rc<dyn HttpClient> = Rc::new(DirHttp(dir));
    let source = HttpSource::new(Rc::clone(&http), "http://test/assets/");
    let mut l = AssetLoader::new(http, "http://test/assets/", Box::new(source));
    l.load_manifest(None);
    l
}

#[test]
fn m2_boots_makes_the_core_and_waits_for_the_snapshot() {
    let Some(dir) = set_dir() else { return };
    let mut loader = loader(dir);
    let clock: Rc<dyn Clock> = Rc::new(Ticks(std::cell::Cell::new(0)));
    let mut m2 = M2Ui::new(Rc::new(platform::MemoryStore::default()), 1.0);
    let mut list = UiDrawList::new();
    let mut now = 0u64;
    for _ in 0..2000 {
        let env = M2Env {
            now_ms: now,
            loader: Some(&mut loader),
            size: SIZE,
            net: None,
            clock: &clock,
            seat: Default::default(),
            ai_turn: None,
            seat_known: true,
            projection: None,
        };
        let drawn = m2.begin_frame(env, &mut list);
        assert!(!drawn, "nothing is drawn before the game is live");
        assert_ne!(m2.phase(), M2Phase::Failed, "{:?}", m2.failure());
        if m2.phase() == M2Phase::Joining {
            break;
        }
        now += 16;
    }
    assert_eq!(
        m2.phase(),
        M2Phase::Joining,
        "the ui set and the game data load, and the core is made"
    );
    assert!(m2.layer().is_none());
    assert!(m2.contexts().iter().any(|c| c == "InGame"));
}

#[test]
fn the_context_filter_leaves_refused_contexts_empty() {
    let Some(dir) = set_dir() else { return };
    let mut loader = loader(dir);
    let mut boot = UiBoot::new(BootPlan::m1());
    let mut tries = 0;
    let store = loop {
        match boot.update(&mut loader) {
            BootState::Ready => break boot.store().expect("store"),
            BootState::Failed(m) => panic!("{m}"),
            BootState::Loading { .. } => {
                tries += 1;
                assert!(tries < 5000, "the ui set did not load");
            }
        }
    };
    let hooks = LayerHooks {
        context_filter: Some(Rc::new(|name: &str| name != "NotificationPanel")),
        ..LayerHooks::default()
    };
    let mut layer = UiLayer::with_hooks(store, &LayerConfig::m1(), SIZE, hooks).expect("layer");
    assert!(
        layer
            .core()
            .filtered_contexts()
            .iter()
            .any(|c| c == "NotificationPanel"),
        "{:?}",
        layer.core().filtered_contexts()
    );
    let mut list = UiDrawList::new();
    layer.frame(16, &mut list);
    assert_eq!(layer.stats().core.lua_errors, 0);
}
