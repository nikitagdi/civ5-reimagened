//! A touch secondary click: a finger held still on the screen for `HOLD_MS` presses the right button, and lifting it
//! releases the button (the click happens on the lift).
//!
//! The original is a mouse game: the right button gives the unit its move and attack orders (holding it over a target
//! shows the path, letting go issues the order), closes the interface modes and opens the help of a control. A touch
//! screen has no second button, and the other touch gestures are taken: a tap is the left click, one finger dragging
//! pans the map, two fingers pinch (`map_camera.rs`). A long press is free (it is also what the phone systems' own
//! context menus and most mobile strategy games use), so holding one finger down without moving it holds the right
//! button, like holding it on a PC.
//!
//! The press already went to the layers as a left button down. When the hold ripens the touch is cancelled off-screen
//! (the layers see a release that clicks no control, as for a drag, see `ui_event_after_drag`), then the mouse
//! moves to the finger and presses the right button. While the finger stays down its moves are mouse moves (the path
//! preview follows it) and nothing else of that touch reaches the layers: no pan, no left click. The lift moves the
//! mouse to the release spot and releases the right button: that is the click. A cancelled touch, a second finger or
//! a lost focus also release the button (at the last spot), so it never sticks. A second finger, or a move beyond
//! `SLOP_CSS_PX`, before the hold ripens ends the wait.
//!
//! SIMPLIFIED(SIMP-exe-1080): the touch right button is the long press; the original has no touch input. The hold time
//! and the slop are set by feel (the usual 500 ms), not measured on a phone.

use platform::{
    InputEvent, MouseButton, PhysicalPosition, PointerEvent, PointerId, PointerKind, PointerPhase,
};

/// How long a finger must stay down for the right button (the usual long-press time of the phone systems).
pub(crate) const HOLD_MS: u64 = 500;
/// How far the finger may wander during the hold, in CSS pixels (a finger is never perfectly still).
pub(crate) const SLOP_CSS_PX: f64 = 10.0;

#[derive(Debug, Clone, Copy)]
struct Hold {
    id: PointerId,
    since_ms: u64,
    /// Where the finger is now (the right button goes down there).
    at: PhysicalPosition,
    origin: PhysicalPosition,
}

#[derive(Debug, Default)]
pub(crate) struct LongPress {
    /// The fingers on the screen.
    down: Vec<PointerId>,
    /// The single finger that may still become the right button.
    hold: Option<Hold>,
    /// The finger that did: the rest of its contact is the right button's, not the layers'.
    swallow: Option<PointerId>,
    /// Where the right button is held (the finger's last spot); `None` once released.
    right: Option<PhysicalPosition>,
    /// Physical pixels per CSS pixel.
    scale: f64,
}

fn mouse(phase: PointerPhase, at: PhysicalPosition, button: Option<MouseButton>) -> InputEvent {
    InputEvent::Pointer(PointerEvent {
        id: PointerId::MOUSE,
        kind: PointerKind::Mouse,
        phase,
        position: at,
        button,
        modifiers: Default::default(),
    })
}

impl LongPress {
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

    /// Lets go of the right button at its last spot (nothing when it is not held).
    fn release(&mut self, out: &mut Vec<InputEvent>) {
        if let Some(at) = self.right.take() {
            out.push(mouse(PointerPhase::Move, at, None));
            out.push(mouse(PointerPhase::Up, at, Some(MouseButton::Right)));
        }
    }

    /// What goes on to the app for `ev` at `now_ms`: the event itself, nothing, or the mouse events of a touch that
    /// became the right button.
    pub(crate) fn filter(&mut self, ev: InputEvent, now_ms: u64) -> Vec<InputEvent> {
        let mut out = Vec::new();
        match &ev {
            InputEvent::Focused(false) => {
                self.release(&mut out);
                self.down.clear();
                self.hold = None;
                self.swallow = None;
            }
            InputEvent::Pointer(p) if p.kind == PointerKind::Touch => {
                if self.swallow == Some(p.id) {
                    match p.phase {
                        PointerPhase::Move => {
                            if self.right.is_some() {
                                self.right = Some(p.position);
                                out.push(mouse(PointerPhase::Move, p.position, None));
                            }
                        }
                        PointerPhase::Up => {
                            if self.right.is_some() {
                                self.right = Some(p.position);
                            }
                            self.release(&mut out);
                            self.swallow = None;
                            self.down.retain(|d| *d != p.id);
                        }
                        PointerPhase::Cancel | PointerPhase::Left => {
                            self.release(&mut out);
                            self.swallow = None;
                            self.down.retain(|d| *d != p.id);
                        }
                        PointerPhase::Down => {}
                    }
                    return out;
                }
                match p.phase {
                    PointerPhase::Down => {
                        // A second finger while the right button is held: the button is let go, the first finger's
                        // contact stays swallowed until it lifts.
                        self.release(&mut out);
                        if !self.down.contains(&p.id) {
                            self.down.push(p.id);
                        }
                        self.hold = (self.down.len() == 1).then_some(Hold {
                            id: p.id,
                            since_ms: now_ms,
                            at: p.position,
                            origin: p.position,
                        });
                    }
                    PointerPhase::Move => {
                        let slop = SLOP_CSS_PX * self.scale;
                        if let Some(h) = self.hold.as_mut()
                            && h.id == p.id
                        {
                            h.at = p.position;
                            let (dx, dy) = (p.position.x - h.origin.x, p.position.y - h.origin.y);
                            if dx.hypot(dy) > slop {
                                self.hold = None;
                            }
                        }
                    }
                    PointerPhase::Up | PointerPhase::Cancel | PointerPhase::Left => {
                        self.down.retain(|d| *d != p.id);
                        if self.hold.is_some_and(|h| h.id == p.id) {
                            self.hold = None;
                        }
                    }
                }
            }
            _ => {}
        }
        out.push(ev);
        out
    }

