//! `Network.*`: the UI's messages to the server (`Network.Send*` is the exe's `gDLL->send*`) and a few getters.
//!
//! SIMPLIFIED(SIMP-exe-35): every `Network.Send<X>` takes the arguments of the matching `gDLL->send<X>` in the same
//! order (numbers and booleans as integers; the one string argument, a custom name, goes to the message text).
//! The table is derived from the names of the DLL's net messages (`NET_MESSAGE_NAMES`, `RtNetId`), not from the
//! bodies of `cvLuaNetworkLibrary` (`re-ui` O-EXE-API-LIBS); where the exe's Lua signature differs from the DLL's, the
//! difference shows as a message the server rejects.
//!
//! The turn handshake (`SendTurnComplete`, `SendTurnUnready`) goes out as the DLL's `sendTurnComplete` /
//! `sendTurnUnready`; the replica's router turns them into `EndTurn` / `CancelEndTurn`.

use std::rc::Rc;

use lua_sys::{Lua, LuaError, LuaValue, MultiValue, Result, Table};

use crate::NetMessage;
use crate::core::{Arg, Core};
use crate::registry::Status;

/// The names of the DLL's net messages (`gamecore::replica::NET_MESSAGE_NAMES`, append only; `gamecore` is
/// reached only through `replica` (R10), so the list is copied and a test keeps it equal to the core's ids).
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

/// `Network.Send<X>` for a DLL message name: `sendFoo`/`SendFoo` -> `SendFoo`, `IdeologyChoice` -> `SendIdeologyChoice`.
/// `None` for the messages the UI does not send by name.
pub fn lua_name(dll: &str) -> Option<String> {
    match dll {
        "None"
        | "SendAICivsProcessed"
        | "sendAutoMoves"
        | "sendNetDealAccepted"
        | "sendNetDemandAccepted"
        | "sendNetDealProposed"
        | "sendNetPlayerDealFinalized"
        | "sendPurchase_1"
        | "sendPurchase_2" => None,
        _ => {
            let rest = dll
                .strip_prefix("send")
                .or_else(|| dll.strip_prefix("Send"))
                .unwrap_or(dll);
            Some(format!("Send{rest}"))
        }
    }
}

/// All `(Network.<lua name>, gDLL name)` pairs.
pub fn send_table() -> Vec<(String, &'static str)> {
    NET_MESSAGE_NAMES
        .iter()
        .filter_map(|d| lua_name(d).map(|l| (l, *d)))
        .collect()
}

/// The Lua arguments (0-based positions) that the core's message does not carry: the sending player. The Lua of the
/// original passes the player first (`Network.SendMoveSpy(Game.GetActivePlayer(), ...)`, the parameters of
/// `gDLL->sendMoveSpy`), the core's `RtEngine::Send*` stores the arguments after it (`aiArg[0] = iSpyIndex`; the sender is
/// the seat) and `Deliver` reads them so (`h->ResponseMoveSpy(eP, A(0), ...)`). Not listed: the messages whose first
/// parameter is another player (a city-state, a rival) and the ones without a player.
/// SIMPLIFIED(SIMP-exe-331): the table is read off `rt_net.cpp` of the portable runtime, not off the exe's Lua library.
const DROPPED_ARG: &[(&str, usize)] = &[
    ("IdeologyChoice", 0),
    ("ArchaeologyChoice", 0),
    ("SendFoundPantheon", 0),
    ("SendFoundReligion", 0),
    ("SendEnhanceReligion", 0),
    ("SendMoveSpy", 0),
    ("SendStageCoup", 0),
    ("SendFaithPurchase", 0),
    ("sendLaunch", 0),
    ("sendAdvancedStartAction", 1),
    ("sendMinorPledgeProtection", 0),
    ("sendMinorGiftTileImprovement", 0),
    ("sendMinorBullyGold", 0),
    ("sendMinorBullyUnit", 0),
    ("sendMinorBuyout", 0),
    ("sendGreatPersonChoice", 0),
    ("sendMayaBonusChoice", 0),
    ("sendFaithGreatPersonChoice", 0),
    ("sendGoodyChoice", 0),
    ("sendSetSwappableGreatWork", 0),
    ("sendSwapGreatWorks", 0),
    ("sendMoveGreatWorks", 0),
    ("SendLeagueVoteEnact", 2),
    ("SendLeagueVoteRepeal", 2),
    ("SendLeagueVoteAbstain", 1),
    ("SendLeagueProposeEnact", 2),
    ("SendLeagueProposeRepeal", 2),
    ("SendLeagueEditName", 1),
];

/// The Lua argument (0-based position) that is the message's text (`m.strText` of `rt_net.cpp`, the `const char*` of
/// `gDLL->send*`). The Lua may pass nil there: ChooseReligionPopup.lua sends `customName = nil` when the religion keeps
/// its own name (`SendFoundReligion(player, religion, nil, b1..b4, x, y)`); the nil is an empty text, not a number.
const TEXT_ARG: &[(&str, usize)] = &[
    ("SendFoundReligion", 2),
    ("SendEnhanceReligion", 2),
    ("SendRenameCity", 1),
    ("SendRenameUnit", 1),
    ("SendLeagueEditName", 2),
];

