// What a screenshot shows, roughly: enough to tell a message on screen from a blank (white or
// single-colour) page. Decodes the 8-bit RGB/RGBA non-interlaced PNGs Chromium writes, with
// node:zlib only.
import { inflateSync } from "node:zlib";

export function decode(buf) {
  if (buf.readUInt32BE(0) !== 0x89504e47) throw new Error("not a PNG");
  let at = 8;
  let width = 0;
  let height = 0;
  let channels = 0;
  const idat = [];
  while (at < buf.length) {
    const len = buf.readUInt32BE(at);
    const type = buf.toString("latin1", at + 4, at + 8);
    const data = buf.subarray(at + 8, at + 8 + len);
    if (type === "IHDR") {
      width = data.readUInt32BE(0);
      height = data.readUInt32BE(4);
      const depth = data[8];
      const colour = data[9];
      if (depth !== 8 || (colour !== 2 && colour !== 6) || data[12] !== 0) throw new Error(`unsupported PNG (depth ${depth}, colour ${colour})`);
      channels = colour === 6 ? 4 : 3;
    } else if (type === "IDAT") idat.push(data);
    else if (type === "IEND") break;
    at += 12 + len;
  }
  const raw = inflateSync(Buffer.concat(idat));
  const stride = width * channels;
  const px = Buffer.alloc(stride * height);
  for (let y = 0; y < height; y++) {
    const f = raw[y * (stride + 1)];
    const line = raw.subarray(y * (stride + 1) + 1, (y + 1) * (stride + 1));
    for (let x = 0; x < stride; x++) {
      const a = x >= channels ? px[y * stride + x - channels] : 0;
      const b = y > 0 ? px[(y - 1) * stride + x] : 0;
      const c = x >= channels && y > 0 ? px[(y - 1) * stride + x - channels] : 0;
      let v = line[x];
      if (f === 1) v += a;
      else if (f === 2) v += b;
      else if (f === 3) v += (a + b) >> 1;
      else if (f === 4) {
        const p = a + b - c;
        const pa = Math.abs(p - a);
        const pb = Math.abs(p - b);
        const pc = Math.abs(p - c);
        v += pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
      }
      px[y * stride + x] = v & 0xff;
    }
  }
  return { width, height, channels, px };
}

/**
 * { width, height, whiteShare, colours, dominantShare }: the share of near-white pixels, the
 * number of distinct colours (quantised to 4 bits per channel) and the share of the most common.
 */
export function screenStats(buf) {
  const { width, height, channels, px } = decode(buf);
  const counts = new Map();
  let white = 0;
  const n = width * height;
  for (let i = 0; i < n; i++) {
    const r = px[i * channels];
    const g = px[i * channels + 1];
    const b = px[i * channels + 2];
    if (r > 240 && g > 240 && b > 240) white++;
    const key = ((r >> 4) << 8) | ((g >> 4) << 4) | (b >> 4);
    counts.set(key, (counts.get(key) ?? 0) + 1);
  }
  const dominant = Math.max(...counts.values());
  return {
    width,
    height,
    whiteShare: Math.round((white / n) * 1000) / 1000,
    colours: counts.size,
    dominantShare: Math.round((dominant / n) * 1000) / 1000,
  };
}
