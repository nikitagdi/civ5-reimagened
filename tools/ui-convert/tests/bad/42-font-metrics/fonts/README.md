# Metrics probe font

`MetricsProbe.ttf` is a test font derived from Fira Sans Regular (`third_party/fonts/fira-sans`, SIL Open Font
License 1.1, text in `OFL.txt` here). It keeps only the glyphs of "Test skin", is renamed to "Metrics Probe", and
has its vertical metrics changed on purpose: hhea 1100/-400/0, OS/2 typo 935/-265/0 (as Fira Sans), OS/2 win
1150/420, and the USE_TYPO_METRICS bit cleared. The converter must refuse such a face (E5-008): its line height
differs between Chrome on Windows (win metrics) and the game (hhea).

Made with fontTools: subset to the text, then set `hhea.ascent/descent/lineGap`, `OS/2.usWinAscent/usWinDescent`,
clear bit 7 of `OS/2.fsSelection`, and rename the name records 1, 3, 4, 6 and 16.
