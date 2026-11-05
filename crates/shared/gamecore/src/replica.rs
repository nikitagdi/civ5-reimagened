//! The read-only replica of the game (B4 gc-replica).
//!
//! The server runs the game with [`Role::Server`]: every input (the humans' commands, their UIs' net
//! messages, slot changes, the slices of every pump) goes to a journal (`GCJ1`). A client holds a
//! [`ReplicaGame`]: a snapshot (`GCS1`) on join, then the journal batches in order, each checked
//! against the server's replica digest; at a turn boundary or on a divergence a new snapshot
//! replaces the game in place. The UI's own actions never change the replica: the DLL's net
//! messages (`gDLL->send*`) go to an outbox ([`NetMessage`], `GCN1`) for the server.

use gamecore_sys::ffi;

use crate::game::{Game, GameError};
use crate::host::HostServices;
use crate::ui::{UiEvent, UiOp};

/// Who drives the game in this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The host drives the game; nothing is journaled.
    Plain,
    /// The authority: every input is journaled for [`Game::journal_take`].
    Server,
    /// A read-only copy: only [`Game::journal_apply`] and [`Game::resync`] change it. `seat` is the
    /// player whose UI it serves (the sender of its net messages on the server).
    Replica { seat: Option<u8> },
}

impl Role {
    fn abi(self) -> (i32, i32) {
        match self {
            Role::Plain => (ffi::GC_ROLE_PLAIN, -1),
            Role::Server => (ffi::GC_ROLE_SERVER, -1),
            Role::Replica { seat } => (ffi::GC_ROLE_REPLICA, seat.map_or(-1, i32::from)),
        }
    }
}

/// The DLL's net messages in the order of `rt_net.h` (`RtNetId`, append only): the id of a
/// [`NetMessage`] is the index here. The arguments are those of the `gDLL->send*` call, in order.
pub const NET_MESSAGE_NAMES: [&str; 73] = [
    "None",
    "sendTurnComplete",
    "sendTurnUnready",
    "SendAICivsProcessed",
    "IdeologyChoice",
    "ArchaeologyChoice",
    "SendDiploVote",
    "SendEnhanceReligion",
    "SendFaithPurchase",
    "SendFoundPantheon",
    "SendFoundReligion",
    "SendLeagueEditName",
    "SendLeagueProposeEnact",
    "SendLeagueProposeRepeal",
    "SendLeagueVoteAbstain",
    "SendLeagueVoteEnact",
    "SendLeagueVoteRepeal",
    "SendLiberateMinor",
    "SendMinorNoUnitSpawning",
    "SendMoveSpy",
    "SendPledgeMinorProtection",
    "SendRenameCity",
    "SendRenameUnit",
    "SendSellBuilding",
    "SendStageCoup",
    "SendUpdateCityCitizens",
    "sendAdvancedStartAction",
    "sendAutoMission",
    "sendBarbarianRansom",
    "sendChangeIdeology",
    "sendChangeWar",
    "sendCityBuyPlot",
    "sendDoCommand",
    "sendDoTask",
    "sendFaithGreatPersonChoice",
    "sendFromUIDiploEvent",
    "sendGiftUnit",
    "sendGoodyChoice",
    "sendGreatPersonChoice",
    "sendIgnoreWarning",
    "sendLaunch",
    "sendMayaBonusChoice",
    "sendMinorBullyGold",
    "sendMinorBullyUnit",
    "sendMinorBuyout",
    "sendMinorCivEnterTerritory",
    "sendMinorCivIntrusion",
    "sendMinorCivQuestCompleted",
    "sendMinorCivQuestNoInterest",
    "sendMinorGiftGold",
    "sendMinorGiftTileImprovement",
    "sendMinorPledgeProtection",
    "sendMoveGreatWorks",
    "sendPlayerHurry",
    "sendPopOrder",
    "sendPurchase_1",
    "sendPurchase_2",
    "sendPushMission",
    "sendPushOrder",
    "sendResearch",
    "sendReturnCivilian",
    "sendSetCityAIFocus",
    "sendSetCityAvoidGrowth",
    "sendSetSwappableGreatWork",
    "sendSwapGreatWorks",
    "sendSwapOrder",
    "sendSwapUnits",
    "sendUpdatePolicies",
    "sendAutoMoves",
    "sendNetDealAccepted",
    "sendNetDemandAccepted",
    "sendNetDealProposed",
    "sendNetPlayerDealFinalized",
];

