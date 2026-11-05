//! Login, resume and attachment to a game (net-protocol 4.2, 4.3, 4.12). Field and variant order is the
//! wire format.
//!
//! `Debug` of `Login`, `Token` and `Session` prints `***` instead of the password and the token, so they
//! never reach a log (tested).

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::stream::{ChatTo, CmdId};
use crate::world::{GameId, PlayerId};

/// Session token: 128 random bits, issued by the gateway at login (`ServerMsg::LoggedIn`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Token(pub [u8; 16]);

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(***)")
    }
}

/// Where a reconnecting client continues the stream. Kept in page memory only: after a reload there is no
/// view and nothing to resume (the client logs in with the token alone and gets `Fresh`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resume {
    /// `Attached.stream` of the stream the client was on.
    pub stream: u64,
    /// `seq` of the last batch the client received.
    pub last_seq: u64,
    /// The map basis the client already holds (NP-D22).
    pub basis: Option<BasisClaim>,
}

/// The map basis a client already has (NP-D22): the game and its `basis_hash`. The gateway does not send
/// `MapBasis` when the hash equals the hash of the seat's basis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BasisClaim {
    pub game: GameId,
    pub hash: u64,
}

/// `ClientMsg::Login`, sent right after `Hello` (one round trip).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Login {
    pub password: String,
    /// The token of an earlier session; unknown means as without a token.
    pub token: Option<Token>,
    pub resume: Option<Resume>,
}

impl fmt::Debug for Login {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Login")
            .field("password", &format_args!("***"))
            .field("token", &self.token)
            .field("resume", &self.resume)
            .finish()
    }
}

/// `ServerMsg::LoggedIn`: who the connection is and the token to log in with next time.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    /// The device token. `None`: the connection logged in with the HttpOnly session cookie of the web page (v9), and the
    /// page never holds the token. `Some`: a token or password login (native client, bots); the client stores it.
    pub token: Option<Token>,
    pub nick: String,
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("token", &format_args!("***"))
            .field("nick", &self.nick)
            .finish()
    }
}

/// `ClientMsg::Chat` payload. The text is at most 500 characters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatSend {
    pub to: ChatTo,
    pub text: String,
}

/// How the stream of an `Attached` starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttachMode {
    /// The first batch carries a keyframe (and `MapBasis` unless the client holds the same one).
    Fresh,
    /// The batches after `Resume.last_seq` follow as they were.
    Replay,
}

/// Where the client fetches the catalog (`GET /catalog/{hash:016x}`, NP-D14): xxh64 of the body and its
/// length.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogRef {
    pub hash: u64,
    pub len: u32,
}

/// `ServerMsg::Attached`: the connection now plays a seat; the game stream follows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attached {
    pub game: GameId,
    pub player: PlayerId,
    /// Number of the stream; changes when the gateway restarts.
    pub stream: u64,
    pub mode: AttachMode,
    pub catalog: CatalogRef,
    /// Orders continue from `last_cmd_id + 1`.
    pub last_cmd_id: CmdId,
}

/// Why the connection stops playing its seat (`ServerMsg::Detached`). A `LobbyState` always follows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DetachReason {
    Kicked,
    GameOver,
    GameStopped {
        why: String,
    },
    /// The same token logged in elsewhere; sent to the old socket before it is closed.
    Replaced,
    /// Answer to `GameCtl::Leave`; the seat stays with the player (NP-D21).
    Left,
}

/// `ClientMsg::GameCtl`: host and seat controls of a running game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameCtl {
    Kick {
        player: PlayerId,
    },
    Pause(bool),
    /// A voluntary leave is a disconnect (N-D7, NP-D21).
    Leave,
    /// v5: the host asks for a save of the game now (the menu's "Save game", B4 K20). The server saves like an
    /// administrator's save does (`SaveReason::Admin`) and the save joins the list of saves; only the host may.
    Save,
}

/// Why a `GameCtl` was refused (`ServerMsg::GameCtlRejected`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameCtlReason {
    NotHost,
    NoSuchPlayer,
    /// The target is not a human.
    NotHuman,
    /// The host cannot kick itself.
    SelfKick,
    /// The target is the last human of the game.
    LastHuman,
    NoGame,
}
