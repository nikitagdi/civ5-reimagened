//! Client mod API 1.3: the mod's UI v2 model fields (`ClientCtx::ui_model`) and the intents of its panels
//! (`ClientMod::on_ui_intent`, `ModDispatcher::on_ui_intent`).

use mod_api::{Cap, CapDenied, Caps, ChannelId, LogLevel, PlayerId, Version};
use mod_client::testing::{MockClientCtx, MockUiModel, UiOp};
use mod_client::{
    AttachInfo, ClientCtx, ClientMod, ClientRegistry, CtxError, Guarded, ModAssets, ModInfo, ModKv,
    ModTerrain, ModWorld, ScreenInfo, UiValue,
};
use mod_manifest::ModId;

fn info(idx: u16, id: &str, caps: &[Cap]) -> ModInfo {
    ModInfo {
        mod_idx: idx,
        id: ModId::new(id).unwrap(),
        version: Version::new(1, 0, 0),
        caps: Caps::of(caps),
    }
}

/// A 1.3 mod: counts echoes from its server half into `count` and the list `echoes`, says hello on its intent.
#[derive(Default)]
struct Echo {
    count: i64,
    intents: Vec<(String, Vec<(String, UiValue)>)>,
}

impl ClientMod for Echo {
    fn attach(&mut self, cx: &mut dyn ClientCtx, _info: &AttachInfo) {
        if let Ok(ui) = cx.ui_model() {
            let _ = ui.set("count", UiValue::Int(0));
        }
    }

    fn on_msg(&mut self, cx: &mut dyn ClientCtx, _ch: ChannelId, from: Option<PlayerId>, p: &[u8]) {
        self.count += 1;
        let text = String::from_utf8_lossy(p).to_string();
        if let Ok(ui) = cx.ui_model() {
            let _ = ui.set("count", self.count.into());
            let _ = ui.list_put(
                "echoes",
                self.count,
                &[
                    ("from", i64::from(from.unwrap_or(-1)).into()),
                    ("text", text.into()),
                ],
            );
        }
    }

    fn on_ui_intent(&mut self, cx: &mut dyn ClientCtx, name: &str, args: &[(String, UiValue)]) {
        self.intents.push((name.to_string(), args.to_vec()));
        if name == "say" {
            let _ = cx.send(0, b"hello");
        }
        if name == "reset"
            && let Ok(ui) = cx.ui_model()
        {
            let _ = ui.list_clear("echoes");
            let _ = ui.set("count", UiValue::Int(0));
        }
    }
}

/// A 1.2 mod: it knows nothing of the UI v2 and implements only what it did before.
#[derive(Default)]
struct OldMod {
    attached: bool,
}

impl ClientMod for OldMod {
    fn attach(&mut self, cx: &mut dyn ClientCtx, _info: &AttachInfo) {
        self.attached = true;
        cx.log(LogLevel::Info, "old mod attached");
    }
}

#[test]
fn the_ui_model_needs_the_ui_capability() {
    let mut host = MockClientCtx::new();
    let none = Caps::of(&[Cap::Hud]);
    let mut g = Guarded::new(&mut host, &none);
    assert!(matches!(
        g.ui_model(),
        Err(CtxError::Denied(CapDenied(Cap::Ui)))
    ));
    let caps = Caps::of(&[Cap::Ui]);
    let mut g = Guarded::new(&mut host, &caps);
    let ui = g.ui_model().unwrap();
    ui.set("count", UiValue::Int(3)).unwrap();
    ui.list_put("echoes", 7, &[("text", "hi".into())]).unwrap();
    ui.list_remove("echoes", 8).unwrap();
    let m = host.ui_of(0);
    assert_eq!(m.get("count"), Some(&UiValue::Int(3)));
    assert_eq!(m.keys("echoes"), [7]);
    assert_eq!(m.ops.len(), 3);
    assert_eq!(
        m.ops[1],
        UiOp::ListPut {
            list: "echoes".into(),
            key: 7,
            rec: vec![("text".into(), UiValue::Str("hi".into()))],
        }
    );
}

#[test]
fn a_refused_write_names_the_field() {
    let mut host = MockClientCtx::new();
    host.ui.insert(0, MockUiModel::declared(&["count"]));
    let caps = Caps::of(&[Cap::Ui]);
    let mut g = Guarded::new(&mut host, &caps);
    let ui = g.ui_model().unwrap();
    assert!(ui.set("count", 1.into()).is_ok());
    let e = ui.set("nope", 1.into()).unwrap_err();
    assert!(
        matches!(&e, CtxError::Rejected(t) if t.contains("nope")),
        "{e}"
    );
    assert_eq!(host.ui_of(0).ops.len(), 1, "the refused write is dropped");
}

#[test]
fn the_model_is_written_from_every_call_and_kept_per_mod() {
    let mut reg = ClientRegistry::new();
    reg.add("echo", || Box::new(Echo::default()));
    reg.add("other", || Box::new(Echo::default()));
    let set = [
        info(0, "echo", &[Cap::Ui, Cap::NetChannel]),
        info(1, "other", &[Cap::NetChannel]),
    ];
    let (mut d, missing) = reg.instantiate(&set);
    assert!(missing.is_empty());
    let mut host = MockClientCtx::new();
    d.attach(&mut host);
    assert!(d.on_msg(&mut host, 0, 0, Some(2), b"one"));
    assert!(d.on_msg(&mut host, 0, 0, Some(3), b"two"));
    assert!(d.on_msg(&mut host, 1, 0, Some(3), b"x"));
    let m = host.ui_of(0);
    assert_eq!(m.get("count"), Some(&UiValue::Int(2)));
    assert_eq!(m.keys("echoes"), [1, 2]);
    assert_eq!(
        m.lists["echoes"][&2],
        [
            ("from".to_string(), UiValue::Int(3)),
            ("text".to_string(), UiValue::Str("two".into()))
        ]
    );
    // The mod without `ui` was refused at every call and wrote nothing.
    assert!(host.ui_of(1).ops.is_empty());
}