/// The id of a net message by its `gDLL->send*` name (`"sendPushOrder"`).
pub fn net_message_id(name: &str) -> Option<i32> {
    NET_MESSAGE_NAMES
        .iter()
        .position(|n| *n == name)
        .and_then(|i| i32::try_from(i).ok())
}

/// The turn handshake is not a UI message: the end of turn is `END_TURN`, checked by the server.
pub fn net_message_from_ui(id: i32) -> bool {
    let name = usize::try_from(id)
        .ok()
        .and_then(|i| NET_MESSAGE_NAMES.get(i));
    !matches!(
        name,
        None | Some(
            &"None"
                | &"sendTurnComplete"
                | &"sendTurnUnready"
                | &"SendAICivsProcessed"
                | &"sendAutoMoves"
        )
    )
}

/// One DLL net message (one `gDLL->send*` call). The sender is not part of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetMessage {
    pub id: i32,
    pub args: [i32; ffi::GC_NET_ARGS],
    pub text: String,
}

impl NetMessage {
    /// A message by `gDLL->send*` name and its leading arguments (the rest are 0).
    pub fn new(name: &str, args: &[i32]) -> Option<NetMessage> {
        let id = net_message_id(name)?;
        if args.len() > ffi::GC_NET_ARGS {
            return None;
        }
        let mut a = [0; ffi::GC_NET_ARGS];
        a[..args.len()].copy_from_slice(args);
        Some(NetMessage {
            id,
            args: a,
            text: String::new(),
        })
    }

    /// Its `gDLL->send*` name, `"?"` for an id this build does not know.
    pub fn name(&self) -> &'static str {
        usize::try_from(self.id)
            .ok()
            .and_then(|i| NET_MESSAGE_NAMES.get(i))
            .copied()
            .unwrap_or("?")
    }
}

/// A malformed `GCJ1` / `GCN1` buffer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("malformed {format} data: {why}")]
pub struct FormatError {
    pub format: &'static str,
    pub why: String,
}

const NET_MAGIC: &[u8; 4] = b"GCN1";
const JOURNAL_MAGIC: &[u8; 4] = b"GCJ1";
const FORMAT_VERSION: u16 = 1;
const NO_TEXT: u16 = 0xFFFF;

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|s| s[0])
    }
    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|s| u16::from_le_bytes([s[0], s[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4)
            .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn i32(&mut self) -> Option<i32> {
        self.u32().map(|v| v as i32)
    }
    fn u64(&mut self) -> Option<u64> {
        self.take(8)
            .map(|s| u64::from_le_bytes(s.try_into().expect("8 bytes")))
    }
    fn text(&mut self) -> Option<Option<String>> {
        let n = self.u16()?;
        if n == NO_TEXT {
            return Some(None);
        }
        let s = self.take(usize::from(n))?;
        Some(Some(String::from_utf8_lossy(s).into_owned()))
    }
    fn at_end(&self) -> bool {
        self.pos == self.b.len()
    }
}

fn put_text(out: &mut Vec<u8>, text: Option<&str>) {
    match text {
        None => out.extend_from_slice(&NO_TEXT.to_le_bytes()),
        Some(t) => {
            let n = t.len().min(usize::from(NO_TEXT - 1));
            out.extend_from_slice(&(n as u16).to_le_bytes());
            out.extend_from_slice(&t.as_bytes()[..n]);
        }
    }
}

fn header(out: &mut Vec<u8>, magic: &[u8; 4], count: usize) {
    out.extend_from_slice(magic);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(count as u32).to_le_bytes());
}

