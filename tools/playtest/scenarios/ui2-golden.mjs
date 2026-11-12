// `ui2-golden`: the pixels of UI v2 against references recorded by our own rendering. The cases, thresholds and
// defaults are in `tools/ui-convert/golden.toml`; geometry against Chrome is `cargo xtask ui verify`, not this.
//
//   cargo xtask playtest --scenario ui2-golden --attach <url of a real-core server> [--record]
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8893 --scenario ui2-golden [--record] [--headed]
//
// Each case renders the skin over a solid background (`?ui2bg=`, the world hidden, the scene frozen) on SwiftShader
// and compares the frame with `<refs>/<case>-<backend>.png` by LDR-FLIP over the interface pixels only (`ui_flip`
// with a mask; `cargo build -p golden --bin ui_flip` runs first). `<refs>` is `CIV5_UI2_GOLDEN_DIR`, else
// `<cache>/ui2-golden/<skin>` with `<cache>` = `CIV5_CACHE` or `cache` next to the repository: references stay out of
// the repository (the frames may show the game's text and, later, its icons).
//
// `--record` writes the references instead: every case twice, in two fresh sessions; the first frame is the
// reference, the FLIP of the two is the measured noise (`<case>-<backend>.json`), and the skin's source page in
// Chromium with its sample (`<case>-chrome.png`) is saved next to it to be looked at once (never compared).
//
// Sources: `game` signs in, starts a game (seed 7, turn 0) with `?ui=v2:<skin>&ui2bg=<hex>` and waits for the skin;
// the state `hud` is the game screen as it comes up (cases of one game that differ in the skin change it on the
// screen), shot once the game's UI set has loaded and every game picture's texture is on the GPU (`ui2.assets.*`
// of the probe): before that a unit flag shows its fallback mark, after it the unit's icon. `sample` opens `?ui=v2:<skin>&ui2sample=<state>` with no game (the client's sample mode). A case may also
// hover a part (`hover = { part, row }`: the centre of the part, or of its `row`-th list instance), play with mods
// (`mods = "hello"`: the game's quick start takes them; `UI2_MODS_ATTACH` names the server that has them, else
// `--attach`) and break a mod's script (`broken_mod = "hello"`: the mod's panels are converted again from
// `examples/mods/<id>` with a script that throws, and the page gets that package instead of the server's).
//
// SIMPLIFIED(SIMP-216): golden frames of UI v2 have no world under the interface (a solid background), so transparency
// and blending over the map are not checked.
import { spawnSync } from "node:child_process";
import { copyFileSync, cpSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync, unlinkSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { openPage, act, until, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { encodePng, readRgba } from "../lib/flip.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(here, "../../..");
const GOLDEN_TOML = path.join(ROOT, "tools/ui-convert/golden.toml");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
/** How long a game may take to bring the skin up (a cold boot on SwiftShader is slow). */
const UP_MS = Number(process.env.UI2_UP_MS ?? 600000);

// ---------------------------------------------------------------- TOML (the subset the configuration files use)

/**
 * A small TOML reader: comments, `[table]`, `[[array of tables]]`, dotted table names, bare or quoted keys, and
 * values that are strings, numbers, booleans, arrays and inline tables (an array may span lines).
 */
export function parseToml(text) {
  const root = {};
  let table = root;
  const lines = text.split(/\r?\n/);
  for (let n = 0; n < lines.length; n++) {
    let line = stripComment(lines[n]).trim();
    if (!line) continue;
    const where = `line ${n + 1}`;
    let m;
    if ((m = /^\[\[\s*([^\]]+?)\s*\]\]$/.exec(line))) {
      const keys = splitKey(m[1], where);
      const parent = descend(root, keys.slice(0, -1), where);
      const last = keys[keys.length - 1];
      if (parent[last] === undefined) parent[last] = [];
      if (!Array.isArray(parent[last])) throw new Error(`${where}: ${m[1]} is not an array of tables`);
      table = {};
      parent[last].push(table);
      continue;
    }
    if ((m = /^\[\s*([^\]]+?)\s*\]$/.exec(line))) {
      table = descend(root, splitKey(m[1], where), where);
      continue;
    }
    // An array or inline table may go on over the next lines: join until the brackets balance.
    while (!balanced(line) && n + 1 < lines.length) line += " " + stripComment(lines[++n]).trim();
    const eq = findEq(line);
    if (eq < 0) throw new Error(`${where}: expected key = value`);
    const keys = splitKey(line.slice(0, eq), where);
    const at = descend(table, keys.slice(0, -1), where);
    const [value, end] = parseValue(line, eq + 1, where);
    if (line.slice(end).trim()) throw new Error(`${where}: unexpected ${line.slice(end).trim()}`);
    const last = keys[keys.length - 1];
    if (last in at) throw new Error(`${where}: ${last} is set twice`);
    at[last] = value;
  }
  return root;
}

function stripComment(line) {
  let q = null;
  for (let i = 0; i < line.length; i++) {
    const c = line[i];
    if (q) {
      if (c === "\\" && q === '"') i++;
      else if (c === q) q = null;
    } else if (c === '"' || c === "'") q = c;
    else if (c === "#") return line.slice(0, i);
  }
  return line;
}

