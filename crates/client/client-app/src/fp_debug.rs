//! A debug first-person camera on a hook (client-hooks C2, T27): the way to see the camera override work without
//! a mod. Off unless the environment has `CIV5_FP_CAM` (`1`, or `x,z,yaw,pitch` with the angles in degrees to
//! start in the view at once); then F4 takes the screen's camera and gives it back.
//!
//! While it holds the camera: W A S D move (Shift is faster), the arrows (or J L) turn and R F tilt, Q E lower and raise
//! the eye. Keys are read by their physical code, so the layout does not matter. The mouse is the game's.

use std::collections::HashSet;

use glam::{Vec2, Vec3};
use platform::InputEvent;
use render::FirstPerson;

use crate::mod_hooks::{FrameInfo, HookCtl, ModHook};

/// The environment variable that registers the hook.
pub const ENV: &str = "CIV5_FP_CAM";

/// The height of the eye above the ground, in world units (a person: 6.5).
const EYE_HEIGHT: f32 = 6.5;

pub struct FirstPersonDebug {
    on: bool,
    /// Wait for the first frame to put the eye where the strategic camera looks.
    placed: bool,
    pos: Vec2,
    yaw: f32,
    pitch: f32,
    eye_h: f32,
    ground: f32,
    held: HashSet<String>,
}

impl FirstPersonDebug {
    /// The hook as the value of [`ENV`] asks for it, or `None` when the variable is not set.
    pub fn from_env() -> Option<Self> {
        let value = std::env::var(ENV).ok()?;
        Some(Self::from_value(&value))
    }

    /// `1` (or anything not four numbers): off until F4. `x,z,yaw,pitch`: on at that place and look.
    pub fn from_value(value: &str) -> Self {
        let nums: Vec<f32> = value
            .split(',')
            .filter_map(|n| n.trim().parse().ok())
            .collect();
        let mut hook = Self {
            on: false,
            placed: false,
            pos: Vec2::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            eye_h: EYE_HEIGHT,
            ground: 0.0,
            held: HashSet::new(),
        };
        if let [x, z, yaw, pitch] = nums[..] {
            hook.on = true;
            hook.placed = true;
            hook.pos = Vec2::new(x, z);
            hook.yaw = yaw.to_radians();
            hook.pitch = pitch.to_radians();
        }
        hook
    }

    fn held(&self, code: &str) -> bool {
        self.held.contains(code)
    }

    /// One step of `dt` seconds of the held keys.
    fn step(&mut self, dt: f32) {
        let turn = 1.5 * dt;
        if self.held("ArrowLeft") || self.held("KeyJ") {
            self.yaw -= turn;
        }
        if self.held("ArrowRight") || self.held("KeyL") {
            self.yaw += turn;
        }
        if self.held("ArrowUp") || self.held("KeyR") {
            self.pitch += turn;
        }
        if self.held("ArrowDown") || self.held("KeyF") {
            self.pitch -= turn;
        }
        self.pitch = self
            .pitch
            .clamp(-FirstPerson::MAX_PITCH, FirstPerson::MAX_PITCH);
        let speed = if self.held("ShiftLeft") || self.held("ShiftRight") {
            300.0
        } else {
            40.0
        } * dt;
        let forward = Vec2::new(self.yaw.sin(), -self.yaw.cos());
        let right = Vec2::new(self.yaw.cos(), self.yaw.sin());
        if self.held("KeyW") {
            self.pos += forward * speed;
        }
        if self.held("KeyS") {
            self.pos -= forward * speed;
        }
        if self.held("KeyD") {
            self.pos += right * speed;
        }
        if self.held("KeyA") {
            self.pos -= right * speed;
        }
        if self.held("KeyE") {
            self.eye_h += 20.0 * dt;
        }
        if self.held("KeyQ") {
            self.eye_h = (self.eye_h - 20.0 * dt).max(0.5);
        }
    }

    /// The camera of the state.
    fn camera(&self) -> FirstPerson {
        let mut cam = FirstPerson::new(Vec3::new(self.pos.x, self.ground + self.eye_h, self.pos.y));
        cam.yaw = self.yaw;
        cam.pitch = self.pitch;
        cam
    }
}

