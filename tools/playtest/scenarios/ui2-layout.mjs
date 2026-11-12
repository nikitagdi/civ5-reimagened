// `ui2-layout`: the layout of the web client against Chrome. The skin's source page in Chromium (the converter's
// preview in layout mode, with the sample `sample/<state>.json`) and the game page in the sample mode
// (`?ui=v2:<skin>&ui2sample=<state>`, no game) lay out the same state; every record of every `data-part` (the
// element and its text leaves) must agree within 0.5 px on x, y, w and h at the reference viewports:
//   cargo xtask playtest --scenario ui2-layout [--attach <url>]
//   node tools/playtest/run.mjs --scenario ui2-layout [--attach http://127.0.0.1:8860]
//
// `cargo xtask ui verify` does the same with the native runtime; this one checks the runtime as it runs in the browser
// (wasm, the host's viewport and device scale, the sample source). The page's rectangles come from
// `__civ5test.ui2Rects(parts)` (`Runtime::dump_rects` of the first element of each part name), Chrome's from the
// converter's own dump (`tools/ui-convert/node/rects.mjs`); the records of Chrome counted are those under the first
// element of each part name, in document order, so both sides cover the same elements (the second and later city
// banners of a sample are under no part the hook reaches; the native verify compares them).
//
// `UI2_LAYOUT_STATES` (comma list) picks samples, `UI2_LAYOUT_SKIN` the skin (default engraved). The font subsets
// come from `target/ui2/<skin>/fonts` (`cargo xtask ui build <skin>`; built here when missing). Reports:
// <logs>/ui2-layout.json (worst record per state and viewport), <logs>/ui2-layout-<state>-<WxH>[@dsf].txt (the 30
// worst records of a viewport that failed).
import { spawnSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { openPage, appUrl } from "../lib/browser.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(here, "../../..");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const TOLERANCE = 0.5;
/** The reference viewports of the layout check: CSS size and device scale (the last one is the 150 % scale). */
export const VIEWPORTS = [
  [[1280, 720], 1],
  [[1920, 1200], 1],
  [[3440, 1440], 1],
  [[5120, 1440], 1],
  [[1280, 720], 1.5],
];
const UP_MS = Number(process.env.UI2_UP_MS ?? 240000);
/** The parts of the map layer: placed by anchors (the sample's `anchors`), not by the layout. */
const MAP_PARTS = ["city-banner", "unit-flag"];

async function ui2(page) {
  return page.evaluate(() => (window.__civ5test?.ui2 ? window.__civ5test.ui2() : null)).catch(() => null);
}

const vpName = ([w, h], dsf) => `${w}x${h}${dsf === 1 ? "" : `@${dsf}`}`;

/** Runs in the preview page: the part names in document order and, for every node under the first element of a
 * name (that element included), `node|keys` -> its nearest part (what `Runtime::find_part` and `dump_rects` reach). */
function partsInPage() {
  const first = new Map();
  for (const el of document.querySelectorAll("[data-part]")) {
    const name = el.getAttribute("data-part");
    if (!first.has(name)) first.set(name, el);
  }
  const tops = [...first.values()];
  const reached = {};
  for (const el of document.querySelectorAll("[data-ui2-node]")) {
    if (!tops.some((t) => t.contains(el))) continue;
    reached[`${el.getAttribute("data-ui2-node")}|${el.getAttribute("data-ui2-inst") || ""}`] = el.closest("[data-part]").getAttribute("data-part");
  }
  return { names: [...first.keys()], reached };
}

/** Chrome's side: the converter's preview server and dump, plus which part each node is in. */
async function chromeSide(skin, fontsDir) {
  const { startServer } = await import("../../ui-convert/node/server.mjs");
  const { launchChromium } = await import("../../ui-convert/node/chromium.mjs");
  const { rectsOf } = await import("../../ui-convert/node/rects.mjs");
  const { entryUrl, loadPage } = await import("../../ui-convert/node/ir.mjs");
  // The preview keys loop instances by the schema's list keys, as the runtime does (`ui2_convert::list_keys`).
  const server = await startServer({ root: path.join(ROOT, "skins", skin), fontsDir, preview: true, listKeys: listKeys() });
  const browser = await launchChromium();
  const guarded = async (viewport, dsf) => {
    const ctx = await browser.newContext({ viewport, deviceScaleFactor: dsf, serviceWorkers: "block" });
    // Nothing leaves the local server.
    await ctx.route("**/*", (r) => (r.request().url().startsWith(`${server.url}/`) || /^(data|blob|about):/.test(r.request().url()) ? r.continue() : r.abort("blockedbyclient")));
    return ctx;
  };
  const api = { newContext: guarded, server: () => server, entry: () => "index.html" };
  return {
    async dump(state, [w, h], dsf) {
      const d = await rectsOf({ session: api, req: { state, viewport: [w, h], dsf }, skin });
      const ctx = await guarded({ width: w, height: h }, dsf);
      try {
        const page = await ctx.newPage();
        await loadPage(page, entryUrl(server, "index.html", "layout", state));
        await page.addStyleTag({ content: "*{transition:none!important}" });
        return { rects: d.rects, parts: await page.evaluate(partsInPage) };
      } finally {
        await ctx.close();
      }
    },
    async close() {
      await browser.close().catch(() => {});
      await server.close();
    },
  };
}

/** Our records: `ui2Rects` of every part name, merged (a record may be under several parts). */
async function ourRects(page, parts) {
  const rects = await page.evaluate((p) => window.__civ5test.ui2Rects(p, 20000), parts);
  const out = new Map();
  for (const r of rects) {
    const k = `${r.node}|${r.keys}`;
    if (!out.has(k)) out.set(k, { r: [r.x, r.y, r.w, r.h], parts: [r.part] });
    else out.get(k).parts.push(r.part);
  }
  return out;
}

/** The rectangles once two dumps in a row are the same (the layout settled after a viewport change). */
async function settledRects(page, parts) {
  let last = null;
  for (let k = 0; k < 20; k++) {
    await sleep(k === 0 ? 800 : 300);
    const now = await ourRects(page, parts);
    const sig = JSON.stringify([...now.entries()]);
    if (last === sig) return now;
    last = sig;
  }
  return ourRects(page, parts);
}

/**
 * The list keys of the model schema for the preview, as `ui2_convert::list_keys` gives them to it: schema path of
 * every list -> `{field, kind, variants?}` (kind `key`, `int`, `uint`, `enum` or `str`), from `schema.gen.json`.
 */
export function listKeys() {
  const s = JSON.parse(readFileSync(path.join(ROOT, "crates/shared/ui2-model/gen/schema.gen.json"), "utf8"));
  const types = new Map(s.types.map((t) => [t.name, t]));
  const enums = new Map(s.enums.map((e) => [e.name, e]));
  const out = {};
  for (const p of s.paths) {
    const m = /^list<(.+)>$/.exec(p.type ?? "");
    if (!m || !p.list_key) continue;
    const ty = types.get(m[1])?.fields?.find((f) => f.name === p.list_key)?.type;
    if (!ty) continue;
    const info = { field: p.list_key };
    if (/^key</.test(ty)) info.kind = "key";
    else if (["i8", "i16", "i32"].includes(ty)) info.kind = "int";
    else if (ty === "u32") info.kind = "uint";
    else if (ty === "str") info.kind = "str";
    else if (enums.has(ty)) {
      info.kind = "enum";
      info.variants = enums.get(ty).variants;
    } else continue;
    out[p.path] = info;
  }
  return out;
}

function fontsDirOf(skin) {
  const dir = path.join(ROOT, "target", "ui2", skin, "fonts");
  if (existsSync(dir) && readdirSync(dir).length) return dir;
  const r = spawnSync("cargo", ["run", "-q", "-p", "ui2-convert", "--", "build", path.join("skins", skin)], { cwd: ROOT, stdio: ["ignore", "inherit", "inherit"] });
  if (r.status !== 0 || !existsSync(dir)) throw new Error(`no font subsets in ${dir} (cargo run -p ui2-convert -- build skins/${skin} failed)`);
  return dir;
}

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "ui2-layout", pass: false, logs: o.logDir, checks: [], numbers: { table: [] }, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const base = o.attach ?? ctx.server?.url;
  const skin = process.env.UI2_LAYOUT_SKIN ?? "engraved";
  const samples = readdirSync(path.join(ROOT, "skins", skin, "sample")).filter((f) => f.endsWith(".json")).map((f) => f.slice(0, -5)).sort();
  const states = process.env.UI2_LAYOUT_STATES ? process.env.UI2_LAYOUT_STATES.split(",").map((s) => s.trim()).filter(Boolean) : samples;
  let chrome = null;
  let b = null;
  try {
    chrome = await chromeSide(skin, fontsDirOf(skin));
    b = await openPage({ headed: o.headed, logDir: o.logDir, viewport: { width: 1280, height: 720 }, consoleFile: "console-ui2-layout.log" });
    const cdp = await b.page.context().newCDPSession(b.page);
    // The sample mode needs no server connection; without this the page signs in as a guest with no password again
    // and again, which a dev server counts as failed logins and then refuses every login from this address.
    await b.page.routeWebSocket(/.*/, (ws) => ws.close());
    for (const state of states) {
      // One load per state; the viewports change on the same page.
      await cdp.send("Emulation.setDeviceMetricsOverride", { width: VIEWPORTS[0][0][0], height: VIEWPORTS[0][0][1], deviceScaleFactor: VIEWPORTS[0][1], mobile: false });
      await b.page.goto(appUrl(base, o, { ui: `v2:${skin}`, ui2sample: state, ui2bg: "202020" }));
      const t0 = Date.now();
      for (;;) {
        const p = await ui2(b.page);
        if (p?.state === "classic") throw new Error(`${state}: UI v2 fell back to the classic UI (see console-ui2-layout.log)`);
        if (p?.state === "running" && p.frames > 10) break;
        if (Date.now() - t0 > UP_MS) throw new Error(`${state}: UI v2 is not running after ${UP_MS / 1000} s`);
        await sleep(250);
      }
      for (const [[w, h], dsf] of VIEWPORTS) {
        const label = `${state} ${vpName([w, h], dsf)}`;
        await cdp.send("Emulation.setDeviceMetricsOverride", { width: w, height: h, deviceScaleFactor: dsf, mobile: false });
        const c = await chrome.dump(state, [w, h], dsf);
        const names = c.parts.names;
        const ours = await settledRects(b.page, names);
        const row = compare(c, ours, names);
        row.state = state;
        row.viewport = vpName([w, h], dsf);
        result.numbers.table.push(row);
        const off = row.off.length;
        // The map layer (city banners, unit flags) apart: the hook reaches them only where the host placed them.
        const isMap = (k) => MAP_PARTS.some((p) => k.endsWith(`[${p}]`));
        const missUi = row.missing.filter((k) => !isMap(k));
        const missMap = row.missing.filter(isMap);
        check(`${label}: ${row.compared} records within ${TOLERANCE} px, none missing or extra`,
          off === 0 && missUi.length === 0 && row.extra.length === 0,
          `worst ${row.worst.toFixed(3)} px${row.worstAt ? ` (${row.worstAt})` : ""}; ${off} off, ${missUi.length} missing (${missUi.slice(0, 3).join(", ")}), ${row.extra.length} extra (${row.extra.slice(0, 3).join(", ")}); ${row.parts} parts`);
        check(`${label}: the map layer's banners and flags are laid out as in Chrome`, missMap.length === 0,
          `${missMap.length} record(s) of ${MAP_PARTS.join(", ")} not on the page (${missMap.slice(0, 3).join(", ")})`);
        row.missingMap = missMap.length;
        if (off || row.missing.length || row.extra.length) {
          const lines = [`${label}: worst ${row.worst} px`, ...row.off.slice(0, 30).map((x) => `${x.delta.toFixed(3)} ${x.key} [${x.part}] ours ${JSON.stringify(x.ours)} chrome ${JSON.stringify(x.chrome)}`),
            `missing: ${row.missing.join(" ")}`, `extra: ${row.extra.join(" ")}`];
          writeFileSync(path.join(o.logDir, `ui2-layout-${state}-${row.viewport}.txt`), lines.join("\n") + "\n");
        }
        // Only the summary goes into the JSON.
        row.off = row.off.slice(0, 5);
        row.missing = row.missing.slice(0, 10);
        row.extra = row.extra.slice(0, 10);
      }
      // SAMPLE-MODE FAULT: the page's lobby (`#civ5-ui`, the sign-in form) stays over the canvas in the sample mode;
      // hidden for the picture only (the rectangles do not depend on it).
      await b.page.addStyleTag({ content: "body > :not(canvas){display:none!important}" }).catch(() => {});
      await b.page.screenshot({ path: path.join(o.logDir, `ui2-layout-${state}.png`) }).catch(() => {});
    }
  } catch (e) {
    check("no scenario error", false, e.message.split("\n")[0]);
    result.error = e.stack ?? String(e);
  }
  if (b) {
    const errs = b.errors.filter((e) => !e.includes("/pkg-mt/"));
    result.consoleErrors.push(...errs);
    check("no console errors", errs.length === 0, `${errs.length} error(s)${errs[0] ? `: ${errs[0]}` : ""}`);
    await b.close();
  }
  if (chrome) await chrome.close();
  try { writeFileSync(path.join(o.logDir, "ui2-layout.json"), JSON.stringify(result.numbers, null, 2)); } catch { /* no log dir */ }
  result.pass = result.checks.length > 0 && result.checks.every((x) => x.pass);
  result.durationMs = Date.now() - started;
  return result;
}

