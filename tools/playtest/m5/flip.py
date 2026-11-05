"""FLIP (NVIDIA, flip-evaluator on PyPI, BSD-3) of our frame against the golden, for the M5 golden scenes (BUILD B4 AC-S2).

  python -I tools/playtest/m5/flip.py GOLDEN.png OURS.png OUT_DIR [--hide x,y,w,h ...] [--ppd 67]

Writes OUT_DIR/flip.png (the error map, magma colours), OUT_DIR/side.png (golden | ours | map, 3 x 640 px wide) and prints one
JSON line: mean FLIP, the 95th and 99th percentile, the share of pixels with FLIP > 0.25, and the same per screen band
(the top panel, the left column, the right column, the bottom, the map). `--hide` rectangles (golden pixel coordinates) are
set equal in both images before the comparison (live data without a twin in the other frame).
Install: pip install flip-evaluator==1.7 (the tool finds it on PYTHONPATH too).
"""
import json
import sys

import numpy as np
from PIL import Image
import flip_evaluator as flip

BANDS = {  # name: (x0, y0, x1, y1) of the 1920x1080 frame
    "top_panel": (0, 0, 1920, 40),
    "left_column": (0, 40, 330, 1080),
    "right_column": (1560, 40, 1920, 1080),
    "bottom": (330, 900, 1560, 1080),
    "map": (330, 40, 1560, 900),
}


def load(path):
    return np.asarray(Image.open(path).convert("RGB"), dtype=np.float32) / 255.0


def main(argv):
    golden, ours, out = argv[1], argv[2], argv[3]
    hides, ppd = [], 67.0
    i = 4
    while i < len(argv):
        if argv[i] == "--hide":
            hides.append([int(v) for v in argv[i + 1].split(",")])
            i += 2
        elif argv[i] == "--ppd":
            ppd = float(argv[i + 1])
            i += 2
        else:
            raise SystemExit(f"unknown argument {argv[i]}")
    a, b = load(golden), load(ours)
    if a.shape != b.shape:
        raise SystemExit(f"sizes differ: golden {a.shape}, ours {b.shape}")
    for x, y, w, h in hides:
        b[y:y + h, x:x + w] = a[y:y + h, x:x + w]
    err, mean, _ = flip.evaluate(a, b, "LDR", parameters={"ppd": ppd}, applyMagma=False)
    err = np.asarray(err, dtype=np.float32)
    if err.ndim == 3:
        err = err[..., 0]
    res = {"mean": round(float(mean), 5), "p95": round(float(np.percentile(err, 95)), 4), "p99": round(float(np.percentile(err, 99)), 4),
           "over_0_25": round(float((err > 0.25).mean()), 5), "bands": {}}
    for name, (x0, y0, x1, y1) in BANDS.items():
        e = err[y0:y1, x0:x1]
        res["bands"][name] = {"mean": round(float(e.mean()), 5), "over_0_25": round(float((e > 0.25).mean()), 5)}
    # the magma map (the library's own colour map of the error)
    mapped, _, _ = flip.evaluate(a, b, "LDR", parameters={"ppd": ppd}, applyMagma=True, computeMeanError=False)
    mp = (np.clip(np.asarray(mapped, dtype=np.float32), 0, 1) * 255 + 0.5).astype(np.uint8)
    Image.fromarray(mp).save(f"{out}/flip.png")
    h, w = a.shape[:2]
    sw, sh = 640, int(round(640 * h / w))
    tiles = [Image.fromarray((x * 255 + 0.5).astype(np.uint8)).resize((sw, sh), Image.LANCZOS) for x in (a, b)] + [Image.fromarray(mp).resize((sw, sh), Image.LANCZOS)]
    side = Image.new("RGB", (sw * 3, sh))
    for k, t in enumerate(tiles):
        side.paste(t, (k * sw, 0))
    side.save(f"{out}/side.png")
    print(json.dumps(res))


if __name__ == "__main__":
    main(sys.argv)
