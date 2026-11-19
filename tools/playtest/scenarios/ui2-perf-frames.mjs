// The frame-cost rows of `ui2-perf` (`UI2_PERF_ROWS=frames` runs only these): main-thread CPU per frame of the classic UI
// against UI v2 in ONE game (same state, same camera: the UI is switched with the test port's `setUi`), 1920 x 1080 at
// DPR 1, with only the HUD and with each of the windows of `WINDOWS` open, idle and while the mouse moves (the technology
// tree also while it scrolls). See docs-dev/ui2-perf.md.
//
// Per phase (`PHASE_S` seconds, after the window settled):
// - `frame.cpu_ms` of `__civ5stats` at the end of the phase: the p50 of the app's whole frame (update, UI, encoding) over
//   the renderer's last 10 s: the classic/v2 comparison;
// - UI v2's own per-frame phases (`__civ5test.ui2Frames()`, a ring the client fills each UI v2 frame): source, runtime
//   (bind, style, text, layout, keys, paint), glyph resolve, scripts' main-thread steps, anchor placement, the
//   renderer's take of the scene (prepare) and the test probe; with the work counters (bindings evaluated, nodes
//   restyled, layout runs, chunks rebuilt, nodes visited, bytes uploaded). A build without the ring (older bundles)
//   falls back to the `ui2.*` counters of `__civ5stats` sampled twice a second (single frames: indicative).
// - the GPU side: what the `Ui2` pass draws per frame (draw calls, instances, the megapixels its quads cover, of them
//   blurred box shadows and textured quads) and its GPU time from timestamp queries where the device has them
//   (WebGPU; Playwright's own Chromium falls back to WebGL2 here, which has none: `CHROMIUM_PATH` at an installed
//   Chrome gives WebGPU), with the frame's and the other passes' GPU times from `__civ5stats`.
// Budgets (`FRAME_LIMITS`): generous multiples of the targets (an idle HUD under 0.5 ms over the classic UI, the open
// technology tree under 1 ms, scrolling it under 3 ms), so a loaded machine does not flake; the targets themselves are
// reported beside. The GPU budgets (`GPU_LIMITS`) are counts that do not depend on the machine (draw calls, fill per
// screen area) and, where timestamps exist, the pass's GPU time.
// Env: UI2_PERF_DSF (device scale, default 1), UI2_PERF_VIEWPORT (`1920x1080`, CSS px), UI2_PERF_UNCAPPED=1 (no vsync,
// no frame limit: the frame rate is what the CPU or the GPU allows), UI2_PERF_PHASE_S (12).
import { act, until, state, openPage, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { ensureCity } from "../lib/screen-city.mjs";
import { pct, round } from "../lib/bench.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const [vpw, vph] = (process.env.UI2_PERF_VIEWPORT ?? "1920x1080").split("x").map(Number);
export const VP = { width: vpw || 1920, height: vph || 1080 };
const DSF = Number(process.env.UI2_PERF_DSF ?? 1);
const UNCAPPED = process.env.UI2_PERF_UNCAPPED === "1";
const SKIN = process.env.UI2_PERF_SKIN ?? "engraved";
/** Seconds per phase: `frame.cpu_ms` is the p50 of the last 10 s, so a phase is a little longer. */
const PHASE_S = Number(process.env.UI2_PERF_PHASE_S ?? 12);
/** The windows measured, with the text part that says their content is there. */
export const WINDOWS = [
  { id: "techs", ready: "techs.name", min: 50 },
  { id: "city", ready: "city.name", min: 1, city: true },
  { id: "policies", ready: null },
  { id: "diplo", ready: null },
  { id: "economy", ready: null },
  { id: "trade_routes", ready: null },
];

/** Budgets of the UI v2 frame (its own phases, mean ms per frame), and of the overhead over the classic UI. */
export const FRAME_LIMITS = {
  // targets (reported): 0.5 / 1 / 3 ms
  hudIdleMs: 1.5,
  windowIdleMs: 3,
  techsScrollMs: 9,
  mouseMs: 6,
};

/** GPU budgets of the `Ui2` pass: draw calls, the fill as screens' worth of pixels, and the GPU time (timestamps). */
export const GPU_LIMITS = {
  draws: 64,
  hudFillScreens: 1.5,
  windowFillScreens: 6,
  gpuMs: 4,
};

const FIELDS = ["app", "cpu", "source", "runtime", "bind", "style", "text", "layout", "keys", "paint", "resolve", "js", "input", "place", "prepare", "probe"];
const COUNTS = ["binds", "restyled", "layoutRuns", "chunks", "visited", "uploadKb", "ops", "inputEvents", "draws", "instances", "fillKpx", "shadowKpx", "textureKpx", "gpuMs"];

function stat(xs) {
  const v = xs.filter((x) => typeof x === "number" && Number.isFinite(x));
  if (!v.length) return null;
  const s = v.slice().sort((a, b) => a - b);
  return { n: v.length, mean: round(v.reduce((a, b) => a + b, 0) / v.length, 3), p50: round(pct(s, 50), 3), p95: round(pct(s, 95), 3), max: round(s[s.length - 1], 3) };
}

/** `ui2Frames(reset)` of the client, or null on a build without the ring. */
async function ring(page, reset) {
  return page.evaluate((r) => (typeof window.__civ5test?.ui2Frames === "function" ? window.__civ5test.ui2Frames(r) : null), reset).catch(() => null);
}

/** The counters of `__civ5stats` the rows use (published once a second with `stats=1`). */
async function counters(page) {
  return page.evaluate(() => {
    const c = window.__civ5stats?.counters ?? {};
    const out = {};
    for (const [k, v] of Object.entries(c)) if (k.startsWith("frame.") || k.startsWith("ui2.")) out[k] = v;
    // GPU times (timestamp queries, a few frames old; -1 or absent without them) of the whole frame and per pass.
    const f = window.__civ5stats?.frame;
    if (f && f.gpu_ms >= 0) out["gpu.frame_ms"] = f.gpu_ms;
    for (const [name, p] of Object.entries(window.__civ5stats?.passes_full ?? {})) {
      if (p && p.gpu_ms >= 0) out[`gpu.${name}_ms`] = p.gpu_ms;
      if (p && name === "Ui2") { out["gpu.Ui2_draws"] = p.draws; out["gpu.Ui2_instances"] = p.instances; }
    }
    return out;
  }).catch(() => ({}));
}

/** The mouse goes round a figure over `box` (CSS px) until `stop()`: about one move per frame. */
function moveMouse(page, box) {
  let on = true;
  const done = (async () => {
    for (let i = 0; on; i++) {
      const t = i / 60;
      const x = box.x + box.w * (0.5 + 0.45 * Math.sin(t * 1.3));
      const y = box.y + box.h * (0.5 + 0.45 * Math.sin(t * 2.1 + 0.7));
      await page.mouse.move(Math.round(x), Math.round(y)).catch(() => {});
      await sleep(14);
    }
  })();
  return async () => {
    on = false;
    await done;
  };
}

/** The wheel over `at` until `stop()`: horizontal and vertical steps, turning round every second. */
function scrollWheel(page, at) {
  let on = true;
  const done = (async () => {
    await page.mouse.move(at.x, at.y).catch(() => {});
    for (let i = 0; on; i++) {
      const dir = Math.floor(i / 30) % 2 === 0 ? 1 : -1;
      await page.mouse.wheel(0, 60 * dir).catch(() => {});
      await sleep(30);
    }
  })();
  return async () => {
    on = false;
    await done;
  };
}

/** One measured phase: `action` runs for `PHASE_S` s; returns the frame's p50 CPU, fps and the UI v2 phases. */
async function phase(page, label, action) {
  await ring(page, true);
  const samples = [];
  const stop = action ? action() : null;
  const t0 = Date.now();
  while (Date.now() - t0 < PHASE_S * 1000) {
    await sleep(500);
    samples.push(await counters(page));
  }
  if (stop) await stop();
  const end = samples[samples.length - 1] ?? {};
  const r = await ring(page, true);
  const out = { label, frameCpuP50: end["frame.cpu_ms"] ?? null, fps: end["frame.fps"] ?? null, via: r ? "ring" : "counters" };
  // GPU: the frame and the passes, sampled twice a second (means of the samples that had a time).
  out.gpu = {};
  // The fill per layer of the last sample (kpx; a build without the counters has none).
  out.fillByLayer = Object.fromEntries(["map", "hud", "window", "tooltip", "shadow"].map((l) => [l, end[`ui2.fill_kpx.${l}`] ?? null]));
  for (const k of new Set(samples.flatMap((s) => Object.keys(s).filter((x) => x.startsWith("gpu."))))) out.gpu[k.slice(4)] = stat(samples.map((s) => s[k]))?.mean ?? null;
  if (r && r.frames?.length) {
    out.ui2Frames = r.frames.length;
    for (const f of FIELDS) out[f] = stat(r.frames.map((x) => x[f]));
    for (const c of COUNTS) out[c] = stat(r.frames.map((x) => x[c]));
    out.total = stat(r.frames.map((x) => (x.cpu ?? 0) + (x.input ?? 0) + (x.place ?? 0) + (x.prepare ?? 0) + (x.probe ?? 0)));
  } else {
    // An older bundle: single-frame snapshots of the `ui2.*` counters.
    const of = (k) => stat(samples.map((s) => s[k]));
    out.cpu = of("ui2.cpu_ms");
    out.source = of("ui2.source_ms");
    out.runtime = of("ui2.runtime_ms");
    out.resolve = of("ui2.resolve_ms");
    out.place = of("ui2.place_ms");
    out.prepare = of("ui2.prepare_ms");
    out.js = of("ui2.js.main_ms");
    out.binds = of("ui2.binds");
    out.chunks = of("ui2.chunks_rebuilt");
    out.total = stat(samples.map((s) => (s["ui2.cpu_ms"] ?? 0) + (s["ui2.place_ms"] ?? 0) + (s["ui2.prepare_ms"] ?? 0)));
  }
  return out;
}

async function probe(page) {
  return page.evaluate(() => (window.__civ5test?.ui2 ? window.__civ5test.ui2() : null)).catch(() => null);
}

async function waitProbe(page, pred, ms, what) {
  const t0 = Date.now();
  for (;;) {
    const p = await probe(page);
    if (p && pred(p)) return p;
    if (Date.now() - t0 > ms) throw new Error(`timed out waiting for ${what}`);
    await sleep(250);
  }
}

const texts = (p, part) => (p?.texts ?? []).filter((t) => t.part === part);

async function intent(page, name, args) {
  await page.evaluate(([n, a]) => window.__civ5test.ui2Intent({ name: n, args: a }), [name, args]);
}

/** Signs in to a fresh small game (seed 7, 2 AIs) under the classic UI. */
async function enterGame(page, base, o) {
  for (let attempt = 0; ; attempt++) {
    const nick = `pfr${(Date.now() + attempt) % 1000000}`;
    await page.goto(appUrl(base, o, { nick, autostart: "1", ai: "2", size: "small", seed: "7", mode: "simultaneous", manual: "1", stats: "1" }));
    try {
      const s = await until(page, (x) => x.screen === "login" || x.screen === "play", 90000, "sign-in window or the game");
      if (s.screen === "login") await act(page, { kind: "login", nick, password: DEV_PASSWORD });
      await until(page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 120000, "the game");
      return nick;
    } catch (e) {
      if (attempt >= 2) throw e;
    }
  }
}

/** Waits until the terrain in view is drawn (max 120 s) and a few seconds more. */
async function settleMap(page) {
  const t0 = Date.now();
  while (Date.now() - t0 < 120000) {
    const v = await page.evaluate(() => window.__civ5stats?.counters?.["terrain.view_ready"] ?? null).catch(() => null);
    if (v === 1) break;
    await sleep(250);
  }
  await sleep(4000);
}

/** The classic UI's technology tree (the original popup, through the UI's Lua). */
async function classicTechTree(page, open) {
  const code = open
    ? "Events.SerialEventGameMessagePopup({ Type = ButtonPopupTypes.BUTTONPOPUP_TECH_TREE, Data2 = -1 })"
    : "UIManager:DequeuePopup(ContextPtr:LookUpControl('/InGame/TechTree'))";
  await page.evaluate((c) => window.__civ5test.lua?.(c), code).catch(() => {});
}

/**
 * The frame rows. Returns `{ meta, phases: {name: phase}, rows: [{metric, value, limit, verdict}] }`; `check(name, pass,
 * detail)` gets one call per budget.
 */
export async function frameCost(base, o, check) {
  const b = await openPage({
    headed: o.headed, logDir: o.logDir, viewport: VP, consoleFile: "console-ui2-perf-frames.log", uncapped: UNCAPPED,
    ...(DSF === 1 ? {} : { deviceScaleFactor: null, chromeArgs: [`--force-device-scale-factor=${DSF}`] }),
  });
  const page = b.page;
  const res = { meta: { viewport: VP, dsf: DSF, uncapped: UNCAPPED, browser: process.env.CHROMIUM_PATH ? "chrome" : "playwright", phaseS: PHASE_S, skin: SKIN }, phases: {}, rows: [], errors: [] };
  const map = { x: 560, y: 260, w: 800, h: 520 };
  const full = { x: 40, y: 40, w: VP.width - 80, h: VP.height - 80 };
  try {
    await enterGame(page, base, o);
    const city = await ensureCity(page);
    await settleMap(page);
    res.meta.dpr = await page.evaluate(() => devicePixelRatio);
    res.meta.canvas = await page.evaluate(() => [...document.querySelectorAll("canvas")].map((c) => [c.width, c.height]).find(([w]) => w > 0) ?? null);
    res.meta.backend = await page.evaluate(() => window.__civ5stats?.backend ?? null).catch(() => null);
    res.meta.threads = await page.evaluate(() => window.__civ5threads?.mode ?? "single").catch(() => null);
    // ---- the classic UI
    res.phases["classic hud idle"] = await phase(page, "classic hud idle", null);
    res.phases["classic hud mouse"] = await phase(page, "classic hud mouse", () => moveMouse(page, map));
    await classicTechTree(page, true);
    await sleep(3000);
    res.phases["classic techs idle"] = await phase(page, "classic techs idle", null);
    res.phases["classic techs mouse"] = await phase(page, "classic techs mouse", () => moveMouse(page, full));
    await classicTechTree(page, false);
    await page.keyboard.press("Escape").catch(() => {});
    await sleep(2000);
    // ---- UI v2 in the same game
    await act(page, { kind: "setUi", value: `v2:${SKIN}` });
    await waitProbe(page, (p) => p.state === "running" && p.frames > 30 && texts(p, "top.yield.gold").length > 0, 180000, "UI v2 running");
    await sleep(4000);
    res.phases["v2 hud idle"] = await phase(page, "v2 hud idle", null);
    res.phases["v2 hud mouse"] = await phase(page, "v2 hud mouse", () => moveMouse(page, map));
    for (const w of WINDOWS) {
      try {
        await intent(page, "window.open", w.city ? { window: w.id, a: city, b: 0 } : { window: w.id });
        await waitProbe(page, (p) => (p.ui?.windows ?? []).includes(w.id) && (!w.ready || texts(p, w.ready).length >= w.min), 30000, `${w.id} window`);
        await sleep(2500);
        res.phases[`v2 ${w.id} idle`] = await phase(page, `v2 ${w.id} idle`, null);
        res.phases[`v2 ${w.id} mouse`] = await phase(page, `v2 ${w.id} mouse`, () => moveMouse(page, full));
        if (w.id === "techs") res.phases["v2 techs scroll"] = await phase(page, "v2 techs scroll", () => scrollWheel(page, { x: VP.width / 2, y: VP.height / 2 }));
      } catch (e) {
        res.errors.push(`${w.id}: ${String(e.message ?? e).split("\n")[0]}`);
      }
      await intent(page, "window.close", { window: w.id }).catch(() => {});
      await page.mouse.move(VP.width / 2, VP.height / 2).catch(() => {});
      await sleep(1500);
    }
    // ---- back to the classic UI: the same scene (a drift of the game or the camera shows here)
    await act(page, { kind: "setUi", value: "classic" });
    await sleep(5000);
    res.phases["classic hud idle (end)"] = await phase(page, "classic hud idle (end)", null);
  } catch (e) {
    res.error = String(e.stack ?? e);
  } finally {
    res.consoleErrors = b.errors.filter((e) => !e.includes("/pkg-mt/")).slice(0, 10);
    await b.close();
  }
  // ---- the rows
  const P = res.phases;
  // Device pixels of the screen.
  const pixels = (res.meta.canvas ? res.meta.canvas[0] * res.meta.canvas[1] : VP.width * VP.height * DSF * DSF) || 1;
  const classic = P["classic hud idle"]?.frameCpuP50 ?? null;
  const classicMouse = P["classic hud mouse"]?.frameCpuP50 ?? null;
  const ms = (v) => (v === null || v === undefined ? "n/a" : `${round(v, 2)} ms`);
  const row = (metric, value, limit, ok, note) => {
    res.rows.push({ id: "frame", metric, value, limit, verdict: ok === null ? "n/a" : ok ? "pass" : "FAIL", note: note ?? "" });
    if (ok !== null) check(`frame ${metric}`, ok, `${value} (limit ${limit})${note ? `; ${note}` : ""}`);
  };
  for (const [name, p] of Object.entries(P)) {
    if (!name.startsWith("v2 ")) continue;
    const base0 = name.endsWith("mouse") ? classicMouse : classic;
    const extra = p.frameCpuP50 !== null && base0 !== null ? p.frameCpuP50 - base0 : null;
    const ui2 = p.total?.mean ?? null;
    let limit = null;
    if (name === "v2 hud idle") limit = FRAME_LIMITS.hudIdleMs;
    else if (name === "v2 techs scroll") limit = FRAME_LIMITS.techsScrollMs;
    else if (name.endsWith("idle")) limit = FRAME_LIMITS.windowIdleMs;
    else if (name.endsWith("mouse")) limit = FRAME_LIMITS.mouseMs;
    const parts = ["source", "runtime", "keys", "resolve", "js", "input", "place", "prepare", "probe"].map((f) => `${f} ${p[f]?.mean ?? "-"}`).join(", ");
    row(`${name}: UI v2 frame (own phases, mean)`, `${ms(ui2)} (${parts}); frame.cpu_ms p50 ${ms(p.frameCpuP50)}, over classic ${ms(extra)}`, limit === null ? "report" : `<= ${limit} ms`, limit === null || ui2 === null ? null : ui2 <= limit, `${p.ui2Frames ?? "?"} frames (${p.via})`);
    // The GPU side (a build without it has no `draws` in the ring).
    if (p.draws) {
      const screens = (p.fillKpx?.mean ?? 0) * 1000 / pixels;
      const fillLimit = name.startsWith("v2 hud") ? GPU_LIMITS.hudFillScreens : GPU_LIMITS.windowFillScreens;
      row(`${name}: Ui2 draw calls`, `${p.draws.mean} (${p.instances?.mean ?? "?"} instances)`, `<= ${GPU_LIMITS.draws}`, p.draws.mean <= GPU_LIMITS.draws);
      row(`${name}: Ui2 fill`, `${round(screens, 2)} screens (${round((p.fillKpx?.mean ?? 0) / 1000, 2)} Mpx; shadows ${round((p.shadowKpx?.mean ?? 0) / 1000, 2)}, textured ${round((p.textureKpx?.mean ?? 0) / 1000, 2)})`, `<= ${fillLimit} screens`, screens <= fillLimit);
      const gpu = p.gpuMs?.mean ?? p.gpu?.Ui2_ms ?? null;
      row(`${name}: Ui2 GPU time`, gpu === null ? "no timestamps" : ms(gpu), `<= ${GPU_LIMITS.gpuMs} ms`, gpu === null ? null : gpu <= GPU_LIMITS.gpuMs);
    }
  }
  if (res.error) check("frame rows ran", false, res.error.split("\n")[0]);
  return res;
}

/** The table of the phases as text (one line per phase). */
export function frameTable(res) {
  const lines = [];
  const f = (x) => (x === null || x === undefined ? "-" : String(round(x, 3)));
  lines.push(["phase", "frame.cpu p50", "fps", "ui2 total", "cpu", "source", "runtime", "bind", "style", "text", "layout", "keys", "paint", "resolve", "js", "input", "place", "prepare", "probe", "binds", "chunks", "upload kB", "draws", "instances", "fill Mpx", "shadow Mpx", "textured Mpx", "ui2 gpu", "frame gpu"].join(" | "));
  for (const [name, p] of Object.entries(res.phases)) {
    const mpx = (k) => (p[k]?.mean === undefined ? null : p[k].mean / 1000);
    lines.push([name, f(p.frameCpuP50), f(p.fps), f(p.total?.mean), ...["cpu", "source", "runtime", "bind", "style", "text", "layout", "keys", "paint", "resolve", "js", "input", "place", "prepare", "probe", "binds", "chunks", "uploadKb", "draws", "instances"].map((k) => f(p[k]?.mean)), f(mpx("fillKpx")), f(mpx("shadowKpx")), f(mpx("textureKpx")), f(p.gpuMs?.mean ?? p.gpu?.Ui2_ms), f(p.gpu?.frame_ms)].join(" | "));
  }
  return lines.join("\n");
}
