//! A Lua runtime with the exe API over a fake game side (`FakeGame`, a `replica::GameSide` with the core's types).
#![allow(dead_code)]

use std::cell::RefCell;
use std::rc::Rc;

use exe_api::{
    ExeApi, ExeConfig, ExeHost, GameSide, NetMessage, PathNode, SeatInfo, UiEvent, UiEventKind,
    UiOp, UiValue,
};
use lua_sys::{Context, MemorySource, Runtime, RuntimeConfig};
use platform::{KeyValueStore, MemoryStore};

/// An event with integer arguments (the layouts of `gamecore_abi.h`).
pub fn ev(kind: UiEventKind, args: &[i32]) -> UiEvent {
    UiEvent::new(kind, args.to_vec())
}

/// A unit block (`GC_UI_UNIT_INTS`): owner, id, x, y, type 7, culture 1, civ 2, colours 3 4, flag index 5, fog 0 (seen),
/// not selected, military, visible.
pub fn unit_block(owner: i32, id: i32, x: i32, y: i32) -> Vec<i32> {
    vec![owner, id, x, y, 7, 1, 2, 3, 4, 5, 0, 0, 1, 1]
}

/// A city block (`GC_UI_CITY_INTS`): owner, id, x, y, population, damage 0, max hp 200, size 1, team = owner,
/// not a puppet, not a capital.
pub fn city_block(owner: i32, id: i32, x: i32, y: i32, pop: i32) -> Vec<i32> {
    vec![owner, id, x, y, pop, 0, 200, 1, owner, 0, 0]
}

/// A kind whose arguments are a block and then `rest`.
pub fn with_block(kind: UiEventKind, block: Vec<i32>, rest: &[i32]) -> UiEvent {
    let mut args = block;
    args.extend_from_slice(rest);
    UiEvent::new(kind, args)
}

/// The builders the core's `UiEvent` does not have.
pub trait EvExt {
    fn with_text(self, text: &str) -> Self;
    fn with_text2(self, text: &str) -> Self;
    fn with_values(self, values: Vec<UiValue>) -> Self;
    fn rebuilt(self) -> Self;
}

impl EvExt for UiEvent {
    fn with_text(mut self, text: &str) -> Self {
        self.text = Some(text.to_owned());
        self
    }
    fn with_text2(mut self, text: &str) -> Self {
        self.text2 = Some(text.to_owned());
        self
    }
    fn with_values(mut self, values: Vec<UiValue>) -> Self {
        self.values = values;
        self
    }
    fn rebuilt(mut self) -> Self {
        self.rebuild = true;
        self
    }
}

/// A game side for tests: events are scripted, the interface state is a few fields and answers like the core's
/// `RtUiReplica` (selection and city screen changes come back as events).
#[derive(Default)]
pub struct FakeGame {
    pub events: Vec<UiEvent>,
    /// What `setup_graphical` queues (the world as the fake keeps it).
    pub graphical: Vec<UiEvent>,
    pub generation: u64,
    pub posted: Vec<NetMessage>,
    pub subscribed: Vec<String>,
    pub ops: Vec<(UiOp, Vec<i32>)>,
    pub attached: u32,
    pub invalidated: u32,
    pub graphical_calls: u32,
    /// Lua run on `attach_lua` (the stand-in of the DLL's libraries).
    pub attach_script: String,
    pub head_unit: Option<(i32, i32)>,
    pub head_city: Option<(i32, i32)>,
    pub mode: i32,
    pub mode_value: i32,
    pub city_screen: bool,
    pub can_end_turn: bool,
    pub cycle_counter: i32,
    /// What `unit_path` answers (the asked target is recorded in `path_asked`).
    pub path: Vec<PathNode>,
    pub path_asked: Vec<(i32, i32, i32, i32)>,
    /// What `unit_reach` answers.
    pub reach: Vec<PathNode>,
    /// What `unit_range_targets` answers.
    pub targets: Vec<(i16, i16)>,
    /// What `trade_paths` answers (the asked viewers are recorded in `trade_asked`).
    pub trade: Vec<exe_api::TradePath>,
    pub trade_asked: Vec<i32>,
    /// The router took the end of turn back (`take_turn_reopened` answers it once).
    pub reopened: bool,
    /// The last `set_turn_sent`.
    pub turn_sent: Option<bool>,
    /// The last `set_turn_locked`.
    pub turn_locked: Option<bool>,
    /// What `own_orders_pending` answers (UI messages of the seat not back in the journal).
    pub own_pending: bool,
    /// The replica is still joining (`GameSide::catching_up`).
    pub catching_up: bool,
}

