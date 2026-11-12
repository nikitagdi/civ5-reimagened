# UI skins

A UI skin is a folder of plain HTML, CSS and a few classic scripts under `skins/<id>/`. It opens in an
ordinary browser as a static mockup, and the skin converter turns it into a package (`<id>-<hash16>.ui2`) that
the game client lays out and draws itself, without a browser engine. The repository has two skins:
`skins/engraved`, a full skin, and `skins/vellum`, a light one derived from it.

The document has four parts. "Making a skin" takes you from an empty folder to a skin running in the game.
"Converter" is the reference of the tool: what it accepts, its commands and its error codes. "Skin package"
describes the file the converter writes. "Profile" is generated from the profile file and lists every element,
property and error code.

## Making a skin

This part is written for someone who has not made a skin before. Read "Required tools" under "Converter" first and
install what it lists. Run every command from the repository root.

### Step by step: a skin called `tiny`

The skin below has one strip with a number and a button. It is small on purpose; every file of it is shown.

1. **Make the folder.** A skin is a folder `skins/<id>/`. The id is lower case letters, digits and `-`, starting with
   a letter. Make `skins/tiny/` with the folders `css/`, `parts/`, `l10n/`, `sample/` and `dev/`.

2. **Write `skin.toml`.** It names the skin, its fonts and what to check:

   ```toml
   [skin]
   id = "tiny"
   version = "0.1.0"
   title = "skin.title"
   entry = "index.html"
   model = "1.0"
   api = "1.0"

   [fonts]
   subset = ["latin"]
   sans = "Fira Sans"

   [fonts.files]
   "fonts/FiraSans-Regular.ttf" = "third_party/fonts/fira-sans/FiraSans-Regular.ttf"

   [verify]
   states = ["hud"]
   ```

   `title` is a text key (step 6). `model` and `api` are the versions of the model schema and of the script API the
   skin is written against. The key `[fonts.files]` is the path a stylesheet names for a font; the value is the
   font file, counted from the repository root. A skin never ships font files of its own. `states` lists the
   sample files the checks use (step 7).

3. **Write the page, `index.html`.** The children of `body` are the four layer roots, in this order: `map`, `hud`,
   `window`, `tooltip`. The page also has one element with `data-slot="mods"`, where panels of mods are mounted.
   `data-include` pulls in a part file; `data-ui2-dev` marks what only the browser mockup needs.

   ```html
   <!doctype html>
   <html lang="en">
   <head>
     <meta charset="utf-8">
     <title>Tiny</title>
     <link rel="stylesheet" href="css/tokens.css">
     <link rel="stylesheet" href="css/base.css">
     <link rel="stylesheet" href="dev/mockup.css" data-ui2-dev>
   </head>
   <body>
     <div class="layer" data-layer="map"></div>
     <div class="layer" data-layer="hud">
       <div data-include="parts/top.html"></div>
       <div class="mods" data-part="mods" data-slot="mods"></div>
     </div>
     <div class="layer" data-layer="window"></div>
     <div class="layer" data-layer="tooltip"></div>
   </body>
   </html>
   ```

4. **Write a part, `parts/top.html`.** A part is a piece of markup that the page includes. It shows a text, a
   number from the model and a button that sends an intent and has a hotkey:

   ```html
   <header class="top" data-part="top">
     <span class="label" data-l10n="top.gold">Gold</span>
     <span class="value" data-part="top.gold" data-bind-value="{{top.gold}}">312</span>
     <button class="end" data-part="top.end-turn" data-hotkey="Enter" data-on-click="intent turn.primary()">
       <span data-l10n="top.end">End turn</span>
       <span class="key" data-hotkey-label>Enter</span>
     </button>
   </header>
   ```

   The text inside `data-l10n` and `data-bind-value` is only what the mockup shows in a browser. The game shows the
   text of the key and the value of the model path.

5. **Write the styles.** `css/tokens.css` holds the color tokens and the 17 `--ui-*` synonyms that every skin must
   define (the list is in "Extension points and color synonyms" under "Profile 1.0"):

   ```css
   :root {
     --paper: #1b1815;
     --paper-2: #26221d;
     --line: #4d4032;
     --ink: #ebe2d2;
     --ink-2: #b2a794;
     --ink-3: #8a8072;
     --brass: #c9a36a;
     --brass-ink: #1b1815;
     --red: #cf6f5c;

     --ui-surface: var(--paper);
     --ui-surface-2: var(--paper-2);
     --ui-line: var(--line);
     --ui-text: var(--ink);
     --ui-text-2: var(--ink-2);
     --ui-text-3: var(--ink-3);
     --ui-accent: var(--brass);
     --ui-accent-ink: var(--brass-ink);
     --ui-negative: var(--red);
     --ui-y-food: var(--ink);
     --ui-y-prod: var(--ink);
     --ui-y-gold: var(--brass);
     --ui-y-sci: var(--ink);
     --ui-y-cult: var(--ink);
     --ui-y-faith: var(--ink);
     --ui-y-happy: var(--ink);
     --ui-y-tour: var(--ink);
   }
   ```

   `css/base.css` loads the font, resets the page, makes the layer roots and styles the strip. The properties must
   be in the profile; the page itself must have no background.

   ```css
   @font-face {
     font-family: "Fira Sans";
     font-style: normal;
     font-weight: 400;
     src: url("../fonts/FiraSans-Regular.ttf");
   }

   * { box-sizing: border-box; margin: 0; padding: 0; }

   html, body { width: 100%; height: 100%; overflow: hidden; }

   body { position: relative; font: 400 14px/20px "Fira Sans"; color: var(--ui-text); }

   .layer { position: absolute; inset: 0; pointer-events: none; }

   button {
     appearance: none; border: 0; border-radius: 0; background-color: transparent;
     color: inherit; font: inherit; cursor: pointer;
   }

   .top {
     position: absolute; top: 0; left: 0; right: 0;
     display: flex; align-items: center; gap: 12px;
     height: 36px; padding: 0 12px;
     background-color: var(--ui-surface);
     pointer-events: auto;
   }
   .value { color: var(--ui-y-gold); }
   .end {
     display: flex; align-items: center; gap: 8px; height: 24px; padding: 0 10px;
     background-color: var(--ui-accent); color: var(--ui-accent-ink);
   }
   .end:hover { background-color: var(--ui-text); }
   .key { padding: 0 4px; border: 1px solid var(--ui-accent-ink); font-size: 12px; }
   .mods { position: absolute; top: 44px; right: 12px; display: flex; gap: 8px; }
   ```

   `dev/mockup.css` is only for the browser: a flat page color that stands in for the world. It is linked with
   `data-ui2-dev`, so the converter ignores it.

   ```css
   html, body { background-color: #0f0e0c; }
   ```

6. **Write the texts, `l10n/en_US.toml`.** Every visible text comes from a table like this one or from a binding:

   ```toml
   "skin.title" = "Tiny"
   "top.gold" = "Gold"
   "top.end" = "End turn"
   ```

7. **Write a sample, `sample/hud.json`.** A sample is a piece of the model written as JSON. The checks and the
   browser preview use it instead of a game:

   ```json
   { "model": { "top": { "gold": 312 } } }
   ```

8. **Check it.**

   ```
   cargo xtask ui check tiny
   ```

   The command loads the skin in Chromium and compares it with the profile. It prints counts and then
   `ui2-convert: no errors`, or errors with a file and a line (see "Error codes"). Fix them one by one.

9. **Compare the layout, then build.**

   ```
   cargo xtask ui verify tiny
   cargo xtask ui build tiny
   ```

   `verify` lays the package out with the game's runtime and compares it with Chromium at several window sizes; it
   must end with `verify: layout matches Chrome`. `build` writes `target/ui2/tiny/tiny-<hash16>.ui2` and prints the
   size of each section against the budget.

10. **See it in a browser.** Run `cargo xtask ui watch tiny`. It prints the address of a preview page
    (`http://127.0.0.1:8095/index.html` by default). The page shows the skin with its includes expanded and its
    bindings filled from `sample/hud.json`. The tool keeps running and rebuilds on every save.
    Opening `index.html` straight from the disk shows only the entry file, without the includes. Stop the tool
    with Ctrl+C.

11. **Run it in the game.** Start the development server with the skin watched:

    ```
    cargo xtask dev --ui-dev tiny
    ```

    The command also builds the web client with every skin of `skins/`. Open
    `http://127.0.0.1:8080/?ui=v2:tiny&ui2dev=http://127.0.0.1:8095&ui2sample=hud`. The page runs the skin on its
    sample, with no game. Without `&ui2sample=hud` you sign in and start a game as usual, and the skin shows the
    game. Save a file of the skin, and the running page changes within a few seconds. The first build of the
    watch runs Chromium and takes longer.

