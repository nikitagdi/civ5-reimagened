// How sharp the text of a screenshot is, in numbers, and zoomed crops to look at it.
//
// `sharpness(png, rect?)`: over the crop (device px), the luminance L (sRGB luma, 0-255), its Sobel gradient, the
// contrast range (p99 - p1 of L) and:
// - `acutance`: the mean of the strongest 5 % of the gradients over the range (1: an edge that jumps the whole range
//   in one pixel; blurred text spreads its edges over two pixels or more and scores lower);
// - `midtone`: of the ink pixels (L away from the background by a tenth of the range or more), the share that is
//   neither near the background nor near the ink (between a quarter and three quarters of the range): grey fringes
//   of soft edges count here, so lower is crisper;
// - `ink`: the share of the crop that is ink (the weight of the text: too thin or too bold shows here).
// `zoom(png, rect, factor)`: the crop enlarged by whole pixels (nearest neighbour), as a PNG buffer.
import { deflateSync } from "node:zlib";
import { decode } from "./png.mjs";

function crop(img, rect) {
  const r = rect ?? { x: 0, y: 0, w: img.width, h: img.height };
  const x0 = Math.max(0, Math.round(r.x));
  const y0 = Math.max(0, Math.round(r.y));
  const x1 = Math.min(img.width, Math.round(r.x + r.w));
  const y1 = Math.min(img.height, Math.round(r.y + r.h));
  return { x0, y0, w: Math.max(0, x1 - x0), h: Math.max(0, y1 - y0) };
}

const pct = (sorted, p) => sorted[Math.min(sorted.length - 1, Math.max(0, Math.floor((p / 100) * sorted.length)))];

export function sharpness(buf, rect) {
  const img = decode(buf);
  const c = crop(img, rect);
  const { channels, px, width } = img;
  if (c.w < 3 || c.h < 3) return null;
  const L = new Float64Array(c.w * c.h);
  for (let y = 0; y < c.h; y++) {
    for (let x = 0; x < c.w; x++) {
      const i = ((c.y0 + y) * width + (c.x0 + x)) * channels;
      L[y * c.w + x] = 0.2126 * px[i] + 0.7152 * px[i + 1] + 0.0722 * px[i + 2];
    }
  }
  const sorted = Array.from(L).sort((a, b) => a - b);
  const lo = pct(sorted, 1);
  const hi = pct(sorted, 99);
  const range = Math.max(1, hi - lo);
  const bg = pct(sorted, 50);
  const mags = [];
  for (let y = 1; y < c.h - 1; y++) {
    for (let x = 1; x < c.w - 1; x++) {
      const v = (dx, dy) => L[(y + dy) * c.w + (x + dx)];
      const gx = v(1, -1) + 2 * v(1, 0) + v(1, 1) - v(-1, -1) - 2 * v(-1, 0) - v(-1, 1);
      const gy = v(-1, 1) + 2 * v(0, 1) + v(1, 1) - v(-1, -1) - 2 * v(0, -1) - v(1, -1);
      mags.push(Math.hypot(gx, gy) / 4);
    }
  }
  mags.sort((a, b) => b - a);
  const top = mags.slice(0, Math.max(1, Math.floor(mags.length * 0.05)));
  const acutance = top.reduce((a, b) => a + b, 0) / top.length / range;
  // The ink is on the far side of the background from the median: light text on a dark panel or the reverse.
  const inkHigh = hi - bg > bg - lo;
  const inkEnd = inkHigh ? hi : lo;
  const span = Math.abs(inkEnd - bg) || 1;
  let ink = 0;
  let mid = 0;
  for (const v of L) {
    const t = (v - bg) / (inkEnd - bg || 1);
    if (t >= 0.1) {
      ink++;
      if (t > 0.25 && t < 0.75) mid++;
    }
  }
  const r3 = (x) => Math.round(x * 1000) / 1000;
  return { w: c.w, h: c.h, range: Math.round(range), span: Math.round(span), acutance: r3(acutance), midtone: r3(ink ? mid / ink : 0), ink: r3(ink / L.length) };
}

function crc32(buf) {
  let c = ~0;
  for (let i = 0; i < buf.length; i++) {
    c ^= buf[i];
    for (let k = 0; k < 8; k++) c = c & 1 ? (c >>> 1) ^ 0xedb88320 : c >>> 1;
  }
  return ~c >>> 0;
}

function chunk(type, data) {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const td = Buffer.concat([Buffer.from(type, "latin1"), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(td));
  return Buffer.concat([len, td, crc]);
}

/** An RGB PNG of `w` x `h` from a callback giving each pixel's [r, g, b]. */
export function encodeRgb(w, h, at) {
  const raw = Buffer.alloc((w * 3 + 1) * h);
  for (let y = 0; y < h; y++) {
    raw[y * (w * 3 + 1)] = 0;
    for (let x = 0; x < w; x++) {
      const [r, g, b] = at(x, y);
      const i = y * (w * 3 + 1) + 1 + x * 3;
      raw[i] = r;
      raw[i + 1] = g;
      raw[i + 2] = b;
    }
  }
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8;
  ihdr[9] = 2;
  return Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), chunk("IHDR", ihdr), chunk("IDAT", deflateSync(raw)), chunk("IEND", Buffer.alloc(0))]);
}

export function zoom(buf, rect, factor = 4) {
  const img = decode(buf);
  const c = crop(img, rect);
  const { channels, px, width } = img;
  return encodeRgb(c.w * factor, c.h * factor, (x, y) => {
    const i = ((c.y0 + Math.floor(y / factor)) * width + (c.x0 + Math.floor(x / factor))) * channels;
    return [px[i], px[i + 1], px[i + 2]];
  });
}
