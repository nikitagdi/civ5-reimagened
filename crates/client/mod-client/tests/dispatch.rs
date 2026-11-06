//! The dispatcher, the capability guard and the mock host, exercised through `ClientMod` implementations.

use std::cell::RefCell;
use std::rc::Rc;

use draw_list::UiDrawList;
use glam::Vec2;
use mod_api::{Cap, Caps, ChannelId, PlayerId, PlotClass, Version};
use mod_client::gpu::{
    DrawContext, FirstPerson, InitContext, MainSlot, PassSlot, PrepareContext, SystemId,
};
use mod_client::testing::MockClientCtx;
use mod_client::{
    AttachInfo, Claim, ClientCtx, ClientHost, ClientMod, ClientRegistry, CtxError, CutoutRegion,
    FrameOut, Guarded, HotkeySpec, ModDispatcher, ModInfo, ModRenderSystem, NamespacedKv,
    PackState, PlayOpts,
};
use mod_manifest::ModId;
use platform::{InputEvent, Key, KeyEvent, MemoryStore, Modifiers};

type Log = Rc<RefCell<Vec<String>>>;

fn info(idx: u16, id: &str, caps: &[Cap]) -> ModInfo {
    ModInfo {
        mod_idx: idx,
        id: ModId::new(id).unwrap(),
        version: Version::new(1, 0, 0),
        caps: Caps::of(caps),
    }
}

fn key(code: &str) -> InputEvent {
    InputEvent::Key(KeyEvent {
        key: Key::Character(code.to_lowercase()),
        code: Some(code.to_string()),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::default(),
    })
}