impl FakeGame {
    fn selection_changed(&mut self) {
        let (count, (o, id)) = match self.head_unit {
            Some(h) => (1, h),
            None => (0, (-1, -1)),
        };
        self.events
            .push(ev(UiEventKind::UnitSelectionChanged, &[count, o, id]));
    }
}

impl GameSide for FakeGame {
    fn attach_lua(&mut self, lua: &lua_sys::Lua) -> Result<(), String> {
        self.attached += 1;
        if self.attach_script.is_empty() {
            return Ok(());
        }
        lua.exec(&self.attach_script, "=fake_attach")
            .map_err(|e| e.to_string())
    }

    fn invalidate_lua(&mut self, _lua: &lua_sys::Lua) {
        self.invalidated += 1;
    }

    fn take_events(&mut self, out: &mut Vec<UiEvent>) {
        out.append(&mut self.events);
    }

    fn ui_op(&mut self, op: UiOp, args: &[i32], out: &mut Vec<i32>) -> Result<(), String> {
        self.ops.push((op, args.to_vec()));
        let arg = |i: usize| args.get(i).copied().unwrap_or(0);
        let pair = |p: Option<(i32, i32)>| p.unwrap_or((-1, -1));
        match op {
            UiOp::SelectUnit => {
                self.head_unit = Some((arg(0), arg(1)));
                self.selection_changed();
            }
            UiOp::HeadSelectedUnit | UiOp::LastSelectedUnit => {
                let (a, b) = pair(self.head_unit);
                out.extend([a, b]);
            }
            UiOp::SelectedUnits => {
                if let Some((a, b)) = self.head_unit {
                    out.extend([a, b]);
                }
            }
            UiOp::ClearSelection => {
                if self.head_unit.take().is_some() {
                    self.selection_changed();
                }
            }
            UiOp::IsUnitSelected => out.push(i32::from(self.head_unit == Some((arg(0), arg(1))))),
            UiOp::GetSelectionCount => out.push(i32::from(self.head_unit.is_some())),
            UiOp::SelectCity => self.head_city = Some((arg(0), arg(1))),
            UiOp::HeadSelectedCity => {
                let (a, b) = pair(self.head_city);
                out.extend([a, b]);
            }
            UiOp::IsCitySelected => out.push(i32::from(self.head_city == Some((arg(0), arg(1))))),
            UiOp::ClearSelectedCities => self.head_city = None,
            UiOp::SetInterfaceMode => {
                let old = self.mode;
                self.mode = arg(0);
                if old != self.mode {
                    self.events.push(ev(
                        UiEventKind::InterfaceModeChanged,
                        &[old, self.mode, self.mode_value],
                    ));
                }
            }
            UiOp::GetInterfaceMode => out.push(self.mode),
            UiOp::SetInterfaceModeValue => self.mode_value = arg(0),
            UiOp::GetInterfaceModeValue => out.push(self.mode_value),
            UiOp::CanDoInterfaceMode => out.push(1),
            UiOp::SetCityScreenUp => {
                let up = arg(0) != 0;
                if up != self.city_screen {
                    self.city_screen = up;
                    self.events
                        .push(ev(UiEventKind::CityScreenChanged, &[i32::from(up)]));
                }
            }
            UiOp::IsCityScreenUp => out.push(i32::from(self.city_screen)),
            UiOp::CanSelectionListFound | UiOp::CanSelectionListWork => out.push(0),
            UiOp::CanEndTurn => out.push(i32::from(self.can_end_turn)),
            UiOp::RemoveNotification => self
                .events
                .push(ev(UiEventKind::NotificationRemoved, &[arg(0), 0])),
            UiOp::GetCycleSelectionCounter => out.push(self.cycle_counter),
            UiOp::SetCycleSelectionCounter => self.cycle_counter = arg(0),
            UiOp::SelectGroup
            | UiOp::DoSelectCityAtPlot
            | UiOp::SelectAllAtPlot
            | UiOp::ActivateNotification => {}
        }
        Ok(())
    }

    fn subscribe_game_event(&mut self, name: &str) {
        if !self.subscribed.iter().any(|n| n == name) {
            self.subscribed.push(name.to_owned());
        }
    }

