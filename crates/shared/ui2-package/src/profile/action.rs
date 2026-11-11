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
