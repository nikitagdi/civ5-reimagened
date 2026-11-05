//! The touch way to the performance panel (F3 on a keyboard): three fingers held still on the screen for `HOLD_MS`
//! toggle it. A phone has no F3, and the panel is how the operator reads FPS and GPU memory on one
//! ("Deploy order").
//!
//! The gesture only watches: the touches still go on to the layers (two or more fingers are a pinch there, which a
//! still hand does not move). It fires once per hold; all fingers must lift before it can fire again. A fourth finger,
//! a lifted one or a move beyond `SLOP_CSS_PX` ends the wait.
//!
//! SIMPLIFIED(SIMP-exe-1090): the panel's touch toggle is ours (operator's choice, three fingers for 3 s); the original
//! has neither touch input nor this panel.

use platform::{InputEvent, PhysicalPosition, PointerId, PointerKind, PointerPhase};

/// How long the three fingers must stay down.
pub(crate) const HOLD_MS: u64 = 3_000;
/// How far a finger may wander during the hold, in CSS pixels.
pub(crate) const SLOP_CSS_PX: f64 = 20.0;
const FINGERS: usize = 3;

#[derive(Debug, Default)]
pub(crate) struct HudHold {
    /// The fingers on the screen and where each one went down.
    down: Vec<(PointerId, PhysicalPosition)>,
    /// When the third finger went down, while the hold is still valid.
    since_ms: Option<u64>,
    /// The hold fired; nothing more until every finger is lifted.
    fired: bool,
    /// Physical pixels per CSS pixel.
    scale: f64,
}

impl HudHold {
    pub(crate) fn new() -> Self {
        Self {
            scale: 1.0,
            ..Self::default()
        }
    }

    /// The window's scale factor changed.
    pub(crate) fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.5);
    }

    /// Watches one input event at `now_ms`.
    pub(crate) fn observe(&mut self, ev: &InputEvent, now_ms: u64) {
        match ev {
            InputEvent::Focused(false) => {
                self.down.clear();
                self.since_ms = None;
                self.fired = false;
            }
            InputEvent::Pointer(p) if p.kind == PointerKind::Touch => match p.phase {
                PointerPhase::Down => {
                    if !self.down.iter().any(|(id, _)| *id == p.id) {
                        self.down.push((p.id, p.position));
                    }
                    self.since_ms = (self.down.len() == FINGERS && !self.fired).then_some(now_ms);
                }
                PointerPhase::Move => {
                    let slop = SLOP_CSS_PX * self.scale;
                    if let Some((_, o)) = self.down.iter().find(|(id, _)| *id == p.id)
                        && (p.position.x - o.x).hypot(p.position.y - o.y) > slop
                    {
                        self.since_ms = None;
                    }
                }
                PointerPhase::Up | PointerPhase::Cancel | PointerPhase::Left => {
                    self.down.retain(|(id, _)| *id != p.id);
                    self.since_ms = None;
                    if self.down.is_empty() {
                        self.fired = false;
                    }
                }
            },
            _ => {}
        }
    }

    /// True once when the hold ripens (call every frame).
    pub(crate) fn poll(&mut self, now_ms: u64) -> bool {
        match self.since_ms {
            Some(t) if now_ms.saturating_sub(t) >= HOLD_MS => {
                self.since_ms = None;
                self.fired = true;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform::PointerEvent;

    fn touch(id: u64, phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            id: PointerId(id),
            kind: PointerKind::Touch,
            phase,
            position: PhysicalPosition { x, y },
            button: None,
            modifiers: Default::default(),
        })
    }

    fn three_down(h: &mut HudHold, at: u64) {
        for id in 1..=3 {
            h.observe(&touch(id, PointerPhase::Down, id as f64 * 100.0, 300.0), at);
        }
    }

    #[test]
    fn three_fingers_held_3_s_toggle_the_panel_once() {
        let mut h = HudHold::new();
        three_down(&mut h, 1_000);
        assert!(!h.poll(1_000 + HOLD_MS - 1));
        // A little jitter is fine.
        h.observe(&touch(2, PointerPhase::Move, 210.0, 305.0), 2_000);
        assert!(h.poll(1_000 + HOLD_MS));
        assert!(!h.poll(10_000), "once per hold");
    }

    #[test]
    fn the_hand_must_lift_before_the_gesture_fires_again() {
        let mut h = HudHold::new();
        three_down(&mut h, 0);
        assert!(h.poll(HOLD_MS));
        // One finger lifts and comes back: still the same hold.
        h.observe(&touch(3, PointerPhase::Up, 300.0, 300.0), 4_000);
        h.observe(&touch(3, PointerPhase::Down, 300.0, 300.0), 4_100);
        assert!(!h.poll(20_000));
        for id in 1..=3 {
            h.observe(&touch(id, PointerPhase::Up, 0.0, 0.0), 21_000);
        }
        three_down(&mut h, 22_000);
        assert!(h.poll(22_000 + HOLD_MS));
    }

    #[test]
    fn a_moving_hand_two_fingers_or_four_fingers_do_not_fire() {
        let mut h = HudHold::new();
        three_down(&mut h, 0);
        h.observe(&touch(1, PointerPhase::Move, 100.0, 400.0), 500);
        assert!(!h.poll(10_000), "a pinch or pan moves the fingers");

        let mut h = HudHold::new();
        h.observe(&touch(1, PointerPhase::Down, 0.0, 0.0), 0);
        h.observe(&touch(2, PointerPhase::Down, 50.0, 0.0), 0);
        assert!(!h.poll(10_000), "two fingers");

        let mut h = HudHold::new();
        three_down(&mut h, 0);
        h.observe(&touch(4, PointerPhase::Down, 400.0, 300.0), 100);
        assert!(!h.poll(10_000), "four fingers");
    }

    #[test]
    fn the_mouse_and_a_lost_focus_do_not_fire() {
        let mut h = HudHold::new();
        three_down(&mut h, 0);
        h.observe(&InputEvent::Focused(false), 100);
        assert!(!h.poll(10_000));
    }
}
