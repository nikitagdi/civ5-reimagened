# Server-built UI screens

A screen is an optional model root fetched for the player's seat. Its schema filename, Lua filename and registry name share one lowercase name. Visible screens also use that name for their window and skin `data-screen`. The reserved `hud` service screen supplies HUD facts continuously and has no window. `techs` is the working example. The protocol version is 17 and the model version is 1.2.

## Adding a screen

1. Add `crates/shared/ui2-model/schema/screens/<name>.toml`. Declare `[model.<name>]` with `_optional = true`. Prefix supporting type and enum names with the PascalCase screen name. Follow `techs.toml` for keyed lists and enum declarations. Screen fragments contain enum, type and model sections. Keep the shared schema's version declaration in the base file.
2. Add `crates/shared/game-host/screens/<name>.lua`. Add one `(name, include_str!(...))` entry to `screens/mod.rs`. The interpreter loads only these built-in sources.
3. Run `cargo xtask ui gen-model`. Schema discovery runs at build time and during generation. It checks for the matching Lua file, merges the schema, adds `WindowId`, `ui.windows.<name>` and the generated window accessors. Generated output can be regenerated after parallel merges.
4. Add `skins/engraved/sample/<name>.json` containing `model.<name>` and an open `ui.windows.<name>`. The registry sample test checks every screen automatically. Use representative empty lists, nullable fields and statuses.
5. Add `skins/engraved/parts/<name>.html` and its stylesheet. Include them from the skin entry document and add the sample to the skin's verify states. Set `data-screen` on the modal container and bind visibility to `ui.windows.<name>.open`. Bind the content to the optional root after checking it against null.
6. Open it with `intent window.open(window: '<name>', a: ..., b: ...)`. Close with `window.close`. The runtime's existing modal handling closes the top visible registered window on Escape. Menu destinations whose `ScreenId` matches a registered name also use its window. The technology HUD button opens `techs` explicitly.

Schema discovery and generated accessors cover new windows without hand-written Rust. Parallel screen authors edit separate files and one server registry entry. Skin include lists also need an entry for the new part and stylesheet.

## Script contract

```lua
return {
    data = function(player, args)
        return { items = array() }
    end,
    actions = {
        choose = function(player, args, net)
            -- Validate ids and ownership using this explicit player.
            net.send("sendResearch", args.a, 0, -1, args.b or 0)
        end,
    },
}
```

`data` returns the root object itself. The JSON sample wraps that object under `model.<name>`. Return `array()` for an empty list. Nonempty arrays must be dense and their schema key fields must be unique. List elements are objects. Omitted struct fields use schema defaults. Lua nil omits a field. Invalid numbers, cycles, excessive nesting and unsupported Lua values fail the request.

The player argument always identifies the requesting seat. Read `Players[player]` and its team. Every query must filter foreign cities, units and plots according to that seat's visibility and knowledge. Use ownership checks before emitting orders. Avoid active-player and active-team getters and any API whose implementation depends on those globals. Examine the core implementation when adding an unfamiliar getter.

Scripts perform reads through `Game`, `Players`, `Teams`, `Map`, `GameInfo`, `DB.Query`, `GameDefines` and `PreGame`. Every mutation goes through `net.send`. The interpreter validates message names against the existing UI allowlist and collects a bounded batch. Only after the action returns successfully does the worker submit the batch with the seat's player id. Lua failures discard the batch. Core refusals become ordinary rejected command outcomes. Built-in script authors must keep setters and direct game mutations out of their code.

The worker creates its Lua state on the first request. A new or resumed worker starts with a fresh state. The interpreter is destroyed before its game core. Each outer Lua call has a 20-million-instruction budget. JSON replies have a 512 KiB cap and a structural depth limit. Queries during game startup return an error. Lua errors are logged and returned without stopping the game.

## Shared helpers

- `L(key, ...)` uses the server's `Locale.ConvertTextKey`, installed from the exported English dictionary through `loc`. `Locale.Lookup` is an alias.
- `icon(atlas, index)` returns the existing `icon:<atlas>:<index>` asset reference, or an empty reference for missing metadata.
- `key_city(owner, id)` and `key_unit(owner, id)` return the model's owner/id key strings.
- `key_item(order, item)` produces the order/item key string.
- `team_of(player)` returns the team's Lua object. `met(player, other)` checks contact with the other player's team.
- `visible(plot, team)` checks visibility using a numeric team id.
- `array(values?)` tags an empty or populated Lua table as a JSON array.

