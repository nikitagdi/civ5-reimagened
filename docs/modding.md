# Mods

A mod changes or adds things to a game without touching the engine. The host of a game ticks the mods they want
in the lobby; the server and every client run the same set. The public build ships no mods and behaves exactly as
without this system. This page is for mod authors and for operators who build a server with mods.

Status: the first tier (Rust mods compiled into the server and the client) works, and so do panels in UI v2. Data,
Lua and C++ mods do not exist yet, see [What is not supported yet](#what-is-not-supported-yet).

## What a mod is

A mod is a directory with a `mod.toml` and up to three Rust crates, named `<id>-shared`, `<id>-server` and
`<id>-client`. The example `examples/mods/hello` has all three. A mod is made of parts:

| Part | Runs in | Status |
|---|---|---|
| `server-rs` | The game's actor in the server process | works |
| `client-rs` | The client (browser and desktop) | works |
| `ui` | Panels, fields and scripts in UI v2 (web client only), see [Panels in UI v2](#panels-in-ui-v2) | works |
| `assets` | Textures, sounds and raw files in a pack the client downloads by hash | works |
| `data`, `lua-gameplay`, `lua-ui`, `core-patch` | Game database, Lua, the C++ core | not yet |

Every mod has a sync class:

- **G, `gameplay`**: the mod changes the game state and takes part in the digest. Every client must have the
  exact version. (Reserved for the data, Lua and C++ parts.)
- **S, `cosmetic`**: the mod keeps its own state outside the game core, on the server, and talks to its client
  half over its own channels. It does not touch the digest. With `client = "optional"` a player without the mod
  client can still join; they just do not see the mod.
- **C, `client`**: client only, no server half and no network.

The server half is stored in the save (a `MODS` section) and replayed to a late joiner, so mod state survives a
restart and a reconnect. The server checks every message of a mod (size, rate), then the mod checks the right to
act against server facts, never against what the client claims.

## `mod.toml`

```toml
[mod]
id      = "hello"          # a-z, 0-9, '-', starts with a letter, 2 to 32 characters
name    = "Hello"
version = "0.1.0"          # MAJOR.MINOR.PATCH
api     = "^1"             # which mod API versions this mod works with
license = "MIT"            # an SPDX expression; LicenseRef-X for your own
authors = ["..."]          # optional
sync    = "cosmetic"       # gameplay | cosmetic | client
client  = "optional"       # optional | required (default); only with sync = "cosmetic"
parts   = ["server-rs", "client-rs"]

[rust]
server    = { crate = "hello-server", entry = "hello_server::register" }
client    = { crate = "hello-client", entry = "hello_client::register" }
caps      = ["net.channel", "world.read", "input.claim", "hud", "kv", "render.pipeline"]
state_max = 16             # bytes the server half may put in the save (default 1 MiB, max 64 MiB)
tick_hz   = 0              # calls of the server half's on_tick a second, 1 to 30; 0 (the default) never

[[channels]]               # one table per network channel between the two halves
id      = 0                # 0 to 255
name    = "echo"
c2s_max = 512              # largest client-to-server payload; default 4096, ceiling 56 KiB
s2c_max = 1024             # server-to-client; default 16384
rate    = "5/s"            # per seat and channel; default 30/s
burst   = 10               # default 2x rate
```

Other keys: `deps`, `optional` and `conflicts` are tables of `id = "version requirement"`; `order.after` and
`order.before` are lists of ids; `[content]` points to the `assets` directory; `[ui]` is described in
[Panels in UI v2](#panels-in-ui-v2). Unknown keys are errors.

Dependencies are resolved the same way every time: hard dependencies transitively, one version per id, then a
load order by dependencies and `order`, ties broken by id. A missing dependency, a conflict, a wrong `api` or a
cycle is an error that names the chain of mods that led to it.

Capabilities (`caps`) are declared by the mod and checked on every call. Without the capability the call fails
softly and the mod keeps running:

| Capability | Gives |
|---|---|
| `net.channel` | Sending on the mod's declared channels |
| `world.read` | Read-only facts about the map and owners (server) or the replica (client) |
| `input.claim` | Hotkeys |
| `hud` | A draw list on top of the game UI, whichever is active (classic or UI v2) |
| `ui` | The mod's own fields of the UI v2 model and the intents of its panels (client, API 1.3) |
| `render.pipeline` | A render system in the mod slot of the frame |
| `camera`, `terrain.cutout` | A camera override, a mask that cuts holes in the terrain |
| `world.bodies` | Publishing hit boxes, seeing those of the other mods and dealing hits to them (server, API 1.1) |
| `world.share_terrain` | Letting a seat draw the ground of hexes it has not revealed (server, API 1.2, cosmetic) |
| `kv` | A small per-mod key-value store in the browser |
| `audio`, `assets` | Sounds, the mod's asset pack |

The client-side capabilities (`input.claim`, `camera`, `render.pipeline`, `hud`, `kv`, `audio`, `terrain.cutout`
and `ui`) need a `client-rs` part; a manifest that declares one without it is refused.

## The API crates

| Crate | License | Use |
|---|---|---|
| `mod-manifest` | MIT | Parses `mod.toml`, the version rules, the resolver, `mods.lock` |
| `mod-api` | MIT | The server half: `ServerMod`, the `ServerCtx` facade, `Outgoing`, `ServerRegistry`, the catalogue of UI points (`UI_CATALOG`), and a mock host in `mod_api::testing` |
| `mod-client` | AGPL | The client half: `ClientMod`, the `ClientCtx` facade, `ClientRegistry`, a headless dispatcher and `MockClientCtx` |
| `mod-assets` | MIT | The pack format and its checks |

Only values cross the boundary: bytes, small structs and ids. A mod never receives an engine type, so a later
sandboxed host can offer the same API. A server half depends on `mod-api`, `mod-manifest` and the mod's own shared
crate; a client half on `mod-client` and the shared crate. `cargo xtask mods check` enforces this (rule R11).

Each half exposes one function that registers everything:

```rust
pub fn register(r: &mut ServerRegistry) {
    r.add("hello", || Box::new(HelloServer::default()));
}
```

The server half implements `ServerMod`: `start(saved)`, `on_client`, `on_attach` (the replay for a joining or
reconnecting seat), `on_turn`, `persist`, and optionally `is_dirty` and `mark_saved`. It returns the messages to
send and the host delivers them. A panic in a mod disables it for that game; the game goes on.

The mod API is at version 1.3. What 1.1 added (every new method has a default, so a 1.0 mod builds unchanged):

- **Clock and tick.** `ServerCtx::now_ms()` is the host's monotonic clock in milliseconds. With `tick_hz` in
  `[rust]` the game actor calls `ServerMod::on_tick(cx, now_ms)` that many times a second, serialised with the other
  calls; a late tick is not repeated. The answers of a tick have no sender.
- **A seat goes.** `ServerMod::on_detach(cx, seat)` when a seat's client disconnects, leaves or is kicked. It may
  come back: then `on_attach` comes again.
- **Bodies and hits** (`world.bodies`). `ServerCtx::set_bodies(Vec<Body>)` replaces the mod's hit boxes (at most 64
  bodies of 16 boxes; the host fills `Body::owner`). `ServerCtx::bodies()` returns the bodies of the other mods of the
  game. `ServerCtx::hit(BodyRef, Hit)` queues a hit; after the call the host hands each one to the owner's
  `ServerMod::on_hit(cx, from_mod, target_id, &hit)`. At most 256 hits a call; hits queued inside `on_hit`, on an
  unknown body, or on a mod that is off are dropped with a line in the log. Coordinates are world units, the frame
  of the client camera.
- **One mode at a time on the client.** `ClientCtx::acquire_control()` takes the camera and the input for the mod's
  mode until `release_control()`. While another mod holds them it fails with `CtxError::Busy { holder }`, whose text
  asks the player to leave that mode first; `control_holder()` tells who holds them. While a mod holds them only its
  camera override counts and only it gets input events (hotkeys still reach every mod).

The mock hosts follow: `MockCtx::tick`, `detach` and `hit` drive the new calls, `set_other_bodies` sets the bodies of
the other mods, `published_bodies` and `hits` read back what the mod did; `MockClientCtx::control_holder` plays
another mod in its mode.

What 1.2 added (a 1.1 mod builds unchanged):

- **Terrain share** (`world.share_terrain`, cosmetic). A seat's client draws the ground only of the hexes the seat
  has revealed; elsewhere there is no relief to stand on. `ServerCtx::share_terrain(seat, &plots)` lets that seat draw
  the ground of those plots although it has not revealed them (an arena in another player's land), and
  `ServerCtx::unshare_terrain(seat)` ends this mod's share for the seat. The host sends the seat, and only that seat,
  the fog-free basis records of the plots that are new to it (terrain, relief, continent art, lake and coast flags,
  rivers, relief features such as marsh or a natural wonder): the same records every client builds its relief from,
  so the relief is the same on every client. No owner, unit, city, improvement, route or resource goes with them.
  The client draws the plots like fogged revealed land (relief, ground, water, the trees of a forest or jungle);
  they stay unrevealed for the game: the strategic map, the minimap, the tooltips and the orders see nothing new.
  At most 256 plots a call into the mod; duplicates, plots off the map and plots the mod already shared to the seat
  are not sent again. A seat that lacks the mod gets nothing. The share lasts until the unshare, until the seat's
  client goes away (`on_detach`; share again in the next `on_attach`, which starts from nothing) or the game ends.
  The mock host lists what it would have sent (`MockCtx::terrain_shares`) and what a seat holds (`MockCtx::shared`).

What 1.3 added (a 1.2 mod builds unchanged; the whole UI part is described in [Panels in UI v2](#panels-in-ui-v2)):

- **The `ui` part and the `[ui]` section** of `mod.toml`: panels, a model of fields, scripts and a small state.
- **The `ui` capability** (client). `ClientCtx::ui_model()` gives the mod's own fields `mods.<id>.*` (the trait
  `ModUiModel`: `set`, `list_put`, `list_remove`, `list_clear`, with values of the type `UiValue`). Without the
  capability, or in a client without UI v2, it fails with `CtxError::Denied` or `CtxError::Unavailable("ui")`, and the
  mod goes on.
- **Intents of the panels.** `ClientMod::on_ui_intent(cx, name, args)` receives what the mod's own markup or script
  sends. The default does nothing.
- **The catalogue of UI points** `mod_api::UI_CATALOG` (UI API 1.0, its own version) and `check_ui_points`.
- **Mocks.** `MockClientCtx::ui_of(mod_idx)` returns a `MockUiModel` with the writes in order (`ops`), the values
  (`get`) and the lists (`keys`); `MockUiModel::declared(&[names])` refuses names the model does not have, like the
  real host. The dispatcher's `ClientRegistry`-built `on_ui_intent(host, mod_idx, name, args)` delivers an intent in a
  test.

### A mod that owns the screen

A client hook of a mod (a first-person mode, for example) can hide the game's interface while it holds the screen
(`HookCtl::suppress_original_ui(true)`) and give it back when it leaves (`false`). A mod's frame does the same with
`FrameOut::suppress_ui` (needs `input.claim`). While the interface is hidden, the active game UI of either kind gets
no frame and no input: the classic UI draws nothing, UI v2 draws neither its skin nor the panels of the mods, and the
game UI's cursor, movement path and trade routes are not shown. A focused text field of UI v2 lets the keyboard go.
What the mods write into their UI v2 fields is kept and shows when the interface comes back. The client skips the
whole game-interface block of its frame meanwhile, so its game session does not run either; the mods' own hooks do.
The HUD of the mods (`ClientMod::hud`, capability `hud`) is drawn after the game UI, on top of it, and still draws
while the interface is hidden.

## Panels in UI v2

UI v2 is the new game interface, a skin of HTML and CSS that the client lays out itself (see
[UI skins](ui-skins.md)). A mod can put its own panels into it. A panel is a small page of markup and a stylesheet.
It sits in an extension point of the skin, shows data the mod's Rust half writes, and sends buttons and other events
back to that half. The panels are for the web client only. The classic interface, the desktop client and a client
show none of this; a mod keeps its `hud` for them.

### The `[ui]` section

A mod with panels lists the part `ui` in `parts` and has a `[ui]` section (the manifest is refused if only one of
the two is there). The `[mod] api` requirement must be `^1.3` or narrower: 1.3 is the first mod API that has the
part, and a requirement that admits older versions (`^1`, `*`) is refused. A mod that writes fields or answers
intents also declares the capability `ui` and has a `client-rs` part. This is the section of the example:

```toml
[mod]
api   = "^1.3"
parts = ["server-rs", "client-rs", "ui"]

[rust]
caps = ["net.channel", "ui"]       # `ui`: the Rust half writes fields and gets intents

[ui]
api     = "^1"                     # the UI API (the points and tokens below) the panels need
model   = "ui/model.toml"          # the fields mods.<id>.* the Rust half writes
reads   = ["game.turn"]            # paths of the shared model the panels and the script read
scripts = ["ui/hello.js"]          # the mod's script files
state   = { open = true }          # the panels' own state, with defaults (state.* in the markup)

[[ui.panels]]
id       = "echo"                  # a-z, 0-9, '-'; at most 32; unique in the mod
source   = "ui/echo.html"          # the panel page
point    = "hud.right"             # an extension point, see the list below
mode     = "append"                # append | replace (required)
fallback = "mods"                  # mods (default) | none: where it goes if the skin lacks the point
```

- `api` is required. It is the lowest UI API version the panels need; the client checks it against the one it has. A
  client with another major version, or an older minor version than the panels need, cannot run them and shows one
  line in the `mods` point instead.
- `[[ui.panels]]` is required, from 1 to 16 panels. All panel pages of one mod sit in one folder.
- `model`, `reads`, `scripts` and `state` are optional. `scripts` has at most 8 files, `reads` at most 32 paths (they
  start at a root of the shared model, `players.*` style globs are allowed).
- `state` holds booleans, integers and strings, at most 32 values in all, a string at most 256 bytes. Nested tables
  make dotted paths (`filter = { kind = "all" }` is `filter.kind`). Names use a-z, 0-9 and `_`.
- Every file named in `[ui]` must exist. Unknown keys are errors, as everywhere in `mod.toml`.
- `mode = "replace"` puts the panel in place of the skin's element at the point, which hides that element. Only the
  single points can be replaced. Two mods of one game that replace the same point are a conflict, see
  [In the lobby](#in-the-lobby). `append` never conflicts.

### Extension points and color tokens

A panel goes into a point by its id, never by the classes of a skin. The points of UI API 1.0:

| Point | Kind | What it is |
|---|---|---|
| `mods` | stack | The shared slot every skin has. Panels whose point a skin lacks land here |
| `hud.top.extra` | stack | The free space of the top bar |
| `hud.menu` | stack | The menu group of the top bar (buttons) |
| `hud.right` | stack | The column under the notifications |
| `hud.unit.actions` | stack | The orders of the selected unit (buttons) |
| `window.production.detail` | stack | The detail side of the production window |
| `hud.minimap` | single | The minimap, replaceable |
| `hud.chat` | single | The chat, replaceable |

A stack takes panels after the skin's own children, in load order of the mods, then in the order of
`[[ui.panels]]`. A single point holds one element of the skin that a panel may replace. A skin may lack a point. Then
the panel goes to `mods`, or nowhere with `fallback = "none"`. A point that is not in the catalogue is an error in
`cargo xtask mods check` and keeps the mod from loading on the server. Points for buttons are meant for button
panels; this is advice, not a check. The same list, with the color tokens, is in
[UI skins](ui-skins.md#mount-points-and-tokens-for-mod-panels).

### Writing a panel

A panel page is plain HTML in the profile of the skins (the same elements, attributes and CSS properties; see
[UI skins](ui-skins.md)), with these extra rules:

- **One root element.** A panel page is one element and, before it, the `<link>` of its stylesheets. If the root has
  a `data-part`, it must be the panel's id.
- **Class prefix.** Every class the panel writes, toggles (`data-bind-class-toggle`) or selects in its CSS starts
  with `m-<mod id>-`. For `hello` that is `m-hello-`. A mod's classes then never meet the classes of a skin.
- **No layers, screens or points of its own.** `data-layer`, `data-screen` and `data-slot` are errors.
- **Colors from the tokens.** Read the skin's colors as `var(--ui-surface)` and the like (the list is below). Never
  define a `--ui-*` property, and do not read one that is not in the list. Other custom properties of your own are
  fine. The color follows the skin that is on the screen, so the panel looks at home in each.
- **Fonts by role.** Text uses `font-family: ui-sans` or `ui-display`. The page you get already starts with
  `ui-sans`. A mod brings no font files; `@font-face` is an error.
- **Text.** Write literal text in the markup or bind it from a field (`data-bind-value`). A mod has no text tables, so
  `data-l10n` is an error.
- **Data.** Expressions read the shared model, the mod's own fields `mods.<id>.*` (only its own, not another mod's)
  and the panel's state `state.*`.
- **Events.** `data-on-click="intent mods.hello.say()"` sends an intent of the mod. A panel may also send the intents
  of the catalogue of the shared model, and `set state.x = ...` or `toggle state.x` on its own state. It may not send
  `mods.<other id>.*`.

The color tokens: `--ui-surface`, `--ui-surface-2`, `--ui-line`, `--ui-text`, `--ui-text-2`, `--ui-text-3`,
`--ui-accent`, `--ui-accent-ink`, `--ui-negative`, and the yield colors `--ui-y-food`, `--ui-y-prod`, `--ui-y-gold`,
`--ui-y-sci`, `--ui-y-cult`, `--ui-y-faith`, `--ui-y-happy`, `--ui-y-tour`.

Budgets: a fragment package is at most 1 MB. The panels, scripts, reads and state have the counts given above. At run
time a mod may write at most 1024 values to its fields in one frame, a string of at most 1024 bytes, and keep at most
256 KiB in all; a write over a limit is refused with `CtxError::Rejected` and changes nothing.

### Building and checking

```bash
cargo xtask mods check --mods mods.toml          # parses [ui], checks it against the catalogue and the files
cargo xtask mods build --mods mods.toml --target web      # also converts the panels
cargo run -p ui2-convert -- fragment <mod dir> --out <dir>   # the converter on its own, for one mod
```

`mods check` is fast and needs no browser. It checks the shape of `[ui]`, that every point exists and only single
points are replaced, that the files exist, that `model` is a model fragment, that every `reads` path starts at a
root of the shared model, and that no two mods of the list replace one point. It does not read the markup.

`mods build` with `--target web`, `client` or `all` (and `dev --mods`) runs the converter once per mod. The converter
is `ui2-convert fragment`: it reads the mod's `mod.toml`, checks every panel with the profile and the fragment rules
above, and writes one package `<id>-<version>-<hash16>.ui2`. It prints one line `ui2-fragment <file> <hash>` on
stdout. Errors come with a code (E7-008 structure and tokens, E7-009 class prefix, E5-009 fonts, E8-002 budget, and the
profile's others), a file and a line; they are listed in [UI skins](ui-skins.md#error-codes). The notes of the last
run are under `target/ui2/mods/<id>/`.

The packages go to `ui2/mods/` of the web bundle with an index `ui2/mods-ui.json` (id, version, file, hash, UI API,
panels of each mod). The client reads the index when a game with mods starts and fetches the package of each mod of
the game's set whose version is the one in the index; a mod whose version differs has no panels. The converter needs
Node and Chromium like a skin build (`npm ci` in `tools/ui-convert/node` and in `tools/playtest`). With
`CIV5_SKIP_UI2=1` the bundle is built without the panels and the skins; the Rust halves still run.

### Fields and intents

The Rust half and the panels talk through two things, and only through them.

**Fields, from the half to the panels.** `ui/model.toml` declares the fields. It has the language of a model
fragment: one `[root]`, and optional `[type.Name]` tables for structs and keyed lists:

```toml
[root]
count     = "u32"      # echoes the server has counted
last_from = "str"
last_text = "str"
```

The fields appear in the panels as `mods.<id>.<field>` (`mods.hello.count`) and in the script. The Rust half writes
them with `cx.ui_model()`:

```rust
let ui = cx.ui_model()?;                 // needs the `ui` capability
ui.set("count", 3u32.into())?;           // a scalar field; a field of a struct is "stats.last"
ui.list_put("echoes", 7, &[("text", "hi".into())])?;   // insert or replace record 7
ui.list_remove("echoes", 7)?;
ui.list_clear("echoes")?;
```

A value is a `UiValue`: `Null`, `Bool`, `Int`, `Num` or `Str`. The host converts it to the declared type. An integer must
fit the field (`i8`, `i16`, `i32`, `u32`), a number goes to `f32`, a string to `str` or `text`, an enum field takes the
name of a variant (or its number), and `Null` clears an optional field. A write of another kind, out of range, to a
field that does not exist, or to a field that is a whole struct or list, is refused with `CtxError::Rejected` naming
the field. Fields of the kinds `asset` and `color` cannot be written by the Rust half. Writes are queued whichever UI is
on the screen, so a value written while the classic UI was showing is there when UI v2 takes over. They reach the
panels in load order of the mods, before the next frame is drawn.

**Intents, from the panels to the half.** A button of the panel or a call of its script raises
`mods.<id>.<name>` (the name is `a-z`, `0-9`, `_`, starting with a letter). The host checks that the intent comes from the
mod it names and hands it to the mod's `ClientMod::on_ui_intent(cx, name, args)`; `name` is the part after
`mods.<id>.`. Arguments are named, `intent mods.hello.say(n: 3)`; they arrive as `(String, UiValue)` pairs. Lists and
structs are passed as `Null`. At most 256 intents are kept per frame, later ones are dropped. An intent reaches the
Rust half one frame late: the mods' hooks run before the interface in each frame.

```rust
fn on_ui_intent(&mut self, cx: &mut dyn ClientCtx, name: &str, _args: &[(String, UiValue)]) {
    if name == "say" { self.say(cx); }
}
```

### Scripts and mod state

A mod may list script files in `[ui] scripts`. They are classic scripts (no modules), run in a worker of their own
(context `mod:<id>`), and get the object `ui`: `ui.model.get/on/onList` for reading, `ui.state.get/set/merge/on`
for the mod's own state, `ui.intent(name, args)`, `ui.on(name, fn)` for the events the markup raises with `call`,
`ui.island(id)` for script islands, `ui.settings.get/set/remove`, `ui.timer.after/every/cancel` and `ui.log.info/warn`. A script reads
only what `[ui] reads`, `mods.<id>.*` and its own state name; it cannot write the shared model. It may send only
intents `mods.<its id>.*`. A script that throws while loading, or hangs, is switched off alone: the panels go on with
their markup. A context that cannot start (no worker in this client) leaves the panels on markup alone as well. At
most eight script contexts run at once.

**Mod state.** `[ui] state` declares a small state of booleans, integers and strings with defaults. In the panels it is
`state.<path>` and the markup writes it (`set state.open = true`, `toggle state.open`, a control bound with
`data-bind-model`). In the script it is `ui.state.*`. The model calls it `modstate.<id>.*`. It belongs to the mod:
the skin cannot read or write it and one mod cannot touch another's. It is kept when the skin changes. The Rust half
cannot reach it; use fields for that.

### Looking right in any skin

The panel sits in a skin it did not see. Keep it small and flexible: sizes of its own, colors from the tokens, the
font roles, no fixed position. The converter checks the panel against reference values of the tokens; in the game they
are the skin's own colors. When the skin changes (`ui=v2:<skin>` in the address), the panels are
mounted again in the new skin.

If a client cannot show a mod's panels (another UI API, a model that does not mount, a package that does not load),
the mod shows one placeholder line in `mods` with the reason, and its Rust half runs as before.

## Distribution workflow

A mod is compiled into the server and the client, so an operator builds a distribution from a list of mods:

```toml
# mods.toml (paths are relative to this file)
[[mod]]
path = "examples/mods/hello"
```

```bash
cargo xtask mods check --mods mods.toml          # parse, licenses, resolve, UI points, rules R1-R11 on the dist graph, writes mods.lock
cargo xtask mods build --mods mods.toml          # server (default); --target client|web|desktop|all, --release
cargo xtask dev --mods mods.toml                 # builds the dist server and web client, installs the mods, runs
cargo xtask deploy --target <t> --mods mods.toml # release image of the dist; the mods baked in at /srv/civ5/mods
```

`dist` and `deploy` take `--mods <file>` and `--mod-packs <dir>` as `dev` does: the Linux release server and the
release web client are the dist ones, the mods go to `mods/` of the dist directory (the image copies it to
`/srv/civ5/mods`) and the `server.toml` of the image gets `mods_dir` pointing there.

`mods check` also accepts a git source (`git` with `rev`). It generates a separate workspace under `target/dist`,
so the public sources stay untouched; the build is reproducible from the list and `mods.lock`. In the dist build
the server runs with `panic = "unwind"` so that a panicking mod can be isolated; the server refuses to start with
mods otherwise. `dev --mods` takes the same flags as `dev` (`--core`, `--assets`, `--listen`, ...); the mods are
copied to `target/dist/installed/<id>/` and the server is pointed at that directory with `mods_dir`.

When the build has the web client (`mods build --target client|web|all`, and `dev --mods`), the panels of the mods
that have a `[ui]` section are converted and put into the bundle next to the skins, see
[Building and checking](#building-and-checking). The game server does not hand the panels out: they ship with the
client build, and it does not check that two players have the same panels.

The server scans `mods_dir` once at start (`<dir>/<id>/mod.toml`). A mod that is installed but has no code linked
into the binary is refused when a game tries to start with it. A mod whose panels use a point the server's API does
not have is not installed, with the reason in the log.

## In the lobby

The room settings show a **Mods** section when the server has mods installed. The host ticks the mods; the
server computes the plan (the exact versions, the dependencies it added, the load order, which mods players must
have) and shows it to everyone in the room. If the plan has an error, the text names the mod and the chain, and
the game does not start. At start the set is frozen into the game, the save and the recording. A client that lacks
a required mod is refused with the mod's name and version; a client without an optional cosmetic mod plays on. A
mod with an asset pack makes the client download it (`/mods/<hash>`) and check it before the game.

The panels add one rule to the plan. Two ticked mods that replace the same interface element are an error, and the
lobby says so: "mods `a` and `b` both replace the interface element `hud.minimap`; keep one of them". Un-tick one.
Mods that only append to a point never conflict. The lobby shows nothing else about panels: once the game starts,
each client mounts the panels of the game's mods, in load order, when UI v2 is on the screen.

## Limits and trust

- Mods are trusted code. A mod is compiled into the server and into every client; there is no sandbox. Install
  only mods whose source you have read. A sandboxed tier is planned.
- The server limits every message by the channel's size and rate; repeated violations end the session. State is
  limited by `state_max`.
- A mod sees only what its capabilities allow. The server half reads the whole truth (`world.read`) but never
  trusts a client payload for authority.
- A panel is data, not code of the game: its markup is checked against the profile, its scripts run in a worker with
  a time limit and see only the paths named in `reads`, and the host checks whose intent it is. It still shows what
  the mod's Rust half tells it, so the Rust half is as trusted as before.
- With no mods chosen, the lobby, the protocol bytes and the frame output are as before.

## The `hello` example

`examples/mods/hello` is the smallest complete mod; see its [README](../examples/mods/hello/README.md). The
server half counts `Say` messages and sends an `Echo` to everyone; the client half sends `Say` on **Alt+H**,
shows the echo in a HUD panel and draws a cube from a render system, standing on a plot the player owns. With UI v2
its panel `echo` (`ui/echo.html` in the point `hud.right`) shows "Echoes N", the last echo and a **Say hello** button
that sends `Say` too. Its files:

- `mod.toml` has `api = "^1.3"`, the part `ui`, the capability `ui`, and a `[ui]` with the model `ui/model.toml`, the
  script `ui/hello.js` and one panel (`mode = "append"`, `fallback = "mods"`);
- `ui/echo.html` and `ui/echo.css`: the panel. All classes start with `m-hello-`, the colors are `--ui-*` tokens and
  the font is `ui-sans`. The line "Echoes N" is bound to `mods.hello.count`, the second line is shown when the count
  is above zero, and the button has `data-on-click="intent mods.hello.say()"`;
- `ui/model.toml`: the fields `count` (`u32`), `last_from` and `last_text` (`str`);
- `ui/hello.js`: a script that only logs that it started (a panel needs none);
- `hello-client`: writes the three fields from `on_msg` and `frame` through `cx.ui_model()` and answers the intent
  `say` in `on_ui_intent` by sending `Say`, like the hotkey.

Its tests are the model for yours:

- `hello-server/tests` run the server half on the mock host built from the real `mod.toml`;
- `hello-client/tests` run the client half on `MockClientCtx` (including the panel's fields, with
  `MockUiModel::declared`, and its intent), and render the cube on a real device;
- `crates/server/server/tests/mods_conformance.rs` holds `check_server_mod(&registry, mod_toml, &probe)`, a
  reusable harness that checks the host contract for any server mod: the save round-trip is byte-stable, the
  state fits `state_max`, `on_attach` after a restart equals the live replay, oversize and garbage payloads and
  garbage saves never panic or change the state, and an undeclared channel is refused. Copy the call for your mod
  and give it a few valid messages.

Run the whole example in a browser:

```bash
cargo xtask dev --mods examples/mods/hello/mods.toml
```

Open http://127.0.0.1:8080/?ui=v2 in two tabs, register two accounts with the invite code `dev`, create a room as the
first, tick **Hello**, join with the second, start the game and press Alt+H, or click **Say hello** in the panel under
the notifications. For a quick single-player check, `?autostart=1&ui=v2&mods=hello` starts a game with the mod
ticked and no lobby (`&mods=` takes installed mod ids separated by commas; an id the server does not list is left
out). Without `ui=v2` the classic interface shows the same echo in the HUD panel.

## What is not supported yet

- Data mods (SQL patches, text), Lua gameplay and Lua UI mods, and C++ core patches (class G). The manifest
  reserves the parts; the loaders are not built.
- Class C (client-only) mods as a per-player setting.
- Sandboxed mods (WebAssembly components). The API is value-only so that this can be added without changing mods.
- Installing a mod without rebuilding the server and client. Today an operator compiles the mods in.
- Several versions of one mod in one game, and changing the mods of a running game.

For panels in UI v2:

- Panels in the desktop client and in the classic interface. Only the web client with UI v2 shows them.
- Pictures in a panel. The converter accepts them (at most 256 KiB of pixels each) with a warning, but the client does
  not draw a fragment's pictures yet. A panel cannot use the pictures of the mod's asset pack either. Use text, CSS
  shapes and the skin's colors.
- Other breakpoints. A panel's `@media` rules are used only when the skin has the same breakpoints; with any other
  skin the first one applies. The converter warns.
- Fonts and text tables of its own, and translations. A panel has the skin's font roles and its literal text; the
  language of the interface does not change it.
- Floating-point numbers, lists and tables-as-values in `[ui] state`: booleans, integers and strings only, 32 values.
  The Rust half's fields can hold numbers (`f32`), lists and structs.
- Writing `asset` or `color` fields from the Rust half, and passing lists or structs as intent arguments (they
  arrive as `Null`).
- Reading the mod's state from the Rust half, and writing the shared model or another mod's fields from a panel or a
  script.
- Panels that the server hands out. They come with the client build, and the server does not check that players have the
  same ones.
- Replacing `hud.right`, `hud.menu` and the other stack points, and adding points of a mod's own.
- Hot reload of a mod's panels. Rebuild the mod and reload the page.
