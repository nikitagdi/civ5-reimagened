//! Input to `GameCamera`. All numbers live in `render::camera_params`.
//!
//! Wheel zooms toward the cursor (sign only: one notch is one step, RV-2), `+`/`-`/PgUp/PgDn toward
//! the screen centre (ours, SIMP-020), the left button (or one finger) drags the map (RV-3), arrows
//! and the screen edge scroll, pinch zooms. Release after a drag glides (RV-5).

use std::collections::BTreeMap;

use glam::Vec2;
use platform::{
    InputEvent, Key, MouseButton, NamedKey, PhysicalPosition, PointerEvent, PointerId, PointerKind,
    PointerPhase, WheelDelta,
};
use render::camera_params as p;
use render::{GameCamera, pixels_to_ndc};

/// A press that becomes a drag after moving more than `DRAG_THRESHOLD_PX` along X or Y.
#[derive(Debug, Clone, Copy)]
struct Press {
    down: PhysicalPosition,
    /// Previous pointer position; the drag event carries the step from it, like the original.
    last: PhysicalPosition,
    dragging: bool,
}

impl Press {
    fn new(at: PhysicalPosition) -> Self {
        Self {
            down: at,
            last: at,
            dragging: false,
        }
    }

    /// Segment to apply for a move to `to`, once the press counts as a drag. The move that crosses
    /// the threshold drags by its own step only, the dead zone is not made up.
    // re: UIManager::ProcessCameraDrag (UIManager.cpp:48-62)
    fn moved(&mut self, to: PhysicalPosition) -> Option<(PhysicalPosition, PhysicalPosition)> {
        let seg = (self.last, to);
        self.last = to;
        if !self.dragging {
            let limit = f64::from(p::DRAG_THRESHOLD_PX);
            if !((to.x - self.down.x).abs() > limit || (to.y - self.down.y).abs() > limit) {
                return None;
            }
            self.dragging = true;
        }
        Some(seg)
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct HeldKeys {
    left: bool,
    right: bool,
    up: bool,
    down: bool,
}

#[derive(Debug, Clone)]
pub struct MapCameraController {
    width: f32,
    height: f32,
    press: Option<Press>,
    touches: BTreeMap<PointerId, PhysicalPosition>,
    touch_press: Option<(PointerId, Press)>,
    keys: HeldKeys,
    cursor: Option<PhysicalPosition>,
    edge_scroll: bool,
    /// Focus shift of the drag moves since the last `update`.
    pending_shift: Vec2,
    /// Last `INERTIA_SAMPLES` drag frames: (seconds, focus shift), a ring.
    samples: [(f32, Vec2); p::INERTIA_SAMPLES],
    sample_index: usize,
    /// A drag move happened in this frame / in the frame before (bit 0x40 of the original lives
    /// one frame).
    moved_now: bool,
    moved_prev: bool,
    /// The drag button was released in this frame.
    released: bool,
    /// Wheel pixels not yet worth a notch (pixel-mode wheels).
    wheel_pixels: f64,
    click: Option<PhysicalPosition>,
    /// Right button went down at this position (no drag for it).
    right_down: Option<PhysicalPosition>,
    right_click: Option<PhysicalPosition>,
}

impl MapCameraController {
    /// For a `width × height` physical pixel viewport.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width: width.max(1) as f32,
            height: height.max(1) as f32,
            press: None,
            touches: BTreeMap::new(),
            touch_press: None,
            keys: HeldKeys::default(),
            cursor: None,
            edge_scroll: false,
            pending_shift: Vec2::ZERO,
            samples: [(0.0, Vec2::ZERO); p::INERTIA_SAMPLES],
            sample_index: 0,
            moved_now: false,
            moved_prev: false,
            released: false,
            wheel_pixels: 0.0,
            click: None,
            right_down: None,
            right_click: None,
        }
    }

    /// Screen-edge scroll works in fullscreen only.
    pub fn set_edge_scroll(&mut self, on: bool) {
        self.edge_scroll = on;
    }

    /// A press that ended without becoming a drag (a click), once.
    pub fn take_click(&mut self) -> Option<PhysicalPosition> {
        self.click.take()
    }

    /// A right button press and release within `DRAG_THRESHOLD_PX` (a command click), once.
    pub fn take_right_click(&mut self) -> Option<PhysicalPosition> {
        self.right_click.take()
    }

    /// Camera ray through a physical pixel position.
    pub fn ray_at(&self, at: PhysicalPosition, camera: &GameCamera) -> render::Ray {
        camera.unproject(self.ndc(at), self.aspect())
    }