12. **Ship it.** A skin under `skins/` goes into the web bundle with `cargo xtask build --only web` and into the
    Docker context of `cargo xtask dist`. Set `default = true` in `[skin]` to make it the skin that `?ui=v2` picks.

### Files of a skin

| Path | What it is |
|---|---|
| `skin.toml` | The manifest (see below). |
| `index.html` (or the `entry` file) | The page: the four layer roots and the `data-include` lines. |
| `parts/` | Part files. The name is yours; a part may include other parts, but not itself. |
| `css/` | Stylesheets, linked from the page. |
| `icons.svg` | An icon sprite: `<symbol id="...">` elements. A part shows one with `<svg class="i"><use href="#name"/></svg>`. |
| `l10n/<lang>.toml` | One text table per language, such as `en_US`. |
| `sample/<state>.json` | Samples. |
| `js/` or `scripts/` | Skin scripts, listed in `[scripts] files`. |
| `dev/` | Files only for the browser, linked with `data-ui2-dev`. |
| pictures | `png`, `webp` or `svg`; put a `name@2x.ext` beside `name.ext` for the 2x version. Give the element a size in CSS. |

All paths are relative to the skin folder and may not leave it. A skin loads nothing from the network.

### `skin.toml`

Unknown keys are errors. Every table but `[skin]` is optional.

| Table | Keys |
|---|---|
| `[skin]` | `id`; `version` (`major.minor.patch`); `title` (a text key); `entry` (default `index.html`); `model` and `api` (`major.minor`); `default` (true for the skin `?ui=v2` picks); `extends` (the id of a base skin, see "Derived skins"). |
| `[fonts]` | `subset`: the character ranges every face keeps, from `latin`, `latin-ext`, `cyrillic`. `sans` and `display`: the family names of the font roles `ui-sans` and `ui-display`, which mod panels use. `[fonts.files]`: the path a stylesheet names for a font, mapped to a file under `third_party/fonts`. |
| `[state]` | The skin's own state with defaults (see "Skin state"). |
| `[themes]` | `default = "<name>"` and `<name> = { css = "themes/<name>.css" }`: other sets of values for the root color tokens. A theme file holds `:root { --token: color }` only. The package keeps the themes. |
| `[scripts]` | `files`: classic scripts in the order they run. `reads`: the model roots the scripts read. |
| `[verify]` | `states`: sample names; the first one is the sample the converter scans and `ui verify` uses unless you pass `--state`. `golden`: a list of `{ state, viewport = [w, h], scale }`; the converter only checks that the numbers are positive. |

### Markup in short

The full list of elements, attributes, properties and error codes is in "What it accepts" and in "Profile 1.0".
This is what a skin author uses most.

- **Layers.** `data-layer` is `map`, `hud`, `window` or `tooltip`. A layer root covers the screen and passes the
  pointer through; each panel in it switches `pointer-events: auto` on, so a click beside a panel reaches the map.
- **Parts (`data-part`).** A part name labels one element: `top.yield.gold`. It is a stable name for tools. The
  layout check lists its worst differences by part, and the playtest finds elements by it. A name may be used once
  in a skin. Names are lower case words, digits and `-`, joined by dots.
- **Extension points (`data-slot`).** A slot is a place where something else can be mounted. The skin must have
  `mods`. The skins here also have `hud.menu`, `hud.top.extra`, `hud.unit.actions`, `hud.minimap`, `hud.right` and
  `window.production.detail`; the names are the skin's choice. A slot sits on a plain element, never on one with
  `data-bind-if` or `data-bind-for`.
- **Screens (`data-screen`).** A window of the skin is a child of the `window` layer with `data-screen="<id>"`; in
  `skins/engraved` its `data-bind-if` reads `ui.windows.<id>.open`, so the intents `window.open` and
  `window.close` show and hide it. Esc closes the topmost visible screen.
- **Bindings.** `data-bind-value`, `data-bind-if`, `data-bind-for` with `data-bind-key`, `data-bind-sort` and
  `data-bind-filter`, `data-bind-class-toggle`, `data-bind-style-<property>`, `data-bind-src`,
  `data-bind-disabled`. An expression is written in `{{ }}` and is checked against the model schema when you
  build: a path that does not exist, or a value of the wrong type, is an error at the element.
