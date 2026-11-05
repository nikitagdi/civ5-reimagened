# crates/client/interim-ui: bundled fonts (egui `default_fonts`)

SIMPLIFIED(SIMP-001): the interim UI uses egui with its built-in fonts. They come with the
`epaint_default_fonts` 0.37.0 crate (a dependency of `epaint`, which `egui` uses) and are compiled
into the client binary. The crate's own license is `(MIT OR Apache-2.0) AND OFL-1.1 AND
Ubuntu-font-1.0`. The full license texts are in the crate's `fonts/` directory (and in
https://github.com/emilk/egui/tree/main/crates/epaint_default_fonts/fonts).

| Font | Used for | Copyright | License |
|---|---|---|---|
| Ubuntu-Light | proportional text | Copyright Canonical Ltd. | Ubuntu Font License 1.0 (`UFL.txt`) |
| Hack-Regular | monospace text | Copyright 2018 Source Foundry Authors; based on DejaVu (public domain) and Bitstream Vera Sans Mono, Copyright 2003 Bitstream Inc. (Bitstream Vera License, reserved font names "Bitstream" and "Vera") | MIT (`Hack-Regular.txt`) |
| NotoEmoji-Regular | emoji fallback | Copyright The Noto Project Authors | SIL Open Font License 1.1 (`OFL.txt`) |
| emoji-icon-font | emoji and logo fallback | Copyright (c) 2014 John Slegers | MIT (`emoji-icon-font-mit-license.txt`) |
| egui-icons | five logo glyphs | subset of emoji-icon-font | same as emoji-icon-font |

The code of `egui`, `egui-wgpu`, `epaint`, `emath` and `ecolor` is `MIT OR Apache-2.0`.

The original game UI uses the game's own fonts, which are not part of this repository.
