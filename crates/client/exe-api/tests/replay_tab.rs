//! The end-game replay tab (`ReplayViewer`, K19) on the exe API: opening it must not raise a Lua error nor trap (live crash,
//! game 943, turn 143). Skipped when the game files are absent (set `CIV5_GAME_DIR`).
mod fake;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use asset_format::UiFileKind;
use assets::UiFiles;
use exe_api::{ExeApi, ExeConfig, GameSide};
use fake::FakeGame;
use ui::dev::{DirFiles, UI_LAYERS, build_index};
use ui::{StubHost, StubText, UiConfig, UiCore};

const GAMECORE_STUBS: &str = include_str!("fixtures/gamecore_stubs.lua");
const REPLAY_STUBS: &str = include_str!("fixtures/replay_stubs.lua");

const ROOT_XML: &str = r#"<Context ColorSet="Beige_Black" Font="TwCenMT20" FontStyle="Shadow">
<LuaContext FileName="Assets/UI/InGame/Popups/ReplayViewer" ID="EndGameReplay" DeferLoad="1" Hidden="True" />
</Context>"#;
const ROOT_LUA: &str = "Events.ShowReplayTab.Add(function() Controls.EndGameReplay:DoDeferredLoad(); Controls.EndGameReplay:SetHide(false) end)";

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
fn the_replay_tab_opens_on_the_exe_api() {
    let Some(assets) = game_assets() else {
        return;
    };
    let mut src = DirFiles::default();
    let mut extra = Vec::new();
    for (name, bytes, kind) in [
        ("fixture/ReplayRoot.xml", ROOT_XML, UiFileKind::Xml),
        ("fixture/ReplayRoot.lua", ROOT_LUA, UiFileKind::Lua),
    ] {
        src.memory
            .insert(name.to_owned(), bytes.as_bytes().to_vec());
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
            for (name, code) in [
                ("=gamecore_stubs", GAMECORE_STUBS),
                ("=replay_stubs", REPLAY_STUBS),
            ] {
                lua.load(code)
                    .set_name(name)
                    .set_environment(g.clone())
                    .exec()?;
            }
            Ok(())
        },
    )
    .expect("ui core");
    exe.attach_runtime(ui.runtime());
    ui.load_root("ReplayRoot");

    let game = Rc::new(RefCell::new(FakeGame::default()));
    let dynamic: Rc<RefCell<dyn GameSide>> = game.clone();
    let driver = ui
        .runtime()
        .create_context("replay_driver", |_, _| Ok(()))
        .expect("driver context");
    exe.bind_game(dynamic, driver.lua());
    exe.dispatch(ui.runtime());
    driver
        .exec("Events.ShowReplayTab()", "=replay_driver")
        .expect("show the tab");
    exe.dispatch(ui.runtime());
    ui.update(0.016);
    ui.update(0.016);
    let mut list = draw_list::UiDrawList::new();
    ui.draw(&mut list);

    let errors: Vec<String> = ui
        .runtime()
        .recent_errors()
        .iter()
        .map(|e| format!("[{}] {}", e.context, e.error.message))
        .collect();
    eprintln!("log {:?}", ui.runtime().take_log());
    assert_eq!(ui.stats().lua_errors, 0, "{errors:#?}");
    assert!(
        ui.stats().controls > 100,
        "controls {}",
        ui.stats().controls
    );
}
