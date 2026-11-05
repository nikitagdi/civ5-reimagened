//! The state every part of the exe API shares: handlers, the event FIFO, the bridge to the replica, the counters.
//! The Lua objects (`Events`, `UI`, `Network`) hold it by `Weak`; `ExeApi` owns it.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::rc::Rc;

use lua_sys::{Context, Lua, LuaValue, MultiValue, RegistryKey, Table, ToLua, WeakRuntime};

use crate::buddy::Buddy;
use crate::entities::EntityTable;
use crate::{ExeConfig, ExeHost, Inner};
use crate::{GameSide, NetMessage, UiValue};

/// The three event namespaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Family {
    /// `Events.*`: the exe's events (`re-ui-events-static.tsv`), created by name here.
    Events,
    /// `LuaEvents.*`: created by name, synchronous.
    LuaEvents,
    /// `GameEvents.*`: the DLL's events; the UI side is a listener only.
    GameEvents,
}

impl Family {
    pub fn global(self) -> &'static str {
        match self {
            Family::Events => "Events",
            Family::LuaEvents => "LuaEvents",
            Family::GameEvents => "GameEvents",
        }
    }
}

/// An argument of an event made by Rust.
#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    Nil,
    Bool(bool),
    Num(f64),
    Str(String),
    /// A vector table: `x`, `y`, `z`, `w`.
    Vec(Vec<f64>),
    /// A table with named fields (`popupInfo`).
    Table(Vec<(String, Arg)>),
}

impl From<i32> for Arg {
    fn from(v: i32) -> Arg {
        Arg::Num(f64::from(v))
    }
}

impl From<bool> for Arg {
    fn from(v: bool) -> Arg {
        Arg::Bool(v)
    }
}

impl From<&str> for Arg {
    fn from(v: &str) -> Arg {
        Arg::Str(v.to_owned())
    }
}

impl From<&UiValue> for Arg {
    fn from(v: &UiValue) -> Arg {
        match v {
            UiValue::Nil => Arg::Nil,
            UiValue::Bool(b) => Arg::Bool(*b),
            UiValue::Num(n) => Arg::Num(*n),
            UiValue::Str(s) => Arg::Str(s.clone()),
            UiValue::Vec(v) => Arg::Vec(v.clone()),
        }
    }
}

impl Arg {
    pub fn to_lua<'l>(&self, lua: &'l Lua) -> lua_sys::Result<LuaValue<'l>> {
        Ok(match self {
            Arg::Nil => LuaValue::Nil,
            Arg::Bool(b) => LuaValue::Boolean(*b),
            Arg::Num(n) => LuaValue::Number(*n),
            Arg::Str(s) => s.as_str().to_lua(lua)?,
            Arg::Vec(v) => {
                let t = lua.create_table();
                for (name, n) in ["x", "y", "z", "w"].iter().zip(v) {
                    t.raw_set(*name, *n)?;
                }
                LuaValue::Table(t)
            }
            Arg::Table(fields) => {
                let t = lua.create_table();
                for (k, v) in fields {
                    t.raw_set(k.as_str(), v.to_lua(lua)?)?;
                }
                LuaValue::Table(t)
            }
        })
    }
}

/// The arguments of a fired event: made by Rust, or packed from Lua values (a table with `n`).
pub enum Payload {
    Args(Vec<Arg>),
    Lua(RegistryKey),
}

/// What the exe itself does when it hears one of its own events, before the Lua listeners (the popup queue).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hook {
    PopupProcessed(i32),
    PopupShown(i32),
    /// `Events.SerialEventExitCityScreen()` fired by a script: the exe puts the city screen down (WO-A, K8).
    ExitCityScreen,
    /// Events.ExitToMainMenu() / UserRequestClose() fired by a script: the player leaves for the lobby (WO-G, K20).
    ExitToMainMenu,
    /// The first announcement of a load is behind us in the FIFO (SIMP-exe-420): `Player:Cities()` and `:Units()` list the game again.
    SetupDone,
    /// `Events.ShowMovementRange(player, unit)` (WorldView.lua on a right press): the exe answers with
    /// `StartUnitMoveHexRange`, one `AddUnitMoveHexRangeHex` per plot the unit reaches this turn, `EndUnitMoveHexRange`.
    ShowMovementRange {
        player: i32,
        unit: i32,
    },
    /// `Events.ShowAttackTargets(player, unit)` (InGame.lua in the ranged attack mode): the same range events, the
    /// plots the unit can shoot at as attack plots only.
    ShowAttackTargets {
        player: i32,
        unit: i32,
    },
    /// `Events.DisplayMovementIndicator(show)`: the exe draws the path of the last `UI.SendPathfinderUpdate` or not.
    MovementIndicator(bool),
    /// `Events.AIProcessingStartedForPlayer` / `AIProcessingEndedForPlayer` of the server's turn queue: the AI civilization
    /// is turn-active (`active`) or done from now on (`Player:IsTurnActive` of the replica, SIMP-exe-820).
    AiTurnState {
        player: i32,
        active: bool,
    },
    /// `Events.SerialEventUnitFlagSelected(owner, id)` (a click on a unit flag, UnitFlagManager.lua): the exe selects
    /// the unit when it is the active player's.
    UnitFlagSelected(i32, i32),
    /// `Events.RequestYieldDisplay(type, a, b, c)`: the exe's `YieldIconManager` shows the yields of the asked plots
    /// (`yields.rs`).
    RequestYieldDisplay(i32, [i32; 3]),
    /// `Events.GameplayFX(hexX, hexY, type)` (event 143), fired by a script (the notification panel, the unit panel,
    /// the city banners) or by the core (`GameplayDoFX`: a notification of the seat with a plot, a ruin found among
    /// them): the exe's particle system plays the "look here" effect of the event at the plot (`GameplayFX` of
    /// `Civ5ArtDefines_EventEffects.xml` is `ART_DEF_VEFFECT_LOOK_AT`).
    GameplayFx {
        hx: i32,
        hy: i32,
    },
}

