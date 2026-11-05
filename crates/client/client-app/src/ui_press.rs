//! A left press the layers took: did it move past the drag threshold before the release?
//!
//! A press that starts on a city banner or on the world's own button-down handler goes to the layers and never
//! reaches the map camera, so `MapCameraController` does not know it became a drag. The exe drops the click of any
//! press that moved (`ProcessCameraDrag`, UIManager.cpp:48-62); this follows every left button or finger press the
//! same way, so the release that ends it can be kept from clicking a banner or a hex.

use std::collections::BTreeMap;

use platform::{InputEvent, MouseButton, PhysicalPosition, PointerEvent, PointerId, PointerPhase};
use render::camera_params as p;

#[derive(Debug, Default)]
pub(crate) struct UiPress {
    /// The fingers and the left button down: where, and whether the press moved past the threshold.
    down: BTreeMap<PointerId, (PhysicalPosition, bool)>,
}

fn left(p: &PointerEvent) -> bool {
    p.button.is_none_or(|b| b == MouseButton::Left)
}

impl UiPress {
    /// Follows `ev`; call it before asking `moved`.
    pub(crate) fn observe(&mut self, ev: &InputEvent) {
        match ev {
            InputEvent::Pointer(e) if left(e) => match e.phase {
                PointerPhase::Down => {
                    self.down.insert(e.id, (e.position, false));
                }
                PointerPhase::Move => {
                    if let Some((at, moved)) = self.down.get_mut(&e.id) {
                        let limit = f64::from(p::DRAG_THRESHOLD_PX);
                        *moved |= (e.position.x - at.x).abs() > limit
                            || (e.position.y - at.y).abs() > limit;
                    }
                }
                PointerPhase::Up | PointerPhase::Cancel | PointerPhase::Left => {}
            },
            InputEvent::Focused(false) => self.down.clear(),
            _ => {}
        }
    }

    /// The press `ev` belongs to moved past the threshold. For a release, the press is over afterwards.
    pub(crate) fn moved(&mut self, ev: &InputEvent) -> bool {
        let InputEvent::Pointer(e) = ev else {
            return false;
        };
        if !left(e) {
            return false;
        }
        let ends = matches!(
            e.phase,
            PointerPhase::Up | PointerPhase::Cancel | PointerPhase::Left
        );
        let moved = self.down.get(&e.id).is_some_and(|d| d.1);
        if ends {
            self.down.remove(&e.id);
        }
        moved
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform::PointerKind;

    fn ev(kind: PointerKind, id: u64, phase: PointerPhase, x: f64, y: f64) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            id: PointerId(id),
            kind,
            phase,
            position: PhysicalPosition { x, y },
            button: (kind == PointerKind::Mouse).then_some(MouseButton::Left),
            modifiers: Default::default(),
        })
    }

    fn release_after(kind: PointerKind, to: (f64, f64)) -> bool {
        let mut t = UiPress::default();
        for e in [
            ev(kind, 1, PointerPhase::Down, 100.0, 100.0),
            ev(kind, 1, PointerPhase::Move, to.0, to.1),
        ] {
            t.observe(&e);
        }
        let up = ev(kind, 1, PointerPhase::Up, to.0, to.1);
        t.observe(&up);
        t.moved(&up)
    }

    #[test]
    fn a_mouse_press_that_moved_past_the_threshold_is_not_a_click() {
        assert!(release_after(PointerKind::Mouse, (160.0, 100.0)));
    }

    #[test]
    fn a_touch_press_that_moved_past_the_threshold_is_not_a_click() {
        assert!(release_after(PointerKind::Touch, (100.0, 140.0)));
    }

    #[test]
    fn a_press_that_stays_within_the_threshold_is_a_click() {
        assert!(!release_after(PointerKind::Mouse, (102.0, 101.0)));
    }

    #[test]
    fn a_press_that_moved_and_came_back_is_still_not_a_click() {
        let mut t = UiPress::default();
        for e in [
            ev(PointerKind::Mouse, 0, PointerPhase::Down, 100.0, 100.0),
            ev(PointerKind::Mouse, 0, PointerPhase::Move, 200.0, 100.0),
            ev(PointerKind::Mouse, 0, PointerPhase::Move, 100.0, 100.0),
        ] {
            t.observe(&e);
        }
        let up = ev(PointerKind::Mouse, 0, PointerPhase::Up, 100.0, 100.0);
        t.observe(&up);
        assert!(t.moved(&up));
    }

    #[test]
    fn the_next_press_starts_clean() {
        let mut t = UiPress::default();
        let seq = [
            ev(PointerKind::Mouse, 0, PointerPhase::Down, 0.0, 0.0),
            ev(PointerKind::Mouse, 0, PointerPhase::Move, 90.0, 0.0),
            ev(PointerKind::Mouse, 0, PointerPhase::Up, 90.0, 0.0),
        ];
        for e in &seq {
            t.observe(e);
        }
        assert!(t.moved(&seq[2]));
        let down = ev(PointerKind::Mouse, 0, PointerPhase::Down, 5.0, 5.0);
        t.observe(&down);
        let up = ev(PointerKind::Mouse, 0, PointerPhase::Up, 5.0, 5.0);
        t.observe(&up);
        assert!(!t.moved(&up));
    }
}