fn read_header(
    r: &mut Reader<'_>,
    magic: &[u8; 4],
    format: &'static str,
) -> Result<u32, FormatError> {
    let bad = |why: &str| FormatError {
        format,
        why: why.into(),
    };
    if r.take(4) != Some(&magic[..]) {
        return Err(bad("bad magic"));
    }
    if r.u16() != Some(FORMAT_VERSION) {
        return Err(bad("unknown format version"));
    }
    r.u16().ok_or_else(|| bad("truncated header"))?;
    r.u32().ok_or_else(|| bad("truncated header"))
}

fn net_body(out: &mut Vec<u8>, m: &NetMessage) {
    out.extend_from_slice(&m.id.to_le_bytes());
    for a in m.args {
        out.extend_from_slice(&a.to_le_bytes());
    }
    put_text(out, Some(&m.text));
}

fn read_net_body(r: &mut Reader<'_>) -> Option<NetMessage> {
    let id = r.i32()?;
    let mut args = [0; ffi::GC_NET_ARGS];
    for a in &mut args {
        *a = r.i32()?;
    }
    let text = r.text()?.unwrap_or_default();
    Some(NetMessage { id, args, text })
}

/// `GCN1` bytes of `msgs` (gamecore_abi.h, `gc_net_submit`).
pub fn encode_net(msgs: &[NetMessage]) -> Vec<u8> {
    let mut out = Vec::new();
    header(&mut out, NET_MAGIC, msgs.len());
    for m in msgs {
        let mut body = Vec::new();
        net_body(&mut body, m);
        out.extend_from_slice(&(body.len() as u16).to_le_bytes());
        out.extend_from_slice(&body);
    }
    out
}

/// The messages of `GCN1` bytes.
pub fn decode_net(bytes: &[u8]) -> Result<Vec<NetMessage>, FormatError> {
    let bad = |why: &str| FormatError {
        format: "GCN1",
        why: why.into(),
    };
    let mut r = Reader { b: bytes, pos: 0 };
    let count = read_header(&mut r, NET_MAGIC, "GCN1")?;
    let mut out = Vec::new();
    for _ in 0..count {
        let len = r.u16().ok_or_else(|| bad("truncated message"))?;
        let body = r
            .take(usize::from(len))
            .ok_or_else(|| bad("truncated message"))?;
        let mut b = Reader { b: body, pos: 0 };
        let m = read_net_body(&mut b).ok_or_else(|| bad("malformed message"))?;
        if !b.at_end() {
            return Err(bad("bytes after a message"));
        }
        out.push(m);
    }
    if !r.at_end() {
        return Err(bad("bytes after the last message"));
    }
    Ok(out)
}

/// One input of the game in a server's journal (`GCJ1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JournalRecord {
    /// A command accepted by `gc_apply_command` (or a test bot).
    Command {
        player: u8,
        kind: u16,
        args: Vec<i32>,
        text: Option<String>,
    },
    /// A human UI's net message, accepted by `gc_net_submit`.
    Net { player: u8, message: NetMessage },
    /// A `gc_pump` call: the slices it ran, the core's status and the game turn after it.
    Pump { slices: u32, status: i32, turn: i32 },
    /// A slot change (`gc_slot_control`).
    Slot {
        player: u8,
        op: i32,
        nick: Option<String>,
    },
    /// The server's replica digest (`gc_state_digest(-3)`) at this point.
    Check { digest: u64 },
}

impl JournalRecord {
    /// The game turn after this record, for a pump.
    pub fn turn_after(&self) -> Option<i32> {
        match self {
            JournalRecord::Pump { turn, .. } => Some(*turn),
            _ => None,
        }
    }
}

