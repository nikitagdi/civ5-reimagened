# hello

The smallest complete mod of civ5-reimagened: a synced-cosmetic echo. It is the public example and the
conformance subject of the mod API. Guide: [docs/modding.md](../../../docs/modding.md).

| Crate | Layer | What it does |
|---|---|---|
| `hello-shared` | shared | The messages of channel 0 (`Say`, `Echo`, `State`) and their byte codec |
| `hello-server` | server | Counts the `Say` messages, answers everybody with an `Echo`, replays `State` to a joining seat, keeps the count in the save |
| `hello-client` | client | **Alt+H** sends `Say`; a HUD panel shows the last echo; a cube is drawn in the mod render slot; a key-value store restores the personal counter after a reload; it writes the fields of the UI v2 panel and answers its button |
| `ui/` | UI v2 | The panel `echo` in the point `hud.right`: "Echoes N", "last: <nick>: <text>" and a **Say hello** button (`echo.html`, `echo.css`); its fields `mods.hello.*` (`model.toml`); a script context that only logs its start (`hello.js`) |

`mod.toml` declares class S (`cosmetic`) with `client = "optional"`: a player without the mod client still joins.
The mod has no asset pack. All code is MIT licensed so that you can copy it.

The `ui` part needs the mod API 1.3 (`api = "^1.3"`) and the capability `ui`. The panel is cosmetic: with the
classic interface the HUD panel of `hud()` shows the same echo, and a skin that lacks `hud.right` shows the panel in
its `mods` column. The panel's classes start with `m-hello-`, its colors are the skin's `--ui-*` tokens and its font
is the skin's `ui-sans` role, so it looks at home in any skin.

## Try it

```bash
cargo xtask mods check --mods examples/mods/hello/mods.toml       # resolve and check the rules
cargo xtask mods build --mods examples/mods/hello/mods.toml       # server with hello linked in
cargo xtask dev --mods examples/mods/hello/mods.toml              # server + web client, http://127.0.0.1:8080/
```

In the browser: register two accounts with the invite code `dev`, create a room, tick **Hello** in the Mods
section, let the second player join, start, and press **Alt+H**. Both HUDs show the echo, and an orange cube
stands on a plot of your territory. With UI v2 (`&ui=v2` in the address) the panel under the notifications counts
the echoes and its **Say hello** button says hello like the hotkey. (Click the game canvas first: the hotkey fires only when the game did not use
the press. Two accounts in one browser need two hosts, for example `127.0.0.1` and `localhost`, because the session
cookie is shared.)

## Tests

```bash
cargo test -p hello-shared -p hello-server -p hello-client
cargo test -p server --test mods_conformance     # the reusable host-contract harness, run on hello
```

To write your own mod, copy this directory, change the id in `mod.toml` and the crate names, and keep the single
`register` function in each half.
