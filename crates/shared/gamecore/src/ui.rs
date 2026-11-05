//! The UI side of a replica: what the engine tells the UI ([`UiEvent`]),
//! the operations on the exe's interface state ([`UiOp`]) and the Lua bridge (`attach_lua`, `invalidate_lua`).
//!
//! The kinds and operations mirror `gamecore_abi.h` (`GC_UI_EV_*`, `GC_UI_OP_*`); a test keeps the two in step.

use gamecore_sys::{RawUiEvent, ffi};

/// What the engine reported (the `gDLL->Gameplay*` / `DLLUI->*` callbacks and the Lua events). The arguments of each
/// kind, in `UiEvent::args`, are listed in `gamecore_abi.h`; unit and city blocks are `GC_UI_UNIT_INTS` / `GC_UI_CITY_INTS` long.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UiEventKind {
    /// `UNIT_CREATED` (1): UNIT
    UnitCreated,
    /// `UNIT_MOVED` (2): UNIT; data = u32 plot indices of the path (LE)
    UnitMoved,
    /// `UNIT_TELEPORTED` (3): UNIT, x, y (the plot it was put on)
    UnitTeleported,
    /// `UNIT_DESTROYED` (4): UNIT
    UnitDestroyed,
    /// `UNIT_DESTROYED_IN_COMBAT` (5): UNIT
    UnitDestroyedInCombat,
    /// `UNIT_SET_DAMAGE` (6): UNIT, damage, previous damage
    UnitSetDamage,
    /// `UNIT_FORTIFY` (7): UNIT, fortify 0/1
    UnitFortify,
    /// `UNIT_WORK` (8): UNIT, work type
    UnitWork,
    /// `UNIT_PARADROP` (9): UNIT
    UnitParadrop,
    /// `UNIT_ACTIVATE` (10): UNIT
    UnitActivate,
    /// `UNIT_EMBARK` (11): UNIT, embark 0/1
    UnitEmbark,
    /// `UNIT_GARRISON` (12): UNIT, garrison 0/1
    UnitGarrison,
    /// `UNIT_SHOULD_DIM_FLAG` (13): UNIT, dim 0/1
    UnitShouldDimFlag,
    /// `UNIT_MARK_THREATENING` (14): UNIT, mark 0/1
    UnitMarkThreatening,
    /// `UNIT_VISIBILITY` (15): UNIT, visible 0/1, change invisibility 0/1, blend time in ms
    UnitVisibility,
    /// `UNIT_COMBAT` (16): attacker side (kind 0 none / 1 unit / 2 city, owner, id), defender side (3 ints), interceptor side (3 ints), x, y, flags (1 attacker advances,
    UnitCombat,
    /// `UNIT_MISSION_END` (17): UNIT
    UnitMissionEnd,
    /// `UNIT_REBASED` (18): UNIT, old x, old y, new x, new y
    UnitRebased,
    /// `UNIT_RESET_ANIMATION` (19): UNIT
    UnitResetAnimation,
    /// `CITY_CREATED` (20): CITY, era
    CityCreated,
    /// `CITY_DESTROYED` (21): CITY, new owner
    CityDestroyed,
    /// `CITY_CAPTURED` (22): CITY, new owner
    CityCaptured,
    /// `CITY_SET_DAMAGE` (23): CITY, damage, previous damage
    CitySetDamage,
    /// `CITY_POPULATION_CHANGED` (24): CITY, new population
    CityPopulationChanged,
    /// `CITY_COMBAT` (25): as UNIT_COMBAT
    CityCombat,
    /// `ERA_CHANGED` (30): player, era
    EraChanged,
    /// `BRIDGE_CHANGED` (31): bridge 0/1, style
    BridgeChanged,
    /// `TECH_ACQUIRED` (32): team, tech
    TechAcquired,
    /// `FEATURE_CHANGED` (33): x, y, feature
    FeatureChanged,
    /// `PLOT_STATE_CHANGE` (34): x, y, revealed resource, revealed improvement, improvement state, route, route state
    PlotStateChange,
    /// `PLOT_ICON_STATE_CHANGE` (35): x, y, revealed resource, revealed improvement, improvement state
    PlotIconStateChange,
    /// `NATURAL_WONDER_REVEALED` (36): x, y
    NaturalWonderRevealed,
    /// `WALL_CREATED` (37): x, y
    WallCreated,
    /// `DO_FX` (38): x, y (-1, -1 = none), fx type
    DoFx,
    /// `FOW_CHANGED` (39): x, y, FogOfWarModeTypes for the seat's team, set all 0/1
    FowChanged,
    /// `YIELD_MIGHT_HAVE_CHANGED` (40): x, y
    YieldMightHaveChanged,
    /// `WAR_STATE_CHANGED` (41): team 1, team 2, at war 0/1
    WarStateChanged,
    /// `MET_TEAM` (42): my team, their team
    MetTeam,
    /// `DIPLO_AI_LEADER_MESSAGE` (43): player, DiploUIStateTypes, LeaderheadAnimationTypes, data1; text = the leader's message
    DiploAiLeaderMessage,
    /// `CLEAR_DIPLO_TRADE_TABLE` (44): -
    ClearDiploTradeTable,
    /// `NOTIFY_AI_LEADERS_IN_GAME` (45): -
    NotifyAiLeadersInGame,
    /// `NOTIFY_SPECIFIC_AI_LEADER` (46): player
    NotifySpecificAiLeader,
    /// `WONDER_CREATED` (50): player, x, y, building, state
    WonderCreated,
    /// `WONDER_EDITED` (51): player, x, y, building, state
    WonderEdited,
    /// `WONDER_REMOVED` (52): player, x, y, building
    WonderRemoved,
    /// `SPACESHIP_CREATED` (53): x, y, state
    SpaceshipCreated,
    /// `SPACESHIP_EDITED` (54): x, y, state
    SpaceshipEdited,
    /// `SPACESHIP_REMOVED` (55): x, y
    SpaceshipRemoved,
    /// `OPEN_OPTIONS_SCREEN` (56): -
    OpenOptionsScreen,
    /// `SEARCH_FOR_PEDIA_ENTRY` (57): text = the entry
    SearchForPediaEntry,
    /// `OPEN_INFO_CORNER` (58): info corner id
    OpenInfoCorner,
    /// `WORLD_ANCHOR` (59): GenericWorldAnchorTypes, add 0/1, x, y, data1
    WorldAnchor,
    /// `MINIMAP_UNIT_SELECT` (60): x, y
    MinimapUnitSelect,
    /// `MINIMAP_NOTIFICATION` (61): x, y, id
    MinimapNotification,
    /// `GOLDEN_AGE_STARTED` (62): -
    GoldenAgeStarted,
    /// `GOLDEN_AGE_ENDED` (63): -
    GoldenAgeEnded,
    /// `TURN_CHANGED` (64): new turn
    TurnChanged,
    /// `ACTIVE_PLAYER_CHANGED` (65): player
    ActivePlayerChanged,
    /// `PUBLISH_ACTIVE_PLAYER` (66): player, old player
    PublishActivePlayer,
    /// `PUBLISH_NEW_GAME_TURN` (67): turn
    PublishNewGameTurn,
    /// `PUBLISH_ERA_CHANGES` (68): -
    PublishEraChanges,
    /// `TRADE_NEW_ROUTE` (69): route, player, connection type, plot count n, then x, y of the first (n <= 9) plots
    TradeNewRoute,
    /// `TRADE_ROUTE_DIRECTION` (70): route, forwards 0/1
    TradeRouteDirection,
    /// `TRADE_DESTROY_ROUTE` (71): route, player
    TradeDestroyRoute,
    /// `TRADE_ACTIVATE_POPUP_ROUTE` (72): route
    TradeActivatePopupRoute,
    /// `TRADE_DEACTIVATE_POPUP_ROUTE` (73): -
    TradeDeactivatePopupRoute,
    /// `ACTIVE_PLAYER_TURN_START` (80): - (the seat's turn started)
    ActivePlayerTurnStart,
    /// `ACTIVE_PLAYER_TURN_END` (81): -
    ActivePlayerTurnEnd,
    /// `REMOTE_PLAYER_TURN_START` (82): -
    RemotePlayerTurnStart,
    /// `REMOTE_PLAYER_TURN_END` (83): -
    RemotePlayerTurnEnd,
    /// `PLAYER_TURN_STATUS` (84): TURN_STATUS_TYPE, player; text = tag
    PlayerTurnStatus,
    /// `NOTIFICATION_ADDED` (85): id, type, x, y, data1, data2, player; text = message, text2 = summary (the seat's notifications, found by comparison after each batch)
    NotificationAdded,
    /// `NOTIFICATION_REMOVED` (86): id, player
    NotificationRemoved,
    /// `NOTIFICATION_ACTIVATED` (87): id, type, x, y, game index, extra game data, player; text = message (from ActivateNotification)
    NotificationActivated,
    /// `MESSAGE` (88): player, force 0/1, display length, InterfaceMessageTypes, x, y, source (0 plain, 1 unit, 2 plot, 3 city), owner, id; text = the line, text2 = sound
    Message,
    /// `CLEAR_EVENT_MESSAGES` (89): -
    ClearEventMessages,
    /// `POPUP` (90): ButtonPopupTypes, data1, data2, data3, option1 0/1, option2 0/1, player; text = szText
    Popup,
    /// `POPUP_ARRAY` (91): ButtonPopupTypes, n ints (<= 10), the ints, n bools (<= 10), the bools
    PopupArray,
    /// `POPUP_TEXT` (92): x, y, delay in ms; text
    PopupText,
    /// `SET_DIRTY` (93): InterfaceDirtyBits, value 0/1
    SetDirty,
    /// `LOOK_AT` (94): x, y, CameraLookAtTypes
    LookAt,
    /// `CENTER_CAMERA` (95): UNIT
    CenterCamera,
    /// `SPECIFIC_CITY_INFO_DIRTY` (96): CITY, CityUpdateTypes
    SpecificCityInfoDirty,
    /// `UPDATE_END_TURN` (97): -
    UpdateEndTurn,
    /// `END_TURN_BLOCKING_CHANGED` (98): previous EndTurnBlockingTypes, new
    EndTurnBlockingChanged,
    /// `PUBLISH_END_TURN_DIRTY` (99): -
    PublishEndTurnDirty,
    /// `UPDATE_CITY_SCREEN` (100): old city owner, old city id (-1 = none)
    UpdateCityScreen,
    /// `UNIT_SELECTION_CHANGED` (101): selected count, head owner, head id (-1, -1 = none)
    UnitSelectionChanged,
    /// `CITY_SELECTION_CHANGED` (102): selected count, head owner, head id
    CitySelectionChanged,
    /// `INTERFACE_MODE_CHANGED` (103): old mode, new mode, mode value
    InterfaceModeChanged,
    /// `CITY_SCREEN_CHANGED` (104): city screen up 0/1 (SerialEventEnterCityScreen / ExitCityScreen)
    CityScreenChanged,
    /// `UPDATE_COUNTRY_BORDER` (105): x, y
    UpdateCountryBorder,
    /// `UPDATE_ALL_COUNTRY_BORDERS` (106): -
    UpdateAllCountryBorders,
    /// `ADD_HEX_TO_UI_RANGE` (107): x, y
    AddHexToUiRange,
    /// `OPEN_END_GAME_MENU` (108): -
    OpenEndGameMenu,
    /// `OPEN_TECH_TREE` (109): -
    OpenTechTree,
    /// `OPEN_PLAYER_DEAL_SCREEN` (110): player
    OpenPlayerDealScreen,
    /// `ADD_DEFERRED_WONDER_COMMAND` (111): WonderCommandType, CITY, building, state
    AddDeferredWonderCommand,
    /// `TOGGLE_STRATEGIC_VIEW` (112): -
    ToggleStrategicView,
    /// `SET_TEMP_STRING` (113): text
    SetTempString,
    /// `LOOK_AT_SELECTION_PLOT` (114): release 0/1
    LookAtSelectionPlot,
    /// `LOOK_AT_CITY_OFFSET` (115): city id
    LookAtCityOffset,
    /// `FLASHING` (116): player, flashing 0/1
    Flashing,
    /// `REFRESH_YIELD_VISIBLE_MODE` (117): -
    RefreshYieldVisibleMode,
    /// `GAME_EVENT` (150): text = GameEvents name, data = the arguments (GC_UI_VAL_*); after the core's listeners ran, only for names given to gc_ui_subscribe_game_event
    GameEvent,
    /// `LUA_EVENT` (151): text = Events name, data = the arguments; a call of Events.<name>(...) from the core's Lua (Lekmod's contexts)
    LuaEvent,
    /// A kind this library does not know (a newer core).
    Unknown(i32),
}

