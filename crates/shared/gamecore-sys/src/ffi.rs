//! Raw FFI mirror of `gamecore_abi.h` (ABI v2, `cpp/include/gamecore_abi.h`) and `gc_spike.h`, kept in
//! sync by hand. `cargo xtask check` compares the header with the Lekmod original; the layout tests
//! below compare the structs with the sizes the header gives.

#![allow(non_camel_case_types, non_snake_case)]

use std::ffi::{c_char, c_void};

use lua_sys::ffi::lua_State;

pub const GC_ABI_VERSION: u32 = 2;

// Return codes.
pub const GC_OK: i32 = 0;
pub const GC_ERR_ALREADY_EXISTS: i32 = -1;
pub const GC_ERR_INVALID_ARG: i32 = -2;
/// Reserved (v0 replica mode, removed); never returned.
pub const GC_ERR_WRONG_MODE: i32 = -3;
pub const GC_ERR_BAD_DATA: i32 = -4;
pub const GC_ERR_REJECTED: i32 = -5;
pub const GC_ERR_NOT_STARTED: i32 = -6;
pub const GC_ERR_NOT_FOUND: i32 = -7;
pub const GC_ERR_STUCK: i32 = -8;
pub const GC_ERR_NOT_IMPLEMENTED: i32 = -9;
/// B4 gc-replica: `gc_journal_apply` found the replica off the server's game; resync it.
pub const GC_ERR_DIVERGED: i32 = -10;
pub const GC_ERR_INTERNAL: i32 = -100;

// Log levels.
pub const GC_LOG_ERROR: i32 = 0;
pub const GC_LOG_WARN: i32 = 1;
pub const GC_LOG_INFO: i32 = 2;
pub const GC_LOG_DEBUG: i32 = 3;
pub const GC_LOG_TRACE: i32 = 4;

// Events (`gc_event.kind`).
pub const GC_EVENT_TURN_STARTED: i32 = 1;
pub const GC_EVENT_PLAYER_TURN_STARTED: i32 = 2;
pub const GC_EVENT_NOTIFICATION_ADDED: i32 = 3;
pub const GC_EVENT_NOTIFICATION_REMOVED: i32 = 4;
pub const GC_EVENT_AUTO_CHOICE: i32 = 5;
pub const GC_EVENT_DIPLO_AUTO_DECLINED: i32 = 6;
pub const GC_EVENT_POPUP_INFO: i32 = 7;
pub const GC_EVENT_GAME_OVER: i32 = 8;
pub const GC_EVENT_LUA_ERROR: i32 = 9;
pub const GC_EVENT_WAR_STATE: i32 = 10;
pub const GC_EVENT_MESSAGE: i32 = 11;
// v2 events.
pub const GC_EVENT_PLAYER_TURN_ENDED: i32 = 12;
pub const GC_EVENT_SLOT_CHANGED: i32 = 13;
pub const GC_EVENT_HUMAN_PHASE_STARTED: i32 = 14;
pub const GC_EVENT_HUMAN_WAR_DECLARED: i32 = 15;
pub const GC_EVENT_PLAYER_TURN_STATUS: i32 = 16;

// Refusal reasons (`gc_error.reason`).
pub const GC_REASON_NONE: i32 = 0;
pub const GC_REASON_NOT_HUMAN: i32 = 1;
pub const GC_REASON_NOT_YOUR_TURN: i32 = 2;
pub const GC_REASON_TURN_ENDED: i32 = 3;
pub const GC_REASON_AI_RUNNING: i32 = 4;
pub const GC_REASON_UNIT_BUSY: i32 = 5;
pub const GC_REASON_BAD_TARGET: i32 = 6;
pub const GC_REASON_RULE: i32 = 7;
pub const GC_REASON_BLOCKED: i32 = 8;
pub const GC_REASON_PUMP_FIRST: i32 = 9;
pub const GC_REASON_NOT_SAFE_POINT: i32 = 10;
pub const GC_REASON_SLOT_STATE: i32 = 11;

// Slot control ops (`gc_slot_control`).
pub const GC_SLOT_CONNECT: i32 = 1;
pub const GC_SLOT_DISCONNECT: i32 = 2;
pub const GC_SLOT_KICK: i32 = 3;

// B4 gc-replica: roles, journal records (GCJ1), net messages (GCN1).
pub const GC_ROLE_PLAIN: i32 = 0;
pub const GC_ROLE_SERVER: i32 = 1;
pub const GC_ROLE_REPLICA: i32 = 2;
pub const GC_JOURNAL_COMMAND: u8 = 1;
pub const GC_JOURNAL_NET: u8 = 2;
pub const GC_JOURNAL_PUMP: u8 = 3;
pub const GC_JOURNAL_SLOT: u8 = 4;
pub const GC_JOURNAL_CHECK: u8 = 5;
pub const GC_JOURNAL_TAKE_CHECK: u32 = 1;
pub const GC_NET_ARGS: usize = 9;

// B4 M2 WO-1: the UI bridge. Values as in gamecore_abi.h.
pub const GC_UI_MAX_ARGS: usize = 24;
pub const GC_UI_F_REBUILD: u32 = 0x0001;
pub const GC_UI_UNIT_INTS: usize = 14;
pub const GC_UI_CITY_INTS: usize = 11;
pub const GC_UI_EV_UNIT_CREATED: i32 = 1;
pub const GC_UI_EV_UNIT_MOVED: i32 = 2;
pub const GC_UI_EV_UNIT_TELEPORTED: i32 = 3;
pub const GC_UI_EV_UNIT_DESTROYED: i32 = 4;
pub const GC_UI_EV_UNIT_DESTROYED_IN_COMBAT: i32 = 5;
pub const GC_UI_EV_UNIT_SET_DAMAGE: i32 = 6;
pub const GC_UI_EV_UNIT_FORTIFY: i32 = 7;
pub const GC_UI_EV_UNIT_WORK: i32 = 8;
pub const GC_UI_EV_UNIT_PARADROP: i32 = 9;
pub const GC_UI_EV_UNIT_ACTIVATE: i32 = 10;
pub const GC_UI_EV_UNIT_EMBARK: i32 = 11;
pub const GC_UI_EV_UNIT_GARRISON: i32 = 12;
pub const GC_UI_EV_UNIT_SHOULD_DIM_FLAG: i32 = 13;
pub const GC_UI_EV_UNIT_MARK_THREATENING: i32 = 14;
pub const GC_UI_EV_UNIT_VISIBILITY: i32 = 15;
pub const GC_UI_EV_UNIT_COMBAT: i32 = 16;
pub const GC_UI_EV_UNIT_MISSION_END: i32 = 17;
pub const GC_UI_EV_UNIT_REBASED: i32 = 18;
pub const GC_UI_EV_UNIT_RESET_ANIMATION: i32 = 19;
pub const GC_UI_EV_CITY_CREATED: i32 = 20;
pub const GC_UI_EV_CITY_DESTROYED: i32 = 21;
pub const GC_UI_EV_CITY_CAPTURED: i32 = 22;
pub const GC_UI_EV_CITY_SET_DAMAGE: i32 = 23;
pub const GC_UI_EV_CITY_POPULATION_CHANGED: i32 = 24;
pub const GC_UI_EV_CITY_COMBAT: i32 = 25;
pub const GC_UI_EV_ERA_CHANGED: i32 = 30;
pub const GC_UI_EV_BRIDGE_CHANGED: i32 = 31;
pub const GC_UI_EV_TECH_ACQUIRED: i32 = 32;
pub const GC_UI_EV_FEATURE_CHANGED: i32 = 33;
pub const GC_UI_EV_PLOT_STATE_CHANGE: i32 = 34;
pub const GC_UI_EV_PLOT_ICON_STATE_CHANGE: i32 = 35;
pub const GC_UI_EV_NATURAL_WONDER_REVEALED: i32 = 36;
pub const GC_UI_EV_WALL_CREATED: i32 = 37;
pub const GC_UI_EV_DO_FX: i32 = 38;
pub const GC_UI_EV_FOW_CHANGED: i32 = 39;
pub const GC_UI_EV_YIELD_MIGHT_HAVE_CHANGED: i32 = 40;
pub const GC_UI_EV_WAR_STATE_CHANGED: i32 = 41;
pub const GC_UI_EV_MET_TEAM: i32 = 42;
pub const GC_UI_EV_DIPLO_AI_LEADER_MESSAGE: i32 = 43;
pub const GC_UI_EV_CLEAR_DIPLO_TRADE_TABLE: i32 = 44;
pub const GC_UI_EV_NOTIFY_AI_LEADERS_IN_GAME: i32 = 45;
pub const GC_UI_EV_NOTIFY_SPECIFIC_AI_LEADER: i32 = 46;
pub const GC_UI_EV_WONDER_CREATED: i32 = 50;
pub const GC_UI_EV_WONDER_EDITED: i32 = 51;
pub const GC_UI_EV_WONDER_REMOVED: i32 = 52;
pub const GC_UI_EV_SPACESHIP_CREATED: i32 = 53;
pub const GC_UI_EV_SPACESHIP_EDITED: i32 = 54;
pub const GC_UI_EV_SPACESHIP_REMOVED: i32 = 55;
pub const GC_UI_EV_OPEN_OPTIONS_SCREEN: i32 = 56;
pub const GC_UI_EV_SEARCH_FOR_PEDIA_ENTRY: i32 = 57;
pub const GC_UI_EV_OPEN_INFO_CORNER: i32 = 58;
pub const GC_UI_EV_WORLD_ANCHOR: i32 = 59;
pub const GC_UI_EV_MINIMAP_UNIT_SELECT: i32 = 60;
pub const GC_UI_EV_MINIMAP_NOTIFICATION: i32 = 61;
pub const GC_UI_EV_GOLDEN_AGE_STARTED: i32 = 62;
pub const GC_UI_EV_GOLDEN_AGE_ENDED: i32 = 63;
pub const GC_UI_EV_TURN_CHANGED: i32 = 64;
pub const GC_UI_EV_ACTIVE_PLAYER_CHANGED: i32 = 65;
pub const GC_UI_EV_PUBLISH_ACTIVE_PLAYER: i32 = 66;
pub const GC_UI_EV_PUBLISH_NEW_GAME_TURN: i32 = 67;
pub const GC_UI_EV_PUBLISH_ERA_CHANGES: i32 = 68;
pub const GC_UI_EV_TRADE_NEW_ROUTE: i32 = 69;
pub const GC_UI_EV_TRADE_ROUTE_DIRECTION: i32 = 70;
pub const GC_UI_EV_TRADE_DESTROY_ROUTE: i32 = 71;
pub const GC_UI_EV_TRADE_ACTIVATE_POPUP_ROUTE: i32 = 72;
pub const GC_UI_EV_TRADE_DEACTIVATE_POPUP_ROUTE: i32 = 73;
pub const GC_UI_EV_ACTIVE_PLAYER_TURN_START: i32 = 80;
pub const GC_UI_EV_ACTIVE_PLAYER_TURN_END: i32 = 81;
pub const GC_UI_EV_REMOTE_PLAYER_TURN_START: i32 = 82;
pub const GC_UI_EV_REMOTE_PLAYER_TURN_END: i32 = 83;
pub const GC_UI_EV_PLAYER_TURN_STATUS: i32 = 84;
pub const GC_UI_EV_NOTIFICATION_ADDED: i32 = 85;
pub const GC_UI_EV_NOTIFICATION_REMOVED: i32 = 86;
pub const GC_UI_EV_NOTIFICATION_ACTIVATED: i32 = 87;
pub const GC_UI_EV_MESSAGE: i32 = 88;
pub const GC_UI_EV_CLEAR_EVENT_MESSAGES: i32 = 89;
pub const GC_UI_EV_POPUP: i32 = 90;
pub const GC_UI_EV_POPUP_ARRAY: i32 = 91;
pub const GC_UI_EV_POPUP_TEXT: i32 = 92;
pub const GC_UI_EV_SET_DIRTY: i32 = 93;
pub const GC_UI_EV_LOOK_AT: i32 = 94;
pub const GC_UI_EV_CENTER_CAMERA: i32 = 95;
pub const GC_UI_EV_SPECIFIC_CITY_INFO_DIRTY: i32 = 96;
pub const GC_UI_EV_UPDATE_END_TURN: i32 = 97;
pub const GC_UI_EV_END_TURN_BLOCKING_CHANGED: i32 = 98;
pub const GC_UI_EV_PUBLISH_END_TURN_DIRTY: i32 = 99;
pub const GC_UI_EV_UPDATE_CITY_SCREEN: i32 = 100;
pub const GC_UI_EV_UNIT_SELECTION_CHANGED: i32 = 101;
pub const GC_UI_EV_CITY_SELECTION_CHANGED: i32 = 102;
pub const GC_UI_EV_INTERFACE_MODE_CHANGED: i32 = 103;
pub const GC_UI_EV_CITY_SCREEN_CHANGED: i32 = 104;
pub const GC_UI_EV_UPDATE_COUNTRY_BORDER: i32 = 105;
pub const GC_UI_EV_UPDATE_ALL_COUNTRY_BORDERS: i32 = 106;
pub const GC_UI_EV_ADD_HEX_TO_UI_RANGE: i32 = 107;
pub const GC_UI_EV_OPEN_END_GAME_MENU: i32 = 108;
pub const GC_UI_EV_OPEN_TECH_TREE: i32 = 109;
pub const GC_UI_EV_OPEN_PLAYER_DEAL_SCREEN: i32 = 110;
pub const GC_UI_EV_ADD_DEFERRED_WONDER_COMMAND: i32 = 111;
pub const GC_UI_EV_TOGGLE_STRATEGIC_VIEW: i32 = 112;
pub const GC_UI_EV_SET_TEMP_STRING: i32 = 113;
pub const GC_UI_EV_LOOK_AT_SELECTION_PLOT: i32 = 114;
pub const GC_UI_EV_LOOK_AT_CITY_OFFSET: i32 = 115;
pub const GC_UI_EV_FLASHING: i32 = 116;
pub const GC_UI_EV_REFRESH_YIELD_VISIBLE_MODE: i32 = 117;
pub const GC_UI_EV_GAME_EVENT: i32 = 150;
pub const GC_UI_EV_LUA_EVENT: i32 = 151;
pub const GC_UI_VAL_NIL: u8 = 0;
pub const GC_UI_VAL_BOOL: u8 = 1;
pub const GC_UI_VAL_NUM: u8 = 2;
pub const GC_UI_VAL_STR: u8 = 3;
pub const GC_UI_VAL_VEC: u8 = 4;
pub const GC_UI_OP_SELECT_UNIT: i32 = 1;
pub const GC_UI_OP_SELECT_GROUP: i32 = 2;
pub const GC_UI_OP_HEAD_SELECTED_UNIT: i32 = 3;
pub const GC_UI_OP_SELECTED_UNITS: i32 = 4;
pub const GC_UI_OP_CLEAR_SELECTION: i32 = 5;
pub const GC_UI_OP_LAST_SELECTED_UNIT: i32 = 6;
pub const GC_UI_OP_SELECT_CITY: i32 = 7;
pub const GC_UI_OP_HEAD_SELECTED_CITY: i32 = 8;
pub const GC_UI_OP_CLEAR_SELECTED_CITIES: i32 = 9;
pub const GC_UI_OP_DO_SELECT_CITY_AT_PLOT: i32 = 10;
pub const GC_UI_OP_SET_INTERFACE_MODE: i32 = 11;
pub const GC_UI_OP_GET_INTERFACE_MODE: i32 = 12;
pub const GC_UI_OP_SET_INTERFACE_MODE_VALUE: i32 = 13;
pub const GC_UI_OP_GET_INTERFACE_MODE_VALUE: i32 = 14;
pub const GC_UI_OP_CAN_DO_INTERFACE_MODE: i32 = 15;
pub const GC_UI_OP_SET_CITY_SCREEN_UP: i32 = 16;
pub const GC_UI_OP_IS_CITY_SCREEN_UP: i32 = 17;
pub const GC_UI_OP_CAN_SELECTION_LIST_FOUND: i32 = 18;
pub const GC_UI_OP_CAN_SELECTION_LIST_WORK: i32 = 19;
pub const GC_UI_OP_CAN_END_TURN: i32 = 20;
pub const GC_UI_OP_ACTIVATE_NOTIFICATION: i32 = 21;
pub const GC_UI_OP_REMOVE_NOTIFICATION: i32 = 22;
pub const GC_UI_OP_GET_CYCLE_SELECTION_COUNTER: i32 = 23;
pub const GC_UI_OP_SET_CYCLE_SELECTION_COUNTER: i32 = 24;
pub const GC_UI_OP_IS_UNIT_SELECTED: i32 = 25;
pub const GC_UI_OP_IS_CITY_SELECTED: i32 = 26;
pub const GC_UI_OP_SELECT_ALL_AT_PLOT: i32 = 27;
pub const GC_UI_OP_GET_SELECTION_COUNT: i32 = 28;
pub const GC_UI_OP_COUNT: i32 = 29;

