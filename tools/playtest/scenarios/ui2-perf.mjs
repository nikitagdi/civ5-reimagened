// `ui2-perf`: the performance table of UI v2 (`engraved` over a live game), in one run:
//   cargo xtask --gamecore-lib <dir> playtest --scenario ui2-perf --attach <url of a real-core server> [--headed] [--release]
//     [--late-url <url> --late-nick <nick> --late-token <file>] [--perf-idle-s 10]   (env UI2_PERF_TURNS: 20 turns)
// The numbers that count are taken headed, from a release bundle, at the machine's own console: a remote desktop
// session changes the frame pacing and the GPU path. Every run labels itself (`meta`: headed, profile, remote session),
// and the rows that depend on the GPU or the frame rate say "indicative" when the run is not of that kind.
//
// Rows (each with its threshold; the scenario fails when a measured row misses it):
// - idle: `--perf-idle-s` (10) s without input: p95 of `ui2.cpu_ms` (the host's whole UI v2 frame) over the frames;
// - turns: `UI2_PERF_TURNS` (20) end turns (forced through the test port): `ui2.cpu_ms` of the frames that carried a model
//   change (`ui2.model.ops` > 0) from the end turn until the new turn is on the dial, mean and peak;
// - draw calls of the HUD: `ui2.draws.hud` (the renderer's count) and the draws of the `Ui2` row of `__civ5stats`;
// - map banners: the frame rate with the map layer against `&ui2onmap=0`. Needs the skin's `map` layer: without it
//   (no `ui2.draws.map`, no `city-banner` part) the row reports "not available" and is not a failure. With it, the
//   heavy fixture scene (`CIV5_FIXTURE_HEAVY`, the bench's scene) when UI v2 runs there, else the live game;
// - reaction: the frame (counted in UI v2 frames) on which the dial shows the new turn after the replica has it;
// - package: the skin package's size, `ui2.load_ms`, the client wasm (raw and brotli; the text stack's share is not
//   measured here: it needs a build without it);
// - scripts: `ui2.js.worker_ms` and `ui2.js.main_ms` per frame, the scripts' heap (memory by realm, needs the
//   cross-origin isolated page), the peak GC pause of the dedicated workers (a trace of a few turns), the skin's JS size.
// The game: a fresh small game whose settler founds a city and whose soldier steps out of it; before each turn (outside
// the measured window) the skin settles what blocks it (unit orders, production, research), a new unit on an occupied
// tile steps aside. A blocker the skin has no screen for (a policy choice) stops the turns: the table says how many ran.
// The frame data come from `__civ5test.ui2()` read once per animation frame (only new UI v2 frames are kept): that read
// costs the page some time of its own, so the frame rate of these phases is not the bench's.
// Output: `<logs>/ui2-perf.json` (meta, raw series summaries, `table`), the same table printed, and `numbers.table`.
import { execFileSync } from "node:child_process";
import { readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import path from "node:path";
import zlib from "node:zlib";
import { fileURLToPath } from "node:url";
import os from "node:os";
import { openPage, act, until, state, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD, moveAUnit } from "../lib/game.mjs";
import { FOUND_CITY } from "../lib/policy.mjs";
import { frames, pct, round } from "../lib/bench.mjs";
import { startServer } from "../lib/server.mjs";
import { gcPauses } from "./ui2-n6.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const UP_MS = Number(process.env.UI2_UP_MS ?? 420000);
const SKIN = process.env.UI2_PERF_SKIN ?? "engraved";
const VP = { width: 1600, height: 900 };
const REPO = fileURLToPath(new URL("../../../", import.meta.url));

/** The thresholds of the table. */
export const LIMITS = {
  idleP95Ms: 0.45,
  turnMeanMs: 3,
  turnPeakMs: 4,
  hudDraws: 30,
  bannerFpsDropPct: 7.5,
  reactionFrame: 2,
  packageBytes: 3 * 1024 * 1024,
  loadMs: 1500,
  jsFrameMs: 1.5,
  heapMb: 48,
  gcMs: 2,
  jsKb: 750,
};

/** A remote desktop session (Windows): frame pacing and the GPU path are not those of the console. */
function remoteSession() {
  const s = process.env.SESSIONNAME ?? "";
  if (/^RDP-/i.test(s)) return s;
  if (process.platform !== "win32") return null;
  // A process started outside the desktop's shell may have no SESSIONNAME: `query session` marks the current one
  // with ">" (and exits with 1 even when it answers).
  let out = "";
  try {
    out = execFileSync("query", ["session"], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] });
  } catch (e) {
    out = String(e.stdout ?? "");
  }
  const cur = out.split(/\r?\n/).find((l) => l.startsWith(">"));
  return cur && /^>rdp-/i.test(cur) ? cur.slice(1).trim().split(/\s+/)[0] : null;
}

function stat(xs) {
  if (!xs.length) return { n: 0, mean: null, p50: null, p95: null, max: null };
  const s = xs.slice().sort((a, b) => a - b);
  return { n: xs.length, mean: round(xs.reduce((a, b) => a + b, 0) / xs.length, 3), p50: round(pct(s, 50), 3), p95: round(pct(s, 95), 3), max: round(s[s.length - 1], 3) };
}

/**
 * The in-page sampler: each animation frame it stamps the time and, while `on`, reads `ui2()` and keeps one row per
 * new UI v2 frame (`f`, the probe's frame count). With `withState` the row also has the replica's turn (`state()`)
 * and the turn the dial shows.
 */
