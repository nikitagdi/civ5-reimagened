//! API 1.1: the camera and the input are held by one mod at a time (`acquire_control`), and a second mod is told who
//! holds them.

use std::cell::RefCell;
use std::rc::Rc;

use mod_api::{Cap, Caps, Version};
use mod_client::gpu::{FirstPerson, ModCamera, glam::Vec3};
use mod_client::testing::MockClientCtx;
use mod_client::{
    Claim, ClientCtx, ClientMod, CtxError, FrameOut, Guarded, ModDispatcher, ModInfo,
};
use mod_manifest::ModId;
use platform::{InputEvent, Key, KeyEvent, Modifiers};

type Log = Rc<RefCell<Vec<String>>>;

fn id(s: &str) -> ModId {
    ModId::new(s).unwrap()
}

fn info(idx: u16, name: &str, caps: &[Cap]) -> ModInfo {
    ModInfo {
        mod_idx: idx,
        id: id(name),
        version: Version::new(1, 0, 0),
        caps: Caps::of(caps),
    }
}

fn key() -> InputEvent {
    InputEvent::Key(KeyEvent {
        key: Key::Character("w".into()),
        code: Some("KeyW".into()),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::default(),
    })
}

fn camera(near: f32) -> Box<dyn ModCamera> {
    let mut c = FirstPerson::new(Vec3::new(0.0, 6.5, 0.0));
    c.near = near;
    Box::new(c)
}

/// A 1.1 mod with a mode: hotkey `enter` acquires (an error is logged), `leave` releases, `lock` asks for the
/// pointer, `who` logs the holder. In its mode it returns a camera (near plane = its index + 1) and consumes input;
/// `force` makes it return a camera anyway.
struct Mode {
    name: &'static str,
    near: f32,
    log: Log,
    in_mode: bool,
    force: bool,
}

impl Mode {
    fn new(name: &'static str, near: f32, log: &Log) -> Self {
        Self {
            name,
            near,
            log: log.clone(),
            in_mode: false,
            force: false,
        }
    }
    fn note(&self, s: String) {
        self.log.borrow_mut().push(format!("{}:{s}", self.name));
    }
}

impl ClientMod for Mode {
    fn on_hotkey(&mut self, cx: &mut dyn ClientCtx, hk: &str) {
        match hk {
            "enter" => match cx.acquire_control() {
                Ok(()) => self.in_mode = true,
                Err(e) => self.note(format!("refused: {e}")),
            },
            "leave" => {
                cx.release_control();
                self.in_mode = false;
            }
            "lock" => {
                if let Err(e) = cx.request_pointer_lock(true) {
                    self.note(format!("lock: {e}"));
                }
            }
            "who" => self.note(format!(
                "holder {:?}",
                cx.control_holder().map(|m| m.to_string())
            )),
            _ => {}
        }
    }
    fn claim_input(&mut self, _cx: &mut dyn ClientCtx, _ev: &InputEvent) -> Claim {
        self.note("claim".into());
        if self.in_mode {
            Claim::Consume
        } else {
            Claim::Pass
        }
    }
    fn frame(&mut self, _cx: &mut dyn ClientCtx, _dt: f32) -> FrameOut {
        FrameOut {
            camera: (self.in_mode || self.force).then(|| camera(self.near)),
            suppress_ui: self.in_mode || self.force,
        }
    }
}

/// A 1.0 mod: `enter` just starts returning a camera, `leave` stops.
struct Legacy {
    in_mode: bool,
}

impl ClientMod for Legacy {
    fn on_hotkey(&mut self, _cx: &mut dyn ClientCtx, hk: &str) {
        self.in_mode = hk == "enter";
    }
    fn frame(&mut self, _cx: &mut dyn ClientCtx, _dt: f32) -> FrameOut {
        FrameOut {
            camera: self.in_mode.then(|| camera(9.0)),
            suppress_ui: self.in_mode,
        }
    }
}

const BOTH: &[Cap] = &[Cap::Camera, Cap::InputClaim];