// Pump status (`gc_pump_result.status`).
pub const GC_PUMP_WAITING_HUMAN: i32 = 0;
pub const GC_PUMP_AI_RUNNING: i32 = 1;
pub const GC_PUMP_GAME_OVER: i32 = 2;
pub const GC_PUMP_STUCK: i32 = 3;

// Commands (`gc_command.kind`).
pub const GC_CMD_UNIT_MOVE_TO: u16 = 1;
pub const GC_CMD_UNIT_RANGE_ATTACK: u16 = 2;
pub const GC_CMD_UNIT_FOUND_CITY: u16 = 3;
pub const GC_CMD_UNIT_FORTIFY: u16 = 4;
pub const GC_CMD_UNIT_SLEEP: u16 = 5;
pub const GC_CMD_UNIT_SKIP: u16 = 6;
pub const GC_CMD_UNIT_HEAL: u16 = 7;
pub const GC_CMD_UNIT_AUTOMATE_BUILD: u16 = 8;
pub const GC_CMD_UNIT_AUTOMATE_EXPLORE: u16 = 9;
pub const GC_CMD_UNIT_CANCEL: u16 = 10;
pub const GC_CMD_UNIT_DISBAND: u16 = 11;
pub const GC_CMD_UNIT_WAKE: u16 = 12;
pub const GC_CMD_CITY_SET_PRODUCTION: u16 = 20;
pub const GC_CMD_CITY_PURCHASE: u16 = 21;
pub const GC_CMD_CITY_RANGE_ATTACK: u16 = 22;
pub const GC_CMD_DECLARE_WAR: u16 = 30;
pub const GC_CMD_MAKE_PEACE: u16 = 31;
pub const GC_CMD_END_TURN: u16 = 40;
pub const GC_CMD_FORCE_END_TURN: u16 = 41;
pub const GC_CMD_CANCEL_END_TURN: u16 = 42;
pub const GC_CMD_END_TURN_TIMEOUT: u16 = 43;

// Queries (`gc_query` kind).
pub const GC_Q_GAME: i32 = 1;
pub const GC_Q_PLOTS: i32 = 2;
pub const GC_Q_UNITS: i32 = 3;
pub const GC_Q_CITIES: i32 = 4;
pub const GC_Q_PLAYERS: i32 = 5;
pub const GC_Q_TOP_BAR: i32 = 6;
pub const GC_Q_CITY_BUILDS: i32 = 7;
pub const GC_Q_UNIT_ACTIONS: i32 = 8;
pub const GC_Q_PATH: i32 = 9;
pub const GC_Q_REACHABLE: i32 = 10;
pub const GC_Q_RANGE_TARGETS: i32 = 11;
pub const GC_Q_NOTIFICATIONS: i32 = 12;
pub const GC_Q_CATALOG: i32 = 13;
// v2 queries (17 and 18 are preliminary: their records are fixed by package P13).
pub const GC_Q_TURN: i32 = 14;
pub const GC_Q_DIPLO: i32 = 15;
pub const GC_Q_REACHABLE_TREE: i32 = 16;
pub const GC_Q_CITY_ART: i32 = 17;
pub const GC_Q_TRADE_ROUTES: i32 = 18;
pub const GC_Q_PLOT_BASIS: i32 = 19;
pub const GC_Q_COMBAT_PREVIEW: i32 = 20;
pub const GC_Q_PLOTS_DIRTY: i32 = 21;
/// B4 fix-path: the plots of the routes of Q_TRADE_ROUTES (`gcq_trade_plot`).
pub const GC_Q_TRADE_PATHS: i32 = 22;
// UI v2 read-only queries (append only, ABI v2).
/// `{player, city}`: the city's order queue (`gcq_city_order`).
pub const GC_Q_CITY_QUEUE: i32 = 23;
/// `{player}`: every technology and its state for the player (`gcq_tech`).
pub const GC_Q_TECHS: i32 = 24;
/// `{player}`: the resources the player has or uses (`gcq_resource`).
pub const GC_Q_PLAYER_RESOURCES: i32 = 25;
/// `{player}`: every policy branch and the player's progress in it (`gcq_policy_branch`).
pub const GC_Q_POLICY_BRANCHES: i32 = 26;
/// `{player, unit}`: the promotions the unit has (`gcq_promotion`).
pub const GC_Q_UNIT_PROMOTIONS: i32 = 27;

// Q_UNIT_ACTIONS rows (UI v2): the optional third argument and the row kinds that are not commands.
pub const GC_UNIT_ACTIONS_ROWS_EXTRA: i32 = 0x0001;
pub const GC_UNIT_ROW_ALERT: i16 = 101;
pub const GC_UNIT_ROW_PILLAGE: i16 = 102;
pub const GC_UNIT_ROW_UPGRADE: i16 = 103;
// Q_TECHS status, Q_PLAYER_RESOURCES usage (UI v2).
pub const GC_TECH_HAVE: u8 = 0;
pub const GC_TECH_CAN_RESEARCH: u8 = 1;
pub const GC_TECH_LOCKED: u8 = 2;
pub const GC_RESOURCE_USAGE_BONUS: u8 = 0;
pub const GC_RESOURCE_USAGE_STRATEGIC: u8 = 1;
pub const GC_RESOURCE_USAGE_LUXURY: u8 = 2;

/// Opaque instance (`gc_instance`).
#[repr(C)]
pub struct gc_instance {
    _private: [u8; 0],
}

#[repr(C)]
pub struct gc_buf {
    pub data: *mut u8,
    pub len: usize,
    pub release: Option<unsafe extern "C" fn(this: *mut gc_buf)>,
    pub owner: *mut c_void,
}

impl gc_buf {
    pub const fn empty() -> Self {
        gc_buf {
            data: std::ptr::null_mut(),
            len: 0,
            release: None,
            owner: std::ptr::null_mut(),
        }
    }
}

#[repr(C)]
pub struct gc_error {
    pub code: i32,
    pub message: [c_char; 256],
    /// `GC_REASON_*` (v2).
    pub reason: i32,
}

impl gc_error {
    pub const fn zeroed() -> Self {
        gc_error {
            code: 0,
            message: [0; 256],
            reason: 0,
        }
    }
}

/// `args`, `text` and `text2` are valid only during the callback.
#[repr(C)]
pub struct gc_event {
    pub kind: i32,
    pub argc: u32,
    pub args: *const i32,
    pub text: *const c_char,
    pub text2: *const c_char,
}

#[repr(C)]
pub struct gc_host {
    pub user: *mut c_void,
    pub log: Option<unsafe extern "C" fn(user: *mut c_void, level: i32, msg: *const c_char)>,
    pub read_file: Option<
        unsafe extern "C" fn(user: *mut c_void, vfs_path: *const c_char, out: *mut gc_buf) -> i32,
    >,
    pub game_event: Option<unsafe extern "C" fn(user: *mut c_void, ev: *const gc_event)>,
}

#[repr(C)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct gc_pump_result {
    pub turn: i32,
    pub status: i32,
    pub active_player: i32,
    pub blocker: i32,
    pub slices_used: i32,
    /// Humans the turn still waits for (v2).
    pub humans_left: i32,
}

#[repr(C)]
pub struct gc_command {
    pub kind: u16,
    pub player: u8,
    pub reserved: u8,
    pub argc: u32,
    pub args: *const i32,
    pub text: *const c_char,
}

/// Header of every `gc_query` reply (little endian).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct gcq_header {
    pub magic: [u8; 4],
    pub format: u16,
    pub kind: u16,
    pub count: u32,
    pub record_size: u32,
    pub strings_offset: u32,
    pub strings_size: u32,
}

/// `gcq_*` string reference meaning "no string".
pub const GCQ_NO_STRING: u32 = 0xFFFF_FFFF;

// Q_GAME (`gcq_game.status`).
pub const GC_GAME_STATUS_WAITING_HUMAN: i32 = 0;
pub const GC_GAME_STATUS_AI_RUNNING: i32 = 1;
pub const GC_GAME_STATUS_GAME_OVER: i32 = 2;
pub const GC_GAME_STATUS_STUCK: i32 = 3;

// Q_PLOTS (`gcq_plot.flags`, `gcq_plot.plot_type`).
pub const GC_PLOT_F_REVEALED: u16 = 0x0001;
pub const GC_PLOT_F_VISIBLE: u16 = 0x0002;
pub const GC_PLOT_F_CITY: u16 = 0x0004;
pub const GC_PLOT_F_LAKE: u16 = 0x0008;
pub const GC_PLOT_F_COASTAL: u16 = 0x0010;
pub const GC_PLOT_F_RIVER_SHIFT: u32 = 5;
pub const GC_PLOT_F_RIVER_MASK: u16 = 0x07E0;
pub const GC_PLOT_F_PILLAGED: u16 = 0x0800;
pub const GC_PLOT_F_NATURAL_WONDER: u16 = 0x1000;
pub const GC_PLOT_F_WORKED: u16 = 0x2000;
pub const GC_PLOT_F_ROUTE_PILLAGED: u16 = 0x4000;
pub const GC_PLOT_TYPE_MOUNTAIN: i32 = 0;
pub const GC_PLOT_TYPE_HILLS: i32 = 1;
pub const GC_PLOT_TYPE_LAND: i32 = 2;
pub const GC_PLOT_TYPE_OCEAN: i32 = 3;

// Q_PLAYERS (`gcq_player.flags`).
pub const GC_PLAYER_F_ALIVE: u16 = 0x0001;
pub const GC_PLAYER_F_HUMAN: u16 = 0x0002;
pub const GC_PLAYER_F_MINOR: u16 = 0x0004;
pub const GC_PLAYER_F_BARBARIAN: u16 = 0x0008;
pub const GC_PLAYER_F_MET: u16 = 0x0010;
pub const GC_PLAYER_F_AT_WAR_WITH_VIEWER: u16 = 0x0020;
pub const GC_PLAYER_F_OBSERVER: u16 = 0x0040;

// Q_GAME v2 fields (`turn_mode`, `phase`, `unsafe_reason`).
pub const GC_TURN_MODE_SIMULTANEOUS: i32 = 1;
pub const GC_TURN_MODE_HYBRID: i32 = 2;
pub const GC_PHASE_HUMANS: i32 = 0;
pub const GC_PHASE_AI: i32 = 1;
pub const GC_UNSAFE_NONE: i32 = 0;
pub const GC_UNSAFE_AI_RUNNING: i32 = 1;
pub const GC_UNSAFE_COMMAND_GIVEN: i32 = 2;
pub const GC_UNSAFE_AI_REQUEST_OPEN: i32 = 3;
pub const GC_UNSAFE_AUTOMOVES: i32 = 4;