impl UiEventKind {
    /// Every known kind, in the order of the header.
    pub const ALL: [UiEventKind; 106] = [
        UiEventKind::UnitCreated,
        UiEventKind::UnitMoved,
        UiEventKind::UnitTeleported,
        UiEventKind::UnitDestroyed,
        UiEventKind::UnitDestroyedInCombat,
        UiEventKind::UnitSetDamage,
        UiEventKind::UnitFortify,
        UiEventKind::UnitWork,
        UiEventKind::UnitParadrop,
        UiEventKind::UnitActivate,
        UiEventKind::UnitEmbark,
        UiEventKind::UnitGarrison,
        UiEventKind::UnitShouldDimFlag,
        UiEventKind::UnitMarkThreatening,
        UiEventKind::UnitVisibility,
        UiEventKind::UnitCombat,
        UiEventKind::UnitMissionEnd,
        UiEventKind::UnitRebased,
        UiEventKind::UnitResetAnimation,
        UiEventKind::CityCreated,
        UiEventKind::CityDestroyed,
        UiEventKind::CityCaptured,
        UiEventKind::CitySetDamage,
        UiEventKind::CityPopulationChanged,
        UiEventKind::CityCombat,
        UiEventKind::EraChanged,
        UiEventKind::BridgeChanged,
        UiEventKind::TechAcquired,
        UiEventKind::FeatureChanged,
        UiEventKind::PlotStateChange,
        UiEventKind::PlotIconStateChange,
        UiEventKind::NaturalWonderRevealed,
        UiEventKind::WallCreated,
        UiEventKind::DoFx,
        UiEventKind::FowChanged,
        UiEventKind::YieldMightHaveChanged,
        UiEventKind::WarStateChanged,
        UiEventKind::MetTeam,
        UiEventKind::DiploAiLeaderMessage,
        UiEventKind::ClearDiploTradeTable,
        UiEventKind::NotifyAiLeadersInGame,
        UiEventKind::NotifySpecificAiLeader,
        UiEventKind::WonderCreated,
        UiEventKind::WonderEdited,
        UiEventKind::WonderRemoved,
        UiEventKind::SpaceshipCreated,
        UiEventKind::SpaceshipEdited,
        UiEventKind::SpaceshipRemoved,
        UiEventKind::OpenOptionsScreen,
        UiEventKind::SearchForPediaEntry,
        UiEventKind::OpenInfoCorner,
        UiEventKind::WorldAnchor,
        UiEventKind::MinimapUnitSelect,
        UiEventKind::MinimapNotification,
        UiEventKind::GoldenAgeStarted,
        UiEventKind::GoldenAgeEnded,
        UiEventKind::TurnChanged,
        UiEventKind::ActivePlayerChanged,
        UiEventKind::PublishActivePlayer,
        UiEventKind::PublishNewGameTurn,
        UiEventKind::PublishEraChanges,
        UiEventKind::TradeNewRoute,
        UiEventKind::TradeRouteDirection,
        UiEventKind::TradeDestroyRoute,
        UiEventKind::TradeActivatePopupRoute,
        UiEventKind::TradeDeactivatePopupRoute,
        UiEventKind::ActivePlayerTurnStart,
        UiEventKind::ActivePlayerTurnEnd,
        UiEventKind::RemotePlayerTurnStart,
        UiEventKind::RemotePlayerTurnEnd,
        UiEventKind::PlayerTurnStatus,
        UiEventKind::NotificationAdded,
        UiEventKind::NotificationRemoved,
        UiEventKind::NotificationActivated,
        UiEventKind::Message,
        UiEventKind::ClearEventMessages,
        UiEventKind::Popup,
        UiEventKind::PopupArray,
        UiEventKind::PopupText,
        UiEventKind::SetDirty,
        UiEventKind::LookAt,
        UiEventKind::CenterCamera,
        UiEventKind::SpecificCityInfoDirty,
        UiEventKind::UpdateEndTurn,
        UiEventKind::EndTurnBlockingChanged,
        UiEventKind::PublishEndTurnDirty,
        UiEventKind::UpdateCityScreen,
        UiEventKind::UnitSelectionChanged,
        UiEventKind::CitySelectionChanged,
        UiEventKind::InterfaceModeChanged,
        UiEventKind::CityScreenChanged,
        UiEventKind::UpdateCountryBorder,
        UiEventKind::UpdateAllCountryBorders,
        UiEventKind::AddHexToUiRange,
        UiEventKind::OpenEndGameMenu,
        UiEventKind::OpenTechTree,
        UiEventKind::OpenPlayerDealScreen,
        UiEventKind::AddDeferredWonderCommand,
        UiEventKind::ToggleStrategicView,
        UiEventKind::SetTempString,
        UiEventKind::LookAtSelectionPlot,
        UiEventKind::LookAtCityOffset,
        UiEventKind::Flashing,
        UiEventKind::RefreshYieldVisibleMode,
        UiEventKind::GameEvent,
        UiEventKind::LuaEvent,
    ];