/** Chrome's records the hook reaches (see `partsInPage`; a text leaf `n#k` goes with its element `n`) against ours. */
function compare(c, ours, names) {
  const row = { compared: 0, worst: 0, worstAt: null, off: [], missing: [], extra: [], parts: names.length };
  const seen = new Set();
  for (const [node, keys, x, y, w, h] of c.rects) {
    const part = c.parts.reached[`${node.split("#")[0]}|${keys}`];
    if (!part) continue;
    const key = `${node}|${keys}`;
    const mine = ours.get(key);
    if (!mine) {
      row.missing.push(`${key} [${part}]`);
      continue;
    }
    seen.add(key);
    row.compared++;
    const theirs = [x, y, w, h];
    const delta = Math.max(...theirs.map((v, i) => Math.abs(v - mine.r[i])));
    if (delta > row.worst) {
      row.worst = delta;
      row.worstAt = `${key} [${part}]`;
    }
    if (delta > TOLERANCE) row.off.push({ key, part, delta, ours: mine.r, chrome: theirs });
  }
  row.off.sort((a, b) => b.delta - a.delta);
  // Ours that Chrome does not have under the same parts (shown by us, hidden or absent in Chrome).
  for (const k of ours.keys()) if (!seen.has(k)) row.extra.push(k);
  return row;
}