- **Intents.** `data-on-click="intent turn.primary()"` asks the game to do something. The catalogue of intents,
  their arguments and every model root and type is in `crates/shared/ui2-model/schema/ui-model.toml`; the checker
  reads the same file. Arguments are written by name (`window.open(window: 'research')`) or all by position. One
  attribute may hold several actions separated by `;`. The other actions are `set state.x = <expression>`,
  `toggle state.x`, `call <name>(<args>)` (an event for the skin's script) and `stop`. The events are `click`,
  `dblclick`, `contextmenu`, `pointerenter`, `pointerleave`, `pointerdown`, `pointerup`, `input`, `change`,
  `submit`, `focus` and `blur`.
- **Texts.** `data-l10n="<key>"` with `data-l10n-args` for the arguments, as in `"top.turns" = "{1_Num}
  {1_Num: plural 1?turn; other?turns;}"`. A key is lower case words joined by dots.
- **Less common attributes.** `data-asset="render:minimap"` shows a game asset (the element needs a size in CSS).
  `data-tooltip="<id>"` names a `<template data-layer="tooltip" id="<id>">`, a child of the `tooltip` layer root
  that holds exactly one element; while the node is hovered the runtime shows that element in the tooltip layer
  under the node. Its bindings read the model and the skin state, not the loop variables of the node: a row of a
  list names its item through `ui.*` (the production rows set `ui.production.hover`). `data-control` makes a checkbox,
  toggle, radio, slider or select that writes a state value named by `data-bind-model="state.x"`.
  `data-anchor="city:{{ c.id }}"` (also `unit` and `plot`) pins an element to a map object; it goes on the node
  with `data-bind-for`, in the `map` layer. `data-fit="shrink"` with `data-fit-min` shrinks text to fit.
  `data-ui2-island` marks a part of the page that a script drives (see "Skin scripts"). `skins/engraved` uses
  `data-asset` and `data-tooltip` (`parts/tips.html`) and not the others; the attribute grammar is in the profile file and its error texts are
  in "Error codes".

### Tokens

A color token is a custom property on `:root` whose value is a color: `--brass: #c9a36a`. The color properties
that the profile marks *token* may refer to a token with `var(--brass)`, and the package keeps the token names
(section `TOKN`). The `--ui-*` synonyms are the same names in every skin, each defined as `var(--<token>)`. Mods
read the synonyms and never define them, so a mod panel looks right in any skin. Only the values differ between
skins: `skins/vellum/css/tokens.css` has the same names as `skins/engraved/css/tokens.css` with parchment colors.
Other custom properties (a font stack, an easing curve, a size) are fine too: Chromium resolves them when you
build.

### Samples and states

A sample is `sample/<state>.json`:

```json
{ "model": { "top": { "gold": 312 } }, "state": {}, "ui": {}, "anchors": { "city:3": [1320, 640] } }
```

- `model` has the model roots by name. A field you leave out takes its default; a field the schema does not have is
  an error (E4-040). Numbers are numbers, text is a string or `{"k": "key", "a": ["arg"]}`, a key is
  `"owner:id"`, an enum is the name of its value, an optional value is the value or `null`, a list is an array.
- `state` writes the skin's own state; it may use only paths declared in `[state]` (E4-041).
- `ui` writes the `ui` root: which windows are open, the focused row, and so on.
- `anchors` gives the screen position of map objects for `data-anchor`.

Make one sample per situation you want to check: `skins/engraved/sample` has the HUD, the HUD with a turn timer, the
production window and the research window. The names are free; use `a-z`, `0-9`, `-` and `_`. They are used in
four places: the converter scans the first state of `[verify] states`; `ui verify` lays out that state or the one
you pass; the browser preview takes one with `?ui2state=<name>`; the game takes one with `?ui2sample=<name>`.
The web build copies all samples next to the packages.

### Skin state

`[state]` declares values that belong to the skin and are not in the game: the sort order of a list, a hidden
panel. A value is a bool, an integer, a float or a string, and its type is the type of the default. Dotted keys and
inline tables make groups:

```toml
[state]
prod = { sort = "order", hide_locked = false }
```

Each name is lower case letters, digits and `_`, starting with a letter. The converter refuses any other name
(E1-001), because the game could not mount it. At run time the paths are `state.prod.sort` and so on. Markup
writes them with `set` and `toggle` and reads them in bindings; scripts use `ui.state.get("prod.sort")` and
`ui.state.set(...)`. A path that is not declared cannot be written. See "Delivery" for what happens to the state
when the skin changes.

### Skin scripts

A skin script is a classic JavaScript file. List it in `[scripts] files` and name the model roots it reads in
`reads`. It has one global, `ui`, and no `document`, `window`, network or storage (E1-008); `import` and `export` are
errors (E1-007). It runs in a worker, so it never blocks a frame. Markup calls it with `call <name>(<args>)`; the
script answers with `ui.on("<name>", fn)`. What `ui` has:

| Call | What it does |
|---|---|
| `ui.on(name, fn)` | Handles a `call` of the markup. The event `ready` comes once, after the files ran. |
| `ui.model.get(path)`, `ui.model.on(path, fn)`, `ui.model.onList(path, {add, change, remove})` | Reads the model, or watches a path or a list. Only inside `reads`. |
| `ui.state.get(path)`, `.set(path, value)`, `.merge(path, obj)`, `.on(path, fn)` | The skin's state (no `state.` prefix), and the `ui.*` fields the schema lets scripts write. |
| `ui.intent(name, args)` | Sends an intent. |
| `ui.settings.get(key)`, `.set(key, value)`, `.remove(key)` | The skin's settings: JSON values the host keeps for it. |
| `ui.timer.after(ms, fn)`, `.every(ms, fn)`, `.cancel(id)` | Timers. |
| `ui.island(id).text(key, s)`, `.cls(key, name, on)`, `.style(key, prop, v)`, `.show(key, on)` | Drives a `data-ui2-island` part of the page by key. Bindings inside an island are errors (E4-035). |
| `ui.log.info(...)`, `ui.log.warn(...)` | Logging; the rate is limited. |

`skins/engraved/js/production.js` is a worked example: it moves the focus in the production list with the arrow
keys and keeps the sort in the settings. A handler that throws is switched off alone. A script that fails to load
takes the whole skin down, so guard calls that may not exist on a host. In `ui watch`, a change only to
`scripts/*.js`, or only to `l10n/` and `sample/`, is built without Chromium; any other change runs the full cycle.

### Hotkeys and their labels

`data-hotkey="[Ctrl+][Alt+][Shift+]<KeyboardEvent.code>"` on an element makes the key click it: `KeyB`,
`Ctrl+KeyB`, `F5`, `ArrowUp`, `Delete`. The key acts only while the element is shown and not disabled. The topmost
layer wins (`window`, then `hud`, then `map`; the `tooltip` layer never takes keys), and a visible screen blocks the
layers under the `window` layer. A text field has the keyboard first. A hotkey may not be inside a
`data-bind-for` body, and one key may be used once per layer.

`data-hotkey-label` on an element inside the one with `data-hotkey` fills it with the name of the key. The name
is the skin's text `key.<code>` if the table has it, such as `"key.KeyB" = "B"` or `"key.Ctrl" = "Ctrl"`. Without
one, it is the letter of `KeyX`, the digit of `DigitN`, or the code itself, and the modifiers come first:
`Ctrl+Shift+B`. The text between the tags is a mockup text only. A label with no hotkey around it is an error
(E7-013). Key names with the code's exact spelling matter: `key.F7`, not `key.f7`.

### Preview shim and local server

Two helpers make a skin work in a browser.

- **The local server.** `cargo xtask ui watch <skin>` starts a server on `127.0.0.1` (port 8095, or `--port`). It
  serves the skin folder and adds a preview script to every page: it expands `data-include`, numbers the nodes,
  evaluates the bindings against `sample/<state>.json` and fills the texts from `l10n/<lang>.toml`. The page
  accepts `?ui2state=<sample>`, `?ui2lang=<table>` and `?ui2mode=layout|extract|templates`; the default `layout`
  shows the honest sample. Problems never stop the page: they are collected in `window.__ui2preview.errors`. The
  same server answers `/pkg/<id>.ui2`, `/pkg/<id>.rev` and `/pkg/<id>.errors.json` for hot reload.
- **The mockup shim.** `skins/_shim/ui2-shim.js` gives a skin's own scripts the `ui` object in a plain page, also
  from `file://`. Add `<script src="<path>/apps/web/www/ui2/api.js" data-ui2-dev></script>` and the shim script
  with `data-ui2-dev` before the skin's scripts. The shim re-renders only bindings to `state.*` and `ui.*`; lists
  and other model bindings keep the static mockup. Intents go to the console and to `window.__ui2shim.intents`.
  The sample comes from `window.__ui2sample`, a `<script type="application/json" data-ui2-sample>` element, or
  `sample/<state>.json` (`?state=<name>`, served pages only). `?ui2shim=off` stops it.

### Hot reload and page parameters

`cargo xtask dev --ui-dev <skin>` runs `cargo xtask ui watch <skin>` on port 8095 next to the dev server (it
accepts `--ui-dev=<skin>` too) and stops it when the dev server stops. The client page takes these parameters:

| Parameter | Meaning |
|---|---|
| `ui=v2` or `ui=v2:<skin>` | Use UI v2, with the default skin or the named one. A skin the index does not know falls back to the default; a skin that fails to load falls back to the default skin, then to the classic UI, and is not tried again. |
| `ui2dev=<url>` | The address of a watch server, such as `http://127.0.0.1:8095`. The client asks `/pkg/<id>.rev` twice a second and swaps in the new package in place. The model, the open windows and the skin's `state.*` stay (a value stays if its path and type are still declared). A failed build is shown as a notice and the running skin stays. |
| `ui2sample=<state>` | Run on `sample/<state>.json` of the built skin instead of a game (with `ui=v2[:skin]`). The page then opens no server connection and shows no sign-in; map anchors the sample gives no point stand where the layout puts them. The name is `a-z`, `0-9`, `-`, `_`, up to 64 characters. |
| `ui2bg=<rrggbb>` | A solid background under the skin, and the clock of its transitions stopped, for reference images: a style that changes (hover, press, focus) shows at once. |

### Derived skins

A skin with `extends = "<base>"` in `[skin]` holds only the files it changes. The base is the folder `skins/<base>`
beside it. Every file of the base is there; a file of the derived skin replaces the one of the same path; a
`l10n/<lang>.toml` of the derived skin is merged key by key, so it can carry only the keys it changes. Any table
of `skin.toml` that the derived skin writes replaces the base table whole, and `[skin]` is always the derived
skin's own (`default` is not inherited). A chain of bases may be 4 levels deep. All the commands take a derived
skin like any other.

`skins/vellum` is the example. Its `skin.toml` is the `[skin]` table with `extends = "engraved"`; its `css/tokens.css`
has the parchment values for the token names of the base; its `l10n/en_US.toml` has the title; its
`parts/notes-extra.html` replaces the base's column under the notifications with one that is not the mount point
`hud.right`, so a mod panel that asks for that point stands in `mods` under vellum. The rest of the layout, the
scripts, the fonts, the state, the samples and `[verify]` are the base's.

### Worked examples

- `skins/engraved` is a full skin. `index.html` has the layers and includes the parts of `parts/`
  (`top`, `notes`, `unit`, `helm`, `production`, `research`); `css/` has tokens, base, one file per area;
  `icons.svg` is the sprite; `js/production.js` is the script; `sample/` has five samples; `[state]` holds the
  production sort and filter.
- `skins/vellum` is a derived skin: four small files.

### Not supported

- Anything outside the profile: tables and forms, `float`, `::before` and `::after`, structural and sibling
  selectors, `@keyframes`, `prefers-color-scheme`, viewport units and any unit other than px (and `%` and `fr`
  where the profile lists them), a page background.
  Each gives an error with a file and a line.
- Inline scripts, modules, and scripts that use the browser's objects.
- Fallback fonts: a character that no font of the skin has is an error.
- Text that is not in a text table or a binding, and the `title` attribute.
- Lists in `[state]`, and state names that are not lower case snake case.
- Layout comparison in `ui watch`; run `ui verify` for it.
- A way to pick a theme at run time: the package keeps the themes, and no setting of the client selects one yet.
- The desktop application. UI v2 skins run in the web client only (`?ui=v2`).

## Converter

The converter is the tool `tools/ui-convert` (crate `ui2-convert`). It uses Chromium as a style compiler:
Chromium loads the skin, resolves the cascade and computes every style value; the converter checks the page
against the UI profile and writes the package. Nothing is approximated: whatever the profile cannot hold is
an error with a file and a line.

### What it accepts

- **Folder layout.** `skin.toml` (id, version, title key, entry file, model and API versions), the entry page
  (`index.html` by default) with `data-include` parts, stylesheets, `l10n/<lang>.toml` text tables,
  `sample/<state>.json` sample data, pictures (`png`, `webp`, `svg`, with an optional `name@2x.ext` beside
  them), an icon sprite of `<symbol>` elements, and classic scripts listed in `[scripts] files`.
- **Document.** The children of `body` are the four layer roots, in this order: `data-layer="map"`, `hud`,
  `window`, `tooltip`. Each root is `position: absolute; inset: 0; pointer-events: none`. Somewhere in the skin
  an element carries `data-slot="mods"`, and `:root` defines the color synonyms mods rely on (`--ui-text`,
  `--ui-surface`, `--ui-accent` and the others the profile lists).
- **Elements and attributes.** The profile's elements only (blocks, text elements, `button`, `input
  type=text`, `img`, inline `svg` icons). Data comes in through the profile's attributes: `data-bind-value`,
  `data-bind-if`, `data-bind-for` (with `data-bind-key`, `-sort`, `-filter`), `data-bind-class-toggle`,
  `data-bind-style-*`, `data-l10n`, `data-on-<event>` actions (`intent`, `set state.x`, `toggle`, `call`),
  `data-hotkey`, `data-anchor`, `data-tooltip`, `data-asset`. Expressions are typed against the model schema.
- **CSS.** Longhand and shorthand properties of the profile, written with px, %, fr and the profile's keywords;
  selectors made of tags, classes, ids and `:root` joined by a space or `>`, plus `:hover :active :focus
  :focus-visible :disabled`; `@media` on width and height in px; `@font-face` pointing at the font files that
  `skin.toml [fonts.files]` maps to the repository's `third_party/fonts`.
- **Text.** Every visible text comes from an l10n table or a binding. Fonts are subset to the characters the
  skin can show; the runtime has no fallback fonts, so every character must exist in a font of the skin.

The exact tables (elements, properties and their values, error texts) live in the profile file
`crates/shared/ui2-package/profile/profile.toml`, which the converter reads; the section "Profile 1.0" below is
generated from it.

### Commands

| Command | What it does |
|---|---|
| `cargo xtask ui check <skin>` | Runs every check (both Chromium passes, font subsets, pictures, budget) and writes no package. Exit code 1 with errors. `--dump-ir <file>` stores the scan for tests that run without a browser. |
| `cargo xtask ui build <skin>` | The same, then writes `target/ui2/<id>/<id>-<hash16>.ui2`, `build.json` (section sizes and budget), `errors.json` and `ir.json`. Two builds of the same sources give the same bytes. |
| `cargo xtask ui verify <skin> [--state <s>]` | Lays the package out with the game's own runtime and compares every element's rectangle with Chromium's at 1280x720, 1920x1200, 3440x1440 and 5120x1440 (and at 1280x720 with a pixel density of 1.5); a difference over 0.5 px fails. Without `--state` it uses the first `[verify] states` entry. The report lists the worst elements with their source place. |
| `cargo xtask ui watch <skin> [--port 8095]` | Keeps Chromium and a local package server running (127.0.0.1 only) and rebuilds on every saved change. The client polls `/pkg/<id>.rev` when started with `?ui2dev=http://127.0.0.1:8095` and reloads the package; a broken edit keeps the last good package and serves the errors at `/pkg/<id>.errors.json`. |
| `cargo xtask build --only web` | The web bundle; it builds every skin under `skins/` into `target/web/ui2/` with the index `ui2/skins.json`. |
| `cargo xtask mods build --mods <file> --target web` | Also converts the panels of every mod with a `[ui]` section into one package of kind fragment, `ui2/mods/<id>-<version>-<hash16>.ui2`, listed in `ui2/mods-ui.json` (the converter's `fragment <mod dir> --out <dir>` command). A panel page follows the profile and the rules of a fragment: one root element, classes starting with `m-<mod id>-`, the `--ui-*` color tokens read and never defined, the font roles `ui-sans` and `ui-display` instead of font files, text written in the markup or bound (a mod has no text tables), at most 1 MB per fragment. |
| `cargo xtask dist` | The Docker build context; the skin packages are built before the context is assembled, so the image needs no Node, Chromium or Python. |

`<skin>` is a skin id under `skins/` or a folder with a `skin.toml`. Set `CIV5_SKIP_UI2=1` to build the web
bundle or a dist without skins.

### Error codes

Every error is printed as `error[Ex-nnn]: <file>:<line>:<col>: <element or selector> <property>`, followed by
why, a hint and, for a computed value, the element it applied to. Errors are sorted by place, so two runs print
the same list. Each code has its own text in the profile file; the families are:

| Family | Meaning |
|---|---|
| E1 | HTML, attributes, files: an element or attribute outside the profile, a resource outside the skin folder, a script that imports modules or names browser APIs. |
| E2 | Properties and values: a property or value outside the profile (also when it came from the browser's own styles or inheritance), a unit other than px, an unclosed scroll container, a page background. |
| E3 | Selectors, at-rules and variants: structural or sibling selectors, pseudo-elements, at-rules other than `@media` and `@font-face`, media queries outside width and height, more than four dynamic conditions on one node. |
| E4 | Bindings and expressions: unknown paths, wrong types, loops over something that is not a list, malformed actions, intent arguments of the wrong type. |
| E5 | Fonts, pictures and game assets: SVG content outside the profile, icons that mix `currentColor` with fixed colors, characters no skin font has, a font face whose line height depends on the platform (it needs the USE_TYPO_METRICS bit, or the same ascent and descent in its hhea, OS/2 typo and OS/2 win tables and no line gap), a game asset without a CSS size. |
| E6 | Texts: untranslated literal text, missing text keys, malformed templates. |
| E7 | Slots, hotkeys, anchors, layers, tokens: a missing `mods` slot or color synonym, a hotkey used twice in a layer or inside a loop, an anchor outside the map layer. |
| E8 | Package budget: at most 3 MB in all (fonts, pictures, scripts and the rest have their own limits). |
| E9 | Environment: Node or Chromium failed, a request left the local server, the font subsetter is missing. |

`tools/ui-convert/tests/bad/` holds one small skin per typical mistake with the code it must give
(`expect.txt`); they double as examples of each error.

### Required tools

- **Node** (on `PATH`, or `CIV5_NODE`) and the Playwright Chromium of `tools/playtest`: run `npm ci` in
  `tools/playtest` and in `tools/ui-convert/node` once. Only that pinned Chromium is used, with the flags in
  `tools/ui-convert/node/chromium-flags.json`, so the computed values are the same on every machine.
- **Python** with `fontTools` at the version in `tools/ui-convert/fonts/requirements.txt`
  (`python -m pip install --user -r tools/ui-convert/fonts/requirements.txt`); `CIV5_PYTHON` names another
  interpreter.
- The tests that need a browser run only with `CIV5_UI2_CHROME=1`:
  `cargo test -p ui2-convert --test e2e -- --test-threads=1`.

## Skin package

The converter's output is a single binary file, `<id>-<hash16>.ui2`: the skin's markup as node templates, its
styles already resolved by Chromium, color tokens, bindings, texts, scripts, font subsets and rasterized pictures.
The game reads it with the crate `ui2-package` and never needs the HTML, the CSS or a browser. The same format
carries the panels of a mod (see "Mod fragments" below).

### Format

- **Container.** The file starts with the magic `C5UI2PKG`, the format version (major and minor, 16 bits each)
  and a table of at most 32 sections: a four-letter tag, flags (bit 0: a reader that does not know the section
  must refuse the file), offset and length. Sections follow in ascending tag order, each starting at a multiple
  of 8 bytes, with zero bytes between them. The last 32 bytes are the blake3 hash of everything before them: it
  is the package's identity, and its first 16 hex digits are the `hash16` of the file name. A reader refuses files
  over 16 MiB.
- **Sections.**

  | Tag | Holds |
  |---|---|
  | `MANI` | kind (skin or mod fragment), id, version, title key, the profile, model and script API versions the skin was built for, the tools that built it, themes, languages, the faces of the font roles |
  | `STRS` | every string the other sections name, sorted, without repeats |
  | `TMPL` | node templates: template 0 is the document, the others are the bodies of loops, conditions and tooltips |
  | `STYL` | the viewport cells of the breakpoints, the style records (one field per property of the profile), and per node the record for every cell and combination of dynamic conditions |
  | `TOKN` | the root color tokens, their synonyms and the themes |
  | `BIND` | expressions, text templates, the bindings and events of each node, the model paths and intents the skin uses, the skin state with its defaults |
  | `L10N` | the text tables, one per language |
  | `FONT` | the font subsets (TTF or OTF bytes as the skin's characters need them) |
  | `IMGS` | atlas pages of the rasterized pictures (RGBA and alpha-only icon masks, at scale 1 and 2) and where each picture sits |
  | `SCRP` | the classic scripts in execution order and the model roots they read |
  | `AREF` | references to game assets (`portrait:...`, `render:minimap`), never the assets themselves |
  | `EXTN` | extension points, part names, screens, script islands, hotkeys, tooltips |
  | `SRCM` | the source map: file, line and column of every node, for error messages |

  Structural sections are serialized with `postcard`. Fonts and picture pages are zstd frames, decompressed once
  when the skin loads; scripts are stored as they are (the web server compresses them).
- **Determinism.** The same sources give the same bytes on every machine: no build time, no paths of the build
  machine, maps written in sorted order, numbers as Chrome computed them. The tool versions that could change the
  output (Chromium, its flags, the font subsetter) are recorded in `MANI`.
- **Loading.** The client checks the magic, the format version, the size, the hash and the section table, decodes
  every section and checks every index into another section; a damaged file is refused, never half used. Then it
  checks the versions (next section) and that every model path the skin reads exists in the client's model with
  the same type. A package that fails any check is not shown: the skin in use stays, and the reason is logged once.

### Versions

- **Format** (`major.minor` in the header, now 1.0). A new section is a minor change: a reader skips an unknown
  section unless its flag says it must understand it. Any change to the structure or meaning of an existing
  section is a major change.
- **Profile, model and script API** (in `MANI`). The client loads a package whose profile major equals its own,
  whose model major equals its own with a minor not newer than its own, and whose script API major equals its own.
  New model fields therefore do not break built skins; a removed or retyped field does, through the path check.
- Packages are never migrated. A skin built for an older format or profile is rebuilt from its sources with one
  command (`cargo xtask ui build <skin>`).

### Budget

A skin package holds at most 3 MB, counted in the file as written (after compression): fonts at most 1.2 MB,
pictures at most 0.8 MB, scripts at most 750 KB, everything else at most 0.5 MB. The converter refuses a package
over any limit (E8-001) and prints the size of every section with each build (also stored in `build.json`). Game
assets do not count: the package only names them. A mod fragment has its own, smaller budget (E8-002).

### Delivery

- **Web build.** `cargo xtask build --only web` (and the Docker context of `cargo xtask dist`) builds every skin
  under `skins/` and puts the packages into `ui2/` of the web bundle with an index, `ui2/skins.json`: one entry
  per skin with its id, file, hash, title key, themes, script API and whether it is the default skin. The content
  hash in the file name makes every package safe to cache forever.
- **Client.** The client reads the index, downloads the package of the chosen skin, checks that its hash is the
  one the index names, and runs the load checks above before it replaces the skin in use.
- **Development.** `cargo xtask ui watch <skin>` serves the package being edited on 127.0.0.1 (`cargo xtask dev
  --ui-dev <skin>` starts it next to the dev server, on port 8095); a client started with
  `?ui=v2:<skin>&ui2dev=<server>` asks for `/pkg/<id>.rev` twice a second and, when a new build appears, changes
  the running skin to it in place: the model, the open windows and the skin's `state.*` stay (a value stays when
  its path and type are still declared). A build that fails is shown as a notice; the running build stays.
- **Sample mode.** `?ui=v2:<skin>&ui2sample=<state>` runs the skin on its `sample/<state>.json` instead of a game
  (the web build copies the samples to `ui2/sample/<id>/`), with no server connection and no sign-in; the map
  anchors the sample places nowhere stand where the layout puts them, as on the skin's page in a browser.
  `&ui2bg=<rrggbb>` puts a solid background under it and stops the clock of its transitions, for reference images
  (a style that changes while it is stopped shows at once).
- **Skin state.** The paths of `[state]` are mounted as the model's `state.*` with their defaults when the skin
  loads: markup (`set`, `toggle`, controls) and scripts (`ui.state.*`) write them, bindings read them. A path the
  skin did not declare cannot be written. Changing the skin sets the state aside; it comes back when that skin is
  chosen again.
- **Mod fragments.** The panels of a mod are a package of the same format with the kind "fragment", built by
  `cargo xtask mods build` into `ui2/mods/<id>-<version>-<hash16>.ui2` and listed in `ui2/mods-ui.json` with the
  mod's id, version, file, hash, UI API requirement and panels. A fragment has no fonts, themes or text tables:
  its colors name the color tokens of the catalogue and its fonts name the font roles, and the client resolves
  both with the skin the fragment is mounted into. It ships with the client build, not with the game server.

<!-- profile: generated by `cargo xtask ui docs` from the profile file; edit profile.toml, not this section -->
## Profile 1.0

What a skin may use. This section is generated from the profile file `crates/shared/ui2-package/profile/profile.toml`, which the converter reads; a change to the profile is a change to that file, and `cargo xtask ui docs` writes this section again.

### Elements

| Group | Elements |
|---|---|
| document | `html`, `head`, `body`, `meta`, `title`, `style`, `template`, `link`, `script` |
| block | `div`, `section`, `header`, `footer`, `aside`, `nav`, `main`, `article`, `p`, `h1`, `h2`, `h3`, `h4`, `h5`, `h6`, `ul` (`list-style: none`), `ol` (`list-style: none`), `li` (`list-style: none`), `label`, `hr` |
| text | `span`, `b`, `strong`, `i`, `em`, `small` |
| control | `button` (`appearance: none`), `input` (type `text`; `appearance: none`) |
| image | `img`, `svg`, `use`, `symbol`, `defs` |

The notes in parentheses are what the element must have: the allowed `type`, and computed values the skin sets itself (the browser's own look of controls and lists is not part of the profile).

### Properties

Longhands only: Chrome bakes the shorthands of the list below into them. A property outside the table with a value other than its initial one is an error, wherever the value came from. Flags: *inherited*; *transition* (can be animated by `transition`); *bind* (can be set by `data-bind-style-<property>`); *token* (a color that may refer to a `:root` color token).

#### Layout

| Property | Values | Initial | Flags |
|---|---|---|---|
| `display` | one of `none`, `block`, `flex`, `grid`, `inline` | `inline` |  |
| `position` | one of `static`, `relative`, `absolute` | `static` |  |
| `z-index` | an integer, or `auto`; -9999..9999 | `auto` |  |
| `box-sizing` | one of `border-box`, `content-box` | `content-box` |  |
| `top` | a length, or `auto` | `auto` | bind |
| `right` | a length, or `auto` | `auto` | bind |
| `bottom` | a length, or `auto` | `auto` | bind |
| `left` | a length, or `auto` | `auto` | bind |
| `width` | a length, or `auto`; at least 0 | `auto` | bind |
| `height` | a length, or `auto`; at least 0 | `auto` | bind |
| `min-width` | a length, or `auto`; at least 0 | `auto` |  |
| `min-height` | a length, or `auto`; at least 0 | `auto` |  |
| `max-width` | a length, or `none`; at least 0 | `none` |  |
| `max-height` | a length, or `none`; at least 0 | `none` |  |
| `margin-top` | a length, or `auto` | `0px` |  |
| `margin-right` | a length, or `auto` | `0px` |  |
| `margin-bottom` | a length, or `auto` | `0px` |  |
| `margin-left` | a length, or `auto` | `0px` |  |
| `padding-top` | a length; at least 0 | `0px` |  |
| `padding-right` | a length; at least 0 | `0px` |  |
| `padding-bottom` | a length; at least 0 | `0px` |  |
| `padding-left` | a length; at least 0 | `0px` |  |
| `overflow-x` | one of `visible`, `hidden`, `clip`, `auto`, `scroll` | `visible` |  |
| `overflow-y` | one of `visible`, `hidden`, `clip`, `auto`, `scroll` | `visible` |  |
| `flex-direction` | one of `row`, `row-reverse`, `column`, `column-reverse` | `row` |  |
| `flex-wrap` | one of `nowrap`, `wrap` | `nowrap` |  |
| `flex-grow` | a number; at least 0 | `0` | bind |
| `flex-shrink` | a number; at least 0 | `1` |  |
| `flex-basis` | a length, or `auto`; at least 0 | `auto` |  |
| `order` | an integer | `0` |  |
| `justify-content` | one of `normal`, `flex-start`, `flex-end`, `start`, `end`, `center`, `space-between`, `space-around`, `space-evenly` | `normal` |  |
| `align-items` | one of `normal`, `stretch`, `flex-start`, `flex-end`, `start`, `end`, `center`, `baseline` | `normal` |  |
| `align-self` | one of `auto`, `normal`, `stretch`, `flex-start`, `flex-end`, `start`, `end`, `center`, `baseline` | `auto` |  |
| `align-content` | one of `normal`, `flex-start`, `flex-end`, `start`, `end`, `center`, `space-between`, `space-around`, `space-evenly` | `normal` |  |
| `row-gap` | a length, or `normal`; at least 0 | `normal` |  |
| `column-gap` | a length, or `normal`; at least 0 | `normal` |  |

#### Decoration

| Property | Values | Initial | Flags |
|---|---|---|---|
| `color` | a color | `rgb(0, 0, 0)` | inherited, transition, bind, token |
| `background-color` | a color | `rgba(0, 0, 0, 0)` | transition, bind, token |
| `opacity` | a number; 0..1 | `1` | transition, bind |
| `visibility` | one of `visible`, `hidden` | `visible` | inherited, transition |
| `border-top-width` | a length in px; at least 0 | `0px` |  |
| `border-right-width` | a length in px; at least 0 | `0px` |  |
| `border-bottom-width` | a length in px; at least 0 | `0px` |  |
| `border-left-width` | a length in px; at least 0 | `0px` |  |
| `border-top-style` | one of `none`, `solid` | `none` |  |
| `border-right-style` | one of `none`, `solid` | `none` |  |
| `border-bottom-style` | one of `none`, `solid` | `none` |  |
| `border-left-style` | one of `none`, `solid` | `none` |  |
| `border-top-color` | a color | `rgb(0, 0, 0)` | transition, token |
| `border-right-color` | a color | `rgb(0, 0, 0)` | transition, token |
| `border-bottom-color` | a color | `rgb(0, 0, 0)` | transition, token |
| `border-left-color` | a color | `rgb(0, 0, 0)` | transition, token |
| `border-top-left-radius` | one or two lengths; at least 0 | `0px` |  |
| `border-top-right-radius` | one or two lengths; at least 0 | `0px` |  |
| `border-bottom-right-radius` | one or two lengths; at least 0 | `0px` |  |
| `border-bottom-left-radius` | one or two lengths; at least 0 | `0px` |  |
| `box-shadow` | a list of shadows; at most 4 items | `none` | transition, token |
| `text-shadow` | a list of shadows; one item | `none` | inherited, token |
| `transform` | translate, rotate and scale functions; `calc()` | `none` | transition, bind |
| `transform-origin` | two lengths or side keywords; `calc()` | `50% 50%` |  |
| `cursor` | one of `auto`, `default`, `pointer`, `text`, `move`, `grab`, `grabbing`, `not-allowed` | `auto` | inherited |
| `pointer-events` | one of `auto`, `none` | `auto` | inherited |

#### Transitions

| Property | Values | Initial | Flags |
|---|---|---|---|
| `transition-property` | `all`, `none` or animatable properties | `all` |  |
| `transition-duration` | a list of times (s, ms); 0..1000 | `0s` |  |
| `transition-delay` | a list of times (s, ms); 0..1000 | `0s` |  |
| `transition-timing-function` | a list of timing functions | `ease` |  |

#### Text

| Property | Values | Initial | Flags |
|---|---|---|---|
| `font-family` | a list of font families | `serif` | inherited |
| `font-size` | a length in px; at least 0 | `16px` | inherited |
| `font-weight` | an integer; 100..900 | `400` | inherited |
| `font-style` | one of `normal`, `italic` | `normal` | inherited |
| `font-variant-numeric` | `normal`, or any of `tabular-nums`, `proportional-nums`, `lining-nums`, `oldstyle-nums` together | `normal` | inherited |
| `line-height` | `normal`, a length or a number; at least 0 | `normal` | inherited |
| `letter-spacing` | a length in px, or `normal` | `normal` | inherited |
| `word-spacing` | a length in px, or `normal` | `normal` | inherited |
| `text-align` | one of `start`, `end`, `left`, `right`, `center` | `start` | inherited |
| `white-space-collapse` | one of `collapse`, `preserve` | `collapse` | inherited |
| `text-wrap-mode` | one of `wrap`, `nowrap` | `wrap` | inherited |
| `text-overflow` | one of `clip`, `ellipsis` | `clip` |  |
| `overflow-wrap` | one of `normal`, `break-word`, `anywhere` | `normal` | inherited |
| `word-break` | one of `normal`, `break-word` | `normal` | inherited |
| `text-transform` | one of `none` | `none` | inherited |
| `text-decoration-line` | `none`, or any of `underline` together | `none` |  |

#### Icons (svg elements only)

| Property | Values | Initial | Flags |
|---|---|---|---|
| `fill` | a color, or `none` | `rgb(0, 0, 0)` | inherited, transition, token |
| `stroke` | a color, or `none` | `none` | inherited, transition, token |
| `stroke-width` | a length in px; at least 0 | `1px` | inherited |
| `stroke-linecap` | one of `butt`, `round`, `square` | `butt` | inherited |
| `stroke-linejoin` | one of `miter`, `round`, `bevel` | `miter` | inherited |

#### Background layers and outline (decoration)

| Property | Values | Initial | Flags |
|---|---|---|---|
| `background-image` | a list of gradients and `url()` pictures; at most 4 items; `calc()` | `none` | token |
| `background-size` | a list of `auto`, `cover`, `contain` or sizes; at least 0; at most 4 items; `calc()` | `auto` | bind |
| `background-position-x` | a list of lengths; at most 4 items; `calc()` | `0%` |  |
| `background-position-y` | a list of lengths; at most 4 items; `calc()` | `0%` |  |
| `background-repeat` | a list of repeat values; at most 4 items | `repeat` |  |
| `background-origin` | a list, each one of `border-box`, `padding-box`, `content-box`; at most 4 items | `padding-box` |  |
| `background-clip` | a list, each one of `border-box`, `padding-box`, `content-box`; at most 4 items | `border-box` |  |
| `outline-style` | one of `none`, `solid` | `none` |  |
| `outline-width` | a length in px; at least 0 | `3px` |  |
| `outline-color` | a color | `rgb(0, 0, 0)` | transition, token |
| `outline-offset` | a length in px | `0px` |  |

#### Layout, second part

| Property | Values | Initial | Flags |
|---|---|---|---|
| `justify-items` | one of `normal`, `stretch`, `flex-start`, `flex-end`, `start`, `end`, `center`, `baseline` | `normal` |  |
| `justify-self` | one of `auto`, `normal`, `stretch`, `flex-start`, `flex-end`, `start`, `end`, `center`, `baseline` | `auto` |  |
| `aspect-ratio` | `auto` or `w / h` | `auto` |  |
| `scrollbar-width` | one of `auto`, `none` | `auto` |  |
| `grid-template-columns` | `none` or a track list; at least 0 | `none` |  |
| `grid-template-rows` | `none` or a track list; at least 0 | `none` |  |
| `grid-auto-columns` | a list of track sizes; at least 0 | `auto` |  |
| `grid-auto-rows` | a list of track sizes; at least 0 | `auto` |  |
| `grid-auto-flow` | one of `row`, `column`, `dense`, `column dense` | `row` |  |
| `grid-row-start` | `auto`, a line number or `span N` | `auto` |  |
| `grid-row-end` | `auto`, a line number or `span N` | `auto` |  |
| `grid-column-start` | `auto`, a line number or `span N` | `auto` |  |
| `grid-column-end` | `auto`, a line number or `span N` | `auto` |  |

#### 9-slice frames (border-image) and clipping

| Property | Values | Initial | Flags |
|---|---|---|---|
| `border-image-source` | `url()` of a picture, or `none` | `none` |  |
| `border-image-slice` | one to four numbers or percentages, and `fill` | `100%` |  |
| `border-image-width` | one to four px lengths or numbers | `1` |  |
| `border-image-repeat` | one or two of `stretch`, `repeat`, `round` | `stretch` |  |
| `border-image-outset` | a length in px; only 0 | `0` |  |
| `clip-path` | `none` or a convex `polygon()`; at most 8 items; `calc()` | `none` |  |

#### Text, second part

| Property | Values | Initial | Flags |
|---|---|---|---|
| `text-indent` | a length in px; only 0 | `0px` | inherited |
| `text-wrap-style` | one of `auto` | `auto` | inherited |
| `vertical-align` | one of `baseline` | `baseline` |  |
| `direction` | one of `ltr` | `ltr` | inherited |
| `writing-mode` | one of `horizontal-tb` | `horizontal-tb` | inherited |

Shorthands a stylesheet may write: `margin`, `padding`, `inset`, `flex`, `flex-flow`, `gap`, `grid-gap`, `place-items`, `place-content`, `place-self`, `border`, `border-top`, `border-right`, `border-bottom`, `border-left`, `border-width`, `border-style`, `border-color`, `border-radius`, `background`, `background-position`, `border-image`, `grid-template`, `grid-row`, `grid-column`, `grid-area`, `outline`, `font`, `font-variant`, `overflow`, `transition`, `white-space`, `text-wrap`, `text-decoration`, `word-wrap`.

### Ignored properties

Accepted and without effect.

| Property | Why |
|---|---|
| `-webkit-font-smoothing` | the platform decides smoothing; it does not change layout |
| `-webkit-tap-highlight-color` | no tap highlight in the runtime |
| `caret-color` | the runtime draws its own caret |
| `font-kerning` | kerning is always on |
| `font-optical-sizing` | variable fonts are not part of the profile |
| `text-rendering` | the text stack always uses its own shaping and rasterizer |
| `user-select` | the runtime has no text selection |
| `will-change` | a hint for the browser compositor |

### Selectors

Tags, classes, ids and `:root`, joined by a space or `>`; the dynamic pseudo-classes `:hover`, `:active`, `:focus`, `:focus-visible`, `:disabled`. `@media` takes width and height in px; the only other at-rule is `@font-face`.

### Extension points and color synonyms

Every skin has the extension points `mods` (`data-slot`) and defines on `:root` the color synonyms mods read, each as `var(--<root token>)`:

`--ui-accent`, `--ui-accent-ink`, `--ui-line`, `--ui-negative`, `--ui-surface`, `--ui-surface-2`, `--ui-text`, `--ui-text-2`, `--ui-text-3`, `--ui-y-cult`, `--ui-y-faith`, `--ui-y-food`, `--ui-y-gold`, `--ui-y-happy`, `--ui-y-prod`, `--ui-y-sci`, `--ui-y-tour`.

### Error codes

Every code the converter and the profile's parsers report, with the text it prints.

| Code | What | Hint |
|---|---|---|
| E1-001 | skin.toml is malformed | check the file against the skin.toml reference: [skin] id, version, title, entry, model, api; [fonts], [state], [themes], [scripts], [verify] |
| E1-002 | element outside the profile | use the elements of the profile reference; links, tables, forms and media elements have no place in a skin |
| E1-003 | resource outside the skin | refer to files inside the skin folder with a relative path; nothing is loaded from the network |
| E1-004 | inline or module script | put the code into a classic script file listed in skin.toml [scripts] files |
| E1-005 | id used twice | an id names one element; use a class for several |
| E1-006 | the element lacks a computed value the profile requires | set the value the profile lists for this element (appearance: none for controls, list-style: none for lists) |
| E1-007 | import or export in a skin script | skin scripts are classic scripts in the order of skin.toml [scripts] files; share code through globals of the script context |
| E1-008 | browser API in a skin script | a skin script talks to the game only through the ui object; it has no document, window, network or storage |
| E1-009 | included file not found | data-include names a file of the skin folder, relative to the skin root |
| E1-020 | title attribute | use data-tooltip with the id of a tooltip template |
| E1-021 | inline event attribute | use data-on-&lt;event&gt;="intent ..." or "call ..." |
| E1-022 | data attribute outside the profile | check the spelling against the list of data-* attributes in the profile reference |
| E1-023 | srcset attribute | put the 2x picture next to the 1x file as &lt;name&gt;@2x.&lt;ext&gt; |
| E1-024 | attribute outside the profile | remove it, or use the profile's own attribute for the same purpose |
| E1-025 | attribute not allowed on this element | see which elements take the attribute in the profile reference |
| E1-026 | attribute written twice | keep one of them |
| E1-027 | include path outside the skin | write a relative path with / separators that does not leave the skin folder |
| E1-028 | attribute value outside the profile | use one of the values the profile allows for this element |
| E2-002 | value outside the profile | use one of the values the profile table lists for this property |
| E2-003 | unit other than px | viewport and container units would be baked into px at one size; use % or px, or a breakpoint |
| E2-004 | number is not finite | NaN and infinity cannot be stored; check the calc() expression and its variables |
| E2-005 | value out of range | keep the number inside the range the profile table gives |
| E2-006 | color form outside the profile | write the color as #hex, rgb(), rgba(), a color name or transparent; currentcolor and color functions are resolved by Chrome first |
| E2-007 | calc() outside decoration properties | layout cannot use calc(); use flex, grid or a percentage |
| E2-008 | list too long | shorten the list to the number the profile table allows |
| E2-010 | property outside the profile | remove the declaration or use a property of the profile reference; layout is block, flex, grid and absolute |
| E2-011 | computed value cannot be stored | Chrome computed a value of another kind than the property's field holds; simplify the declaration |
| E2-012 | icon paint on an element that is not an icon | fill and stroke style only svg icons |
| E2-013 | page background is not transparent | the world is drawn under the UI; give html and body no background (the preview backdrop is not part of the skin) |
| E2-014 | scroll container without scrollbar-width: none | add scrollbar-width: none next to overflow: auto or scroll; the runtime draws no scrollbar, so Chrome's bar would change the layout |
| E2-020 | grid feature outside the profile | write tracks as px, %, fr, auto, min-content, max-content, minmax() or repeat(&lt;count&gt;, ...); place items by line numbers or span &lt;count&gt;; grid lines and areas have no names |
| E2-021 | clip shape outside the profile | use clip-path: none or a polygon() of three to eight points |
| E2-022 | clip polygon is not convex | the clip is drawn as one convex polygon; list the points in order around the shape, or split the shape into nested elements |
| E2-023 | picture reference outside the profile | refer to a png, webp or svg file inside the skin (a border image is png or webp), without a query or a data: URL |
| E3-001 | at-rule outside the profile | a skin may use @media (width and height) and @font-face only |
| E3-002 | selector outside the profile | use tags, classes, ids, :root, the combinators space and &gt;, and :hover :active :focus :focus-visible :disabled; give the element a class instead of a structural or sibling selector |
| E3-003 | a toggled class is also written in the static class attribute | a class that data-bind-class-toggle switches must not be in class="..." of the same element |
| E3-004 | media query outside the profile | write @media with width and height in px: min-, max- or ranges joined by and, type all or screen |
| E3-005 | more than four conditions on one node | a node can depend on at most four dynamic conditions (hover, focus, toggled classes of it and its ancestors); move a condition to a child or merge classes |
| E3-006 | too many breakpoints | a skin has at most 8 media conditions and at most 16 viewport cells |
| E3-007 | transitions differ between the variants of a node | declare transition on the base rule only; a state may change values, not transitions |
| E3-008 | display: contents changes with a condition or a breakpoint | an element is either a box or not in every state; restructure the markup |
| E3-009 | order changes with a condition or a breakpoint | the order of children is fixed when the skin is built; keep order the same in every state and cell |
| E3-010 | CSS syntax error | fix the stylesheet so Chrome and the converter read the same rules |
| E4-001 | syntax error in an expression | check the expression against the grammar: literals, paths, operators, calls, ?: and parentheses; comparisons cannot be chained |
| E4-002 | unknown function | the functions are fmt.int fmt.signed fmt.fixed fmt.pct fmt.mmss loc len min max clamp round floor ceil abs |
| E4-003 | wrong number of arguments | see the argument counts of the function in the profile reference |
| E4-004 | unknown path | a path starts at a model root, a loop variable or state; check the name against the model schema |
| E4-005 | arithmetic over something that is not a number | only int and float values can be added, multiplied or passed to numeric functions; write text with a {{ }} insert |
| E4-006 | values of different types cannot be compared | compare numbers with numbers, strings with strings or enum values, keys of one kind; null can be compared with anything |
| E4-007 | string is not a value of the enum | use one of the values the schema lists for this enum |
| E4-008 | a bool, a list or a record cannot be written into text | use a condition (data-bind-if, class-toggle) or write a field of the record |
| E4-009 | data-bind-for needs a list | loop over a path whose type is a list |
| E4-010 | the loop variable name is taken | use a name that is not a model root, a keyword (state, mods, true, false, null) or an enclosing variable |
| E4-011 | argument of the wrong type | see the argument types of the function in the profile reference |
| E4-012 | a list has no fields | loop over the list with data-bind-for and read the field of the item |
| E4-013 | an insert is not closed | every {{ needs a }}; a string inside the insert may contain }} only between its quotes |
| E4-020 | the value must be exactly one {{ }} insert | write the whole attribute as {{ expression }} |
| E4-021 | malformed data-bind-for | write [index,] item:{{ list }}, for example c:{{ top.cities }} |
| E4-022 | the list has no key | add data-bind-key="{{ item.field }}" with a unique value per item |
| E4-023 | attribute without the attribute it belongs to | add the missing attribute on the same element, or remove this one |
| E4-024 | malformed data-bind-class-toggle | write class:{{ condition }}, separated by ; for several classes |
| E4-025 | property cannot be bound | only properties marked bind in the profile table can be set by data-bind-style-&lt;property&gt; |
| E4-026 | malformed action | an action is intent name(args), set state.x = expr, toggle state.x, call name(args) or stop; separate several with ; |
| E4-027 | unknown intent | intents come from the catalogue of the model schema |
| E4-028 | wrong intent arguments | pass every argument of the intent, all by name or all by position |
| E4-029 | value of the wrong type | the value must have the type the target declares |
| E4-030 | path is read-only for markup | markup writes only state.* and ui.* fields the schema marks writable |
| E4-031 | intent of another mod | a mod fragment sends intents of the catalogue and mods.&lt;its own id&gt;.&lt;name&gt; |
| E4-032 | malformed data-bind-model | write the path of a skin state value, for example state.volume |
| E4-033 | malformed data-control | a control is checkbox, toggle, radio, slider or select with data-bind-model and the attributes of its kind |
| E4-034 | data-event next to data-on-click | put the call into data-on-click instead |
| E4-035 | binding inside a script island | an island is driven by script commands; move the binding outside the island |
| E4-036 | conflicting sources of text | an element has one source of text (data-bind-value or data-l10n) and then no child elements |
| E4-037 | data-bind-if and data-bind-for on one node | put the condition on a parent element or use data-bind-filter |
| E4-040 | sample does not fit the model schema | write the sample as the schema describes the roots; a field the schema does not have is an error |
| E4-041 | sample state value is not in the skin state | declare the value in skin.toml [state] or remove it from the sample |
| E4-042 | skin state value of an unsupported type | a state value is a bool, an integer, a float or a string |
| E5-001 | SVG content outside the profile | an inline svg is a static picture: no text, script or foreignObject; write text as HTML |
| E5-002 | icon reference outside the sprite | an icon is &lt;use href="#id"&gt; of a symbol in the skin's sprite |
| E5-003 | font face without a skin font file | point src: url() at a path listed in skin.toml [fonts.files], which maps it to a file under third_party/fonts |
| E5-004 | font face not loaded | Chromium could not load the subset of the face; check the file and that the face is a TTF or OTF |
| E5-005 | picture cannot be rasterized | pictures are png, webp or svg files inside the skin with a size from CSS |
| E5-006 | character missing from the skin fonts | the runtime has no fallback fonts; use a font of skin.toml [fonts.files] that has the character, or change the text |
| E5-007 | game asset without a size | give the element of data-asset or data-bind-src a width and a height in CSS; the asset's own size is not known when the skin is built |
| E5-008 | font face whose line height depends on the platform | set the USE_TYPO_METRICS bit (OS/2 fsSelection bit 7) of the font, or give it the same ascent and descent in hhea, OS/2 typo and OS/2 win with no line gap; otherwise Chrome on Windows and the game lay the text out at different heights |
| E5-009 | font of a mod panel that is not a font role of the skin | a mod brings no fonts: write font-family: ui-sans or ui-display (the skin's faces for those roles) on every element that shows text, and no @font-face |
| E5-010 | malformed game asset reference | write &lt;namespace&gt;:&lt;id&gt;, for example render:minimap |
| E6-001 | untranslated text | put the text into the skin's l10n tables and show it with data-l10n, or bind it with data-bind-value |
| E6-002 | text key not found | add the key to l10n/&lt;lang&gt;.toml of every language of the skin |
| E6-003 | malformed text table or template | an l10n file is TOML with "key" = "template"; a template uses {1}, {1_Num}, {1: plural ...} like the game's texts |
| E6-004 | the template uses more arguments than the node passes | pass every {n} of the template in data-l10n-args |
| E6-010 | malformed text key | write lower case words separated by dots, for example top.turn |
| E7-001 | the body does not hold the four layer roots | the children of body are the layer roots data-layer=map, hud, window and tooltip, in this order |
| E7-002 | required extension point missing | add data-slot with this name on a flex or grid element of the skin |
| E7-003 | required color token synonym missing | define the synonym on :root as var(--&lt;root token&gt;) |
| E7-004 | the same hotkey twice in one layer | give each node of a layer its own key |
| E7-005 | the same part name twice | a part name names one element of the skin |
| E7-006 | tooltip template not found | data-tooltip is the id of a &lt;template data-layer="tooltip"&gt; of the skin |
| E7-007 | layer root is not a full-size box without pointer events | a layer root is position:absolute; inset:0; pointer-events:none, and its panels switch pointer events on |
| E7-008 | mod panel outside the rules of a fragment | a panel page holds one root element (and the &lt;link&gt; of its stylesheets); the panel pages of a mod sit in one folder; a mod adds no layers, screens or extension points; it reads the --ui-* color tokens of the catalogue and does not define them |
| E7-009 | class of a mod panel without the mod's prefix | every class a mod panel writes, toggles or selects starts with m-&lt;mod id&gt;-, so it never meets a class of the skin |
| E7-010 | malformed slot or part name | use lower case words, digits and - separated by dots, for example top.right |
| E7-011 | malformed layer or screen | layers are map, hud, window and tooltip; a screen is a child of the window layer |
| E7-012 | malformed or misplaced anchor | write city:{{ id }}, unit:{{ id }} or plot:{{ id }} on the node with data-bind-for in the map layer |
| E7-013 | malformed hotkey | write [Ctrl+][Alt+][Shift+]&lt;KeyboardEvent.code&gt;, for example Ctrl+KeyB or F5 |
| E7-014 | hotkey inside a loop | a hotkey needs a node that exists once; move it out of the data-bind-for body |
| E7-015 | slot on a node with if or for | put data-slot on a plain parent element |
| E7-016 | malformed tooltip reference | data-tooltip is the id of a &lt;template data-layer="tooltip"&gt; |
| E7-017 | malformed data-fit | write data-fit="shrink" and optionally data-fit-min="12px" |
| E7-018 | malformed script island | an island has an id; keys and classes are plain words |
| E8-001 | package budget exceeded | the package holds at most 3 MB: fonts 1.2 MB, pictures 0.8 MB, scripts 750 KB, the rest 0.5 MB; use fewer or smaller pictures, fewer faces or a smaller subset |
| E8-002 | mod fragment budget exceeded | a mod's fragment holds at most 1 MB, and one picture at most 256 KiB of pixels at both scales together; use fewer or smaller pictures |
| E9-001 | request outside the local server | a skin loads nothing from the network or from other servers; refer to files inside the skin folder |
| E9-002 | Node or Chromium failed | run npm ci in tools/playtest and tools/ui-convert/node; check that node is on PATH |
| E9-003 | bad request on the converter protocol | a converter bug: the Node side refused a request |
| E9-004 | skin folder or entry file not found | pass the skin id (skins/&lt;id&gt;) or a folder with skin.toml and the entry file |
| E9-005 | preview script missing | includes and bindings are expanded by node/preview/preview.js; without it the page shows only the entry file |
| E9-006 | font subsetter missing | install it: python -m pip install --user -r tools/ui-convert/fonts/requirements.txt (or point CIV5_PYTHON at a Python that has it) |
| E9-007 | package not written | a converter bug or a file system problem: the package builder refused the data or the output folder is not writable |

<!-- profile: end -->

## Mount points and tokens for mod panels

A mod's panels (see [Mods](modding.md#panels-in-ui-v2)) attach to a point by its id, never by the classes of a skin.
The catalogue is `mod_api::UI_CATALOG`; its own version, the UI API, is 1.0. A minor version only adds points and
tokens. Renaming or removing one is a major version.

| Point | Kind | Holds |
|---|---|---|
| `mods` | stack | The shared slot. Every skin must have it; panels whose point a skin lacks land here |
| `hud.top.extra` | stack | The free space of the top bar |
| `hud.menu` | stack | The menu group of the top bar; panels here should be buttons |
| `hud.right` | stack | The column under the notifications |
| `hud.unit.actions` | stack | The orders of the selected unit; panels here should be buttons |
| `window.production.detail` | stack | The detail side of the production window |
| `hud.minimap` | single | The minimap; a panel may replace it |
| `hud.chat` | single | The chat; a panel may replace it |

A skin marks a point with `data-slot="<id>"` on one element, as it does `mods`. A skin may leave out any point
except `mods`. A stack takes the panels after the element's own children, in load order of the mods. A single point is
one element of the skin that a panel may replace (`mode = "replace"`, one mod per point). A panel for a point the
skin lacks goes to `mods`, or is dropped when the panel says `fallback = "none"`.

The color tokens a panel may read as `var(--ui-*)` are the 17 color synonyms every skin defines (the list under
"Extension points and color synonyms" above): `--ui-surface`, `--ui-surface-2`, `--ui-line`, `--ui-text`,
`--ui-text-2`, `--ui-text-3`, `--ui-accent`, `--ui-accent-ink`, `--ui-negative`, `--ui-y-food`, `--ui-y-prod`,
`--ui-y-gold`, `--ui-y-sci`, `--ui-y-cult`, `--ui-y-faith`, `--ui-y-happy` and `--ui-y-tour`. A panel reads them and
never defines them. Its text uses the font roles `ui-sans` and `ui-display`.
