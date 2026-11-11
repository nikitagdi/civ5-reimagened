"""Writes the probe font of the text-probe fixture: a copy of Fira Sans Regular whose vertical metrics disagree on
purpose. Run by the `text_probe` test (tests/e2e.rs) before the fixture is built:

    python -E make_fonts.py <FiraSans-Regular.ttf> <out dir>

ProbeTypo.ttf  family "Probe Typo": hhea 1100/-400/0, OS/2 typo 935/-265/0 (as Fira), win 1150/420, with
               USE_TYPO_METRICS on: every text stack should take the typo metrics.

The same face with USE_TYPO_METRICS off is refused by the converter (E5-008, the bad mockup 42-font-metrics): its
line height depends on the platform.

The font is derived from an OFL font and renamed; it is generated, never committed.
"""

import sys

from fontTools.ttLib import TTFont


def probe(src, dst, family, use_typo):
    f = TTFont(src, recalcTimestamp=False)
    hhea = f["hhea"]
    hhea.ascent, hhea.descent, hhea.lineGap = 1100, -400, 0
    os2 = f["OS/2"]
    os2.usWinAscent, os2.usWinDescent = 1150, 420
    if use_typo:
        os2.fsSelection |= 1 << 7
    else:
        os2.fsSelection &= ~(1 << 7)
    ps = family.replace(" ", "") + "-Regular"
    for rec in f["name"].names:
        if rec.nameID in (1, 16):
            rec.string = family
        elif rec.nameID == 4:
            rec.string = family + " Regular"
        elif rec.nameID in (3, 6):
            rec.string = ps
    f.save(dst, reorderTables=True)


def main():
    src, out = sys.argv[1], sys.argv[2]
    probe(src, out + "/ProbeTypo.ttf", "Probe Typo", True)


if __name__ == "__main__":
    main()