    /// The kind of an ABI code.
    pub fn from_code(code: i32) -> UiEventKind {
        match code {
            1 => UiEventKind::UnitCreated,
            2 => UiEventKind::UnitMoved,
            3 => UiEventKind::UnitTeleported,
            4 => UiEventKind::UnitDestroyed,
            5 => UiEventKind::UnitDestroyedInCombat,
            6 => UiEventKind::UnitSetDamage,
            7 => UiEventKind::UnitFortify,
            8 => UiEventKind::UnitWork,
            9 => UiEventKind::UnitParadrop,
            10 => UiEventKind::UnitActivate,
            11 => UiEventKind::UnitEmbark,
            12 => UiEventKind::UnitGarrison,
            13 => UiEventKind::UnitShouldDimFlag,
            14 => UiEventKind::UnitMarkThreatening,
            15 => UiEventKind::UnitVisibility,
            16 => UiEventKind::UnitCombat,
            17 => UiEventKind::UnitMissionEnd,
            18 => UiEventKind::UnitRebased,
            19 => UiEventKind::UnitResetAnimation,
            20 => UiEventKind::CityCreated,
            21 => UiEventKind::CityDestroyed,
            22 => UiEventKind::CityCaptured,
            23 => UiEventKind::CitySetDamage,
            24 => UiEventKind::CityPopulationChanged,
            25 => UiEventKind::CityCombat,
            30 => UiEventKind::EraChanged,
            31 => UiEventKind::BridgeChanged,
            32 => UiEventKind::TechAcquired,
            33 => UiEventKind::FeatureChanged,
            34 => UiEventKind::PlotStateChange,
            35 => UiEventKind::PlotIconStateChange,
            36 => UiEventKind::NaturalWonderRevealed,
            37 => UiEventKind::WallCreated,
            38 => UiEventKind::DoFx,
            39 => UiEventKind::FowChanged,
            40 => UiEventKind::YieldMightHaveChanged,
            41 => UiEventKind::WarStateChanged,
            42 => UiEventKind::MetTeam,
            43 => UiEventKind::DiploAiLeaderMessage,
            44 => UiEventKind::ClearDiploTradeTable,
            45 => UiEventKind::NotifyAiLeadersInGame,
            46 => UiEventKind::NotifySpecificAiLeader,
            50 => UiEventKind::WonderCreated,
            51 => UiEventKind::WonderEdited,
            52 => UiEventKind::WonderRemoved,
            53 => UiEventKind::SpaceshipCreated,
            54 => UiEventKind::SpaceshipEdited,
            55 => UiEventKind::SpaceshipRemoved,
            56 => UiEventKind::OpenOptionsScreen,
            57 => UiEventKind::SearchForPediaEntry,
            58 => UiEventKind::OpenInfoCorner,
            59 => UiEventKind::WorldAnchor,
            60 => UiEventKind::MinimapUnitSelect,
            61 => UiEventKind::MinimapNotification,
            62 => UiEventKind::GoldenAgeStarted,
            63 => UiEventKind::GoldenAgeEnded,
            64 => UiEventKind::TurnChanged,
            65 => UiEventKind::ActivePlayerChanged,
            66 => UiEventKind::PublishActivePlayer,
            67 => UiEventKind::PublishNewGameTurn,
            68 => UiEventKind::PublishEraChanges,
            69 => UiEventKind::TradeNewRoute,
            70 => UiEventKind::TradeRouteDirection,
            71 => UiEventKind::TradeDestroyRoute,
            72 => UiEventKind::TradeActivatePopupRoute,
            73 => UiEventKind::TradeDeactivatePopupRoute,
            80 => UiEventKind::ActivePlayerTurnStart,
            81 => UiEventKind::ActivePlayerTurnEnd,
            82 => UiEventKind::RemotePlayerTurnStart,
            83 => UiEventKind::RemotePlayerTurnEnd,
            84 => UiEventKind::PlayerTurnStatus,
            85 => UiEventKind::NotificationAdded,
            86 => UiEventKind::NotificationRemoved,
            87 => UiEventKind::NotificationActivated,
            88 => UiEventKind::Message,
            89 => UiEventKind::ClearEventMessages,
            90 => UiEventKind::Popup,
            91 => UiEventKind::PopupArray,
            92 => UiEventKind::PopupText,
            93 => UiEventKind::SetDirty,
            94 => UiEventKind::LookAt,
            95 => UiEventKind::CenterCamera,
            96 => UiEventKind::SpecificCityInfoDirty,
            97 => UiEventKind::UpdateEndTurn,
            98 => UiEventKind::EndTurnBlockingChanged,
            99 => UiEventKind::PublishEndTurnDirty,
            100 => UiEventKind::UpdateCityScreen,
            101 => UiEventKind::UnitSelectionChanged,
            102 => UiEventKind::CitySelectionChanged,
            103 => UiEventKind::InterfaceModeChanged,
            104 => UiEventKind::CityScreenChanged,
            105 => UiEventKind::UpdateCountryBorder,
            106 => UiEventKind::UpdateAllCountryBorders,
            107 => UiEventKind::AddHexToUiRange,
            108 => UiEventKind::OpenEndGameMenu,
            109 => UiEventKind::OpenTechTree,
            110 => UiEventKind::OpenPlayerDealScreen,
            111 => UiEventKind::AddDeferredWonderCommand,
            112 => UiEventKind::ToggleStrategicView,
            113 => UiEventKind::SetTempString,
            114 => UiEventKind::LookAtSelectionPlot,
            115 => UiEventKind::LookAtCityOffset,
            116 => UiEventKind::Flashing,
            117 => UiEventKind::RefreshYieldVisibleMode,
            150 => UiEventKind::GameEvent,
            151 => UiEventKind::LuaEvent,
            other => UiEventKind::Unknown(other),
        }
    }

