//! Camera control from `platform::InputEvent`: drag pans, wheel zooms, two-finger pinch zooms.

use std::collections::BTreeMap;

use platform::{
    InputEvent, MouseButton, PhysicalPosition, PointerEvent, PointerId, PointerKind, PointerPhase,
    WheelDelta,
};
use render::Camera;

/// Radians per pixel.
const ORBIT_SPEED: f32 = 0.006;
/// Distance factor per wheel line.
const WHEEL_STEP: f32 = 0.9;
/// Trackpad pixels per wheel line.
const PIXELS_PER_LINE: f64 = 50.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DragMode {
    Pan,
    Orbit,
}

#[derive(Debug, Clone)]
pub struct CameraController {
    viewport_width: f32,
    viewport_height: f32,
    mouse_drag: Option<(DragMode, PhysicalPosition)>,
    touches: BTreeMap<PointerId, PhysicalPosition>,
}

impl CameraController {
    /// For a `width × height` physical pixel viewport.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            viewport_width: width.max(1) as f32,
            viewport_height: height.max(1) as f32,
            mouse_drag: None,
            touches: BTreeMap::new(),
        }
    }

    /// `true` if the event changed camera or gesture state.
    pub fn handle(&mut self, event: &InputEvent, camera: &mut Camera) -> bool {
        match event {
            InputEvent::Resized(size) => {
                self.viewport_width = size.width.max(1) as f32;
                self.viewport_height = size.height.max(1) as f32;
                false
            }
            InputEvent::Focused(false) => {
                self.mouse_drag = None;
                self.touches.clear();
                false
            }
            InputEvent::Wheel(w) => {
                let lines = match w.delta {
                    WheelDelta::Lines { y, .. } => y,
                    WheelDelta::Pixels { y, .. } => (y / PIXELS_PER_LINE) as f32,
                };
                // Scroll up zooms in.
                camera.zoom(WHEEL_STEP.powf(lines));
                true
            }
            InputEvent::Pointer(p) => match p.kind {
                PointerKind::Mouse | PointerKind::Pen => self.mouse(p, camera),
                PointerKind::Touch => self.touch(p, camera),
            },
            _ => false,
        }
    }

    fn mouse(&mut self, p: &PointerEvent, camera: &mut Camera) -> bool {
        match p.phase {
            PointerPhase::Down => {
                let mode = match p.button {
                    Some(MouseButton::Left) => DragMode::Pan,
                    Some(MouseButton::Right | MouseButton::Middle) => DragMode::Orbit,
                    _ => return false,
                };
                self.mouse_drag = Some((mode, p.position));
                true
            }
            PointerPhase::Move => {
                let Some((mode, last)) = self.mouse_drag else {
                    return false;
                };
                let dx = (p.position.x - last.x) as f32;
                let dy = (p.position.y - last.y) as f32;
                match mode {
                    DragMode::Pan => camera.pan_pixels(dx, dy, self.viewport_height),
                    DragMode::Orbit => camera.orbit(-dx * ORBIT_SPEED, dy * ORBIT_SPEED),
                }
                self.mouse_drag = Some((mode, p.position));
                true
            }
            PointerPhase::Up | PointerPhase::Cancel | PointerPhase::Left => {
                self.mouse_drag.take().is_some()
            }
        }
    }

    fn touch(&mut self, p: &PointerEvent, camera: &mut Camera) -> bool {
        match p.phase {
            PointerPhase::Down => {
                self.touches.insert(p.id, p.position);
                true
            }
            PointerPhase::Move => {
                if !self.touches.contains_key(&p.id) {
                    return false;
                }
                let before = self.gesture();
                self.touches.insert(p.id, p.position);
                let after = self.gesture();
                match (before, after) {
                    (Gesture::One(a), Gesture::One(b)) => {
                        camera.pan_pixels(
                            (b.x - a.x) as f32,
                            (b.y - a.y) as f32,
                            self.viewport_height,
                        );
                    }
                    (Gesture::Two { mid: m0, dist: d0 }, Gesture::Two { mid: m1, dist: d1 }) => {
                        camera.pan_pixels(
                            (m1.x - m0.x) as f32,
                            (m1.y - m0.y) as f32,
                            self.viewport_height,
                        );
                        if d0 > 1.0 && d1 > 1.0 {
                            // Spread fingers zoom in.
                            self.zoom_at(camera, (d0 / d1) as f32, m1);
                        }
                    }
                    _ => {}
                }
                true
            }
            PointerPhase::Up | PointerPhase::Cancel | PointerPhase::Left => {
                self.touches.remove(&p.id).is_some()
            }
        }
    }

    /// Anchored at `at`, so the map point under it stays fixed (else a pinch drifts).
    fn zoom_at(&self, camera: &mut Camera, factor: f32, at: PhysicalPosition) {
        let before = camera.distance;
        let after = (before * factor).clamp(Camera::MIN_DISTANCE, Camera::MAX_DISTANCE);
        let shrink = 1.0 - after / before;
        let ox = at.x as f32 - self.viewport_width * 0.5;
        let oy = at.y as f32 - self.viewport_height * 0.5;
        camera.pan_pixels(-ox * shrink, -oy * shrink, self.viewport_height);
        camera.zoom(factor);
    }

    /// Gesture from the first two touches, in id order.
    fn gesture(&self) -> Gesture {
        let mut it = self.touches.values();
        match (it.next(), it.next()) {
            (Some(&a), None) => Gesture::One(a),
            (Some(&a), Some(&b)) => Gesture::Two {
                mid: PhysicalPosition {
                    x: (a.x + b.x) * 0.5,
                    y: (a.y + b.y) * 0.5,
                },
                dist: ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt(),
            },
            _ => Gesture::None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Gesture {
    None,
    One(PhysicalPosition),
    Two { mid: PhysicalPosition, dist: f64 },
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform::{Modifiers, WheelEvent};

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

    fn wheel(y: f32) -> InputEvent {
        InputEvent::Wheel(WheelEvent {
            delta: WheelDelta::Lines { x: 0.0, y },
            position: pos(0.0, 0.0),
            modifiers: Modifiers::default(),
        })
    }

    #[test]
    fn left_drag_pans() {
        let mut c = Camera::default();
        let mut ctl = CameraController::new(1200, 800);
        ctl.handle(
            &mouse(PointerPhase::Down, 100.0, 100.0, Some(MouseButton::Left)),
            &mut c,
        );
        ctl.handle(&mouse(PointerPhase::Move, 200.0, 100.0, None), &mut c);
        assert!(
            c.target.x < 0.0,
            "map dragged right means target moves left"
        );
        let yaw = c.yaw;
        ctl.handle(
            &mouse(PointerPhase::Up, 200.0, 100.0, Some(MouseButton::Left)),
            &mut c,
        );
        let t = c.target;
        ctl.handle(&mouse(PointerPhase::Move, 300.0, 300.0, None), &mut c);
        assert_eq!(c.target, t, "without a press, movement does nothing");
        assert_eq!(c.yaw, yaw);
    }

    #[test]
    fn right_drag_orbits() {
        let mut c = Camera::default();
        let (yaw, pitch) = (c.yaw, c.pitch);
        let mut ctl = CameraController::new(1200, 800);
        ctl.handle(
            &mouse(PointerPhase::Down, 0.0, 0.0, Some(MouseButton::Right)),
            &mut c,
        );
        ctl.handle(&mouse(PointerPhase::Move, 50.0, 20.0, None), &mut c);
        assert_ne!(c.yaw, yaw);
        assert!(c.pitch > pitch);
        assert_eq!(c.target, Camera::default().target);
    }

    #[test]
    fn wheel_up_zooms_in() {
        let mut c = Camera::default();
        let d = c.distance;
        let mut ctl = CameraController::new(1200, 800);
        ctl.handle(&wheel(1.0), &mut c);
        assert!(c.distance < d);
        ctl.handle(&wheel(-2.0), &mut c);
        assert!(c.distance > d);
    }

    #[test]
    fn one_finger_pans() {
        let mut c = Camera::default();
        let mut ctl = CameraController::new(1200, 800);
        ctl.handle(&touch(1, PointerPhase::Down, 100.0, 100.0), &mut c);
        ctl.handle(&touch(1, PointerPhase::Move, 100.0, 200.0), &mut c);
        assert!(c.target.z < 0.0);
        assert_eq!(c.distance, Camera::default().distance);
    }

    #[test]
    fn pinch_out_zooms_in_and_pinch_in_zooms_out() {
        let mut c = Camera::default();
        let d = c.distance;
        let mut ctl = CameraController::new(1200, 800);
        ctl.handle(&touch(1, PointerPhase::Down, 300.0, 400.0), &mut c);
        ctl.handle(&touch(2, PointerPhase::Down, 500.0, 400.0), &mut c);
        ctl.handle(&touch(1, PointerPhase::Move, 200.0, 400.0), &mut c);
        ctl.handle(&touch(2, PointerPhase::Move, 600.0, 400.0), &mut c);
        assert!(c.distance < d);
        let d2 = c.distance;
        ctl.handle(&touch(1, PointerPhase::Move, 350.0, 400.0), &mut c);
        assert!(c.distance > d2);
    }

    #[test]
    fn symmetric_pinch_off_center_keeps_anchor() {
        let mut c = Camera::default();
        let mut ctl = CameraController::new(1200, 800);
        ctl.handle(&touch(1, PointerPhase::Down, 850.0, 600.0), &mut c);
        ctl.handle(&touch(2, PointerPhase::Down, 950.0, 600.0), &mut c);
        let k0 = c.world_per_pixel(800.0);
        let anchor0 = c.target + c.ground_right() * (300.0 * k0);
        for i in 1..=10 {
            let s = i as f64 * 10.0;
            ctl.handle(&touch(1, PointerPhase::Move, 850.0 - s, 600.0), &mut c);
            ctl.handle(&touch(2, PointerPhase::Move, 950.0 + s, 600.0), &mut c);
        }
        assert!(c.distance < Camera::default().distance);
        let k1 = c.world_per_pixel(800.0);
        let anchor1 = c.target + c.ground_right() * (300.0 * k1);
        assert!(
            (anchor1.x - anchor0.x).abs() < 0.05,
            "{anchor0} -> {anchor1}"
        );
    }

    #[test]
    fn lifting_finger_switches_back_to_pan() {
        let mut c = Camera::default();
        let mut ctl = CameraController::new(1200, 800);
        ctl.handle(&touch(1, PointerPhase::Down, 0.0, 0.0), &mut c);
        ctl.handle(&touch(2, PointerPhase::Down, 100.0, 0.0), &mut c);
        ctl.handle(&touch(2, PointerPhase::Up, 100.0, 0.0), &mut c);
        let d = c.distance;
        ctl.handle(&touch(1, PointerPhase::Move, 50.0, 0.0), &mut c);
        assert_eq!(c.distance, d);
        assert!(c.target.x < 0.0);
        assert!(!ctl.handle(&touch(9, PointerPhase::Move, 1.0, 1.0), &mut c));
    }

    #[test]
    fn focus_loss_cancels_drag() {
        let mut c = Camera::default();
        let mut ctl = CameraController::new(1200, 800);
        ctl.handle(
            &mouse(PointerPhase::Down, 0.0, 0.0, Some(MouseButton::Left)),
            &mut c,
        );
        ctl.handle(&InputEvent::Focused(false), &mut c);
        ctl.handle(&mouse(PointerPhase::Move, 100.0, 0.0, None), &mut c);
        assert_eq!(c.target, Camera::default().target);
    }
}