pub struct Fire {
    pub family: Family,
    pub name: String,
    pub payload: Payload,
    pub hook: Option<Hook>,
}

impl Fire {
    pub fn events(name: &str, args: Vec<Arg>) -> Fire {
        Fire {
            family: Family::Events,
            name: name.to_owned(),
            payload: Payload::Args(args),
            hook: None,
        }
    }
}

pub struct Handler {
    pub id: u64,
    pub key: RegistryKey,
    /// Name of the context that registered it (errors are blamed on it).
    pub ctx: String,
    /// The string key `Add(f, key)` gave.
    pub name_key: Option<String>,
    pub removed: Cell<bool>,
}

/// A notification the exe keeps for `UI.RebroadcastNotifications`.
#[derive(Debug, Clone, PartialEq)]
pub struct NoteRec {
    pub id: i32,
    pub kind: i32,
    pub tip: String,
    pub summary: String,
    pub game_data: i32,
    pub extra: i32,
    pub player: i32,
}

/// The temporary popup route of `TradeVisuals_NewRoute(-1, player, type, n, plots)`: the event carries the first 9
/// plots of the `n` only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TempRoute {
    pub player: i32,
    /// `TradeConnectionType`: 0 international, 1 food, 2 production.
    pub kind: i32,
    pub count: i32,
    pub plots: Vec<(i32, i32)>,
}

/// What the client keeps of the interface (mouse, keys, the flags of `UI.*`).
#[derive(Debug, Default)]
pub struct UiState {
    pub mouse_hex: (i32, i32),
    /// The plot the last `SerialEventMouseOverHex` was fired for (the exe fires it when the plot changes).
    pub mouse_hex_fired: Option<(i32, i32)>,
    /// The path of the last `UI.SendPathfinderUpdate` (the unit's plot first; empty: no path).
    pub path: Vec<crate::PathNode>,
    /// `Events.DisplayMovementIndicator`: the path is drawn.
    pub path_shown: bool,
    /// The route of the trade route popup shown now (`TradeVisuals_ActivatePopupRoute`), -1 for the temporary route.
    pub popup_route: Option<i32>,
    /// The temporary popup route the core made (`TradeVisuals_NewRoute(-1, ..)`), the caravan's target choice.
    pub temp_route: Option<TempRoute>,
    /// The plot `Game.SelectedUnit_SpeculativePopupTradeRoute_Display` was asked for last.
    pub popup_dest: Option<(i32, i32)>,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub goto_plot: (i32, i32),
    pub place_unit: i32,
    pub repeat_action_player: i32,
    pub turn_timer_semaphore: i32,
    pub leader_head_root_up: bool,
    pub city_screen_viewing: bool,
    pub grid_on: bool,
    pub sent_turn_complete: bool,
}

/// Counters of the report (`__civ5stats.exe.*`).
#[derive(Debug, Default)]
pub struct Counters {
    pub events: Cell<u64>,
    pub dispatch_us: Cell<u64>,
    pub frames: Cell<u64>,
    pub game_events_in: Cell<u64>,
    pub unmapped: Cell<u64>,
    pub attach_errors: Cell<u64>,
}

