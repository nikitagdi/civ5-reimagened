# Mods

A mod changes or adds things to a game without touching the engine. The host of a game ticks the mods they want
in the lobby; the server and every client run the same set. The public build ships no mods and behaves exactly as
without this system. This page is for mod authors and for operators who build a server with mods.

Status: the first tier (Rust mods compiled into the server and the client) works. Data, Lua and C++ mods do not
exist yet, see [What is not supported yet](#what-is-not-supported-yet).

## What a mod is

A mod is a directory with a `mod.toml` and up to three Rust crates, named `<id>-shared`, `<id>-server` and
`<id>-client`. The example `examples/mods/hello` has all three. A mod is made of parts:

| Part | Runs in | Status |
|---|---|---|
| `server-rs` | The game's actor in the server process | works |
| `client-rs` | The client (browser and desktop) | works |
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
`order.before` are lists of ids; `[content]` points to the `assets` directory. Unknown keys are errors.

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
| `hud` | A draw list on top of the game UI |
| `render.pipeline` | A render system in the mod slot of the frame |
| `camera`, `terrain.cutout` | A camera override, a mask that cuts holes in the terrain |
| `world.bodies` | Publishing hit boxes, seeing those of the other mods and dealing hits to them (server, API 1.1) |
| `kv` | A small per-mod key-value store in the browser |
| `audio`, `assets` | Sounds, the mod's asset pack |

## The API crates

| Crate | License | Use |
|---|---|---|
| `mod-manifest` | MIT | Parses `mod.toml`, the version rules, the resolver, `mods.lock` |
| `mod-api` | MIT | The server half: `ServerMod`, the `ServerCtx` facade, `Outgoing`, `ServerRegistry`, and a mock host in `mod_api::testing` |
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

The mod API is at version 1.1. What 1.1 added (every new method has a default, so a 1.0 mod builds unchanged):

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

## Distribution workflow

A mod is compiled into the server and the client, so an operator builds a distribution from a list of mods:

```toml
# mods.toml (paths are relative to this file)
[[mod]]
path = "examples/mods/hello"
```

```bash
cargo xtask mods check --mods mods.toml          # parse, licenses, resolve, rules R1-R11 on the dist graph, writes mods.lock
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

The server scans `mods_dir` once at start (`<dir>/<id>/mod.toml`). A mod that is installed but has no code linked
into the binary is refused when a game tries to start with it.

## In the lobby

The room settings show a **Mods** section when the server has mods installed. The host ticks the mods; the
server computes the plan (the exact versions, the dependencies it added, the load order, which mods players must
have) and shows it to everyone in the room. If the plan has an error, the text names the mod and the chain, and
the game does not start. At start the set is frozen into the game, the save and the recording. A client that lacks
a required mod is refused with the mod's name and version; a client without an optional cosmetic mod plays on. A
mod with an asset pack makes the client download it (`/mods/<hash>`) and check it before the game.

## Limits and trust

- Mods are trusted code. A mod is compiled into the server and into every client; there is no sandbox. Install
  only mods whose source you have read. A sandboxed tier is planned.
- The server limits every message by the channel's size and rate; repeated violations end the session. State is
  limited by `state_max`.
- A mod sees only what its capabilities allow. The server half reads the whole truth (`world.read`) but never
  trusts a client payload for authority.
- With no mods chosen, the lobby, the protocol bytes and the frame output are as before.

## The `hello` example

`examples/mods/hello` is the smallest complete mod; see its [README](../examples/mods/hello/README.md). The
server half counts `Say` messages and sends an `Echo` to everyone; the client half sends `Say` on **Alt+H**,
shows the echo in a HUD panel and draws a cube from a render system, standing on a plot the player owns. Its tests are the model for yours:

- `hello-server/tests` run the server half on the mock host built from the real `mod.toml`;
- `hello-client/tests` run the client half on `MockClientCtx`, and render the cube on a real device;
- `crates/server/server/tests/mods_conformance.rs` holds `check_server_mod(&registry, mod_toml, &probe)`, a
  reusable harness that checks the host contract for any server mod: the save round-trip is byte-stable, the
  state fits `state_max`, `on_attach` after a restart equals the live replay, oversize and garbage payloads and
  garbage saves never panic or change the state, and an undeclared channel is refused. Copy the call for your mod
  and give it a few valid messages.

Run the whole example in a browser:

```bash
cargo xtask dev --mods examples/mods/hello/mods.toml
```

Open http://127.0.0.1:8080/ in two tabs, register two accounts with the invite code `dev`, create a room as the
first, tick **Hello**, join with the second, start the game and press Alt+H.

## What is not supported yet

- Data mods (SQL patches, text), Lua gameplay and Lua UI mods, and C++ core patches (class G). The manifest
  reserves the parts; the loaders are not built.
- Class C (client-only) mods as a per-player setting.
- Sandboxed mods (WebAssembly components). The API is value-only so that this can be added without changing mods.
- Installing a mod without rebuilding the server and client. Today an operator compiles the mods in.
- Several versions of one mod in one game, and changing the mods of a running game.
