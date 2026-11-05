//! The end-game replay tab (ReplayViewer, K19) over the real game files: opening it must not raise a Lua error (live crash,
//! game 943, turn 143: the replica core aborted on an empty replay data set). Skipped when the game files are absent.
mod support;

const STUBS: &str = include_str!("fixtures/m1_stubs.lua");
const REPLAY: &str = include_str!("fixtures/replay_stubs.lua");

#[test]
fn the_replay_tab_opens() {
    let Some(assets) = support::game_assets() else {
        return;
    };
    let stubs = format!("{STUBS}\n{REPLAY}");
    let r = ui::dev::show_replay_tab(&assets, &stubs);
    eprintln!("{} controls, log {:?}", r.controls, r.log);
    assert_eq!(r.lua_errors, 0, "{:?}", r.errors);
    assert!(r.controls > 20, "controls {}", r.controls);
}

mod map {
    use super::support::ctx::*;

    /// `ReplayMap` (the map tab of the replay viewer) draws one rectangle for each plot the Lua coloured.
    #[test]
    fn the_replay_map_draws_the_plots_the_lua_set() {
        let mut f = fixture(&[
            (
                "UI/Root.xml",
                r#"<Context><ReplayMap ID="M" Anchor="L,T" Offset="0,0" Size="40,20"/></Context>"#,
            ),
            (
                "UI/Root.lua",
                "Controls.M:SetMapSize(4, 2, 0, -1)
                 Controls.M:SetPlot(0, 0, 1, 1, 0, 0, 1)
                 Controls.M:SetPlot(3, 1, 1, 0, 1, 0, 1)
                 Controls.M:SetPlot(9, 9, 1, 0, 0, 1, 1)
                 Controls.M:SetPlot(-1, 0, 1, 0, 0, 1, 1)",
            ),
        ]);
        f.ui.load_root("Assets/UI/Root");
        assert!(f.errors().is_empty(), "{:?}", f.errors());
        f.ui.update(0.016);
        let mut list = draw_list::UiDrawList::new();
        f.ui.draw(&mut list);
        // Two plots in the grid (a plot outside it is ignored), four vertices each.
        assert_eq!(list.vertices().len(), 8);
    }
}