    fn post_net(&mut self, msg: NetMessage) {
        self.posted.push(msg);
    }

    fn generation(&self) -> u64 {
        self.generation
    }

    fn set_turn_sent(&mut self, sent: bool) {
        self.turn_sent = Some(sent);
    }

    fn set_turn_locked(&mut self, locked: bool) {
        self.turn_locked = Some(locked);
    }

    fn take_turn_reopened(&mut self) -> bool {
        std::mem::take(&mut self.reopened)
    }

    fn own_orders_pending(&self) -> bool {
        self.own_pending
    }

    fn catching_up(&self) -> bool {
        self.catching_up
    }

    fn setup_graphical(&mut self) -> Result<(), String> {
        self.graphical_calls += 1;
        let batch: Vec<UiEvent> = self
            .graphical
            .iter()
            .cloned()
            .map(|mut e| {
                e.rebuild = true;
                e
            })
            .collect();
        self.events.extend(batch);
        Ok(())
    }

    fn unit_path(
        &mut self,
        player: i32,
        unit: i32,
        x: i32,
        y: i32,
    ) -> Result<Vec<PathNode>, String> {
        self.path_asked.push((player, unit, x, y));
        Ok(self.path.clone())
    }

    fn unit_reach(&mut self, _player: i32, _unit: i32) -> Result<Vec<PathNode>, String> {
        Ok(self.reach.clone())
    }

    fn unit_range_targets(&mut self, _player: i32, _unit: i32) -> Result<Vec<(i16, i16)>, String> {
        Ok(self.targets.clone())
    }

    fn trade_paths(&mut self, viewer: i32) -> Result<Vec<exe_api::TradePath>, String> {
        self.trade_asked.push(viewer);
        Ok(self.trade.clone())
    }
}

/// A path node.
pub fn node(x: i16, y: i16, turn: i16, flags: u16) -> PathNode {
    PathNode {
        x,
        y,
        turn,
        moves_left: 0,
        flags: exe_api::ReachFlags(flags),
    }
}

#[derive(Default)]
pub struct TestHost {
    pub kv: MemoryStore,
    pub looked: RefCell<Vec<(i32, i32)>>,
    pub seats: RefCell<SeatInfo>,
    /// `notification_sound` calls (type names).
    pub notification_sounds: RefCell<Vec<String>>,
    /// `unit_selection_sound` calls (scripts).
    pub unit_sounds: RefCell<Vec<String>>,
    /// `effect_sound` calls (scripts).
    pub effect_sounds: RefCell<Vec<String>>,
    /// `sound_preload` calls (scripts).
    pub sound_preloads: RefCell<Vec<String>>,
    /// `popup_sound` calls (popup type names).
    pub popup_sounds: RefCell<Vec<String>>,
    /// `event_sound` calls (event names).
    pub event_sounds: RefCell<Vec<String>>,
    /// `city_ambience` calls.
    pub ambience: RefCell<Vec<Option<String>>>,
    /// The camera's hex rectangle `visible_cells` answers.
    pub visible: std::cell::Cell<Option<[i32; 4]>>,
    /// `set_grid_visible` calls.
    pub grid: RefCell<Vec<bool>>,
    /// `gameplay_fx` calls (grid cells).
    pub fx: RefCell<Vec<(i32, i32)>>,
    /// The AI turn log `ai_turn` answers.
    pub ai_turn: RefCell<Option<exe_api::AiTurnLog>>,
    /// `voice_over` calls (script, delay).
    pub voices: RefCell<Vec<(String, u32)>>,
    /// `minor_civ_sound` calls (minor civ types).
    pub minor_voices: RefCell<Vec<String>>,
    /// `unit_clip_sound` calls.
    pub unit_cues: RefCell<Vec<exe_api::UnitClipCue>>,
    /// `music_event` calls.
    pub music_events: RefCell<Vec<exe_api::music::MusicEvent>>,
}