function balanced(s) {
  let depth = 0;
  let q = null;
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (q) {
      if (c === "\\" && q === '"') i++;
      else if (c === q) q = null;
    } else if (c === '"' || c === "'") q = c;
    else if (c === "[" || c === "{") depth++;
    else if (c === "]" || c === "}") depth--;
  }
  return depth <= 0;
}

function findEq(s) {
  let q = null;
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (q) {
      if (c === q) q = null;
    } else if (c === '"' || c === "'") q = c;
    else if (c === "=") return i;
  }
  return -1;
}

function splitKey(s, where) {
  const keys = [];
  let i = 0;
  s = s.trim();
  while (i < s.length) {
    while (s[i] === " " || s[i] === "\t") i++;
    if (s[i] === '"' || s[i] === "'") {
      const [v, end] = parseString(s, i, where);
      keys.push(v);
      i = end;
    } else {
      const m = /^[A-Za-z0-9_-]+/.exec(s.slice(i));
      if (!m) throw new Error(`${where}: bad key ${s}`);
      keys.push(m[0]);
      i += m[0].length;
    }
    while (s[i] === " " || s[i] === "\t") i++;
    if (i < s.length) {
      if (s[i] !== ".") throw new Error(`${where}: bad key ${s}`);
      i++;
    }
  }
  if (!keys.length) throw new Error(`${where}: empty key`);
  return keys;
}

function descend(at, keys, where) {
  for (const k of keys) {
    if (at[k] === undefined) at[k] = {};
    at = Array.isArray(at[k]) ? at[k][at[k].length - 1] : at[k];
    if (typeof at !== "object" || at === null) throw new Error(`${where}: ${k} is not a table`);
  }
  return at;
}

function parseString(s, i, where) {
  const q = s[i];
  let out = "";
  for (let j = i + 1; j < s.length; j++) {
    const c = s[j];
    if (c === q) return [out, j + 1];
    if (c === "\\" && q === '"') {
      const e = s[++j];
      const simple = { n: "\n", t: "\t", r: "\r", '"': '"', "\\": "\\", b: "\b", f: "\f" };
      if (e in simple) out += simple[e];
      else if (e === "u") {
        out += String.fromCharCode(parseInt(s.slice(j + 1, j + 5), 16));
        j += 4;
      } else throw new Error(`${where}: bad escape \\${e}`);
    } else out += c;
  }
  throw new Error(`${where}: unterminated string`);
}

function parseValue(s, i, where) {
  while (s[i] === " " || s[i] === "\t") i++;
  const c = s[i];
  if (c === '"' || c === "'") return parseString(s, i, where);
  if (c === "[") {
    const out = [];
    i++;
    for (;;) {
      while (/\s|,/.test(s[i] ?? "")) i++;
      if (s[i] === "]") return [out, i + 1];
      if (i >= s.length) throw new Error(`${where}: unterminated array`);
      const [v, end] = parseValue(s, i, where);
      out.push(v);
      i = end;
    }
  }
  if (c === "{") {
    const out = {};
    i++;
    for (;;) {
      while (/\s|,/.test(s[i] ?? "")) i++;
      if (s[i] === "}") return [out, i + 1];
      if (i >= s.length) throw new Error(`${where}: unterminated inline table`);
      const eq = findEq(s.slice(i));
      if (eq < 0) throw new Error(`${where}: expected key = value in an inline table`);
      const keys = splitKey(s.slice(i, i + eq), where);
      const [v, end] = parseValue(s, i + eq + 1, where);
      descend(out, keys.slice(0, -1), where)[keys[keys.length - 1]] = v;
      i = end;
    }
  }
  const m = /^(true|false|[+-]?\d[\d_]*(\.\d+)?([eE][+-]?\d+)?)/.exec(s.slice(i));
  if (!m) throw new Error(`${where}: bad value ${s.slice(i)}`);
  const v = m[1] === "true" ? true : m[1] === "false" ? false : Number(m[1].replace(/_/g, ""));
  return [v, i + m[1].length];
}

// ---------------------------------------------------------------- the configuration

const SOURCES = ["game", "sample"];
const BACKENDS = ["webgpu", "webgl2"];
const METRICS = ["mean", "over02"];

