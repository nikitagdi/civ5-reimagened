# UI skins

A UI skin is a folder of plain HTML, CSS and a few classic scripts under `skins/<id>/`. It opens in an
ordinary browser as a static mockup, and the skin converter turns it into a package (`<id>-<hash16>.ui2`) that
the game client lays out and draws itself, without a browser engine. The first skin is `skins/engraved`.

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
`crates/shared/ui2-package/profile/profile.toml`, which the converter reads.

### Commands

| Command | What it does |
|---|---|
| `cargo xtask ui check <skin>` | Runs every check (both Chromium passes, font subsets, pictures, budget) and writes no package. Exit code 1 with errors. `--dump-ir <file>` stores the scan for tests that run without a browser. |
| `cargo xtask ui build <skin>` | The same, then writes `target/ui2/<id>/<id>-<hash16>.ui2`, `build.json` (section sizes and budget), `errors.json` and `ir.json`. Two builds of the same sources give the same bytes. |
| `cargo xtask ui verify <skin> [--state <s>]` | Lays the package out with the game's own runtime and compares every element's rectangle with Chromium's at 1280x720, 1920x1200, 3440x1440 and 5120x1440; a difference over 0.5 px fails. The report lists the worst elements with their source place. |
| `cargo xtask ui watch <skin> [--port 8095]` | Keeps Chromium and a local package server running (127.0.0.1 only) and rebuilds on every saved change. The client polls `/pkg/<id>.rev` when started with `?ui2dev=http://127.0.0.1:8095` and reloads the package; a broken edit keeps the last good package and serves the errors at `/pkg/<id>.errors.json`. |
| `cargo xtask build --only web` | The web bundle; it builds every skin under `skins/` into `target/web/ui2/` with the index `ui2/skins.json`. |
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
