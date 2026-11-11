# Chrome reference data for `ui2-text`

`chrome-text.json` holds text measurements dumped from headless Chromium 148 (Windows) for the UI v2 mockup
text, laid out with the vendored fonts in `third_party/fonts` (Fira Sans 400/500, Alegreya 400/500). Layout in
Chrome does not depend on the device pixel ratio or on the font rendering flags; the numbers are the same at DPR 1
and 2.

- `runs`: single-line runs (601). `s` text, `family`, `weight`, `size` (px), `numeric` (computed
  `font-variant-numeric`), `letter_spacing` (px), `width` (px, from `Range.getClientRects()`). Runs from the
  synthetic page also carry `line_height` (`"normal"` or px), `box_h` (line box height), `baseline` (px from the
  line top) and `content_h` (inline box height).
- `paragraphs`: 30 wrapped paragraphs of 48 words. `max_width` (px) of the block, `line_height`, `line_starts`
  (byte offsets of the first character of every line), `line_widths` (px, one rect per line).

Chrome's rects of a wrapped line exclude the trailing space (all 288 lines).