    /// The ABI code (`ffi::GC_UI_EV_*`).
    pub fn code(self) -> i32 {
        match self {
            UiEventKind::UnitCreated => 1,
            UiEventKind::UnitMoved => 2,
            UiEventKind::UnitTeleported => 3,
            UiEventKind::UnitDestroyed => 4,
            UiEventKind::UnitDestroyedInCombat => 5,
            UiEventKind::UnitSetDamage => 6,
            UiEventKind::UnitFortify => 7,
            UiEventKind::UnitWork => 8,
            UiEventKind::UnitParadrop => 9,
            UiEventKind::UnitActivate => 10,
            UiEventKind::UnitEmbark => 11,
            UiEventKind::UnitGarrison => 12,
            UiEventKind::UnitShouldDimFlag => 13,
            UiEventKind::UnitMarkThreatening => 14,
            UiEventKind::UnitVisibility => 15,
            UiEventKind::UnitCombat => 16,
            UiEventKind::UnitMissionEnd => 17,
            UiEventKind::UnitRebased => 18,
            UiEventKind::UnitResetAnimation => 19,
            UiEventKind::CityCreated => 20,
            UiEventKind::CityDestroyed => 21,
            UiEventKind::CityCaptured => 22,
            UiEventKind::CitySetDamage => 23,
            UiEventKind::CityPopulationChanged => 24,
            UiEventKind::CityCombat => 25,
            UiEventKind::EraChanged => 30,
            UiEventKind::BridgeChanged => 31,
            UiEventKind::TechAcquired => 32,
            UiEventKind::FeatureChanged => 33,
            UiEventKind::PlotStateChange => 34,
            UiEventKind::PlotIconStateChange => 35,
            UiEventKind::NaturalWonderRevealed => 36,
            UiEventKind::WallCreated => 37,
            UiEventKind::DoFx => 38,
            UiEventKind::FowChanged => 39,
            UiEventKind::YieldMightHaveChanged => 40,
            UiEventKind::WarStateChanged => 41,
            UiEventKind::MetTeam => 42,
            UiEventKind::DiploAiLeaderMessage => 43,
            UiEventKind::ClearDiploTradeTable => 44,
            UiEventKind::NotifyAiLeadersInGame => 45,
            UiEventKind::NotifySpecificAiLeader => 46,
            UiEventKind::WonderCreated => 50,
            UiEventKind::WonderEdited => 51,
            UiEventKind::WonderRemoved => 52,
            UiEventKind::SpaceshipCreated => 53,
            UiEventKind::SpaceshipEdited => 54,
            UiEventKind::SpaceshipRemoved => 55,
            UiEventKind::OpenOptionsScreen => 56,
            UiEventKind::SearchForPediaEntry => 57,
            UiEventKind::OpenInfoCorner => 58,
            UiEventKind::WorldAnchor => 59,
            UiEventKind::MinimapUnitSelect => 60,
            UiEventKind::MinimapNotification => 61,
            UiEventKind::GoldenAgeStarted => 62,
            UiEventKind::GoldenAgeEnded => 63,
            UiEventKind::TurnChanged => 64,
            UiEventKind::ActivePlayerChanged => 65,
            UiEventKind::PublishActivePlayer => 66,
            UiEventKind::PublishNewGameTurn => 67,
            UiEventKind::PublishEraChanges => 68,
            UiEventKind::TradeNewRoute => 69,
            UiEventKind::TradeRouteDirection => 70,
            UiEventKind::TradeDestroyRoute => 71,
            UiEventKind::TradeActivatePopupRoute => 72,
            UiEventKind::TradeDeactivatePopupRoute => 73,
            UiEventKind::ActivePlayerTurnStart => 80,
            UiEventKind::ActivePlayerTurnEnd => 81,
            UiEventKind::RemotePlayerTurnStart => 82,
            UiEventKind::RemotePlayerTurnEnd => 83,
            UiEventKind::PlayerTurnStatus => 84,
            UiEventKind::NotificationAdded => 85,
            UiEventKind::NotificationRemoved => 86,
            UiEventKind::NotificationActivated => 87,
            UiEventKind::Message => 88,
            UiEventKind::ClearEventMessages => 89,
            UiEventKind::Popup => 90,
            UiEventKind::PopupArray => 91,
            UiEventKind::PopupText => 92,
            UiEventKind::SetDirty => 93,
            UiEventKind::LookAt => 94,
            UiEventKind::CenterCamera => 95,
            UiEventKind::SpecificCityInfoDirty => 96,
            UiEventKind::UpdateEndTurn => 97,
            UiEventKind::EndTurnBlockingChanged => 98,
            UiEventKind::PublishEndTurnDirty => 99,
            UiEventKind::UpdateCityScreen => 100,
            UiEventKind::UnitSelectionChanged => 101,
            UiEventKind::CitySelectionChanged => 102,
            UiEventKind::InterfaceModeChanged => 103,
            UiEventKind::CityScreenChanged => 104,
            UiEventKind::UpdateCountryBorder => 105,
            UiEventKind::UpdateAllCountryBorders => 106,
            UiEventKind::AddHexToUiRange => 107,
            UiEventKind::OpenEndGameMenu => 108,
            UiEventKind::OpenTechTree => 109,
            UiEventKind::OpenPlayerDealScreen => 110,
            UiEventKind::AddDeferredWonderCommand => 111,
            UiEventKind::ToggleStrategicView => 112,
            UiEventKind::SetTempString => 113,
            UiEventKind::LookAtSelectionPlot => 114,
            UiEventKind::LookAtCityOffset => 115,
            UiEventKind::Flashing => 116,
            UiEventKind::RefreshYieldVisibleMode => 117,
            UiEventKind::GameEvent => 150,
            UiEventKind::LuaEvent => 151,
            UiEventKind::Unknown(code) => code,
        }
    }