// Q_TURN (`gcq_turn.flags`).
pub const GC_TURN_F_HUMAN: u16 = 0x0001;
pub const GC_TURN_F_ALIVE: u16 = 0x0002;
pub const GC_TURN_F_CONNECTED: u16 = 0x0004;
pub const GC_TURN_F_DISCONNECTED: u16 = 0x0008;
pub const GC_TURN_F_KICKED: u16 = 0x0010;
pub const GC_TURN_F_TURN_ACTIVE: u16 = 0x0020;
pub const GC_TURN_F_TURN_ENDED: u16 = 0x0040;
pub const GC_TURN_F_CAN_END_TURN: u16 = 0x0080;
pub const GC_TURN_F_SIMULTANEOUS: u16 = 0x0100;
pub const GC_TURN_F_AUTOMOVES: u16 = 0x0200;
pub const GC_TURN_F_WAITING_FOR: u16 = 0x0400;

// Q_DIPLO (`gcq_diplo.flags`).
pub const GC_DIPLO_F_MET: u16 = 0x0001;
pub const GC_DIPLO_F_AT_WAR: u16 = 0x0002;
pub const GC_DIPLO_F_HUMAN: u16 = 0x0004;
pub const GC_DIPLO_F_CAN_DECLARE_WAR: u16 = 0x0008;
pub const GC_DIPLO_F_CAN_MAKE_PEACE: u16 = 0x0010;

// Q_PATH / Q_REACHABLE / Q_REACHABLE_TREE node flags.
pub const GC_REACH_F_ATTACK: u16 = 0x0001;
pub const GC_REACH_F_EMBARK: u16 = 0x0002;
pub const GC_REACH_F_DISEMBARK: u16 = 0x0004;
pub const GC_REACH_F_CITY: u16 = 0x0008;
pub const GC_REACH_F_STOP: u16 = 0x0010;
pub const GC_REACH_F_UNREVEALED: u16 = 0x0020;
pub const GC_REACH_F_CAPTURE: u16 = 0x0040;
pub const GC_REACH_F_UNREACHABLE: u16 = 0x0080;

// Q_PLOT_BASIS (`gcq_plot_basis.flags`).
pub const GC_BASIS_F_LAKE: u16 = 0x0001;
pub const GC_BASIS_F_COASTAL: u16 = 0x0002;
pub const GC_BASIS_F_NATURAL_WONDER: u16 = 0x0004;

// Q_COMBAT_PREVIEW (`gcq_combat_preview.flags`).
pub const GC_COMBAT_F_KILLS: u16 = 0x0001;
pub const GC_COMBAT_F_CAPTURES: u16 = 0x0002;
pub const GC_COMBAT_F_CITY: u16 = 0x0004;

// Q_CATALOG tables (the query arg).
pub const GC_CATALOG_UNITS: i32 = 0;
pub const GC_CATALOG_BUILDINGS: i32 = 1;
pub const GC_CATALOG_PROJECTS: i32 = 2;
pub const GC_CATALOG_TECHNOLOGIES: i32 = 3;
pub const GC_CATALOG_POLICIES: i32 = 4;
pub const GC_CATALOG_TERRAINS: i32 = 5;
pub const GC_CATALOG_FEATURES: i32 = 6;
pub const GC_CATALOG_RESOURCES: i32 = 7;
pub const GC_CATALOG_IMPROVEMENTS: i32 = 8;
pub const GC_CATALOG_ROUTES: i32 = 9;
pub const GC_CATALOG_CIVILIZATIONS: i32 = 10;
pub const GC_CATALOG_LEADERS: i32 = 11;
pub const GC_CATALOG_ERAS: i32 = 12;
pub const GC_CATALOG_HANDICAPS: i32 = 13;
pub const GC_CATALOG_PROMOTIONS: i32 = 14;
pub const GC_CATALOG_TABLE_COUNT: i32 = 15;
// v2 tables (served when package P5 lands; `GC_CATALOG_TABLE_COUNT` still counts the v1 tables).
pub const GC_CATALOG_TURN_TIMERS: i32 = 15;
pub const GC_CATALOG_ART_TECHS: i32 = 16;
pub const GC_CATALOG_WORLD_SIZES: i32 = 17;
pub const GC_CATALOG_GAME_SPEEDS: i32 = 18;
pub const GC_CATALOG_TABLE_COUNT_V2: i32 = 19;
// v3 tables (login-lobby P2): the staging room options of the game data, records `gcq_catalog_ext`.
pub const GC_CATALOG_GAME_OPTIONS: i32 = 19;
pub const GC_CATALOG_VICTORIES: i32 = 20;
pub const GC_CATALOG_MINOR_CIVS: i32 = 21;
pub const GC_CATALOG_MAP_OPTIONS: i32 = 22;
pub const GC_CATALOG_MAP_OPTION_VALUES: i32 = 23;
pub const GC_CATALOG_TABLE_COUNT_V3: i32 = 24;
// v4 (b9 input-lobby): the civilization hover tip, records `gcq_catalog_ext`.
pub const GC_CATALOG_CIV_INFO: i32 = 24;
pub const GC_CATALOG_TABLE_COUNT_V4: i32 = 25;
// v5 (UI v2): the policy branches, records `gcq_catalog` (id, type, name).
pub const GC_CATALOG_POLICY_BRANCHES: i32 = 25;
pub const GC_CATALOG_TABLE_COUNT_V5: i32 = 26;

// Q_UNITS / Q_CITIES / Q_TOP_BAR / Q_CITY_BUILDS (G1a): flags, OrderTypes, YieldTypes.
pub const GC_UNIT_F_CAN_MOVE: u16 = 0x0001;
pub const GC_UNIT_F_NEEDS_ORDERS: u16 = 0x0002;
pub const GC_UNIT_F_FORTIFIED: u16 = 0x0004;
pub const GC_UNIT_F_SLEEPING: u16 = 0x0008;
pub const GC_UNIT_F_AUTOMATED: u16 = 0x0010;
pub const GC_UNIT_F_EMBARKED: u16 = 0x0020;
pub const GC_UNIT_F_CIVILIAN: u16 = 0x0040;
pub const GC_UNIT_F_CAN_PROMOTE: u16 = 0x0080;
pub const GC_UNIT_F_GARRISONED: u16 = 0x0100;
pub const GC_UNIT_F_CARGO: u16 = 0x0200;
pub const GC_ORDER_TRAIN: i32 = 0;
pub const GC_ORDER_CONSTRUCT: i32 = 1;
pub const GC_ORDER_CREATE: i32 = 2;
pub const GC_ORDER_MAINTAIN: i32 = 4;
pub const GC_YIELD_FOOD: i32 = 0;
pub const GC_YIELD_PRODUCTION: i32 = 1;
pub const GC_YIELD_GOLD: i32 = 2;
pub const GC_YIELD_SCIENCE: i32 = 3;
pub const GC_YIELD_CULTURE: i32 = 4;
pub const GC_YIELD_FAITH: i32 = 5;
pub const GC_CITY_F_CAPITAL: u16 = 0x0001;
pub const GC_CITY_F_PUPPET: u16 = 0x0002;
pub const GC_CITY_F_RAZING: u16 = 0x0004;
pub const GC_CITY_F_OCCUPIED: u16 = 0x0008;
pub const GC_CITY_F_CAN_RANGE_ATTACK: u16 = 0x0010;
pub const GC_CITY_F_VISIBLE: u16 = 0x0020;
pub const GC_CITY_F_RESISTANCE: u16 = 0x0040;
pub const GC_TOPBAR_F_UNHAPPY: u16 = 0x0001;
pub const GC_TOPBAR_F_VERY_UNHAPPY: u16 = 0x0002;
pub const GC_TOPBAR_F_GOLDEN_AGE: u16 = 0x0004;
pub const GC_BUILD_F_WONDER: u16 = 0x0001;
pub const GC_BUILD_F_CURRENT: u16 = 0x0002;
pub const GC_BUILD_F_CAN_PURCHASE: u16 = 0x0004;
pub const GC_BUILD_F_ENABLED: u16 = 0x0008;
// Q_RANGE_TARGETS (G1b).
pub const GC_RANGE_TARGET_F_CITY: u16 = 0x0001;

/// Q_GAME record (56 bytes; v1: 36, the v2 fields start at `game_type`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct gcq_game {
    pub turn: i32,
    pub year: i32,
    pub blocker_unit: i32,
    pub blocker_city: i32,
    pub blocker_x: i16,
    pub blocker_y: i16,
    pub map_width: i16,
    pub map_height: i16,
    pub human_player: i8,
    pub human_team: i8,
    pub active_player: i8,
    pub status: i8,
    pub blocker: i8,
    pub wrap_x: u8,
    pub wrap_y: u8,
    pub difficulty: i8,
    pub winner: i8,
    pub victory_type: i8,
    pub reserved: [u8; 2],
    pub game_type: i8,
    pub turn_mode: i8,
    pub phase: i8,
    pub humans: u8,
    pub humans_left: u8,
    pub safe_point: i8,
    pub unsafe_reason: i8,
    pub reserved1: u8,
    pub turn_len_s: i32,
    pub turn_base_s: i32,
    pub map_seed: u32,
}

/// Q_PLOTS record (32 bytes; v1: 28, the v2 fields start at `build_type`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct gcq_plot {
    pub x: i16,
    pub y: i16,
    pub flags: u16,
    pub plot_type: i8,
    pub reserved0: u8,
    pub terrain: i16,
    pub feature: i16,
    pub resource: i16,
    pub resource_qty: i8,
    pub reserved1: u8,
    pub improvement: i16,
    pub route: i16,
    pub owner: i8,
    pub continent_art: i8,
    pub yields: [i8; 6],
    pub build_type: i16,
    pub build_pct: u8,
    pub reserved2: u8,
}

/// Q_PLAYERS record (48 bytes; v1: 28, the v2 fields start at `nick`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct gcq_player {
    pub id: i8,
    pub team: i8,
    pub civ_type: i16,
    pub leader_type: i16,
    pub flags: u16,
    pub color_primary: u32,
    pub color_secondary: u32,
    pub name: u32,
    pub civ_name: u32,
    pub era: i8,
    pub reserved0: [u8; 3],
    pub nick: u32,
    pub slot: i8,
    pub reserved1: [u8; 3],
    pub reserved2: u32,
    pub art_techs: u64,
}

/// Q_CATALOG record (36 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct gcq_catalog {
    pub id: i16,
    pub icon_index: i16,
    pub flag_index: i16,
    pub reserved: i16,
    pub r#type: u32,
    pub name: u32,
    pub art_define: u32,
    pub icon_atlas: u32,
    pub flag_atlas: u32,
    pub art_style: u32,
    pub strategic_view: u32,
}

/// Q_UNITS record (44 bytes; v1: 36, the v2 fields start at `transport_id`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct gcq_unit {
    pub id: i32,
    pub owner: i8,
    pub reserved0: u8,
    pub unit_type: i16,
    pub x: i16,
    pub y: i16,
    pub hp: i16,
    pub max_hp: i16,
    pub moves_left: i16,
    pub max_moves: i16,
    pub strength: i16,
    pub ranged: i16,
    pub range: i8,
    pub domain: i8,
    pub activity: i8,
    pub level: i8,
    pub experience: i16,
    pub flags: u16,
    pub name: u32,
    pub transport_id: i32,
    pub build_type: i16,
    pub facing: i8,
    pub reserved1: u8,
}

/// Q_UNIT_ACTIONS record (12 bytes; v1: 8, UI v2 appends `cost`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct gcq_unit_action {
    pub action: i16,
    pub enabled: u8,
    pub reserved: u8,
    pub reason: u32,
    pub cost: i32,
}

/// Q_PATH / Q_REACHABLE record (12 bytes; v1: 8, the v2 fields start at `flags`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct gcq_path_node {
    pub x: i16,
    pub y: i16,
    pub turn: i16,
    pub moves_left: i16,
    pub flags: u16,
    pub reserved: u16,
}

/// Q_CATALOG record of the TurnTimers table (24 bytes, v2).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct gcq_catalog_timer {
    pub id: i16,
    pub reserved: i16,
    pub r#type: u32,
    pub name: u32,
    pub base_time: i32,
    pub city_resource: i32,
    pub unit_resource: i32,
}

/// Q_CATALOG record of the v3 tables (32 bytes): four integers and four strings, what they hold depends on the table
/// (see `gamecore_abi.h`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct gcq_catalog_ext {
    pub n: [i32; 4],
    pub s: [u32; 4],
}

/// Q_TURN record (24 bytes, v2).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct gcq_turn {
    pub player: i8,
    pub team: i8,
    pub slot_status: i8,
    pub blocker: i8,
    pub flags: u16,
    pub turn_order: i16,
    pub blocker_unit: i32,
    pub blocker_city: i32,
    pub blocker_x: i16,
    pub blocker_y: i16,
    pub nick: u32,
}

/// Q_DIPLO record (8 bytes, v2).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct gcq_diplo {
    pub other: i8,
    pub team: i8,
    pub flags: u16,
    pub war_locked_turns: i16,
    pub reserved: i16,
}

/// Q_REACHABLE_TREE record (16 bytes, v2).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct gcq_tree_node {
    pub x: i16,
    pub y: i16,
    pub parent_x: i16,
    pub parent_y: i16,
    pub moves_left: i16,
    pub turn: u8,
    pub reserved0: u8,
    pub flags: u16,
    pub reserved1: u16,
}

/// Q_PLOT_BASIS record (16 bytes, v2).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct gcq_plot_basis {
    pub index: u32,
    pub plot_type: i8,
    pub continent_art: u8,
    pub terrain: i16,
    pub feature: i16,
    pub flags: u16,
    pub rivers: u8,
    pub river_flow: [i8; 3],
}

/// Q_COMBAT_PREVIEW record (20 bytes, v2).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct gcq_combat_preview {
    pub damage_to_target: i16,
    pub damage_to_attacker: i16,
    pub attacker_strength: i32,
    pub defender_strength: i32,
    pub target_hp: i16,
    pub target_max_hp: i16,
    pub flags: u16,
    pub reserved: u16,
}

/// Q_PLOTS_DIRTY record (4 bytes, v2).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct gcq_plot_dirty {
    pub index: u32,
}

/// Q_RANGE_TARGETS record (8 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct gcq_range_target {
    pub x: i16,
    pub y: i16,
    pub flags: u16,
    pub reserved: u16,
}