    /// The pointer over the window, `None` once it has left.
    pub fn cursor(&self) -> Option<PhysicalPosition> {
        self.cursor
    }

    pub fn is_dragging(&self) -> bool {
        self.press.is_some_and(|d| d.dragging) || self.touch_press.is_some_and(|(_, d)| d.dragging)
    }

    /// The left button (or a finger) is down on the map, dragging or not yet.
    pub fn is_pressed(&self) -> bool {
        self.press.is_some() || self.touch_press.is_some()
    }

    fn aspect(&self) -> f32 {
        self.width / self.height
    }

    fn ndc(&self, at: PhysicalPosition) -> Vec2 {
        pixels_to_ndc(at.x as f32, at.y as f32, self.width, self.height)
    }

    /// `true` if the event changed camera or gesture state.
    pub fn handle(&mut self, event: &InputEvent, camera: &mut GameCamera) -> bool {
        match event {
            InputEvent::Resized(size) => {
                self.width = size.width.max(1) as f32;
                self.height = size.height.max(1) as f32;
                false
            }
            InputEvent::Focused(false) => {
                self.press = None;
                self.touch_press = None;
                self.touches.clear();
                self.keys = HeldKeys::default();
                self.cursor = None;
                false
            }
            InputEvent::Wheel(w) => {
                // Only the sign of a wheel event counts: one event is one notch (RV-2).
                let notches = match w.delta {
                    WheelDelta::Lines { y, .. } => {
                        self.wheel_pixels = 0.0;
                        if y > 0.0 {
                            1
                        } else if y < 0.0 {
                            -1
                        } else {
                            0
                        }
                    }
                    WheelDelta::Pixels { y, .. } => self.wheel_pixel_notches(y),
                };
                // Scroll up zooms in, which lowers the level.
                let at = self.ndc(w.position);
                let step = if notches > 0 {
                    -p::WHEEL_STEP_LEVELS
                } else {
                    p::WHEEL_STEP_LEVELS
                };
                for _ in 0..notches.unsigned_abs() {
                    camera.zoom_at(step, at, self.aspect());
                }
                notches != 0
            }
            InputEvent::Key(k) => self.key(k, camera),
            InputEvent::Pointer(ptr) => match ptr.kind {
                PointerKind::Mouse | PointerKind::Pen => self.mouse(ptr, camera),
                PointerKind::Touch => self.touch(ptr, camera),
            },
            _ => false,
        }
    }

    /// Whole notches (signed, up is positive) in the pixel wheel delta `y`; the rest is kept.
    fn wheel_pixel_notches(&mut self, y: f64) -> i32 {
        if y * self.wheel_pixels < 0.0 {
            self.wheel_pixels = 0.0;
        }
        self.wheel_pixels += y;
        let notches = (self.wheel_pixels / p::PIXELS_PER_WHEEL_LINE).trunc();
        self.wheel_pixels -= notches * p::PIXELS_PER_WHEEL_LINE;
        notches as i32
    }

    fn key(&mut self, k: &platform::KeyEvent, camera: &mut GameCamera) -> bool {
        let pressed = k.pressed;
        let held = match &k.key {
            Key::Named(NamedKey::ArrowLeft) => Some(&mut self.keys.left),
            Key::Named(NamedKey::ArrowRight) => Some(&mut self.keys.right),
            Key::Named(NamedKey::ArrowUp) => Some(&mut self.keys.up),
            Key::Named(NamedKey::ArrowDown) => Some(&mut self.keys.down),
            _ => None,
        };
        if let Some(h) = held {
            *h = pressed;
            return true;
        }
        if !pressed {
            return false;
        }
        // Negative levels zoom in.
        let delta = match &k.key {
            Key::Named(NamedKey::PageUp) => -p::KEY_ZOOM_STEP_LEVELS,
            Key::Named(NamedKey::PageDown) => p::KEY_ZOOM_STEP_LEVELS,
            Key::Character(c) if c == "+" || c == "=" => -p::KEY_ZOOM_STEP_LEVELS,
            Key::Character(c) if c == "-" || c == "_" => p::KEY_ZOOM_STEP_LEVELS,
            _ => return false,
        };
        camera.zoom_at(delta, Vec2::ZERO, self.aspect());
        true
    }

    fn apply_drag(&mut self, seg: (PhysicalPosition, PhysicalPosition), camera: &mut GameCamera) {
        camera.stop_inertia();
        let shift = camera.drag(self.ndc(seg.0), self.ndc(seg.1), self.aspect());
        self.pending_shift += shift;
        self.moved_now = true;
    }

