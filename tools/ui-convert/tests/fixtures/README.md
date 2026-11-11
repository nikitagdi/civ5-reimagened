# Converter fixtures

Stored runs of the converter for the tests that need no browser. Each `*.ir.json` is a dump
(`ui2-convert check <skin> --dump-ir <file>`): the page scan, the style plan and Chrome's answer
to it. A test runs the later stages on the dump; when the plan changes, the stored answer no
longer fits and the test says so (dump the skin again).

| File | Skin | How it was made |
|---|---|---|
| `top-strip.ir.json` | `skins/engraved` | The skin with its `data-include` elements replaced by the included files (the preview script, which expands includes in the page, was not available yet), then `check --dump-ir`. Source positions point into that combined `index.html`. Once the preview script exists: `cargo xtask ui check engraved --dump-ir tools/ui-convert/tests/fixtures/top-strip.ir.json`. |
| `layout.ir.json` | `layout/` | `cargo xtask ui check tools/ui-convert/tests/fixtures/layout --dump-ir tools/ui-convert/tests/fixtures/layout.ir.json`: anonymous boxes, `display: contents`, `order`. |

The bad mockups under `tests/bad/` carry their dumps as `ir.json`; `tests/e2e.rs`
(`CIV5_UI2_CHROME=1`) checks that those are current.
