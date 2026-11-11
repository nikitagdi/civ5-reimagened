"""Font subsetting for the skin converter (tools/ui-convert, src/fonts.rs runs this with `python -I`).

    python -I subset.py --version
        prints the fontTools version
    python -I subset.py <in.ttf> <out.ttf> <unicodes>
        writes the subset of <in.ttf> that keeps <unicodes> (`20-7E,A0-FF,...`, hex ranges)

The subset keeps the font's format (TTF stays TTF, a variable font stays variable with its axes), every
OpenType layout feature (the runtime's shaper turns on kern, liga, clig, tnum, lnum, pnum, onum as Chrome
does; nothing is frozen), the .notdef outline, hinting and the font's own timestamps, so the same input
gives the same bytes on every run.
"""

import sys

from fontTools import subset, version


def main(argv):
    if argv == ["--version"]:
        print(version)
        return 0
    if len(argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2
    src, out, unicodes = argv
    return subset.main([
        src,
        "--output-file=" + out,
        "--unicodes=" + unicodes,
        "--layout-features=*",
        "--layout-scripts=*",
        "--notdef-outline",
        "--notdef-glyph",
        "--name-IDs=*",
        "--name-languages=*",
        "--no-recalc-timestamp",
        "--no-prune-unicode-ranges",
        "--no-prune-codepage-ranges",
    ]) or 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