    /// The name of the ABI constant without its prefix (`UNIT_CREATED`).
    pub fn name(self) -> &'static str {
        match self {
            UiEventKind::UnitCreated => "UNIT_CREATED",
            UiEventKind::UnitMoved => "UNIT_MOVED",
            UiEventKind::UnitTeleported => "UNIT_TELEPORTED",
            UiEventKind::UnitDestroyed => "UNIT_DESTROYED",
            UiEventKind::UnitDestroyedInCombat => "UNIT_DESTROYED_IN_COMBAT",
            UiEventKind::UnitSetDamage => "UNIT_SET_DAMAGE",
            UiEventKind::UnitFortify => "UNIT_FORTIFY",
            UiEventKind::UnitWork => "UNIT_WORK",
            UiEventKind::UnitParadrop => "UNIT_PARADROP",
            UiEventKind::UnitActivate => "UNIT_ACTIVATE",
            UiEventKind::UnitEmbark => "UNIT_EMBARK",
            UiEventKind::UnitGarrison => "UNIT_GARRISON",
            UiEventKind::UnitShouldDimFlag => "UNIT_SHOULD_DIM_FLAG",
            UiEventKind::UnitMarkThreatening => "UNIT_MARK_THREATENING",
            UiEventKind::UnitVisibility => "UNIT_VISIBILITY",
            UiEventKind::UnitCombat => "UNIT_COMBAT",
            UiEventKind::UnitMissionEnd => "UNIT_MISSION_END",
            UiEventKind::UnitRebased => "UNIT_REBASED",
            UiEventKind::UnitResetAnimation => "UNIT_RESET_ANIMATION",
            UiEventKind::CityCreated => "CITY_CREATED",
            UiEventKind::CityDestroyed => "CITY_DESTROYED",
            UiEventKind::CityCaptured => "CITY_CAPTURED",
            UiEventKind::CitySetDamage => "CITY_SET_DAMAGE",
            UiEventKind::CityPopulationChanged => "CITY_POPULATION_CHANGED",
            UiEventKind::CityCombat => "CITY_COMBAT",
            UiEventKind::EraChanged => "ERA_CHANGED",
            UiEventKind::BridgeChanged => "BRIDGE_CHANGED",
            UiEventKind::TechAcquired => "TECH_ACQUIRED",
            UiEventKind::FeatureChanged => "FEATURE_CHANGED",
            UiEventKind::PlotStateChange => "PLOT_STATE_CHANGE",
            UiEventKind::PlotIconStateChange => "PLOT_ICON_STATE_CHANGE",
            UiEventKind::NaturalWonderRevealed => "NATURAL_WONDER_REVEALED",
            UiEventKind::WallCreated => "WALL_CREATED",
            UiEventKind::DoFx => "DO_FX",
            UiEventKind::FowChanged => "FOW_CHANGED",
            UiEventKind::YieldMightHaveChanged => "YIELD_MIGHT_HAVE_CHANGED",
            UiEventKind::WarStateChanged => "WAR_STATE_CHANGED",
            UiEventKind::MetTeam => "MET_TEAM",
            UiEventKind::DiploAiLeaderMessage => "DIPLO_AI_LEADER_MESSAGE",
            UiEventKind::ClearDiploTradeTable => "CLEAR_DIPLO_TRADE_TABLE",
            UiEventKind::NotifyAiLeadersInGame => "NOTIFY_AI_LEADERS_IN_GAME",
            UiEventKind::NotifySpecificAiLeader => "NOTIFY_SPECIFIC_AI_LEADER",
            UiEventKind::WonderCreated => "WONDER_CREATED",
            UiEventKind::WonderEdited => "WONDER_EDITED",
            UiEventKind::WonderRemoved => "WONDER_REMOVED",
            UiEventKind::SpaceshipCreated => "SPACESHIP_CREATED",
            UiEventKind::SpaceshipEdited => "SPACESHIP_EDITED",
            UiEventKind::SpaceshipRemoved => "SPACESHIP_REMOVED",
            UiEventKind::OpenOptionsScreen => "OPEN_OPTIONS_SCREEN",
            UiEventKind::SearchForPediaEntry => "SEARCH_FOR_PEDIA_ENTRY",
            UiEventKind::OpenInfoCorner => "OPEN_INFO_CORNER",
            UiEventKind::WorldAnchor => "WORLD_ANCHOR",
            UiEventKind::MinimapUnitSelect => "MINIMAP_UNIT_SELECT",
            UiEventKind::MinimapNotification => "MINIMAP_NOTIFICATION",
            UiEventKind::GoldenAgeStarted => "GOLDEN_AGE_STARTED",
            UiEventKind::GoldenAgeEnded => "GOLDEN_AGE_ENDED",
            UiEventKind::TurnChanged => "TURN_CHANGED",
            UiEventKind::ActivePlayerChanged => "ACTIVE_PLAYER_CHANGED",
            UiEventKind::PublishActivePlayer => "PUBLISH_ACTIVE_PLAYER",
            UiEventKind::PublishNewGameTurn => "PUBLISH_NEW_GAME_TURN",
            UiEventKind::PublishEraChanges => "PUBLISH_ERA_CHANGES",
            UiEventKind::TradeNewRoute => "TRADE_NEW_ROUTE",
            UiEventKind::TradeRouteDirection => "TRADE_ROUTE_DIRECTION",
            UiEventKind::TradeDestroyRoute => "TRADE_DESTROY_ROUTE",
            UiEventKind::TradeActivatePopupRoute => "TRADE_ACTIVATE_POPUP_ROUTE",
            UiEventKind::TradeDeactivatePopupRoute => "TRADE_DEACTIVATE_POPUP_ROUTE",
            UiEventKind::ActivePlayerTurnStart => "ACTIVE_PLAYER_TURN_START",
            UiEventKind::ActivePlayerTurnEnd => "ACTIVE_PLAYER_TURN_END",
            UiEventKind::RemotePlayerTurnStart => "REMOTE_PLAYER_TURN_START",
            UiEventKind::RemotePlayerTurnEnd => "REMOTE_PLAYER_TURN_END",
            UiEventKind::PlayerTurnStatus => "PLAYER_TURN_STATUS",
            UiEventKind::NotificationAdded => "NOTIFICATION_ADDED",
            UiEventKind::NotificationRemoved => "NOTIFICATION_REMOVED",
            UiEventKind::NotificationActivated => "NOTIFICATION_ACTIVATED",
            UiEventKind::Message => "MESSAGE",
            UiEventKind::ClearEventMessages => "CLEAR_EVENT_MESSAGES",
            UiEventKind::Popup => "POPUP",
            UiEventKind::PopupArray => "POPUP_ARRAY",
            UiEventKind::PopupText => "POPUP_TEXT",
            UiEventKind::SetDirty => "SET_DIRTY",
            UiEventKind::LookAt => "LOOK_AT",
            UiEventKind::CenterCamera => "CENTER_CAMERA",
            UiEventKind::SpecificCityInfoDirty => "SPECIFIC_CITY_INFO_DIRTY",
            UiEventKind::UpdateEndTurn => "UPDATE_END_TURN",
            UiEventKind::EndTurnBlockingChanged => "END_TURN_BLOCKING_CHANGED",
            UiEventKind::PublishEndTurnDirty => "PUBLISH_END_TURN_DIRTY",
            UiEventKind::UpdateCityScreen => "UPDATE_CITY_SCREEN",
            UiEventKind::UnitSelectionChanged => "UNIT_SELECTION_CHANGED",
            UiEventKind::CitySelectionChanged => "CITY_SELECTION_CHANGED",
            UiEventKind::InterfaceModeChanged => "INTERFACE_MODE_CHANGED",
            UiEventKind::CityScreenChanged => "CITY_SCREEN_CHANGED",
            UiEventKind::UpdateCountryBorder => "UPDATE_COUNTRY_BORDER",
            UiEventKind::UpdateAllCountryBorders => "UPDATE_ALL_COUNTRY_BORDERS",
            UiEventKind::AddHexToUiRange => "ADD_HEX_TO_UI_RANGE",
            UiEventKind::OpenEndGameMenu => "OPEN_END_GAME_MENU",
            UiEventKind::OpenTechTree => "OPEN_TECH_TREE",
            UiEventKind::OpenPlayerDealScreen => "OPEN_PLAYER_DEAL_SCREEN",
            UiEventKind::AddDeferredWonderCommand => "ADD_DEFERRED_WONDER_COMMAND",
            UiEventKind::ToggleStrategicView => "TOGGLE_STRATEGIC_VIEW",
            UiEventKind::SetTempString => "SET_TEMP_STRING",
            UiEventKind::LookAtSelectionPlot => "LOOK_AT_SELECTION_PLOT",
            UiEventKind::LookAtCityOffset => "LOOK_AT_CITY_OFFSET",
            UiEventKind::Flashing => "FLASHING",
            UiEventKind::RefreshYieldVisibleMode => "REFRESH_YIELD_VISIBLE_MODE",
            UiEventKind::GameEvent => "GAME_EVENT",
            UiEventKind::LuaEvent => "LUA_EVENT",
            UiEventKind::Unknown(_) => "UNKNOWN",
        }
    }
}

