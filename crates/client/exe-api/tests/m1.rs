//! The M1 run of EUI (tool tips, `EUI_context`, `TopPanel`, `NotificationPanel`) on the exe API instead of the
//! exe part of the fixture: the contexts register their listeners with `Events.X.Add`, `exe-api` fires
//! `SequenceGameInitComplete` after the first announcement of the game, and a notification arrives as an engine
//! callback of the (fake) replica. Skipped when the game files are absent (set `CIV5_GAME_DIR`).
mod fake;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use asset_format::UiFileKind;
use assets::UiFiles;
use exe_api::{ExeApi, ExeConfig, GameSide, UiEventKind as K};
use fake::{EvExt, FakeGame, ev};
use ui::dev::{DirFiles, M1_ROOT_XML, UI_LAYERS, build_index};
use ui::{StubHost, StubText, UiConfig, UiCore};

const GAMECORE_STUBS: &str = include_str!("fixtures/gamecore_stubs.lua");

fn game_assets() -> Option<PathBuf> {
    let candidates = [std::env::var_os("CIV5_GAME_DIR").map(PathBuf::from)];
    for dir in candidates.into_iter().flatten() {
        let assets = dir.join("Assets");
        if assets.join("UI/InstanceManager.lua").is_file() {
            return Some(assets);
        }
    }
    eprintln!("SKIPPED: game files not found (set CIV5_GAME_DIR)");
    None
}

#[test]
fn top_panel_and_notification_panel_run_on_the_exe_api() {
    let Some(assets) = game_assets() else {
        return;
    };
    let mut src = DirFiles::default();
    let mut extra = Vec::new();
    for (name, bytes, kind) in [
        (
            "fixture/M1Root.xml",
            M1_ROOT_XML.as_bytes(),
            UiFileKind::Xml,
        ),
        ("fixture/M1Root.lua", &b""[..], UiFileKind::Lua),
    ] {
        src.memory.insert(name.to_owned(), bytes.to_vec());
        extra.push((name.to_owned(), kind));
    }
    let index = build_index(&assets, &UI_LAYERS, &extra);
    let files = UiFiles::open(&index.encode()).expect("index");

    let host = Rc::new(fake::TestHost::default());
    let mut exe = ExeApi::new(ExeConfig::default(), host);
    let setup = exe.base_setup();
    let mut host_ui = StubHost::with_default_size(256, 256);
    host_ui.add_texture("Assets/UI/Art/Controls/9 Grids/SelectedUp.dds", 32, 32);
    let mut ui = UiCore::new(
        UiConfig::default(),
        files,
        Rc::new(src),
        Box::new(StubText::default()),
        Box::new(host_ui),
        move |lua, g| {
            setup(lua, g)?;
            lua.load(GAMECORE_STUBS)
                .set_name("=gamecore_stubs")
                .set_environment(g.clone())
                .exec()
        },
    )
    .expect("ui core");
    exe.attach_runtime(ui.runtime());

    ui.load_root("M1Root");
    // The panels have registered their listeners through the exe API.
    assert!(
        exe.listener_count(exe_api::Family::Events, "SequenceGameInitComplete") > 0,
        "nobody waits for SequenceGameInitComplete"
    );

    let game = Rc::new(RefCell::new(FakeGame::default()));
    let dynamic: Rc<RefCell<dyn GameSide>> = game.clone();
    let driver = ui
        .runtime()
        .create_context("m1_driver", |_, _| Ok(()))
        .expect("driver context");
    exe.bind_game(dynamic, driver.lua());

    // The first frame: the game is announced, the panels hear SequenceGameInitComplete.
    exe.dispatch(ui.runtime());
    // They filled themselves in and now listen to the rest.
    for name in ["NotificationAdded", "SerialEventGameDataDirty"] {
        assert!(
            exe.listener_count(exe_api::Family::Events, name) > 0,
            "nobody listens to {name}"
        );
    }
    driver
        .exec(
            "hits = {} Events.NotificationAdded.Add(function(id, ty, tip, sum) table.insert(hits, id .. ':' .. tip .. ':' .. sum) end)",
            "=m1_driver",
        )
        .unwrap();
    // The game core sends a notification.
    game.borrow_mut().events.push(
        ev(K::NotificationAdded, &[1, 0, 0, 0, 0, 0, 0])
            .with_text("Your Warrior has finished exploring the nearby land.")
            .with_text2("Exploration finished"),
    );
    game.borrow_mut().events.push(ev(K::SetDirty, &[11, 1]));
    exe.dispatch(ui.runtime());
    ui.update(0.016);
    exe.dispatch(ui.runtime());
    ui.update(0.016);
    let mut list = draw_list::UiDrawList::new();
    ui.draw(&mut list);

    let stats = ui.stats();
    let errors: Vec<String> = ui
        .runtime()
        .recent_errors()
        .iter()
        .map(|e| format!("[{}] {}", e.context, e.error.message))
        .collect();
    assert_eq!(stats.lua_errors, 0, "{errors:#?}");
    assert!(stats.controls > 150, "controls {}", stats.controls);
    assert!(!list.commands().is_empty());
    let hits: String = driver
        .lua()
        .load("return table.concat(hits, ';')")
        .set_environment(driver.env())
        .eval()
        .unwrap();
    assert_eq!(
        hits,
        "1:Your Warrior has finished exploring the nearby land.:Exploration finished"
    );
    let tree = ui.dump_tree();
    eprintln!(
        "{}",
        tree.lines()
            .filter(|l| l.contains("Generic") || l.contains("Notification"))
            .take(8)
            .collect::<Vec<_>>()
            .join(
                "
"
            )
    );
    let s = exe.stats();
    eprintln!(
        "exe: {} events, {} frames, simp {:?}",
        s.events, s.frames, s.simp_calls
    );
    assert!(s.events >= 4, "{s:?}");
}