/** The cases of `golden.toml`, with the defaults filled in; `only` (a list of ids or names) picks some. */
export function loadConfig(text, only = null) {
  const t = parseToml(text);
  const d = t.defaults ?? {};
  const cases = (t.case ?? []).map((c, i) => {
    const x = { ...d, ...c };
    const where = `case ${c.id ?? i + 1}`;
    for (const k of ["id", "name", "state", "skin", "background"]) if (typeof x[k] !== "string" || !x[k]) throw new Error(`${where}: ${k} is required`);
    if (!/^[\w.-]+$/.test(x.name)) throw new Error(`${where}: name ${x.name} must be a file name`);
    if (!/^[0-9a-fA-F]{6}$/.test(x.background)) throw new Error(`${where}: background ${x.background} is not rrggbb`);
    if (!SOURCES.includes(x.source)) throw new Error(`${where}: source ${x.source}: want ${SOURCES.join(" or ")}`);
    if (!Array.isArray(x.viewport) || x.viewport.length !== 2 || !x.viewport.every((v) => Number.isInteger(v) && v > 0)) throw new Error(`${where}: viewport is [width, height]`);
    if (!(typeof x.scale === "number" && x.scale > 0)) throw new Error(`${where}: scale must be a positive number`);
    if (x.hover !== undefined && !(typeof x.hover === "object" && typeof x.hover.part === "string" && (x.hover.row === undefined || Number.isInteger(x.hover.row)))) throw new Error(`${where}: hover is { part = "<data-part>", row = <n> }`);
    for (const k of ["mods", "broken_mod"]) if (x[k] !== undefined && !(typeof x[k] === "string" && /^[a-z0-9_-]+$/.test(x[k]))) throw new Error(`${where}: ${k} is a mod id`);
    if ((x.mods || x.broken_mod) && x.source !== "game") throw new Error(`${where}: mods play in a game: they need the game source`);
    if (x.broken_mod && x.broken_mod !== x.mods) throw new Error(`${where}: broken_mod must be one of the case's mods`);
    const backends = x.backends ?? BACKENDS;
    for (const b of backends) if (!BACKENDS.includes(b)) throw new Error(`${where}: backend ${b}: want ${BACKENDS.join(" or ")}`);
    return { ...x, backends };
  });
  const names = new Set();
  for (const c of cases) {
    if (names.has(c.name)) throw new Error(`case ${c.id}: name ${c.name} is used twice`);
    names.add(c.name);
  }
  const picked = only ? cases.filter((c) => only.includes(c.id) || only.includes(c.name)) : cases;
  const threshold = {};
  for (const m of METRICS) {
    const s = t.threshold?.[m] ?? {};
    threshold[m] = { factor: s.factor ?? 1.5, margin: s.margin ?? 0, floor: s.floor ?? 0 };
  }
  const compare = { backgroundTolerance: t.compare?.background_tolerance ?? 2, grow: t.compare?.grow ?? 2 };
  return { cases: picked, threshold, compare };
}

/** threshold = max(factor * noise, noise + margin, floor). */
export function limit(t, noise) {
  const n = Math.max(0, noise ?? 0);
  return Math.max(t.factor * n, n + t.margin, t.floor);
}

/** Where the references of `skin` live (never inside the repository). */
export function refDir(skin) {
  if (process.env.CIV5_UI2_GOLDEN_DIR) return path.resolve(process.env.CIV5_UI2_GOLDEN_DIR, skin);
  const cache = process.env.CIV5_CACHE || path.resolve(ROOT, "..", "cache");
  return path.join(cache, "ui2-golden", skin);
}

// ---------------------------------------------------------------- pixels

/**
 * The interface mask: 255 where either frame differs from the background by more than `tol` in any channel, grown
 * by `grow` pixels (a square), else 0.
 */
export function uiMask(a, b, bg, tol, grow) {
  const { width: w, height: h } = a;
  const core = new Uint8Array(w * h);
  const off = (img, o) => Math.abs(img[o] - bg[0]) > tol || Math.abs(img[o + 1] - bg[1]) > tol || Math.abs(img[o + 2] - bg[2]) > tol;
  for (let i = 0; i < w * h; i++) if (off(a.rgba, i * 4) || off(b.rgba, i * 4)) core[i] = 1;
  // Grow along rows, then along columns (a square of 2 * grow + 1).
  const g = Math.max(0, grow);
  const rows = new Uint8Array(w * h);
  for (let y = 0; y < h; y++) grow1d(core, rows, w, 1, y * w, g);
  const out = new Uint8Array(w * h);
  for (let x = 0; x < w; x++) grow1d(rows, out, h, w, x, g);
  return Buffer.from(out.map((v) => v * 255));
}