async function installSampler(page) {
  await page.evaluate(() => {
    if (window.__ui2perf) return;
    const P = (window.__ui2perf = { on: false, withState: false, rows: [], ts: [], last: -1 });
    const num = (v) => (typeof v === "number" && Number.isFinite(v) ? v : null);
    const tick = (ts) => {
      P.ts.push(ts);
      if (P.on && window.__civ5test?.ui2) {
        try {
          const p = window.__civ5test.ui2();
          if (p && p.frames !== P.last) {
            P.last = p.frames;
            const c = p.counters ?? {};
            // The renderer's draw counts reach `__civ5stats.counters` only (once a second).
            const r = window.__civ5stats?.counters ?? {};
            const row = {
              t: ts,
              f: p.frames,
              rev: p.model?.rev ?? null,
              cpu: num(c["ui2.cpu_ms"]),
              ops: num(c["ui2.model.ops"]),
              source: num(c["ui2.source_ms"]),
              runtime: num(c["ui2.runtime_ms"]),
              jsMain: num(c["ui2.js.main_ms"]),
              jsWorker: num(c["ui2.js.worker_ms"]),
              hud: num(c["ui2.draws.hud"] ?? r["ui2.draws.hud"]),
              map: num(c["ui2.draws.map"] ?? r["ui2.draws.map"]),
              draws: num(c["ui2.draws"] ?? r["ui2.draws"]),
            };
            // The source's profile of the frame (µs per root and per kind of read, readable, pending roots, the
            // game's turn it read): only its non-zero entries.
            const src = {};
            for (const [k, v] of Object.entries(c)) if (k.startsWith("ui2.source.") && v) src[k.slice(11)] = v;
            row.src = src;
            row.step = num(c["ui2.replica.step"]);
            if (P.withState) {
              const s = window.__civ5test.state();
              row.turn = s?.turn ?? null;
              const dial = (p.texts ?? []).filter((x) => x.part === "helm.dial").map((x) => x.text).find((x) => /^\d+$/.test(x));
              row.dial = dial === undefined ? null : Number(dial);
            }
            P.rows.push(row);
          }
        } catch {
          /* the page navigates */
        }
      }
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  });
}

/** Runs `fn` with the sampler on; returns the rows and the rAF stamps of that time. */
async function sampled(page, withState, fn) {
  await page.evaluate((w) => {
    const P = window.__ui2perf;
    P.rows = [];
    P.ts = [];
    P.withState = w;
    P.on = true;
  }, withState);
  const from = await page.evaluate(() => performance.now());
  await fn();
  const to = await page.evaluate(() => performance.now());
  return page.evaluate(([a, b]) => {
    const P = window.__ui2perf;
    P.on = false;
    return { from: a, to: b, rows: P.rows.splice(0), ts: P.ts.splice(0) };
  }, [from, to]);
}

async function ui2(page) {
  return page.evaluate(() => (window.__civ5test?.ui2 ? window.__civ5test.ui2() : null)).catch(() => null);
}

/** The `Ui2` row of `__civ5stats.passes_full` (published once a second with `stats=1`). */
async function ui2Pass(page) {
  return page.evaluate(() => {
    const s = window.__civ5stats;
    const p = s?.passes_full?.Ui2 ?? null;
    return p ? JSON.parse(JSON.stringify(p)) : null;
  }).catch(() => null);
}

/** Signs in and waits for the game (a fresh quick start, or the late save's game with its token). */
async function enterGame(b, base, o, extra, prefix) {
  if (o.lateUrl) {
    const token = readFileSync(o.lateToken, "utf8").trim();
    await b.page.addInitScript(([nick, tok]) => {
      try {
        localStorage.setItem("civ5.nick", nick);
        localStorage.setItem(`civ5.token.${nick}`, tok);
      } catch {
        /* storage blocked */
      }
    }, [o.lateNick, token]);
    await b.page.goto(appUrl(o.lateUrl, o, { nick: o.lateNick, stats: "1", ...extra }));
    await until(b.page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 300000, "the late game");
    return;
  }
  for (let attempt = 0; ; attempt++) {
    const nick = `${prefix}${(Date.now() + attempt) % 1000000}`;
    await b.page.goto(appUrl(base, o, { nick, autostart: "1", ai: "2", size: "small", seed: "7", mode: "simultaneous", manual: "1", stats: "1", ...extra }));
    try {
      const s = await until(b.page, (x) => x.screen === "login" || x.screen === "play", 90000, "sign-in window or the game");
      if (s.screen === "login") await act(b.page, { kind: "login", nick, password: DEV_PASSWORD });
      let inRoom = 0;
      await until(b.page, (x) => {
        inRoom = x.screen === "room" ? inRoom + 1 : 0;
        if (inRoom === 200) b.page.mouse.click(470, 683);
        return x.screen === "play" && x.net?.phase === "play" && x.turn !== null;
      }, 90000, "the game");
      return;
    } catch (e) {
      if (attempt >= 2) throw e;
    }
  }
}

/** Waits until UI v2 runs and its HUD has texts; returns the probe. */
async function waitRunning(page, ms) {
  const t0 = Date.now();
  for (;;) {
    const p = await ui2(page);
    if (p?.state === "classic") throw new Error("UI v2 fell back to the classic UI (see the console log)");
    if (p?.state === "running" && p.frames > 30 && (p.texts ?? []).some((t) => t.part === "top.yield.gold")) return p;
    if ((await page.evaluate(() => window.__civ5stats?.m2?.phase ?? null).catch(() => null)) === -1) throw new Error("the game's layer failed (m2 phase -1)");
    if (Date.now() - t0 > ms) throw new Error(`UI v2 is not running after ${ms / 1000} s: ${JSON.stringify({ state: p?.state, frames: p?.frames })}`);
    await sleep(250);
  }
}

/** The settler founds a city (the test port's order): the game has a city, production and yields to show. */
async function foundCity(page) {
  const settler = ((await state(page))?.units ?? []).find((u) => /settler/i.test(u.name));
  if (!settler) return "no settler";
  await act(page, { kind: "selectUnit", id: settler.id });
  await until(page, (x) => x.selectedUnit === settler.id, 10000, "the settler selected");
  await act(page, { kind: "unitAction", action: FOUND_CITY });
  await until(page, (x) => (x.cities?.length ?? 0) > 0, 30000, "the new city");
  return `found ${settler.x},${settler.y}`;
}

// ---- the skin's layout at 1600 x 900 (as `ui2-actions` clicks it: css/hud.css, css/production.css, css/research.css)
const DIAL = { x: VP.width - 8 - 108, y: VP.height - 392 + 8 + 108 };
const PWIN = { left: 200, top: 60, right: 1160, bottom: VP.height - 56 };
const DETAIL_X = PWIN.right - 156;
const RWIN = { left: (VP.width - 520) / 2, top: 60, head: 64, pad: 8, row: 36 };
/** The production the driver prefers, in order (see `settleBlockers`). */
const PREFER = ["Worker", "Warrior", "Work Boat", "Scout"];
/** What the driver has had built in this run. */
const BUILT = new Set();
const partTexts = (p, part) => (p?.texts ?? []).filter((t) => t.part === part).map((t) => t.text);
const helmState = (p) => partTexts(p, "helm.state")[0] ?? null;
const partsOf = (p, prefix) => [...new Set((p?.texts ?? []).map((t) => t.part).filter((x) => x?.startsWith(prefix)))];

async function waitUi(page, pred, ms) {
  const t0 = Date.now();
  for (;;) {
    const p = await ui2(page);
    if (p && pred(p)) return p;
    if (Date.now() - t0 > ms) return null;
    await sleep(150);
  }
}

/** The centre of a unit order button (rows of 488 px, 56 px buttons, 4 px gaps; Delete 60 px with 8 px more). */
function actButton(acts, which) {
  let x = 0;
  let row = 0;
  const boxes = [];
  for (const a of acts) {
    const lead = a === "unit.act.disband" ? 8 : 0;
    const w = a === "unit.act.disband" ? 60 : 56;
    let at = x === 0 ? lead : x + 4 + lead;
    if (x > 0 && at + w > 488) {
      row += 1;
      at = lead;
    }
    boxes.push({ a, row, x: at, w });
    x = at + w;
  }
  const box = boxes.find((b) => b.a === which);
  return box ? [16 + box.x + box.w / 2, VP.height - 16 - 52 - (row - box.row) * 56 + 26] : null;
}

/**
 * What blocks the end of the turn, settled through the skin the way a player does it (the classic UI sleeps under UI
 * v2 and the test port has no research order): a unit that waits fortifies, sleeps or skips; a city without
 * production builds the first row of the production window; no research: the first technology. Returns the steps.
 */
async function settleBlockers(page) {
  const steps = [];
  for (let guard = 0; guard < 12; guard++) {
    const p = await ui2(page);
    const st = helmState(p);
    if (st === "Unit needs orders") {
      // A new unit on the tile of another one steps aside first (stacked units block the turn whatever their orders).
      const units = (await state(page))?.units ?? [];
      const stacked = units.find((u) => u.movesLeft > 0 && units.some((v) => v.id !== u.id && v.x === u.x && v.y === u.y));
      if (stacked) {
        const moved = await moveAUnit(page, { units: [stacked] }).catch(() => null);
        steps.push(moved ? `moved ${stacked.name}` : `could not move ${stacked.name}`);
        if (moved) continue;
      }
      await page.mouse.click(DIAL.x, DIAL.y);
      const sel = await waitUi(page, (q) => partsOf(q, "unit.act.").length > 0, 5000);
      const acts = partsOf(sel, "unit.act.");
      const pick = ["unit.act.fortify", "unit.act.sleep", "unit.act.skip"].find((a) => acts.includes(a));
      const at = pick && actButton(acts, pick);
      if (!at) return [...steps, `no order for ${JSON.stringify(acts)}`];
      await page.mouse.click(at[0], at[1]);
      await sleep(600);
      steps.push(pick);
    } else if (st === "Choose production") {
      await page.mouse.click(DIAL.x, DIAL.y);
      if (!(await waitUi(page, (q) => partTexts(q, "production.head").length > 0, 5000))) return [...steps, "no production window"];
      // The rows of the list (their root rectangles, from `ui2Rects`), one after the other until a row has the focus
      // (the detail names it instead of its hint); each focused row's name is noted, then the preferred one is built.
      const hint = partTexts(await ui2(page), "production.detail")[0] ?? null;
      const rects = await page.evaluate(() => window.__civ5test.ui2Rects(["production.list"], 5000)).catch(() => []);
      const rowRects = [];
      for (const r of rects ?? []) if (!rowRects.some((x) => x.keys === r.keys)) rowRects.push(r);
      // Only rows inside the window (a list may lay out rows below its scrolled view).
      const visible = rowRects.filter((r) => r.y >= PWIN.top && r.y + r.h <= PWIN.bottom - 20);
      rowRects.length = 0;
      rowRects.push(...visible);
      const named = [];
      for (const r of rowRects) {
        await page.mouse.click(r.x + r.w / 3, r.y + r.h / 2);
        await sleep(250);
        const d = partTexts(await ui2(page), "production.detail")[0] ?? null;
        if (d && d !== hint) named.push({ r, name: d });
      }
      // What keeps the turn free longest: a civilian unit, then a single soldier (each may share the city's tile
      // with one of the other kind); a culture building would bring a policy choice, which UI v2 has no screen for.
      // Each preferred item once: a second one of the same kind would stack with the first.
      const pickRow = PREFER.filter((p) => !BUILT.has(p)).map((p) => named.find((x) => x.name === p)).find(Boolean) ?? named.find((x) => !/Monument/.test(x.name)) ?? named[0];
      if (pickRow) BUILT.add(pickRow.name);
      let q = null;
      if (pickRow) {
        await page.mouse.click(pickRow.r.x + pickRow.r.w / 3, pickRow.r.y + pickRow.r.h / 2);
        await sleep(250);
        q = await ui2(page);
      }
      const short = partTexts(q, "production.orders").some((t) => /short/.test(t));
      await page.mouse.click(DETAIL_X, PWIN.bottom - 20 - (short ? 20 : 0) - 164);
      await waitUi(page, (x) => partTexts(x, "production.current").length > 0 && !partTexts(x, "production.current").some((t) => /nothing/.test(t)), 5000);
      await page.keyboard.press("Escape");
      await waitUi(page, (x) => partTexts(x, "production.head").length === 0, 3000);
      steps.push(`build ${partTexts(q, "production.detail")[0] ?? "?"}`);
    } else if (st === "Choose research") {
      await page.mouse.click(DIAL.x, DIAL.y);
      const open = await waitUi(page, (q) => partTexts(q, "research.list").length > 0, 5000);
      if (!open) return [...steps, "no research window"];
      const now = partTexts(open, "research.window").includes("Now") ? 36 : 0;
      await page.mouse.click(RWIN.left + 120, RWIN.top + RWIN.head + now + RWIN.pad + RWIN.row / 2);
      await waitUi(page, (q) => partTexts(q, "research.list").length === 0, 5000);
      steps.push(`research ${partTexts(open, "research.list")[0] ?? "?"}`);
    } else {
      return steps;
    }
  }
  return steps;
}

/** One forced end turn; resolves when the new turn is the human's and the dial shows it (or after 10 s more). */
async function endTurn(page, o) {
  const cur = (await state(page)).turn;
  const t0 = Date.now();
  await act(page, { kind: "endTurn", force: true });
  await until(page, (x) => x.turn > cur && x.status === "human", Math.min(o.turnTimeoutMs ?? 300000, 120000), `turn after ${cur} (helm: ${helmState(await ui2(page))})`);
  const wallMs = Date.now() - t0;
  const deadline = Date.now() + 10000;
  for (;;) {
    const p = await ui2(page);
    const dial = (p?.texts ?? []).filter((x) => x.part === "helm.dial").map((x) => x.text);
    if (dial.includes(String(cur + 1)) || Date.now() > deadline) break;
    await sleep(100);
  }
  // A short tail: the frames that follow the turn's batch (notifications, yields).
  await sleep(1000);
  return { turn: cur, wallMs };
}

/**
 * Reaction of one turn: the UI v2 frame on which the replica first has turn `next` and the one on which the dial first
 * shows it. The answer is the n-th frame (1: the same frame).
 */
function reaction(rows, next) {
  const r = rows.find((x) => x.turn !== undefined && x.turn !== null && x.turn >= next);
  const d = rows.find((x) => x.dial !== undefined && x.dial !== null && x.dial >= next);
  if (!r || !d) return { next, replicaFrame: r?.f ?? null, dialFrame: d?.f ?? null, nth: null };
  // Where the frames went: the first model change at or after the replica's turn (the source's share), the frames with
  // a model change until the dial shows it.
  const m = rows.find((x) => x.f >= r.f && (x.ops ?? 0) > 0);
  const changes = rows.filter((x) => x.f >= r.f && x.f <= d.f && (x.ops ?? 0) > 0).length;
  return { next, replicaFrame: r.f, firstModelFrame: m?.f ?? null, dialFrame: d.f, modelFramesBetween: changes, nth: d.f - r.f + 1 };
}

/** Frames of the UI v2 frame counter that the sampler did not see (two frames between two animation frames). */
function missed(rows) {
  let n = 0;
  for (let i = 1; i < rows.length; i++) n += Math.max(0, rows[i].f - rows[i - 1].f - 1);
  return n;
}

/** The skin package of the bundle (`ui2/skins.json`): its file and size. */
function packageInfo(webRoot) {
  try {
    const dir = path.join(webRoot, "ui2");
    const index = JSON.parse(readFileSync(path.join(dir, "skins.json"), "utf8"));
    const list = Array.isArray(index) ? index : index.skins ?? [];
    const e = list.find((x) => x.id === SKIN);
    if (!e) return { error: `no ${SKIN} in ui2/skins.json` };
    return { file: e.file, bytes: statSync(path.join(dir, e.file)).size };
  } catch (e) {
    return { error: String(e.message ?? e) };
  }
}

/** The client wasm of the bundle (`pkg/*.wasm`, `pkg-mt/*.wasm`): raw and brotli (quality 11) sizes. */
function wasmInfo(webRoot) {
  const out = {};
  for (const d of ["pkg", "pkg-mt"]) {
    let files = [];
    try {
      files = readdirSync(path.join(webRoot, d)).filter((f) => f.endsWith(".wasm"));
    } catch {
      continue;
    }
    for (const f of files) {
      const buf = readFileSync(path.join(webRoot, d, f));
      const br = zlib.brotliCompressSync(buf, { params: { [zlib.constants.BROTLI_PARAM_QUALITY]: 11, [zlib.constants.BROTLI_PARAM_SIZE_HINT]: buf.length } }).length;
      out[`${d}/${f}`] = { bytes: buf.length, brotli: br };
    }
  }
  return out;
}

/** The skin's own scripts (`skins/<skin>/js/*.js`), bytes. */
function skinJsBytes() {
  try {
    const dir = path.join(REPO, "skins", SKIN, "js");
    return readdirSync(dir).filter((f) => f.endsWith(".js")).reduce((a, f) => a + statSync(path.join(dir, f)).size, 0);
  } catch {
    return 0;
  }
}

/** Memory by realm (`measureUserAgentSpecificMemory`): the bytes of the UI v2 script workers, or why there are none. */
async function scriptHeap(page) {
  return page.evaluate(async () => {
    if (!self.crossOriginIsolated || typeof performance.measureUserAgentSpecificMemory !== "function") {
      return { bytes: null, why: self.crossOriginIsolated ? "measureUserAgentSpecificMemory is missing" : "the page is not cross-origin isolated" };
    }
    try {
      const m = await performance.measureUserAgentSpecificMemory();
      const workers = [];
      for (const b of m.breakdown) {
        for (const a of b.attribution ?? []) {
          if (a.scope === "DedicatedWorkerGlobalScope" && /\/ui2\//.test(a.url ?? "")) workers.push({ url: a.url, bytes: b.bytes });
        }
      }
      return { bytes: workers.length ? Math.max(...workers.map((w) => w.bytes)) : null, workers, total: m.bytes, why: workers.length ? null : "no UI v2 script worker in the breakdown" };
    } catch (e) {
      return { bytes: null, why: String(e) };
    }
  }).catch((e) => ({ bytes: null, why: String(e) }));
}

/** The map layer is there: the renderer drew it, or the skin has city banners. */
async function mapLayer(page, rows) {
  const drawn = rows.some((r) => (r.map ?? 0) > 0);
  const rects = await page.evaluate(() => window.__civ5test.ui2Rects(["city-banner"], 3000)).catch(() => null);
  const banners = Array.isArray(rects) ? rects.length : 0;
  return { present: drawn || banners > 0, drawn, banners };
}

/**
 * One arm of the banner comparison: a fresh page with `extra`, UI v2 running, then `secs` of animation-frame stamps
 * (no probe reads in between) and the count of the banners on the screen.
 */
async function fpsArm(base, o, extra, label, secs, viaQuickStart) {
  const b = await openPage({ headed: o.headed, uncapped: !!o.headed, logDir: o.logDir, viewport: VP, consoleFile: `console-${label}.log` });
  try {
    if (viaQuickStart) await enterGame(b, base, { ...o, lateUrl: null }, extra, `pf${label.slice(0, 2)}`);
    else await enterGame(b, base, o, extra, `pf${label.slice(0, 2)}`);
    await waitRunning(b.page, UP_MS);
    await sleep(5000);
    await installSampler(b.page);
    const r = await sampled(b.page, false, () => sleep(secs * 1000));
    const rects = await b.page.evaluate(() => window.__civ5test.ui2Rects(["city-banner"], 3000)).catch(() => null);
    const f = frames(r.ts, r.from, r.to);
    return { label, fpsP50: f.fpsP50 ?? null, frameMsP50: f.frameMsP50 ?? null, frameMsP99: f.frameMsP99 ?? null, frames: f.frames, banners: Array.isArray(rects) ? rects.length : null, errors: b.errors.slice(0, 3) };
  } finally {
    await b.close();
  }
}

/** The banner rows: the heavy fixture scene when UI v2 runs there, else the live game. */
async function bannerCost(ctx, o, n) {
  const secs = o.perfIdleS ?? 10;
  const on = { ui: `v2:${SKIN}` };
  const off = { ui: `v2:${SKIN}`, ui2onmap: "0" };
  if (o.server && o.assets && o.webRoot) {
    let fixture = null;
    try {
      fixture = await startServer({
        exe: o.server,
        webRoot: path.resolve(o.webRoot),
        assets: path.resolve(o.assets),
        logDir: o.logDir,
        env: { CIV5_FIXTURE_REVEAL_ALL: "1", CIV5_FIXTURE_HEAVY: `${o.benchUnits ?? 1500},${o.benchCities ?? 80}` },
        crossOriginIsolation: !!o.threads,
      });
      const heavy = { ...on, size: "huge", ai: String(o.benchAi ?? 6) };
      const a = await fpsArm(fixture.url, o, heavy, "banners-on", secs, true);
      const b = await fpsArm(fixture.url, o, { ...heavy, ui2onmap: "0" }, "banners-off", secs, true);
      n.bannerScene = "heavy fixture (CIV5_FIXTURE_HEAVY)";
      return { on: a, off: b };
    } catch (e) {
      n.bannerFixture = `UI v2 does not run on the heavy fixture: ${String(e.message ?? e).split("\n")[0]}`;
    } finally {
      if (fixture) await fixture.stop();
    }
  }
  const base = o.attach ?? ctx.server?.url;
  const a = await fpsArm(base, o, on, "banners-on", secs, false);
  const b = await fpsArm(base, o, off, "banners-off", secs, false);
  n.bannerScene = o.lateUrl ? "late game (live)" : "live game (few banners)";
  return { on: a, off: b };
}

const fmt = (v, unit = "") => (v === null || v === undefined ? "n/a" : `${v}${unit}`);
const KB = (b) => (b === null || b === undefined ? null : round(b / 1024, 1));
const MB = (b) => (b === null || b === undefined ? null : round(b / 1048576, 2));

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "ui2-perf", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const base = o.attach ?? ctx.server?.url;
  if (!o.attach && !o.lateUrl) {
    check("options", false, "ui2-perf needs --attach <url of the real-core server> (tools/playtest/m2/server.ps1) or --late-url/--late-nick/--late-token: UI v2 reads the game's layer, which the fixture server does not bring up");
    result.durationMs = Date.now() - started;
    return result;
  }
  if (o.lateUrl && (!o.lateNick || !o.lateToken)) {
    check("options", false, "--late-url needs --late-nick and --late-token");
    result.durationMs = Date.now() - started;
    return result;
  }
  const remote = remoteSession();
  // Everything goes to `<logs>/ui2-perf.json`; the report keeps the meta and the table.
  const n = {};
  BUILT.clear();
  const meta = (n.meta = {
    skin: SKIN,
    headed: !!o.headed,
    profile: o.profile ?? "debug",
    remote,
    console: !remote,
    scene: o.lateUrl ? `late game ${o.lateUrl}` : "fresh small game (seed 7, 2 AIs), early turns: not the late game of the threshold",
    host: { cpu: os.cpus()[0]?.model, cores: os.cpus().length, os: `${os.type()} ${os.release()}` },
    startedAt: new Date().toISOString(),
  });
  // The kind of the numbers: only headed + release at the console are final.
  meta.kind = o.headed && meta.profile === "release" && !remote ? "final (headed, release, console)" : `indicative (${[o.headed ? "headed" : "headless", meta.profile, remote ? `remote session ${remote}` : "console"].join(", ")})`;
  // The runner's `--turns` defaults to another scenario's 50: the count of this one is `UI2_PERF_TURNS`.
  const turnsN = Number(process.env.UI2_PERF_TURNS ?? 20);
  const idleS = o.perfIdleS ?? 10;
  const appErrors = (errs) => errs.filter((e) => !e.includes("/pkg-mt/"));
  let error = null;
  // Memory by realm right away (as `ui2-n6`): the renderer locked to the site, no wait for the next GC.
  const b = await openPage({
    headed: o.headed,
    logDir: o.logDir,
    viewport: VP,
    consoleFile: "console-ui2-perf.log",
    chromeArgs: ["--enable-blink-features=ForceEagerMeasureMemory", "--site-per-process"],
  });
  const page = b.page;
  const rows = [];
  const row = (id, metric, value, limit, ok, note) => rows.push({ id, metric, value, limit, verdict: ok === null ? "n/a" : ok ? "pass" : "FAIL", note: note ?? "" });
  try {
    meta.chrome = b.browser.version();
    const t0 = Date.now();
    await enterGame(b, base, o, { ui: `v2:${SKIN}` }, "ui2p");
    const probe = await waitRunning(page, UP_MS);
    n.toRunningMs = Date.now() - t0;
    n.loadMs = probe.loadMs ?? null;
    if (!o.lateUrl) {
      n.found = await foundCity(page);
      // The soldier steps out of the city: a soldier built there later would stack with it (a blocker of the turn
      // that no order of the unit panel settles).
      n.moved = await moveAUnit(page, await state(page)).catch((e) => `not moved: ${e.message}`);
    }
    await installSampler(page);
    // The page settles: the first frames after the start and the city.
    await sleep(5000);

    // ---- N1 idle
    const idle = await sampled(page, false, () => sleep(idleS * 1000));
    const idleCpu = idle.rows.map((r) => r.cpu).filter((x) => x !== null);
    n.idle = { cpu: stat(idleCpu), framesSeen: idle.rows.length, framesMissed: missed(idle.rows), withModelChange: idle.rows.filter((r) => (r.ops ?? 0) > 0).length, fps: frames(idle.ts, idle.from, idle.to) };
    n.hud = { draws: stat(idle.rows.map((r) => r.hud).filter((x) => x !== null)), ui2Draws: stat(idle.rows.map((r) => r.draws).filter((x) => x !== null)), pass: await ui2Pass(page) };
    n.map = await mapLayer(page, idle.rows);
    n.js = { idle: { worker: stat(idle.rows.map((r) => r.jsWorker).filter((x) => x !== null)), main: stat(idle.rows.map((r) => r.jsMain).filter((x) => x !== null)) } };

    // ---- N1 turns, N3 (the replica's turn and the dial per frame)
    const turns = [];
    const turnRows = [];
    const reactions = [];
    for (let i = 0; i < turnsN; i++) {
      // Outside the measured window: what the turn waits for (a window opened by the dial is not a turn's cost).
      const settled = await settleBlockers(page);
      if (settled.length) (n.blockers ??= []).push({ turn: (await state(page))?.turn, steps: settled });
      let info = null;
      let r = null;
      try {
        r = await sampled(page, true, async () => {
          info = await endTurn(page, o);
        });
      } catch (e) {
        // A blocker the skin cannot settle (UI v2 has no policy screen, for one): the turns measured so far stand,
        // the table says how many there were.
        await page.evaluate(() => { window.__ui2perf.on = false; }).catch(() => {});
        n.turnsStopped = String(e.message ?? e).split("\n")[0];
        break;
      }
      const changed = r.rows.filter((x) => (x.ops ?? 0) > 0);
      const cpu = changed.map((x) => x.cpu).filter((x) => x !== null);
      turnRows.push(...r.rows);
      const re = reaction(r.rows, info.turn + 1);
      reactions.push(re);
      const worst = r.rows.reduce((a, x) => ((x.cpu ?? -1) > (a?.cpu ?? -1) ? x : a), null);
      // The frames from the end turn until the dial shows the new turn, with what the source did in each (where
      // a slow frame or a late dial comes from).
      const trail = r.rows
        .filter((x) => re.dialFrame !== null && x.f <= re.dialFrame && ((x.ops ?? 0) > 0 || (x.source ?? 0) > 0.05 || x.f >= (re.replicaFrame ?? Infinity)))
        .map((x) => ({ f: x.f, turn: x.turn, dial: x.dial, step: x.step, ops: x.ops, cpu: x.cpu, source: x.source, src: x.src }));
      turns.push({ ...info, trail, worst: worst && { cpu: worst.cpu, source: worst.source, runtime: worst.runtime, ops: worst.ops, src: worst.src }, frames: r.rows.length, missed: missed(r.rows), changedFrames: changed.length, cpu: stat(cpu), allCpu: stat(r.rows.map((x) => x.cpu).filter((x) => x !== null)), reaction: re.nth });
    }
    const changedCpu = turnRows.filter((x) => (x.ops ?? 0) > 0).map((x) => x.cpu).filter((x) => x !== null);
    n.turns = { list: turns, cpuChanged: stat(changedCpu), cpuAll: stat(turnRows.map((x) => x.cpu).filter((x) => x !== null)), hud: stat(turnRows.map((x) => x.hud).filter((x) => x !== null)) };
    n.reaction = { list: reactions, worst: reactions.every((x) => x.nth !== null) ? Math.max(...reactions.map((x) => x.nth)) : null };
    n.js.turns = { worker: stat(turnRows.map((r) => r.jsWorker).filter((x) => x !== null)), main: stat(turnRows.map((r) => r.jsMain).filter((x) => x !== null)) };

    // ---- N6 heap and GC: memory by realm after the turns; a trace of a few more turns.
    n.js.heap = await scriptHeap(page);
    try {
      await b.browser.startTracing(page, { categories: ["devtools.timeline", "v8", "disabled-by-default-v8.gc"] });
      n.js.gcOver = n.turnsStopped ? "10 s of the stuck turn (no more turns)" : "3 turns";
      try {
        if (n.turnsStopped) await sleep(10000);
        else {
          for (let i = 0; i < 3; i++) {
            await settleBlockers(page);
            await endTurn(page, o);
          }
        }
      } finally {
        n.js.gc = gcPauses(await b.browser.stopTracing());
      }
    } catch (e) {
      n.js.gc = { ...(n.js.gc ?? {}), error: String(e.message ?? e).split("\n")[0] };
    }
    const after = await ui2(page);
    n.js.counters = Object.fromEntries(Object.entries(after?.counters ?? {}).filter(([k]) => k.startsWith("ui2.js.")));
    n.js.scripts = after?.scripts ?? null;
  } catch (e) {
    error = e;
    n.lastProbe = await ui2(page);
    try { await page.screenshot({ path: path.join(o.logDir, "failure-ui2-perf.png"), timeout: 60000 }); } catch { /* none */ }
  }
  result.consoleErrors.push(...b.errors);
  const errs = appErrors(b.errors);
  await b.close();

  // ---- N2 banners: only with the skin's map layer.
  if (!error) {
    if (n.map?.present) {
      try {
        n.banners = await bannerCost(ctx, o, n);
      } catch (e) {
        n.banners = { error: String(e.message ?? e).split("\n")[0] };
      }
    } else {
      n.banners = { notAvailable: "the skin has no map layer on this build (no ui2.draws.map, no city-banner part)" };
    }
  }

  // ---- N4 sizes (from the bundle the page was served from, when it is this tree's)
  const webRoot = path.resolve(o.webRoot ?? "target/web");
  n.pkg = packageInfo(webRoot);
  n.wasm = wasmInfo(webRoot);
  n.skinJs = skinJsBytes();

  // ---- the table
  if (!error) {
    const indicative = meta.kind.startsWith("indicative") ? " (indicative)" : "";
    const i = n.idle.cpu;
    row("N1", "idle: ui2.cpu_ms p95 over the frames", `${fmt(i.p95)} ms (p50 ${fmt(i.p50)}, max ${fmt(i.max)}; ${i.n} frames, ${n.idle.framesMissed} unseen)`, `<= ${LIMITS.idleP95Ms} ms`, i.p95 !== null && i.p95 <= LIMITS.idleP95Ms, `${idleS} s; frames with a model change: ${n.idle.withModelChange}${indicative}`);
    const t = n.turns.cpuChanged;
    const measured = n.turns.list.length;
    row("N1", "turns measured", `${measured} of ${turnsN}`, `${turnsN}`, measured === turnsN, n.turnsStopped ? `stopped: ${n.turnsStopped}` : "");
    row("N1", "turns: ui2.cpu_ms of the frames with a model change, mean", `${fmt(t.mean)} ms (${t.n} frames over ${measured} turns)`, `<= ${LIMITS.turnMeanMs} ms`, t.mean !== null && t.mean <= LIMITS.turnMeanMs, `${meta.scene}${indicative}`);
    row("N1", "turns: same, peak", `${fmt(t.max)} ms (p95 ${fmt(t.p95)})`, `<= ${LIMITS.turnPeakMs} ms`, t.max !== null && t.max <= LIMITS.turnPeakMs, "the worst frame of all turns");
    const hudMax = Math.max(n.hud.draws.max ?? -1, n.turns.hud.max ?? -1);
    row("N2", "HUD draw calls (ui2.draws.hud, max of idle and turns)", `${hudMax < 0 ? "n/a" : hudMax} (Ui2 pass row: ${fmt(n.hud.pass?.draws)} draws, ui2.draws ${fmt(n.hud.ui2Draws.max)})`, `<= ${LIMITS.hudDraws}`, hudMax >= 0 && hudMax <= LIMITS.hudDraws, "");
    if (n.banners?.on && n.banners?.off) {
      const on = n.banners.on.fpsP50;
      const off = n.banners.off.fpsP50;
      const drop = on !== null && off ? round(((off - on) / off) * 100, 1) : null;
      row("N2", "map banners and flags: FPS drop against ?ui2onmap=0", `${fmt(drop, " %")} (${fmt(on)} vs ${fmt(off)} fps p50; ${fmt(n.banners.on.banners)} banners)`, `<= ${LIMITS.bannerFpsDropPct} %`, drop !== null && drop <= LIMITS.bannerFpsDropPct, `${n.bannerScene}${n.bannerFixture ? `; ${n.bannerFixture}` : ""}; ${o.headed ? "uncapped" : "headless: meaningless"}${indicative}`);
    } else {
      row("N2", "map banners and flags: FPS drop against ?ui2onmap=0", "not available", `<= ${LIMITS.bannerFpsDropPct} %`, null, n.banners?.notAvailable ?? n.banners?.error ?? "");
    }
    row("N3", "reaction: frame on which the dial shows the replica's new turn, worst", `${fmt(n.reaction.worst)} (per turn: ${n.reaction.list.map((x) => x.nth ?? "?").join(",")})`, `<= ${LIMITS.reactionFrame}`, n.reaction.worst !== null && n.reaction.worst <= LIMITS.reactionFrame, "1 = the same UI v2 frame");
    row("N4", "skin package size", n.pkg.bytes !== undefined ? `${MB(n.pkg.bytes)} MB (${n.pkg.file})` : `n/a (${n.pkg.error})`, "<= 3 MB", n.pkg.bytes !== undefined ? n.pkg.bytes <= LIMITS.packageBytes : null, "");
    row("N4", "ui2.load_ms", fmt(n.loadMs, " ms"), `<= ${LIMITS.loadMs} ms`, typeof n.loadMs === "number" ? n.loadMs <= LIMITS.loadMs : null, `${meta.profile} bundle`);
    const wasm = Object.entries(n.wasm).map(([f, w]) => `${f} ${KB(w.bytes)} KB raw, ${KB(w.brotli)} KB brotli`).join("; ");
    row("N4", "client wasm (the text stack's share: report only)", wasm || "n/a", "report", null, "the share of the text stack needs a build without it (not measured here)");
    const jw = Math.max(n.js.idle.worker.p95 ?? -1, n.js.turns.worker.p95 ?? -1);
    const jm = Math.max(n.js.idle.main.p95 ?? -1, n.js.turns.main.p95 ?? -1);
    const scripts = (n.js.counters?.["ui2.js.contexts"] ?? 0) > 0;
    row("N6", "JS in a frame, p95 (ui2.js.worker_ms / ui2.js.main_ms)", scripts ? `${fmt(jw < 0 ? null : jw)} / ${fmt(jm < 0 ? null : jm)} ms (turn max ${fmt(n.js.turns.worker.max)} / ${fmt(n.js.turns.main.max)})` : "no script context", `<= ${LIMITS.jsFrameMs} ms`, scripts ? jw <= LIMITS.jsFrameMs && jm <= LIMITS.jsFrameMs : null, `contexts ${fmt(n.js.counters?.["ui2.js.contexts"])}, posted ${fmt(n.js.counters?.["ui2.js.posted"])}, errors ${fmt(n.js.counters?.["ui2.js.errors"])}, restarts ${fmt(n.js.counters?.["ui2.js.restarts"])}`);
    row("N6", "script heap", n.js.heap?.bytes ? `${MB(n.js.heap.bytes)} MB` : `n/a (${n.js.heap?.why})`, `<= ${LIMITS.heapMb} MB`, n.js.heap?.bytes ? MB(n.js.heap.bytes) <= LIMITS.heapMb : null, "memory by realm of the ui2 worker");
    const gc = n.js.gc?.worker?.peakMs ?? null;
    row("N6", "GC peak pause, dedicated workers", gc === null ? `n/a (${n.js.gc?.error ?? "no GC event"})` : `${gc} ms (page thread ${fmt(n.js.gc?.main?.peakMs)} ms)`, `<= ${LIMITS.gcMs} ms`, gc === null ? null : gc <= LIMITS.gcMs, `trace of ${n.js.gcOver ?? "3 turns"}; the game's own workers are dedicated workers too`);
    row("N6", "skin JS size", `${KB(n.skinJs)} KB`, `<= ${LIMITS.jsKb} KB`, KB(n.skinJs) <= LIMITS.jsKb, `skins/${SKIN}/js`);
    for (const r of rows) if (r.verdict !== "n/a") check(`${r.id} ${r.metric}`, r.verdict === "pass", `${r.value} (limit ${r.limit})${r.note ? `; ${r.note}` : ""}`);
  }
  n.table = rows;
  result.numbers = { meta, table: rows, toRunningMs: n.toRunningMs ?? null, json: path.join(o.logDir, "ui2-perf.json") };
  check("no console errors", errs.length === 0, `${errs.length} error(s)${errs[0] ? `: ${errs[0]}` : ""}`);
  if (error) {
    check("no scenario error", false, error.message);
    result.error = error.stack ?? String(error);
  }
  meta.finishedAt = new Date().toISOString();
  try { writeFileSync(path.join(o.logDir, "ui2-perf.json"), JSON.stringify(n, null, 2)); } catch { /* no log dir */ }
  if (rows.length) {
    console.log(`ui2-perf table (${meta.kind}; ${meta.scene}):`);
    for (const r of rows) console.log(`  ${r.id} ${r.metric}: ${r.value} | limit ${r.limit} | ${r.verdict}${r.note ? ` | ${r.note}` : ""}`);
  }
  result.pass = result.checks.every((c) => c.pass);
  result.durationMs = Date.now() - started;
  return result;
}
