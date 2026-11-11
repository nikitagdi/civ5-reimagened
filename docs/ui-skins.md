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
`crates/shared/ui2-package/profile/profile.toml`, which the converter reads; the section "Profile 1.0" below is
generated from it.

### Commands

| Command | What it does |
|---|---|
| `cargo xtask ui check <skin>` | Runs every check (both Chromium passes, font subsets, pictures, budget) and writes no package. Exit code 1 with errors. `--dump-ir <file>` stores the scan for tests that run without a browser. |
| `cargo xtask ui build <skin>` | The same, then writes `target/ui2/<id>/<id>-<hash16>.ui2`, `build.json` (section sizes and budget), `errors.json` and `ir.json`. Two builds of the same sources give the same bytes. |
| `cargo xtask ui verify <skin> [--state <s>]` | Lays the package out with the game's own runtime and compares every element's rectangle with Chromium's at 1280x720, 1920x1200, 3440x1440 and 5120x1440; a difference over 0.5 px fails. The report lists the worst elements with their source place. |
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