/// A message from Lua arguments: numbers and booleans in order (without the sending player, [`DROPPED_ARG`]), the
/// string as the text (a nil at the text's place, [`TEXT_ARG`], is an empty text). The message is the core's
/// (`gamecore::replica::NetMessage`): its id is the index of `dll` in the table, the arguments past the given ones are 0.
pub fn message_from(dll: &str, args: &[LuaValue<'_>]) -> Result<NetMessage> {
    let dropped = DROPPED_ARG.iter().find(|(n, _)| *n == dll).map(|(_, i)| *i);
    let text_at = TEXT_ARG.iter().find(|(n, _)| *n == dll).map(|(_, i)| *i);
    let mut ints = Vec::with_capacity(args.len());
    let mut text = String::new();
    for (i, a) in args.iter().enumerate() {
        if dropped == Some(i) {
            continue;
        }
        if text_at == Some(i) && matches!(a, LuaValue::Nil) {
            continue;
        }
        match a {
            LuaValue::Nil => ints.push(0),
            LuaValue::Boolean(b) => ints.push(i32::from(*b)),
            LuaValue::Number(n) => ints.push(*n as i32),
            LuaValue::String(s) => {
                if text.is_empty() {
                    text = s.to_string_lossy().into_owned();
                }
            }
            other => {
                return Err(LuaError::msg(format!(
                    "Network: argument {} of {dll} is a {}",
                    i + 1,
                    other.type_name()
                )));
            }
        }
    }
    // The Lua of the original sends `Network.SendUpdatePolicies(policyOrBranch, isPolicy, true)`
    // (SocialPolicyPopup.lua), the DLL's message is `(isPolicy, policyOrBranch, value)` (`ResponseUpdatePolicies`).
    // SIMPLIFIED(SIMP-exe-480): the one message whose Lua argument order differs from `gDLL->send*` (found by M5 WO-J: a click on
    // the second policy sent policy 1, only a branch 0 unlock, whose two leading arguments are both 0, worked).
    if dll == "sendUpdatePolicies" && ints.len() >= 2 {
        ints.swap(0, 1);
    }
    let mut m = NetMessage::new(dll, &ints)
        .ok_or_else(|| LuaError::msg(format!("Network: {dll} has too many arguments")))?;
    m.text = text;
    Ok(m)
}

impl Core {
    /// Hands a message to the game side (the order of the UI's messages is kept); kept until the game is bound.
    pub fn post_net(&self, msg: NetMessage) {
        // The exe's SetSentTurnComplete / SendTurnUnready redraw the end-turn button at once ("Waiting for players");
        // the server's answer redraws it again (`track_turn_complete`).
        let sent = match msg.name() {
            "sendTurnComplete" => Some(true),
            "sendTurnUnready" => Some(false),
            _ => None,
        };
        // `setHasMovedUnit(true)` of CvUnitMission::PushMission for a manual order of the seat's unit (auto end turn, Part B).
        if matches!(
            msg.name(),
            "sendPushMission" | "sendAutoMission" | "sendSwapUnits"
        ) {
            self.has_moved_unit.set(true);
        }
        if sent == Some(true) {
            // The player's click on END TURN: the start of the turn's timeline (`c.click`).
            self.span("c.click", self.now_ms_f64(), -1.0, Vec::new());
        }
        if let Some(sent) = sent {
            let was = std::mem::replace(&mut self.ui.borrow_mut().sent_turn_complete, sent);
            if was && !sent {
                self.latch_auto_end();
            }
            if was != sent {
                self.fire_events("SerialEventEndTurnDirty", vec![]);
            }
        }
        let renamed = (msg.name() == "SendRenameUnit").then(|| msg.args[0]);
        if let Some(g) = self.game() {
            match g.try_borrow_mut() {
                Ok(mut g) => {
                    g.post_net(msg);
                    drop(g);
                    if let Some(id) = renamed {
                        // Told now (a replica that applies it at once) and again after the next batch.
                        self.renamed_units.borrow_mut().push(id);
                        self.tell_renamed();
                        self.renamed_units.borrow_mut().push(id);
                    }
                    return;
                }
                Err(_) => {
                    tracing::warn!(target: "exe.net", "the game is busy; {} kept", msg.name())
                }
            }
        }
        self.pending_net.borrow_mut().push(msg);
    }

    /// `Network.HasSentNetTurnAllComplete`: the exe's `m_sentTurnAllComplete`, set when every human of the game has ended
    /// the turn (NetTurnAllComplete goes out; the point after which the end of turn cannot be taken back, RB4-MP (b)).
    /// It was the seat's own end of turn, so ActionInfoPanel's setEndTurnWaiting disabled the "Waiting for players"
    /// button and a click could not take the end of turn back (live report 2026-10-31).
    ///
    /// SIMPLIFIED(SIMP-exe-633): read off the replica's `HasReceivedNetTurnComplete` of the human seats (the last frame's
    /// look, [`Core::track_turn_complete`]) and the UI's own end of turn, not set by a net message.
    ///
    /// SIMPLIFIED(SIMP-exe-740): the seat's own end of turn counts as soon as the UI sent it (the exe's own
    /// NetTurnComplete is "received" on its machine at once): the last human to end the turn is in the "turn is
    /// processed" state from the click, not from the server's echo, which comes after the AI phase when the core call
    /// is long. A refusal of that end of turn ([`crate::ExeApi::turn_complete_refused`]) takes it back.
    pub fn turn_all_complete(&self) -> bool {
        if !self.ui.borrow().sent_turn_complete {
            return false;
        }
        let (Some(mask), Some((_, humans))) = (self.turn_mask.get(), self.seat_masks.get()) else {
            return false;
        };
        let me = self.turn_active.get();
        let mine = if (0..64).contains(&me) { 1u64 << me } else { 0 };
        humans != 0 && (mask | mine) & humans == humans
    }

    /// Every human's end of turn is in the replica, the seat's own included (the server has them all: the turn is
    /// being processed and cannot be taken back, whatever the server answers to a later duplicate).
    pub fn turn_all_confirmed(&self) -> bool {
        let (Some(mask), Some((_, humans))) = (self.turn_mask.get(), self.seat_masks.get()) else {
            return false;
        };
        self.ui.borrow().sent_turn_complete && humans != 0 && mask & humans == humans
    }

    /// The messages kept before the game was bound go out.
    pub fn flush_net(&self) {
        let pending = std::mem::take(&mut *self.pending_net.borrow_mut());
        for m in pending {
            self.post_net(m);
        }
    }

    /// Once per frame (`ExeApi::dispatch`): the turn handshake of the network game as the UI sees it.
    ///
    /// - The router took the seat's end of turn back ahead of a gameplay message (the exe's `BroadcastGameplayMessage`):
    ///   `Network.HasSentNetTurnComplete` is false again and the end-turn button is told (`SerialEventEndTurnDirty`).
    /// - After a load the seat's end of turn is the loaded game's (a resync inside the turn keeps it), for the UI and the router.
    /// - A human's turn complete came or was taken back (`Player:HasReceivedNetTurnComplete`, which the replica sets when it
    ///   replays the server's `END_TURN` / `CANCEL_END_TURN`): the exe's `NetTurnComplete::Execute` / `NetTurnUnready::Execute`
    ///   (0x086fe92a / 0x086fed6c) raise `SerialEventEndTurnDirty` ("waiting for" of the end-turn button), and
    ///   `CvPlayerController::SetReceivedTurnComplete` (0x087599d4) raises `Events.RemotePlayerTurnEnd` through
    ///   `InterfaceBuddy::PublishRemotePlayerTurnEnd` (0x086af99e) for a human of a multiplayer game, the local one too
    ///   (RB4-MP item 1): the players panel (MPList), the turn queue at the bottom (MPTurnPanel) and EUI's civ list redraw.
    /// - A seat came, dropped or was kicked ([`Core::track_seats`]).
    ///
    /// SIMPLIFIED(SIMP-exe-570): the state is read off the replica every frame instead of from the net message, so one
    /// `RemotePlayerTurnEnd` stands for all the changes of a frame (the Lua re-reads every player).
    pub fn track_turn_complete(&self, lua: &Lua, loaded: bool) {
        let Some(game) = self.game() else {
            return;
        };
        let reopened = match game.try_borrow_mut() {
            Ok(mut g) => g.take_turn_reopened(),
            Err(_) => false,
        };
        if reopened {
            self.latch_auto_end();
            self.ui.borrow_mut().sent_turn_complete = false;
            self.fire_events("SerialEventEndTurnDirty", vec![]);
        }
        // SIMPLIFIED(SIMP-exe-742): a human whose END_TURN the journal applied counts as done
        // (`Player:HasReceivedNetTurnComplete`, wrapped at bind) before the core's pump sets its flag; for the last human
        // that pump is the AI phase, which reaches the replica only as the next turn's snapshot.
        let ended = game.try_borrow().map_or(0, |g| g.turn_ended_by_journal());
        if self.journal_ended.replace(ended) != ended {
            let list: Vec<String> = (0..64)
                .filter(|p| ended & (1u64 << p) != 0)
                .map(|p| format!("[{p}]=true"))
                .collect();
            if let Err(e) = lua
                .load(&format!("__exe_turn_ended = {{{}}}", list.join(",")))
                .set_name("=exe_turn_ended")
                .exec()
            {
                tracing::debug!(target: "exe.net", "the journal's ends of turn: {e}");
            }
        }
        let (mask, active, humans, flags) = match lua
            .load(TURN_COMPLETE_LUA)
            .set_name("=exe_turn_complete")
            .eval::<(f64, i32, f64, f64)>()
        {
            Ok(v) => v,
            Err(e) => {
                tracing::debug!(target: "exe.net", "the turn-complete probe failed: {e}");
                return;
            }
        };
        if mask < 0.0 {
            // Not a network game.
            self.turn_mask.set(None);
            self.seat_masks.set(None);
            return;
        }
        self.track_seats(lua, humans as u64);
        self.turn_active.set(active);
        self.track_human_turns(humans as u64, flags as u64);
        let mask = mask as u64;
        if loaded {
            let mine = (0..64).contains(&active) && mask & (1 << active) != 0;
            self.ui.borrow_mut().sent_turn_complete = mine;
            if let Ok(mut g) = game.try_borrow_mut() {
                g.set_turn_sent(mine);
            }
            self.turn_mask.set(Some(mask));
            self.lock_turn(&game);
            return;
        }
        let before = self.turn_mask.replace(Some(mask));
        if before.is_some_and(|b| b != mask) {
            self.fire_events("RemotePlayerTurnEnd", vec![]);
            self.fire_events("SerialEventEndTurnDirty", vec![]);
        }
        self.lock_turn(&game);
    }

    /// "The turn is being processed": every human has ended the turn ([`Core::turn_all_complete`]), so until the next
    /// load (the new turn's snapshot) the end of turn cannot be taken back. The router of the replica then sends a
    /// gameplay message without the `CancelEndTurn` ahead of it, as the exe's `BroadcastGameplayMessage` whose
    /// `SendTurnUnready` fails after NetTurnAllComplete (the server refuses the message while the AI moves). Without it
    /// a city or unit order given while the server ran the AI reopened the turn here, the END TURN button showed the
    /// old turn again, the auto end of turn sent END_TURN again, the server refused it, and the button flipped every
    /// half second until the new turn was loaded (live report 2026-10-31, game 936: 16 END_TURNs refused in 9 s).
    ///
    /// SIMPLIFIED(SIMP-exe-741): read off the replica and the UI's own end of turn once per frame, not set by the
    /// exe's NetTurnAllComplete message.
    fn lock_turn(&self, game: &std::cell::RefCell<dyn crate::GameSide>) {
        let locked = self.turn_all_complete();
        if let Ok(mut g) = game.try_borrow_mut() {
            g.set_turn_locked(locked);
        }
    }

    /// The server's AI turn queue (`AiTurnLog`): the original DLL activates every AI civilization when the new turn begins
    /// and deactivates each one when it has moved, and the exe turns each `setTurnActive` into the queued event
    /// `Events.AIProcessingStartedForPlayer(id, "")` / `AIProcessingEndedForPlayer(id, "")` (`InterfaceBuddy::
    /// PublishPlayerTurnStatus`, RB8-AITURN item 1). Lekmod's MPTurnPanel (the turn queue at the bottom) and EUI's
    /// NotificationPanel (the civ list) listen to them. The events the UI has not been told of go out in order, each after the
    /// overlay of `Player:IsTurnActive` flipped (a hook of the event, [`Hook::AiTurnState`]), so the handler reads the state
    /// the event is about.
    ///
    /// SIMPLIFIED(SIMP-exe-820): the replica does not play the AI phase; the server tells the AI civilizations its core
    /// started and ended ([`crate::AiTurnLog`]) and an overlay over the replica's `Player:IsTurnActive` (`__exe_ai_turn`)
    /// holds them until the next turn's state is loaded, which carries the real flags (all AIs done). The events arrive
    /// when the server's pump call that did them returns (calls of 4 slices, SIMP-exe-791), not at the instant of the
    /// core's `setTurnActive`; the humans' own start and end come from the replica as before, and the server's shuffled
    /// order of the start burst is not kept (ascending).
    pub(crate) fn track_ai_turn(&self, lua: &Lua, loaded: bool) {
        // A snapshot came in this frame: its flags are the truth (every AI is done); the overlay is gone.
        if loaded {
            self.set_ai_turn_state_with(lua, -1, None);
        }
        let Some(log) = self.host.ai_turn() else {
            return;
        };
        let (turn, seen) = self.ai_cursor.get();
        let seen = if turn == log.turn && seen <= log.events.len() {
            seen
        } else {
            0
        };
        if seen >= log.events.len() {
            return;
        }
        self.ai_cursor.set((log.turn, log.events.len()));
        // Events of a frame with a snapshot still go out so the turn queue and the civ list end in step, but they do not
        // set the overlay again: the loaded flags are the truth.
        for &(player, ended) in &log.events[seen..] {
            let name = if ended {
                "AIProcessingEndedForPlayer"
            } else {
                "AIProcessingStartedForPlayer"
            };
            let mut fire =
                crate::core::Fire::events(name, vec![Arg::from(player), Arg::Str(String::new())]);
            if !loaded {
                fire.hook = Some(crate::core::Hook::AiTurnState {
                    player,
                    active: !ended,
                });
            }
            self.fire(fire);
        }
    }

    /// `AIProcessingStartedForPlayer` / `AIProcessingEndedForPlayer` for the HUMAN seats (RB8-AITURN item 1: the exe raises
    /// them for every player at `setTurnActive`, human and remote included). The replica does not replay the turn change
    /// (a snapshot brings the new turn with the flags already set), so its core never calls `setTurnActive` for the humans
    /// at a turn's start; the client announces the change of `IsTurnActive` itself, and the end when every human has
    /// ended the turn. [`Core::announce_human`] keeps the core's own events (if any) from coming twice.
    ///
    /// SIMPLIFIED(SIMP-exe-911): read off the replica's flags once per frame, not raised at the instant of the core's
    /// `setTurnActive`; the order inside a frame is ascending by player.
    fn track_human_turns(&self, humans: u64, flags: u64) {
        let want = if self.turn_all_complete() {
            0
        } else {
            flags & humans
        };
        let Some(prev) = self.human_active.get() else {
            self.human_active.set(Some(want));
            return;
        };
        for p in 0..64 {
            let bit = 1u64 << p;
            if (want ^ prev) & bit != 0 {
                self.announce_human(p, want & bit != 0);
            }
        }
    }

    /// Raises the event of a human's `setTurnActive` unless the UI has been told that state already.
    pub(crate) fn announce_human(&self, p: i32, active: bool) -> bool {
        let bit = 1u64 << p;
        let prev = self.human_active.get().unwrap_or(0);
        if self.human_active.get().is_some() && (prev & bit != 0) == active {
            return false;
        }
        self.human_active
            .set(Some(if active { prev | bit } else { prev & !bit }));
        let name = if active {
            "AIProcessingStartedForPlayer"
        } else {
            "AIProcessingEndedForPlayer"
        };
        self.fire_events(name, vec![Arg::from(p), Arg::Str(String::new())]);
        true
    }

    /// The overlay of `Player:IsTurnActive` for an AI civilization (`Some`), or all of it gone (`None`, `player` ignored).
    pub(crate) fn set_ai_turn_state(&self, player: i32, active: Option<bool>) {
        if let Some(ctx) = self.lua_ctx() {
            self.set_ai_turn_state_with(ctx.lua(), player, active);
        }
    }

    fn set_ai_turn_state_with(&self, lua: &Lua, player: i32, active: Option<bool>) {
        let chunk = match active {
            Some(a) => format!(
                "if __exe_ai_turn == nil then __exe_ai_turn = {{}} end __exe_ai_turn[{player}] = {a}"
            ),
            None => "__exe_ai_turn = nil".to_owned(),
        };
        if let Err(e) = lua.load(&chunk).set_name("=exe_ai_turn").exec() {
            tracing::debug!(target: "exe.net", "the AI turn overlay: {e}");
        }
    }

    /// While the server runs the AI phase ("the turn is being processed", [`Core::turn_all_complete`]) the end-turn
    /// button says how far it is: "Processing AI turns: N of M" instead of "Please Wait". The original shows the
    /// AI's moves as they are made and no such text.
    ///
    /// SIMPLIFIED(SIMP-exe-790): the server does not send the AI's moves while it plays them (SIMP-058), so the wait is
    /// filled with a count of AI civilizations, N the ones the server's core has ended and M the ones it started this
    /// turn (the turn queue of [`Core::track_ai_turn`], so the count is the core's own, not an estimate; the
    /// city-states and barbarians are among them), put on the label `EndTurnText` of the original ActionInfoPanel after
    /// the frame's Lua has run; English only. When the progress ends the button's own text is drawn again
    /// (`SerialEventEndTurnDirty`).
    pub(crate) fn track_ai_progress(&self, lua: &Lua) {
        let want = if self.turn_all_complete() {
            self.host
                .ai_turn()
                .filter(|l| self.turn_seen.get() == Some(l.turn))
                .and_then(|l| ai_counts(&l))
        } else {
            None
        };
        let shown = self.ai_label.get();
        let Some((done, total)) = want else {
            if shown.is_some() {
                self.ai_label.set(None);
                self.fire_events("SerialEventEndTurnDirty", vec![]);
            }
            return;
        };
        // The original's own redraws (a human's end of turn, a notification) may put "Please Wait" back: the text is set
        // again every 15 frames besides when the numbers change.
        let age = self.ai_label_age.get().wrapping_add(1);
        self.ai_label_age.set(age);
        if shown == want && !age.is_multiple_of(15) {
            return;
        }
        self.ai_label.set(want);
        let text = ai_progress_text(done, total);
        let chunk = format!(
            "local c = __exe_ai_lbl              if not c or not pcall(c.GetText, c) then c = __ui_find_label('/EndTurnText') __exe_ai_lbl = c end              if c then c:SetText('{text}') end"
        );
        if let Err(e) = lua.load(&chunk).set_name("=exe_ai_progress").exec() {
            tracing::debug!(target: "exe.net", "the AI progress text: {e}");
        }
    }

    /// Once per frame of a network game: the seats on line (`Network.IsPlayerConnected`, the lobby's view) and the human
    /// seats of the game (`IsHuman() and not IsObserver()`, the replica's). A seat that drops off line or stops being a
    /// human (a kick gives it to the AI) raises `Events.MultiplayerGamePlayerDisconnected(player)`, a frame with any change
    /// `Events.MultiplayerGamePlayerUpdated()`: MPList, MPTurnPanel and EUI's civ list rebuild on them (RB4-MP2 item 5:
    /// `NetProxy::PlayerLeft` 0x08735830 raises both when a connection closes, a kick included; the DLL's
    /// `CvPlayer::disconnected` gives the slot to the AI; the kicked seat stayed in the list until a reload).
    ///
    /// SIMPLIFIED(SIMP-exe-611): read off the lobby's seat list and the replica once per frame, not raised by a net layer's
    /// connection callback; the first look after a game is bound raises nothing.
    pub(crate) fn track_seats(&self, lua: &Lua, humans: u64) {
        let info = self.host.seat_info();
        let mut online = 0u64;
        let mut ping_sig = 0u64;
        for (p, ms) in &info.pings_ms {
            if (0..64).contains(p) {
                online |= 1 << p;
                ping_sig = ping_sig
                    .wrapping_mul(1_000_003)
                    .wrapping_add((*p as u64) << 32 | (*ms as u64 & 0xffff_ffff));
            }
        }
        let Some((was_online, was_human)) = self.seat_masks.replace(Some((online, humans))) else {
            self.host_seen.set(Some(info.host_id));
            self.ping_seen.set(ping_sig);
            self.ping_frame.set(self.counters.frames.get());
            return;
        };
        let me = info.local_id;
        let mine = if (0..64).contains(&me) { 1u64 << me } else { 0 };
        let gone = (was_online & !online) | (was_human & !humans);
        let changed = (was_online ^ online) | (was_human ^ humans);
        // A seat that stopped being a human went to the AI: a kick (`Network.IsPlayerKicked`, CvPlayer::disconnected's
        // NOTIFICATION_PLAYER_KICKED); one that only dropped off line is a disconnect.
        let kicked = was_human & !humans;
        self.kicked_mask
            .set((self.kicked_mask.get() | kicked) & !humans);
        for p in (0..64).filter(|p| gone & (1 << p) != 0) {
            self.fire_events("MultiplayerGamePlayerDisconnected", vec![Arg::from(p)]);
        }
        // `CvPlayer::disconnected` adds the notification to the ACTIVE player of the machine; the replica's core adds it to
        // the server seat (its active player during a replay), so the client makes it (SIMP-exe-910).
        for p in (0..64).filter(|p| gone & (1 << p) != 0 && mine & (1 << p) == 0) {
            if was_human & (1 << p) != 0 {
                self.player_gone_note(lua, p, kicked & (1 << p) != 0);
            }
        }
        if changed != 0 {
            // `NullEventClass`: no arguments (RB4-MP2 item 5, event 146).
            self.fire_events("MultiplayerGamePlayerUpdated", vec![]);
        }
        // Hot join (RB10 item 4b): `NetHotJoinStart::Execute` -> `NetProxy::OnHotJoinStart` raises
        // `Events.MultiplayerHotJoinStarted()` (151) on every peer, `NetHotJoinCompleted::Execute` ->
        // `OnHotJoinComplete` raises `MultiplayerHotJoinCompleted()` (152); neither has arguments, and between them
        // `Network.IsPlayerHotJoining(pid)` is true. A seat comes on line as a human of a game in progress: start;
        // the next look at the seats (the replica has taken it up): complete.
        //
        // SIMPLIFIED(SIMP-exe-1023): read off the lobby's seat list and the replica once per frame, not raised by the
        // host's NetHotJoinStart/NetHotJoinCompleted messages (there is no host state machine here: the server
        // snapshots the joiner); the joiner's own machine raises nothing (it enters the game by the snapshot, not
        // through a staging room), and the original's repeated 151 per joiner (one per NetHotJoinStart) is one event.
        let busy = self.hot_joining.get();
        let new_ones =
            (((online & !was_online) & humans) | ((humans & !was_human) & online)) & !mine & !busy;
        let done = busy & !new_ones;
        if done != 0 {
            self.hot_joining.set(busy & !done);
            self.fire_events("MultiplayerHotJoinCompleted", vec![]);
        }
        if new_ones != 0 {
            self.hot_joining.set(self.hot_joining.get() | new_ones);
            self.fire_events("MultiplayerHotJoinStarted", vec![]);
        }
        // `NetKicked::Execute` on the kicked machine: the game is abandoned with reason 2 (NetKicked.BY_HOST); WorldView.lua
        // opens BUTTONPOPUP_KICKED (re-mp2 item 5 section 3).
        if kicked & mine != 0 {
            self.fire_events("MultiplayerGameAbandoned", vec![Arg::from(2)]);
        }
        // `FNetSession::ConnectionClosed`: a machine whose player count drops to one or less (itself) raises
        // `MultiplayerGameLastPlayer`; WorldView.lua opens the "last player in game" popup.
        if was_online & !online != 0 && online.count_ones() == 0 && kicked & mine == 0 {
            self.fire_events("MultiplayerGameLastPlayer", vec![]);
        }
        // `NetProxy::HostMigrated`: the host's seat changed.
        if self.host_seen.replace(Some(info.host_id)) != Some(info.host_id) && info.host_id >= 0 {
            self.fire_events(
                "MultiplayerGameHostMigration",
                vec![Arg::from(info.host_id)],
            );
        }
        // `MultiplayerPingTimesChanged`: the ping figures moved (at most every two seconds of frames).
        let frames = self.counters.frames.get();
        if self.ping_seen.get() != ping_sig
            && frames.saturating_sub(self.ping_frame.get()) >= PING_EVENT_FRAMES
        {
            self.ping_seen.set(ping_sig);
            self.ping_frame.set(frames);
            self.fire_events("MultiplayerPingTimesChanged", vec![]);
        }
    }

    /// The notification `CvPlayer::disconnected` adds for the active player: TXT_KEY_PLAYER_KICKED or
    /// TXT_KEY_PLAYER_DISCONNECTED with the player's name, type NOTIFICATION_PLAYER_KICKED / _DISCONNECTED.
    ///
    /// SIMPLIFIED(SIMP-exe-910): made by the client when it sees the seat leave (the replica's core adds it to the server
    /// seat, not to this machine's seat), with an id of its own; it is not in the core's list, so a reload drops it.
    fn player_gone_note(&self, lua: &Lua, p: i32, kicked: bool) {
        let (key, ty) = if kicked {
            ("TXT_KEY_PLAYER_KICKED", "NOTIFICATION_PLAYER_KICKED")
        } else {
            (
                "TXT_KEY_PLAYER_DISCONNECTED",
                "NOTIFICATION_PLAYER_DISCONNECTED",
            )
        };
        let chunk = format!(
            "local p = Players[{p}]              local ok, name = pcall(function() return Locale.ConvertTextKey(p:GetNameKey()) end)              if not ok or not name then name = 'Player {p}' end              local t = NotificationTypes and NotificationTypes.{ty}              if not t then return nil end              return Locale.ConvertTextKey('{key}', name), t"
        );
        let Ok((text, kind)) = lua
            .load(&chunk)
            .set_name("=exe_player_gone")
            .eval::<(String, i32)>()
        else {
            tracing::debug!(target: "exe.net", "no notification for the seat {p} that left");
            return;
        };
        let id = self.note_serial.get();
        self.note_serial.set(id - 1);
        let rec = crate::core::NoteRec {
            id,
            kind,
            tip: text.clone(),
            summary: text,
            game_data: p,
            extra: -1,
            player: self.host.seat_info().local_id,
        };
        self.notes.borrow_mut().insert(rec.id, rec.clone());
        self.fire_note(&rec);
    }
}

/// Frames between two `MultiplayerPingTimesChanged` (about two seconds).
const PING_EVENT_FRAMES: u64 = 120;

/// `(ended, started)` civilizations of an AI turn log; `None` before the first one is started.
pub(crate) fn ai_counts(log: &crate::AiTurnLog) -> Option<(u32, u32)> {
    let mut started = std::collections::BTreeSet::new();
    let mut ended = std::collections::BTreeSet::new();
    for &(p, e) in &log.events {
        if e {
            ended.insert(p);
        } else {
            started.insert(p);
        }
    }
    // A civilization the client heard end without its start (it joined in the middle of the phase) counts in both.
    let total = started.union(&ended).count();
    (total > 0).then_some((ended.len() as u32, total as u32))
}

/// The button text of the AI phase.
pub(crate) fn ai_progress_text(done: u32, total: u32) -> String {
    format!("Processing AI turns: {done} of {total}")
}

/// The humans' `HasReceivedNetTurnComplete` as a bit mask (bit = player), -1 outside a network game; the active player; the
/// human seats (not observers) as a bit mask.
const TURN_COMPLETE_LUA: &str = "if not Game.IsNetworkMultiPlayer() then return -1, Game.GetActivePlayer(), 0, 0 end \
local m, h, a = 0, 0, 0 \
for i = 0, GameDefines.MAX_MAJOR_CIVS - 1 do \
local p = Players[i] \
if p and p:IsHuman() then \
if p:HasReceivedNetTurnComplete() then m = m + 2 ^ i end \
if p.IsTurnActive and p:IsTurnActive() then a = a + 2 ^ i end \
if not (p.IsObserver and p:IsObserver()) then h = h + 2 ^ i end \
end \
end \
return m, Game.GetActivePlayer(), h, a";

/// The getters of `Network` outside K1-K3 (SIMP-exe-38): name and the Lua literal they answer.
pub const NET_STUBS: &[(&str, &str)] = &[
    ("GetLocalTurnSliceInfo", "nil"),
    ("GetPlayerTurnSliceInfo", "nil"),
    ("GetTurnSliceMaxMessageCount", "0"),
    ("SetTurnSliceMaxMessageCount", "nil"),
    ("SetDebugLogLevel", "nil"),
];

/// Puts `Network` into the base globals.
pub fn install(core: &Rc<Core>, lua: &Lua, g: &Table<'_>) -> Result<()> {
    let net = lua.create_table();
    for (name, dll) in send_table() {
        let weak = Rc::downgrade(core);
        net.raw_set(
            name.as_str(),
            lua.create_function(move |_, args: MultiValue| {
                let core = weak
                    .upgrade()
                    .ok_or_else(|| LuaError::msg("the exe API is gone"))?;
                // `Network.SendTurnUnready` answers whether the end of turn was taken back (cvLuaNetworkLibrary
                // 0x08960f2c -> CvGameController::SendTurnUnready): false once every human has ended it (the
                // exe's NetTurnAllComplete is out). ActionInfoPanel redraws the button only on true.
                if dll == "sendTurnUnready" && core.turn_all_complete() {
                    return Ok(false);
                }
                let msg = message_from(dll, &args.into_vec())?;
                core.post_net(msg);
                Ok(true)
            }),
        )?;
    }
    // Getters.
    let w = Rc::downgrade(core);
    net.raw_set(
        "HasSentNetTurnComplete",
        lua.create_function(move |_, _: MultiValue| {
            Ok(w.upgrade()
                .is_some_and(|c| c.ui.borrow().sent_turn_complete))
        }),
    )?;
    let w = Rc::downgrade(core);
    net.raw_set(
        "HasSentNetTurnAllComplete",
        lua.create_function(move |_, _: MultiValue| {
            Ok(w.upgrade().is_some_and(|c| c.turn_all_complete()))
        }),
    )?;
    let w = Rc::downgrade(core);
    net.raw_set(
        "IsPlayerConnected",
        lua.create_function(move |_, id: Option<i32>| {
            Ok(w.upgrade()
                .is_some_and(|c| c.host.seat_info().connected(id.unwrap_or(-1))))
        }),
    )?;
    let w = Rc::downgrade(core);
    net.raw_set(
        "GetPingTime",
        lua.create_function(move |_, id: Option<i32>| {
            Ok(w.upgrade()
                .map_or(0, |c| c.host.seat_info().ping(id.unwrap_or(-1))))
        }),
    )?;
    let w = Rc::downgrade(core);
    net.raw_set(
        "IsPlayerHotJoining",
        lua.create_function(move |_, id: Option<i32>| {
            Ok(w.upgrade().is_some_and(|c| {
                let id = id.unwrap_or(-1);
                (0..64).contains(&id) && c.hot_joining.get() & (1u64 << id) != 0
            }))
        }),
    )?;
    // `Network.IsPlayerKicked`: the seat stopped being a human (the host's kick gave it to the AI).
    let w = Rc::downgrade(core);
    net.raw_set(
        "IsPlayerKicked",
        lua.create_function(move |_, id: Option<i32>| {
            Ok(w.upgrade().is_some_and(|c| {
                let id = id.unwrap_or(-1);
                (0..64).contains(&id) && c.kicked_mask.get() & (1u64 << id) != 0
            }))
        }),
    )?;
    g.raw_set("Network", net)?;
    let mut code = String::from(
        "local function stub(name, ret) Network[name] = function() __exe_simp('Network.' .. name) return ret end end\n",
    );
    for (name, ret) in NET_STUBS {
        code.push_str(&format!("stub('{name}', {ret})\n"));
    }
    let net: Table = g.raw_get("Network")?;
    {
        let mut reg = core.inner.registry.borrow_mut();
        reg.add("Network", Status::Real);
        for pair in net.pairs::<String, LuaValue>() {
            let (name, _) = pair?;
            reg.add(format!("Network.{name}"), Status::Real);
        }
        for (name, _) in NET_STUBS {
            reg.add(format!("Network.{name}"), Status::Simp("SIMP-exe-38"));
        }
    }
    lua.load(&code)
        .set_name("=network")
        .set_environment(g.clone())
        .exec()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The copied table is the core's: every name builds a message whose id is its index.
    /// Every name of the table is a message of the core.
    /// ChooseReligionPopup.lua sends `customName = nil` when the religion keeps its own name:
    /// `Network.SendFoundReligion(player, religion, nil, b1, b2, b3, b4, x, y)`. Counted as a number, the nil moved
    /// the beliefs and the city one place on, the core found no city at (b4, x) and dropped the message without a
    /// word (live game 931, turn 57: the founding popup closed, the block stayed).
    #[test]
    fn a_nil_name_is_empty_text_not_an_argument() {
        let n = LuaValue::Number;
        let m = message_from(
            "SendFoundReligion",
            &[
                n(1.0),
                n(4.0),
                LuaValue::Nil,
                n(10.0),
                n(20.0),
                n(-1.0),
                n(-1.0),
                n(33.0),
                n(17.0),
            ],
        )
        .unwrap();
        assert_eq!(&m.args[..7], &[4, 10, 20, -1, -1, 33, 17]);
        assert!(m.text.is_empty());
        // SendEnhanceReligion(player, religion, nil, follower, enhancer, x, y)
        let m = message_from(
            "SendEnhanceReligion",
            &[
                n(1.0),
                n(4.0),
                LuaValue::Nil,
                n(30.0),
                n(40.0),
                n(33.0),
                n(17.0),
            ],
        )
        .unwrap();
        assert_eq!(&m.args[..5], &[4, 30, 40, 33, 17]);
        // A string at the text's place is the text, as before.
        let name = Lua::new().unwrap();
        let s: LuaValue = name.load("return 'Tengri'").eval().unwrap();
        let m = message_from(
            "SendFoundReligion",
            &[
                n(1.0),
                n(4.0),
                s,
                n(10.0),
                n(20.0),
                n(-1.0),
                n(-1.0),
                n(33.0),
                n(17.0),
            ],
        )
        .unwrap();
        assert_eq!(&m.args[..7], &[4, 10, 20, -1, -1, 33, 17]);
        assert_eq!(m.text, "Tengri");
    }

    #[test]
    fn the_text_arguments_name_messages() {
        for (n, _) in TEXT_ARG {
            assert!(NET_MESSAGE_NAMES.contains(n), "{n}");
        }
    }

    #[test]
    fn the_dropped_player_arguments_name_messages() {
        for (n, _) in DROPPED_ARG {
            assert!(NET_MESSAGE_NAMES.contains(n), "{n}");
        }
    }

    #[test]
    fn the_sender_is_not_an_argument() {
        fn a(v: &[f64]) -> Vec<LuaValue<'static>> {
            v.iter().map(|x| LuaValue::Number(*x)).collect()
        }
        // SendMoveSpy(player, spy, targetPlayer, targetCity, asDiplomat)
        let m = message_from("SendMoveSpy", &a(&[0.0, 1.0, 2.0, 8192.0, 0.0])).unwrap();
        assert_eq!(&m.args[..4], &[1, 2, 8192, 0]);
        // SendLeagueVoteEnact(league, resolution, voter, votes, choice)
        let m = message_from("SendLeagueVoteEnact", &a(&[0.0, 7.0, 3.0, 2.0, 5.0])).unwrap();
        assert_eq!(&m.args[..4], &[0, 7, 2, 5]);
        // SendUpdatePolicies(policy, isPolicy, true): the Lua order of SocialPolicyPopup.lua; the core's message is (isPolicy, policy, value).
        let m = message_from(
            "sendUpdatePolicies",
            &[
                LuaValue::Number(7.0),
                LuaValue::Boolean(true),
                LuaValue::Boolean(true),
            ],
        )
        .unwrap();
        assert_eq!(&m.args[..3], &[1, 7, 1]);
        let m = message_from(
            "sendUpdatePolicies",
            &[
                LuaValue::Number(3.0),
                LuaValue::Boolean(false),
                LuaValue::Boolean(true),
            ],
        )
        .unwrap();
        assert_eq!(&m.args[..3], &[0, 3, 1]);
        // A message with a rival as the first argument keeps it.
        let m = message_from("sendMinorGiftGold", &a(&[11.0, 40.0])).unwrap();
        assert_eq!(&m.args[..2], &[11, 40]);
    }

    #[test]
    fn the_names_are_the_cores_ids() {
        for (i, name) in NET_MESSAGE_NAMES.iter().enumerate() {
            let m = NetMessage::new(name, &[]).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(m.id, i as i32, "{name}");
            assert_eq!(m.name(), *name);
        }
        assert!(NetMessage::new("sendNoSuchThing", &[]).is_none());
    }
}
