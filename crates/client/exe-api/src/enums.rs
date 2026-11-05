//! `Mouse`, `Keys`, `KeyEvents`, `MouseEvents`, `PopupPriority`: the tables of `cvLuaUIEnumsLibrary`.
//!
//! The reverse headers hold no values for them ("enum tables must come from data"), so the values are
//! the ones the rest of the client fixes: the Win32 messages and virtual-key codes `ui` hands to the
//! input handlers of the contexts (`ui::input`, `ui::core::vk_of`), and the callback slots of
//! `RegisterCallback` (`ui::lua_api`): 0 left click, 1 left double click, 2 right click, 3 middle click,
//! 5 mouse enter, 6 mouse exit. A test runs the real `ui` against these numbers.

use std::rc::Rc;

use lua_sys::{Lua, Result, Table};

use crate::Inner;
use crate::registry::Reg;

/// `RegisterCallback` slots, by the names of `Mouse`.
pub const MOUSE: &[(&str, i32)] = &[
    ("eLClick", 0),
    // Vanilla CityView spells it this way; the exe has the key.
    ("eLCLick", 0),
    // CommonBehaviors.lua (the tabs of the policy popup) spells it this way (SIMPLIFIED(SIMP-exe-100)).
    ("eLclick", 0),
    ("eLDblClick", 1),
    ("eRClick", 2),
    ("eMClick", 3),
    ("eMouseEnter", 5),
    ("eMouseExit", 6),
];

/// Win32 messages the input handlers get as `uiMsg`.
pub const MOUSE_EVENTS: &[(&str, i32)] = &[
    ("MouseMove", 0x200),
    ("LButtonDown", 0x201),
    ("LButtonUp", 0x202),
    ("LButtonDoubleClick", 0x203),
    ("RButtonDown", 0x204),
    ("RButtonUp", 0x205),
    ("RButtonDoubleClick", 0x206),
    ("MButtonDown", 0x207),
    ("MButtonUp", 0x208),
    ("MButtonDoubleClick", 0x209),
    ("MouseWheel", 0x20a),
    ("PointerUpdate", 0x245),
    ("PointerDown", 0x246),
    ("PointerUp", 0x247),
];

/// Reasons a player left the game (`NetKicked`, the argument of `MultiplayerGamePlayerDisconnected`
/// handlers). SIMPLIFIED(SIMP-exe-21): the numbers are ours; the names are the ones the scripts use.
pub const NET_KICKED: &[(&str, i32)] = &[("BY_HOST", 1), ("NO_HOST", 2), ("NO_ROOM", 3)];

pub const KEY_EVENTS: &[(&str, i32)] = &[("KeyDown", 0x100), ("KeyUp", 0x101)];