/// A timed piece of the UI side for the shell's timeline (`c.click`, `c.lua_reset`, `c.turn_events`); `dur_ms` is
/// negative for a mark. Times are on the clock given to [`crate::ExeApi::set_clock`].
#[derive(Debug, Clone, PartialEq)]
pub struct ExeSpan {
    pub name: &'static str,
    pub t0_ms: f64,
    pub dur_ms: f64,
    pub args: Vec<(&'static str, f64)>,
}

/// Spans kept until the shell takes them.
const SPANS_MAX: usize = 128;

pub(crate) type HandlerMap = HashMap<(Family, String), Vec<Rc<Handler>>>;

pub struct Core {
    pub(crate) inner: Rc<Inner>,
    pub cfg: ExeConfig,
    pub host: Rc<dyn ExeHost>,
    pub clock: RefCell<Option<Rc<dyn platform::Clock>>>,
    pub(crate) spans: RefCell<Vec<ExeSpan>>,
    pub(crate) handlers: RefCell<HandlerMap>,
    pub(crate) next_handler: Cell<u64>,
    pub(crate) queue: RefCell<VecDeque<Fire>>,
    pub(crate) draining: Cell<bool>,
    pub(crate) rt: RefCell<Option<WeakRuntime>>,
    pub(crate) ctxs: RefCell<HashMap<String, Context>>,
    pub game: RefCell<Option<Rc<RefCell<dyn GameSide>>>>,
    /// `GameEvents.<name>` the Lua side listens to; told to the game when it is bound.
    pub game_subscribed: RefCell<BTreeSet<String>>,
    pub entities: RefCell<EntityTable>,
    pub buddy: RefCell<Buddy>,
    pub notes: RefCell<BTreeMap<i32, NoteRec>>,
    /// Dirty bits since the last dispatch (`InterfaceDirtyBits`).
    pub dirty: RefCell<BTreeSet<i32>>,
    /// Units of the seat that were selected before (their first selection plays `FirstSelectionSound`).
    pub selected_before: RefCell<BTreeSet<(i32, i32)>>,
    /// Unit types of the seat whose selection sound files were asked for ahead, and those scripts.
    pub hinted_unit_types: RefCell<BTreeSet<i32>>,
    pub hinted_scripts: RefCell<BTreeSet<String>>,
    pub ui: RefCell<UiState>,
    pub counters: Counters,
    pub simp: RefCell<BTreeMap<String, u64>>,
    pub pending_net: RefCell<Vec<NetMessage>>,
    /// Units the seat renamed whose `UnitNameChanged` is still to be told ([`Core::tell_renamed`]).
    pub renamed_units: RefCell<Vec<i32>>,
    pub generation: Cell<Option<u64>>,
    /// Until this frame the Lua errors about dead objects of the previous load are expected (SIMP-exe-64).
    pub stale_until: Cell<u64>,
    /// Sounds of notifications, popups and events are held up to this frame (the join's catch-up, a load: SIMP-exe-702).
    pub sound_quiet_until: Cell<u64>,
    /// The frame the current catch-up began (0: the replica is live).
    pub sound_quiet_since: Cell<u64>,
    /// The replica's applied batches at the last sweep of dead units and cities ([`ExeApi::dispatch`]).
    pub applied_seen: Cell<u64>,
    pub error_filter_set: Cell<bool>,
    pub init_fired: Cell<bool>,
    /// The first announcement of the loaded units and cities (`setup_graphical`, the `rebuild` events) is done: until then the
    /// unit and city lists the glue hands out (`Player:Cities()`, `:Units()`) are empty (SIMP-exe-420).
    pub setup_done: Cell<bool>,
    /// Exits the exe itself did on a script's `SerialEventExitCityScreen`: the core's echo of each is not fired again.
    pub city_exit_echo: Cell<u32>,
    /// The game turn the UI saw at the last load of the game (WO-C: a loaded turn fires the turn events).
    pub turn_seen: Cell<Option<i32>>,
    /// Events.EndGameShow was raised for this game (WO-G, K19: once per loaded game that is over).
    pub end_game_shown: Cell<bool>,
    /// Frames since the last look at the game state for K19 (`check_game_over`).
    pub(crate) over_tick: Cell<u32>,
    /// The humans' `HasReceivedNetTurnComplete` the UI was last told of, as a bit mask (`track_turn_complete`).
    pub(crate) turn_mask: Cell<Option<u64>>,
    /// The seats on line and the human seats the UI was last told of, as bit masks (`track_seats`).
    pub(crate) seat_masks: Cell<Option<(u64, u64)>>,
    /// Seats that stopped being human (a kick gives the slot to the AI): `Network.IsPlayerKicked` (`track_seats`).
    pub(crate) kicked_mask: Cell<u64>,
    /// The host's seat and the ping figures the UI was last told of, and the frame of the last `MultiplayerPingTimesChanged`.
    pub(crate) host_seen: Cell<Option<i32>>,
    pub(crate) ping_seen: Cell<u64>,
    pub(crate) ping_frame: Cell<u64>,
    /// Ids of the notifications this client makes itself (a kick, a drop): they count down from here.
    pub(crate) note_serial: Cell<i32>,
    /// The humans whose `IsTurnActive` the UI was last told of, as a bit mask (`track_human_turns`).
    pub(crate) human_active: Cell<Option<u64>>,
    /// Auto end turn: the frame of the last unit move (0: none yet) and whether the seat gave a manual order this turn.
    pub(crate) unit_move_frame: Cell<u64>,
    /// The turn slice counter (`CvGame::getTurnSlice`), its frame-time accumulator (microseconds) and the clock at the
    /// last frame (`GameViewState::Update`'s `DAT_0bff8a08`).
    pub(crate) slice: Cell<u64>,
    /// The seats that are hot joining (`Network.IsPlayerHotJoining`), as a bit mask (`track_seats`).
    pub(crate) hot_joining: Cell<u64>,
    pub(crate) slice_acc_us: Cell<u64>,
    pub(crate) slice_last_us: Cell<Option<u64>>,
    pub(crate) has_moved_unit: Cell<bool>,
    pub(crate) end_counter: Cell<i32>,
    /// The active player at the last look (`track_turn_complete`), for `turn_all_complete`.
    pub(crate) turn_active: Cell<i32>,
    /// The AI progress the end-turn button shows (`track_ai_progress`) and the frames since it was set.
    pub(crate) ai_label: Cell<Option<(u32, u32)>>,
    pub(crate) ai_label_age: Cell<u32>,
    /// The turn and the number of events of the server's AI turn log the UI has been told of (`track_ai_turn`).
    pub(crate) ai_cursor: Cell<(i32, usize)>,
    /// The farthest an owned plot lay from an own city so far (`city_zoom_height`), and whether the camera moves.
    pub(crate) city_highwater: Cell<i32>,
    pub(crate) camera_moving: Cell<bool>,
    /// The replica's ends of turn not yet in the core's flag ([`crate::GameSide::turn_ended_by_journal`]) the Lua was
    /// last given (`__exe_turn_ended`).
    pub(crate) journal_ended: Cell<u64>,
    /// The "multiplayer auto end turn" option's wait (`auto_end.rs`).
    /// The player took the end of turn back; no auto end this turn (SIMP-exe-700).
    pub(crate) auto_end_latch: RefCell<crate::auto_end::AutoEndLatch>,
    /// The observer of some `Events.*` families (`ExeApi::set_event_tap`).
    pub tap: RefCell<Option<EventTap>>,
    /// The yield and resource icons of the map (`yields.rs`).
    pub map_icons: RefCell<crate::yields::MapIcons>,
    /// The active team's bridge state the host was last told of, and the frames since the last look (`bridge.rs`).
    pub(crate) bridge_seen: Cell<Option<(bool, bool)>>,
    pub(crate) bridge_tick: Cell<u32>,
}

/// The function an event tap calls with the name and the arguments of an event.
pub type TapFn = Rc<dyn Fn(&str, &[Arg])>;

/// An observer of the named `Events.*` families, called before the Lua listeners.
#[derive(Clone)]
pub struct EventTap {
    pub names: Vec<String>,
    pub f: TapFn,
}

impl Core {
    pub(crate) fn new(cfg: ExeConfig, host: Rc<dyn ExeHost>, inner: Rc<Inner>) -> Rc<Core> {
        let buddy = Buddy::new(&cfg);
        Rc::new(Core {
            inner,
            cfg,
            host,
            clock: RefCell::new(None),
            spans: RefCell::new(Vec::new()),
            handlers: RefCell::new(HashMap::new()),
            next_handler: Cell::new(1),
            queue: RefCell::new(VecDeque::new()),
            draining: Cell::new(false),
            rt: RefCell::new(None),
            ctxs: RefCell::new(HashMap::new()),
            game: RefCell::new(None),
            game_subscribed: RefCell::new(BTreeSet::new()),
            entities: RefCell::new(EntityTable::default()),
            buddy: RefCell::new(buddy),
            notes: RefCell::new(BTreeMap::new()),
            dirty: RefCell::new(BTreeSet::new()),
            selected_before: RefCell::new(BTreeSet::new()),
            hinted_unit_types: RefCell::new(BTreeSet::new()),
            hinted_scripts: RefCell::new(BTreeSet::new()),
            ui: RefCell::new(UiState::default()),
            counters: Counters::default(),
            simp: RefCell::new(BTreeMap::new()),
            pending_net: RefCell::new(Vec::new()),
            renamed_units: RefCell::new(Vec::new()),
            generation: Cell::new(None),
            stale_until: Cell::new(0),
            sound_quiet_until: Cell::new(0),
            sound_quiet_since: Cell::new(0),
            applied_seen: Cell::new(0),
            error_filter_set: Cell::new(false),
            init_fired: Cell::new(false),
            setup_done: Cell::new(false),
            city_exit_echo: Cell::new(0),
            turn_seen: Cell::new(None),
            end_game_shown: Cell::new(false),
            over_tick: Cell::new(0),
            turn_mask: Cell::new(None),
            seat_masks: Cell::new(None),
            kicked_mask: Cell::new(0),
            host_seen: Cell::new(None),
            ping_seen: Cell::new(0),
            ping_frame: Cell::new(0),
            note_serial: Cell::new(-1000),
            human_active: Cell::new(None),
            unit_move_frame: Cell::new(0),
            slice: Cell::new(1),
            hot_joining: Cell::new(0),
            slice_acc_us: Cell::new(0),
            slice_last_us: Cell::new(None),
            has_moved_unit: Cell::new(false),
            end_counter: Cell::new(3),
            turn_active: Cell::new(-1),
            ai_label: Cell::new(None),
            ai_label_age: Cell::new(0),
            ai_cursor: Cell::new((i32::MIN, 0)),
            city_highwater: Cell::new(1),
            camera_moving: Cell::new(false),
            journal_ended: Cell::new(0),
            auto_end_latch: RefCell::new(crate::auto_end::AutoEndLatch::default()),
            tap: RefCell::new(None),
            map_icons: RefCell::new(crate::yields::MapIcons::default()),
            bridge_seen: Cell::new(None),
            bridge_tick: Cell::new(0),
        })
    }