/// A value of a `GAME_EVENT` / `LUA_EVENT` argument list.
#[derive(Debug, Clone, PartialEq)]
pub enum UiValue {
    Nil,
    Bool(bool),
    Num(f64),
    Str(String),
    /// A flat table of numbers (a hex position, `HexToWorld`'s vector).
    Vec(Vec<f64>),
}

impl UiValue {
    /// The values of an event's data blob (`u8` count, then per value a tag and its payload, little endian).
    /// Anything malformed ends the list early.
    pub fn decode_list(data: &[u8]) -> Vec<UiValue> {
        let mut r = Cursor { b: data, pos: 0 };
        let mut out = Vec::new();
        let Some(count) = r.u8() else {
            return out;
        };
        for _ in 0..count {
            let Some(tag) = r.u8() else { break };
            let v = match tag {
                ffi::GC_UI_VAL_NIL => Some(UiValue::Nil),
                ffi::GC_UI_VAL_BOOL => r.u8().map(|b| UiValue::Bool(b != 0)),
                ffi::GC_UI_VAL_NUM => r.f64().map(UiValue::Num),
                ffi::GC_UI_VAL_STR => (|| {
                    let n = usize::from(r.u16()?);
                    let bytes = r.take(n)?;
                    Some(UiValue::Str(String::from_utf8_lossy(bytes).into_owned()))
                })(),
                ffi::GC_UI_VAL_VEC => (|| {
                    let n = r.u8()?;
                    let mut v = Vec::with_capacity(usize::from(n));
                    for _ in 0..n {
                        v.push(r.f64()?);
                    }
                    Some(UiValue::Vec(v))
                })(),
                _ => None,
            };
            match v {
                Some(v) => out.push(v),
                None => break,
            }
        }
        out
    }
}

