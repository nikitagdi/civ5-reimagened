"""crop.py GOLDEN OURS OUT x y w h [scale]: golden above ours, the same region, scaled (a dev helper to read the differences)."""
import sys
from PIL import Image
g, o, out = sys.argv[1:4]
x, y, w, h = map(int, sys.argv[4:8])
s = float(sys.argv[8]) if len(sys.argv) > 8 else 2.0
a = Image.open(g).convert("RGB").crop((x, y, x + w, y + h))
b = Image.open(o).convert("RGB").crop((x, y, x + w, y + h))
W, H = int(w * s), int(h * s)
im = Image.new("RGB", (W, H * 2 + 6), (255, 0, 255))
im.paste(a.resize((W, H), Image.LANCZOS), (0, 0))
im.paste(b.resize((W, H), Image.LANCZOS), (0, H + 6))
im.save(out)