impl ExeHost for TestHost {
    fn gameplay_fx(&self, x: i32, y: i32) {
        self.fx.borrow_mut().push((x, y));
    }
    fn visible_cells(&self) -> Option<[i32; 4]> {
        self.visible.get()
    }
    fn set_grid_visible(&self, on: bool) {
        self.grid.borrow_mut().push(on);
    }
    fn kv(&self) -> &dyn KeyValueStore {
        &self.kv
    }
    fn look_at(&self, x: i32, y: i32) {
        self.looked.borrow_mut().push((x, y));
    }
    fn notification_sound(&self, type_name: &str) {
        self.notification_sounds
            .borrow_mut()
            .push(type_name.to_owned());
    }
    fn popup_sound(&self, type_name: &str) {
        self.popup_sounds.borrow_mut().push(type_name.to_owned());
    }
    fn event_sound(&self, name: &str) {
        self.event_sounds.borrow_mut().push(name.to_owned());
    }
    fn voice_over(&self, script: &str, delay_ms: u32) {
        self.voices.borrow_mut().push((script.to_owned(), delay_ms));
    }
    fn minor_civ_sound(&self, minor_type: &str) {
        self.minor_voices.borrow_mut().push(minor_type.to_owned());
    }
    fn unit_clip_sound(&self, cue: exe_api::UnitClipCue) {
        self.unit_cues.borrow_mut().push(cue);
    }
    fn unit_selection_sound(&self, script: &str) {
        self.unit_sounds.borrow_mut().push(script.to_owned());
    }
    fn sound_preload(&self, script: &str) {
        self.sound_preloads.borrow_mut().push(script.to_owned());
    }
    fn effect_sound(&self, script: &str) {
        self.effect_sounds.borrow_mut().push(script.to_owned());
    }
    fn city_ambience(&self, script: Option<&str>) {
        self.ambience.borrow_mut().push(script.map(str::to_owned));
    }
    fn seat_info(&self) -> SeatInfo {
        self.seats.borrow().clone()
    }
    fn ai_turn(&self) -> Option<exe_api::AiTurnLog> {
        self.ai_turn.borrow().clone()
    }
    fn music_event(&self, e: exe_api::music::MusicEvent) {
        self.music_events.borrow_mut().push(e);
    }
}

pub struct Rig {
    pub rt: Runtime,
    pub exe: ExeApi,
    pub game: Rc<RefCell<FakeGame>>,
    pub ctx: Context,
    pub host: Rc<TestHost>,
}

pub fn rig() -> Rig {
    rig_with(ExeConfig::default())
}

pub fn rig_with(cfg: ExeConfig) -> Rig {
    rig_with_game(cfg, FakeGame::default())
}

/// A rig over a prepared fake (an `attach_script`, a world).
pub fn rig_with_game(cfg: ExeConfig, game: FakeGame) -> Rig {
    rig_with_host(cfg, game, TestHost::default())
}

/// A rig over a host that already holds state (stored options).
pub fn rig_with_host(cfg: ExeConfig, game: FakeGame, host: TestHost) -> Rig {
    let host = Rc::new(host);
    let mut exe = ExeApi::new(cfg, host.clone());
    let rt = Runtime::new(
        RuntimeConfig::default(),
        Rc::new(MemorySource::new()),
        exe.base_setup(),
    )
    .expect("runtime");
    let ctx = rt.create_context("Test", |_, _| Ok(())).expect("context");
    let game = Rc::new(RefCell::new(game));
    let dynamic: Rc<RefCell<dyn GameSide>> = game.clone();
    exe.attach_runtime(&rt);
    exe.bind_game(dynamic, ctx.lua());
    // The first frame announces the (empty) game: SequenceGameInitComplete and a data refresh.
    exe.dispatch(&rt);
    Rig {
        rt,
        exe,
        game,
        ctx,
        host,
    }
}

impl Rig {
    pub fn exec(&self, code: &str) {
        self.ctx
            .exec(code, "=test")
            .unwrap_or_else(|e| panic!("{e}"));
    }

    pub fn eval_str(&self, code: &str) -> String {
        self.ctx
            .lua()
            .load(code)
            .set_name("=eval")
            .set_environment(self.ctx.env())
            .eval::<String>()
            .unwrap_or_else(|e| panic!("{e}"))
    }

    pub fn eval_bool(&self, code: &str) -> bool {
        self.ctx
            .lua()
            .load(code)
            .set_name("=eval")
            .set_environment(self.ctx.env())
            .eval::<bool>()
            .unwrap_or_else(|e| panic!("{e}"))
    }

    /// The global `log` (a Lua list of strings) joined with `,`.
    pub fn log(&self) -> String {
        self.eval_str("return table.concat(log, ',')")
    }

    pub fn errors(&self) -> u64 {
        self.rt.error_count()
    }
}