struct Cursor<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
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
    fn f64(&mut self) -> Option<f64> {
        self.take(8)
            .map(|s| f64::from_le_bytes(s.try_into().expect("8 bytes")))
    }
}

/// One thing the engine told a replica's UI.
#[derive(Debug, Clone, PartialEq)]
pub struct UiEvent {
    pub kind: UiEventKind,
    /// Produced by `setup_graphical` (the engine re-announcing everything alive after a load).
    pub rebuild: bool,
    /// The integer arguments of the kind (see `gamecore_abi.h`).
    pub args: Vec<i32>,
    /// The kind's text: the `GameEvents` / `Events` name of `GameEvent` / `LuaEvent`, a message, a city's name key.
    pub text: Option<String>,
    /// The second text: a notification's summary, a message's sound.
    pub text2: Option<String>,
    /// The arguments of a `GameEvent` / `LuaEvent`.
    pub values: Vec<UiValue>,
    /// The plot indices of a `UnitMoved` path; of a `TradeNewRoute` with more than 9 plots, all of them as x, y pairs (i32 as u32).
    pub path: Vec<u32>,
}

impl UiEvent {
    /// An event with integer arguments only.
    pub fn new(kind: UiEventKind, args: Vec<i32>) -> UiEvent {
        UiEvent {
            kind,
            rebuild: false,
            args,
            text: None,
            text2: None,
            values: Vec::new(),
            path: Vec::new(),
        }
    }

    pub(crate) fn from_raw(raw: RawUiEvent) -> UiEvent {
        let kind = UiEventKind::from_code(raw.kind);
        let (values, path) = match kind {
            UiEventKind::GameEvent | UiEventKind::LuaEvent => {
                (UiValue::decode_list(&raw.data), Vec::new())
            }
            UiEventKind::UnitMoved | UiEventKind::TradeNewRoute => (
                Vec::new(),
                raw.data
                    .chunks_exact(4)
                    .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                    .collect(),
            ),
            _ => (Vec::new(), Vec::new()),
        };
        UiEvent {
            kind,
            rebuild: raw.flags & ffi::GC_UI_F_REBUILD != 0,
            args: raw.args,
            text: raw.text,
            text2: raw.text2,
            values,
            path,
        }
    }
}

/// An operation on the exe's interface state that the DLL reads by pointer (selection, interface mode, city screen).
/// "unit" and "city" are `(owner, id)` pairs, `(-1, -1)` = none; the inputs and outputs of each operation are listed in `gamecore_abi.h`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UiOp {
    /// `SELECT_UNIT` (1): in: owner, id [, clear 0/1 (default 1), toggle 0/1 (default 0)]
    SelectUnit,
    /// `SELECT_GROUP` (2): in: owner, id [, shift, ctrl, alt]; the unit's group (ctrl: every unit of its type on the plot, alt: every unit of the owner on the plot)
    SelectGroup,
    /// `HEAD_SELECTED_UNIT` (3): out: owner, id
    HeadSelectedUnit,
    /// `SELECTED_UNITS` (4): out: owner, id, owner, id, ... (as many as fit; the first out value is not a count)
    SelectedUnits,
    /// `CLEAR_SELECTION` (5): -
    ClearSelection,
    /// `LAST_SELECTED_UNIT` (6): out: owner, id
    LastSelectedUnit,
    /// `SELECT_CITY` (7): in: owner, id [, test production 0/1]
    SelectCity,
    /// `HEAD_SELECTED_CITY` (8): out: owner, id
    HeadSelectedCity,
    /// `CLEAR_SELECTED_CITIES` (9): -
    ClearSelectedCities,
    /// `DO_SELECT_CITY_AT_PLOT` (10): in: x, y; out: selected 0/1 (a city of the seat's player on that plot is selected, as UI.DoSelectCityAtPlot)
    DoSelectCityAtPlot,
    /// `SET_INTERFACE_MODE` (11): in: InterfaceModeTypes
    SetInterfaceMode,
    /// `GET_INTERFACE_MODE` (12): out: mode
    GetInterfaceMode,
    /// `SET_INTERFACE_MODE_VALUE` (13): in: value
    SetInterfaceModeValue,
    /// `GET_INTERFACE_MODE_VALUE` (14): out: value
    GetInterfaceModeValue,
    /// `CAN_DO_INTERFACE_MODE` (15): in: mode [, test visible 0/1]; out: 0/1
    CanDoInterfaceMode,
    /// `SET_CITY_SCREEN_UP` (16): in: 0/1
    SetCityScreenUp,
    /// `IS_CITY_SCREEN_UP` (17): out: 0/1
    IsCityScreenUp,
    /// `CAN_SELECTION_LIST_FOUND` (18): out: 0/1
    CanSelectionListFound,
    /// `CAN_SELECTION_LIST_WORK` (19): out: 0/1
    CanSelectionListWork,
    /// `CAN_END_TURN` (20): out: 0/1 (the host's canEndTurn of the seat)
    CanEndTurn,
    /// `ACTIVATE_NOTIFICATION` (21): in: lookup id of a notification of the seat: the DLL's CvNotifications::Activate (popup, look at, selection) comes out of the sink
    ActivateNotification,
    /// `REMOVE_NOTIFICATION` (22): in: lookup id: NOTIFICATION_REMOVED for the UI; the game state (dismissed flag) is the server's and does not change
    RemoveNotification,
    /// `GET_CYCLE_SELECTION_COUNTER` (23): out: counter
    GetCycleSelectionCounter,
    /// `SET_CYCLE_SELECTION_COUNTER` (24): in: counter
    SetCycleSelectionCounter,
    /// `IS_UNIT_SELECTED` (25): in: owner, id; out: 0/1
    IsUnitSelected,
    /// `IS_CITY_SELECTED` (26): in: owner, id; out: 0/1
    IsCitySelected,
    /// `SELECT_ALL_AT_PLOT` (27): in: x, y; every unit of the seat's player on the plot
    SelectAllAtPlot,
    /// `GET_SELECTION_COUNT` (28): out: number of selected units
    GetSelectionCount,
}

impl UiOp {
    /// Every operation, in the order of the header.
    pub const ALL: [UiOp; 28] = [
        UiOp::SelectUnit,
        UiOp::SelectGroup,
        UiOp::HeadSelectedUnit,
        UiOp::SelectedUnits,
        UiOp::ClearSelection,
        UiOp::LastSelectedUnit,
        UiOp::SelectCity,
        UiOp::HeadSelectedCity,
        UiOp::ClearSelectedCities,
        UiOp::DoSelectCityAtPlot,
        UiOp::SetInterfaceMode,
        UiOp::GetInterfaceMode,
        UiOp::SetInterfaceModeValue,
        UiOp::GetInterfaceModeValue,
        UiOp::CanDoInterfaceMode,
        UiOp::SetCityScreenUp,
        UiOp::IsCityScreenUp,
        UiOp::CanSelectionListFound,
        UiOp::CanSelectionListWork,
        UiOp::CanEndTurn,
        UiOp::ActivateNotification,
        UiOp::RemoveNotification,
        UiOp::GetCycleSelectionCounter,
        UiOp::SetCycleSelectionCounter,
        UiOp::IsUnitSelected,
        UiOp::IsCitySelected,
        UiOp::SelectAllAtPlot,
        UiOp::GetSelectionCount,
    ];

