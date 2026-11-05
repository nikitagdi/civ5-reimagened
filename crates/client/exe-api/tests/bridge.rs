//! The viewer's bridges (`bridge.rs`): read from the replica's Lua as the original's `GameplayBridgeChanged`
//! tells them: the active team knows the technology with `BridgeBuilding`, and the style from the era
//! `LAST_BRIDGE_ART_ERA`.

mod support;

use exe_api::bridge::probe_bridge;
use support::{lua_with, run};

use platform::MemoryStore;

fn world(lua: &lua_sys::Lua, bridge: bool, era: i32) {
    run(
        lua,
        &format!(
            "GameDefines = {{ LAST_BRIDGE_ART_ERA = 5 }}
             Game = {{ GetActiveTeam = function() return 1 end }}
             Teams = {{ [1] = {{ IsBridgeBuilding = function() return {bridge} end,
                                 GetCurrentEra = function() return {era} end }} }}"
        ),
    );
}

#[test]
fn the_state_is_the_active_teams_technology_and_era() {
    let store = MemoryStore::default();
    let api = support::api(&store);
    let lua = lua_with(&api);
    // Before the technology: no bridge, stone style.
    world(&lua, false, 1);
    assert_eq!(probe_bridge(&lua), Some((false, false)));
    // Engineering learnt: a stone bridge.
    world(&lua, true, 1);
    assert_eq!(probe_bridge(&lua), Some((true, false)));
    // The Modern era: iron (and the Industrial one is still stone).
    world(&lua, true, 4);
    assert_eq!(probe_bridge(&lua), Some((true, false)));
    world(&lua, true, 5);
    assert_eq!(probe_bridge(&lua), Some((true, true)));
}

#[test]
fn no_active_team_is_unknown() {
    let store = MemoryStore::default();
    let api = support::api(&store);
    let lua = lua_with(&api);
    run(
        &lua,
        "Game = { GetActiveTeam = function() return -1 end } Teams = {}",
    );
    assert_eq!(probe_bridge(&lua), None);
}