/// Windows virtual-key codes.
pub const KEYS: &[(&str, i32)] = &[
    ("VK_BACK", 0x08),
    ("VK_TAB", 0x09),
    ("VK_CLEAR", 0x0c),
    ("VK_RETURN", 0x0d),
    ("VK_SHIFT", 0x10),
    ("VK_CONTROL", 0x11),
    ("VK_MENU", 0x12),
    ("VK_PAUSE", 0x13),
    ("VK_CAPITAL", 0x14),
    ("VK_ESCAPE", 0x1b),
    ("VK_SPACE", 0x20),
    ("VK_PRIOR", 0x21),
    ("VK_NEXT", 0x22),
    ("VK_END", 0x23),
    ("VK_HOME", 0x24),
    ("VK_LEFT", 0x25),
    ("VK_UP", 0x26),
    ("VK_RIGHT", 0x27),
    ("VK_DOWN", 0x28),
    ("VK_SELECT", 0x29),
    ("VK_PRINT", 0x2a),
    ("VK_EXECUTE", 0x2b),
    ("VK_SNAPSHOT", 0x2c),
    ("VK_INSERT", 0x2d),
    ("VK_DELETE", 0x2e),
    ("VK_HELP", 0x2f),
    ("VK_LWIN", 0x5b),
    ("VK_RWIN", 0x5c),
    ("VK_APPS", 0x5d),
    ("VK_NUMPAD0", 0x60),
    ("VK_NUMPAD1", 0x61),
    ("VK_NUMPAD2", 0x62),
    ("VK_NUMPAD3", 0x63),
    ("VK_NUMPAD4", 0x64),
    ("VK_NUMPAD5", 0x65),
    ("VK_NUMPAD6", 0x66),
    ("VK_NUMPAD7", 0x67),
    ("VK_NUMPAD8", 0x68),
    ("VK_NUMPAD9", 0x69),
    ("VK_MULTIPLY", 0x6a),
    ("VK_ADD", 0x6b),
    ("VK_SEPARATOR", 0x6c),
    ("VK_SUBTRACT", 0x6d),
    ("VK_DECIMAL", 0x6e),
    ("VK_DIVIDE", 0x6f),
    ("VK_F1", 0x70),
    ("VK_F2", 0x71),
    ("VK_F3", 0x72),
    ("VK_F4", 0x73),
    ("VK_F5", 0x74),
    ("VK_F6", 0x75),
    ("VK_F7", 0x76),
    ("VK_F8", 0x77),
    ("VK_F9", 0x78),
    ("VK_F10", 0x79),
    ("VK_F11", 0x7a),
    ("VK_F12", 0x7b),
    ("VK_NUMLOCK", 0x90),
    ("VK_SCROLL", 0x91),
    ("VK_LSHIFT", 0xa0),
    ("VK_RSHIFT", 0xa1),
    ("VK_LCONTROL", 0xa2),
    ("VK_RCONTROL", 0xa3),
    ("VK_LMENU", 0xa4),
    ("VK_RMENU", 0xa5),
    ("VK_OEM_1", 0xba),
    ("VK_OEM_PLUS", 0xbb),
    ("VK_OEM_COMMA", 0xbc),
    ("VK_OEM_MINUS", 0xbd),
    ("VK_OEM_PERIOD", 0xbe),
    ("VK_OEM_2", 0xbf),
    ("VK_OEM_3", 0xc0),
    ("VK_OEM_4", 0xdb),
    ("VK_OEM_5", 0xdc),
    ("VK_OEM_6", 0xdd),
    ("VK_OEM_7", 0xde),
    ("VK_OEM_8", 0xdf),
];

/// Names of `PopupPriority`, lowest priority first. `Current` is the priority of the popup that is up.
/// SIMPLIFIED(SIMP-exe-10): the numbers are the positions in this list; the exe's are not recovered.
pub const POPUP_PRIORITY: &[&str] = &[
    "Default",
    "Current",
    "LegalScreen",
    "MainMenu",
    "SinglePlayerScreen",
    "MultiplayerSelectScreen",
    "ModMultiplayerSelectScreen",
    "ModdingSinglePlayer",
    "ModsMenuScreen",
    "ModsBrowserScreen",
    "ModsCustom",
    "CustomMod",
    "ModsEULAScreen",
    "ModdingGameSetupScreen",
    "GameSetupScreen",
    "MPGameSetupScreen",
    "AdvancedSetup",
    "SelectCivilization",
    "LobbyScreen",
    "StagingScreen",
    "JoiningScreen",
    "OtherMenu",
    "HallOfFame",
    "Credits",
    "Leaderboard",
    "LoadMenu",
    "LoadGameScreen",
    "MPLoadGameScreen",
    "LoadTutorial",
    "Civilopedia",
    "TechTree",
    "ProductionPopup",
    "MilitaryOverview",
    "EconOverview",
    "Demographics",
    "DiploOverview",
    "NotificationLog",
    "WhosWinning",
    "VictoryProgress",
    "SocialPolicy",
    "LeaderHead",
    "LeaderHeadPopup",
    // The leader screen of the bc1 UI queues itself at LeaderDiscuss and the trade table must cover it (K12).
    "LeaderDiscuss",
    "LeaderTrade",
    "CityStateGreeting",
    "CityStateDiplo",
    "DiploVote",
    "VoteResults",
    "TechAward",
    "NewEraPopup",
    "NaturalWonderPopup",
    "GoodyHut",
    "BarbarianCamp",
    "GoldenAge",
    "GreatPersonReward",
    "Priority_GreatPersonReward",
    "GreatWorkPopup",
    "WonderPopup",
    "TextPopup",
    "GenericPopup",
    "AdvisorInfo",
    "AdvisorCounsel",
    "AdvisorModal",
    "EndGameMenu",
    "InGameMenu",
    // The sub-menus of the game menu are queued while it is up and replace it (the GameMenu context counts
    // the shows and hides: "Moving between sub-menus causes this screen to be shown and hidden"), so they
    // rank above it.
    "SaveMenu",
    "SaveMapMenu",
    "OptionsMenu",
    "InGameUtmost",
    "eUtmost",
];