/// Q_CITIES record (60 bytes; v1: 52, the v2 fields start at `founder`, UI v2 appends `overflow`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct gcq_city {
    pub id: i32,
    pub owner: i8,
    pub production_order: i8,
    pub x: i16,
    pub y: i16,
    pub population: i16,
    pub flags: u16,
    pub hp: i16,
    pub max_hp: i16,
    pub defense: i16,
    pub production_item: i16,
    pub production_turns: i16,
    pub growth_turns: i16,
    pub reserved: i16,
    pub production_stored: i32,
    pub production_needed: i32,
    pub yields: [i16; 6],
    pub name: u32,
    pub founder: i8,
    pub reserved3: [u8; 3],
    pub overflow: i32,
}

/// Q_CITY_ART record (12 bytes, v2).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct gcq_city_art {
    pub city_id: i32,
    pub owner: i8,
    pub reserved0: u8,
    pub building: i16,
    pub x: i16,
    pub y: i16,
}

/// Q_TRADE_ROUTES record (16 bytes, v2).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct gcq_trade_route {
    pub id: i32,
    pub owner: i8,
    pub dest_owner: i8,
    pub domain: i8,
    pub reserved0: u8,
    pub from_x: i16,
    pub from_y: i16,
    pub to_x: i16,
    pub to_y: i16,
}

/// Q_TRADE_PATHS record (8 bytes, B4 fix-path): one plot of a route's path.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct gcq_trade_plot {
    pub route: i32,
    pub x: i16,
    pub y: i16,
}

/// Q_TOP_BAR record (64 bytes; v1: 60, UI v2 appends `tourism`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct gcq_top_bar {
    pub gold: i32,
    pub gold_per_turn: i32,
    pub science_per_turn: i32,
    pub research_tech: i16,
    pub research_turns: i16,
    pub research_progress: i32,
    pub research_cost: i32,
    pub culture: i32,
    pub culture_per_turn: i32,
    pub next_policy_cost: i32,
    pub happiness: i16,
    pub golden_age_turns: i16,
    pub faith: i32,
    pub faith_per_turn: i32,
    pub era: i16,
    pub flags: u16,
    pub golden_age_progress: i32,
    pub golden_age_threshold: i32,
    pub tourism: i32,
}

/// Q_CITY_BUILDS record (24 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct gcq_city_build {
    pub order_type: i8,
    pub reserved0: u8,
    pub item: i16,
    pub turns: i16,
    pub flags: u16,
    pub production_cost: i32,
    pub gold_cost: i32,
    pub reason: u32,
    pub faith_cost: i32,
}

/// Q_NOTIFICATIONS record (32 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct gcq_notification {
    pub id: i32,
    pub r#type: u32,
    pub x: i16,
    pub y: i16,
    pub data1: i32,
    pub data2: i32,
    pub turn: i16,
    pub dismissed: u8,
    pub reserved: u8,
    pub message: u32,
    pub summary: u32,
}

/// Q_CITY_QUEUE record (16 bytes, UI v2): one entry of a city's order queue.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct gcq_city_order {
    pub slot: i32,
    pub order: i8,
    pub reserved0: u8,
    pub item: i16,
    pub turns: i16,
    pub reserved1: u16,
    pub needed: i32,
}

/// Q_TECHS record (16 bytes, UI v2).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct gcq_tech {
    pub tech: i16,
    pub status: u8,
    pub queue: i8,
    pub turns: i16,
    pub reserved: u16,
    pub progress: i32,
    pub cost: i32,
}

/// Q_PLAYER_RESOURCES record (16 bytes, UI v2).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct gcq_resource {
    pub resource: i16,
    pub usage: u8,
    pub reserved: u8,
    pub available: i32,
    pub used: i32,
    pub total: i32,
}

/// Q_POLICY_BRANCHES record (8 bytes, UI v2).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct gcq_policy_branch {
    pub branch: i16,
    pub unlocked: u8,
    pub owned: u8,
    pub total: u8,
    pub reserved: [u8; 3],
}

/// Q_UNIT_PROMOTIONS record (4 bytes, UI v2).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct gcq_promotion {
    pub promotion: i16,
    pub reserved: u16,
}

/// `gc_ui_event`: one thing the engine told a replica's UI; every pointer is valid during the callback only.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct gc_ui_event {
    pub kind: i32,
    pub flags: u32,
    pub n_args: u32,
    pub args: [i32; GC_UI_MAX_ARGS],
    pub text: *const c_char,
    pub text2: *const c_char,
    pub data: *const u8,
    pub data_len: u32,
}

/// `gc_ui_sink`: the callback of a replica's UI events (must not call `gc_*`).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct gc_ui_sink {
    pub user: *mut c_void,
    pub event: Option<unsafe extern "C" fn(user: *mut c_void, ev: *const gc_ui_event)>,
}

/// `gc_lua_fn`: host function callable from game Lua.
pub type gc_lua_fn = Option<unsafe extern "C" fn(user: *mut c_void, L: *mut lua_State) -> i32>;

unsafe extern "C" {
    pub fn gc_abi_version() -> u32;
    pub fn gc_buf_release(buf: *mut gc_buf);
    pub fn gc_create(host: *const gc_host, err: *mut gc_error) -> *mut gc_instance;
    pub fn gc_destroy(inst: *mut gc_instance);
    pub fn gc_lua_register(
        inst: *mut gc_instance,
        table: *const c_char,
        name: *const c_char,
        fn_: gc_lua_fn,
        user: *mut c_void,
        err: *mut gc_error,
    ) -> i32;
    pub fn gc_new_game(
        inst: *mut gc_instance,
        setup_json: *const c_char,
        err: *mut gc_error,
    ) -> i32;
    pub fn gc_load_game(inst: *mut gc_instance, p: *const u8, n: u32, err: *mut gc_error) -> i32;
    pub fn gc_pump(
        inst: *mut gc_instance,
        max_slices: u32,
        out: *mut gc_pump_result,
        err: *mut gc_error,
    ) -> i32;
    pub fn gc_apply_command(
        inst: *mut gc_instance,
        cmd: *const gc_command,
        err: *mut gc_error,
    ) -> i32;
    pub fn gc_query(
        inst: *mut gc_instance,
        kind: i32,
        args: *const i32,
        argc: u32,
        out: *mut gc_buf,
        err: *mut gc_error,
    ) -> i32;
    pub fn gc_save(inst: *mut gc_instance, out: *mut gc_buf, err: *mut gc_error) -> i32;
    pub fn gc_state_digest(
        inst: *mut gc_instance,
        viewer_team: i32,
        out: *mut u64,
        err: *mut gc_error,
    ) -> i32;
    pub fn gc_build_id(inst: *mut gc_instance, out: *mut gc_buf, err: *mut gc_error) -> i32;
    pub fn gc_slot_control(
        inst: *mut gc_instance,
        player: i32,
        op: i32,
        nick: *const c_char,
        err: *mut gc_error,
    ) -> i32;
    // B4 gc-replica.
    pub fn gc_set_role(inst: *mut gc_instance, role: i32, player: i32, err: *mut gc_error) -> i32;
    pub fn gc_journal_take(
        inst: *mut gc_instance,
        flags: u32,
        out: *mut gc_buf,
        err: *mut gc_error,
    ) -> i32;
    pub fn gc_journal_apply(
        inst: *mut gc_instance,
        p: *const u8,
        n: u32,
        err: *mut gc_error,
    ) -> i32;
    pub fn gc_net_take(inst: *mut gc_instance, out: *mut gc_buf, err: *mut gc_error) -> i32;
    pub fn gc_net_submit(
        inst: *mut gc_instance,
        player: i32,
        p: *const u8,
        n: u32,
        err: *mut gc_error,
    ) -> i32;
    pub fn gc_resync(inst: *mut gc_instance, p: *const u8, n: u32, err: *mut gc_error) -> i32;
    // B4 M2 WO-1: the UI bridge of a replica.
    pub fn gc_lua_attach_ui(inst: *mut gc_instance, l: *mut lua_State, err: *mut gc_error) -> i32;
    pub fn gc_lua_invalidate_ui(
        inst: *mut gc_instance,
        l: *mut lua_State,
        err: *mut gc_error,
    ) -> i32;
    pub fn gc_set_ui_sink(
        inst: *mut gc_instance,
        sink: *const gc_ui_sink,
        err: *mut gc_error,
    ) -> i32;
    pub fn gc_ui_op(
        inst: *mut gc_instance,
        op: i32,
        input: *const i32,
        n_in: u32,
        out: *mut i32,
        n_out: *mut u32,
        err: *mut gc_error,
    ) -> i32;
    pub fn gc_ui_subscribe_game_event(
        inst: *mut gc_instance,
        name: *const c_char,
        err: *mut gc_error,
    ) -> i32;
    pub fn gc_ui_setup_graphical(inst: *mut gc_instance, err: *mut gc_error) -> i32;
}

// Spike (gc_spike.h).
unsafe extern "C" {
    pub fn gc_spike_ctor_count() -> i32;
    pub fn gc_spike_global_ok() -> i32;
    pub fn gc_spike_throw_catch(n: i32, msg: *mut c_char, cap: usize) -> i32;
    pub fn gc_spike_stl(msg: *mut c_char, cap: usize) -> i32;
    pub fn gc_spike_mix(x: i32) -> i32;
    pub fn gc_spike_lua_register(L: *mut lua_State) -> i32;
    pub fn gc_spike_lua_call(
        L: *mut lua_State,
        func: *const c_char,
        x: i32,
        out: *mut i32,
        err: *mut c_char,
        cap: usize,
    ) -> i32;
}

#[cfg(test)]
mod tests {
    use std::mem::{align_of, offset_of, size_of};

    use super::*;

    const P: usize = size_of::<usize>();