struct RenderStub(u16);
impl ModRenderSystem for RenderStub {
    fn id(&self) -> SystemId {
        SystemId::Mod(self.0)
    }
    fn passes(&self) -> &'static [PassSlot] {
        &[PassSlot::Main(MainSlot::Mod)]
    }
    fn init(&mut self, _cx: &mut InitContext<'_>) {}
    fn prepare(&mut self, _cx: &mut PrepareContext<'_>) {}
    fn record(&self, _slot: PassSlot, _cx: &mut DrawContext<'_, '_>) {}
}

/// A scriptable mod that logs every call it receives.
struct Probe {
    name: &'static str,
    log: Log,
    consume: bool,
    /// Near plane of the camera it returns each frame.
    camera: Option<f32>,
    suppress_ui: bool,
    systems: Vec<u16>,
}

impl Probe {
    fn new(name: &'static str, log: &Log) -> Self {
        Self {
            name,
            log: log.clone(),
            consume: false,
            camera: None,
            suppress_ui: false,
            systems: vec![],
        }
    }
    fn note(&self, what: String) {
        self.log.borrow_mut().push(format!("{}:{what}", self.name));
    }
}

impl ClientMod for Probe {
    fn attach(&mut self, _cx: &mut dyn ClientCtx, info: &AttachInfo) {
        let others: Vec<String> = info.set.iter().map(|m| m.id.to_string()).collect();
        self.note(format!(
            "attach idx={} set={}",
            info.this.mod_idx,
            others.join(",")
        ));
    }
    fn detach(&mut self, _cx: &mut dyn ClientCtx) {
        self.note("detach".into());
    }
    fn on_msg(&mut self, _cx: &mut dyn ClientCtx, ch: ChannelId, from: Option<PlayerId>, p: &[u8]) {
        self.note(format!("msg ch={ch} from={from:?} len={}", p.len()));
    }
    fn on_hotkey(&mut self, _cx: &mut dyn ClientCtx, id: &str) {
        self.note(format!("hotkey {id}"));
    }
    fn claim_input(&mut self, _cx: &mut dyn ClientCtx, _ev: &InputEvent) -> Claim {
        self.note("claim".into());
        if self.consume {
            Claim::Consume
        } else {
            Claim::Pass
        }
    }
    fn frame(&mut self, _cx: &mut dyn ClientCtx, dt: f32) -> FrameOut {
        self.note(format!("frame {dt}"));
        FrameOut {
            camera: self.camera.map(|near| {
                let mut c = FirstPerson::new(mod_client::gpu::glam::Vec3::new(1.0, 6.5, 2.0));
                c.near = near;
                Box::new(c) as Box<dyn mod_client::gpu::ModCamera>
            }),
            suppress_ui: self.suppress_ui,
        }
    }
    fn hud(&mut self, _cx: &mut dyn ClientCtx, list: &mut UiDrawList) {
        self.note("hud".into());
        list.add_filled_rect(Vec2::ZERO, Vec2::new(10.0, 10.0), [255, 0, 0, 255]);
    }
    fn render_systems(&mut self, _cx: &mut dyn ClientCtx) -> Vec<Box<dyn ModRenderSystem>> {
        self.systems
            .iter()
            .map(|&n| Box::new(RenderStub(n)) as Box<dyn ModRenderSystem>)
            .collect()
    }
}

fn dispatcher(mods: Vec<(ModInfo, Probe)>) -> ModDispatcher {
    let set: Vec<ModInfo> = mods.iter().map(|(i, _)| i.clone()).collect();
    let mut d = ModDispatcher::new();
    for (i, p) in mods {
        d.push(i, Box::new(p), set.clone());
    }
    d
}

#[test]
fn empty_dispatcher_does_nothing() {
    let mut d = ModDispatcher::new();
    let mut host = MockClientCtx::new();
    assert!(d.is_empty());
    d.attach(&mut host);
    assert_eq!(d.claim_input(&mut host, &key("KeyB")), Claim::Pass);
    let out = d.frame(&mut host, 0.016);
    assert!(out.camera.is_none() && !out.suppress_ui);
    let mut list = UiDrawList::new();
    d.hud(&mut host, &mut list);
    assert!(list.is_empty());
    assert!(d.render_systems(&mut host).is_empty());
    assert!(!d.on_msg(&mut host, 0, 0, None, b"x"));
    d.detach(&mut host);
    assert!(host.sent.is_empty() && host.logs.is_empty());
}

#[test]
fn attach_in_load_order_and_detach_in_reverse() {
    let log: Log = Log::default();
    let mut d = dispatcher(vec![
        (info(0, "alpha", &[]), Probe::new("alpha", &log)),
        (info(1, "beta", &[]), Probe::new("beta", &log)),
    ]);
    let mut host = MockClientCtx::new();
    d.attach(&mut host);
    d.detach(&mut host);
    assert_eq!(
        *log.borrow(),
        [
            "alpha:attach idx=0 set=alpha,beta",
            "beta:attach idx=1 set=alpha,beta",
            "beta:detach",
            "alpha:detach"
        ]
    );
}

#[test]
fn attach_info_finds_siblings() {
    let a = info(0, "alpha", &[]);
    let ai = AttachInfo {
        this: a.clone(),
        set: vec![a, info(1, "beta", &[])],
    };
    assert!(ai.sibling("beta").is_some());
    assert!(ai.sibling("gamma").is_none());
}

#[test]
fn messages_route_by_index_and_need_the_net_capability() {
    let log: Log = Log::default();
    let mut d = dispatcher(vec![
        (
            info(0, "alpha", &[Cap::NetChannel]),
            Probe::new("alpha", &log),
        ),
        (info(1, "beta", &[]), Probe::new("beta", &log)),
    ]);
    let mut host = MockClientCtx::new();
    assert!(d.on_msg(&mut host, 0, 3, Some(2), b"abc"));
    assert!(!d.on_msg(&mut host, 1, 3, None, b"abc"), "no net.channel");
    assert!(!d.on_msg(&mut host, 7, 3, None, b"abc"), "unknown index");
    assert_eq!(*log.borrow(), ["alpha:msg ch=3 from=Some(2) len=3"]);
}

#[test]
fn disabled_mods_receive_nothing() {
    let log: Log = Log::default();
    let mut d = dispatcher(vec![(
        info(0, "alpha", &[Cap::NetChannel, Cap::InputClaim, Cap::Hud]),
        Probe::new("alpha", &log),
    )]);
    let mut host = MockClientCtx::new();
    assert!(d.is_enabled(0));
    d.detach_one(&mut host, 0);
    assert!(!d.is_enabled(0));
    assert!(!d.set_enabled(9, false));
    assert!(!d.on_msg(&mut host, 0, 0, None, b""));
    assert_eq!(d.claim_input(&mut host, &key("KeyB")), Claim::Pass);
    d.frame(&mut host, 0.1);
    d.attach(&mut host);
    assert_eq!(*log.borrow(), ["alpha:detach"]);
    assert!(d.set_enabled(0, true));
    d.frame(&mut host, 0.1);
    assert_eq!(log.borrow().len(), 2);
}

#[test]
fn first_consume_wins_and_only_input_claim_mods_are_offered() {
    let log: Log = Log::default();
    let mut a = Probe::new("alpha", &log);
    a.consume = true;
    let mut b = Probe::new("beta", &log);
    b.consume = true;
    let mut c = Probe::new("gamma", &log);
    c.consume = true;
    let mut d = dispatcher(vec![
        (info(0, "alpha", &[]), a), // no input.claim: not asked
        (info(1, "beta", &[Cap::InputClaim]), b),
        (info(2, "gamma", &[Cap::InputClaim]), c),
    ]);
    let mut host = MockClientCtx::new();
    assert_eq!(d.claim_input(&mut host, &key("KeyB")), Claim::Consume);
    assert_eq!(*log.borrow(), ["beta:claim"]);
}

#[test]
fn passing_mods_let_the_event_through() {
    let log: Log = Log::default();
    let mut d = dispatcher(vec![
        (
            info(0, "alpha", &[Cap::InputClaim]),
            Probe::new("alpha", &log),
        ),
        (
            info(1, "beta", &[Cap::InputClaim]),
            Probe::new("beta", &log),
        ),
    ]);
    let mut host = MockClientCtx::new();
    assert_eq!(d.claim_input(&mut host, &key("KeyB")), Claim::Pass);
    assert_eq!(*log.borrow(), ["alpha:claim", "beta:claim"]);
}

#[test]
fn frame_merges_camera_and_suppress_ui_by_capability() {
    let log: Log = Log::default();
    let mut a = Probe::new("alpha", &log);
    a.camera = Some(0.5);
    a.suppress_ui = true; // no capability for either
    let mut b = Probe::new("beta", &log);
    b.camera = Some(1.0);
    let mut c = Probe::new("gamma", &log);
    c.camera = Some(2.0);
    c.suppress_ui = true;
    let mut d = dispatcher(vec![
        (info(0, "alpha", &[]), a),
        (info(1, "beta", &[Cap::Camera]), b),
        (info(2, "gamma", &[Cap::Camera, Cap::InputClaim]), c),
    ]);
    let mut host = MockClientCtx::new();
    let out = d.frame(&mut host, 0.016);
    assert_eq!(
        out.camera.map(|c| c.near()),
        Some(1.0),
        "first holder of `camera` wins"
    );
    assert!(out.suppress_ui, "gamma may suppress");
    assert!(host.logged("mod `alpha` returned a camera override without the `camera` capability"));
    assert!(
        host.logged(
            "mod `alpha` asked to suppress the game UI without the `input.claim` capability"
        )
    );
    // Warned once per capability, not per frame.
    d.frame(&mut host, 0.016);
    assert_eq!(host.logs.len(), 2);
}

#[test]
fn suppress_ui_alone_without_input_claim_is_dropped() {
    let log: Log = Log::default();
    let mut a = Probe::new("alpha", &log);
    a.suppress_ui = true;
    let mut d = dispatcher(vec![(info(0, "alpha", &[Cap::Camera]), a)]);
    let mut host = MockClientCtx::new();
    assert!(!d.frame(&mut host, 0.0).suppress_ui);
}

#[test]
fn hud_and_render_systems_need_their_capabilities() {
    let log: Log = Log::default();
    let mut a = Probe::new("alpha", &log);
    a.systems = vec![7];
    let mut b = Probe::new("beta", &log);
    b.systems = vec![1, 2];
    let mut d = dispatcher(vec![
        (info(0, "alpha", &[]), a),
        (info(1, "beta", &[Cap::Hud, Cap::RenderPipeline]), b),
    ]);
    let mut host = MockClientCtx::new();
    let mut list = UiDrawList::new();
    d.hud(&mut host, &mut list);
    assert_eq!(*log.borrow(), ["beta:hud"]);
    assert_eq!(list.commands().len(), 1);
    let systems = d.render_systems(&mut host);
    let ids: Vec<(u16, SystemId)> = systems.iter().map(|(i, s)| (*i, s.id())).collect();
    assert_eq!(ids, [(1, SystemId::Mod(1)), (1, SystemId::Mod(2))]);
}

#[test]
fn hotkeys_register_conflict_and_route() {
    struct Keys;
    impl ClientMod for Keys {
        fn attach(&mut self, cx: &mut dyn ClientCtx, _i: &AttachInfo) {
            let hk = cx.hotkeys().unwrap();
            hk.register(HotkeySpec::new("enter", "KeyB", "Enter the world"))
                .unwrap();
            assert!(
                hk.register(HotkeySpec::new("enter", "KeyC", "dup id"))
                    .is_err()
            );
            assert!(
                hk.register(HotkeySpec::new("other", "KeyB", "same chord"))
                    .is_err()
            );
            assert!(
                hk.register(HotkeySpec::new("taken", "KeyG", "game key"))
                    .is_err()
            );
        }
        fn on_hotkey(&mut self, cx: &mut dyn ClientCtx, id: &str) {
            cx.send(0, id.as_bytes()).unwrap();
        }
    }
    let mut d = ModDispatcher::new();
    let i = info(4, "keys", &[Cap::InputClaim, Cap::NetChannel]);
    d.push(i.clone(), Box::new(Keys), vec![i]);
    let mut host = MockClientCtx::new();
    host.hotkeys.reserve("KeyG");
    d.attach(&mut host);
    let (idx, id) = host.hotkeys.lookup("KeyB", false, false, false).unwrap();
    let id = id.to_string();
    assert_eq!(idx, 4);
    assert!(d.on_hotkey(&mut host, idx, &id));
    assert_eq!(host.sent_on(4, 0), [&b"enter"[..]]);
    assert!(!d.on_hotkey(&mut host, 9, "enter"));
}

#[test]
fn registry_instantiates_known_mods_and_reports_missing_ones() {
    let log: Log = Log::default();
    let mut reg = ClientRegistry::new();
    let l = log.clone();
    reg.add("alpha", move || Box::new(Probe::new("alpha", &l)));
    let set = [info(0, "alpha", &[]), info(1, "ghost", &[])];
    let (d, missing) = reg.instantiate(&set);
    assert_eq!(d.len(), 1);
    assert_eq!(missing, [ModId::new("ghost").unwrap()]);
    assert_eq!(d.mods().next().unwrap().mod_idx, 0);
}

#[test]
fn registry_records_bad_and_duplicate_ids_first_wins() {
    let mut reg = ClientRegistry::new();
    reg.add("alpha", || Box::new(Quiet));
    reg.add("alpha", || Box::new(Quiet));
    reg.add("Not Valid", || Box::new(Quiet));
    assert_eq!(reg.problems().len(), 2);
    assert_eq!(reg.ids().count(), 1);
    assert!(reg.create(&ModId::new("alpha").unwrap()).is_some());
    assert!(reg.create(&ModId::new("beta").unwrap()).is_none());
}

struct Quiet;
impl ClientMod for Quiet {}

// ----- the guard -----

#[test]
fn guard_denies_every_service_without_its_capability() {
    let mut host = MockClientCtx::new();
    let none = Caps::none();
    let mut g = Guarded::new(&mut host, &none);
    assert!(matches!(g.send(0, b"x"), Err(CtxError::Denied(d)) if d.0 == Cap::NetChannel));
    assert!(matches!(g.world(), Err(CtxError::Denied(d)) if d.0 == Cap::WorldRead));
    assert!(matches!(g.terrain(), Err(CtxError::Denied(d)) if d.0 == Cap::WorldRead));
    assert!(matches!(g.cutout(), Err(CtxError::Denied(d)) if d.0 == Cap::TerrainCutout));
    assert!(matches!(g.kv(), Err(CtxError::Denied(d)) if d.0 == Cap::Kv));
    assert!(matches!(g.hotkeys(), Err(CtxError::Denied(d)) if d.0 == Cap::InputClaim));
    assert!(matches!(g.audio(), Err(CtxError::Denied(d)) if d.0 == Cap::Audio));
    assert!(matches!(g.assets(), Err(CtxError::Denied(d)) if d.0 == Cap::Assets));
    assert!(matches!(g.hud_textures(), Err(CtxError::Denied(d)) if d.0 == Cap::Hud));
    assert!(
        matches!(g.request_pointer_lock(true), Err(CtxError::Denied(d)) if d.0 == Cap::InputClaim)
    );
    assert!(!g.has(Cap::Kv));
    // Screen and logging are free.
    assert_eq!(g.screen().width, 1280);
    g.log(mod_api::LogLevel::Info, "hello");
    assert!(host.sent.is_empty() && host.pointer_lock_requests.is_empty());
    assert!(host.logged("hello"));
}

#[test]
fn guard_passes_calls_with_capabilities() {
    let mut host = MockClientCtx::new().with_pack(&[("textures/dirt", &[1, 2, 3, 4])]);
    host.world.set_me(2).reveal(1, 1, PlotClass::Land, Some(2));
    let caps = Caps::all();
    let mut g = Guarded::new(&mut host, &caps);
    g.send(1, b"hi").unwrap();
    assert!(g.world().is_ok() && g.terrain().is_ok());
    g.kv().unwrap().set("k", "v");
    g.audio().unwrap().play("dig", PlayOpts::default()).unwrap();
    g.request_pointer_lock(true).unwrap();
    assert_eq!(
        g.assets().unwrap().get("textures/dirt"),
        Some(&[1u8, 2, 3, 4][..])
    );
    assert_eq!(host.sent_on(0, 1), [&b"hi"[..]]);
    assert_eq!(host.pointer_lock_requests, [true]);
    assert_eq!(host.audio.played.len(), 1);
}

#[test]
fn send_rejects_a_payload_that_can_never_fit_a_frame() {
    let mut host = MockClientCtx::new();
    let caps = Caps::of(&[Cap::NetChannel]);
    let mut g = Guarded::new(&mut host, &caps);
    let max = 56 * 1024;
    assert!(g.send(0, &vec![0; max]).is_ok());
    assert_eq!(
        g.send(0, &vec![0; max + 1]),
        Err(CtxError::TooLarge { len: max + 1, max })
    );
    assert_eq!(host.sent.len(), 1);
}

// ----- services -----

#[test]
fn world_reads_follow_the_replica_and_its_revision() {
    let mut host = MockClientCtx::new();
    host.world
        .set_me(3)
        .add_player(3, "me")
        .add_player(1, "ann");
    host.world
        .reveal(7, 0, PlotClass::Hills, Some(1))
        .set_turn(12);
    let caps = Caps::of(&[Cap::WorldRead]);
    let g = Guarded::new(&mut host, &caps);
    let w = g.world().unwrap();
    assert_eq!(w.me(), Some(3));
    assert_eq!(w.turn(), 12);
    assert_eq!(w.players().len(), 2);
    // The map wraps in x: x = 8 on an 8-wide map is x = 0, x = -1 is x = 7.
    let p = w.plot_at(-1, 0).unwrap();
    assert_eq!(p, w.plot_at(7, 0).unwrap());
    assert_eq!(w.plot_owner(p), Some(1));
    assert_eq!(w.plot(p).class, Some(PlotClass::Hills));
    assert_eq!(w.plot_xy(p), (7, 0));
    // Off the map in y, and a plot never seen.
    assert_eq!(w.plot_at(0, 99), None);
    let unseen = w.plot_at(2, 2).unwrap();
    assert_eq!(w.plot(unseen), mod_client::PlotView::UNKNOWN);
    assert_eq!(w.plot(unseen).class, None);

    let rev = w.revision();
    host.world.set_owner(7, 0, Some(3));
    let g = Guarded::new(&mut host, &caps);
    let w = g.world().unwrap();
    assert!(w.revision() > rev);
    assert_eq!(w.plot_owner(p), Some(3));
}

#[test]
fn terrain_samples_and_revision() {
    let mut host = MockClientCtx::new();
    host.terrain.base = 12.0;
    host.terrain.slope_x = 0.5;
    host.terrain.revision = 4;
    let caps = Caps::of(&[Cap::WorldRead]);
    let g = Guarded::new(&mut host, &caps);
    let t = g.terrain().unwrap();
    assert_eq!(t.height(4.0, 0.0), Some(14.0));
    assert_eq!(t.height_revision(), 4);
    assert_eq!(t.hex_at(2.5, 3.5), Some((2, 3)));
    assert!(t.is_built(0));
}

#[test]
fn cutout_masks_are_size_checked() {
    let mut host = MockClientCtx::new();
    let caps = Caps::of(&[Cap::TerrainCutout]);
    let mut g = Guarded::new(&mut host, &caps);
    let region = CutoutRegion {
        x0: 10,
        z0: -4,
        width: 3,
        height: 2,
    };
    let c = g.cutout().unwrap();
    c.set_cutout_mask(region, &[0, 1, 1, 0, 0, 1]).unwrap();
    assert!(matches!(
        c.set_cutout_mask(region, &[0; 5]),
        Err(CtxError::Rejected(_))
    ));
    c.clear_cutout();
    assert_eq!(host.cutout.writes.len(), 1);
    assert_eq!(host.cutout.cleared, 1);
}

#[test]
fn hud_texture_upload_checks_pixel_data() {
    let mut host = MockClientCtx::new();
    let caps = Caps::of(&[Cap::Hud]);
    let mut g = Guarded::new(&mut host, &caps);
    let h = g.hud_textures().unwrap();
    let a = h.upload_rgba8(2, 2, &[0; 16]).unwrap();
    let b = h.upload_rgba8(1, 1, &[0; 4]).unwrap();
    assert_ne!(a, b);
    assert!(h.upload_rgba8(2, 2, &[0; 15]).is_err());
}

#[test]
fn kv_is_per_mod_in_the_mock_and_namespaced_over_a_platform_store() {
    let mut host = MockClientCtx::new();
    host.ctx(1).kv().unwrap().set("sens", "0.5");
    host.ctx(2).kv().unwrap().set("sens", "0.9");
    assert_eq!(
        host.ctx(1).kv().unwrap().get("sens").as_deref(),
        Some("0.5")
    );
    assert_eq!(
        host.ctx(2).kv().unwrap().get("sens").as_deref(),
        Some("0.9")
    );

    use mod_client::ModKv;
    use platform::KeyValueStore;
    let store = MemoryStore::new();
    let shared: Rc<dyn KeyValueStore> = Rc::new(store.clone());
    let mut a = NamespacedKv::new(shared.clone(), &ModId::new("alpha").unwrap());
    let mut b = NamespacedKv::new(shared, &ModId::new("beta").unwrap());
    a.set("slot", "3");
    assert_eq!(a.get("slot").as_deref(), Some("3"));
    assert_eq!(b.get("slot"), None, "beta cannot see alpha's key");
    assert_eq!(store.get("civ5.mod.alpha.slot").as_deref(), Some("3"));
    b.set("slot", "1");
    a.remove("slot");
    assert_eq!(a.get("slot"), None);
    assert_eq!(b.get("slot").as_deref(), Some("1"));
    // Over-limit keys and values are dropped.
    a.set("", "x");
    a.set(&"k".repeat(mod_client::KV_KEY_MAX + 1), "x");
    a.set("big", &"v".repeat(mod_client::KV_VALUE_MAX + 1));
    assert_eq!(store.len(), 1);
}

#[test]
fn a_mod_can_disable_itself_when_the_pack_is_missing() {
    struct NeedsPack {
        ok: Rc<RefCell<Option<bool>>>,
    }
    impl ClientMod for NeedsPack {
        fn attach(&mut self, cx: &mut dyn ClientCtx, _i: &AttachInfo) {
            let a = cx.assets().unwrap();
            *self.ok.borrow_mut() =
                Some(a.state() == PackState::Ready && a.get("textures/dirt").is_some());
            if a.state() == PackState::Missing {
                cx.log(mod_api::LogLevel::Warn, "asset pack missing: build it");
            }
        }
    }
    let seen = Rc::new(RefCell::new(None));
    let i = info(0, "needs", &[Cap::Assets]);
    let mut d = ModDispatcher::new();
    d.push(i.clone(), Box::new(NeedsPack { ok: seen.clone() }), vec![i]);

    let mut missing = MockClientCtx::new();
    d.attach(&mut missing);
    assert_eq!(*seen.borrow(), Some(false));
    assert!(missing.logged("asset pack missing"));

    let mut ready = MockClientCtx::new().with_pack(&[("textures/dirt", b"px")]);
    d.attach(&mut ready);
    assert_eq!(*seen.borrow(), Some(true));
    // Files are hidden until the pack is ready.
    ready.assets.state = PackState::Pending;
    assert!(ready.assets.files.contains_key("textures/dirt"));
    use mod_client::ModAssets;
    assert_eq!(ready.assets.get("textures/dirt"), None);
}

#[test]
fn pointer_lock_is_requested_from_claim_input() {
    struct Fps {
        locked: bool,
    }
    impl ClientMod for Fps {
        fn claim_input(&mut self, cx: &mut dyn ClientCtx, ev: &InputEvent) -> Claim {
            match ev {
                InputEvent::Key(k) if k.pressed && k.code.as_deref() == Some("KeyB") => {
                    self.locked = true;
                    cx.request_pointer_lock(true).unwrap();
                    Claim::Consume
                }
                _ if self.locked => Claim::Consume,
                _ => Claim::Pass,
            }
        }
        fn detach(&mut self, cx: &mut dyn ClientCtx) {
            if self.locked {
                cx.request_pointer_lock(false).unwrap();
            }
        }
    }
    let i = info(0, "fps", &[Cap::InputClaim]);
    let mut d = ModDispatcher::new();
    d.push(i.clone(), Box::new(Fps { locked: false }), vec![i]);
    let mut host = MockClientCtx::new();
    assert_eq!(
        d.claim_input(&mut host, &InputEvent::Focused(true)),
        Claim::Pass
    );
    assert_eq!(d.claim_input(&mut host, &key("KeyB")), Claim::Consume);
    assert_eq!(
        d.claim_input(&mut host, &InputEvent::Focused(true)),
        Claim::Consume
    );
    d.detach(&mut host);
    assert_eq!(host.pointer_lock_requests, [true, false]);
}

#[test]
fn echo_round_trip_through_the_dispatcher() {
    struct Echo;
    impl ClientMod for Echo {
        fn on_msg(
            &mut self,
            cx: &mut dyn ClientCtx,
            ch: ChannelId,
            _from: Option<PlayerId>,
            p: &[u8],
        ) {
            cx.send(ch, &p.iter().rev().copied().collect::<Vec<u8>>())
                .unwrap();
            cx.audio()
                .unwrap()
                .play(
                    "ping",
                    PlayOpts {
                        volume: 0.5,
                        ..PlayOpts::default()
                    },
                )
                .unwrap();
        }
    }
    let mut reg = ClientRegistry::new();
    reg.add("echo", || Box::new(Echo));
    let (mut d, missing) = reg.instantiate(&[info(5, "echo", &[Cap::NetChannel, Cap::Audio])]);
    assert!(missing.is_empty());
    let mut host = MockClientCtx::new();
    assert!(d.on_msg(&mut host, 5, 0, Some(1), b"abc"));
    assert_eq!(host.sent_on(5, 0), [&b"cba"[..]]);
    assert_eq!(host.audio.played[0].1.volume, 0.5);
}