fn two_modes(log: &Log) -> ModDispatcher {
    let set = vec![info(0, "alpha", BOTH), info(1, "beta", BOTH)];
    let mut d = ModDispatcher::new();
    d.push(
        set[0].clone(),
        Box::new(Mode::new("alpha", 1.0, log)),
        set.clone(),
    );
    d.push(
        set[1].clone(),
        Box::new(Mode::new("beta", 2.0, log)),
        set.clone(),
    );
    d
}

fn near(out: &FrameOut) -> Option<f32> {
    out.camera.as_ref().map(|c| c.near())
}

#[test]
fn a_second_mod_is_refused_with_the_name_of_the_holder() {
    let log = Log::default();
    let mut d = two_modes(&log);
    let mut host = MockClientCtx::new();
    assert_eq!(d.control_holder(), None);
    d.on_hotkey(&mut host, 0, "enter");
    assert_eq!(d.control_holder(), Some((0, &id("alpha"))));
    d.on_hotkey(&mut host, 1, "enter");
    d.on_hotkey(&mut host, 1, "who");
    d.on_hotkey(&mut host, 1, "lock");
    let log = log.borrow();
    assert!(
        log[0].starts_with("beta:refused:")
            && log[0].contains("`alpha`")
            && log[0].contains("leave its mode"),
        "{log:?}"
    );
    assert_eq!(log[1], "beta:holder Some(\"alpha\")");
    assert!(
        log[2].starts_with("beta:lock:") && log[2].contains("`alpha`"),
        "{log:?}"
    );
    assert!(
        host.pointer_lock_requests.is_empty(),
        "the lock never reached the host"
    );
}

#[test]
fn only_the_holder_s_camera_and_input_count() {
    let log = Log::default();
    let mut d = two_modes(&log);
    let mut host = MockClientCtx::new();
    // Beta takes the control and gives it back; then alpha takes it.
    d.on_hotkey(&mut host, 1, "enter");
    d.on_hotkey(&mut host, 1, "leave");
    d.on_hotkey(&mut host, 0, "enter");
    let out = d.frame(&mut host, 0.016);
    assert_eq!(near(&out), Some(1.0));
    assert!(out.suppress_ui);
    // Only the holder is offered input.
    log.borrow_mut().clear();
    assert_eq!(d.claim_input(&mut host, &key()), Claim::Consume);
    assert_eq!(*log.borrow(), ["alpha:claim"]);
    // Alpha leaves: beta gets the events again and may take over.
    d.on_hotkey(&mut host, 0, "leave");
    assert_eq!(d.control_holder(), None);
    log.borrow_mut().clear();
    assert_eq!(d.claim_input(&mut host, &key()), Claim::Pass);
    assert_eq!(*log.borrow(), ["alpha:claim", "beta:claim"]);
    d.on_hotkey(&mut host, 1, "enter");
    assert_eq!(d.control_holder(), Some((1, &id("beta"))));
    assert_eq!(
        near(&d.frame(&mut host, 0.016)),
        Some(2.0),
        "the later mod holds now"
    );
}

#[test]
fn a_camera_from_another_mod_is_dropped_while_one_holds_with_one_line() {
    let log = Log::default();
    let set = vec![info(0, "alpha", BOTH), info(1, "beta", BOTH)];
    let mut d = ModDispatcher::new();
    let mut forced = Mode::new("alpha", 1.0, &log);
    forced.force = true;
    d.push(set[0].clone(), Box::new(forced), set.clone());
    d.push(
        set[1].clone(),
        Box::new(Mode::new("beta", 2.0, &log)),
        set.clone(),
    );
    let mut host = MockClientCtx::new();
    // Alpha's override wins this frame, so alpha holds implicitly: beta is refused.
    assert_eq!(near(&d.frame(&mut host, 0.016)), Some(1.0));
    assert_eq!(d.control_holder(), Some((0, &id("alpha"))));
    d.on_hotkey(&mut host, 1, "enter");
    assert!(log.borrow()[0].contains("`alpha`"), "{:?}", log.borrow());
    // Alpha gives the control up (it keeps returning a camera, now without the right to).
    d.on_hotkey(&mut host, 0, "enter");
    d.on_hotkey(&mut host, 0, "leave");
    assert_eq!(d.control_holder(), None);
    // Beta acquires: from now on alpha's override is dropped, with one line in the log.
    d.on_hotkey(&mut host, 1, "enter");
    for _ in 0..3 {
        let out = d.frame(&mut host, 0.016);
        assert_eq!(
            near(&out),
            Some(2.0),
            "the holder's camera, not the first in load order"
        );
    }
    let lines = host
        .logs
        .iter()
        .filter(|(_, m)| m.contains("holds the camera"))
        .count();
    assert_eq!(lines, 1, "{:?}", host.logs);
}

