//! The parts of markup behaviour a package stores: events, actions, anchors and controls.
//!
//! These types are always available (the package keeps them and the runtime acts on them); the
//! grammars that produce them are in `attrs` and need feature `profile`. Postcard writes the
//! variant index: new variants go at the end.

use serde::{Deserialize, Serialize};

use super::expr::Expr;

macro_rules! names {
    ($(#[$doc:meta])* $name:ident { $($(#[$vdoc:meta])* $var:ident => $text:literal,)* }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub enum $name {
            $($(#[$vdoc])* $var,)*
        }

        impl $name {
            /// Every variant, in wire order.
            pub const ALL: &'static [$name] = &[$($name::$var,)*];

            /// The name as written in markup.
            pub const fn name(self) -> &'static str {
                match self {
                    $($name::$var => $text,)*
                }
            }

            /// The variant of a name.
            pub fn from_name(name: &str) -> Option<$name> {
                match name {
                    $($text => Some($name::$var),)*
                    _ => None,
                }
            }
        }
    };
}

names! {
    /// An event a node can handle with `data-on-<event>`.
    Event {
        /// `click`
        Click => "click",
        /// `dblclick`
        Dblclick => "dblclick",
        /// `contextmenu`
        Contextmenu => "contextmenu",
        /// `pointerenter`
        Pointerenter => "pointerenter",
        /// `pointerleave`
        Pointerleave => "pointerleave",
        /// `pointerdown`
        Pointerdown => "pointerdown",
        /// `pointerup`
        Pointerup => "pointerup",
        /// `input`
        Input => "input",
        /// `change`
        Change => "change",
        /// `submit`
        Submit => "submit",
        /// `focus`
        Focus => "focus",
        /// `blur`
        Blur => "blur",
        /// Wheel movement in CSS pixels.
        Wheel => "wheel",
        /// The screen's Escape policy.
        Escape => "escape",
    }
}

names! {
    /// What a `data-anchor` follows on the map.
    AnchorKind {
        /// A city.
        City => "city",
        /// A unit.
        Unit => "unit",
        /// A map plot.
        Plot => "plot",
        /// A map plot, its chunk drawn at the size of the world there (the original's `WorldAnchor Scaling="True"`:
        /// the yield icons).
        PlotScaled => "plot-scaled",
        /// The point 30 west and 15 north of a plot's centre, about its upper left vertex (where the original's
        /// `ResourceIconManager` hangs a resource icon).
        PlotCorner => "plot-corner",
    }
}

impl AnchorKind {
    /// An anchor on a plot (any key or integer names it).
    pub const fn is_plot(self) -> bool {
        matches!(
            self,
            AnchorKind::Plot | AnchorKind::PlotScaled | AnchorKind::PlotCorner
        )
    }
}

names! {
    /// The behaviour of a `data-control` node.
    ControlKind {
        /// An on/off box.
        Checkbox => "checkbox",
        /// An on/off switch.
        Toggle => "toggle",
        /// One of several values.
        Radio => "radio",
        /// A number in a range.
        Slider => "slider",
        /// One of the items of a list.
        Select => "select",
    }
}

/// An argument of an `intent` or `call`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Arg {
    /// The argument's name; `None` for a positional argument of a mod intent or a `call`.
    pub name: Option<String>,
    /// The value.
    pub value: Expr,
}

/// One step of `data-on-<event>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Action {
    /// `intent name(args)`: a command from the schema's catalogue, or `mods.<id>.<name>`.
    Intent {
        /// The dotted intent name.
        name: String,
        /// The arguments; named once the converter has checked them against the catalogue.
        args: Vec<Arg>,
    },
    /// `set state.path = expr`
    Set {
        /// The written path (`state.x` or a writable `ui.x`).
        path: String,
        /// The new value.
        value: Expr,
    },
    /// `toggle state.path`
    Toggle {
        /// The flipped path.
        path: String,
    },
    /// `call name(args)`: an event for the skin's script.
    Call {
        /// The handler name.
        name: String,
        /// The arguments.
        args: Vec<Arg>,
    },
    /// `stop`: the event does not bubble.
    Stop,
}

/// A `KeyboardEvent.code` value.
pub fn is_key_code(c: &str) -> bool {
    const NAMED: &[&str] = &[
        "Space",
        "Enter",
        "Escape",
        "Tab",
        "Backspace",
        "Delete",
        "Insert",
        "Home",
        "End",
        "PageUp",
        "PageDown",
        "ArrowUp",
        "ArrowDown",
        "ArrowLeft",
        "ArrowRight",
        "Minus",
        "Equal",
        "Comma",
        "Period",
        "Slash",
        "Backslash",
        "Semicolon",
        "Quote",
        "Backquote",
        "BracketLeft",
        "BracketRight",
        "NumpadAdd",
        "NumpadSubtract",
        "NumpadMultiply",
        "NumpadDivide",
        "NumpadEnter",
        "NumpadDecimal",
    ];
    if NAMED.contains(&c) {
        return true;
    }
    let one = |rest: &str, ok: fn(u8) -> bool| rest.len() == 1 && ok(rest.as_bytes()[0]);
    if let Some(rest) = c.strip_prefix("Key") {
        return one(rest, |b| b.is_ascii_uppercase());
    }
    if let Some(rest) = c.strip_prefix("Digit").or_else(|| c.strip_prefix("Numpad")) {
        return one(rest, |b| b.is_ascii_digit());
    }
    if let Some(n) = c.strip_prefix('F') {
        return !n.starts_with('0')
            && n.parse::<u8>().is_ok_and(|n| (1..=24).contains(&n))
            && n.bytes().all(|b| b.is_ascii_digit());
    }
    false
}
