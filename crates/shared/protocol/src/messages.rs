//! Protocol messages.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::account::{AccountData, AccountSave};
use crate::context::Focus;
use crate::lobby::{LobbyChoices, LobbyCmd, LobbyState};
use crate::mods::{BuiltMod, ModC2S, ModProblem};
use crate::replica::GcResyncReason;
use crate::session::{Attached, ChatSend, DetachReason, GameCtl, GameCtlReason, Login, Session};
use crate::stream::{Cmd, GameBatch, KeyframeReason};
use crate::world::{
    Catalog, Command, JournalEntry, LobbyInfo, PlayerId, Query, QueryAnswer, SaveMeta, Setup,
    WorldView,
};

/// Client to server. Variant order is fixed; new variants go last.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientMsg {
    /// First message after connecting. One nick is one player (D11).
    Hello {
        protocol: u32,
        build: String,
        nick: String,
    },
    Ping {
        seq: u32,
        client_ms: u64,
    },
    /// Start a new game (local host only).
    NewGame(Setup),
    /// Continue from the newest valid save.
    Continue,
    Command(Command),
    Query {
        id: u32,
        q: Query,
    },
    /// v3 (tag 6): password, token and resume point, right after `Hello`.
    Login(Login),
    /// v3 (tag 7): a lobby command.
    Lobby(LobbyCmd),
    /// v3 (tag 8): a numbered order.
    Cmd(Cmd),
    /// v3 (tag 9): the units and city the player works with.
    Focus(Focus),
    /// v3 (tag 10): the client cannot keep its view; the answer is a keyframe (`MapBasis` first for
    /// `BasisMismatch`).
    NeedKeyframe {
        reason: KeyframeReason,
    },
    /// v3 (tag 11): a chat line.
    Chat(ChatSend),
    /// v3 (tag 12): host and seat controls of a running game.
    GameCtl(GameCtl),
    /// v4 (tag 13): GCN1 messages of the seat's UI (at most 64 KiB); the server answers with a `NET`
    /// journal record or a refusal line.
    GcNet(Vec<u8>),
    /// v4 (tag 14): the replica cannot go on; the answer is a fresh `GcBase` for this seat only.
    GcResync {
        epoch: u32,
        have_raw_hash: Option<u64>,
        at_jseq: u64,
        reason: GcResyncReason,
    },
    /// v12 (tag 15): a change of the account's store (settings and toggles); no answer.
    Account(AccountSave),
    /// v13 (tag 16): the mods this client has built in and the mod API it speaks; sent once, right after `Hello`.
    ModHello {
        api: (u16, u16, u16),
        built_in: Vec<BuiltMod>,
    },
    /// v13 (tag 17): a message of a mod's client part (at most `MOD_PAYLOAD_MAX` bytes).
    Mod(ModC2S),
}

impl ClientMsg {
    pub fn kind(&self) -> &'static str {
        match self {
            ClientMsg::Hello { .. } => "Hello",
            ClientMsg::Ping { .. } => "Ping",
            ClientMsg::NewGame(_) => "NewGame",
            ClientMsg::Continue => "Continue",
            ClientMsg::Command(_) => "Command",
            ClientMsg::Query { .. } => "Query",
            ClientMsg::Login(_) => "Login",
            ClientMsg::Lobby(_) => "Lobby",
            ClientMsg::Cmd(_) => "Cmd",
            ClientMsg::Focus(_) => "Focus",
            ClientMsg::NeedKeyframe { .. } => "NeedKeyframe",
            ClientMsg::Chat(_) => "Chat",
            ClientMsg::GameCtl(_) => "GameCtl",
            ClientMsg::GcNet(_) => "GcNet",
            ClientMsg::GcResync { .. } => "GcResync",
            ClientMsg::Account(_) => "Account",
            ClientMsg::ModHello { .. } => "ModHello",
            ClientMsg::Mod(_) => "Mod",
        }
    }
}