    /// The drag ended; `update` decides about the glide.
    fn release_drag(&mut self) {
        self.released = true;
    }

    fn clear_samples(&mut self) {
        self.samples = [(0.0, Vec2::ZERO); p::INERTIA_SAMPLES];
        self.sample_index = 0;
    }

    fn mouse(&mut self, ptr: &PointerEvent, camera: &mut GameCamera) -> bool {
        match ptr.phase {
            PointerPhase::Down => {
                // re: the left button, when no UI element took it
                let drag_button = MouseButton::Left;
                if ptr.button == Some(MouseButton::Right) {
                    self.right_down = Some(ptr.position);
                    return false;
                }
                if ptr.button != Some(drag_button) {
                    return false;
                }
                camera.stop_inertia();
                self.clear_samples();
                self.press = Some(Press::new(ptr.position));
                true
            }
            PointerPhase::Move => {
                self.cursor = Some(ptr.position);
                let Some(press) = self.press.as_mut() else {
                    return false;
                };
                match press.moved(ptr.position) {
                    Some(seg) => {
                        self.apply_drag(seg, camera);
                        true
                    }
                    None => false,
                }
            }
            PointerPhase::Up => {
                if ptr.button == Some(MouseButton::Right) {
                    if let Some(down) = self.right_down.take() {
                        let d = ((ptr.position.x - down.x).powi(2)
                            + (ptr.position.y - down.y).powi(2))
                        .sqrt();
                        if d < f64::from(p::DRAG_THRESHOLD_PX) {
                            self.right_click = Some(ptr.position);
                        }
                    }
                    return false;
                }
                if ptr.button.is_some() && ptr.button != Some(MouseButton::Left) {
                    return false;
                }
                self.end_press(ptr.position)
            }
            PointerPhase::Cancel => {
                self.press = None;
                false
            }
            PointerPhase::Left => {
                self.cursor = None;
                self.end_press(ptr.position)
            }
        }
    }

    fn end_press(&mut self, at: PhysicalPosition) -> bool {
        let Some(press) = self.press.take() else {
            return false;
        };
        if press.dragging {
            self.release_drag();
        } else {
            self.click = Some(at);
        }
        true
    }

    fn touch(&mut self, ptr: &PointerEvent, camera: &mut GameCamera) -> bool {
        match ptr.phase {
            PointerPhase::Down => {
                camera.stop_inertia();
                self.clear_samples();
                self.touches.insert(ptr.id, ptr.position);
                if self.touches.len() == 1 {
                    self.touch_press = Some((ptr.id, Press::new(ptr.position)));
                } else {
                    // A second finger turns the gesture into pinch.
                    self.touch_press = None;
                }
                true
            }
            PointerPhase::Move => {
                if !self.touches.contains_key(&ptr.id) {
                    return false;
                }
                let before = self.pinch();
                self.touches.insert(ptr.id, ptr.position);
                if let Some((id, press)) = self.touch_press.as_mut() {
                    if *id != ptr.id {
                        return false;
                    }
                    return match press.moved(ptr.position) {
                        Some(seg) => {
                            self.apply_drag(seg, camera);
                            true
                        }
                        None => false,
                    };
                }
                let (Some((m0, d0)), Some((m1, d1))) = (before, self.pinch()) else {
                    return false;
                };
                let aspect = self.aspect();
                camera.drag(self.ndc(m0), self.ndc(m1), aspect);
                if d0 > 1.0 && d1 > 1.0 {
                    // Fingers apart: zoom in, so the level falls by ln(d1/d0) / rate.
                    let delta = -((d1 / d0).ln() as f32) / p::ZOOM_RATE;
                    camera.zoom_at_now(delta, self.ndc(m1), aspect);
                }
                true
            }
            PointerPhase::Up | PointerPhase::Cancel | PointerPhase::Left => {
                let known = self.touches.remove(&ptr.id).is_some();
                if let Some((id, press)) = self.touch_press
                    && id == ptr.id
                {
                    self.touch_press = None;
                    if press.dragging {
                        self.release_drag();
                    } else if ptr.phase == PointerPhase::Up {
                        self.click = Some(ptr.position);
                    }
                }
                known
            }
        }
    }

    /// Midpoint and distance of the first two touches, in id order.
    fn pinch(&self) -> Option<(PhysicalPosition, f64)> {
        let mut it = self.touches.values();
        let (a, b) = (it.next()?, it.next()?);
        Some((
            PhysicalPosition {
                x: (a.x + b.x) * 0.5,
                y: (a.y + b.y) * 0.5,
            },
            ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt(),
        ))
    }