## Requests, actions and refresh

`Query::Screen { screen, args }` returns `QueryAnswer::Screen { screen, json }`. Arguments on the wire are named i64 values. Lua receives exact numeric values through the double-precision integer range. Duplicate names and excessive argument counts are rejected.

`intent screen.act(screen: '<name>', action: '<action>', a: ..., b: ..., c: ..., d: ..., text: ...)` sends `Command::ScreenAction` through the normal numbered order path. The optional `text` is an owned UTF-8 string on the wire and arrives as `args.text` in Lua, or nil when omitted. The server rejects values longer than 256 bytes or containing Unicode control characters as action errors. Numeric arguments cannot use the reserved name `text`. Waiting orders appear in `ui.pending`. The `techs.research` action accepts `a` as the technology id and `b` as 0 for replacement or 1 for append.

`ui2_source::remote::RemoteRoots` owns subscriptions. An open window is queried immediately, then on WorldView revision changes at most every 250 ms with one request outstanding per subscription. Final outcomes of its own screen actions invalidate the subscription. Every final command outcome also invalidates `hud`, covering queue changes that leave the synchronized world unchanged. Closing clears the root and discards late responses. Reconnection resets pending subscriptions. Responses pass the same schema conversion as `Sample`. Failures clear the root and are reported through `UiActions::screen_error` into `ui.errors`.

The host supplies time, world revision and `HostNet::query`. Query answers reach it as `ActionEvent::QueryResult`. The host keeps the `hud` subscription active while v2 runs and feeds its validated root to `ui2_source::hud::HudScreen`, the default `HudExtras` implementation.

## Tests

Run fast screen-channel checks from the repository root.

```sh
cargo test -p game-host screens::runtime::tests
cargo test -p ui2-source remote::tests
cargo test -p ui2-actions --test screens
cargo test -p ui2-model --test skin_samples every_registered_screen
cargo xtask ui check engraved
```

On Linux, with exported game data and real gamecore archives available, run the bridge proof and the automatic registry test. `--gamecore-lib` names the directory containing `linux/libgamecore.a`. `CIV5_GAMEDATA` names the exported game-data directory. Confirm the output identifies the real linked implementation. A stub-linked run prints a skip diagnostic when `--nocapture` is used.

```sh
cargo xtask test -p game-host --gamecore-lib "$CIV5_GAMECORE_LIB" --test real_screen_bridge -- --nocapture
CIV5_SCREEN=techs cargo xtask test -p server --gamecore-lib "$CIV5_GAMECORE_LIB" --test worker_real registered_screens_validate_and_research_changes -- --nocapture
```

Without `CIV5_SCREEN`, the second test queries every registry entry and validates its JSON against the schema. On a fresh game it also selects a technology and verifies that the next server response reports it as current. A separate test checks that the server registry and schema screen directory agree, including on stub builds.

For a quick late-game probe, point `CIV5_SCREEN_SAVE` at a private C5SV save and set `CIV5_SCREEN_PLAYER` to a human seat id. Run that same registry test. It loads the save through the worker and validates screen data. The late-game path leaves research orders untouched and prints no response contents. Keep saves and any identifying data outside the repository.

Start a real-core server with `tools/playtest/m2/server.ps1`, supplying the exported assets, game data, archives and a private state directory. Build the web bundle and skin from the same checkout. Run the acceptance scenario against its URL.

```sh
node tools/playtest/run.mjs --scenario ui2-screens --attach http://127.0.0.1:8860 --out target/playtest-screens
cargo xtask check
```

The scenario opens the HUD technology button, checks the full catalog, selects an available technology, observes both the refreshed screen status and HUD research, and closes the window with Escape.

## Current simplifications

- SIMP-300 keeps window and intent arguments in the model's existing signed 32-bit scalar representation and widens them to protocol i64 values.
- SIMP-301 uses en_US screen text from the exported dictionary.
- SIMP-302 refreshes complete roots.
- SIMP-303 rejects Lua arguments outside the exact integer range of its double-precision numbers.