/// The value of a `Keys` letter or digit: `Keys.Z` is the character code of `Z`, `Keys["1"]` that of `1`.
fn letters() -> impl Iterator<Item = (String, i32)> {
    ('0'..='9')
        .map(|c| (c.to_string(), c as i32))
        .chain(letters_only())
}

fn letters_only() -> impl Iterator<Item = (String, i32)> {
    ('A'..='Z').map(|c| (c.to_string(), c as i32))
}

fn table_of<'l>(
    reg: &Reg<'_>,
    lua: &'l Lua,
    g: &Table<'l>,
    name: &str,
    items: impl IntoIterator<Item = (String, i32)>,
) -> Result<()> {
    let t = lua.create_table();
    for (k, v) in items {
        reg.value(&t, name, &k, v)?;
    }
    g.raw_set(name, t)?;
    reg.scope(name);
    Ok(())
}

/// UI enum tables of `cvLuaUIEnumsLibrary` that the game core does not carry; values from the exe.
const OTHER_ENUMS: &[(&str, &[(&str, i32)])] = &[
    (
        "YieldDisplayTypes",
        &[
            ("USER_ALL_ON", 0),
            ("USER_ALL_OFF", 1),
            ("USER_ALL_RESOURCE_ON", 2),
            ("USER_ALL_RESOURCE_OFF", 3),
            ("EMPIRE", 4),
            ("CITY_OWNED", 5),
            ("CITY_WORKED", 6),
            ("CITY_PURCHASABLE", 7),
            ("AREA", 8),
        ],
    ),
    (
        "ButtonStates",
        &[
            ("eNormal", 0),
            ("eMouseOver", 1),
            ("eMouseDown", 2),
            ("eDisabled", 3),
        ],
    ),
    (
        "GameViewTypes",
        &[
            ("GAMEVIEW_NONE", 0),
            ("GAMEVIEW_STANDARD", 1),
            ("GAMEVIEW_STRATEGIC", 2),
        ],
    ),
    (
        "InfoCornerID",
        &[
            ("None", 0),
            ("Tech", 1),
            ("Units", 2),
            ("Cities", 3),
            ("Diplo", 4),
        ],
    ),
];

pub(crate) fn install<'l>(inner: &Rc<Inner>, lua: &'l Lua, g: &Table<'l>) -> Result<()> {
    let reg = Reg::new(inner);
    let own = |items: &[(&str, i32)]| -> Vec<(String, i32)> {
        items.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect()
    };
    table_of(&reg, lua, g, "Mouse", own(MOUSE))?;
    table_of(&reg, lua, g, "MouseEvents", own(MOUSE_EVENTS))?;
    table_of(&reg, lua, g, "KeyEvents", own(KEY_EVENTS))?;
    table_of(&reg, lua, g, "NetKicked", own(NET_KICKED))?;
    table_of(&reg, lua, g, "Keys", own(KEYS).into_iter().chain(letters()))?;
    table_of(
        &reg,
        lua,
        g,
        "PopupPriority",
        POPUP_PRIORITY
            .iter()
            .enumerate()
            .map(|(i, n)| ((*n).to_owned(), i as i32)),
    )?;
    for (name, items) in OTHER_ENUMS {
        if matches!(
            g.raw_get::<_, lua_sys::LuaValue>(*name)?,
            lua_sys::LuaValue::Nil
        ) {
            table_of(&reg, lua, g, name, own(items))?;
        }
    }
    Ok(())
}