/** Marks in `dst` every one of the `n` cells (`offset + k * stride`) within `g` cells of a marked cell of `src`. */
function grow1d(src, dst, n, stride, offset, g) {
  let last = -Infinity;
  for (let k = 0; k < n; k++) {
    const i = offset + k * stride;
    if (src[i]) last = k;
    if (k - last <= g) dst[i] = 1;
  }
  let next = Infinity;
  for (let k = n - 1; k >= 0; k--) {
    const i = offset + k * stride;
    if (src[i]) next = k;
    if (next - k <= g) dst[i] = 1;
  }
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

let flipBin = null;
/** `ui_flip`, built once per run (`CIV5_UI_FLIP` names a ready one). */
function uiFlip() {
  if (flipBin) return flipBin;
  if (process.env.CIV5_UI_FLIP) return (flipBin = process.env.CIV5_UI_FLIP);
  const r = spawnSync("cargo", ["build", "-q", "-p", "golden", "--bin", "ui_flip"], { cwd: ROOT, encoding: "utf8", stdio: ["ignore", "inherit", "inherit"] });
  if (r.status !== 0) throw new Error("cargo build -p golden --bin ui_flip failed");
  const exe = path.join(ROOT, "target", "debug", `ui_flip${process.platform === "win32" ? ".exe" : ""}`);
  if (!existsSync(exe)) throw new Error(`ui_flip is not at ${exe}`);
  return (flipBin = exe);
}

/**
 * FLIP of `ours` against `ref` (PNG files) over the interface mask. Writes `<out>/<name>-flip.png` (the heat map)
 * and `<name>-side.png` (ours | reference | FLIP at half size). Returns { mean, p95, over02, pixels }.
 */
export function compareFrames(refPng, oursPng, out, name, bgHex, cmp) {
  const ref = readRgba(refPng);
  const ours = readRgba(oursPng);
  if (ref.width !== ours.width || ref.height !== ours.height) throw new Error(`size differs: reference ${ref.width}x${ref.height}, ours ${ours.width}x${ours.height}`);
  const bg = [0, 2, 4].map((k) => parseInt(bgHex.slice(k, k + 2), 16));
  const mask = uiMask(ref, ours, bg, cmp.backgroundTolerance, cmp.grow);
  const tmp = path.join(out, name);
  writeFileSync(`${tmp}-ref.rgba`, ref.rgba);
  writeFileSync(`${tmp}-ours.rgba`, ours.rgba);
  writeFileSync(`${tmp}-mask.r8`, mask);
  try {
    const r = spawnSync(uiFlip(), [`${tmp}-ref.rgba`, `${tmp}-ours.rgba`, String(ref.width), String(ref.height), `${tmp}-heat.rgba`, `${tmp}-mask.r8`], { encoding: "utf8", maxBuffer: 1 << 24 });
    if (r.status !== 0) throw new Error(`ui_flip failed: ${r.stderr || r.error}`);
    const m = /mean=([\d.]+) p95=([\d.]+) over02=([\d.]+) pixels=(\d+)/.exec(r.stdout);
    if (!m) throw new Error(`ui_flip output: ${r.stdout}`);
    const heat = { width: ref.width, height: ref.height, rgba: readFileSync(`${tmp}-heat.rgba`) };
    writeFileSync(`${tmp}-flip.png`, encodePng(heat.width, heat.height, heat.rgba));
    const parts = [half(ours), half(ref), half(heat)];
    const w = parts[0].width;
    const h = parts[0].height;
    const side = Buffer.alloc(w * 3 * h * 4);
    parts.forEach((p, i) => {
      for (let y = 0; y < h; y++) p.rgba.copy(side, (y * w * 3 + i * w) * 4, y * w * 4, (y + 1) * w * 4);
    });
    writeFileSync(`${tmp}-side.png`, encodePng(w * 3, h, side));
    return { mean: Number(m[1]), p95: Number(m[2]), over02: Number(m[3]), pixels: Number(m[4]) };
  } finally {
    for (const s of ["-ref.rgba", "-ours.rgba", "-heat.rgba", "-mask.r8"]) {
      try { unlinkSync(`${tmp}${s}`); } catch { /* ignore */ }
    }
  }
}

// ---------------------------------------------------------------- the page

/** `ui2()` (null without `&ui=v2` or before the hooks exist). */
async function ui2(page) {
  return page.evaluate(() => (window.__civ5test?.ui2 ? window.__civ5test.ui2() : null)).catch(() => null);
}

/** The viewport in CSS px and the device scale, through the DevTools protocol (the frame is in device pixels). */
async function setMetrics(cdp, c) {
  await cdp.send("Emulation.setDeviceMetricsOverride", { width: c.viewport[0], height: c.viewport[1], deviceScaleFactor: c.scale, mobile: false });
}

/** Signs in and waits for the game to be played (autostart, seed 7, turn 0). */
async function enterGame(page, base, o, extra) {
  for (let attempt = 0; ; attempt++) {
    const nick = `ui2g${(Date.now() + attempt) % 1000000}`;
    await page.goto(appUrl(base, o, { nick, autostart: "1", ai: "2", size: "small", seed: "7", mode: "simultaneous", manual: "1", ...extra }));
    try {
      const s = await until(page, (x) => x.screen === "login" || x.screen === "play", 90000, "sign-in window or the game");
      if (s.screen === "login") await act(page, { kind: "login", nick, password: DEV_PASSWORD });
      await until(page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 120000, "the game");
      return;
    } catch (e) {
      if (attempt >= 2) throw e;
    }
  }
}

/** Waits until the skin runs and has drawn a while. */
async function skinUp(page) {
  const t0 = Date.now();
  for (;;) {
    const p = await ui2(page);
    if (p?.state === "classic") throw new Error("UI v2 fell back to the classic UI (see the console log)");
    if (p?.state === "running" && p.frames > 30) return p;
    if (Date.now() - t0 > UP_MS) throw new Error(`UI v2 is not running after ${UP_MS / 1000} s: ${JSON.stringify({ state: p?.state, frames: p?.frames })}`);
    await sleep(250);
  }
}

/** One frame (PNG, device pixels) once two in a row are the same; { png, settled, tries }. */
async function settledFrame(cdp) {
  let last = null;
  for (let tries = 1; tries <= 30; tries++) {
    await sleep(tries === 1 ? 1000 : 400);
    const { data } = await cdp.send("Page.captureScreenshot", { format: "png", captureBeyondViewport: false });
    const png = Buffer.from(data, "base64");
    if (last && png.equals(last)) return { png, settled: true, tries };
    last = png;
  }
  return { png: last, settled: false, tries: 30 };
}

/** The instances of a list under a part (`ui2Rects` records): per instance key, the box of its largest record. */
function rowsOf(rects) {
  const by = new Map();
  for (const r of rects) {
    if (!r.keys || r.node.includes("#")) continue;
    const best = by.get(r.keys);
    if (!best || r.w * r.h > best.w * best.h) by.set(r.keys, r);
  }
  return [...by.values()].sort((a, b) => a.y - b.y || a.x - b.x);
}

/** Moves the pointer onto `hover` (a part, or its `row`-th list instance); returns where, or throws. */
async function hoverPart(page, hover) {
  const rects = await page.evaluate((p) => window.__civ5test.ui2Rects([p]), hover.part);
  if (!Array.isArray(rects) || !rects.length) throw new Error(`hover: no part ${hover.part} on the screen`);
  let box = rects[0];
  if (hover.row !== undefined) {
    // The innermost list when lists nest (the options of the production list's groups: keys `group/option`).
    const all = rowsOf(rects);
    const depth = Math.max(...all.map((r) => r.keys.split("/").length));
    const rows = all.filter((r) => r.keys.split("/").length === depth);
    box = rows[hover.row];
    if (!box) throw new Error(`hover: ${hover.part} has ${rows.length} rows, not ${hover.row + 1}`);
  }
  const at = [box.x + Math.min(box.w / 2, 120), box.y + box.h / 2];
  await page.mouse.move(at[0] - 3, at[1]);
  await sleep(100);
  await page.mouse.move(at[0], at[1]);
  return at;
}

let broken = null;
/**
 * The fragment package of mod `id` with scripts that throw (built once per run): `{ file, hash, path }`. The mod's
 * folder is copied to `<logs>/broken-<id>`, its UI scripts replaced, and `ui2-convert fragment` builds the package.
 */
function brokenFragment(id, logDir) {
  if (broken) return broken;
  const src = path.join(ROOT, "examples", "mods", id);
  const dir = path.join(logDir, `broken-${id}`);
  rmSync(dir, { recursive: true, force: true });
  cpSync(src, dir, { recursive: true, filter: (f) => !/[\\/](target|node_modules)$/.test(f) });
  const manifest = parseToml(readFileSync(path.join(dir, "mod.toml"), "utf8"));
  for (const sc of manifest.ui?.scripts ?? []) writeFileSync(path.join(dir, sc), "// Broken on purpose: the panel must give way to its placeholder.\nthrow new Error(\"broken on purpose\");\n");
  const out = path.join(dir, "out");
  mkdirSync(out, { recursive: true });
  const r = spawnSync("cargo", ["run", "-q", "-p", "ui2-convert", "--", "fragment", dir, "--out", out], { cwd: ROOT, encoding: "utf8" });
  const m = /^ui2-fragment (\S+) ([0-9a-fA-F]+)\s*$/m.exec(r.stdout ?? "");
  if (r.status !== 0 || !m) throw new Error(`ui2-convert fragment ${dir} failed: ${(r.stderr || r.stdout || "").trim().split("\n").pop()}`);
  return (broken = { file: m[1], hash: m[2].toLowerCase(), path: path.join(out, m[1]) });
}

/** The page gets the broken package of mod `id` in place of the server's (the index entry and the file). */
async function routeBroken(page, id, logDir) {
  const b = brokenFragment(id, logDir);
  await page.route("**/ui2/mods-ui.json", async (r) => {
    const resp = await r.fetch();
    const list = await resp.json();
    for (const e of list) if (e.id === id) Object.assign(e, { file: `mods/${b.file}`, hash: b.hash });
    await r.fulfill({ response: resp, json: list });
  });
  await page.route(`**/ui2/mods/${b.file}`, (r) => r.fulfill({ path: b.path, contentType: "application/octet-stream" }));
  return b;
}

/** Waits until UI v2 runs `skin` (after a change on the screen). */
async function skinIs(page, skin) {
  const t0 = Date.now();
  for (;;) {
    const p = await ui2(page);
    if (p?.state === "running" && p.skin === skin && p.frames > 10) return p;
    if (Date.now() - t0 > 30000) throw new Error(`the skin ${skin} is not on the screen after 30 s: ${JSON.stringify({ skin: p?.skin, state: p?.state })}`);
    await sleep(200);
  }
}

/**
 * Waits until the game's pictures are drawn as they will stay: the game's UI set is loaded (`ui2.assets.set`), the
 * skin has named its pictures and the renderer holds every texture they name (`ui2.assets.resident` ==
 * `ui2.assets.textures`). Before that a unit flag shows its fallback mark instead of the unit's icon.
 */
async function gamePictures(page) {
  const t0 = Date.now();
  for (;;) {
    const c = (await ui2(page))?.counters ?? {};
    const set = c["ui2.assets.set"];
    const textures = c["ui2.assets.textures"];
    if (set === 1 && textures > 0 && c["ui2.assets.resident"] === textures) return { ms: Date.now() - t0, textures };
    if (Date.now() - t0 > UP_MS) throw new Error(`the game's pictures are not drawn after ${UP_MS / 1000} s: ${JSON.stringify({ set, textures, resident: c["ui2.assets.resident"] })}`);
    await sleep(250);
  }
}

/** Waits until the panels of the case's mod are up (or show their placeholder, for a broken mod). */
async function modPanels(page, c) {
  const t0 = Date.now();
  for (;;) {
    const p = await ui2(page);
    const mine = (p?.panels ?? []).filter((x) => (x.owner ?? "").includes(c.mods));
    if (mine.length && mine.every((x) => x.status === (c.broken_mod ? "placeholder" : "ok"))) return { ok: true, panels: mine };
    if (Date.now() - t0 > 90000) return { ok: false, panels: p?.panels ?? null };
    await sleep(250);
  }
}

/**
 * One session: a fresh browser per group of cases (the sample cases on one background share one; the game cases
 * that share their mods share one game, the skin changing on the screen), the cases shot one after another (the
 * viewport and scale change on the same page). Returns [{ c, backend, file, pkg, settled }] and the console errors.
 */
async function session(ctx, cases, backend, outDir, tag, check) {
  const o = ctx.opts;
  const base = o.attach ?? ctx.server?.url;
  const shots = [];
  const errors = [];
  const groups = new Map();
  for (const c of cases) {
    const k = c.source === "game" ? `game|${c.mods ?? ""}|${c.broken_mod ?? ""}|${c.background}` : `sample|${c.background}`;
    if (!groups.has(k)) groups.set(k, []);
    groups.get(k).push(c);
  }
  for (const group of groups.values()) {
    const first = group[0];
    const label = `${tag} ${backend} ${first.source}:${first.name}`;
    const server = first.mods ? (process.env.UI2_MODS_ATTACH ?? base) : base;
    if (first.source === "game" && !o.attach) {
      for (const c of group) check(`${c.id} ${c.name} (${backend}): rendered`, false, "the game source needs --attach <url of a real-core server> (tools/playtest/m2/server.ps1)");
      continue;
    }
    const b = await openPage({ headed: o.headed, swiftshader: true, logDir: o.logDir, viewport: { width: first.viewport[0], height: first.viewport[1] }, consoleFile: `console-${tag}-${backend}-${first.name}.log` });
    let current = null;
    try {
      const cdp = await b.page.context().newCDPSession(b.page);
      const gfx = { gfx: backend === "webgl2" ? "webgl2" : null };
      let skin = null;
      for (const c of group) {
        current = c;
        await setMetrics(cdp, c);
        const v2 = { ui: `v2:${c.skin}`, ui2bg: c.background.toLowerCase() };
        if (c.source === "sample") {
          await b.page.goto(appUrl(server, gfx, { ...v2, ui2sample: c.state }));
          await skinUp(b.page);
        } else {
          if (c.state !== "hud") throw new Error(`state ${c.state}: the game source has only hud (others need the sample source)`);
          if (c === first) {
            if (c.broken_mod) await routeBroken(b.page, c.broken_mod, o.logDir);
            await enterGame(b.page, server, gfx, { ...v2, ...(c.mods ? { mods: c.mods } : {}) });
            await skinUp(b.page);
          } else if (skin !== c.skin) {
            await act(b.page, { kind: "setUi", value: `v2:${c.skin}` });
          }
          await skinIs(b.page, c.skin);
          if (c.mods) {
            const m = await modPanels(b.page, c);
            check(`${c.id} ${c.name} (${backend}, ${tag}): the mod's panels are ${c.broken_mod ? "placeholders" : "up"}`, m.ok, JSON.stringify(m.panels));
          }
          // The game's pictures (unit flags, resources) come from the game's UI set, which loads after the skin:
          // the frame is taken once the set is here and every picture's texture is on the GPU, not before.
          await gamePictures(b.page);
        }
        skin = c.skin;
        // The pointer rests in the middle of the screen (the world, or nothing), away from any hover style.
        await b.page.mouse.move(c.viewport[0] / 2, c.viewport[1] / 2);
        if (c.hover) await hoverPart(b.page, c.hover);
        const probe = await ui2(b.page);
        const { png, settled, tries } = await settledFrame(cdp);
        const file = path.join(outDir, `${c.name}-${backend}${tag === "ours" ? "" : `-${tag}`}.png`);
        writeFileSync(file, png);
        shots.push({ c, backend, file, pkg: probe?.pkg ?? null, settled, tries });
      }
    } catch (e) {
      check(`${label}: session (at ${current?.id} ${current?.name})`, false, e.message.split("\n")[0]);
      try { await b.page.screenshot({ path: path.join(o.logDir, `failure-${tag}-${backend}-${current?.name ?? first.name}.png`) }); } catch { /* ignore */ }
    }
    // A broken mod's scripts fail and are switched off, which the console says: that is the case, not an error. A
    // mod script context that is not ready in time is checked on its own (it is not this scenario's subject).
    const late = (e) => /\[mod:[\w-]+\].*(not ready after|switched off)/.test(e);
    if (first.mods && !first.broken_mod) {
      const n = b.errors.filter(late).length;
      check(`${label}: the mod's script context starts in time (no "not ready" kill)`, n === 0, `${n} kill(s) or switch-off(s): ${b.errors.find(late) ?? ""}`);
    }
    errors.push(...b.errors.filter((e) => !e.includes("/pkg-mt/") && !late(e) && !(first.broken_mod && e.includes(`[mod:${first.broken_mod}]`))));
    await b.close();
  }
  return { shots, errors };
}

/** The fonts a skin's stylesheets name (`[fonts.files]` of `skin.toml`), copied into `dir/fonts`. */
function skinFonts(skinRoot, dir) {
  const files = parseToml(readFileSync(path.join(skinRoot, "skin.toml"), "utf8")).fonts?.files ?? {};
  for (const [name, from] of Object.entries(files)) {
    const to = path.join(dir, name);
    mkdirSync(path.dirname(to), { recursive: true });
    copyFileSync(path.join(ROOT, from), to);
  }
}

/**
 * The skin's source page in Chromium with its sample (`?ui2state=<state>`), on the same background: the frame to
 * look at next to a new reference. Returns the file, or throws.
 */
async function chromeFrame(c, file) {
  const { startServer } = await import("../../ui-convert/node/server.mjs");
  const { launchChromium } = await import("../../ui-convert/node/chromium.mjs");
  // A derived skin has no page of its own: the converter's merged folder of it is a plain skin.
  let skinRoot = path.join(ROOT, "skins", c.skin);
  if (!existsSync(path.join(skinRoot, "index.html"))) skinRoot = path.join(ROOT, "target", "ui2", "merged", c.skin);
  const fontsDir = path.join(path.dirname(file), `.fonts-${c.skin}`);
  skinFonts(skinRoot, fontsDir);
  const server = await startServer({ root: skinRoot, fontsDir, preview: true });
  const browser = await launchChromium();
  try {
    const bctx = await browser.newContext({ viewport: { width: c.viewport[0], height: c.viewport[1] }, deviceScaleFactor: c.scale, serviceWorkers: "block" });
    // Nothing leaves the local server.
    await bctx.route("**/*", (r) => (/^http:\/\/127\.0\.0\.1:/.test(r.request().url()) || /^(data|blob|about):/.test(r.request().url()) ? r.continue() : r.abort("blockedbyclient")));
    const page = await bctx.newPage();
    await page.goto(`${server.url}/index.html?ui2state=${encodeURIComponent(c.state)}`, { waitUntil: "load" });
    await page.evaluate(() => window.__ui2preview?.ready);
    await page.addStyleTag({ content: `html, body { background: #${c.background} !important; }` });
    await page.evaluate(() => document.fonts.ready);
    if (c.hover) {
      // The same part (or its `row`-th list instance) under the pointer.
      const box = await page.evaluate(({ part, row }) => {
        const el = document.querySelector(`[data-part="${part}"]`);
        if (!el) return null;
        let target = el;
        if (row !== undefined) {
          const seen = new Map();
          const insts = [...el.querySelectorAll("[data-ui2-inst]")];
          const depth = Math.max(...insts.map((n) => n.getAttribute("data-ui2-inst").split("/").length));
          for (const n of insts) {
            const k = n.getAttribute("data-ui2-inst");
            if (k.split("/").length === depth && !seen.has(k)) seen.set(k, n);
          }
          target = [...seen.values()][row] ?? null;
        }
        const r = target?.getBoundingClientRect();
        return r ? { x: r.x, y: r.y, w: r.width, h: r.height } : null;
      }, c.hover);
      if (box) await page.mouse.move(box.x + Math.min(box.w / 2, 120), box.y + box.h / 2);
      await page.waitForTimeout(300);
    }
    await page.screenshot({ path: file });
    return file;
  } finally {
    await browser.close();
    await server.close();
  }
}

// ---------------------------------------------------------------- the scenario

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "ui2-golden", pass: false, logs: o.logDir, checks: [], numbers: { record: !!o.record, cases: [] }, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const only = process.env.UI2_GOLDEN_CASES ? process.env.UI2_GOLDEN_CASES.split(",").map((s) => s.trim()).filter(Boolean) : null;
  const cfg = loadConfig(readFileSync(GOLDEN_TOML, "utf8"), only);
  result.numbers.threshold = cfg.threshold;
  if (!cfg.cases.length) {
    check("cases", false, `no case in ${GOLDEN_TOML}${only ? ` named ${only.join(",")}` : ""}`);
    return result;
  }
  const backends = [...new Set(cfg.cases.flatMap((c) => c.backends))];
  const of = (backend) => cfg.cases.filter((c) => c.backends.includes(backend));
  const dirOf = (c) => refDir(c.skin);
  try {
    if (o.record) {
      // Two fresh sessions per backend: the first gives the references, the second the noise.
      for (const backend of backends) {
        const a = await session(ctx, of(backend), backend, o.logDir, "rec-a", check);
        const b = await session(ctx, of(backend), backend, o.logDir, "rec-b", check);
        result.consoleErrors.push(...a.errors, ...b.errors);
        for (const sa of a.shots) {
          const sb = b.shots.find((x) => x.c === sa.c);
          const name = `${sa.c.id} ${sa.c.name} (${backend})`;
          if (!sb) {
            check(`${name}: recorded twice`, false, "the second session gave no frame");
            continue;
          }
          const noise = compareFrames(sa.file, sb.file, o.logDir, `${sa.c.name}-${backend}-noise`, sa.c.background, cfg.compare);
          const dir = dirOf(sa.c);
          mkdirSync(dir, { recursive: true });
          const ref = path.join(dir, `${sa.c.name}-${backend}.png`);
          copyFileSync(sa.file, ref);
          const meta = { id: sa.c.id, name: sa.c.name, state: sa.c.state, source: sa.c.source, skin: sa.c.skin, viewport: sa.c.viewport, scale: sa.c.scale, background: sa.c.background, backend, pkg: sa.pkg, recordedAt: new Date().toISOString(), settled: [sa.settled, sb.settled], noise };
          writeFileSync(path.join(dir, `${sa.c.name}-${backend}.json`), JSON.stringify(meta, null, 2) + "\n");
          const limits = Object.fromEntries(METRICS.map((m) => [m, limit(cfg.threshold[m], Math.max(noise[m], sa.c[`noise_${m}`] ?? 0))]));
          result.numbers.cases.push({ id: sa.c.id, name: sa.c.name, backend, ref, noise, limits, settled: [sa.settled, sb.settled], pkg: sa.pkg });
          check(`${name}: recorded twice, frames settled`, sa.settled && sb.settled, `noise mean ${noise.mean}, over02 ${noise.over02}, p95 ${noise.p95} over ${noise.pixels} px; limits ${JSON.stringify(limits)}; ${ref}`);
          // Within the floor, or within the noise the case declares (a game's frames carry its random nick).
          check(`${name}: the two recordings agree within the floor or the case's declared noise`, METRICS.every((m) => noise[m] <= Math.max(cfg.threshold[m].floor, sa.c[`noise_${m}`] ?? 0)), `noise ${JSON.stringify(noise)}`);
        }
      }
      // The Chrome frame of each case, once, to look at next to the references.
      for (const c of cfg.cases) {
        const file = path.join(dirOf(c), `${c.name}-chrome.png`);
        try {
          await chromeFrame(c, file);
          copyFileSync(file, path.join(o.logDir, `${c.name}-chrome.png`));
          check(`${c.id} ${c.name}: Chrome frame to look at`, true, file);
        } catch (e) {
          check(`${c.id} ${c.name}: Chrome frame to look at`, false, e.message.split("\n")[0]);
        }
      }
    } else {
      for (const backend of backends) {
        const s = await session(ctx, of(backend), backend, o.logDir, "ours", check);
        result.consoleErrors.push(...s.errors);
        for (const shot of s.shots) {
          const c = shot.c;
          const name = `${c.id} ${c.name} (${backend})`;
          const ref = path.join(dirOf(c), `${c.name}-${backend}.png`);
          if (!existsSync(ref)) {
            check(`${name}: within the threshold`, false, `no reference ${ref}: record one with --record`);
            continue;
          }
          let meta = {};
          try { meta = JSON.parse(readFileSync(ref.replace(/\.png$/, ".json"), "utf8")); } catch { /* no noise recorded */ }
          try {
            const r = compareFrames(ref, shot.file, o.logDir, `${c.name}-${backend}`, c.background, cfg.compare);
            const limits = Object.fromEntries(METRICS.map((m) => [m, limit(cfg.threshold[m], Math.max(meta.noise?.[m] ?? 0, c[`noise_${m}`] ?? 0))]));
            const stale = meta.pkg && shot.pkg && meta.pkg !== shot.pkg ? `; the skin package changed since the record (${meta.pkg.slice(0, 16)} -> ${shot.pkg.slice(0, 16)}): look at ${c.name}-${backend}-side.png and record again if it is right` : "";
            result.numbers.cases.push({ id: c.id, name: c.name, backend, flip: r, limits, settled: shot.settled, ref, pkg: shot.pkg, refPkg: meta.pkg ?? null });
            check(`${name}: frame settled`, shot.settled, `after ${shot.tries} frame(s)`);
            check(`${name}: within the threshold`, METRICS.every((m) => r[m] <= limits[m]), `mean ${r.mean} (<= ${limits.mean.toFixed(4)}), over02 ${r.over02} (<= ${limits.over02.toFixed(4)}), p95 ${r.p95}, ${r.pixels} px of interface${stale}`);
          } catch (e) {
            check(`${name}: within the threshold`, false, e.message.split("\n")[0]);
          }
        }
      }
    }
  } catch (e) {
    check("no scenario error", false, e.message);
    result.error = e.stack ?? String(e);
  }
  check("no console errors", result.consoleErrors.length === 0, `${result.consoleErrors.length} error(s)${result.consoleErrors[0] ? `: ${result.consoleErrors[0]}` : ""}`);
  try { writeFileSync(path.join(o.logDir, "ui2-golden.json"), JSON.stringify(result.numbers, null, 2)); } catch { /* no log dir */ }
  result.pass = result.checks.length > 0 && result.checks.every((x) => x.pass);
  result.durationMs = Date.now() - started;
  return result;
}