/// `GCJ1` bytes of `records` (a server's tools and tests; the core writes its own).
pub fn encode_journal(records: &[JournalRecord]) -> Vec<u8> {
    let mut out = Vec::new();
    header(&mut out, JOURNAL_MAGIC, records.len());
    for rec in records {
        let mut p = Vec::new();
        let (kind, player) = match rec {
            JournalRecord::Command {
                player,
                kind,
                args,
                text,
            } => {
                p.extend_from_slice(&kind.to_le_bytes());
                p.extend_from_slice(&(args.len() as u16).to_le_bytes());
                for a in args {
                    p.extend_from_slice(&a.to_le_bytes());
                }
                put_text(&mut p, text.as_deref());
                (ffi::GC_JOURNAL_COMMAND, *player)
            }
            JournalRecord::Net { player, message } => {
                net_body(&mut p, message);
                (ffi::GC_JOURNAL_NET, *player)
            }
            JournalRecord::Pump {
                slices,
                status,
                turn,
            } => {
                p.extend_from_slice(&slices.to_le_bytes());
                p.extend_from_slice(&status.to_le_bytes());
                p.extend_from_slice(&turn.to_le_bytes());
                (ffi::GC_JOURNAL_PUMP, 0xFF)
            }
            JournalRecord::Slot { player, op, nick } => {
                p.extend_from_slice(&op.to_le_bytes());
                put_text(&mut p, nick.as_deref());
                (ffi::GC_JOURNAL_SLOT, *player)
            }
            JournalRecord::Check { digest } => {
                p.extend_from_slice(&digest.to_le_bytes());
                (ffi::GC_JOURNAL_CHECK, 0xFF)
            }
        };
        out.push(kind);
        out.push(player);
        out.extend_from_slice(&(p.len() as u16).to_le_bytes());
        out.extend_from_slice(&p);
    }
    out
}

/// The records of `GCJ1` bytes, in order.
pub fn decode_journal(bytes: &[u8]) -> Result<Vec<JournalRecord>, FormatError> {
    let bad = |why: &str| FormatError {
        format: "GCJ1",
        why: why.into(),
    };
    let mut r = Reader { b: bytes, pos: 0 };
    let count = read_header(&mut r, JOURNAL_MAGIC, "GCJ1")?;
    let mut out = Vec::new();
    for _ in 0..count {
        let kind = r.u8().ok_or_else(|| bad("truncated record"))?;
        let player = r.u8().ok_or_else(|| bad("truncated record"))?;
        let len = r.u16().ok_or_else(|| bad("truncated record"))?;
        let body = r
            .take(usize::from(len))
            .ok_or_else(|| bad("truncated record"))?;
        let mut b = Reader { b: body, pos: 0 };
        let rec = match kind {
            ffi::GC_JOURNAL_COMMAND => (|| {
                let kind = b.u16()?;
                let argc = b.u16()?;
                let mut args = Vec::with_capacity(usize::from(argc));
                for _ in 0..argc {
                    args.push(b.i32()?);
                }
                let text = b.text()?;
                Some(JournalRecord::Command {
                    player,
                    kind,
                    args,
                    text,
                })
            })(),
            ffi::GC_JOURNAL_NET => {
                read_net_body(&mut b).map(|message| JournalRecord::Net { player, message })
            }
            ffi::GC_JOURNAL_PUMP => (|| {
                Some(JournalRecord::Pump {
                    slices: b.u32()?,
                    status: b.i32()?,
                    turn: b.i32()?,
                })
            })(),
            ffi::GC_JOURNAL_SLOT => (|| {
                let op = b.i32()?;
                let nick = b.text()?;
                Some(JournalRecord::Slot { player, op, nick })
            })(),
            ffi::GC_JOURNAL_CHECK => b.u64().map(|digest| JournalRecord::Check { digest }),
            _ => return Err(bad("unknown record type")),
        }
        .ok_or_else(|| bad("malformed record"))?;
        if !b.at_end() {
            return Err(bad("bytes after a record"));
        }
        out.push(rec);
    }
    if !r.at_end() {
        return Err(bad("bytes after the last record"));
    }
    Ok(out)
}