impl ModHook for FirstPersonDebug {
    fn claim_input(&mut self, event: &InputEvent, ctl: &mut HookCtl) -> bool {
        let InputEvent::Key(k) = event else {
            return false;
        };
        let code = k.code.as_deref().unwrap_or("");
        if code == "F4" {
            if k.pressed && !k.repeat {
                self.on = !self.on;
                self.held.clear();
                self.placed = false;
                if !self.on {
                    ctl.set_camera(None);
                }
            }
            return true;
        }
        // F3 (the performance panel) stays the game's; every other key is the camera's while it is on.
        if !self.on || code == "F3" {
            return false;
        }
        if k.pressed {
            self.held.insert(code.to_owned());
        } else {
            self.held.remove(code);
        }
        true
    }

    fn frame(&mut self, info: &FrameInfo<'_>, ctl: &mut HookCtl) {
        if !self.on {
            return;
        }
        if !self.placed {
            self.pos = info.focus;
            self.placed = true;
        }
        self.step(info.dt);
        if let Some(terrain) = info.terrain {
            self.ground = terrain.surface_height(self.pos);
        }
        ctl.set_camera(Some(Box::new(self.camera())));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform::{Key, KeyEvent, Modifiers, NamedKey};

    fn key(code: &str, pressed: bool) -> InputEvent {
        InputEvent::Key(KeyEvent {
            key: Key::Character("ц".into()),
            code: Some(code.into()),
            pressed,
            repeat: false,
            modifiers: Modifiers::default(),
        })
    }

    fn info(dt: f32) -> FrameInfo<'static> {
        FrameInfo {
            dt,
            now_ms: 0,
            focus: Vec2::new(40.0, -30.0),
            terrain: None,
        }
    }

    #[test]
    fn off_it_takes_nothing_and_asks_for_nothing() {
        let mut hooks = crate::mod_hooks::ModHooks::new();
        hooks.register(Box::new(FirstPersonDebug::from_value("1")));
        assert!(!hooks.claim_input(&key("KeyW", true)).taken);
        hooks.frame(&info(0.1));
        assert!(hooks.camera().is_none());
        let f3 = InputEvent::Key(KeyEvent {
            key: Key::Named(NamedKey::F3),
            code: Some("F3".into()),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::default(),
        });
        assert!(!hooks.claim_input(&f3).taken);
    }

    #[test]
    fn f4_takes_the_camera_at_the_strategic_focus_and_gives_it_back() {
        let mut hooks = crate::mod_hooks::ModHooks::new();
        hooks.register(Box::new(FirstPersonDebug::from_value("1")));
        assert!(hooks.claim_input(&key("F4", true)).taken);
        hooks.frame(&info(0.0));
        let cam = hooks.camera().expect("the camera is the hook's");
        assert_eq!(cam.terrain_focus(), Vec2::new(40.0, -30.0));
        assert_eq!(cam.eye().y, EYE_HEIGHT);
        // W (by its code, on any layout) moves along the view: yaw 0 looks along -Z.
        assert!(hooks.claim_input(&key("KeyW", true)).taken);
        hooks.frame(&info(0.5));
        let cam = hooks.camera().expect("held");
        assert!(
            (cam.eye().z - (-30.0 - 20.0)).abs() < 1e-4,
            "{:?}",
            cam.eye()
        );
        assert!((cam.eye().x - 40.0).abs() < 1e-4);
        assert!(hooks.claim_input(&key("KeyW", false)).taken);
        hooks.frame(&info(0.5));
        assert!((hooks.camera().unwrap().eye().z - (-50.0)).abs() < 1e-4);
        // F4 again: the strategic camera is back.
        assert!(hooks.claim_input(&key("F4", true)).taken);
        assert!(hooks.camera().is_none());
        assert!(!hooks.claim_input(&key("KeyW", true)).taken);
    }

    #[test]
    fn a_value_with_four_numbers_starts_in_the_view() {
        let mut hooks = crate::mod_hooks::ModHooks::new();
        hooks.register(Box::new(FirstPersonDebug::from_value("10, 20, 90, -10")));
        hooks.frame(&info(0.0));
        let cam = hooks.camera().expect("on from the start");
        assert_eq!(cam.terrain_focus(), Vec2::new(10.0, 20.0));
        // Yaw 90 degrees looks along +X.
        let looked = cam.view_proj(1.0) * Vec3::new(30.0, 6.5, 20.0).extend(1.0);
        assert!(looked.w > 0.0);
    }
}
