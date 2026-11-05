//! `platform::InputEvent` to `egui::RawInput`, plus the routing rule (play-ui §3.1).
//!
//! Positions arrive in physical pixels (Y down) and go to egui in points. egui sees every
//! event; [`InputBridge::handle`] says whether the map camera should see it too:
//! - a press that starts over egui belongs to egui until it ends (the map never sees the release);
//! - a press that starts elsewhere belongs to the map, even if the cursor then crosses a panel;
//! - the wheel goes to egui over egui, else to the map;
//! - key presses go to the map unless egui has a focused widget; releases always reach the map,
//!   so a held arrow key never sticks.

use std::collections::BTreeSet;

use egui::{Event, Modifiers as EModifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use platform::{
    InputEvent, Key, KeyEvent, Modifiers, MouseButton, NamedKey, PhysicalPosition, PointerEvent,
    PointerId, PointerKind, PointerPhase, WheelDelta,
};

/// All touches come from one logical device.
const FIRST_TOUCH_DEVICE: u64 = 1;

#[derive(Debug)]
pub struct InputBridge {
    events: Vec<Event>,
    pixels_per_point: f32,
    focused: bool,
    /// Pointers whose current press started over egui.
    egui_presses: BTreeSet<PointerId>,
    /// The modifiers of the newest event, for `RawInput::modifiers` (Shift+click on a button).
    modifiers: EModifiers,
}

impl Default for InputBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl InputBridge {
    pub fn new() -> Self {
        Self {
            events: Vec::new(),
            pixels_per_point: 1.0,
            focused: true,
            egui_presses: BTreeSet::new(),
            modifiers: EModifiers::default(),
        }
    }

    /// Device pixels per egui point; positions are divided by it.
    pub fn set_pixels_per_point(&mut self, scale: f32) {
        if scale.is_finite() && scale > 0.0 {
            self.pixels_per_point = scale;
        }
    }

    pub fn pixels_per_point(&self) -> f32 {
        self.pixels_per_point
    }

    /// Feeds an event to egui and returns whether the map should handle it too.
    ///
    /// Keys: only a focused text edit takes key presses and text. Otherwise presses go to the map
    /// alone and egui does not see them: a button keeps the focus after a click, and egui would
    /// both swallow the next Enter (`egui_wants_keyboard_input` is true for any focused widget)
    /// and press that button again with it.
    pub fn handle(&mut self, event: &InputEvent, ctx: &egui::Context) -> bool {
        self.track_modifiers(event);
        match event {
            InputEvent::Key(k) if k.pressed => {
                let typing = ctx.text_edit_focused();
                if typing {
                    self.push(event);
                }
                !typing
            }
            InputEvent::Text(_) => {
                let typing = ctx.text_edit_focused();
                if typing {
                    self.push(event);
                }
                !typing
            }
            _ => {
                self.push(event);
                match event {
                    InputEvent::Pointer(p) => self.route_pointer(p, ctx),
                    InputEvent::Wheel(w) => !self.over_egui(w.position, ctx),
                    // Releases always reach the map, so a held key never sticks.
                    InputEvent::Key(_)
                    | InputEvent::Text(_)
                    | InputEvent::Resized(_)
                    | InputEvent::ScaleFactor(_)
                    | InputEvent::Focused(_) => true,
                }
            }
        }
    }

    fn route_pointer(&mut self, p: &PointerEvent, ctx: &egui::Context) -> bool {
        match p.phase {
            PointerPhase::Down => {
                if self.egui_presses.contains(&p.id) || self.over_egui(p.position, ctx) {
                    self.egui_presses.insert(p.id);
                    false
                } else {
                    true
                }
            }
            PointerPhase::Move => !self.egui_presses.contains(&p.id),
            PointerPhase::Up | PointerPhase::Cancel | PointerPhase::Left => {
                !self.egui_presses.remove(&p.id)
            }
        }
    }

    /// Is egui under this physical position? Floating areas and windows are tested at the exact
    /// position; panels use egui's own answer, which lags one frame.
    fn over_egui(&self, at: PhysicalPosition, ctx: &egui::Context) -> bool {
        let pos = self.to_points(at);
        ctx.layer_id_at(pos)
            .is_some_and(|l| l.order != egui::Order::Background)
            || crate::labels::hit(ctx, pos)
            || ctx.egui_wants_pointer_input()
    }

    fn to_points(&self, at: PhysicalPosition) -> Pos2 {
        let s = f64::from(self.pixels_per_point);
        Pos2::new((at.x / s) as f32, (at.y / s) as f32)
    }

    /// A change of the held modifiers becomes `ModifiersChanged`, which sets egui's modifiers.
    fn track_modifiers(&mut self, event: &InputEvent) {
        if let Some(m) = event_modifiers(event)
            && m != self.modifiers
        {
            self.modifiers = m;
            self.events.push(Event::ModifiersChanged(m));
        }
    }

    /// Converts one event into egui events without routing.
    pub fn push(&mut self, event: &InputEvent) {
        self.track_modifiers(event);
        match event {
            InputEvent::Pointer(p) => self.push_pointer(p),
            InputEvent::Wheel(w) => {
                let (unit, delta) = match w.delta {
                    WheelDelta::Lines { x, y } => (egui::MouseWheelUnit::Line, Vec2::new(x, y)),
                    WheelDelta::Pixels { x, y } => (
                        egui::MouseWheelUnit::Point,
                        Vec2::new(
                            (x / f64::from(self.pixels_per_point)) as f32,
                            (y / f64::from(self.pixels_per_point)) as f32,
                        ),
                    ),
                };
                self.events.push(Event::MouseWheel {
                    unit,
                    delta,
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Unknown,
                    modifiers: modifiers(w.modifiers),
                });
            }
            InputEvent::Key(k) => self.push_key(k),
            InputEvent::Text(s) => {
                if !s.is_empty() {
                    self.events.push(Event::Text(s.clone()));
                }
            }
            InputEvent::ScaleFactor(f) => self.set_pixels_per_point(*f as f32),
            InputEvent::Focused(f) => {
                self.focused = *f;
                self.events.push(Event::WindowFocused(*f));
            }
            InputEvent::Resized(_) => {}
        }
    }

    fn push_pointer(&mut self, p: &PointerEvent) {
        let pos = self.to_points(p.position);
        let mods = modifiers(p.modifiers);
        let touch = p.kind == PointerKind::Touch;
        match p.phase {
            PointerPhase::Move => {
                self.events.push(Event::PointerMoved(pos));
                if touch {
                    self.push_touch(p.id, egui::TouchPhase::Move, pos);
                }
            }
            PointerPhase::Down | PointerPhase::Up => {
                let pressed = p.phase == PointerPhase::Down;
                let button = if touch {
                    Some(PointerButton::Primary)
                } else {
                    p.button.and_then(button)
                };
                self.events.push(Event::PointerMoved(pos));
                if let Some(button) = button {
                    self.events.push(Event::PointerButton {
                        pos,
                        button,
                        pressed,
                        modifiers: mods,
                    });
                }
                if touch {
                    let phase = if pressed {
                        egui::TouchPhase::Start
                    } else {
                        egui::TouchPhase::End
                    };
                    self.push_touch(p.id, phase, pos);
                    if !pressed {
                        self.events.push(Event::PointerGone);
                    }
                }
            }
            PointerPhase::Cancel => {
                if touch {
                    self.events.push(Event::PointerButton {
                        pos,
                        button: PointerButton::Primary,
                        pressed: false,
                        modifiers: mods,
                    });
                    self.push_touch(p.id, egui::TouchPhase::Cancel, pos);
                }
                self.events.push(Event::PointerGone);
            }
            PointerPhase::Left => self.events.push(Event::PointerGone),
        }
    }

    fn push_touch(&mut self, id: PointerId, phase: egui::TouchPhase, pos: Pos2) {
        self.events.push(Event::Touch {
            device_id: egui::TouchDeviceId(FIRST_TOUCH_DEVICE),
            id: egui::TouchId(id.0),
            phase,
            pos,
            force: None,
        });
    }

    fn push_key(&mut self, k: &KeyEvent) {
        let mods = modifiers(k.modifiers);
        if let Some(key) = egui_key(&k.key) {
            self.events.push(Event::Key {
                key,
                physical_key: None,
                pressed: k.pressed,
                repeat: k.repeat,
                modifiers: mods,
            });
        }
        // Characters become text unless a command modifier is held. Enter and Tab are keys only.
        if k.pressed && !k.modifiers.ctrl && !k.modifiers.meta {
            match &k.key {
                Key::Character(c) if !c.chars().all(char::is_control) => {
                    self.events.push(Event::Text(c.clone()));
                }
                Key::Named(NamedKey::Space) => self.events.push(Event::Text(" ".to_string())),
                _ => {}
            }
        }
    }

    /// The accumulated events as this frame's input. `size` is the surface in physical pixels,
    /// `time` is seconds on any monotonic clock.
    pub fn take(&mut self, size: (u32, u32), scale: f32, time: f64) -> RawInput {
        self.set_pixels_per_point(scale);
        let ppp = self.pixels_per_point;
        let mut raw = RawInput {
            screen_rect: Some(Rect::from_min_size(
                Pos2::ZERO,
                Vec2::new(size.0.max(1) as f32 / ppp, size.1.max(1) as f32 / ppp),
            )),
            time: Some(time),
            focused: self.focused,
            events: std::mem::take(&mut self.events),
            ..RawInput::default()
        };
        if let Some(v) = raw.viewports.get_mut(&egui::ViewportId::ROOT) {
            v.native_pixels_per_point = Some(ppp);
        }
        raw
    }
}

fn modifiers(m: Modifiers) -> EModifiers {
    EModifiers {
        alt: m.alt,
        ctrl: m.ctrl,
        shift: m.shift,
        mac_cmd: false,
        // Windows, Linux and web: Ctrl is the command key. Meta stands in where Ctrl is not held.
        command: m.ctrl || m.meta,
    }
}

/// The modifiers an event carries, as egui's.
fn event_modifiers(event: &InputEvent) -> Option<EModifiers> {
    match event {
        InputEvent::Pointer(p) => Some(modifiers(p.modifiers)),
        InputEvent::Wheel(w) => Some(modifiers(w.modifiers)),
        InputEvent::Key(k) => Some(modifiers(k.modifiers)),
        _ => None,
    }
}

fn button(b: MouseButton) -> Option<PointerButton> {
    Some(match b {
        MouseButton::Left => PointerButton::Primary,
        MouseButton::Right => PointerButton::Secondary,
        MouseButton::Middle => PointerButton::Middle,
        MouseButton::Back => PointerButton::Extra1,
        MouseButton::Forward => PointerButton::Extra2,
        MouseButton::Other(_) => return None,
    })
}

fn egui_key(key: &Key) -> Option<egui::Key> {
    use egui::Key as K;
    Some(match key {
        Key::Named(n) => match n {
            NamedKey::Enter => K::Enter,
            NamedKey::Escape => K::Escape,
            NamedKey::Tab => K::Tab,
            NamedKey::Backspace => K::Backspace,
            NamedKey::Delete => K::Delete,
            NamedKey::Insert => K::Insert,
            NamedKey::Space => K::Space,
            NamedKey::ArrowUp => K::ArrowUp,
            NamedKey::ArrowDown => K::ArrowDown,
            NamedKey::ArrowLeft => K::ArrowLeft,
            NamedKey::ArrowRight => K::ArrowRight,
            NamedKey::Home => K::Home,
            NamedKey::End => K::End,
            NamedKey::PageUp => K::PageUp,
            NamedKey::PageDown => K::PageDown,
            NamedKey::F1 => K::F1,
            NamedKey::F2 => K::F2,
            NamedKey::F3 => K::F3,
            NamedKey::F4 => K::F4,
            NamedKey::F5 => K::F5,
            NamedKey::F6 => K::F6,
            NamedKey::F7 => K::F7,
            NamedKey::F8 => K::F8,
            NamedKey::F9 => K::F9,
            NamedKey::F10 => K::F10,
            NamedKey::F11 => K::F11,
            NamedKey::F12 => K::F12,
            NamedKey::Shift | NamedKey::Control | NamedKey::Alt | NamedKey::Meta => return None,
        },
        Key::Character(c) => K::from_name(&c.to_uppercase())?,
        Key::Unidentified => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform::{PhysicalSize, WheelEvent};

    fn pos(x: f64, y: f64) -> PhysicalPosition {
        PhysicalPosition { x, y }
    }

    fn mouse(phase: PointerPhase, x: f64, y: f64, b: Option<MouseButton>) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            id: PointerId::MOUSE,
            kind: PointerKind::Mouse,
            phase,
            position: pos(x, y),
            button: b,
            modifiers: Modifiers::default(),
        })
    }

    fn key(key: Key, pressed: bool, modifiers: Modifiers) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key,
            code: None,
            pressed,
            repeat: false,
            modifiers,
        })
    }

    fn raw(b: &mut InputBridge) -> RawInput {
        b.take((800, 600), 1.0, 0.0)
    }

    #[test]
    fn pointer_positions_are_divided_by_scale() {
        let mut b = InputBridge::new();
        b.set_pixels_per_point(2.0);
        b.push(&mouse(
            PointerPhase::Down,
            200.0,
            100.0,
            Some(MouseButton::Right),
        ));
        let r = b.take((800, 600), 2.0, 1.5);
        assert_eq!(r.screen_rect.unwrap().max, Pos2::new(400.0, 300.0));
        assert_eq!(r.time, Some(1.5));
        assert!(
            r.events
                .contains(&Event::PointerMoved(Pos2::new(100.0, 50.0)))
        );
        assert!(r.events.iter().any(|e| matches!(e,
            Event::PointerButton { pos, button: PointerButton::Secondary, pressed: true, .. }
                if *pos == Pos2::new(100.0, 50.0))));
        assert!(b.take((800, 600), 2.0, 2.0).events.is_empty(), "taken once");
    }

    #[test]
    fn buttons_up_leave_and_other() {
        let mut b = InputBridge::new();
        b.push(&mouse(PointerPhase::Up, 5.0, 6.0, Some(MouseButton::Left)));
        b.push(&mouse(
            PointerPhase::Down,
            5.0,
            6.0,
            Some(MouseButton::Other(9)),
        ));
        b.push(&mouse(PointerPhase::Left, 0.0, 0.0, None));
        let r = raw(&mut b);
        assert!(r.events.iter().any(|e| matches!(
            e,
            Event::PointerButton {
                button: PointerButton::Primary,
                pressed: false,
                ..
            }
        )));
        assert_eq!(
            r.events
                .iter()
                .filter(|e| matches!(e, Event::PointerButton { .. }))
                .count(),
            1,
            "unknown button is dropped"
        );
        assert_eq!(r.events.last(), Some(&Event::PointerGone));
    }

    #[test]
    fn wheel_lines_and_pixels() {
        let mut b = InputBridge::new();
        b.set_pixels_per_point(2.0);
        let w = |delta| {
            InputEvent::Wheel(WheelEvent {
                delta,
                position: pos(0.0, 0.0),
                modifiers: Modifiers::default(),
            })
        };
        b.push(&w(WheelDelta::Lines { x: 0.0, y: 1.0 }));
        b.push(&w(WheelDelta::Pixels { x: 20.0, y: -40.0 }));
        let r = b.take((100, 100), 2.0, 0.0);
        assert!(
            matches!(&r.events[0], Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta, .. }
            if *delta == Vec2::new(0.0, 1.0))
        );
        assert!(
            matches!(&r.events[1], Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta, .. }
            if *delta == Vec2::new(10.0, -20.0))
        );
    }

    #[test]
    fn characters_become_text_unless_ctrl() {
        let mut b = InputBridge::new();
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        b.push(&key(Key::Character("a".into()), true, Modifiers::default()));
        b.push(&key(Key::Character("B".into()), true, shift));
        b.push(&key(
            Key::Named(NamedKey::Space),
            true,
            Modifiers::default(),
        ));
        b.push(&key(
            Key::Named(NamedKey::Enter),
            true,
            Modifiers::default(),
        ));
        b.push(&key(
            Key::Character("c".into()),
            true,
            Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
        ));
        b.push(&key(
            Key::Character("a".into()),
            false,
            Modifiers::default(),
        ));
        let r = raw(&mut b);
        let texts: Vec<&str> = r
            .events
            .iter()
            .filter_map(|e| match e {
                Event::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, ["a", "B", " "]);
        let keys: Vec<(egui::Key, bool)> = r
            .events
            .iter()
            .filter_map(|e| match e {
                Event::Key { key, pressed, .. } => Some((*key, *pressed)),
                _ => None,
            })
            .collect();
        assert_eq!(
            keys,
            [
                (egui::Key::A, true),
                (egui::Key::B, true),
                (egui::Key::Space, true),
                (egui::Key::Enter, true),
                (egui::Key::C, true),
                (egui::Key::A, false),
            ]
        );
        assert!(r.events.iter().any(|e| matches!(e,
            Event::Key { key: egui::Key::C, modifiers, .. } if modifiers.ctrl && modifiers.command)));
    }

    #[test]
    fn resize_scale_and_focus() {
        let mut b = InputBridge::new();
        b.push(&InputEvent::Resized(PhysicalSize {
            width: 1,
            height: 1,
        }));
        b.push(&InputEvent::ScaleFactor(1.5));
        b.push(&InputEvent::Focused(false));
        assert_eq!(b.pixels_per_point(), 1.5);
        let r = b.take((800, 600), 1.5, 0.0);
        assert!(!r.focused);
        assert_eq!(r.events, [Event::WindowFocused(false)]);
        b.set_pixels_per_point(f32::NAN);
        b.set_pixels_per_point(0.0);
        assert_eq!(b.pixels_per_point(), 1.5);
    }

    #[test]
    fn touch_sends_pointer_and_touch_events() {
        let mut b = InputBridge::new();
        let t = |phase, x| {
            InputEvent::Pointer(PointerEvent {
                id: PointerId(7),
                kind: PointerKind::Touch,
                phase,
                position: pos(x, 10.0),
                button: None,
                modifiers: Modifiers::default(),
            })
        };
        b.push(&t(PointerPhase::Down, 10.0));
        b.push(&t(PointerPhase::Up, 10.0));
        let r = raw(&mut b);
        assert!(r.events.iter().any(|e| matches!(
            e,
            Event::PointerButton {
                button: PointerButton::Primary,
                pressed: true,
                ..
            }
        )));
        assert!(r.events.iter().any(|e| matches!(e,
            Event::Touch { phase: egui::TouchPhase::End, id, .. } if id.0 == 7)));
        assert_eq!(r.events.last(), Some(&Event::PointerGone));
    }

    // Routing against a live egui context with one window at (8, 8).
    fn ctx_with_window() -> (egui::Context, Rect) {
        let ctx = egui::Context::default();
        let mut b = InputBridge::new();
        let mut rect = Rect::NOTHING;
        for _ in 0..2 {
            let out = ctx.run_pass(b.take((800, 600), 1.0, 0.0), |ctx| {
                let r = egui::Window::new("w")
                    .anchor(egui::Align2::LEFT_TOP, [8.0, 8.0])
                    .show(ctx, |ui| {
                        ui.label("hello");
                    });
                rect = r.unwrap().response.rect;
            });
            out.drop_without_applying_deltas();
        }
        (ctx, rect)
    }

    #[test]
    fn press_over_egui_stays_with_egui() {
        let (ctx, rect) = ctx_with_window();
        let inside = rect.center();
        let (ix, iy) = (f64::from(inside.x), f64::from(inside.y));
        let mut b = InputBridge::new();
        let l = Some(MouseButton::Left);
        assert!(
            b.handle(&mouse(PointerPhase::Move, ix, iy, None), &ctx),
            "moves always reach the map"
        );
        assert!(!b.handle(&mouse(PointerPhase::Down, ix, iy, l), &ctx));
        assert!(
            !b.handle(&mouse(PointerPhase::Move, 700.0, 500.0, None), &ctx),
            "drag out of the panel"
        );
        assert!(!b.handle(&mouse(PointerPhase::Up, 700.0, 500.0, l), &ctx));
        assert!(b.handle(&mouse(PointerPhase::Move, 700.0, 500.0, None), &ctx));
    }

    #[test]
    fn press_outside_stays_with_the_map_even_over_a_panel() {
        let (ctx, rect) = ctx_with_window();
        let inside = rect.center();
        let mut b = InputBridge::new();
        let l = Some(MouseButton::Left);
        assert!(b.handle(&mouse(PointerPhase::Down, 700.0, 500.0, l), &ctx));
        assert!(b.handle(
            &mouse(
                PointerPhase::Move,
                f64::from(inside.x),
                f64::from(inside.y),
                None
            ),
            &ctx
        ));
        assert!(b.handle(
            &mouse(
                PointerPhase::Up,
                f64::from(inside.x),
                f64::from(inside.y),
                l
            ),
            &ctx
        ));
    }

    #[test]
    fn wheel_and_keys() {
        let (ctx, rect) = ctx_with_window();
        let mut b = InputBridge::new();
        let wheel = |p: Pos2| {
            InputEvent::Wheel(WheelEvent {
                delta: WheelDelta::Lines { x: 0.0, y: 1.0 },
                position: pos(f64::from(p.x), f64::from(p.y)),
                modifiers: Modifiers::default(),
            })
        };
        assert!(!b.handle(&wheel(rect.center()), &ctx));
        assert!(b.handle(&wheel(Pos2::new(700.0, 500.0)), &ctx));
        let k = key(Key::Named(NamedKey::Enter), true, Modifiers::default());
        assert!(b.handle(&k, &ctx), "no focused widget: the map gets keys");
    }

    /// A clicked button keeps egui's focus; Enter must still reach the map, every time, and must
    /// not press the button again.
    #[test]
    fn a_focused_button_does_not_take_enter() {
        let ctx = egui::Context::default();
        let mut b = InputBridge::new();
        let mut presses = 0;
        let enter = key(Key::Named(NamedKey::Enter), true, Modifiers::default());
        for frame in 0..6 {
            if frame == 3 {
                for _ in 0..3 {
                    assert!(b.handle(&enter, &ctx), "Enter goes to the map");
                }
            }
            let out = ctx.run_pass(b.take((800, 600), 1.0, 0.0), |ctx| {
                egui::Window::new("b").show(ctx, |ui| {
                    let r = ui.button("press");
                    r.request_focus();
                    if r.clicked() {
                        presses += 1;
                    }
                });
            });
            out.drop_without_applying_deltas();
        }
        assert!(ctx.egui_wants_keyboard_input(), "the button has the focus");
        assert_eq!(presses, 0, "egui never saw the Enter presses");
    }

    #[test]
    fn modifier_changes_reach_egui() {
        let mut b = InputBridge::new();
        let shift = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        b.push(&key(Key::Named(NamedKey::Shift), true, shift));
        b.push(&mouse(PointerPhase::Move, 1.0, 1.0, None));
        let r = raw(&mut b);
        let changes: Vec<_> = r
            .events
            .iter()
            .filter_map(|e| match e {
                Event::ModifiersChanged(m) => Some(m.shift),
                _ => None,
            })
            .collect();
        // Shift down, then the pointer event without modifiers releases it.
        assert_eq!(changes, [true, false]);
    }

    #[test]
    fn focused_text_edit_takes_keys_but_not_releases() {
        let ctx = egui::Context::default();
        let mut b = InputBridge::new();
        let mut text = String::new();
        for _ in 0..3 {
            let out = ctx.run_pass(b.take((800, 600), 1.0, 0.0), |ctx| {
                egui::Window::new("t").show(ctx, |ui| {
                    let r = ui.text_edit_singleline(&mut text);
                    r.request_focus();
                });
            });
            out.drop_without_applying_deltas();
        }
        assert!(ctx.egui_wants_keyboard_input());
        let down = key(Key::Character("w".into()), true, Modifiers::default());
        let up = key(Key::Character("w".into()), false, Modifiers::default());
        assert!(!b.handle(&down, &ctx));
        assert!(b.handle(&up, &ctx));
        assert!(!b.handle(&InputEvent::Text("x".into()), &ctx));
    }
}