#[test]
fn a_1_0_mod_in_its_mode_holds_until_it_returns_no_camera() {
    let log = Log::default();
    let set = vec![info(0, "blocks", BOTH), info(1, "beta", BOTH)];
    let mut d = ModDispatcher::new();
    d.push(
        set[0].clone(),
        Box::new(Legacy { in_mode: false }),
        set.clone(),
    );
    d.push(
        set[1].clone(),
        Box::new(Mode::new("beta", 2.0, &log)),
        set.clone(),
    );
    let mut host = MockClientCtx::new();
    d.on_hotkey(&mut host, 0, "enter");
    assert_eq!(near(&d.frame(&mut host, 0.016)), Some(9.0));
    d.on_hotkey(&mut host, 1, "enter");
    assert!(log.borrow()[0].contains("`blocks`"), "{:?}", log.borrow());
    d.on_hotkey(&mut host, 0, "leave");
    assert!(d.frame(&mut host, 0.016).camera.is_none());
    assert_eq!(d.control_holder(), None);
    d.on_hotkey(&mut host, 1, "enter");
    assert_eq!(d.control_holder(), Some((1, &id("beta"))));
}

#[test]
fn detaching_or_switching_off_the_holder_frees_the_control() {
    let log = Log::default();
    let mut d = two_modes(&log);
    let mut host = MockClientCtx::new();
    d.on_hotkey(&mut host, 0, "enter");
    d.detach_one(&mut host, 0);
    assert_eq!(d.control_holder(), None);
    d.on_hotkey(&mut host, 1, "enter");
    assert!(d.set_enabled(1, false));
    assert_eq!(d.control_holder(), None);
    d.set_enabled(1, true);
    d.on_hotkey(&mut host, 1, "enter");
    d.detach(&mut host);
    assert_eq!(d.control_holder(), None);
}

#[test]
fn acquire_needs_camera_or_input_claim() {
    let log = Log::default();
    let set = vec![info(0, "alpha", &[]), info(1, "beta", &[Cap::InputClaim])];
    let mut d = ModDispatcher::new();
    d.push(
        set[0].clone(),
        Box::new(Mode::new("alpha", 1.0, &log)),
        set.clone(),
    );
    d.push(
        set[1].clone(),
        Box::new(Mode::new("beta", 2.0, &log)),
        set.clone(),
    );
    let mut host = MockClientCtx::new();
    d.on_hotkey(&mut host, 0, "enter");
    assert!(
        log.borrow()[0].contains("input.claim"),
        "{:?}",
        log.borrow()
    );
    d.on_hotkey(&mut host, 1, "enter");
    assert_eq!(d.control_holder(), Some((1, &id("beta"))));
}

#[test]
fn the_mock_alone_plays_another_mod_in_its_mode() {
    let mut host = MockClientCtx::new();
    let caps = Caps::of(BOTH);
    {
        let mut g = Guarded::new(&mut host, &caps);
        assert_eq!(g.acquire_control(), Ok(()));
        assert_eq!(g.control_holder(), Some(id("mock")));
        g.release_control();
        assert_eq!(g.control_holder(), None);
    }
    host.control_holder = Some(id("other"));
    let mut g = Guarded::new(&mut host, &caps);
    assert_eq!(
        g.acquire_control(),
        Err(CtxError::Busy {
            holder: id("other")
        })
    );
    assert!(matches!(
        g.request_pointer_lock(true),
        Err(CtxError::Busy { .. })
    ));
    assert_eq!(
        g.request_pointer_lock(false),
        Ok(()),
        "letting go is always allowed"
    );
}