#[test]
fn intents_reach_only_mods_with_ui_by_index() {
    let mut reg = ClientRegistry::new();
    reg.add("echo", || Box::new(Echo::default()));
    reg.add("plain", || Box::new(Echo::default()));
    let set = [
        info(0, "plain", &[Cap::NetChannel]),
        info(1, "echo", &[Cap::Ui, Cap::NetChannel]),
    ];
    let (mut d, _) = reg.instantiate(&set);
    let mut host = MockClientCtx::new();
    d.attach(&mut host);
    assert_eq!(d.idx_of("echo"), Some(1));
    assert_eq!(d.idx_of("ghost"), None);

    let args = vec![("text".to_string(), UiValue::Str("hi".into()))];
    assert!(d.on_ui_intent(&mut host, 1, "say", &args));
    assert_eq!(host.sent_on(1, 0), [&b"hello"[..]]);
    // A mod without the capability, an unknown index: nobody sees it.
    assert!(!d.on_ui_intent(&mut host, 0, "say", &[]));
    assert!(!d.on_ui_intent(&mut host, 9, "say", &[]));
    assert!(host.sent_on(0, 0).is_empty());
    // The intent's model writes land in the mod's own fields.
    assert!(d.on_ui_intent(&mut host, 1, "reset", &[]));
    assert_eq!(host.ui_of(1).get("count"), Some(&UiValue::Int(0)));
    assert!(host.ui_of(0).ops.is_empty());
    // A switched-off mod gets nothing.
    assert!(d.set_enabled(1, false));
    assert!(!d.on_ui_intent(&mut host, 1, "say", &[]));
    assert_eq!(d.idx_of("echo"), None);
}

#[test]
fn a_1_2_mod_builds_and_runs_and_ignores_intents() {
    let mut reg = ClientRegistry::new();
    reg.add("old", || Box::new(OldMod::default()));
    let (mut d, _) = reg.instantiate(&[info(0, "old", &[Cap::Hud, Cap::Ui])]);
    let mut host = MockClientCtx::new();
    d.attach(&mut host);
    assert!(host.logged("old mod attached"));
    // The default `on_ui_intent` does nothing and the call is harmless.
    assert!(d.on_ui_intent(&mut host, 0, "anything", &[]));
    assert!(host.sent.is_empty() && host.ui_of(0).ops.is_empty());
    let _ = d.frame(&mut host, 0.016);
}

/// A host written for 1.2: it implements only the methods it had to, so `ui_model` is the default.
struct OldHost(MockClientCtx);

impl ClientCtx for OldHost {
    fn has(&self, cap: Cap) -> bool {
        self.0.has(cap)
    }
    fn log(&mut self, level: LogLevel, msg: &str) {
        self.0.log(level, msg);
    }
    fn send(&mut self, ch: ChannelId, payload: &[u8]) -> Result<(), CtxError> {
        self.0.send(ch, payload)
    }
    fn world(&self) -> Result<&dyn ModWorld, CtxError> {
        self.0.world()
    }
    fn terrain(&self) -> Result<&dyn ModTerrain, CtxError> {
        self.0.terrain()
    }
    fn cutout(&mut self) -> Result<&mut dyn mod_client::CutoutMask, CtxError> {
        self.0.cutout()
    }
    fn kv(&mut self) -> Result<&mut dyn ModKv, CtxError> {
        self.0.kv()
    }
    fn hotkeys(&mut self) -> Result<&mut dyn mod_client::Hotkeys, CtxError> {
        self.0.hotkeys()
    }
    fn audio(&mut self) -> Result<&mut dyn mod_client::ModAudio, CtxError> {
        self.0.audio()
    }
    fn assets(&self) -> Result<&dyn ModAssets, CtxError> {
        self.0.assets()
    }
    fn hud_textures(&mut self) -> Result<&mut dyn mod_client::HudTextures, CtxError> {
        self.0.hud_textures()
    }
    fn screen(&self) -> ScreenInfo {
        self.0.screen()
    }
    fn request_pointer_lock(&mut self, lock: bool) -> Result<(), CtxError> {
        self.0.request_pointer_lock(lock)
    }
}

#[test]
fn a_host_without_the_ui_v2_answers_unavailable() {
    let mut host = OldHost(MockClientCtx::new());
    let caps = Caps::of(&[Cap::Ui]);
    let mut g = Guarded::new(&mut host, &caps);
    assert!(matches!(g.ui_model(), Err(CtxError::Unavailable("ui"))));
    // An Echo attached to it simply has no fields to write.
    let mut m = Echo::default();
    let info = AttachInfo {
        this: info(0, "echo", &[Cap::Ui]),
        set: vec![],
    };
    m.attach(&mut g, &info);
}

#[test]
fn ui_values_convert_from_plain_types() {
    assert_eq!(UiValue::from(true), UiValue::Bool(true));
    assert_eq!(UiValue::from(5i32), UiValue::Int(5));
    assert_eq!(UiValue::from(5u32).as_int(), Some(5));
    assert_eq!(UiValue::from(0.5f32), UiValue::Num(0.5));
    assert_eq!(UiValue::from("x").as_str(), Some("x"));
    assert_eq!(UiValue::from(String::from("y")), UiValue::Str("y".into()));
    assert_eq!(UiValue::Int(2).as_num(), Some(2.0));
    assert_eq!(UiValue::Null.as_bool(), None);
}