/// The replica digest (`gc_state_digest(-3)`): what a server and its replicas compare.
pub const REPLICA_DIGEST: i32 = -3;

/// What one journal batch did to a replica.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// Every record ran as on the server.
    InStep,
    /// The replica left the server's game at this batch; it needs a snapshot ([`ReplicaGame::resync`]).
    Diverged(String),
}

/// A client's read-only copy of the server's game.
pub struct ReplicaGame {
    game: Game,
    seat: Option<u8>,
    /// The seat the core's role was set with.
    role_seat: Option<u8>,
    joined: bool,
    batches: u64,
    divergences: u64,
    resyncs: u64,
    in_step: bool,
}

impl ReplicaGame {
    /// Boots the core (game data through `host`). One per process, like [`Game`].
    pub fn new(host: impl HostServices + 'static, seat: Option<u8>) -> Result<Self, GameError> {
        Ok(ReplicaGame {
            game: Game::new(host)?,
            seat,
            role_seat: None,
            joined: false,
            batches: 0,
            divergences: 0,
            resyncs: 0,
            in_step: false,
        })
    }

    /// The seat of the player in the next game this core replicates (the core boots once per process): the role
    /// follows with the next snapshot.
    pub fn set_seat(&mut self, seat: Option<u8>) {
        self.seat = seat;
    }

    /// Takes the server's snapshot: the first one loads the game, later ones replace it in place.
    pub fn resync(&mut self, snapshot: &[u8]) -> Result<(), GameError> {
        if self.joined {
            self.game.resync(snapshot)?;
            self.resyncs += 1;
            // Another game of the same process (the player left and joined one with another seat).
            if self.role_seat != self.seat {
                self.game.set_role(Role::Replica { seat: self.seat })?;
                self.role_seat = self.seat;
            }
        } else {
            self.game.load(snapshot)?;
            self.game.set_role(Role::Replica { seat: self.seat })?;
            self.role_seat = self.seat;
            self.joined = true;
        }
        self.in_step = true;
        Ok(())
    }

    /// Replays one journal batch of the server. After a divergence nothing more is applied until
    /// the next [`resync`](Self::resync).
    pub fn apply(&mut self, batch: &[u8]) -> Result<Applied, GameError> {
        if !self.joined {
            return Err(GameError::NotStarted);
        }
        if !self.in_step {
            return Ok(Applied::Diverged("waiting for a snapshot".into()));
        }
        match self.game.journal_apply(batch) {
            Ok(()) => {
                self.batches += 1;
                Ok(Applied::InStep)
            }
            Err(GameError::Diverged(why)) => {
                self.batches += 1;
                self.divergences += 1;
                self.in_step = false;
                Ok(Applied::Diverged(why))
            }
            Err(e) => Err(e),
        }
    }

    /// The net messages the UI sent since the last call, for the server.
    pub fn take_ui_messages(&mut self) -> Result<Vec<NetMessage>, GameError> {
        self.game.net_take()
    }

    /// The game, for the UI's getters. The core refuses anything that would move it.
    pub fn game(&mut self) -> &mut Game {
        &mut self.game
    }

    /// Puts the DLL's Lua libraries (`Game`, `Players`, `Map`, `GameInfo`, `DB.Query`, `GameDefines`, `PreGame` reads)
    /// into the UI's Lua state, so its scripts read this replica. The state's stack is as before,
    /// also on an error. The replica must stay alive as long as the state calls into it, and be attached again to a new state.
    pub fn attach_lua(&mut self, lua: &lua_sys::Lua) -> Result<(), GameError> {
        // SAFETY: `lua` is a live state of the Lua this build links; the replica outlives its use per the contract above.
        let r = unsafe { self.game.sys().lua_attach_ui(lua.as_ptr()) };
        crate::flush_cpp_output();
        Ok(r?)
    }