## HUD service contract

`hud` is reserved during schema assembly. `SCREEN_NAMES` includes every registry screen. `WINDOW_SCREEN_NAMES` contains the screens with generated `WinArgs` and `WindowId` entries. `Windows::remote("hud")` returns `None`.

`Query::Screen { screen: "hud", args }` accepts optional `city` and `unit` ids scoped to the requesting seat. The client chooses the open production city, falling back to the selected city, and the selected unit. Changing focus clears only the corresponding `hud.city` or `hud.unit` facts and rejects late answers with old arguments. Shared research, resources, turn and popup facts remain available during the refresh. Disconnects and new games clear the root. Refreshes use the open-window throttle and one outstanding request.

The root has `research.techs` (status, zero-based queue position, turns, progress and cost), `top.resources` (strategic and luxury counts), `top.policies` (branch metadata and adopted counts), and `top.tourism`. Optional `city` contains its id, full slot-keyed queue, overflow and per-option lock explanations. Optional `unit` contains its id, promotions and applicable action rows. Omitted focus arguments omit the corresponding object. City and unit lookups enforce ownership.

`HudScreen::update(Option<&Hud>)` consumes a schema-validated root and changes its `HudExtras::revision` only when its value changes. An unavailable root or a mismatched focused id returns unsupported reads. Root changes invalidate the source's unknown-field and catalog caches. `V2Ui::set_hud_extras` remains available for embedding with a custom provider.

`hud.alert`, `hud.pillage` and `hud.upgrade` take `a` as the seat's unit id. They recheck native availability before emitting `sendPushMission` or `sendDoCommand`. The existing `unit.action` intents route these three kinds through screen actions. Their buttons use the server's applicability and enabled state.

- Locked units and buildings carry the core's own reason (`CanTrainTooltip` / `CanConstructTooltip`, the tooltip sink of `CvCity::canTrain` / `canConstruct`). SIMP-324 remains for projects and processes, for which the core writes no reason, and uses a general explanation there. The queue, overflow and other HUD fields are populated. SIMP-310 and SIMP-250 have been removed from the resolved paths.

Run the service checks and browser scenario with the same real-core setup described above.

```sh
cargo test -p ui2-source hud::tests
cargo test -p ui2-source remote::tests
cargo test -p ui2-host --test world
cargo xtask test -p game-host --gamecore-lib "$CIV5_GAMECORE_LIB" --test real_hud_screen -- --nocapture
CIV5_SCREEN=hud cargo xtask test -p server --gamecore-lib "$CIV5_GAMECORE_LIB" --test worker_real registered_screens_validate_and_research_changes -- --nocapture
node tools/playtest/run.mjs --scenario ui2-hud-extras --attach http://127.0.0.1:8860 --out target/playtest-hud-extras
```

The browser scenario creates two city queue rows, appends research and checks the research queue, then requires `client.core.loads` to remain zero throughout the cold v2 session.


## Game-opened windows

Declare native popup routing in `game-host/screens/_popups.lua`. Keys are exported `ButtonPopupTypes` names. Each entry declares `screen`, optional `a` and `b`, and `decision`. Argument values may be integer constants or functions of the popup payload. For example the technology award uses its third data value.

```lua
BUTTONPOPUP_TECH_AWARD = {
    screen = "tech_award", a = function(p) return p.data3 end, decision = false,
}
```

The payload supplies `id`, `kind`, `data1`, `data2`, `data3`, `flag`, `option1`, `option2`, optional `data4` and `data5` (the fourth and fifth integers of an array popup: for `BUTTONPOPUP_CITY_CAPTURED` the Great Works captured and the player that may be liberated, -1 for none; for `BUTTONPOPUP_DECLAREWARMOVE` the acting unit, -1 when unknown, and its move flags) and optional `text`. The worker assigns increasing positive signed 32-bit ids for the lifetime of the game. `hud.popup` is the queue head `{id, screen, a, b, decision}` or null. `hud.popup_count` counts pending entries for that seat. HUD polling every 250 ms delivers events even when the world revision stays constant. The queue belongs to the worker and survives a client reconnect. A restarted worker begins a fresh queue.