/// Server to client. Variant order is fixed; new variants go last.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerMsg {
    Welcome {
        session: u64,
        build: String,
    },
    Reject(RejectReason),
    Pong {
        seq: u32,
        client_ms: u64,
        server_ms: u64,
    },
    Lobby(LobbyInfo),
    GameStarted {
        catalog: Catalog,
    },
    World(WorldView),
    /// An AI player is moving.
    TurnProgress {
        active: PlayerId,
    },
    Journal(Vec<JournalEntry>),
    QueryResult {
        id: u32,
        result: QueryAnswer,
    },
    CommandRejected {
        reason: String,
    },
    Saved(SaveMeta),
    Fatal {
        message: String,
    },
    /// An autosave could not be read or written (corrupt or incompatible slot, storage error); the
    /// game goes on. Before a game it is the lobby's note on the saves.
    SaveError {
        message: String,
    },
    /// v3 (tag 13): the login worked.
    LoggedIn(Session),
    /// v3 (tag 14): the pick lists, once after login.
    Choices(LobbyChoices),
    /// v3 (tag 15): the lobby whole, on every change.
    LobbyState(LobbyState),
    /// v3 (tag 16): the connection plays a seat; the game stream follows.
    Attached(Attached),
    /// v3 (tag 17): one batch of the game stream.
    Game(GameBatch),
    /// v3 (tag 18): the connection stops playing; a `LobbyState` follows.
    Detached {
        reason: DetachReason,
    },
    /// v3 (tag 19): a lobby command was refused.
    LobbyError {
        text: String,
    },
    /// v3 (tag 20): a `GameCtl` was refused. Not in the stream, changes no game state.
    GameCtlRejected {
        ctl: GameCtl,
        reason: GameCtlReason,
    },
    /// v12 (tag 21): the account's store, after login and when the server changes its last room.
    Account(AccountData),
}

impl ServerMsg {
    pub fn kind(&self) -> &'static str {
        match self {
            ServerMsg::Welcome { .. } => "Welcome",
            ServerMsg::Reject(_) => "Reject",
            ServerMsg::Pong { .. } => "Pong",
            ServerMsg::Lobby(_) => "Lobby",
            ServerMsg::GameStarted { .. } => "GameStarted",
            ServerMsg::World(_) => "World",
            ServerMsg::TurnProgress { .. } => "TurnProgress",
            ServerMsg::Journal(_) => "Journal",
            ServerMsg::QueryResult { .. } => "QueryResult",
            ServerMsg::CommandRejected { .. } => "CommandRejected",
            ServerMsg::Saved(_) => "Saved",
            ServerMsg::Fatal { .. } => "Fatal",
            ServerMsg::SaveError { .. } => "SaveError",
            ServerMsg::LoggedIn(_) => "LoggedIn",
            ServerMsg::Choices(_) => "Choices",
            ServerMsg::LobbyState(_) => "LobbyState",
            ServerMsg::Attached(_) => "Attached",
            ServerMsg::Game(_) => "Game",
            ServerMsg::Detached { .. } => "Detached",
            ServerMsg::LobbyError { .. } => "LobbyError",
            ServerMsg::GameCtlRejected { .. } => "GameCtlRejected",
            ServerMsg::Account(_) => "Account",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RejectReason {
    VersionMismatch {
        server: u32,
    },
    BadNick,
    /// v3 (tag 2): wrong password.
    BadPassword,
    /// v3 (tag 3): the message is a B1 variant the v3 server does not take (NP-D18).
    Unsupported {
        kind: String,
    },
    /// v3 (tag 4): the server is full.
    Full,
    /// v3 (tag 5): the nick is reserved for another token, or its record has a live session.
    NickTaken,
    /// v13 (tag 6): the client does not have the mods the game needs.
    ModMismatch(Vec<ModProblem>),
}

impl RejectReason {
    pub fn kind(&self) -> &'static str {
        match self {
            RejectReason::VersionMismatch { .. } => "VersionMismatch",
            RejectReason::BadNick => "BadNick",
            RejectReason::BadPassword => "BadPassword",
            RejectReason::Unsupported { .. } => "Unsupported",
            RejectReason::Full => "Full",
            RejectReason::NickTaken => "NickTaken",
            RejectReason::ModMismatch(_) => "ModMismatch",
        }
    }
}

impl fmt::Display for RejectReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RejectReason::VersionMismatch { server } => write!(
                f,
                "protocol version mismatch: server {server}, client {}",
                crate::PROTOCOL_VERSION
            ),
            RejectReason::BadNick => f.write_str("invalid nickname"),
            RejectReason::BadPassword => f.write_str("wrong password"),
            RejectReason::Unsupported { kind } => write!(f, "unsupported message: {kind}"),
            RejectReason::Full => f.write_str("server is full"),
            RejectReason::NickTaken => f.write_str("nickname is taken"),
            RejectReason::ModMismatch(p) => {
                f.write_str("mod mismatch")?;
                for (i, x) in p.iter().enumerate() {
                    f.write_str(if i == 0 { ": " } else { "; " })?;
                    f.write_str(&x.text)?;
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameCommand {
    pub kind: u16,
    pub player: u8,
    pub args: Vec<i32>,
    pub text: Option<String>,
}