    /// After a snapshot replaced the game (`resync`, a later one): objects of the old game that the UI's Lua state still
    /// holds (units, cities, plots, areas, deals, the league) raise a Lua error when used instead of reading freed memory.
    /// `Players` and `Teams` stay valid.
    pub fn invalidate_lua(&mut self, lua: &lua_sys::Lua) {
        // SAFETY: as in `attach_lua`.
        if let Err(e) = unsafe { self.game.sys().lua_invalidate_ui(lua.as_ptr()) } {
            tracing::warn!("invalidate_lua: {e}");
        }
        crate::flush_cpp_output();
    }

    /// Moves the events the engine sent the UI since the last call to `out`, in order (replica role).
    pub fn take_ui_events(&mut self, out: &mut Vec<UiEvent>) {
        let mut raw = Vec::new();
        self.game.sys().take_ui_events(&mut raw);
        out.extend(raw.into_iter().map(UiEvent::from_raw));
    }

    /// An operation on the exe's interface state (selection, interface mode, city screen); the answer is appended to `out`.
    pub fn ui_op(&mut self, op: UiOp, args: &[i32], out: &mut Vec<i32>) -> Result<(), GameError> {
        let r = self.game.sys().ui_op(op.code(), args, out);
        crate::flush_cpp_output();
        Ok(r?)
    }

    /// The UI listens to `GameEvents.<name>`: after the core's listeners it gets the same call as a
    /// [`UiEventKind::GameEvent`] (its results do not count: the rules do not see the UI's listeners).
    pub fn subscribe_game_event(&mut self, name: &str) {
        if let Err(e) = self.game.sys().ui_subscribe_game_event(name) {
            tracing::warn!("subscribe_game_event({name}): {e}");
        }
    }

    /// After a load or a resync: the engine announces everything alive to the UI, as the exe does after a load
    /// (events with `rebuild` set; units, cities, wonders, the fog of the seat, the seat's notifications).
    pub fn setup_graphical(&mut self) -> Result<(), GameError> {
        let r = self.game.sys().ui_setup_graphical();
        crate::flush_cpp_output();
        Ok(r?)
    }

    /// False after a divergence until the next snapshot.
    pub fn in_step(&self) -> bool {
        self.in_step
    }

    /// Counters for `__civ5stats`: batches applied, divergences, resyncs after the first load.
    pub fn stats(&self) -> (u64, u64, u64) {
        (self.batches, self.divergences, self.resyncs)
    }
}

impl Game {
    /// Sets who drives this game (B4 gc-replica).
    pub fn set_role(&mut self, role: Role) -> Result<(), GameError> {
        let (r, p) = role.abi();
        Ok(self.sys().set_role(r, p)?)
    }

    /// Server: the inputs since the last call as `GCJ1`; `check_digest` ends a non-empty batch with
    /// the replica digest (costs one serialization of the game).
    pub fn journal_take(&mut self, check_digest: bool) -> Result<Vec<u8>, GameError> {
        let r = self.sys().journal_take(check_digest);
        crate::flush_cpp_output();
        Ok(r?)
    }

    /// Replica: replays a `GCJ1` batch. `Err(Diverged)` when the replica left the server's game.
    pub fn journal_apply(&mut self, batch: &[u8]) -> Result<(), GameError> {
        let r = self.sys().journal_apply(batch);
        crate::flush_cpp_output();
        Ok(r?)
    }

    /// Replica: the UI's net messages since the last call.
    pub fn net_take(&mut self) -> Result<Vec<NetMessage>, GameError> {
        let bytes = self.sys().net_take()?;
        decode_net(&bytes).map_err(|e| GameError::BadData(e.to_string()))
    }

    /// Server: queues a seated human's UI messages, all or nothing.
    pub fn net_submit(&mut self, player: u8, msgs: &[NetMessage]) -> Result<(), GameError> {
        let r = self.sys().net_submit(i32::from(player), &encode_net(msgs));
        crate::flush_cpp_output();
        Ok(r?)
    }