The client opens the registered `ui.windows.<screen>` with `a`, `b`, `popup_id` and `popup_decision`. Screen queries and actions carry `popup_id`. The server rejects stale popup actions. Scripts receive the complete current payload in `args.popup` and its id in `args.popup_id`, scoped to the seat and matching screen. Avoid printing payload text.

An informational window's usual close submits `screen.act(screen: 'hud', action: 'popup_done', a: id)`. A failed dismissal reopens the window so it can be retried. Decision windows ignore ordinary close and toggle intents. Finish a decision from its validated screen action.

```lua
choose = function(player, args, net)
    -- Validate the chosen value and submit the native order.
    net.send("sendResearch", args.a, 0, -1, 0)
    if args.popup_id then net.popup_done(args.popup_id) end
    return {close = true}
end
```

`net.popup_done(id)` stages completion. The worker validates the seat, head id and owning screen before submission, then removes the entry only after successful native batch submission. Lua errors, invalid transitions and native refusals retain the entry. Completion also closes its window. Existing end-turn blockers remain authoritative.

A decision that can no longer be answered (its city or unit is gone, the choice was made another way, such as production set from the city banner) would block the seat's queue for good: only its own screen action completes it and the client keeps its window open. A mapping in `_popups.lua` may therefore name `lapsed = function(player, popup)`, which returns a reason when the decision lapsed. Whenever the HUD is read and before every screen action, the worker asks it of the decisions at the head of the seat's queue (read scope) and drops the lapsed ones; a test that fails keeps the decision. Tests exist for captured cities, production, city renames, research and free techs, ideology, barbarian ransom and civilian return. The same decision asked again (same kind and payload) replaces the older one; a move's war question keeps its own rule.

The queue evicts the oldest informational entry on overflow and logs it. Unknown kinds are dropped with one warning per kind. SIMPLIFIED(SIMP-411): when every pending entry is a distinct live decision, another one exceeds the 64-entry soft bound to preserve it. Informational arrivals are rejected in that state. The core sends `GC_EVENT_POPUP_INFO` with eight values ordered as type, data1, data2, data3, player, flag, option1, option2, and with ten for a captured city and for a move that needs war (data4, data5 appended). A seat's move order into a rival's borders, city or unit at peace opens that question instead of moving, as the original game's move click does. The `declare_war` window then shows the move reason and its plot. Its confirm action declares war and replays the move for the named unit through `sendPushMission`. Cancel only finishes the popup. The decoder still reads the five-value form of older cores (zero flags, false options). Embeddings and tests can call `Worker::inject_popup(player, Popup)` with the full named payload without a native event.

## Action window transitions

A successful action may return `open`, `close`, or both. The worker validates the destination against its registered window screens and carries the transition in `CommandOutcome::ScreenApplied {screen, open, close, popup_done}`. The client applies it once for the matching outstanding command, closing the originating screen before opening the destination. Rejections leave the windows intact. Arguments use the model's signed 32-bit integers.

## Session operations

