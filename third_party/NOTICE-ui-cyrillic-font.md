# tools/asset-pipeline: Cyrillic glyphs for the Tw Cen MT atlases

SIMPLIFIED(SIMP-exe-716): the game's Tw Cen MT bitmap fonts have no Cyrillic. The asset pipeline
(`tools/asset-pipeline/src/ui/cyrillic.rs`) rasterises the missing Cyrillic letters from Ubuntu Light
(`tools/asset-pipeline/fonts/Ubuntu-Light.ttf`, Copyright Canonical Ltd., Ubuntu Font License 1.0, text in
`tools/asset-pipeline/fonts/UFL.txt`) and appends them to every Tw Cen MT atlas and `.ggxml` of the `ui` set.
The same file is described in `NOTICE-egui-fonts.md`. The rasterised glyphs are part of the generated asset
set only, never of the source tree.