    // Sizes and offsets follow the C layout rules for the header: LP64 / wasm32 / LLP64 all have
    // 4-byte int and pointer-sized size_t, and the pointer fields below are aligned by construction.
    #[test]
    fn struct_layouts_match_the_header() {
        assert_eq!(size_of::<gc_buf>(), 4 * P);
        assert_eq!(offset_of!(gc_buf, len), P);
        assert_eq!(offset_of!(gc_buf, release), 2 * P);
        assert_eq!(offset_of!(gc_buf, owner), 3 * P);

        assert_eq!(size_of::<gc_error>(), 264);
        assert_eq!(align_of::<gc_error>(), 4);
        assert_eq!(offset_of!(gc_error, message), 4);
        assert_eq!(offset_of!(gc_error, reason), 260);

        // int32 kind; uint32 argc; then three pointers.
        let ev_head = 8usize.next_multiple_of(P);
        assert_eq!(size_of::<gc_event>(), ev_head + 3 * P);
        assert_eq!(offset_of!(gc_event, argc), 4);
        assert_eq!(offset_of!(gc_event, args), ev_head);
        assert_eq!(offset_of!(gc_event, text), ev_head + P);
        assert_eq!(offset_of!(gc_event, text2), ev_head + 2 * P);

        assert_eq!(size_of::<gc_host>(), 4 * P);
        assert_eq!(offset_of!(gc_host, game_event), 3 * P);

        assert_eq!(size_of::<gc_pump_result>(), 24);
        assert_eq!(offset_of!(gc_pump_result, slices_used), 16);
        assert_eq!(offset_of!(gc_pump_result, humans_left), 20);

        // uint16 kind; uint8 player; uint8 reserved; uint32 argc; then two pointers.
        let cmd_head = 8usize.next_multiple_of(P);
        assert_eq!(size_of::<gc_command>(), cmd_head + 2 * P);
        assert_eq!(offset_of!(gc_command, player), 2);
        assert_eq!(offset_of!(gc_command, reserved), 3);
        assert_eq!(offset_of!(gc_command, argc), 4);
        assert_eq!(offset_of!(gc_command, args), cmd_head);
        assert_eq!(offset_of!(gc_command, text), cmd_head + P);

        assert_eq!(size_of::<gcq_header>(), 24);
        assert_eq!(align_of::<gcq_header>(), 4);
        assert_eq!(offset_of!(gcq_header, format), 4);
        assert_eq!(offset_of!(gcq_header, kind), 6);
        assert_eq!(offset_of!(gcq_header, count), 8);
        assert_eq!(offset_of!(gcq_header, record_size), 12);
        assert_eq!(offset_of!(gcq_header, strings_offset), 16);
        assert_eq!(offset_of!(gcq_header, strings_size), 20);

        assert_eq!(size_of::<gcq_game>(), 56);
        assert_eq!(offset_of!(gcq_game, blocker_x), 16);
        assert_eq!(offset_of!(gcq_game, human_player), 24);
        assert_eq!(offset_of!(gcq_game, blocker), 28);
        assert_eq!(offset_of!(gcq_game, victory_type), 33);
        assert_eq!(offset_of!(gcq_game, reserved), 34);
        assert_eq!(offset_of!(gcq_game, game_type), 36);
        assert_eq!(offset_of!(gcq_game, humans_left), 40);
        assert_eq!(offset_of!(gcq_game, unsafe_reason), 42);
        assert_eq!(offset_of!(gcq_game, turn_len_s), 44);
        assert_eq!(offset_of!(gcq_game, turn_base_s), 48);
        assert_eq!(offset_of!(gcq_game, map_seed), 52);

        assert_eq!(size_of::<gcq_plot>(), 32);
        assert_eq!(offset_of!(gcq_plot, build_type), 28);
        assert_eq!(offset_of!(gcq_plot, build_pct), 30);
        assert_eq!(offset_of!(gcq_plot, flags), 4);
        assert_eq!(offset_of!(gcq_plot, terrain), 8);
        assert_eq!(offset_of!(gcq_plot, resource_qty), 14);
        assert_eq!(offset_of!(gcq_plot, improvement), 16);
        assert_eq!(offset_of!(gcq_plot, owner), 20);
        assert_eq!(offset_of!(gcq_plot, yields), 22);

        assert_eq!(size_of::<gcq_player>(), 48);
        assert_eq!(offset_of!(gcq_player, nick), 28);
        assert_eq!(offset_of!(gcq_player, slot), 32);
        assert_eq!(offset_of!(gcq_player, reserved2), 36);
        assert_eq!(offset_of!(gcq_player, art_techs), 40);
        assert_eq!(offset_of!(gcq_player, era), 24);
        assert_eq!(offset_of!(gcq_player, flags), 6);
        assert_eq!(offset_of!(gcq_player, color_primary), 8);
        assert_eq!(offset_of!(gcq_player, civ_name), 20);

        assert_eq!(size_of::<gcq_catalog>(), 36);
        assert_eq!(offset_of!(gcq_catalog, r#type), 8);
        assert_eq!(offset_of!(gcq_catalog, strategic_view), 32);

        assert_eq!(size_of::<gcq_unit>(), 44);
        assert_eq!(offset_of!(gcq_unit, transport_id), 36);
        assert_eq!(offset_of!(gcq_unit, build_type), 40);
        assert_eq!(offset_of!(gcq_unit, facing), 42);
        assert_eq!(offset_of!(gcq_unit, unit_type), 6);
        assert_eq!(offset_of!(gcq_unit, ranged), 22);
        assert_eq!(offset_of!(gcq_unit, range), 24);
        assert_eq!(offset_of!(gcq_unit, experience), 28);
        assert_eq!(offset_of!(gcq_unit, name), 32);
        assert_eq!(size_of::<gcq_unit_action>(), 12);
        assert_eq!(offset_of!(gcq_unit_action, reason), 4);
        assert_eq!(offset_of!(gcq_unit_action, cost), 8);
        assert_eq!(size_of::<gcq_path_node>(), 12);
        assert_eq!(offset_of!(gcq_path_node, flags), 8);
        assert_eq!(size_of::<gcq_catalog_timer>(), 24);
        assert_eq!(offset_of!(gcq_catalog_timer, base_time), 12);
        assert_eq!(offset_of!(gcq_catalog_timer, unit_resource), 20);
        assert_eq!(size_of::<gcq_catalog_ext>(), 32);
        assert_eq!(offset_of!(gcq_catalog_ext, s), 16);
        assert_eq!(size_of::<gcq_turn>(), 24);
        assert_eq!(offset_of!(gcq_turn, flags), 4);
        assert_eq!(offset_of!(gcq_turn, turn_order), 6);
        assert_eq!(offset_of!(gcq_turn, blocker_unit), 8);
        assert_eq!(offset_of!(gcq_turn, blocker_city), 12);
        assert_eq!(offset_of!(gcq_turn, blocker_x), 16);
        assert_eq!(offset_of!(gcq_turn, nick), 20);
        assert_eq!(size_of::<gcq_diplo>(), 8);
        assert_eq!(offset_of!(gcq_diplo, war_locked_turns), 4);
        assert_eq!(size_of::<gcq_tree_node>(), 16);
        assert_eq!(offset_of!(gcq_tree_node, moves_left), 8);
        assert_eq!(offset_of!(gcq_tree_node, turn), 10);
        assert_eq!(offset_of!(gcq_tree_node, flags), 12);
        assert_eq!(size_of::<gcq_plot_basis>(), 16);
        assert_eq!(offset_of!(gcq_plot_basis, terrain), 6);
        assert_eq!(offset_of!(gcq_plot_basis, feature), 8);
        assert_eq!(offset_of!(gcq_plot_basis, flags), 10);
        assert_eq!(offset_of!(gcq_plot_basis, rivers), 12);
        assert_eq!(offset_of!(gcq_plot_basis, river_flow), 13);
        assert_eq!(size_of::<gcq_combat_preview>(), 20);
        assert_eq!(offset_of!(gcq_combat_preview, attacker_strength), 4);
        assert_eq!(offset_of!(gcq_combat_preview, target_hp), 12);
        assert_eq!(offset_of!(gcq_combat_preview, flags), 16);
        assert_eq!(size_of::<gcq_plot_dirty>(), 4);
        assert_eq!(size_of::<gcq_range_target>(), 8);
        assert_eq!(offset_of!(gcq_range_target, flags), 4);
        assert_eq!(size_of::<gcq_city>(), 60);
        assert_eq!(offset_of!(gcq_city, founder), 52);
        assert_eq!(offset_of!(gcq_city, overflow), 56);
        assert_eq!(size_of::<gcq_city_art>(), 12);
        assert_eq!(offset_of!(gcq_city_art, building), 6);
        assert_eq!(size_of::<gcq_trade_route>(), 16);
        assert_eq!(offset_of!(gcq_trade_route, from_x), 8);
        assert_eq!(size_of::<gcq_trade_plot>(), 8);
        assert_eq!(offset_of!(gcq_trade_plot, y), 6);
        assert_eq!(offset_of!(gcq_city, production_stored), 28);
        assert_eq!(offset_of!(gcq_city, yields), 36);
        assert_eq!(offset_of!(gcq_city, name), 48);
        assert_eq!(size_of::<gcq_top_bar>(), 64);
        assert_eq!(offset_of!(gcq_top_bar, research_progress), 16);
        assert_eq!(offset_of!(gcq_top_bar, happiness), 36);
        assert_eq!(offset_of!(gcq_top_bar, era), 48);
        assert_eq!(offset_of!(gcq_top_bar, golden_age_threshold), 56);
        assert_eq!(offset_of!(gcq_top_bar, tourism), 60);
        // UI v2 read-only queries.
        assert_eq!(size_of::<gcq_city_order>(), 16);
        assert_eq!(offset_of!(gcq_city_order, order), 4);
        assert_eq!(offset_of!(gcq_city_order, item), 6);
        assert_eq!(offset_of!(gcq_city_order, turns), 8);
        assert_eq!(offset_of!(gcq_city_order, needed), 12);
        assert_eq!(size_of::<gcq_tech>(), 16);
        assert_eq!(offset_of!(gcq_tech, status), 2);
        assert_eq!(offset_of!(gcq_tech, queue), 3);
        assert_eq!(offset_of!(gcq_tech, turns), 4);
        assert_eq!(offset_of!(gcq_tech, progress), 8);
        assert_eq!(offset_of!(gcq_tech, cost), 12);
        assert_eq!(size_of::<gcq_resource>(), 16);
        assert_eq!(offset_of!(gcq_resource, usage), 2);
        assert_eq!(offset_of!(gcq_resource, available), 4);
        assert_eq!(offset_of!(gcq_resource, used), 8);
        assert_eq!(offset_of!(gcq_resource, total), 12);
        assert_eq!(size_of::<gcq_policy_branch>(), 8);
        assert_eq!(offset_of!(gcq_policy_branch, unlocked), 2);
        assert_eq!(offset_of!(gcq_policy_branch, owned), 3);
        assert_eq!(offset_of!(gcq_policy_branch, total), 4);
        assert_eq!(size_of::<gcq_promotion>(), 4);
        assert_eq!(size_of::<gcq_city_build>(), 24);
        assert_eq!(offset_of!(gcq_city_build, production_cost), 8);
        assert_eq!(offset_of!(gcq_city_build, reason), 16);
        assert_eq!(offset_of!(gcq_city_build, faith_cost), 20);

        assert_eq!(size_of::<gcq_notification>(), 32);
        assert_eq!(offset_of!(gcq_notification, r#type), 4);
        assert_eq!(offset_of!(gcq_notification, x), 8);
        assert_eq!(offset_of!(gcq_notification, data1), 12);
        assert_eq!(offset_of!(gcq_notification, data2), 16);
        assert_eq!(offset_of!(gcq_notification, turn), 20);
        assert_eq!(offset_of!(gcq_notification, dismissed), 22);
        assert_eq!(offset_of!(gcq_notification, message), 24);
        assert_eq!(offset_of!(gcq_notification, summary), 28);
    }

    /// The constants repeat the header; the header copy in the crate is the one the stub is built from.
    #[test]
    fn constants_match_the_header_copy() {
        let header = include_str!("../cpp/include/gamecore_abi.h");
        let define = |name: &str| -> i64 {
            let prefix = format!("#define {name} ");
            let line = header
                .lines()
                .find(|l| l.starts_with(&prefix))
                .unwrap_or_else(|| panic!("no #define {name} in the header"));
            let value = line[prefix.len()..]
                .split("/*")
                .next()
                .unwrap()
                .trim()
                .trim_matches(|c| c == '(' || c == ')')
                .trim_end_matches('u');
            match value.strip_prefix("0x") {
                Some(hex) => i64::from_str_radix(hex, 16),
                None => value.parse(),
            }
            .unwrap_or_else(|_| panic!("{name}: cannot parse {value:?}"))
        };
        let table: &[(&str, i64)] = &[
            ("GC_ABI_VERSION", GC_ABI_VERSION as i64),
            ("GC_OK", GC_OK as i64),
            ("GC_ERR_ALREADY_EXISTS", GC_ERR_ALREADY_EXISTS as i64),
            ("GC_ERR_INVALID_ARG", GC_ERR_INVALID_ARG as i64),
            ("GC_ERR_WRONG_MODE", GC_ERR_WRONG_MODE as i64),
            ("GC_ERR_BAD_DATA", GC_ERR_BAD_DATA as i64),
            ("GC_ERR_REJECTED", GC_ERR_REJECTED as i64),
            ("GC_ERR_NOT_STARTED", GC_ERR_NOT_STARTED as i64),
            ("GC_ERR_NOT_FOUND", GC_ERR_NOT_FOUND as i64),
            ("GC_ERR_STUCK", GC_ERR_STUCK as i64),
            ("GC_ERR_NOT_IMPLEMENTED", GC_ERR_NOT_IMPLEMENTED as i64),
            ("GC_ERR_DIVERGED", GC_ERR_DIVERGED as i64),
            ("GC_ERR_INTERNAL", GC_ERR_INTERNAL as i64),
            ("GC_LOG_ERROR", GC_LOG_ERROR as i64),
            ("GC_LOG_WARN", GC_LOG_WARN as i64),
            ("GC_LOG_INFO", GC_LOG_INFO as i64),
            ("GC_LOG_DEBUG", GC_LOG_DEBUG as i64),
            ("GC_LOG_TRACE", GC_LOG_TRACE as i64),
            ("GC_EVENT_TURN_STARTED", GC_EVENT_TURN_STARTED as i64),
            (
                "GC_EVENT_PLAYER_TURN_STARTED",
                GC_EVENT_PLAYER_TURN_STARTED as i64,
            ),
            (
                "GC_EVENT_NOTIFICATION_ADDED",
                GC_EVENT_NOTIFICATION_ADDED as i64,
            ),
            (
                "GC_EVENT_NOTIFICATION_REMOVED",
                GC_EVENT_NOTIFICATION_REMOVED as i64,
            ),
            ("GC_EVENT_AUTO_CHOICE", GC_EVENT_AUTO_CHOICE as i64),
            (
                "GC_EVENT_DIPLO_AUTO_DECLINED",
                GC_EVENT_DIPLO_AUTO_DECLINED as i64,
            ),
            ("GC_EVENT_POPUP_INFO", GC_EVENT_POPUP_INFO as i64),
            ("GC_EVENT_GAME_OVER", GC_EVENT_GAME_OVER as i64),
            ("GC_EVENT_LUA_ERROR", GC_EVENT_LUA_ERROR as i64),
            ("GC_EVENT_WAR_STATE", GC_EVENT_WAR_STATE as i64),
            ("GC_EVENT_MESSAGE", GC_EVENT_MESSAGE as i64),
            ("GC_PUMP_WAITING_HUMAN", GC_PUMP_WAITING_HUMAN as i64),
            ("GC_PUMP_AI_RUNNING", GC_PUMP_AI_RUNNING as i64),
            ("GC_PUMP_GAME_OVER", GC_PUMP_GAME_OVER as i64),
            ("GC_PUMP_STUCK", GC_PUMP_STUCK as i64),
            ("GC_CMD_UNIT_MOVE_TO", GC_CMD_UNIT_MOVE_TO as i64),
            ("GC_CMD_UNIT_RANGE_ATTACK", GC_CMD_UNIT_RANGE_ATTACK as i64),
            ("GC_CMD_UNIT_FOUND_CITY", GC_CMD_UNIT_FOUND_CITY as i64),
            ("GC_CMD_UNIT_FORTIFY", GC_CMD_UNIT_FORTIFY as i64),
            ("GC_CMD_UNIT_SLEEP", GC_CMD_UNIT_SLEEP as i64),
            ("GC_CMD_UNIT_SKIP", GC_CMD_UNIT_SKIP as i64),
            ("GC_CMD_UNIT_HEAL", GC_CMD_UNIT_HEAL as i64),
            (
                "GC_CMD_UNIT_AUTOMATE_BUILD",
                GC_CMD_UNIT_AUTOMATE_BUILD as i64,
            ),
            (
                "GC_CMD_UNIT_AUTOMATE_EXPLORE",
                GC_CMD_UNIT_AUTOMATE_EXPLORE as i64,
            ),
            ("GC_CMD_UNIT_CANCEL", GC_CMD_UNIT_CANCEL as i64),
            ("GC_CMD_UNIT_DISBAND", GC_CMD_UNIT_DISBAND as i64),
            ("GC_CMD_UNIT_WAKE", GC_CMD_UNIT_WAKE as i64),
            (
                "GC_CMD_CITY_SET_PRODUCTION",
                GC_CMD_CITY_SET_PRODUCTION as i64,
            ),
            ("GC_CMD_CITY_PURCHASE", GC_CMD_CITY_PURCHASE as i64),
            ("GC_CMD_CITY_RANGE_ATTACK", GC_CMD_CITY_RANGE_ATTACK as i64),
            ("GC_CMD_DECLARE_WAR", GC_CMD_DECLARE_WAR as i64),
            ("GC_CMD_END_TURN", GC_CMD_END_TURN as i64),
            ("GC_CMD_FORCE_END_TURN", GC_CMD_FORCE_END_TURN as i64),
            ("GC_Q_GAME", GC_Q_GAME as i64),
            ("GC_Q_PLOTS", GC_Q_PLOTS as i64),
            ("GC_Q_UNITS", GC_Q_UNITS as i64),
            ("GC_Q_CITIES", GC_Q_CITIES as i64),
            ("GC_Q_PLAYERS", GC_Q_PLAYERS as i64),
            ("GC_Q_TOP_BAR", GC_Q_TOP_BAR as i64),
            ("GC_Q_CITY_BUILDS", GC_Q_CITY_BUILDS as i64),
            ("GC_Q_UNIT_ACTIONS", GC_Q_UNIT_ACTIONS as i64),
            ("GC_Q_PATH", GC_Q_PATH as i64),
            ("GC_Q_REACHABLE", GC_Q_REACHABLE as i64),
            ("GC_Q_RANGE_TARGETS", GC_Q_RANGE_TARGETS as i64),
            ("GC_Q_NOTIFICATIONS", GC_Q_NOTIFICATIONS as i64),
            ("GC_Q_CATALOG", GC_Q_CATALOG as i64),
            ("GCQ_NO_STRING", GCQ_NO_STRING as i64),
            (
                "GC_GAME_STATUS_WAITING_HUMAN",
                GC_GAME_STATUS_WAITING_HUMAN as i64,
            ),
            (
                "GC_GAME_STATUS_AI_RUNNING",
                GC_GAME_STATUS_AI_RUNNING as i64,
            ),
            ("GC_GAME_STATUS_GAME_OVER", GC_GAME_STATUS_GAME_OVER as i64),
            ("GC_GAME_STATUS_STUCK", GC_GAME_STATUS_STUCK as i64),
            ("GC_PLOT_F_REVEALED", GC_PLOT_F_REVEALED as i64),
            ("GC_PLOT_F_VISIBLE", GC_PLOT_F_VISIBLE as i64),
            ("GC_PLOT_F_CITY", GC_PLOT_F_CITY as i64),
            ("GC_PLOT_F_LAKE", GC_PLOT_F_LAKE as i64),
            ("GC_PLOT_F_COASTAL", GC_PLOT_F_COASTAL as i64),
            ("GC_PLOT_F_RIVER_SHIFT", GC_PLOT_F_RIVER_SHIFT as i64),
            ("GC_PLOT_F_RIVER_MASK", GC_PLOT_F_RIVER_MASK as i64),
            ("GC_PLOT_F_PILLAGED", GC_PLOT_F_PILLAGED as i64),
            ("GC_PLOT_F_NATURAL_WONDER", GC_PLOT_F_NATURAL_WONDER as i64),
            ("GC_PLOT_F_WORKED", GC_PLOT_F_WORKED as i64),
            ("GC_PLOT_F_ROUTE_PILLAGED", GC_PLOT_F_ROUTE_PILLAGED as i64),
            ("GC_PLOT_TYPE_MOUNTAIN", GC_PLOT_TYPE_MOUNTAIN as i64),
            ("GC_PLOT_TYPE_HILLS", GC_PLOT_TYPE_HILLS as i64),
            ("GC_PLOT_TYPE_LAND", GC_PLOT_TYPE_LAND as i64),
            ("GC_PLOT_TYPE_OCEAN", GC_PLOT_TYPE_OCEAN as i64),
            ("GC_PLAYER_F_ALIVE", GC_PLAYER_F_ALIVE as i64),
            ("GC_PLAYER_F_HUMAN", GC_PLAYER_F_HUMAN as i64),
            ("GC_PLAYER_F_MINOR", GC_PLAYER_F_MINOR as i64),
            ("GC_PLAYER_F_BARBARIAN", GC_PLAYER_F_BARBARIAN as i64),
            ("GC_PLAYER_F_MET", GC_PLAYER_F_MET as i64),
            (
                "GC_PLAYER_F_AT_WAR_WITH_VIEWER",
                GC_PLAYER_F_AT_WAR_WITH_VIEWER as i64,
            ),
            ("GC_CATALOG_UNITS", GC_CATALOG_UNITS as i64),
            ("GC_CATALOG_BUILDINGS", GC_CATALOG_BUILDINGS as i64),
            ("GC_CATALOG_PROJECTS", GC_CATALOG_PROJECTS as i64),
            ("GC_CATALOG_TECHNOLOGIES", GC_CATALOG_TECHNOLOGIES as i64),
            ("GC_CATALOG_POLICIES", GC_CATALOG_POLICIES as i64),
            ("GC_CATALOG_TERRAINS", GC_CATALOG_TERRAINS as i64),
            ("GC_CATALOG_FEATURES", GC_CATALOG_FEATURES as i64),
            ("GC_CATALOG_RESOURCES", GC_CATALOG_RESOURCES as i64),
            ("GC_CATALOG_IMPROVEMENTS", GC_CATALOG_IMPROVEMENTS as i64),
            ("GC_CATALOG_ROUTES", GC_CATALOG_ROUTES as i64),
            ("GC_CATALOG_CIVILIZATIONS", GC_CATALOG_CIVILIZATIONS as i64),
            ("GC_CATALOG_LEADERS", GC_CATALOG_LEADERS as i64),
            ("GC_CATALOG_ERAS", GC_CATALOG_ERAS as i64),
            ("GC_CATALOG_HANDICAPS", GC_CATALOG_HANDICAPS as i64),
            ("GC_CATALOG_PROMOTIONS", GC_CATALOG_PROMOTIONS as i64),
            ("GC_CATALOG_TABLE_COUNT", GC_CATALOG_TABLE_COUNT as i64),
            ("GC_UNIT_F_CAN_MOVE", GC_UNIT_F_CAN_MOVE as i64),
            ("GC_UNIT_F_NEEDS_ORDERS", GC_UNIT_F_NEEDS_ORDERS as i64),
            ("GC_UNIT_F_FORTIFIED", GC_UNIT_F_FORTIFIED as i64),
            ("GC_UNIT_F_SLEEPING", GC_UNIT_F_SLEEPING as i64),
            ("GC_UNIT_F_AUTOMATED", GC_UNIT_F_AUTOMATED as i64),
            ("GC_UNIT_F_EMBARKED", GC_UNIT_F_EMBARKED as i64),
            ("GC_UNIT_F_CIVILIAN", GC_UNIT_F_CIVILIAN as i64),
            ("GC_UNIT_F_CAN_PROMOTE", GC_UNIT_F_CAN_PROMOTE as i64),
            ("GC_UNIT_F_GARRISONED", GC_UNIT_F_GARRISONED as i64),
            ("GC_UNIT_F_CARGO", GC_UNIT_F_CARGO as i64),
            ("GC_ORDER_TRAIN", GC_ORDER_TRAIN as i64),
            ("GC_ORDER_CONSTRUCT", GC_ORDER_CONSTRUCT as i64),
            ("GC_ORDER_CREATE", GC_ORDER_CREATE as i64),
            ("GC_ORDER_MAINTAIN", GC_ORDER_MAINTAIN as i64),
            ("GC_YIELD_FOOD", GC_YIELD_FOOD as i64),
            ("GC_YIELD_PRODUCTION", GC_YIELD_PRODUCTION as i64),
            ("GC_YIELD_GOLD", GC_YIELD_GOLD as i64),
            ("GC_YIELD_SCIENCE", GC_YIELD_SCIENCE as i64),
            ("GC_YIELD_CULTURE", GC_YIELD_CULTURE as i64),
            ("GC_YIELD_FAITH", GC_YIELD_FAITH as i64),
            ("GC_CITY_F_CAPITAL", GC_CITY_F_CAPITAL as i64),
            ("GC_CITY_F_PUPPET", GC_CITY_F_PUPPET as i64),
            ("GC_CITY_F_RAZING", GC_CITY_F_RAZING as i64),
            ("GC_CITY_F_OCCUPIED", GC_CITY_F_OCCUPIED as i64),
            (
                "GC_CITY_F_CAN_RANGE_ATTACK",
                GC_CITY_F_CAN_RANGE_ATTACK as i64,
            ),
            ("GC_CITY_F_VISIBLE", GC_CITY_F_VISIBLE as i64),
            ("GC_CITY_F_RESISTANCE", GC_CITY_F_RESISTANCE as i64),
            ("GC_TOPBAR_F_UNHAPPY", GC_TOPBAR_F_UNHAPPY as i64),
            ("GC_TOPBAR_F_VERY_UNHAPPY", GC_TOPBAR_F_VERY_UNHAPPY as i64),
            ("GC_TOPBAR_F_GOLDEN_AGE", GC_TOPBAR_F_GOLDEN_AGE as i64),
            ("GC_BUILD_F_WONDER", GC_BUILD_F_WONDER as i64),
            ("GC_BUILD_F_CURRENT", GC_BUILD_F_CURRENT as i64),
            ("GC_BUILD_F_CAN_PURCHASE", GC_BUILD_F_CAN_PURCHASE as i64),
            ("GC_BUILD_F_ENABLED", GC_BUILD_F_ENABLED as i64),
            ("GC_RANGE_TARGET_F_CITY", GC_RANGE_TARGET_F_CITY as i64),
            // v2
            (
                "GC_EVENT_PLAYER_TURN_ENDED",
                GC_EVENT_PLAYER_TURN_ENDED as i64,
            ),
            ("GC_EVENT_SLOT_CHANGED", GC_EVENT_SLOT_CHANGED as i64),
            (
                "GC_EVENT_HUMAN_PHASE_STARTED",
                GC_EVENT_HUMAN_PHASE_STARTED as i64,
            ),
            (
                "GC_EVENT_HUMAN_WAR_DECLARED",
                GC_EVENT_HUMAN_WAR_DECLARED as i64,
            ),
            (
                "GC_EVENT_PLAYER_TURN_STATUS",
                GC_EVENT_PLAYER_TURN_STATUS as i64,
            ),
            ("GC_REASON_NONE", GC_REASON_NONE as i64),
            ("GC_REASON_NOT_HUMAN", GC_REASON_NOT_HUMAN as i64),
            ("GC_REASON_NOT_YOUR_TURN", GC_REASON_NOT_YOUR_TURN as i64),
            ("GC_REASON_TURN_ENDED", GC_REASON_TURN_ENDED as i64),
            ("GC_REASON_AI_RUNNING", GC_REASON_AI_RUNNING as i64),
            ("GC_REASON_UNIT_BUSY", GC_REASON_UNIT_BUSY as i64),
            ("GC_REASON_BAD_TARGET", GC_REASON_BAD_TARGET as i64),
            ("GC_REASON_RULE", GC_REASON_RULE as i64),
            ("GC_REASON_BLOCKED", GC_REASON_BLOCKED as i64),
            ("GC_REASON_PUMP_FIRST", GC_REASON_PUMP_FIRST as i64),
            ("GC_REASON_NOT_SAFE_POINT", GC_REASON_NOT_SAFE_POINT as i64),
            ("GC_REASON_SLOT_STATE", GC_REASON_SLOT_STATE as i64),
            ("GC_SLOT_CONNECT", GC_SLOT_CONNECT as i64),
            ("GC_SLOT_DISCONNECT", GC_SLOT_DISCONNECT as i64),
            ("GC_SLOT_KICK", GC_SLOT_KICK as i64),
            ("GC_ROLE_PLAIN", GC_ROLE_PLAIN as i64),
            ("GC_ROLE_SERVER", GC_ROLE_SERVER as i64),
            ("GC_ROLE_REPLICA", GC_ROLE_REPLICA as i64),
            ("GC_JOURNAL_COMMAND", GC_JOURNAL_COMMAND as i64),
            ("GC_JOURNAL_NET", GC_JOURNAL_NET as i64),
            ("GC_JOURNAL_PUMP", GC_JOURNAL_PUMP as i64),
            ("GC_JOURNAL_SLOT", GC_JOURNAL_SLOT as i64),
            ("GC_JOURNAL_CHECK", GC_JOURNAL_CHECK as i64),
            ("GC_JOURNAL_TAKE_CHECK", GC_JOURNAL_TAKE_CHECK as i64),
            ("GC_NET_ARGS", GC_NET_ARGS as i64),
            ("GC_UI_MAX_ARGS", GC_UI_MAX_ARGS as i64),
            ("GC_UI_F_REBUILD", GC_UI_F_REBUILD as i64),
            ("GC_UI_UNIT_INTS", GC_UI_UNIT_INTS as i64),
            ("GC_UI_CITY_INTS", GC_UI_CITY_INTS as i64),
            ("GC_UI_EV_UNIT_CREATED", GC_UI_EV_UNIT_CREATED as i64),
            ("GC_UI_EV_UNIT_MOVED", GC_UI_EV_UNIT_MOVED as i64),
            ("GC_UI_EV_UNIT_TELEPORTED", GC_UI_EV_UNIT_TELEPORTED as i64),
            ("GC_UI_EV_UNIT_DESTROYED", GC_UI_EV_UNIT_DESTROYED as i64),
            (
                "GC_UI_EV_UNIT_DESTROYED_IN_COMBAT",
                GC_UI_EV_UNIT_DESTROYED_IN_COMBAT as i64,
            ),
            ("GC_UI_EV_UNIT_SET_DAMAGE", GC_UI_EV_UNIT_SET_DAMAGE as i64),
            ("GC_UI_EV_UNIT_FORTIFY", GC_UI_EV_UNIT_FORTIFY as i64),
            ("GC_UI_EV_UNIT_WORK", GC_UI_EV_UNIT_WORK as i64),
            ("GC_UI_EV_UNIT_PARADROP", GC_UI_EV_UNIT_PARADROP as i64),
            ("GC_UI_EV_UNIT_ACTIVATE", GC_UI_EV_UNIT_ACTIVATE as i64),
            ("GC_UI_EV_UNIT_EMBARK", GC_UI_EV_UNIT_EMBARK as i64),
            ("GC_UI_EV_UNIT_GARRISON", GC_UI_EV_UNIT_GARRISON as i64),
            (
                "GC_UI_EV_UNIT_SHOULD_DIM_FLAG",
                GC_UI_EV_UNIT_SHOULD_DIM_FLAG as i64,
            ),
            (
                "GC_UI_EV_UNIT_MARK_THREATENING",
                GC_UI_EV_UNIT_MARK_THREATENING as i64,
            ),
            ("GC_UI_EV_UNIT_VISIBILITY", GC_UI_EV_UNIT_VISIBILITY as i64),
            ("GC_UI_EV_UNIT_COMBAT", GC_UI_EV_UNIT_COMBAT as i64),
            (
                "GC_UI_EV_UNIT_MISSION_END",
                GC_UI_EV_UNIT_MISSION_END as i64,
            ),
            ("GC_UI_EV_UNIT_REBASED", GC_UI_EV_UNIT_REBASED as i64),
            (
                "GC_UI_EV_UNIT_RESET_ANIMATION",
                GC_UI_EV_UNIT_RESET_ANIMATION as i64,
            ),
            ("GC_UI_EV_CITY_CREATED", GC_UI_EV_CITY_CREATED as i64),
            ("GC_UI_EV_CITY_DESTROYED", GC_UI_EV_CITY_DESTROYED as i64),
            ("GC_UI_EV_CITY_CAPTURED", GC_UI_EV_CITY_CAPTURED as i64),
            ("GC_UI_EV_CITY_SET_DAMAGE", GC_UI_EV_CITY_SET_DAMAGE as i64),
            (
                "GC_UI_EV_CITY_POPULATION_CHANGED",
                GC_UI_EV_CITY_POPULATION_CHANGED as i64,
            ),
            ("GC_UI_EV_CITY_COMBAT", GC_UI_EV_CITY_COMBAT as i64),
            ("GC_UI_EV_ERA_CHANGED", GC_UI_EV_ERA_CHANGED as i64),
            ("GC_UI_EV_BRIDGE_CHANGED", GC_UI_EV_BRIDGE_CHANGED as i64),
            ("GC_UI_EV_TECH_ACQUIRED", GC_UI_EV_TECH_ACQUIRED as i64),
            ("GC_UI_EV_FEATURE_CHANGED", GC_UI_EV_FEATURE_CHANGED as i64),
            (
                "GC_UI_EV_PLOT_STATE_CHANGE",
                GC_UI_EV_PLOT_STATE_CHANGE as i64,
            ),
            (
                "GC_UI_EV_PLOT_ICON_STATE_CHANGE",
                GC_UI_EV_PLOT_ICON_STATE_CHANGE as i64,
            ),
            (
                "GC_UI_EV_NATURAL_WONDER_REVEALED",
                GC_UI_EV_NATURAL_WONDER_REVEALED as i64,
            ),
            ("GC_UI_EV_WALL_CREATED", GC_UI_EV_WALL_CREATED as i64),
            ("GC_UI_EV_DO_FX", GC_UI_EV_DO_FX as i64),
            ("GC_UI_EV_FOW_CHANGED", GC_UI_EV_FOW_CHANGED as i64),
            (
                "GC_UI_EV_YIELD_MIGHT_HAVE_CHANGED",
                GC_UI_EV_YIELD_MIGHT_HAVE_CHANGED as i64,
            ),
            (
                "GC_UI_EV_WAR_STATE_CHANGED",
                GC_UI_EV_WAR_STATE_CHANGED as i64,
            ),
            ("GC_UI_EV_MET_TEAM", GC_UI_EV_MET_TEAM as i64),
            (
                "GC_UI_EV_DIPLO_AI_LEADER_MESSAGE",
                GC_UI_EV_DIPLO_AI_LEADER_MESSAGE as i64,
            ),
            (
                "GC_UI_EV_CLEAR_DIPLO_TRADE_TABLE",
                GC_UI_EV_CLEAR_DIPLO_TRADE_TABLE as i64,
            ),
            (
                "GC_UI_EV_NOTIFY_AI_LEADERS_IN_GAME",
                GC_UI_EV_NOTIFY_AI_LEADERS_IN_GAME as i64,
            ),
            (
                "GC_UI_EV_NOTIFY_SPECIFIC_AI_LEADER",
                GC_UI_EV_NOTIFY_SPECIFIC_AI_LEADER as i64,
            ),
            ("GC_UI_EV_WONDER_CREATED", GC_UI_EV_WONDER_CREATED as i64),
            ("GC_UI_EV_WONDER_EDITED", GC_UI_EV_WONDER_EDITED as i64),
            ("GC_UI_EV_WONDER_REMOVED", GC_UI_EV_WONDER_REMOVED as i64),
            (
                "GC_UI_EV_SPACESHIP_CREATED",
                GC_UI_EV_SPACESHIP_CREATED as i64,
            ),
            (
                "GC_UI_EV_SPACESHIP_EDITED",
                GC_UI_EV_SPACESHIP_EDITED as i64,
            ),
            (
                "GC_UI_EV_SPACESHIP_REMOVED",
                GC_UI_EV_SPACESHIP_REMOVED as i64,
            ),
            (
                "GC_UI_EV_OPEN_OPTIONS_SCREEN",
                GC_UI_EV_OPEN_OPTIONS_SCREEN as i64,
            ),
            (
                "GC_UI_EV_SEARCH_FOR_PEDIA_ENTRY",
                GC_UI_EV_SEARCH_FOR_PEDIA_ENTRY as i64,
            ),
            (
                "GC_UI_EV_OPEN_INFO_CORNER",
                GC_UI_EV_OPEN_INFO_CORNER as i64,
            ),
            ("GC_UI_EV_WORLD_ANCHOR", GC_UI_EV_WORLD_ANCHOR as i64),
            (
                "GC_UI_EV_MINIMAP_UNIT_SELECT",
                GC_UI_EV_MINIMAP_UNIT_SELECT as i64,
            ),
            (
                "GC_UI_EV_MINIMAP_NOTIFICATION",
                GC_UI_EV_MINIMAP_NOTIFICATION as i64,
            ),
            (
                "GC_UI_EV_GOLDEN_AGE_STARTED",
                GC_UI_EV_GOLDEN_AGE_STARTED as i64,
            ),
            (
                "GC_UI_EV_GOLDEN_AGE_ENDED",
                GC_UI_EV_GOLDEN_AGE_ENDED as i64,
            ),
            ("GC_UI_EV_TURN_CHANGED", GC_UI_EV_TURN_CHANGED as i64),
            (
                "GC_UI_EV_ACTIVE_PLAYER_CHANGED",
                GC_UI_EV_ACTIVE_PLAYER_CHANGED as i64,
            ),
            (
                "GC_UI_EV_PUBLISH_ACTIVE_PLAYER",
                GC_UI_EV_PUBLISH_ACTIVE_PLAYER as i64,
            ),
            (
                "GC_UI_EV_PUBLISH_NEW_GAME_TURN",
                GC_UI_EV_PUBLISH_NEW_GAME_TURN as i64,
            ),
            (
                "GC_UI_EV_PUBLISH_ERA_CHANGES",
                GC_UI_EV_PUBLISH_ERA_CHANGES as i64,
            ),
            ("GC_UI_EV_TRADE_NEW_ROUTE", GC_UI_EV_TRADE_NEW_ROUTE as i64),
            (
                "GC_UI_EV_TRADE_ROUTE_DIRECTION",
                GC_UI_EV_TRADE_ROUTE_DIRECTION as i64,
            ),
            (
                "GC_UI_EV_TRADE_DESTROY_ROUTE",
                GC_UI_EV_TRADE_DESTROY_ROUTE as i64,
            ),
            (
                "GC_UI_EV_TRADE_ACTIVATE_POPUP_ROUTE",
                GC_UI_EV_TRADE_ACTIVATE_POPUP_ROUTE as i64,
            ),
            (
                "GC_UI_EV_TRADE_DEACTIVATE_POPUP_ROUTE",
                GC_UI_EV_TRADE_DEACTIVATE_POPUP_ROUTE as i64,
            ),
            (
                "GC_UI_EV_ACTIVE_PLAYER_TURN_START",
                GC_UI_EV_ACTIVE_PLAYER_TURN_START as i64,
            ),
            (
                "GC_UI_EV_ACTIVE_PLAYER_TURN_END",
                GC_UI_EV_ACTIVE_PLAYER_TURN_END as i64,
            ),
            (
                "GC_UI_EV_REMOTE_PLAYER_TURN_START",
                GC_UI_EV_REMOTE_PLAYER_TURN_START as i64,
            ),
            (
                "GC_UI_EV_REMOTE_PLAYER_TURN_END",
                GC_UI_EV_REMOTE_PLAYER_TURN_END as i64,
            ),
            (
                "GC_UI_EV_PLAYER_TURN_STATUS",
                GC_UI_EV_PLAYER_TURN_STATUS as i64,
            ),
            (
                "GC_UI_EV_NOTIFICATION_ADDED",
                GC_UI_EV_NOTIFICATION_ADDED as i64,
            ),
            (
                "GC_UI_EV_NOTIFICATION_REMOVED",
                GC_UI_EV_NOTIFICATION_REMOVED as i64,
            ),
            (
                "GC_UI_EV_NOTIFICATION_ACTIVATED",
                GC_UI_EV_NOTIFICATION_ACTIVATED as i64,
            ),
            ("GC_UI_EV_MESSAGE", GC_UI_EV_MESSAGE as i64),
            (
                "GC_UI_EV_CLEAR_EVENT_MESSAGES",
                GC_UI_EV_CLEAR_EVENT_MESSAGES as i64,
            ),
            ("GC_UI_EV_POPUP", GC_UI_EV_POPUP as i64),
            ("GC_UI_EV_POPUP_ARRAY", GC_UI_EV_POPUP_ARRAY as i64),
            ("GC_UI_EV_POPUP_TEXT", GC_UI_EV_POPUP_TEXT as i64),
            ("GC_UI_EV_SET_DIRTY", GC_UI_EV_SET_DIRTY as i64),
            ("GC_UI_EV_LOOK_AT", GC_UI_EV_LOOK_AT as i64),
            ("GC_UI_EV_CENTER_CAMERA", GC_UI_EV_CENTER_CAMERA as i64),
            (
                "GC_UI_EV_SPECIFIC_CITY_INFO_DIRTY",
                GC_UI_EV_SPECIFIC_CITY_INFO_DIRTY as i64,
            ),
            ("GC_UI_EV_UPDATE_END_TURN", GC_UI_EV_UPDATE_END_TURN as i64),
            (
                "GC_UI_EV_END_TURN_BLOCKING_CHANGED",
                GC_UI_EV_END_TURN_BLOCKING_CHANGED as i64,
            ),
            (
                "GC_UI_EV_PUBLISH_END_TURN_DIRTY",
                GC_UI_EV_PUBLISH_END_TURN_DIRTY as i64,
            ),
            (
                "GC_UI_EV_UPDATE_CITY_SCREEN",
                GC_UI_EV_UPDATE_CITY_SCREEN as i64,
            ),
            (
                "GC_UI_EV_UNIT_SELECTION_CHANGED",
                GC_UI_EV_UNIT_SELECTION_CHANGED as i64,
            ),
            (
                "GC_UI_EV_CITY_SELECTION_CHANGED",
                GC_UI_EV_CITY_SELECTION_CHANGED as i64,
            ),
            (
                "GC_UI_EV_INTERFACE_MODE_CHANGED",
                GC_UI_EV_INTERFACE_MODE_CHANGED as i64,
            ),
            (
                "GC_UI_EV_CITY_SCREEN_CHANGED",
                GC_UI_EV_CITY_SCREEN_CHANGED as i64,
            ),
            (
                "GC_UI_EV_UPDATE_COUNTRY_BORDER",
                GC_UI_EV_UPDATE_COUNTRY_BORDER as i64,
            ),
            (
                "GC_UI_EV_UPDATE_ALL_COUNTRY_BORDERS",
                GC_UI_EV_UPDATE_ALL_COUNTRY_BORDERS as i64,
            ),
            (
                "GC_UI_EV_ADD_HEX_TO_UI_RANGE",
                GC_UI_EV_ADD_HEX_TO_UI_RANGE as i64,
            ),
            (
                "GC_UI_EV_OPEN_END_GAME_MENU",
                GC_UI_EV_OPEN_END_GAME_MENU as i64,
            ),
            ("GC_UI_EV_OPEN_TECH_TREE", GC_UI_EV_OPEN_TECH_TREE as i64),
            (
                "GC_UI_EV_OPEN_PLAYER_DEAL_SCREEN",
                GC_UI_EV_OPEN_PLAYER_DEAL_SCREEN as i64,
            ),
            (
                "GC_UI_EV_ADD_DEFERRED_WONDER_COMMAND",
                GC_UI_EV_ADD_DEFERRED_WONDER_COMMAND as i64,
            ),
            (
                "GC_UI_EV_TOGGLE_STRATEGIC_VIEW",
                GC_UI_EV_TOGGLE_STRATEGIC_VIEW as i64,
            ),
            ("GC_UI_EV_SET_TEMP_STRING", GC_UI_EV_SET_TEMP_STRING as i64),
            (
                "GC_UI_EV_LOOK_AT_SELECTION_PLOT",
                GC_UI_EV_LOOK_AT_SELECTION_PLOT as i64,
            ),
            (
                "GC_UI_EV_LOOK_AT_CITY_OFFSET",
                GC_UI_EV_LOOK_AT_CITY_OFFSET as i64,
            ),
            ("GC_UI_EV_FLASHING", GC_UI_EV_FLASHING as i64),
            (
                "GC_UI_EV_REFRESH_YIELD_VISIBLE_MODE",
                GC_UI_EV_REFRESH_YIELD_VISIBLE_MODE as i64,
            ),
            ("GC_UI_EV_GAME_EVENT", GC_UI_EV_GAME_EVENT as i64),
            ("GC_UI_EV_LUA_EVENT", GC_UI_EV_LUA_EVENT as i64),
            ("GC_UI_VAL_NIL", GC_UI_VAL_NIL as i64),
            ("GC_UI_VAL_BOOL", GC_UI_VAL_BOOL as i64),
            ("GC_UI_VAL_NUM", GC_UI_VAL_NUM as i64),
            ("GC_UI_VAL_STR", GC_UI_VAL_STR as i64),
            ("GC_UI_VAL_VEC", GC_UI_VAL_VEC as i64),
            ("GC_UI_OP_SELECT_UNIT", GC_UI_OP_SELECT_UNIT as i64),
            ("GC_UI_OP_SELECT_GROUP", GC_UI_OP_SELECT_GROUP as i64),
            (
                "GC_UI_OP_HEAD_SELECTED_UNIT",
                GC_UI_OP_HEAD_SELECTED_UNIT as i64,
            ),
            ("GC_UI_OP_SELECTED_UNITS", GC_UI_OP_SELECTED_UNITS as i64),
            ("GC_UI_OP_CLEAR_SELECTION", GC_UI_OP_CLEAR_SELECTION as i64),
            (
                "GC_UI_OP_LAST_SELECTED_UNIT",
                GC_UI_OP_LAST_SELECTED_UNIT as i64,
            ),
            ("GC_UI_OP_SELECT_CITY", GC_UI_OP_SELECT_CITY as i64),
            (
                "GC_UI_OP_HEAD_SELECTED_CITY",
                GC_UI_OP_HEAD_SELECTED_CITY as i64,
            ),
            (
                "GC_UI_OP_CLEAR_SELECTED_CITIES",
                GC_UI_OP_CLEAR_SELECTED_CITIES as i64,
            ),
            (
                "GC_UI_OP_DO_SELECT_CITY_AT_PLOT",
                GC_UI_OP_DO_SELECT_CITY_AT_PLOT as i64,
            ),
            (
                "GC_UI_OP_SET_INTERFACE_MODE",
                GC_UI_OP_SET_INTERFACE_MODE as i64,
            ),
            (
                "GC_UI_OP_GET_INTERFACE_MODE",
                GC_UI_OP_GET_INTERFACE_MODE as i64,
            ),
            (
                "GC_UI_OP_SET_INTERFACE_MODE_VALUE",
                GC_UI_OP_SET_INTERFACE_MODE_VALUE as i64,
            ),
            (
                "GC_UI_OP_GET_INTERFACE_MODE_VALUE",
                GC_UI_OP_GET_INTERFACE_MODE_VALUE as i64,
            ),
            (
                "GC_UI_OP_CAN_DO_INTERFACE_MODE",
                GC_UI_OP_CAN_DO_INTERFACE_MODE as i64,
            ),
            (
                "GC_UI_OP_SET_CITY_SCREEN_UP",
                GC_UI_OP_SET_CITY_SCREEN_UP as i64,
            ),
            (
                "GC_UI_OP_IS_CITY_SCREEN_UP",
                GC_UI_OP_IS_CITY_SCREEN_UP as i64,
            ),
            (
                "GC_UI_OP_CAN_SELECTION_LIST_FOUND",
                GC_UI_OP_CAN_SELECTION_LIST_FOUND as i64,
            ),
            (
                "GC_UI_OP_CAN_SELECTION_LIST_WORK",
                GC_UI_OP_CAN_SELECTION_LIST_WORK as i64,
            ),
            ("GC_UI_OP_CAN_END_TURN", GC_UI_OP_CAN_END_TURN as i64),
            (
                "GC_UI_OP_ACTIVATE_NOTIFICATION",
                GC_UI_OP_ACTIVATE_NOTIFICATION as i64,
            ),
            (
                "GC_UI_OP_REMOVE_NOTIFICATION",
                GC_UI_OP_REMOVE_NOTIFICATION as i64,
            ),
            (
                "GC_UI_OP_GET_CYCLE_SELECTION_COUNTER",
                GC_UI_OP_GET_CYCLE_SELECTION_COUNTER as i64,
            ),
            (
                "GC_UI_OP_SET_CYCLE_SELECTION_COUNTER",
                GC_UI_OP_SET_CYCLE_SELECTION_COUNTER as i64,
            ),
            (
                "GC_UI_OP_IS_UNIT_SELECTED",
                GC_UI_OP_IS_UNIT_SELECTED as i64,
            ),
            (
                "GC_UI_OP_IS_CITY_SELECTED",
                GC_UI_OP_IS_CITY_SELECTED as i64,
            ),
            (
                "GC_UI_OP_SELECT_ALL_AT_PLOT",
                GC_UI_OP_SELECT_ALL_AT_PLOT as i64,
            ),
            (
                "GC_UI_OP_GET_SELECTION_COUNT",
                GC_UI_OP_GET_SELECTION_COUNT as i64,
            ),
            ("GC_UI_OP_COUNT", GC_UI_OP_COUNT as i64),
            ("GC_CMD_MAKE_PEACE", GC_CMD_MAKE_PEACE as i64),
            ("GC_CMD_CANCEL_END_TURN", GC_CMD_CANCEL_END_TURN as i64),
            ("GC_CMD_END_TURN_TIMEOUT", GC_CMD_END_TURN_TIMEOUT as i64),
            ("GC_Q_TURN", GC_Q_TURN as i64),
            ("GC_Q_DIPLO", GC_Q_DIPLO as i64),
            ("GC_Q_REACHABLE_TREE", GC_Q_REACHABLE_TREE as i64),
            ("GC_Q_CITY_ART", GC_Q_CITY_ART as i64),
            ("GC_Q_TRADE_ROUTES", GC_Q_TRADE_ROUTES as i64),
            ("GC_Q_PLOT_BASIS", GC_Q_PLOT_BASIS as i64),
            ("GC_Q_COMBAT_PREVIEW", GC_Q_COMBAT_PREVIEW as i64),
            ("GC_Q_PLOTS_DIRTY", GC_Q_PLOTS_DIRTY as i64),
            ("GC_Q_TRADE_PATHS", GC_Q_TRADE_PATHS as i64),
            ("GC_PLAYER_F_OBSERVER", GC_PLAYER_F_OBSERVER as i64),
            (
                "GC_TURN_MODE_SIMULTANEOUS",
                GC_TURN_MODE_SIMULTANEOUS as i64,
            ),
            ("GC_TURN_MODE_HYBRID", GC_TURN_MODE_HYBRID as i64),
            ("GC_PHASE_HUMANS", GC_PHASE_HUMANS as i64),
            ("GC_PHASE_AI", GC_PHASE_AI as i64),
            ("GC_UNSAFE_NONE", GC_UNSAFE_NONE as i64),
            ("GC_UNSAFE_AI_RUNNING", GC_UNSAFE_AI_RUNNING as i64),
            ("GC_UNSAFE_COMMAND_GIVEN", GC_UNSAFE_COMMAND_GIVEN as i64),
            (
                "GC_UNSAFE_AI_REQUEST_OPEN",
                GC_UNSAFE_AI_REQUEST_OPEN as i64,
            ),
            ("GC_UNSAFE_AUTOMOVES", GC_UNSAFE_AUTOMOVES as i64),
            ("GC_TURN_F_HUMAN", GC_TURN_F_HUMAN as i64),
            ("GC_TURN_F_ALIVE", GC_TURN_F_ALIVE as i64),
            ("GC_TURN_F_CONNECTED", GC_TURN_F_CONNECTED as i64),
            ("GC_TURN_F_DISCONNECTED", GC_TURN_F_DISCONNECTED as i64),
            ("GC_TURN_F_KICKED", GC_TURN_F_KICKED as i64),
            ("GC_TURN_F_TURN_ACTIVE", GC_TURN_F_TURN_ACTIVE as i64),
            ("GC_TURN_F_TURN_ENDED", GC_TURN_F_TURN_ENDED as i64),
            ("GC_TURN_F_CAN_END_TURN", GC_TURN_F_CAN_END_TURN as i64),
            ("GC_TURN_F_SIMULTANEOUS", GC_TURN_F_SIMULTANEOUS as i64),
            ("GC_TURN_F_AUTOMOVES", GC_TURN_F_AUTOMOVES as i64),
            ("GC_TURN_F_WAITING_FOR", GC_TURN_F_WAITING_FOR as i64),
            ("GC_DIPLO_F_MET", GC_DIPLO_F_MET as i64),
            ("GC_DIPLO_F_AT_WAR", GC_DIPLO_F_AT_WAR as i64),
            ("GC_DIPLO_F_HUMAN", GC_DIPLO_F_HUMAN as i64),
            (
                "GC_DIPLO_F_CAN_DECLARE_WAR",
                GC_DIPLO_F_CAN_DECLARE_WAR as i64,
            ),
            (
                "GC_DIPLO_F_CAN_MAKE_PEACE",
                GC_DIPLO_F_CAN_MAKE_PEACE as i64,
            ),
            ("GC_REACH_F_ATTACK", GC_REACH_F_ATTACK as i64),
            ("GC_REACH_F_EMBARK", GC_REACH_F_EMBARK as i64),
            ("GC_REACH_F_DISEMBARK", GC_REACH_F_DISEMBARK as i64),
            ("GC_REACH_F_CITY", GC_REACH_F_CITY as i64),
            ("GC_REACH_F_STOP", GC_REACH_F_STOP as i64),
            ("GC_REACH_F_UNREVEALED", GC_REACH_F_UNREVEALED as i64),
            ("GC_REACH_F_CAPTURE", GC_REACH_F_CAPTURE as i64),
            ("GC_REACH_F_UNREACHABLE", GC_REACH_F_UNREACHABLE as i64),
            ("GC_BASIS_F_LAKE", GC_BASIS_F_LAKE as i64),
            ("GC_BASIS_F_COASTAL", GC_BASIS_F_COASTAL as i64),
            (
                "GC_BASIS_F_NATURAL_WONDER",
                GC_BASIS_F_NATURAL_WONDER as i64,
            ),
            ("GC_COMBAT_F_KILLS", GC_COMBAT_F_KILLS as i64),
            ("GC_COMBAT_F_CAPTURES", GC_COMBAT_F_CAPTURES as i64),
            ("GC_COMBAT_F_CITY", GC_COMBAT_F_CITY as i64),
            ("GC_CATALOG_TURN_TIMERS", GC_CATALOG_TURN_TIMERS as i64),
            ("GC_CATALOG_ART_TECHS", GC_CATALOG_ART_TECHS as i64),
            ("GC_CATALOG_WORLD_SIZES", GC_CATALOG_WORLD_SIZES as i64),
            ("GC_CATALOG_GAME_SPEEDS", GC_CATALOG_GAME_SPEEDS as i64),
            (
                "GC_CATALOG_TABLE_COUNT_V2",
                GC_CATALOG_TABLE_COUNT_V2 as i64,
            ),
            ("GC_CATALOG_GAME_OPTIONS", GC_CATALOG_GAME_OPTIONS as i64),
            ("GC_CATALOG_VICTORIES", GC_CATALOG_VICTORIES as i64),
            ("GC_CATALOG_MINOR_CIVS", GC_CATALOG_MINOR_CIVS as i64),
            ("GC_CATALOG_MAP_OPTIONS", GC_CATALOG_MAP_OPTIONS as i64),
            (
                "GC_CATALOG_MAP_OPTION_VALUES",
                GC_CATALOG_MAP_OPTION_VALUES as i64,
            ),
            (
                "GC_CATALOG_TABLE_COUNT_V3",
                GC_CATALOG_TABLE_COUNT_V3 as i64,
            ),
            ("GC_CATALOG_CIV_INFO", GC_CATALOG_CIV_INFO as i64),
            (
                "GC_CATALOG_TABLE_COUNT_V4",
                GC_CATALOG_TABLE_COUNT_V4 as i64,
            ),
            // UI v2 read-only queries
            ("GC_Q_CITY_QUEUE", GC_Q_CITY_QUEUE as i64),
            ("GC_Q_TECHS", GC_Q_TECHS as i64),
            ("GC_Q_PLAYER_RESOURCES", GC_Q_PLAYER_RESOURCES as i64),
            ("GC_Q_POLICY_BRANCHES", GC_Q_POLICY_BRANCHES as i64),
            ("GC_Q_UNIT_PROMOTIONS", GC_Q_UNIT_PROMOTIONS as i64),
            (
                "GC_UNIT_ACTIONS_ROWS_EXTRA",
                GC_UNIT_ACTIONS_ROWS_EXTRA as i64,
            ),
            ("GC_UNIT_ROW_ALERT", GC_UNIT_ROW_ALERT as i64),
            ("GC_UNIT_ROW_PILLAGE", GC_UNIT_ROW_PILLAGE as i64),
            ("GC_UNIT_ROW_UPGRADE", GC_UNIT_ROW_UPGRADE as i64),
            (
                "GC_CATALOG_POLICY_BRANCHES",
                GC_CATALOG_POLICY_BRANCHES as i64,
            ),
            (
                "GC_CATALOG_TABLE_COUNT_V5",
                GC_CATALOG_TABLE_COUNT_V5 as i64,
            ),
            ("GC_TECH_HAVE", GC_TECH_HAVE as i64),
            ("GC_TECH_CAN_RESEARCH", GC_TECH_CAN_RESEARCH as i64),
            ("GC_TECH_LOCKED", GC_TECH_LOCKED as i64),
            ("GC_RESOURCE_USAGE_BONUS", GC_RESOURCE_USAGE_BONUS as i64),
            (
                "GC_RESOURCE_USAGE_STRATEGIC",
                GC_RESOURCE_USAGE_STRATEGIC as i64,
            ),
            ("GC_RESOURCE_USAGE_LUXURY", GC_RESOURCE_USAGE_LUXURY as i64),
        ];
        for (name, rust) in table {
            assert_eq!(define(name), *rust, "{name}");
        }
        // Every `#define GC_*` in the header with a numeric value is in the table (append-only
        // changes of the header must be mirrored here).
        let known: Vec<&str> = table.iter().map(|(n, _)| *n).collect();
        for line in header.lines().filter(|l| l.starts_with("#define GC")) {
            let name = line["#define ".len()..].split_whitespace().next().unwrap();
            if name.ends_with("_H") {
                continue; // include guard
            }
            assert!(
                known.contains(&name),
                "header constant {name} is not in ffi.rs"
            );
        }
    }
}
