//! Shared input types. Coordinates are physical window pixels, Y down.

#[derive(Debug, Clone, PartialEq)]
pub enum InputEvent {
    Pointer(PointerEvent),
    Wheel(WheelEvent),
    Key(KeyEvent),
    Text(String),
    Resized(PhysicalSize),
    ScaleFactor(f64),
    Focused(bool),
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PhysicalPosition {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PhysicalSize {
    pub width: u32,
    pub height: u32,
}

/// The mouse is always `PointerId::MOUSE`; a touch keeps its id for the whole contact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PointerId(pub u64);

impl PointerId {
    pub const MOUSE: PointerId = PointerId(0);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PointerKind {
    Mouse,
    Touch,
    Pen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PointerPhase {
    Down,
    Move,
    Up,
    /// Cancelled by the system; treat as `Up` with no action.
    Cancel,
    Left,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    Other(u16),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PointerEvent {
    pub id: PointerId,
    pub kind: PointerKind,
    pub phase: PointerPhase,
    pub position: PhysicalPosition,
    /// Only for mouse `Down`/`Up`.
    pub button: Option<MouseButton>,
    pub modifiers: Modifiers,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WheelDelta {
    /// Mouse wheel. Positive `y` scrolls up.
    Lines {
        x: f32,
        y: f32,
    },
    Pixels {
        x: f64,
        y: f64,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct WheelEvent {
    pub delta: WheelDelta,
    pub position: PhysicalPosition,
    /// Ctrl + touchpad scroll is a pinch.
    pub modifiers: Modifiers,
}

#[derive(Debug, Clone, PartialEq)]
pub struct KeyEvent {
    pub key: Key,
    /// DOM `KeyboardEvent.code` name; layout-independent. `None` if unreported.
    pub code: Option<String>,
    pub pressed: bool,
    pub repeat: bool,
    pub modifiers: Modifiers,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Key {
    Named(NamedKey),
    Character(String),
    Unidentified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum NamedKey {
    Enter,
    Escape,
    Tab,
    Backspace,
    Delete,
    Insert,
    Space,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Home,
    End,
    PageUp,
    PageDown,
    Shift,
    Control,
    Alt,
    Meta,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
}