    /// Per frame: keyboard and edge scroll, drag frame sampling, glide.
    // re: CvCamera::Update, drag branch (CvCamera.cpp:213-257)
    pub fn update(&mut self, dt: f32, camera: &mut GameCamera) {
        if dt <= 0.0 {
            return;
        }
        if self.is_dragging() {
            // Button held: only frames with a drag move are recorded.
            if self.moved_now {
                self.samples[self.sample_index] = (dt, self.pending_shift);
                self.sample_index = (self.sample_index + 1) % p::INERTIA_SAMPLES;
            }
        } else if self.released {
            // Glide only if the drag moved in the release frame or the one before.
            if self.moved_now || self.moved_prev {
                self.start_glide(camera);
            }
            self.clear_samples();
        } else {
            let dir = self.scroll_dir();
            camera.scroll(dir, dt);
        }
        self.released = false;
        self.pending_shift = Vec2::ZERO;
        self.moved_prev = self.moved_now;
        self.moved_now = false;
        camera.update(dt);
    }

    /// Release velocity: time-weighted mean of the recorded frames times `INERTIA_RELEASE_SCALE`.
    fn start_glide(&mut self, camera: &mut GameCamera) {
        let (mut shift, mut time) = (Vec2::ZERO, 0.0);
        for &(dt, s) in &self.samples {
            if dt != 0.0 {
                shift += s;
                time += dt;
            }
        }
        if time > 0.0 {
            camera.fling(shift / time * p::INERTIA_RELEASE_SCALE);
        }
    }

    fn scroll_dir(&self) -> Vec2 {
        let axis = |neg: bool, pos: bool| f32::from(u8::from(pos)) - f32::from(u8::from(neg));
        let mut dir = Vec2::new(
            axis(self.keys.left, self.keys.right),
            axis(self.keys.down, self.keys.up),
        );
        if self.edge_scroll
            && let Some(c) = self.cursor
        {
            let e = f64::from(p::EDGE_SCROLL_PX);
            let (w, h) = (f64::from(self.width), f64::from(self.height));
            dir.x += axis(c.x < e, c.x >= w - e);
            dir.y += axis(c.y >= h - e, c.y < e);
        }
        dir.clamp(Vec2::splat(-1.0), Vec2::splat(1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform::{KeyEvent, Modifiers, WheelEvent};

    fn pos(x: f64, y: f64) -> PhysicalPosition {
        PhysicalPosition { x, y }
    }

    fn mouse(phase: PointerPhase, x: f64, y: f64, button: Option<MouseButton>) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            id: PointerId::MOUSE,
            kind: PointerKind::Mouse,
            phase,
            position: pos(x, y),
            button,
            modifiers: Modifiers::default(),
        })
    }