    /// Replaces the running game with a `GCS1` snapshot in place.
    pub fn resync(&mut self, snapshot: &[u8]) -> Result<(), GameError> {
        let r = self.sys().resync(snapshot);
        crate::flush_cpp_output();
        Ok(r?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn net_ids_follow_the_core_list() {
        assert_eq!(net_message_id("sendPushOrder"), Some(58));
        assert_eq!(net_message_id("sendFromUIDiploEvent"), Some(35));
        assert_eq!(net_message_id("sendAutoMoves"), Some(68));
        assert_eq!(net_message_id("nope"), None);
        assert!(net_message_from_ui(58));
        assert!(!net_message_from_ui(1));
        assert!(!net_message_from_ui(68));
        assert_eq!(net_message_id("sendNetDealAccepted"), Some(69));
        assert!(net_message_from_ui(69));
        assert!(net_message_from_ui(70));
        // fix-mp-trade: the deals between two humans.
        assert_eq!(net_message_id("sendNetDealProposed"), Some(71));
        assert_eq!(net_message_id("sendNetPlayerDealFinalized"), Some(72));
        assert!(net_message_from_ui(71));
        assert!(net_message_from_ui(72));
        assert!(!net_message_from_ui(73));
        assert!(!net_message_from_ui(-1));
    }

    #[test]
    fn net_messages_round_trip() {
        let mut m = NetMessage::new("sendPushOrder", &[7, 0, 12, 0, 1, 0]).unwrap();
        m.text = "Город".into();
        let rename = NetMessage::new("SendRenameCity", &[3]).unwrap();
        let bytes = encode_net(&[m.clone(), rename.clone()]);
        assert_eq!(&bytes[..4], b"GCN1");
        assert_eq!(decode_net(&bytes).unwrap(), vec![m.clone(), rename]);
        assert_eq!(m.name(), "sendPushOrder");
        assert!(NetMessage::new("sendPushOrder", &[0; 10]).is_none());
        // Truncated and trailing bytes are errors, not panics.
        assert!(decode_net(&bytes[..bytes.len() - 1]).is_err());
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(decode_net(&longer).is_err());
        assert!(decode_net(b"GCJ1\x01\x00\x00\x00\x00\x00\x00\x00").is_err());
    }

    #[test]
    fn journal_round_trips() {
        let recs = vec![
            JournalRecord::Command {
                player: 1,
                kind: 20,
                args: vec![5, 1, 33],
                text: None,
            },
            JournalRecord::Net {
                player: 0,
                message: NetMessage::new("sendResearch", &[12, 0, -1, 0]).unwrap(),
            },
            JournalRecord::Pump {
                slices: 3,
                status: 0,
                turn: 41,
            },
            JournalRecord::Slot {
                player: 2,
                op: ffi::GC_SLOT_KICK,
                nick: Some("bot".into()),
            },
            JournalRecord::Check {
                digest: 0x0123_4567_89ab_cdef,
            },
        ];
        let bytes = encode_journal(&recs);
        assert_eq!(decode_journal(&bytes).unwrap(), recs);
        assert_eq!(recs[2].turn_after(), Some(41));
        assert_eq!(recs[0].turn_after(), None);
        for cut in 0..bytes.len() {
            assert!(decode_journal(&bytes[..cut]).is_err(), "cut at {cut}");
        }
        let mut bad = bytes.clone();
        bad[12] = 9; // the first record's type
        assert!(decode_journal(&bad).is_err());
    }

    #[test]
    fn roles_map_to_the_abi() {
        assert_eq!(Role::Plain.abi(), (ffi::GC_ROLE_PLAIN, -1));
        assert_eq!(Role::Server.abi(), (ffi::GC_ROLE_SERVER, -1));
        assert_eq!(
            Role::Replica { seat: Some(3) }.abi(),
            (ffi::GC_ROLE_REPLICA, 3)
        );
        assert_eq!(
            Role::Replica { seat: None }.abi(),
            (ffi::GC_ROLE_REPLICA, -1)
        );
    }
}