    /// The events of a ripe hold (call every frame): the right button goes down.
    pub(crate) fn poll(&mut self, now_ms: u64) -> Vec<InputEvent> {
        let Some(h) = self.hold else {
            return Vec::new();
        };
        if now_ms.saturating_sub(h.since_ms) < HOLD_MS {
            return Vec::new();
        }
        self.hold = None;
        self.swallow = Some(h.id);
        self.right = Some(h.at);
        let off = PhysicalPosition { x: -1.0, y: -1.0 };
        vec![
            InputEvent::Pointer(PointerEvent {
                id: h.id,
                kind: PointerKind::Touch,
                phase: PointerPhase::Cancel,
                position: off,
                button: None,
                modifiers: Default::default(),
            }),
            mouse(PointerPhase::Move, h.at, None),
            mouse(PointerPhase::Down, h.at, Some(MouseButton::Right)),
        ]
    }

    #[cfg(test)]
    fn pending(&self) -> bool {
        self.hold.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn lp() -> LongPress {
        let mut l = LongPress::new();
        l.set_scale(2.0);
        l
    }

    /// (kind, phase, button, x, y) of each pointer event, to compare whole sequences.
    fn shape(
        evs: &[InputEvent],
    ) -> Vec<(PointerKind, PointerPhase, Option<MouseButton>, f64, f64)> {
        evs.iter()
            .map(|e| {
                let InputEvent::Pointer(p) = e else {
                    panic!("not a pointer event: {e:?}");
                };
                (p.kind, p.phase, p.button, p.position.x, p.position.y)
            })
            .collect()
    }

    const M: PointerKind = PointerKind::Mouse;
    const R: Option<MouseButton> = Some(MouseButton::Right);

    /// A held finger at (300, 200) after 500 ms: the right button is down.
    fn held() -> LongPress {
        let mut l = lp();
        l.filter(touch(1, PointerPhase::Down, 300.0, 200.0), 1_000);
        let evs = l.poll(1_000 + HOLD_MS);
        assert_eq!(evs.len(), 3, "{evs:?}");
        l
    }

    #[test]
    fn the_right_button_goes_down_when_the_finger_has_been_held_500_ms() {
        let mut l = lp();
        let down = touch(1, PointerPhase::Down, 300.0, 200.0);
        assert_eq!(l.filter(down.clone(), 1_000), vec![down]);
        assert!(l.poll(1_000 + HOLD_MS - 1).is_empty(), "not ripe yet");
        // A little jitter (under 10 css px = 20 physical) does not end the wait and moves the press point.
        assert_eq!(
            l.filter(touch(1, PointerPhase::Move, 310.0, 205.0), 1_200)
                .len(),
            1
        );
        let evs = l.poll(1_000 + HOLD_MS);
        // The touch is cancelled off-screen (no left click), then the mouse moves and presses: and no release yet.
        assert_eq!(
            shape(&evs),
            vec![
                (PointerKind::Touch, PointerPhase::Cancel, None, -1.0, -1.0),
                (M, PointerPhase::Move, None, 310.0, 205.0),
                (M, PointerPhase::Down, R, 310.0, 205.0),
            ]
        );
        // Once.
        assert!(l.poll(5_000).is_empty());
    }

    #[test]
    fn moves_while_the_right_button_is_held_are_mouse_moves_and_nothing_else() {
        let mut l = held();
        let evs = l.filter(touch(1, PointerPhase::Move, 400.0, 420.0), 1_700);
        assert_eq!(
            shape(&evs),
            vec![(M, PointerPhase::Move, None, 400.0, 420.0)]
        );
        let evs = l.filter(touch(1, PointerPhase::Move, 410.0, 430.0), 1_800);
        assert_eq!(
            shape(&evs),
            vec![(M, PointerPhase::Move, None, 410.0, 430.0)]
        );
    }

    #[test]
    fn lifting_the_finger_releases_the_right_button_where_it_lifted() {
        let mut l = held();
        l.filter(touch(1, PointerPhase::Move, 400.0, 400.0), 1_700);
        let evs = l.filter(touch(1, PointerPhase::Up, 420.0, 410.0), 1_800);
        // No left click, no pan: only the mouse moves to the release spot and lets go.
        assert_eq!(
            shape(&evs),
            vec![
                (M, PointerPhase::Move, None, 420.0, 410.0),
                (M, PointerPhase::Up, R, 420.0, 410.0),
            ]
        );
        // The next tap is a plain one again.
        let tap = touch(1, PointerPhase::Down, 5.0, 5.0);
        assert_eq!(l.filter(tap.clone(), 2_000), vec![tap]);
        assert!(l.pending());
    }

    #[test]
    fn a_short_tap_is_still_a_left_click() {
        let mut l = lp();
        let down = touch(1, PointerPhase::Down, 10.0, 10.0);
        let up = touch(1, PointerPhase::Up, 10.0, 10.0);
        assert_eq!(l.filter(down.clone(), 0), vec![down]);
        assert_eq!(l.filter(up.clone(), 200), vec![up]);
        assert!(l.poll(10_000).is_empty(), "no right button for a tap");
    }

    #[test]
    fn a_drag_before_500_ms_stays_a_pan_and_a_pinch_is_no_right_button() {
        // A drag: moved past the slop, passes through untouched.
        let mut l = lp();
        l.filter(touch(1, PointerPhase::Down, 10.0, 10.0), 0);
        let mv = touch(1, PointerPhase::Move, 10.0, 60.0);
        assert_eq!(l.filter(mv.clone(), 100), vec![mv]);
        assert!(l.poll(10_000).is_empty());
        // A pinch: a second finger.
        let mut l = lp();
        l.filter(touch(1, PointerPhase::Down, 10.0, 10.0), 0);
        l.filter(touch(2, PointerPhase::Down, 90.0, 10.0), 100);
        assert!(l.poll(10_000).is_empty());
        // The first finger lifting leaves the second one no right button either.
        l.filter(touch(1, PointerPhase::Up, 10.0, 10.0), 200);
        assert!(l.poll(10_000).is_empty());
    }

    #[test]
    fn a_cancelled_touch_releases_the_right_button_so_it_never_sticks() {
        let mut l = held();
        l.filter(touch(1, PointerPhase::Move, 350.0, 250.0), 1_700);
        let evs = l.filter(touch(1, PointerPhase::Cancel, -1.0, -1.0), 1_800);
        assert_eq!(
            shape(&evs),
            vec![
                (M, PointerPhase::Move, None, 350.0, 250.0),
                (M, PointerPhase::Up, R, 350.0, 250.0),
            ]
        );
        // The button is not released twice.
        assert!(l.poll(10_000).is_empty());
    }

    #[test]
    fn a_second_finger_releases_the_right_button_and_the_first_stays_quiet() {
        let mut l = held();
        let evs = l.filter(touch(2, PointerPhase::Down, 90.0, 10.0), 1_700);
        let s = shape(&evs);
        assert_eq!(s[0], (M, PointerPhase::Move, None, 300.0, 200.0));
        assert_eq!(s[1], (M, PointerPhase::Up, R, 300.0, 200.0));
        // No second release when the first finger lifts, and its moves are dropped.
        assert!(
            l.filter(touch(1, PointerPhase::Move, 5.0, 5.0), 1_800)
                .is_empty()
        );
        assert!(
            l.filter(touch(1, PointerPhase::Up, 5.0, 5.0), 1_900)
                .is_empty()
        );
    }

    #[test]
    fn losing_the_focus_releases_the_right_button_and_drops_a_pending_hold() {
        let mut l = held();
        let evs = l.filter(InputEvent::Focused(false), 1_700);
        assert_eq!(shape(&evs[..2]).len(), 2);
        assert_eq!(evs.len(), 3, "the release and the event itself: {evs:?}");
        // A pending hold is dropped too.
        let mut l = lp();
        l.filter(touch(1, PointerPhase::Down, 10.0, 10.0), 0);
        l.filter(InputEvent::Focused(false), 10);
        assert!(l.poll(10_000).is_empty());
    }

    #[test]
    fn the_mouse_passes_untouched() {
        let mut l = lp();
        let m = mouse(
            PointerPhase::Down,
            PhysicalPosition { x: 1.0, y: 1.0 },
            Some(MouseButton::Left),
        );
        assert_eq!(l.filter(m.clone(), 0), vec![m]);
        assert!(l.poll(10_000).is_empty());
    }
}
