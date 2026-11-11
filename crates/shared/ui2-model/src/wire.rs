//! Types that cross module boundaries on the wire between the UI runtime, the script host, the actions module
//! and the host. They live here so that none of those modules depends on another one just for its messages.
//! They carry plain strings and [`Value`]s only: this crate knows nothing about the skin package format.

use std::sync::Arc;

use crate::value::Value;

/// Index of a mod in the host's mod table (a small number, not the mod id).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModIdx(pub u16);

/// Who raised an intent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Origin {
    /// A markup action; `node` is the node index in the package template.
    Markup { node: u32 },
    /// A script.
    Script,
    /// A `data-hotkey` key press.
    Hotkey,
    /// A click on the map.
    Map,
    /// A mod.
    Mod(ModIdx),
}

/// A command the UI sends to the actions module: a name from the intent catalog and named arguments.
#[derive(Clone, Debug, PartialEq)]
pub struct WireIntent {
    pub name: Arc<str>,
    pub args: Vec<(Arc<str>, Value)>,
    pub origin: Origin,
}

/// A write to the local state of a skin or mod (`state.*`, `modstate.<id>.*`, `js = true` fields of `ui`).
#[derive(Clone, Debug, PartialEq)]
pub struct StateOp {
    pub path: Arc<str>,
    pub value: Value,
}

/// An intent addressed to a mod's Rust half.
#[derive(Clone, Debug, PartialEq)]
pub struct ModIntent {
    pub mod_id: Arc<str>,
    pub name: Arc<str>,
    pub args: Vec<(Arc<str>, Value)>,
}

/// A script context: the skin's own scripts or one mod's.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ScriptCtxId {
    Skin,
    Mod(Arc<str>),
}

/// A markup event delivered to a script (`data-on-<ev>="call ..."`, `data-event`).
#[derive(Clone, Debug, PartialEq)]
pub struct ScriptEvent {
    pub ctx: ScriptCtxId,
    pub name: Arc<str>,
    pub arg: Value,
    /// Where the node is in the skin source, for error messages.
    pub node_path: Arc<str>,
}

/// What a script may do to a node of an island (a part of the page that markup does not bind).
#[derive(Clone, Debug, PartialEq)]
pub enum IslandOp {
    /// Replace the text.
    Text(Value),
    /// Add (`true`) or remove (`false`) a class.
    Cls(Arc<str>, bool),
    /// Set a style property (name as in CSS) to a value.
    Style(Arc<str>, Value),
    /// Show or hide.
    Show(bool),
}

/// A script command to one node of an island, addressed by the island name and the node's key inside it.
#[derive(Clone, Debug, PartialEq)]
pub struct IslandCmd {
    pub island: Arc<str>,
    pub key: Arc<str>,
    pub op: IslandOp,
}

macro_rules! name_enum {
    ($(#[$m:meta])* $name:ident { $($var:ident => $s:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name { $($var),+ }

        impl $name {
            /// Every value, in declaration order.
            pub const ALL: &'static [$name] = &[$($name::$var),+];

            /// The name used in markup, samples and scripts.
            #[must_use]
            pub fn as_str(self) -> &'static str {
                match self { $($name::$var => $s),+ }
            }

            /// Parse the name.
            #[must_use]
            pub fn from_name(s: &str) -> Option<$name> {
                match s { $($s => Some($name::$var),)+ _ => None }
            }
        }
    };
}

name_enum! {
    /// The windows of the skin (they open over the map and are bound to the `ui.windows` state).
    WindowId {
        Production => "production",
        Research => "research",
        Menu => "menu",
        Chat => "chat",
        NotesLog => "notes_log",
    }
}

name_enum! {
    /// The screens that live outside the skin.
    ScreenId {
        TechTree => "tech_tree",
        Policies => "policies",
        Religion => "religion",
        Diplomacy => "diplomacy",
        Overview => "overview",
        NotificationLog => "notification_log",
    }
}