Saving, leaving for the lobby, pausing and resuming the turn timer and kicking a seat belong to the session (the gateway's game actor), not to the game core, so no screen script performs them. A skin asks with the intent `session.op(op: 'save' | 'leave' | 'pause' | 'resume' | 'kick', player?)`. The actions module checks what the `session` root shows (host-only operations need `session.is_host`; a kick needs another human seat) and refuses with a toast (`not_host`, `no_such_player`, `not_human`, `self_kick`, `no_game`); otherwise the host sends the classic client's `ClientMsg::GameCtl` (`Save`, `Leave`, `Pause(bool)`, `Kick`). The gateway checks the authority again; its `GameCtlRejected` reaches the actions module as `ActionEvent::SessionRefused` and becomes a toast with the same reasons. Success shows as its effect: the save in `session.saves`, the pause in `session.paused`, the seat list, the lobby. No protocol message is new.

A screen action can carry a session operation to run after the server applied it: `screen.act(..., then: 'leave')`. The operation is checked when the intent is applied, kept with the order id, run in the next frame after `Applied` or `ScreenApplied`, and dropped with a rejected, lost or rolled-back action. The confirmed exit question (`BUTTONPOPUP_CONFIRM_MENU`) uses it: yes completes the popup on the server, then the client leaves.

The `session` root is built by the client from the session alone, every frame, and written when it changes: `is_host`, `paused`, `ping_ms` (our own round trip, to 10 ms; other seats' pings are not known to a client, as in the classic UI), `seats` (player key, nick of a human, control, connection, host mark, ours) and `saves` (the lobby's saves of this game, newest first, at most 32; the gateway sends the players of a game the lobby view again whenever one of its saves is written). Windows that show players join it by `player_key` (`civ_ribbon` majors, `mp_vote` roster rows), so connection, host mark and nicknames have one source.

After a victory a server core plays on by itself (the engine reports autorun, so the core sets `GAMESTATE_EXTENDED`); `end_game`'s `continue` only closes the screen. When every seated human is dead the core sets `GAMESTATE_OVER`, the worker takes no more orders and `continue` is refused with a reason; `exit` is `session.op(op: 'leave')`.

```lua
return {close = true, open = {screen = "techs", a = 1, b = 0}}
```

Run the popup fixtures and native proof with the usual build environment.

```sh
cargo test -p game-host --lib popup
cargo test -p game-host screens::runtime::tests
cargo test -p ui2-actions --test popups
cargo test -p ui2-host --test world
cargo xtask test -p game-host --gamecore-lib "$CIV5_GAMECORE_LIB" --test real_popups -- --nocapture
node --check tools/playtest/scenarios/ui2-popups.mjs
```

For the browser acceptance fixture, start a fresh real-core server with `CIV5_TEST_POPUPS=0` in its environment. The value selects the numeric seat. The worker seeds an informational technology window, a research decision, then another informational window. This opt-in hook uses `Worker::inject_popup` and is absent unless the environment variable is set. The scenario closes the first window, checks decision-close protection, reconnects, chooses research, then dismisses the final entry. The probe exposes `ui.popup` and `ui.popup_count` without payload text.

```sh
node tools/playtest/run.mjs --scenario ui2-popups --attach http://127.0.0.1:8860 --out target/playtest-popups
```


## Integrated screen groups

`city`, `city_list`, `city_rename`, `city_captured`, `free_great_person`, `techs`, `tech_award`, `policies`, `ideology`, `pantheon`, `religion` and `religion_overview` are registered windows. The research panel consumes the HUD roots. The top strip opens the city list, policies and religion overview; an owned map banner opens the full city screen, which retains a production chooser entry.

`city_rename.rename` takes `a` as the seat-local city id and `text` as the proposed name. It validates the trimmed name before `net.send_text("SendRenameCity", name, city)`. `religion.found` preserves `text` with `net.send_text("SendFoundReligion", name, ...ints)`. Successful decisions finish `args.popup_id` transactionally. Capture decisions require `args.popup`; pending capture cities remain read-only across both screen actions and production commands.

`rich_tooltips` is a service root in `SCREEN_NAMES`, excluded from `WINDOW_SCREEN_NAMES` and `WindowId`. Query it with `Query::Screen`, without opening a window. The skin names the thing under the pointer in `ui.tip` (`kind`, `id`, optional `city`; markup sets it on `pointerenter` and clears `id` on `pointerleave`); while `ui.tip.id` is set `RemoteRoots` asks `Query::Screen { screen: "rich_tooltips", args: [kind, id, city] }`, clears the root when the thing changes and drops late answers, as for the plot (`ui2-source/src/remote.rs`). The technology cards of the tree set it, and their tooltip shows the core's progress line (`techs.tip.core`); `parts/rich_tooltips.html` (`rich_tooltips-tip`) keeps the technology section of the generic template, which no element shows yet: the sections of the other kinds come with the windows that hover them.

Decision confirmation buttons bind `data-bind-hotkey="'Enter'"`. A visible, enabled explicit Enter hotkey in the active screen takes priority over the previously focused control. Repeats are consumed without resubmission. Group roots use `data-esc="call policies.escape()"` to cancel nested drafts before attempting the queue-protected window close.


## Integrated HUD and diplomacy

`civ_ribbon` is a service root, excluded from `WindowId` and `WINDOW_SCREEN_NAMES`.
The host polls `Query::Screen { screen: "civ_ribbon", args: [] }` every 250 ms,
with one outstanding request and the same disconnect/reset handling as `hud`.
`state.civ_ribbon.visible` folds the list while preserving the diplomacy corner.
`Digit1` toggles it. `state.civ_ribbon.left` selects its side.

The top menu's `hud.menu` slot in `parts/top.html` contains one button per available
destination. Add a destination there as one button when its group is integrated.
`city_list` stays a registered window but occupies the flex info-corner slot.
It replaces the research/list content and city ribbon while keeping the unit panel.
The column uses the window layer so `data-screen="city_list"` retains normal input
and Escape handling. `css/hud_layout.css` follows the responsive stylesheet and
owns the shared HUD bounds. `hud.top-center` is the existing toast/streamer slot.

Common native catalog actions preserve `unit.act.found-city`, `unit.act.fortify`,
`unit.act.sleep` and `unit.act.skip`. Their filtered buttons share server validation
and bound native hotkeys with `unit-panel.action`.
`diplo.declare-war` opens `declare_war` with `a` as the rival team and `b=-1`.
`deal-ai.*` names AI negotiation parts. Human negotiation retains `deal.*`.
Diplomacy tooltips are included through `parts/diplo_tips.html` in the tooltip layer.

`ui2-hud-integration` checks HUD bounds at both supported acceptance resolutions,
the city/research section switch, and HUD suppression under the technology window.

## Map link

The app owns the movement preview (`client-app/src/play/preview.rs`). While a unit is selected in `ui.selection` it draws the selection ring, the reach outline from the focused unit's reach tree, and the path to the plot under the pointer. Plots outside this turn's reach get their path from `Query::Path` after a short dwell. A pointer over a panel shows no path. The turn badges reach the skin as `ui.path` (`PathLabel {cell, plot, turn}`, `cell = x*65536+y`). The skin anchors them with `data-anchor="plot:{{p.cell}}"` on the `data-bind-for` node. A right click on the map moves the selected unit.

`plot_help` is a service root, like `rich_tooltips`. After the pointer has rested on a plot for 0.5 s the host writes it to `ui.hover`. `RemoteRoots` then asks `Query::Screen { screen: "plot_help", args: [x, y, unit] }`. Changing the arguments clears the root and drops late answers, so the tooltip goes as soon as the pointer moves on. The script follows the seat's fog. An unrevealed plot answers only its coordinates. A revealed one adds its revealed improvement, route and owner. Units, yields, pillage and city state appear only while the plot is visible. With a unit in focus and an enemy on the plot, `combat` carries the prediction of the original enemy unit panel. SIMPLIFIED(SIMP-446): yields are shown only on visible plots. SIMPLIFIED(SIMP-447): there is no city attack preview. The rich tooltips follow `ui.tip` the same way.

The map options of the minimap panel are skin state: `state.minimap.grid`, `routes`, `yields` and `resources`. The host follows them each frame (`client-app/src/ui_host/v2_map.rs`). Grid and trade routes are renderer switches. Yields and resources fill `ui.map_icons` for the revealed plots in view, rebuilt when the view, the world revision or a switch changes. The switches are stored per browser under the key `ui2.map_options`.

Notifications carry `go`, `data1`, `data2` and `dismissable` (model 1.3). The source maps the core's type hash to the click action of `CvNotifications::Activate` (`ui2-source/src/build/notes.rs`). `note.activate` opens the window with the arguments that the matching popup would give it (`_popups.lua`), selects the promoted unit, looks at the capital, or by default looks at the plot. `note.dismiss` does nothing for the types that `MayUserDismiss` keeps. The skin's cards dismiss on a right click (`data-on-contextmenu`) and show the dismiss button only for dismissable notes. SIMPLIFIED(SIMP-448): the activations that start an AI conversation do nothing, and a completed wonder looks at its city instead of showing the splash again. SIMPLIFIED(SIMP-449): a policy note is never dismissable here.

```sh
cargo test -p client-app --lib preview
cargo test -p ui2-actions --test chat
cargo test -p ui2-source remote::tests
cargo test -p game-host --test plot_help_script
node tools/playtest/run.mjs --scenario ui2-maplink --attach http://127.0.0.1:8860 --out target/pt-maplink
```
