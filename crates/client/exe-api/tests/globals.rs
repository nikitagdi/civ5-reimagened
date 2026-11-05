//! The globals WO-5 gives: user data that survives, content packages, the enums against the real `ui`,
//! the options store, the hex helpers, `PreGame`, `Locale`.
mod support;

use exe_api::{ExeApi, ExeConfig, Status};
use platform::{
    InputEvent, KeyValueStore, MemoryStore, Modifiers, MouseButton, PhysicalPosition, PointerEvent,
    PointerId, PointerKind, PointerPhase,
};
use support::*;

const BNW: &str = "6DA07636-4123-4018-B643-6575B4EC336B";
const GK: &str = "0E3751A1-F840-4e1b-9706-519BF484E59D";

#[test]
fn open_user_data_survives_a_new_exe_api_on_the_same_store() {
    let store = MemoryStore::new();
    {
        let api = api(&store);
        let lua = lua_with(&api);
        run(
            &lua,
            r#"
            local eui = Modding.OpenUserData("Enhanced User Interface Options", 1)
            eui.SetValue("UnitRibbon", 0)
            eui.SetValue("RIBBON_UNIT_WARRIOR", 1)
            eui.SetValue("Mode", "wide")
            eui.SetValue("AutoClose", true)
            Modding.OpenUserData("LekmodData", 1).SetValue("bEnabledStratView", 1)
            Modding.OpenUserData("IngameHotkeyManager", 1).SetValue("bShowInvalidActions", 1)
            "#,
        );
        // Nothing is written before the flush (once a second, or on pagehide).
        assert!(store.is_empty());
        api.flush();
        assert_eq!(store.len(), 3);
    }
    let api = api(&store);
    let lua = lua_with(&api);
    run(
        &lua,
        r#"eui = Modding.OpenUserData("Enhanced User Interface Options", 1)"#,
    );
    assert_eq!(eval_num(&lua, r#"eui.GetValue("UnitRibbon")"#), 0.0);
    assert_eq!(
        eval_num(&lua, r#"eui.GetValue("RIBBON_UNIT_WARRIOR")"#),
        1.0
    );
    assert_eq!(eval_str(&lua, r#"eui.GetValue("Mode")"#), "wide");
    // A boolean is stored as 1 / 0, as the exe does (the Lekmod NotificationPanel compares `== 0` / `== 1`).
    assert_eq!(eval_num(&lua, r#"eui.GetValue("AutoClose")"#), 1.0);
    assert!(eval_bool(&lua, r#"eui.GetValue("missing") == nil"#));
    assert_eq!(
        eval_num(
            &lua,
            r#"Modding.OpenUserData("LekmodData", 1).GetValue("bEnabledStratView")"#
        ),
        1.0
    );
    assert_eq!(
        eval_num(
            &lua,
            r#"Modding.OpenUserData("IngameHotkeyManager", 1).GetValue("bShowInvalidActions")"#
        ),
        1.0
    );
    // Another version is another document.
    assert!(eval_bool(
        &lua,
        r#"Modding.OpenUserData("LekmodData", 2).GetValue("bEnabledStratView") == nil"#
    ));
}

#[test]
fn eui_options_get_our_defaults_once_and_keep_the_players_change() {
    let store = MemoryStore::new();
    // Saved by an older build: the civ list off, the ribbons on.
    store.set(
        "civ5.userdata.Enhanced User Interface Options.1",
        r#"{"CivRibbon":false,"UnitRibbon":1,"Clock":1}"#,
    );
    {
        let api = api(&store);
        let lua = lua_with(&api);
        run(
            &lua,
            r#"eui = Modding.OpenUserData("Enhanced User Interface Options", 1)"#,
        );
        assert_eq!(eval_num(&lua, r#"eui.GetValue("CivRibbon")"#), 1.0);
        assert_eq!(eval_num(&lua, r#"eui.GetValue("UnitRibbon")"#), 0.0);
        assert_eq!(eval_num(&lua, r#"eui.GetValue("CityRibbon")"#), 0.0);
        assert_eq!(eval_num(&lua, r#"eui.GetValue("Clock")"#), 1.0);
        // The player turns the unit ribbon back on.
        run(&lua, r#"eui.SetValue("UnitRibbon", true)"#);
        api.flush();
    }
    let api = api(&store);
    let lua = lua_with(&api);
    run(
        &lua,
        r#"eui = Modding.OpenUserData("Enhanced User Interface Options", 1)"#,
    );
    assert_eq!(eval_num(&lua, r#"eui.GetValue("UnitRibbon")"#), 1.0);
    assert_eq!(eval_num(&lua, r#"eui.GetValue("CityRibbon")"#), 0.0);
}

#[test]
fn user_data_is_written_by_the_tick_once_a_second_and_when_the_state_goes() {
    let store = MemoryStore::new();
    let api1 = api(&store);
    let lua = lua_with(&api1);
    run(
        &lua,
        r#"d = Modding.OpenUserData("X", 1); d.SetValue("a", 1)"#,
    );
    api1.tick(0.4);
    assert!(store.is_empty(), "too early");
    api1.tick(1.2);
    assert_eq!(store.len(), 1);
    // A change after the write waits for the next one, or for the end of the state.
    run(&lua, r#"d.SetValue("a", 2)"#);
    api1.tick(1.5);
    let key = "civ5.userdata.X.1";
    assert_eq!(store.get(key).as_deref(), Some(r#"{"a":1}"#));
    drop(lua);
    drop(api1);
    assert_eq!(store.get(key).as_deref(), Some(r#"{"a":2}"#));
}

#[test]
fn content_manager_reports_both_expansions_active() {
    let store = MemoryStore::new();
    let api = api(&store);
    let lua = lua_with(&api);
    assert!(eval_bool(
        &lua,
        &format!("ContentManager.IsActive('{BNW}', ContentType.GAMEPLAY)")
    ));
    assert!(eval_bool(
        &lua,
        &format!("ContentManager.IsActive('{GK}', ContentType.GAMEPLAY)")
    ));
    assert!(eval_bool(
        &lua,
        &format!(
            "ContentManager.IsActive('{{{}}}', ContentType.GAMEPLAY)",
            BNW.to_lowercase()
        )
    ));
    assert!(!eval_bool(
        &lua,
        "ContentManager.IsActive('00000000-0000-0000-0000-000000000000', ContentType.GAMEPLAY)"
    ));
    assert_eq!(eval_num(&lua, "#ContentManager.GetAllPackageIDs()"), 2.0);
    // The way EUI_context.lua prints them.
    run(
        &lua,
        "for _, v in pairs(ContentManager.GetAllPackageIDs()) do assert(Locale.Lookup(ContentManager.GetPackageDescription(v)) ~= '') end",
    );
}

#[test]
fn strategic_view_is_off_and_keys_have_the_windows_codes() {
    let store = MemoryStore::new();
    let api = api(&store);
    let lua = lua_with(&api);
    assert!(!eval_bool(&lua, "InStrategicView()"));
    // `civ5_mode = InStrategicView ~= nil`
    assert!(eval_bool(&lua, "InStrategicView ~= nil"));
    assert_eq!(eval_num(&lua, "Keys.VK_ESCAPE"), 27.0);
    assert_eq!(eval_num(&lua, "Keys.VK_RETURN"), 13.0);
    assert_eq!(eval_num(&lua, "Keys.Z"), 90.0);
    assert_eq!(eval_num(&lua, "KeyEvents.KeyDown"), 256.0);
    assert_eq!(eval_num(&lua, "MouseEvents.LButtonDown"), 513.0);
    assert_eq!(
        eval_num(&lua, "Mouse.eLCLick"),
        eval_num(&lua, "Mouse.eLClick")
    );
    assert!(eval_num(&lua, "PopupPriority.eUtmost") > eval_num(&lua, "PopupPriority.InGameUtmost"));
    assert!(
        eval_num(&lua, "PopupPriority.InGameUtmost") > eval_num(&lua, "PopupPriority.GenericPopup")
    );
}

fn ptr(phase: PointerPhase, x: f64, y: f64, button: Option<MouseButton>) -> InputEvent {
    InputEvent::Pointer(PointerEvent {
        id: PointerId::MOUSE,
        kind: PointerKind::Mouse,
        phase,
        position: PhysicalPosition { x, y },
        button,
        modifiers: Modifiers::default(),
    })
}

/// `Mouse.eLClick` is the slot `ui` calls on a left click, `eRClick` on a right click, `eMouseEnter` and
/// `eMouseExit` when the pointer comes and goes, and the Win32 messages are the ones the input handlers get.
#[test]
fn mouse_enum_matches_the_slots_and_messages_of_ui() {
    let store = MemoryStore::new();
    let api = api(&store);
    let mut core = ui_with(
        &api,
        &[
            (
                "UI/Root.xml",
                r#"<Context><Button ID="B" Size="100,100" Anchor="L,T"/></Context>"#,
            ),
            (
                "UI/Root.lua",
                r#"
                Controls.B:RegisterCallback(Mouse.eLClick, function() print("left") end)
                Controls.B:RegisterCallback(Mouse.eRClick, function() print("right") end)
                Controls.B:RegisterCallback(Mouse.eMouseEnter, function() print("enter") end)
                Controls.B:RegisterCallback(Mouse.eMouseExit, function() print("exit") end)
                ContextPtr:SetInputHandler(function(msg, w, l)
                    if msg == MouseEvents.LButtonUp then print("msg LButtonUp") end
                    if msg == KeyEvents.KeyDown and w == Keys.VK_ESCAPE then print("msg Escape") end
                    return false
                end)
                "#,
            ),
        ],
    );
    core.load_root("Assets/UI/Root");
    core.update(0.016);
    core.runtime().take_log();
    core.handle_input(&ptr(PointerPhase::Move, 10.0, 10.0, None));
    let left = [
        core.handle_input(&ptr(
            PointerPhase::Down,
            10.0,
            10.0,
            Some(MouseButton::Left),
        )),
        core.handle_input(&ptr(PointerPhase::Up, 10.0, 10.0, Some(MouseButton::Left))),
    ];
    assert_eq!(left[0], ui::InputResult::Consumed);
    core.handle_input(&ptr(
        PointerPhase::Down,
        10.0,
        10.0,
        Some(MouseButton::Right),
    ));
    core.handle_input(&ptr(PointerPhase::Up, 10.0, 10.0, Some(MouseButton::Right)));
    core.handle_input(&ptr(PointerPhase::Move, 500.0, 500.0, None));
    core.handle_input(&InputEvent::Key(platform::KeyEvent {
        key: platform::Key::Named(platform::NamedKey::Escape),
        code: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::default(),
    }));
    let log = core.runtime().take_log();
    for want in ["enter", "left", "right", "exit"] {
        assert!(log.iter().any(|l| l == want), "{want} not in {log:?}");
    }
    assert!(log.iter().any(|l| l == "msg Escape"), "{log:?}");
    assert!(
        core.runtime().recent_errors().is_empty(),
        "{:?}",
        core.runtime().recent_errors()
    );
}

#[test]
fn options_cached_set_waits_for_commit_and_survives() {
    let store = MemoryStore::new();
    {
        let api = api(&store);
        let lua = lua_with(&api);
        // The defaults of UserSettings.ini.
        assert!(eval_bool(&lua, "OptionsManager.GetGridOn()"));
        assert!(!eval_bool(&lua, "OptionsManager.GetYieldOn()"));
        assert!(!eval_bool(&lua, "OptionsManager.IsNoBasicHelp()"));
        assert!(eval_bool(&lua, "OptionsManager.GetSmallUIAssets()"));
        assert_eq!(eval_num(&lua, "OptionsManager.GetTooltip1Seconds()"), 150.0);
        assert_eq!(eval_num(&lua, "OptionsManager.GetTooltip2Seconds()"), 500.0);
        run(
            &lua,
            "OptionsManager.SetGridOn_Cached(false); OptionsManager.SetYieldOn_Cached(true)",
        );
        assert!(
            eval_bool(&lua, "OptionsManager.GetGridOn()"),
            "cached, not committed"
        );
        assert!(store.get("civ5.options.GridOn").is_none());
        run(&lua, "OptionsManager.CommitGameOptions(true)");
        assert!(!eval_bool(&lua, "OptionsManager.GetGridOn()"));
        assert!(eval_bool(&lua, "OptionsManager.GetYieldOn()"));
        assert_eq!(store.get("civ5.options.GridOn").as_deref(), Some("0"));
        assert_eq!(store.get("civ5.options.YieldOn").as_deref(), Some("1"));
    }
    let api = api(&store);
    let lua = lua_with(&api);
    assert!(!eval_bool(&lua, "OptionsManager.GetGridOn()"));
    assert!(eval_bool(&lua, "OptionsManager.GetYieldOn()"));
    // The options menu (K20) calls the graphics functions: they store the pending values like the game ones
    // (WO-G, SIMP-exe-13: they drive nothing in the browser) and run without a stub's counter.
    run(
        &lua,
        "OptionsManager.SetVSync_Cached(false); OptionsManager.CommitGraphicsOptions()",
    );
    assert_eq!(api.stats().simp_calls, 0);
    assert!(!eval_bool(&lua, "OptionsManager.GetVSync()"));
    assert_eq!(store.get("civ5.options.VSync").as_deref(), Some("0"));
}

#[test]
fn hex_helpers_follow_the_exe_shapes() {
    let store = MemoryStore::new();
    let api = api(&store);
    let lua = lua_with(&api);
    // UnitPanel: `ToHexFromGrid{ x = .., y = .. }`; flags: `Map_GetPlot( ToGridFromHex( hexPos.x, hexPos.y ) )`.
    run(&lua, "hex = ToHexFromGrid{ x = 5, y = 3 }");
    assert_eq!(eval_num(&lua, "hex.x"), 4.0);
    assert_eq!(eval_num(&lua, "hex.y"), 3.0);
    assert_eq!(
        eval_num(&lua, "select('#', ToGridFromHex(hex.x, hex.y))"),
        2.0
    );
    assert_eq!(eval_num(&lua, "(ToGridFromHex(hex.x, hex.y))"), 5.0);
    assert_eq!(
        eval_num(&lua, "select(2, ToGridFromHex(hex.x, hex.y))"),
        3.0
    );
    // `local x, y, z = GridToWorld(plot:GetX(), plot:GetY())` and `HexToWorld(hexPos)` with `.x .y .z`.
    assert_eq!(eval_num(&lua, "select('#', GridToWorld(5, 3))"), 3.0);
    assert_eq!(eval_num(&lua, "(GridToWorld(2, 0))"), 128.0);
    run(&lua, "w = HexToWorld(hex); gx, gy, gz = GridToWorld(5, 3)");
    assert_eq!(eval_num(&lua, "w.x"), eval_num(&lua, "gx"));
    assert_eq!(eval_num(&lua, "w.y"), eval_num(&lua, "gy"));
    assert_eq!(eval_num(&lua, "w.z"), 0.0);
}

#[test]
fn pregame_client_part_sits_on_the_table_of_the_replica() {
    let store = MemoryStore::new();
    let api = api(&store);
    let lua = lua_with(&api);
    assert_eq!(eval_str(&lua, "PreGame.GetNickName()"), "Alice");
    assert!(eval_bool(&lua, "PreGame.IsMultiplayerGame()"));
    assert!(eval_bool(&lua, "PreGame.GameStarted()"));
    assert!(!eval_bool(&lua, "PreGame.IsHotSeatGame()"));
    run(&lua, "PreGame.SetLeaderKey(2, 'F9')");
    assert_eq!(eval_str(&lua, "PreGame.GetLeaderKey(2)"), "F9");
    // The replica replaces the table; reinstalling puts the client part on the new one.
    run(&lua, "PreGame = { GetEra = function() return 3 end }");
    assert!(eval_bool(&lua, "PreGame.IsMultiplayerGame == nil"));
    api.reinstall_pregame(&lua).expect("reinstall");
    assert!(eval_bool(&lua, "PreGame.IsMultiplayerGame()"));
    assert_eq!(eval_num(&lua, "PreGame.GetEra()"), 3.0);
}

#[test]
fn matchmaking_and_steam_follow_the_seat() {
    let store = MemoryStore::new();
    let api = api(&store);
    let lua = lua_with(&api);
    assert!(!eval_bool(&lua, "Matchmaking.IsHost()"));
    assert_eq!(eval_num(&lua, "Matchmaking.GetLocalID()"), 1.0);
    assert_eq!(eval_num(&lua, "Matchmaking.GetHostID()"), 0.0);
    assert_eq!(eval_num(&lua, "Steam.GetMaxCloudSaves()"), 0.0);
    assert!(eval_bool(&lua, "Steam.ActivateInviteOverlay() == nil"));
}

#[test]
fn locale_without_a_localizer_answers_the_key_and_with_one_composes() {
    let store = MemoryStore::new();
    let lua = lua_with(&api(&store));
    assert_eq!(
        eval_str(&lua, "Locale.ConvertTextKey('TXT_KEY_X', 3)"),
        "TXT_KEY_X"
    );
    assert!(eval_bool(&lua, "Locale.IsNilOrWhitespace('  ')"));
    assert_eq!(eval_num(&lua, "Locale.Length('abc')"), 3.0);

    let mut table = loc::MapTable::new();
    table.insert_text(
        "TXT_KEY_UNITS",
        "{1_Num} {1_Num: plural 1?unit; other?units;}",
    );
    let l = loc::Localizer::new(loc::LanguageInfo::en_us(), Box::new(table));
    let cfg = ExeConfig {
        localizer: Some(std::rc::Rc::new(l)),
        ..ExeConfig::default()
    };
    let api = ExeApi::new(cfg, TestHost::new(&store));
    let lua = lua_with(&api);
    assert_eq!(
        eval_str(&lua, "Locale.ConvertTextKey('TXT_KEY_UNITS', 3)"),
        "3 units"
    );
    assert_eq!(
        eval_str(&lua, "Locale.Lookup('TXT_KEY_UNITS', 1)"),
        "1 unit"
    );
    assert!(eval_bool(&lua, "Locale.HasTextKey('TXT_KEY_UNITS')"));
    assert!(!eval_bool(&lua, "Locale.HasTextKey('TXT_KEY_NOPE')"));
}

#[test]
fn registry_names_what_is_installed_and_marks_the_stubs() {
    let store = MemoryStore::new();
    let api = api(&store);
    let _lua = lua_with(&api);
    let reg = api.registered();
    assert_eq!(reg.get("OptionsManager.GetGridOn"), Some(Status::Real));
    assert_eq!(reg.get("Keys.VK_ESCAPE"), Some(Status::Real));
    assert_eq!(reg.get("InStrategicView"), Some(Status::Real));
    assert_eq!(
        reg.get("OptionsManager.CommitGraphicsOptions"),
        Some(Status::Simp("SIMP-exe-13"))
    );
    assert_eq!(
        reg.get("Steam.SaveGameToCloud"),
        Some(Status::Simp("SIMP-exe-12"))
    );
    assert_eq!(reg.get("OptionsManager.NoSuchThing"), None);
    // Every SIMP id a stub names has a row.
    for (name, st) in reg.iter() {
        if let Status::Simp(id) = st {
            assert!(exe_api::simp::row(id).is_some(), "{name}: {id}");
        }
    }
}

#[test]
fn the_civ_list_is_on_the_left_unless_the_player_chose_the_right() {
    let store = MemoryStore::new();
    // Saved by a build with the first defaults: marked, no side chosen.
    store.set(
        "civ5.userdata.Enhanced User Interface Options.1",
        r#"{"ReimDefaults":1,"CivRibbon":1,"UnitRibbon":1}"#,
    );
    let api1 = api(&store);
    let lua = lua_with(&api1);
    run(
        &lua,
        r#"eui = Modding.OpenUserData("Enhanced User Interface Options", 1)"#,
    );
    assert_eq!(eval_num(&lua, r#"eui.GetValue("DB_bAnchorLeft")"#), 1.0);
    // The version-1 defaults are not applied again over the player's unit ribbon.
    assert_eq!(eval_num(&lua, r#"eui.GetValue("UnitRibbon")"#), 1.0);
    // The player picks the right side in the options menu.
    run(&lua, r#"eui.SetValue("DB_bAnchorLeft", false)"#);
    api1.flush();
    drop(lua);
    drop(api1);
    let api2 = api(&store);
    let lua = lua_with(&api2);
    run(
        &lua,
        r#"eui = Modding.OpenUserData("Enhanced User Interface Options", 1)"#,
    );
    assert_eq!(eval_num(&lua, r#"eui.GetValue("DB_bAnchorLeft")"#), 0.0);
}