    /// The ABI code (`ffi::GC_UI_OP_*`).
    pub fn code(self) -> i32 {
        match self {
            UiOp::SelectUnit => 1,
            UiOp::SelectGroup => 2,
            UiOp::HeadSelectedUnit => 3,
            UiOp::SelectedUnits => 4,
            UiOp::ClearSelection => 5,
            UiOp::LastSelectedUnit => 6,
            UiOp::SelectCity => 7,
            UiOp::HeadSelectedCity => 8,
            UiOp::ClearSelectedCities => 9,
            UiOp::DoSelectCityAtPlot => 10,
            UiOp::SetInterfaceMode => 11,
            UiOp::GetInterfaceMode => 12,
            UiOp::SetInterfaceModeValue => 13,
            UiOp::GetInterfaceModeValue => 14,
            UiOp::CanDoInterfaceMode => 15,
            UiOp::SetCityScreenUp => 16,
            UiOp::IsCityScreenUp => 17,
            UiOp::CanSelectionListFound => 18,
            UiOp::CanSelectionListWork => 19,
            UiOp::CanEndTurn => 20,
            UiOp::ActivateNotification => 21,
            UiOp::RemoveNotification => 22,
            UiOp::GetCycleSelectionCounter => 23,
            UiOp::SetCycleSelectionCounter => 24,
            UiOp::IsUnitSelected => 25,
            UiOp::IsCitySelected => 26,
            UiOp::SelectAllAtPlot => 27,
            UiOp::GetSelectionCount => 28,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_round_trip_and_stay_unique() {
        let mut seen = std::collections::HashSet::new();
        for k in UiEventKind::ALL {
            assert_eq!(UiEventKind::from_code(k.code()), k, "{}", k.name());
            assert!(seen.insert(k.code()), "{} repeats a code", k.name());
        }
        assert_eq!(UiEventKind::from_code(9999), UiEventKind::Unknown(9999));
        assert_eq!(UiEventKind::Unknown(9999).code(), 9999);
        assert_eq!(UiEventKind::UnitCreated.code(), ffi::GC_UI_EV_UNIT_CREATED);
        assert_eq!(UiEventKind::GameEvent.code(), ffi::GC_UI_EV_GAME_EVENT);
        assert_eq!(UiEventKind::LuaEvent.code(), ffi::GC_UI_EV_LUA_EVENT);
        assert_eq!(UiEventKind::NotificationAdded.name(), "NOTIFICATION_ADDED");
    }

    /// Every `GC_UI_EV_*` / `GC_UI_OP_*` of the header (the copy of the Lekmod original) is a known kind / operation.
    #[test]
    fn the_header_is_covered() {
        let header = include_str!("../../gamecore-sys/cpp/include/gamecore_abi.h");
        let (mut kinds, mut ops) = (0, 0);
        for line in header.lines() {
            let mut words = line.split_whitespace();
            if words.next() != Some("#define") {
                continue;
            }
            let (Some(name), Some(code)) = (words.next(), words.next()) else {
                continue;
            };
            let Ok(code) = code.parse::<i32>() else {
                continue;
            };
            if let Some(n) = name.strip_prefix("GC_UI_EV_") {
                let k = UiEventKind::from_code(code);
                assert_eq!(k.name(), n, "kind {code}");
                kinds += 1;
            } else if let Some(n) = name.strip_prefix("GC_UI_OP_")
                && n != "COUNT"
            {
                assert!(UiOp::ALL.iter().any(|o| o.code() == code), "op {n}");
                ops += 1;
            }
        }
        assert_eq!(kinds, UiEventKind::ALL.len());
        assert_eq!(ops, UiOp::ALL.len());
    }

    #[test]
    fn ops_are_the_headers() {
        let mut codes: Vec<i32> = UiOp::ALL.iter().map(|o| o.code()).collect();
        codes.sort_unstable();
        assert_eq!(codes, (1..ffi::GC_UI_OP_COUNT).collect::<Vec<_>>());
        assert_eq!(UiOp::SelectUnit.code(), ffi::GC_UI_OP_SELECT_UNIT);
        assert_eq!(
            UiOp::GetSelectionCount.code(),
            ffi::GC_UI_OP_GET_SELECTION_COUNT
        );
    }

    #[test]
    fn values_decode() {
        let mut blob = vec![5_u8];
        blob.extend([ffi::GC_UI_VAL_NIL]);
        blob.extend([ffi::GC_UI_VAL_BOOL, 1]);
        blob.push(ffi::GC_UI_VAL_NUM);
        blob.extend(2.5_f64.to_le_bytes());
        blob.push(ffi::GC_UI_VAL_STR);
        blob.extend(2_u16.to_le_bytes());
        blob.extend(*b"hi");
        blob.extend([ffi::GC_UI_VAL_VEC, 2]);
        blob.extend(1.0_f64.to_le_bytes());
        blob.extend((-3.0_f64).to_le_bytes());
        assert_eq!(
            UiValue::decode_list(&blob),
            vec![
                UiValue::Nil,
                UiValue::Bool(true),
                UiValue::Num(2.5),
                UiValue::Str("hi".into()),
                UiValue::Vec(vec![1.0, -3.0]),
            ]
        );
        // Truncated at every byte: never a panic, only a shorter list.
        for cut in 0..blob.len() {
            let _ = UiValue::decode_list(&blob[..cut]);
        }
        assert!(UiValue::decode_list(&[]).is_empty());
        assert!(UiValue::decode_list(&[1, 99]).is_empty());
    }

    #[test]
    fn events_unpack() {
        let raw = RawUiEvent {
            kind: ffi::GC_UI_EV_UNIT_MOVED,
            flags: ffi::GC_UI_F_REBUILD,
            args: vec![1, 2],
            text: Some("t".into()),
            text2: None,
            data: vec![1, 0, 0, 0, 0x10, 0x27, 0, 0],
        };
        let e = UiEvent::from_raw(raw);
        assert_eq!(e.kind, UiEventKind::UnitMoved);
        assert!(e.rebuild);
        assert_eq!(e.path, vec![1, 10_000]);
        assert!(e.values.is_empty());
    }
}
