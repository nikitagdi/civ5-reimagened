//! Mod envelope types (protocol v13, SPEC D5). Plain structs: the protocol does not depend on `mod-manifest` (R5);
//! versions are strings, resolved by the manifest crate. All uses are appended at the end of their enums and
//! structs; the layout of this file is part of the wire format.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::world::PlayerId;

/// Largest payload of one mod message, either direction. Below the 64 KiB gateway frame with room for the envelope.
pub const MOD_PAYLOAD_MAX: usize = 56 * 1024;
/// Most mods in one game (and in one `ModHello` / `ModSetView`).
pub const MAX_MODS: usize = 32;
/// Longest mod id, in characters.
pub const MAX_MOD_ID_CHARS: usize = 64;
/// Longest version or version requirement string, in characters.
pub const MAX_MOD_VERSION_CHARS: usize = 64;
/// Most problems in one `ModPlan::Err` or `RejectReason::ModMismatch`.
pub const MAX_MOD_PROBLEMS: usize = 16;
/// Longest problem text, in characters.
pub const MAX_MOD_PROBLEM_CHARS: usize = 240;

/// How a mod takes part in a game. Variant order is the wire format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModSync {
    /// Changes game state: every seat must have the same code and content.
    Gameplay,
    /// Server-side logic, clients may lack the visuals.
    Cosmetic,
    /// Client-only: the server does not know about it.
    Client,
}

/// A mod the client has built in (`ClientMsg::ModHello`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuiltMod {
    pub id: String,
    pub version: String,
    /// Hash of the mod's code identity (the build); 16 bytes of the manifest crate's digest.
    pub code_hash: [u8; 16],
}

/// `ClientMsg::Mod`: a message of a mod's client part to its server part.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModC2S {
    /// Index in the game's resolved load order (`ModView.load_idx`).
    pub mod_idx: u16,
    pub ch: u8,
    /// At most [`MOD_PAYLOAD_MAX`] bytes.
    pub payload: Vec<u8>,
}

/// `GameMsg::Mod`: a message of a mod's server part to its client part.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModS2C {
    pub mod_idx: u16,
    pub ch: u8,
    /// The seat that caused the message, if the mod says so.
    pub from: Option<PlayerId>,
    /// At most [`MOD_PAYLOAD_MAX`] bytes.
    pub payload: Vec<u8>,
}

// A payload is binary and up to 56 KiB: the log shows its length only.
impl fmt::Debug for ModC2S {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModC2S")
            .field("mod_idx", &self.mod_idx)
            .field("ch", &self.ch)
            .field("payload_len", &self.payload.len())
            .finish()
    }
}

impl fmt::Debug for ModS2C {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModS2C")
            .field("mod_idx", &self.mod_idx)
            .field("ch", &self.ch)
            .field("from", &self.from)
            .field("payload_len", &self.payload.len())
            .finish()
    }
}

impl ModC2S {
    pub fn within_limits(&self) -> bool {
        self.payload.len() <= MOD_PAYLOAD_MAX
    }
}

impl ModS2C {
    pub fn within_limits(&self) -> bool {
        self.payload.len() <= MOD_PAYLOAD_MAX
    }
}

/// One mod of a resolved game set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModView {
    pub id: String,
    pub version: String,
    /// Position in the load order; the `mod_idx` of its messages.
    pub load_idx: u16,
    pub sync: ModSync,
    /// The client must have the mod to play.
    pub client_required: bool,
    /// For the receiving client: it has this mod (always true for a server-side plan).
    pub available: bool,
}

/// `GameMsg::Mods`: the frozen set of a game, the first message of every `Fresh` stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModSetView {
    /// Hash of the resolved set (manifest crate), shown in the room and checked at attach.
    pub modset_hash: u64,
    pub mods: Vec<ModView>,
}

/// The host's request for one mod (`GameSettings.mods`): an id and a version requirement (`"^1.2"`, `"*"`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModPick {
    pub id: String,
    pub req: String,
}

/// A mod the server has installed (`LobbyChoices.mods`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledModView {
    pub id: String,
    pub version: String,
    pub title: String,
    pub sync: ModSync,
    pub client_required: bool,
}

/// Why a set cannot be played, for people.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModProblem {
    pub text: String,
}

impl fmt::Display for ModProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

/// `RoomState.mod_plan`: the resolved set of the room, or why there is none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModPlan {
    Ok(Vec<ModView>),
    Err(Vec<ModProblem>),
}

impl Default for ModPlan {
    fn default() -> Self {
        ModPlan::Ok(Vec::new())
    }
}

/// Longest `mods.lock` text of a [`ModsSection`], bytes.
pub const MAX_MODS_LOCK: usize = 256 * 1024;

/// The mods block of a save (`MODS` section of `C5SV`) and of the gateway-worker IPC (`ToWorker::Mods`,
/// `WorkerMsg::ModsLoaded`): the frozen set of the game and the state of each server-side mod. Opaque to the
/// worker (it does not know the manifest types): `lock` is the `mods.lock` text (it carries the set hash), and the
/// mod host checks it with `mod_manifest::parse_lock`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModsSection {
    pub lock: String,
    /// `persist()` of every server-side mod that has state, by mod id, in load order.
    pub states: Vec<ModState>,
}

/// The saved state of one server-side mod.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModState {
    pub id: String,
    pub state: Vec<u8>,
}

impl ModsSection {
    /// The bounds that protect a reader from a damaged or hostile file or frame.
    pub fn within_limits(&self) -> bool {
        self.lock.len() <= MAX_MODS_LOCK && self.states.len() <= MAX_MODS
    }
}
