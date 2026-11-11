# Converter fixtures

Stored runs of the converter for the tests that need no browser. Each `*.ir.json` is a dump
(`ui2-convert check <skin> --dump-ir <file>`): the page scan, the style plan and Chrome's answer
to it. A test runs the later stages on the dump; when the plan changes, the stored answer no
longer fits and the test says so (dump the skin again).

| File | Skin | How it was made |
|---|---|---|
| `top-strip.ir.json` | `skins/engraved` | `cargo xtask ui check engraved --dump-ir tools/ui-convert/tests/fixtures/top-strip.ir.json`: the page as the preview script expands it in extract mode (includes in place, every `if` drawn, one instance per `for`, node marks), sample state `hud`, the font subsets served to the page (`check` runs the build's two passes). Source positions point into the skin's own files (`index.html`, `parts/top.html`). |
| `layout.ir.json` | `layout/` | `cargo xtask ui check tools/ui-convert/tests/fixtures/layout --dump-ir tools/ui-convert/tests/fixtures/layout.ir.json`: anonymous boxes, `display: contents`, `order`. |

The bad mockups under `tests/bad/` carry their dumps as `ir.json`; `tests/e2e.rs`
(`CIV5_UI2_CHROME=1`) checks that those are current.