    fn touch(id: u64, phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            id: PointerId(id),
            kind: PointerKind::Touch,
            phase,
            position: pos(x, y),
            button: None,
            modifiers: Modifiers::default(),
        })
    }

    fn wheel(y: f32, at: PhysicalPosition) -> InputEvent {
        InputEvent::Wheel(WheelEvent {
            delta: WheelDelta::Lines { x: 0.0, y },
            position: at,
            modifiers: Modifiers::default(),
        })
    }

    fn key(key: Key, pressed: bool) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key,
            code: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::default(),
        })
    }

    fn setup() -> (MapCameraController, GameCamera) {
        let mut cam = GameCamera::new(Vec2::new(1000.0, -500.0));
        cam.set_level(15.0);
        (MapCameraController::new(1200, 800), cam)
    }

    fn ground_under(ctl: &MapCameraController, cam: &GameCamera, at: PhysicalPosition) -> Vec2 {
        let g = cam.ground_point(ctl.ndc(at), ctl.aspect()).unwrap();
        Vec2::new(g.x, g.z)
    }

    #[test]
    fn wheel_step_is_two_levels_toward_cursor() {
        let (mut ctl, mut cam) = setup();
        let at = pos(900.0, 200.0);
        let before = ground_under(&ctl, &cam, at);
        ctl.handle(&wheel(1.0, at), &mut cam);
        assert!((cam.target_level() - 13.0).abs() < 1e-4);
        assert!(ground_under(&ctl, &cam, at).distance(before) < 0.5);
        ctl.handle(&wheel(-1.0, at), &mut cam);
        assert!((cam.target_level() - 15.0).abs() < 1e-4);
        // RV-2: only the sign counts, the size of the delta is ignored.
        ctl.handle(&wheel(-2.0, at), &mut cam);
        assert!((cam.target_level() - 17.0).abs() < 1e-4);
        ctl.handle(&wheel(0.1, at), &mut cam);
        assert!((cam.target_level() - 15.0).abs() < 1e-4);
        assert!(!ctl.handle(&wheel(0.0, at), &mut cam));
        for _ in 0..10 {
            ctl.handle(&wheel(-10.0, at), &mut cam);
        }
        assert_eq!(cam.target_level(), p::LEVEL_MAX);
    }

    #[test]
    fn wheel_zoom_is_animated_over_frames() {
        let (mut ctl, mut cam) = setup();
        let at = pos(900.0, 200.0);
        ctl.handle(&wheel(1.0, at), &mut cam);
        assert_eq!(cam.level(), 15.0, "the wheel sets the target only");
        let mut levels = Vec::new();
        for _ in 0..300 {
            ctl.update(1.0 / 60.0, &mut cam);
            levels.push(cam.level());
        }
        assert!(
            levels[0] < 15.0 && levels[0] > 13.5,
            "first frame is a small step"
        );
        assert!(levels.windows(2).all(|w| w[1] <= w[0]), "monotone");
        assert!((cam.level() - 13.0).abs() < 0.05, "{}", cam.level());
        assert!(!cam.is_following());
    }

    #[test]
    fn a_drag_during_a_look_at_takes_the_camera_over() {
        let (mut ctl, mut cam) = setup();
        let btn = Some(MouseButton::Left);
        cam.look_at(Vec2::new(3000.0, -800.0));
        ctl.update(1.0 / 60.0, &mut cam);
        assert!(cam.is_following());
        ctl.handle(&mouse(PointerPhase::Down, 600.0, 400.0, btn), &mut cam);
        ctl.handle(&mouse(PointerPhase::Move, 640.0, 400.0, None), &mut cam);
        assert!(!cam.is_following(), "a drag sets the camera, not a target");
        let f = cam.focus();
        ctl.update(1.0 / 60.0, &mut cam);
        assert_eq!(cam.focus(), f);
    }

    #[test]
    fn pixel_wheel_accumulates_to_notches() {
        let (mut ctl, mut cam) = setup();
        let at = pos(600.0, 400.0);
        let px = |y: f64| {
            InputEvent::Wheel(WheelEvent {
                delta: WheelDelta::Pixels { x: 0.0, y },
                position: at,
                modifiers: Modifiers::default(),
            })
        };
        ctl.handle(&px(30.0), &mut cam);
        assert_eq!(cam.target_level(), 15.0, "below one notch");
        ctl.handle(&px(30.0), &mut cam);
        assert!(
            (cam.target_level() - 13.0).abs() < 1e-4,
            "60 px is one notch"
        );
        ctl.handle(&px(-60.0), &mut cam);
        assert!(
            (cam.target_level() - 15.0).abs() < 1e-4,
            "the opposite direction drops the 10 px left"
        );
    }

    #[test]
    fn camera_starts_at_the_startup_game_level() {
        let cam = GameCamera::default();
        assert!((cam.level() - 14.25).abs() < 1e-6);
        assert!((cam.height() - 424.26).abs() < 1.0, "{}", cam.height());
        assert_eq!(p::LEVEL_MIN, 11.0);
    }

    #[test]
    fn zoom_keys_zoom_to_centre() {
        let (mut ctl, mut cam) = setup();
        let focus = cam.focus();
        ctl.handle(&key(Key::Character("+".into()), true), &mut cam);
        assert!((cam.target_level() - 13.0).abs() < 1e-4);
        ctl.handle(&key(Key::Named(NamedKey::PageDown), true), &mut cam);
        ctl.handle(&key(Key::Character("-".into()), true), &mut cam);
        assert!((cam.target_level() - 17.0).abs() < 1e-4);
        assert!(cam.focus().distance(focus) < 0.5);
        assert!(!ctl.handle(&key(Key::Character("-".into()), false), &mut cam));
    }

    #[test]
    fn small_move_is_a_click_not_a_drag() {
        let (mut ctl, mut cam) = setup();
        let focus = cam.focus();
        let btn = Some(MouseButton::Left);
        ctl.handle(&mouse(PointerPhase::Down, 100.0, 100.0, btn), &mut cam);
        ctl.handle(&mouse(PointerPhase::Move, 102.0, 101.0, None), &mut cam);
        assert!(!ctl.is_dragging());
        ctl.handle(&mouse(PointerPhase::Up, 102.0, 101.0, btn), &mut cam);
        assert_eq!(cam.focus(), focus);
        assert_eq!(ctl.take_click(), Some(pos(102.0, 101.0)));
        assert_eq!(ctl.take_click(), None);
    }

    #[test]
    fn press_is_held_until_release_and_a_drag_gives_no_click() {
        let (mut ctl, mut cam) = setup();
        let btn = Some(MouseButton::Left);
        assert!(!ctl.is_pressed());
        ctl.handle(&mouse(PointerPhase::Down, 100.0, 100.0, btn), &mut cam);
        assert!(ctl.is_pressed() && !ctl.is_dragging());
        ctl.handle(&mouse(PointerPhase::Move, 140.0, 100.0, None), &mut cam);
        assert!(ctl.is_pressed() && ctl.is_dragging());
        ctl.handle(&mouse(PointerPhase::Up, 140.0, 100.0, btn), &mut cam);
        assert!(!ctl.is_pressed());
        assert_eq!(ctl.take_click(), None, "a drag release is no click");
    }

    #[test]
    fn drag_starts_only_past_four_pixels_on_one_axis() {
        let (mut ctl, mut cam) = setup();
        let focus = cam.focus();
        let btn = Some(MouseButton::Left);
        ctl.handle(&mouse(PointerPhase::Down, 100.0, 100.0, btn), &mut cam);
        // Exactly 4 px on X and on Y: not past the limit (strict), even though the distance is 5.7.
        ctl.handle(&mouse(PointerPhase::Move, 104.0, 104.0, None), &mut cam);
        assert!(!ctl.is_dragging());
        assert_eq!(cam.focus(), focus);
        // 4.5 px on one axis starts it; this move drags by its own 0.5 px step only.
        ctl.handle(&mouse(PointerPhase::Move, 104.5, 104.0, None), &mut cam);
        assert!(ctl.is_dragging());
        let moved = cam.focus().distance(focus);
        assert!(moved > 0.0 && moved < 2.0, "{moved}");
    }

    #[test]
    fn left_drag_moves_the_ground_under_the_cursor() {
        let (mut ctl, mut cam) = setup();
        let btn = Some(MouseButton::Left);
        let start = pos(600.0, 400.0);
        let grabbed = ground_under(&ctl, &cam, start);
        ctl.handle(&mouse(PointerPhase::Down, start.x, start.y, btn), &mut cam);
        let end = pos(700.0, 330.0);
        for i in 1..=10 {
            let t = i as f64 / 10.0;
            let (x, y) = (
                start.x + (end.x - start.x) * t,
                start.y + (end.y - start.y) * t,
            );
            ctl.handle(&mouse(PointerPhase::Move, x, y, None), &mut cam);
        }
        assert!(ctl.is_dragging());
        assert!(ground_under(&ctl, &cam, end).distance(grabbed) < 0.5);
        ctl.handle(&mouse(PointerPhase::Up, end.x, end.y, btn), &mut cam);
        assert!(!ctl.is_dragging());
        assert_eq!(ctl.take_click(), None);
    }

    #[test]
    fn right_press_without_moving_is_a_right_click() {
        let (mut ctl, mut cam) = setup();
        let btn = Some(MouseButton::Right);
        ctl.handle(&mouse(PointerPhase::Down, 50.0, 60.0, btn), &mut cam);
        ctl.handle(&mouse(PointerPhase::Up, 52.0, 61.0, btn), &mut cam);
        assert_eq!(ctl.take_right_click(), Some(pos(52.0, 61.0)));
        assert_eq!(ctl.take_right_click(), None);
        assert_eq!(ctl.take_click(), None);
        ctl.handle(&mouse(PointerPhase::Down, 50.0, 60.0, btn), &mut cam);
        ctl.handle(&mouse(PointerPhase::Up, 150.0, 60.0, btn), &mut cam);
        assert_eq!(ctl.take_right_click(), None, "moved too far");
    }

    #[test]
    fn right_button_does_not_drag() {
        let (mut ctl, mut cam) = setup();
        let focus = cam.focus();
        let btn = Some(MouseButton::Right);
        assert!(!ctl.handle(&mouse(PointerPhase::Down, 0.0, 0.0, btn), &mut cam));
        ctl.handle(&mouse(PointerPhase::Move, 300.0, 300.0, None), &mut cam);
        assert_eq!(cam.focus(), focus);
    }

    /// Drag left by `step` px per 60 Hz frame for `frames` frames, button still held.
    fn drag_left(ctl: &mut MapCameraController, cam: &mut GameCamera, step: f64, frames: i32) {
        for i in 1..=frames {
            ctl.handle(
                &mouse(PointerPhase::Move, 600.0 - step * i as f64, 400.0, None),
                cam,
            );
            ctl.update(1.0 / 60.0, cam);
        }
    }

    #[test]
    fn fast_drag_release_glides_by_the_original_formula() {
        let (mut ctl, mut cam) = setup();
        let btn = Some(MouseButton::Left);
        ctl.handle(&mouse(PointerPhase::Down, 600.0, 400.0, btn), &mut cam);
        drag_left(&mut ctl, &mut cam, 20.0, 10);
        // The mean of the last two frames equals the per-frame shift of the pointer.
        let speed = {
            let a = cam
                .ground_point(ctl.ndc(pos(420.0, 400.0)), ctl.aspect())
                .unwrap();
            let b = cam
                .ground_point(ctl.ndc(pos(400.0, 400.0)), ctl.aspect())
                .unwrap();
            (b.x - a.x).abs() * 60.0 * p::INERTIA_RELEASE_SCALE
        };
        ctl.handle(&mouse(PointerPhase::Up, 400.0, 400.0, btn), &mut cam);
        assert!(!cam.is_gliding(), "the glide starts in the next update");
        let x = cam.focus().x;
        let dur = p::inertia_duration(speed, cam.height());
        ctl.update(1.0 / 60.0, &mut cam);
        assert!(cam.is_gliding());
        // First glide frame runs at factor 1: v * dt.
        assert!(
            (cam.focus().x - x - speed / 60.0).abs() < speed / 60.0 * 0.05,
            "{} vs {}",
            cam.focus().x - x,
            speed / 60.0
        );
        assert!(
            cam.focus().x > x,
            "dragged left, so the focus keeps moving east"
        );
        let mut frames = 1;
        while cam.is_gliding() && frames < 200 {
            ctl.update(1.0 / 60.0, &mut cam);
            frames += 1;
        }
        assert!(!cam.is_gliding());
        assert!(
            (frames as f32 / 60.0 - dur).abs() < 0.05,
            "{frames} frames, {dur} s"
        );
    }

    #[test]
    fn release_in_the_move_frame_or_the_next_one_glides() {
        let btn = Some(MouseButton::Left);
        for idle_frames in [0, 1] {
            let (mut ctl, mut cam) = setup();
            ctl.handle(&mouse(PointerPhase::Down, 600.0, 400.0, btn), &mut cam);
            drag_left(&mut ctl, &mut cam, 20.0, 5);
            // The last drag frame is over; the pointer is released after `idle_frames` idle frames.
            for _ in 0..idle_frames {
                ctl.update(1.0 / 60.0, &mut cam);
            }
            ctl.handle(&mouse(PointerPhase::Up, 500.0, 400.0, btn), &mut cam);
            ctl.update(1.0 / 60.0, &mut cam);
            // Bit 0x40 lives one frame: with an idle frame in between, it is gone.
            assert_eq!(
                cam.is_gliding(),
                idle_frames == 0,
                "{idle_frames} idle frames"
            );
        }
        // A move and the release in the same frame.
        let (mut ctl, mut cam) = setup();
        ctl.handle(&mouse(PointerPhase::Down, 600.0, 400.0, btn), &mut cam);
        drag_left(&mut ctl, &mut cam, 20.0, 5);
        ctl.update(1.0 / 60.0, &mut cam);
        ctl.handle(&mouse(PointerPhase::Move, 480.0, 400.0, None), &mut cam);
        ctl.handle(&mouse(PointerPhase::Up, 480.0, 400.0, btn), &mut cam);
        ctl.update(1.0 / 60.0, &mut cam);
        assert!(cam.is_gliding());
    }

    #[test]
    fn release_after_a_pause_does_not_glide() {
        let (mut ctl, mut cam) = setup();
        let btn = Some(MouseButton::Left);
        ctl.handle(&mouse(PointerPhase::Down, 600.0, 400.0, btn), &mut cam);
        drag_left(&mut ctl, &mut cam, 20.0, 10);
        // Held still for two frames: bit 0x40 is gone in the release frame and the frame before.
        ctl.update(1.0 / 60.0, &mut cam);
        ctl.update(1.0 / 60.0, &mut cam);
        ctl.handle(&mouse(PointerPhase::Up, 400.0, 400.0, btn), &mut cam);
        ctl.update(1.0 / 60.0, &mut cam);
        assert!(!cam.is_gliding());
        let f = cam.focus();
        ctl.update(1.0 / 60.0, &mut cam);
        assert_eq!(cam.focus(), f);
    }

    #[test]
    fn new_press_stops_the_glide() {
        let (mut ctl, mut cam) = setup();
        let btn = Some(MouseButton::Left);
        ctl.handle(&mouse(PointerPhase::Down, 600.0, 400.0, btn), &mut cam);
        drag_left(&mut ctl, &mut cam, 20.0, 10);
        ctl.handle(&mouse(PointerPhase::Up, 400.0, 400.0, btn), &mut cam);
        ctl.update(1.0 / 60.0, &mut cam);
        assert!(cam.is_gliding());
        ctl.handle(&mouse(PointerPhase::Down, 300.0, 300.0, btn), &mut cam);
        assert!(!cam.is_gliding());
    }

    #[test]
    fn arrows_scroll_at_height_per_second() {
        let (mut ctl, mut cam) = setup();
        let (h, start) = (cam.height(), cam.focus());
        ctl.handle(&key(Key::Named(NamedKey::ArrowRight), true), &mut cam);
        ctl.update(0.5, &mut cam);
        assert!((cam.focus().x - start.x - h * 0.5).abs() < 1e-2);
        ctl.handle(&key(Key::Named(NamedKey::ArrowRight), false), &mut cam);
        ctl.handle(&key(Key::Named(NamedKey::ArrowUp), true), &mut cam);
        ctl.update(0.5, &mut cam);
        assert!(cam.focus().y < start.y, "up is north, -Z");
        ctl.handle(&key(Key::Named(NamedKey::ArrowUp), false), &mut cam);
        let f = cam.focus();
        ctl.update(0.5, &mut cam);
        assert_eq!(cam.focus(), f);
    }

    #[test]
    fn edge_scroll_only_when_enabled() {
        let (mut ctl, mut cam) = setup();
        let start = cam.focus();
        ctl.handle(&mouse(PointerPhase::Move, 1199.0, 400.0, None), &mut cam);
        ctl.update(0.5, &mut cam);
        assert_eq!(cam.focus(), start, "windowed: off");
        ctl.set_edge_scroll(true);
        ctl.update(0.5, &mut cam);
        assert!(cam.focus().x > start.x);
        ctl.handle(&mouse(PointerPhase::Move, 600.0, 400.0, None), &mut cam);
        let f = cam.focus();
        ctl.update(0.5, &mut cam);
        assert_eq!(cam.focus(), f);
    }

    #[test]
    fn one_finger_drags_and_pinch_zooms() {
        let (mut ctl, mut cam) = setup();
        let start = cam.focus();
        ctl.handle(&touch(1, PointerPhase::Down, 300.0, 400.0), &mut cam);
        ctl.handle(&touch(1, PointerPhase::Move, 400.0, 400.0), &mut cam);
        assert!(cam.focus().x < start.x);
        ctl.handle(&touch(1, PointerPhase::Up, 400.0, 400.0), &mut cam);

        let level = cam.level();
        ctl.handle(&touch(1, PointerPhase::Down, 500.0, 400.0), &mut cam);
        ctl.handle(&touch(2, PointerPhase::Down, 700.0, 400.0), &mut cam);
        ctl.handle(&touch(1, PointerPhase::Move, 400.0, 400.0), &mut cam);
        ctl.handle(&touch(2, PointerPhase::Move, 800.0, 400.0), &mut cam);
        assert!(cam.level() < level, "fingers apart zoom in");
        let l2 = cam.level();
        ctl.handle(&touch(1, PointerPhase::Move, 550.0, 400.0), &mut cam);
        assert!(cam.level() > l2, "fingers together zoom out");
    }

    #[test]
    fn focus_loss_cancels_drag_and_keys() {
        let (mut ctl, mut cam) = setup();
        let btn = Some(MouseButton::Left);
        ctl.handle(&mouse(PointerPhase::Down, 0.0, 0.0, btn), &mut cam);
        ctl.handle(&key(Key::Named(NamedKey::ArrowLeft), true), &mut cam);
        ctl.handle(&InputEvent::Focused(false), &mut cam);
        let f = cam.focus();
        ctl.handle(&mouse(PointerPhase::Move, 200.0, 0.0, None), &mut cam);
        ctl.update(1.0, &mut cam);
        assert_eq!(cam.focus(), f);
    }
}
