// FLIP of our frame against a golden of the original (B4 M5, AC-S2): `flipCompare(ref.png, ours.png, outDir, name)` decodes
// the PNGs, runs the golden crate's `ui_flip` (LDR-FLIP of `tools/golden/src/metrics/flip.rs`) and writes
// `<outDir>/<name>-flip.png` (the heat map) and `<name>-side.png` (ours | golden | FLIP, scaled down). Returns { mean, p95, over02 }.
// The binary: `cargo build -p golden --bin ui_flip` (CIV5_UI_FLIP overrides the path).
import { spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync, existsSync, unlinkSync } from "node:fs";
import { deflateSync } from "node:zlib";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { decode } from "./png.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, "../../..");

function crc32(buf) {
  let c;
  let crc = 0xffffffff;
  for (let n = 0; n < buf.length; n++) {
    c = (crc ^ buf[n]) & 0xff;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    crc = (crc >>> 8) ^ c;
  }
  return (crc ^ 0xffffffff) >>> 0;
}
function chunk(type, data) {
  const t = Buffer.from(type, "latin1");
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(Buffer.concat([t, data])));
  return Buffer.concat([len, t, data, crc]);
}
/** RGBA8 -> PNG. */
export function encodePng(width, height, rgba) {
  const stride = width * 4;
  const raw = Buffer.alloc((stride + 1) * height);
  for (let y = 0; y < height; y++) rgba.copy(raw, y * (stride + 1) + 1, y * stride, (y + 1) * stride);
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(width, 0);
  ihdr.writeUInt32BE(height, 4);
  ihdr[8] = 8;
  ihdr[9] = 6;
  return Buffer.concat([Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]), chunk("IHDR", ihdr), chunk("IDAT", deflateSync(raw)), chunk("IEND", Buffer.alloc(0))]);
}
/** A PNG file as RGBA8 { width, height, rgba }. */
export function readRgba(file) {
  const { width, height, channels, px } = decode(readFileSync(file));
  if (channels === 4) return { width, height, rgba: Buffer.from(px) };
  const rgba = Buffer.alloc(width * height * 4);
  for (let i = 0; i < width * height; i++) {
    rgba[i * 4] = px[i * 3];
    rgba[i * 4 + 1] = px[i * 3 + 1];
    rgba[i * 4 + 2] = px[i * 3 + 2];
    rgba[i * 4 + 3] = 255;
  }
  return { width, height, rgba };
}
function half(img) {
  const w = img.width >> 1;
  const h = img.height >> 1;
  const out = Buffer.alloc(w * h * 4);
  for (let y = 0; y < h; y++)
    for (let x = 0; x < w; x++)
      for (let c = 0; c < 4; c++) {
        const o = (y * 2 * img.width + x * 2) * 4 + c;
        out[(y * w + x) * 4 + c] = (img.rgba[o] + img.rgba[o + 4] + img.rgba[o + img.width * 4] + img.rgba[o + img.width * 4 + 4]) >> 2;
      }
  return { width: w, height: h, rgba: out };
}
function ui_flip_bin() {
  if (process.env.CIV5_UI_FLIP) return process.env.CIV5_UI_FLIP;
  for (const p of [`target/debug/ui_flip.exe`, `target/release/ui_flip.exe`, `target/debug/ui_flip`, `target/release/ui_flip`]) if (existsSync(path.join(root, p))) return path.join(root, p);
  throw new Error("ui_flip is not built: cargo build -p golden --bin ui_flip");
}

function crop(img, [x, y, w, h]) {
  const out = Buffer.alloc(w * h * 4);
  for (let r = 0; r < h; r++) img.rgba.copy(out, r * w * 4, ((y + r) * img.width + x) * 4, ((y + r) * img.width + x + w) * 4);
  return { width: w, height: h, rgba: out };
}

/** `region` = [x, y, w, h]: compare only that part of both frames (the window, when the world behind it differs). */
export function flipCompare(refPng, oursPng, outDir, name, region = null) {
  mkdirSync(outDir, { recursive: true });
  let ref = readRgba(refPng);
  let ours = readRgba(oursPng);
  if (ref.width !== ours.width || ref.height !== ours.height) throw new Error(`size differs: golden ${ref.width}x${ref.height}, ours ${ours.width}x${ours.height}`);
  if (region) { ref = crop(ref, region); ours = crop(ours, region); }
  const tmp = path.join(outDir, `${name}`);
  writeFileSync(`${tmp}-ref.rgba`, ref.rgba);
  writeFileSync(`${tmp}-ours.rgba`, ours.rgba);
  const r = spawnSync(ui_flip_bin(), [`${tmp}-ref.rgba`, `${tmp}-ours.rgba`, String(ref.width), String(ref.height), `${tmp}-heat.rgba`], { encoding: "utf8", maxBuffer: 1 << 24 });
  if (r.status !== 0) throw new Error(`ui_flip failed: ${r.stderr || r.error}`);
  const heat = { width: ref.width, height: ref.height, rgba: readFileSync(`${tmp}-heat.rgba`) };
  writeFileSync(`${tmp}-flip.png`, encodePng(heat.width, heat.height, heat.rgba));
  // ours | golden | FLIP, each at half size, side by side
  const parts = [half(ours), half(ref), half(heat)];
  const w = parts[0].width;
  const h = parts[0].height;
  const side = Buffer.alloc(w * 3 * h * 4);
  parts.forEach((p, i) => {
    for (let y = 0; y < h; y++) p.rgba.copy(side, (y * w * 3 + i * w) * 4, y * w * 4, (y + 1) * w * 4);
  });
  writeFileSync(`${tmp}-side.png`, encodePng(w * 3, h, side));
  for (const s of ["-ref.rgba", "-ours.rgba", "-heat.rgba"]) { try { unlinkSync(`${tmp}${s}`); } catch { /* ignore */ } }
  const m = /mean=([\d.]+) p95=([\d.]+) over02=([\d.]+)/.exec(r.stdout);
  if (!m) throw new Error(`ui_flip output: ${r.stdout}`);
  return { mean: Number(m[1]), p95: Number(m[2]), over02: Number(m[3]) };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const [ref, ours, out, name] = process.argv.slice(2);
  console.log(JSON.stringify(flipCompare(ref, ours, out, name ?? "flip")));
}