    /// Counts a call of a stub (`exe.simp_calls`).
    pub fn simp_call(&self, name: &str) {
        *self.simp.borrow_mut().entry(name.to_owned()).or_insert(0) += 1;
        self.inner.simp_calls.set(self.inner.simp_calls.get() + 1);
    }

    /// The clock in ms with the fraction; 0 until a clock is set.
    pub fn now_ms_f64(&self) -> f64 {
        self.clock.borrow().as_ref().map_or(0.0, |c| c.now_ms_f64())
    }

    /// Records a timed piece (`dur_ms` negative: a mark) for [`crate::ExeApi::take_spans`].
    pub(crate) fn span(
        &self,
        name: &'static str,
        t0_ms: f64,
        dur_ms: f64,
        args: Vec<(&'static str, f64)>,
    ) {
        if self.clock.borrow().is_none() {
            return;
        }
        let mut v = self.spans.borrow_mut();
        if v.len() >= SPANS_MAX {
            v.remove(0);
        }
        v.push(ExeSpan {
            name,
            t0_ms,
            dur_ms,
            args,
        });
    }

    pub fn now_us(&self) -> u64 {
        self.clock
            .borrow()
            .as_ref()
            .map_or(0, |c| (c.now_ms_f64() * 1000.0) as u64)
    }

    /// The Lua context handlers of `name` are called in (blame for their errors); `None` until the runtime is known.
    pub fn context_for(&self, name: &str) -> Option<Context> {
        if let Some(c) = self.ctxs.borrow().get(name) {
            return Some(c.clone());
        }
        let rt = self.rt.borrow().as_ref()?.upgrade()?;
        let c = rt.create_context(name, |_, _| Ok(())).ok()?;
        self.ctxs.borrow_mut().insert(name.to_owned(), c.clone());
        Some(c)
    }

    pub fn has_runtime(&self) -> bool {
        self.rt
            .borrow()
            .as_ref()
            .is_some_and(|w| w.upgrade().is_some())
    }

    /// The Lua state of the UI (through the internal context).
    pub fn lua_ctx(&self) -> Option<Context> {
        self.context_for("exe-api")
    }

    /// The game, if bound.
    pub fn game(&self) -> Option<Rc<RefCell<dyn GameSide>>> {
        self.game.borrow().clone()
    }

    /// A game operation; nothing (and `Ok`) when no game is bound.
    pub fn op(&self, op: crate::UiOp, args: &[i32]) -> Vec<i32> {
        let mut out = Vec::new();
        if let Some(g) = self.game() {
            let r = g.try_borrow_mut().map(|mut g| g.ui_op(op, args, &mut out));
            match r {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::warn!(target: "exe.ui", "{op:?}: {e}"),
                Err(_) => tracing::warn!(target: "exe.ui", "{op:?}: the game is busy"),
            }
        }
        out
    }

    /// The name of a `NotificationTypes` value (`NOTIFICATION_GOODY`), read from the enum the DLL registered.
    pub fn notification_type_name(&self, kind: i32) -> Option<String> {
        self.enum_name("NotificationTypes", kind)
    }

    /// The name of a `ButtonPopupTypes` value (`BUTTONPOPUP_NEW_ERA`), read from the enum the DLL registered.
    pub fn popup_type_name(&self, kind: i32) -> Option<String> {
        self.enum_name("ButtonPopupTypes", kind)
    }

    fn enum_name(&self, table: &str, value: i32) -> Option<String> {
        let ctx = self.lua_ctx()?;
        let f = ctx
            .lua()
            .load(
                "local t, v = ...
                 for k, n in pairs(_G[t] or {}) do if n == v then return k end end
                 return nil",
            )
            .set_name("=exe_enum_name")
            .into_function()
            .ok()?;
        match f.call::<_, LuaValue>((table, value)).ok()? {
            LuaValue::String(s) => Some(s.to_string_lossy().into_owned()),
            _ => None,
        }
    }

    /// Is the unit gone from the core (no `Players[player]:GetUnitByID(unit)`)? True only when the core says so: without
    /// a Lua context or a `Players` table the unit counts as there.
    pub fn unit_gone(&self, player: i32, unit: i32) -> bool {
        let Some(ctx) = self.lua_ctx() else {
            return false;
        };
        let Ok(f) = ctx
            .lua()
            .load(
                "local p, id = ...
                 local P = Players and Players[p]
                 return P ~= nil and P.GetUnitByID ~= nil and P:GetUnitByID(id) == nil",
            )
            .set_name("=exe_unit_gone")
            .into_function()
        else {
            return false;
        };
        f.call::<_, bool>((player, unit)).unwrap_or(false)
    }

    /// Is `player` the seat (`Game.GetActivePlayer()`)? The exe's sounds are the local player's own.
    pub fn is_seat_player(&self, player: i32) -> bool {
        let Some(ctx) = self.lua_ctx() else {
            return false;
        };
        let Ok(f) = ctx
            .lua()
            .load("return Game and Game.GetActivePlayer and Game.GetActivePlayer() == ...")
            .set_name("=exe_is_seat")
            .into_function()
        else {
            return false;
        };
        f.call::<_, bool>(player).unwrap_or(false)
    }

    /// The selection sound rows of a unit of the seat (`UnitGameplay2DScripts` of the unit's type). `None` when the unit is
    /// another player's or unknown.
    pub fn unit_selection_scripts(&self, player: i32, unit: i32) -> Option<UnitSoundRow> {
        let ctx = self.lua_ctx()?;
        let f = ctx
            .lua()
            .load(
                "local p, id = ...
                 if not (Game and Game.GetActivePlayer and Game.GetActivePlayer() == p) then return nil end
                 local P = Players and Players[p]
                 local u = P and P:GetUnitByID(id)
                 local info = u and GameInfo.Units[u:GetUnitType()]
                 if not info then return nil end
                 -- the DLL's ever-selected counter (CvUnit::IsFirstTimeSelected); older cores do not have the method
                 local first = nil
                 if u.IsFirstTimeSelected then first = u:IsFirstTimeSelected() and true or false end
                 for row in GameInfo.UnitGameplay2DScripts{ UnitType = info.Type } do
                     return true, first, row.SelectionSound, row.FirstSelectionSound
                 end
                 return false, first",
            )
            .set_name("=exe_unit_sounds")
            .into_function()
            .ok()?;
        let v = f.call::<_, MultiValue>((player, unit)).ok()?.into_vec();
        let text = |i: usize| match v.get(i) {
            Some(LuaValue::String(s)) => {
                Some(s.to_string_lossy().into_owned()).filter(|s| !s.is_empty())
            }
            _ => None,
        };
        let flag = |i: usize| match v.get(i) {
            Some(LuaValue::Boolean(b)) => Some(*b),
            _ => None,
        };
        Some(UnitSoundRow {
            has_row: flag(0)?,
            first_time: flag(1),
            selection: text(2),
            first_selection: text(3),
        })
    }

    /// The selection scripts (`SelectionSound`, `FirstSelectionSound`) of a unit type, by its id; `None` when the type or
    /// its row is unknown. For the sound files to be fetched before the first selection.
    pub fn unit_type_sound_scripts(
        &self,
        unit_type: i32,
    ) -> Option<(Option<String>, Option<String>)> {
        let ctx = self.lua_ctx()?;
        let f = ctx
            .lua()
            .load(
                "local t = ...
                 local info = GameInfo and GameInfo.Units and GameInfo.Units[t]
                 if not info then return nil end
                 for row in GameInfo.UnitGameplay2DScripts{ UnitType = info.Type } do
                     return row.SelectionSound, row.FirstSelectionSound
                 end
                 return nil",
            )
            .set_name("=exe_unit_type_sounds")
            .into_function()
            .ok()?;
        let v = f.call::<_, MultiValue>(unit_type).ok()?.into_vec();
        let text = |i: usize| match v.get(i) {
            Some(LuaValue::String(s)) => {
                Some(s.to_string_lossy().into_owned()).filter(|s| !s.is_empty())
            }
            _ => None,
        };
        let (sel, first) = (text(0), text(1));
        (sel.is_some() || first.is_some()).then_some((sel, first))
    }

    /// A unit of the seat of a type not seen before is on the map (created, or told by a load): its selection sound files
    /// are fetched now, so the first selection is heard (a file first asked for at the click comes late on a busy loader).
    pub fn hint_unit_sounds(&self, unit_type: i32) {
        if !self.hinted_unit_types.borrow_mut().insert(unit_type) {
            return;
        }
        let Some((sel, first)) = self.unit_type_sound_scripts(unit_type) else {
            return;
        };
        let mut hinted = self.hinted_scripts.borrow_mut();
        for script in [sel, first].into_iter().flatten() {
            if hinted.insert(script.clone()) {
                self.host.sound_preload(&script);
            }
        }
    }
    /// The looping ambience of the city on the city screen: `AS2D_AMBIENCE_CITY_ERA_<ERA>_<SMALL|LARGE>` of the owner's
    /// era and the city's size class (`population / 7`: small, medium, large; medium sounds as small, as in
    /// `Era_CitySoundscapes`). Post-modern sounds as modern (the game has no script for it, SIMP-exe-1000).
    pub fn city_ambience_script(&self) -> Option<String> {
        let ctx = self.lua_ctx()?;
        let &[owner, id, ..] = self.op(crate::UiOp::HeadSelectedCity, &[]).as_slice() else {
            return None;
        };
        if owner < 0 {
            return None;
        }
        let f = ctx
            .lua()
            .load(
                "local p, id = ...
                 local P = Players and Players[p]
                 local c = P and P:GetCityByID(id)
                 local era = P and GameInfo.Eras[P:GetCurrentEra()]
                 if not (c and era) then return nil end
                 return era.Type, c:GetPopulation()",
            )
            .set_name("=exe_city_ambience")
            .into_function()
            .ok()?;
        let v = f.call::<_, MultiValue>((owner, id)).ok()?.into_vec();
        let (LuaValue::String(era), LuaValue::Number(pop)) = (v.first()?, v.get(1)?) else {
            return None;
        };
        city_ambience_name(&era.to_string_lossy(), *pop as i32)
    }

    /// The sound of the head unit just selected: the first selection of a unit plays its `FirstSelectionSound`, later
    /// ones its `SelectionSound`.
    pub fn play_unit_selection(&self, player: i32, unit: i32) {
        let Some(row) = self.unit_selection_scripts(player, unit) else {
            return;
        };
        // The DLL's counter says whether this is the first selection; a core without the method is followed by our own
        // count of the units selected in this session.
        let counted = self.selected_before.borrow_mut().insert((player, unit));
        let first_time = row.first_time.unwrap_or(counted);
        let (chosen, other) = if first_time {
            (row.first_selection, row.selection)
        } else {
            (row.selection, row.first_selection)
        };
        // SIMPLIFIED(SIMP-exe-970): a unit type without a row makes the exe's default sound (AS2D_BUILD_UNIT); a row whose
        // column for this selection is empty plays the other column, then the default (the exe's handling of a NULL
        // column is not traced; the shipped data fills both).
        let script = chosen
            .or(other)
            .unwrap_or_else(|| DEFAULT_SELECTION_SOUND.to_owned());
        self.host.unit_selection_sound(&script);
    }

    /// The `MinorCivilizations` type of a city-state player (`MINOR_CIV_ALMATY`); `None` for a major or no player.
    pub fn minor_civ_type(&self, player: i32) -> Option<String> {
        let ctx = self.lua_ctx()?;
        let f = ctx
            .lua()
            .load(
                "local p = ...
                 local P = Players and Players[p]
                 if not (P and P.IsMinorCiv and P:IsMinorCiv()) then return nil end
                 local row = GameInfo.MinorCivilizations[P:GetMinorCivType()]
                 return row and row.Type",
            )
            .set_name("=exe_minor_type")
            .into_function()
            .ok()?;
        match f.call::<_, LuaValue>(player).ok()? {
            LuaValue::String(s) => Some(s.to_string_lossy().into_owned()).filter(|s| !s.is_empty()),
            _ => None,
        }
    }

    /// The spoken line of a popup (`OnButtonPopupDisplayShow`, re-b15 ITEM 5 P5.2) and its delay: the new era's line
    /// (`Era_NewEraVOs` of the era in `iData1`), the tech's (`Technologies.AudioIntro` of the tech in `iData3`,
    /// `AudioIntroHeader` in a multiplayer game: `GetSoundMP`), the wonder's quote (`Buildings.WonderSplashAudio` of
    /// `iData1`).
    ///
    /// SIMPLIFIED(SIMP-exe-1203): of an era's several lines the exe takes the one a game value (`vf[0x14]`, not named)
    /// picks modulo their count; here the game turn picks.
    pub fn popup_voice(&self, popup: &str, data: [i32; 3]) -> Option<(String, u32)> {
        use crate::sound_lines::{VoiceSource, voice_source};
        let (source, delay) = voice_source(popup)?;
        let ctx = self.lua_ctx()?;
        let (code, arg) = match source {
            VoiceSource::Era => (
                "local id = ...
                 local era = GameInfo.Eras[id]
                 if not era then return nil end
                 local lines = {}
                 for row in GameInfo.Era_NewEraVOs{ EraType = era.Type } do lines[#lines + 1] = row.VOScript end
                 if #lines == 0 then return nil end
                 local turn = (Game and Game.GetGameTurn and Game.GetGameTurn()) or 0
                 return lines[(turn % #lines) + 1]",
                data[0],
            ),
            VoiceSource::Tech => (
                "local id = ...
                 local t = GameInfo.Technologies[id]
                 if not t then return nil end
                 local mp = PreGame and PreGame.IsMultiplayerGame and PreGame.IsMultiplayerGame()
                 if mp then return t.AudioIntroHeader end
                 return t.AudioIntro",
                data[2],
            ),
            VoiceSource::Wonder => (
                "local id = ...
                 local b = GameInfo.Buildings[id]
                 return b and b.WonderSplashAudio",
                data[0],
            ),
        };
        let f = ctx
            .lua()
            .load(code)
            .set_name("=exe_popup_voice")
            .into_function()
            .ok()?;
        match f.call::<_, LuaValue>(arg).ok()? {
            LuaValue::String(s) => Some(s.to_string_lossy().into_owned())
                .filter(|s| !s.is_empty())
                .map(|s| (s, delay)),
            _ => None,
        }
    }

    /// The plot of `player`'s unit, read through the DLL's Lua API.
    pub fn unit_plot(&self, player: i32, unit: i32) -> Option<(i32, i32)> {
        let ctx = self.lua_ctx()?;
        let lua = ctx.lua();
        let f = lua
            .load(
                "local p, id = ...
                 local P = Players and Players[p]
                 local u = P and P:GetUnitByID(id)
                 if u then return u:GetX(), u:GetY() end
                 return nil",
            )
            .set_name("=exe_unit_plot")
            .into_function()
            .ok()?;
        match f.call::<_, MultiValue>((player, unit)).ok()?.into_vec()[..] {
            [LuaValue::Number(x), LuaValue::Number(y), ..] => Some((x as i32, y as i32)),
            _ => None,
        }
    }

    /// Whether `player`'s unit moves in the air (`DOMAIN_AIR`), read through the DLL's Lua API. False for a unit the
    /// Lua cannot tell (no game, an API without `GetDomainType`).
    pub fn unit_is_air(&self, lua: &Lua, player: i32, unit: i32) -> bool {
        let Ok(f) = lua
            .load(
                "local p, id = ...
                 local P = Players and Players[p]
                 local u = P and P:GetUnitByID(id)
                 if not (u and u.GetDomainType and DomainTypes) then return false end
                 return u:GetDomainType() == DomainTypes.DOMAIN_AIR",
            )
            .set_name("=exe_unit_air")
            .into_function()
        else {
            return false;
        };
        matches!(
            f.call::<_, MultiValue>((player, unit))
                .ok()
                .map(|v| v.into_vec()),
            Some(v) if matches!(v.first(), Some(LuaValue::Boolean(true)))
        )
    }

    /// `ShowMovementRange`: the range events of the exe for the unit (the unit's own plot is in the range):
    /// `ClearUnitMoveHexRange`, `StartUnitMoveHexRange`, `AddUnitMoveHexRangeHex(i, j, k, attackMove, unitID)` per plot the unit
    /// reaches this turn, `EndUnitMoveHexRange` (re-ui-live-answers item 3, 1.4 / 2.1).
    pub fn movement_range_fires(&self, player: i32, unit: i32) -> Vec<Fire> {
        let Some(g) = self.game() else {
            return Vec::new();
        };
        let reach = match g.try_borrow_mut().map(|mut g| g.unit_reach(player, unit)) {
            Ok(Ok(r)) => r,
            Ok(Err(e)) => {
                tracing::warn!(target: "exe.ui", "ShowMovementRange({player}, {unit}): {e}");
                return Vec::new();
            }
            Err(_) => {
                tracing::warn!(target: "exe.ui", "ShowMovementRange: the game is busy");
                return Vec::new();
            }
        };
        let mut cells: Vec<(i32, i32, bool)> = Vec::with_capacity(reach.len() + 1);
        if let Some((x, y)) = self.unit_plot(player, unit) {
            cells.push((x, y, false));
        }
        cells.extend(
            reach
                .iter()
                .map(|n| (i32::from(n.x), i32::from(n.y), n.flags.attack())),
        );
        range_fires(unit, &cells)
    }

    /// `ShowAttackTargets`: the range events for a unit in the ranged attack mode: the plots it can hit now, each an
    /// attack plot (`TEMPOnHexUnitChangedAttack`, `AttackPathAdd`: the hexes with a visible enemy only, so the unit's
    /// own plot and the plots it only walks to are not in it).
    // SIMPLIFIED(SIMP-exe-921): the plots are the core's `Q_RANGE_TARGETS` (what the range attack command accepts now:
    // the DLL's own range, line of sight and enemy test), not the exe's flood fill of the interface path finder with
    // `AttackPathAdd`, whose order and cut (this turn, turn 1) are not in the RE answers (re-ui-live-answers U1).
    pub fn attack_targets_fires(&self, player: i32, unit: i32) -> Vec<Fire> {
        let Some(g) = self.game() else {
            return Vec::new();
        };
        let targets = match g
            .try_borrow_mut()
            .map(|mut g| g.unit_range_targets(player, unit))
        {
            Ok(Ok(t)) => t,
            Ok(Err(e)) => {
                tracing::warn!(target: "exe.ui", "ShowAttackTargets({player}, {unit}): {e}");
                return Vec::new();
            }
            Err(_) => {
                tracing::warn!(target: "exe.ui", "ShowAttackTargets: the game is busy");
                return Vec::new();
            }
        };
        let cells: Vec<(i32, i32, bool)> = targets
            .iter()
            .map(|&(x, y)| (i32::from(x), i32::from(y), true))
            .collect();
        range_fires(unit, &cells)
    }
}

/// `ClearUnitMoveHexRange`, `StartUnitMoveHexRange`, one `AddUnitMoveHexRangeHex(i, j, k, attackMove, unitID)` per plot,
/// `EndUnitMoveHexRange`.
fn range_fires(unit: i32, cells: &[(i32, i32, bool)]) -> Vec<Fire> {
    let mut out = Vec::with_capacity(cells.len() + 3);
    out.push(Fire::events("ClearUnitMoveHexRange", vec![]));
    out.push(Fire::events("StartUnitMoveHexRange", vec![]));
    for &(x, y, attack) in cells {
        let (hx, hy) = crate::hex::grid_to_hex(x, y);
        out.push(Fire::events(
            "AddUnitMoveHexRangeHex",
            vec![
                Arg::from(hx),
                Arg::from(hy),
                Arg::from(-hx - hy),
                Arg::Bool(attack),
                Arg::from(unit),
            ],
        ));
    }
    out.push(Fire::events("EndUnitMoveHexRange", vec![]));
    out
}

/// How many frames a catch-up of the replica holds the sounds of notifications, popups and events (about ten seconds).
pub const SOUND_QUIET_MAX_FRAMES: u64 = 600;

/// The exe's sound for a unit without a `UnitGameplay2DScripts` row (`AS2D_BUILD_UNIT`).
pub const DEFAULT_SELECTION_SOUND: &str = "AS2D_BUILD_UNIT";

/// The `UnitGameplay2DScripts` row of a selected unit and the DLL's first-selection flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitSoundRow {
    /// The unit type has a row.
    pub has_row: bool,
    /// `CvUnit::IsFirstTimeSelected` (the DLL's counter); None when the core has no such Lua method.
    pub first_time: Option<bool>,
    pub selection: Option<String>,
    pub first_selection: Option<String>,
}

/// The city ambience script for an era type (`ERA_ANCIENT`) and a population.
///
/// SIMPLIFIED(SIMP-exe-1000): the exe builds the script name from the era's type (`StartCityScreenSound`) and the game
/// has scripts for the seven eras of the base game only, so the data has nothing for `ERA_POSTMODERN` of Brave New
/// World. The players say the original is not silent there (user decision 2026-11-01): the modern-era ambience plays.
pub fn city_ambience_name(era_type: &str, population: i32) -> Option<String> {
    let era = era_type.strip_prefix("ERA_").unwrap_or(era_type);
    let era = if era == "POSTMODERN" { "MODERN" } else { era };
    if !CITY_AMBIENCE_ERAS.contains(&era) {
        return None;
    }
    let size = if population / 7 >= 2 {
        "LARGE"
    } else {
        "SMALL"
    };
    Some(format!("AS2D_AMBIENCE_CITY_ERA_{era}_{size}"))
}

/// The eras that have `AS2D_AMBIENCE_CITY_ERA_<ERA>_<SMALL|LARGE>` scripts (`Audio2DScripts.xml`).
const CITY_AMBIENCE_ERAS: [&str; 7] = [
    "ANCIENT",
    "CLASSICAL",
    "MEDIEVAL",
    "RENAISSANCE",
    "INDUSTRIAL",
    "MODERN",
    "FUTURE",
];

/// `{ n = .., [1] = .. }` of Lua values, kept in the registry.
pub fn pack_lua(lua: &Lua, values: &[LuaValue<'_>]) -> lua_sys::Result<RegistryKey> {
    let t: Table = lua.create_table();
    for (i, v) in values.iter().enumerate() {
        t.raw_set(i + 1, v.clone())?;
    }
    t.raw_set("n", values.len())?;
    lua.create_registry_value(t)
}

pub fn unpack_lua<'l>(lua: &'l Lua, key: &RegistryKey) -> lua_sys::Result<Vec<LuaValue<'l>>> {
    let t: Table = lua.registry_value(key)?;
    let n: usize = t.raw_get("n")?;
    (1..=n).map(|i| t.raw_get::<_, LuaValue>(i)).collect()
}
