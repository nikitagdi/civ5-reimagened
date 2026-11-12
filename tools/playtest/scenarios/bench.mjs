// `bench` (B3 perf-bench, AC-B1): the heavy-scene render benchmark. A Huge game with a late-game load, a
// scripted camera and a series of end turns, measured twice in a visible Chrome with the real GPU:
//  - `capped`: as a player sees it (the browser's vsync / the monitor rate caps the FPS);
//  - `uncapped`: `--disable-frame-rate-limit --disable-gpu-vsync`, so the frame time is what the CPU or the GPU needs
//    (the 144 FPS target of AC-R2 is read from here: frame time <= 6.9 ms).
// The scene (default): the fixture core with `CIV5_FIXTURE_HEAVY=<units>,<cities>` (see crates/shared/game-host/src/fixture/heavy.rs;
// 1500 units, 80 extra cities, roads and improvements, deterministic) and `CIV5_FIXTURE_REVEAL_ALL`, 6 AIs, Huge; it needs no WSL.
// With `--late-url/--late-nick/--late-token` it attaches to the real core with the stored late save instead (as `perf-late`).
// Phases per mode: load, idle-default, idle-far, pan-far, pan-mid, zoom, endturns. Each phase record has the frame statistics
// (p50 FPS, 1% low, frame ms) and every numeric field of `window.__civ5stats` summarised (draws, triangles, passes.*,
// counters.* including terrain.*, gpu_mem.*, ram.*, net.* whatever is published).
// Output: `<logs>/bench.json` (+ `bench.md`), the same in `report.json`. Compare with `node tools/playtest/bench-compare.mjs`
// or `cargo xtask bench --compare <baseline.json>`.
//   cargo xtask playtest --scenario bench --headed --release [--bench-modes capped,uncapped] [--bench-units 1500]
//     [--bench-cities 80] [--bench-turns 8] [--bench-phase-s 10] [--bench-size 1920x1080] [--bench-label name]
// `CIV5_BENCH_UI=v2` (or `v2:<skin>`): the game runs with the new UI (`?ui=v2`, its map banners included) instead of
// the interim panels; compare a run with it against one without (`cargo xtask bench --compare`) for the cost of the
// UI v2 layers. The value is in `meta.ui` of the report. `CIV5_BENCH_UI2ONMAP=0` with it turns the skin's map layer (city
// banners, unit flags) off (`&ui2onmap=0`): a run with it against one without is the cost of that layer.
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { openPage, act, until, state, appUrl, INTERIM } from "../lib/browser.mjs";
import { installProbe, now, drain, phaseRecord, frames, round, docState } from "../lib/bench.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const PASSWORD = "dev";

/** The `ui` parameter of the game page: `CIV5_BENCH_UI` (`v2`, `v2:<skin>`) or the interim panels; `ui2onmap` with it. */
export function benchUi(env = process.env) {
  const v = (env.CIV5_BENCH_UI ?? "").trim();
  if (!/^v2(:[A-Za-z0-9_-]+)?$/.test(v)) return INTERIM;
  return env.CIV5_BENCH_UI2ONMAP === "0" ? { ui: v, ui2onmap: "0" } : { ui: v };
}

/** The camera path: key, hold ms. A rectangle over the map at the normal pan speed, twice. */
export const PAN_PATH = [["ArrowRight", 3000], ["ArrowDown", 2000], ["ArrowLeft", 3000], ["ArrowUp", 2000]];

function git(root, ...args) {
  try {
    return execFileSync("git", args, { cwd: root, encoding: "utf8" }).trim();
  } catch {
    return null;
  }
}

async function gpuInfo(page) {
  return page.evaluate(async () => {
    const out = { dpr: window.devicePixelRatio, w: innerWidth, h: innerHeight };
    try {
      const a = await navigator.gpu?.requestAdapter();
      const i = a?.info;
      out.webgpu = a ? { vendor: i?.vendor, architecture: i?.architecture, device: i?.device, description: i?.description, fallback: a.isFallbackAdapter } : null;
    } catch (e) {
      out.webgpu = `error: ${e}`;
    }
    return out;
  });
}

/** Holds the keys of `path` for `repeat` rounds (the camera moves while a key is down). */
async function pan(page, repeat = 2) {
  for (let r = 0; r < repeat; r++) {
    for (const [key, ms] of PAN_PATH) {
      await page.keyboard.down(key);
      await sleep(ms);
      await page.keyboard.up(key);
    }
  }
}

/** Top self and inclusive times of a CDP CPU profile, by function name (the hash suffix of Rust names dropped). */
export function profileSummary(profile) {
  const byId = new Map(profile.nodes.map((nd) => [nd.id, nd]));
  const parent = new Map();
  for (const nd of profile.nodes) for (const c of nd.children ?? []) parent.set(c, nd.id);
  const name = (f) => `${f.functionName || "(anonymous)"}`.replace(/::h[0-9a-f]{16}$/, "");
  const self = new Map();
  const incl = new Map();
  let total = 0;
  profile.samples.forEach((id, i) => {
    const dt = profile.timeDeltas[i] ?? 0;
    total += dt;
    const k = name(byId.get(id).callFrame);
    self.set(k, (self.get(k) ?? 0) + dt);
    const seen = new Set();
    for (let cur = id; cur !== undefined; cur = parent.get(cur)) {
      const n = name(byId.get(cur).callFrame);
      if (!seen.has(n)) {
        seen.add(n);
        incl.set(n, (incl.get(n) ?? 0) + dt);
      }
    }
  });
  const line = ([k, v]) => `${String(Math.round(v / 1000)).padStart(8)} ms ${String(round((v / total) * 100, 1)).padStart(5)}%  ${k.slice(0, 200)}`;
  const top = (m, n) => [...m.entries()].sort((a, c) => c[1] - a[1]).slice(0, n).map(line);
  return [`total ${Math.round(total / 1000)} ms`, "", "self:", ...top(self, 60), "", "inclusive:", ...top(incl, 150), ""].join("\n");
}

async function wheel(page, clicks, dy, gapMs = 120) {
  for (let i = 0; i < clicks; i++) {
    await page.mouse.wheel(0, dy);
    await sleep(gapMs);
  }
}

/** A launch is in the slow state when the CPU raster of its field cells averages more than this per cell, or the idle
 * view's frame CPU time is above `SLOW_TIER_IDLE_CPU_MS`. Measured on the heavy Huge scene and a hybrid desktop CPU: 0.29
 * ms/cell and 6.3 ms on a performance core, 0.53-0.59 ms/cell and 7.7-8.3 ms when the scheduler kept the renderer on
 * an efficiency core (the cause; not V8's tiering). */
export const SLOW_TIER_RASTER_MS = 0.42;
export const SLOW_TIER_IDLE_CPU_MS = 7.2;

/** Milliseconds per field cell of the raster between `from` and `to` (`terrain.raster_*` counters), with the cell
 * count; null without samples. */
export function rasterPerCell(samples, from, to) {
  const N = "counters.terrain.raster_cells_total";
  const T = "counters.terrain.raster_us_total";
  const inRange = samples.filter((s) => s.t > from && s.t <= to && s.v[N] !== undefined);
  const before = samples.filter((s) => s.t <= from && s.v[N] !== undefined).at(-1);
  const z = inRange.at(-1);
  if (!z) return null;
  const a = before ?? { v: { [N]: 0, [T]: 0 } };
  const cells = z.v[N] - a.v[N];
  if (cells <= 0) return { ms: null, cells: 0 };
  return { ms: round((z.v[T] - a.v[T]) / 1000 / cells, 3), cells };
}

/** One browser, one pass over all phases. Returns {phases, scene}. */
async function runMode(ctx, mode) {
  const o = ctx.opts;
  const [w, h] = (o.benchSize ?? "1920x1080").split("x").map(Number);
  const phaseS = o.benchPhaseS ?? 10;
  // CIV5_BENCH_CHROME_ARGS: extra Chrome switches for an experiment (space separated), e.g. `--js-flags=--no-liftoff`.
  const chromeArgs = (process.env.CIV5_BENCH_CHROME_ARGS ?? "").split(" ").filter(Boolean);
  const b = await openPage({ headed: o.headed, uncapped: mode === "uncapped", logDir: o.logDir, consoleFile: `console-${mode}.log`, viewport: { width: w, height: h }, chromeArgs });
  const { page } = b;
  const phases = {};
  const scene = {};
  let error = null;
  try {
    const popts = { ...o, preset: o.preset ?? "high" };
    let url;
    if (o.lateUrl) {
      const token = fs.readFileSync(o.lateToken, "utf8").trim();
      await page.addInitScript(([nick, tok]) => {
        try {
          localStorage.setItem("civ5.nick", nick);
          localStorage.setItem(`civ5.token.${nick}`, tok);
        } catch {
          /* storage blocked */
        }
      }, [o.lateNick, token]);
      url = appUrl(o.lateUrl, popts, { ...benchUi(), nick: o.lateNick, stats: "1" });
    } else {
      url = appUrl(ctx.server.url, popts, { ...benchUi(), nick: `bench-${mode}`, autostart: "1", ai: String(o.benchAi ?? 6), size: "huge", seed: String(o.seed ?? 7), ...(o.benchStats === "0" ? {} : { stats: "1" }) });
    }
    // CIV5_BENCH_PROFILE=load: the V8 CPU profile of the whole load (page start to settled), `bench-<mode>-load.prof.txt`.
    const loadProf = (process.env.CIV5_BENCH_PROFILE ?? "").split(",").some((x) => x === "load" || x === "all");
    let loadCdp = null;
    if (loadProf) {
      loadCdp = await page.context().newCDPSession(page);
      await loadCdp.send("Profiler.enable");
      await loadCdp.send("Profiler.setSamplingInterval", { interval: 200 });
      await loadCdp.send("Profiler.start");
    }
    const t0 = Date.now();
    await page.goto(url);
    let s = await until(page, (x) => x.screen === "login" || x.screen === "play", 60000, "sign-in window or the game");
    if (s.screen === "login") await act(page, { kind: "login", nick: `bench-${mode}`, password: PASSWORD });
    s = await until(page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null && x.status === "human", 90000, "the heavy game");
    scene.timeToPlayMs = Date.now() - t0;
    // Every process of the browser is up by now: pinned to the performance cores (CIV5_NO_CPU_PIN=1: not).
    await b.pin();
    scene.turn = s.turn;
    scene.units = s.units?.length ?? null;
    scene.cities = s.cities?.length ?? null;
    scene.players = s.players;
    scene.gpu = await gpuInfo(page);
    scene.chrome = b.browser.version();
    // B3 render-mt: whether the page is cross-origin isolated and runs the threaded build (and its workers).
    // `mode` is the bundle the page chose (`threads` by default, `single` with `--single` or a client without threads).
    scene.threads = await page.evaluate(() => ({
      mode: window.__civ5threads?.mode ?? "single",
      reason: window.__civ5threads?.reason ?? null,
      crossOriginIsolated: self.crossOriginIsolated === true,
      workers: window.__civ5threads?.workers ?? 0,
      joined: window.__civ5threads?.joined?.() ?? 0,
    }));
    // The switches this browser was launched with (ours; Playwright adds its defaults before them).
    scene.chromeArgs = b.args;
    await installProbe(page);
    // The terrain and the objects settle (streaming), then the canvas gets the focus for the arrow keys.
    // The `load` phase: what the frames cost while the assets stream in and the pages bake, until the work queues are quiet
    // (no `pending`/`queue` counter above 0 and no baking/upload counter moving for 4 s; at least `--bench-settle-s`, at most 120 s).
    // `scene.viewReadyMs`: when the terrain first had every cell in view and ring built and paged (`terrain.view_ready`).
    const loadFrom = await now(page);
    const minEnd = Date.now() + (o.benchSettleS ?? 8) * 1000;
    const hardEnd = Date.now() + 120000;
    let sig = "";
    let quietSince = Date.now();
    for (;;) {
      await sleep(500);
      const v = await page.evaluate(() => window.__bench.samples.at(-1)?.v ?? {});
      // Work counters only: the pass timings (`passes.TerrainBake`, `passes_full.*`) move with every frame of the
      // background bake of the off-screen base layer, which is budgeted frame cost, not a queue the view waits for.
      const work = Object.entries(v).filter(([k]) => !k.startsWith("passes") && /pending|queue|bake|upload|evict|inflight|in_flight/i.test(k));
      const busy = work.some(([k, x]) => /pending|queue|inflight|in_flight/i.test(k) && x > 0);
      if (scene.viewReadyMs == null && v["counters.terrain.view_ready"] === 1) scene.viewReadyMs = Date.now() - (minEnd - (o.benchSettleS ?? 8) * 1000);
      const next = JSON.stringify(work);
      if (busy || next !== sig) quietSince = Date.now();
      sig = next;
      if (Date.now() >= hardEnd) {
        scene.settleTimedOut = true;
        break;
      }
      if (Date.now() >= minEnd && Date.now() - quietSince >= 4000) break;
    }
    scene.settleMs = Date.now() - (minEnd - (o.benchSettleS ?? 8) * 1000);
    // Where the terrain pages get their vertices (`terrain.vertex_path`): "gpu" (compute pass) or "cpu" (WebGL2, or
    // the compute pipeline failed: the console has the reason).
    {
      const v = await page.evaluate(() => window.__bench.samples.at(-1)?.v ?? {});
      const p = v["counters.terrain.vertex_path"];
      scene.bakePath = p === 1 ? "gpu" : p === 0 ? "cpu" : null;
    }
    if (loadCdp) {
      const { profile } = await loadCdp.send("Profiler.stop");
      const base = path.join(o.logDir, `bench-${mode}-load`);
      fs.writeFileSync(`${base}.cpuprofile`, JSON.stringify(profile));
      fs.writeFileSync(`${base}.prof.txt`, profileSummary(profile));
    }
    {
      const to = await now(page);
      const data = await drain(page);
      phases["load"] = { ...phaseRecord(data, loadFrom, to), from: loadFrom, to, raster: rasterPerCell(data.samples, loadFrom, to) };
      console.log(`  [${mode}] load          ${phases["load"].seconds ?? "-"} s  p50 ${phases["load"].fpsP50} fps  max frame ${phases["load"].frameMsMax} ms`);
    }
    await page.mouse.move(w / 2, h / 2);
    await page.mouse.click(w / 2, h / 2);
    await page.evaluate(() => {
      const c = document.querySelector("canvas");
      if (c) {
        if (c.tabIndex < 0) c.tabIndex = 0;
        c.focus();
      }
    });
    // CPU of the browser's processes (CDP): the share of one core each process type used during a phase, and the main
    // thread's busy share (task time); the renderer's workers (the game worker) are in "renderer" cpu.
    const cdp = await page.context().newCDPSession(page);
    await cdp.send("Performance.enable");
    // Total CPU load of the machine during a phase (other sessions compiling disturb the numbers: it is in the report).
    const hostTimes = () => os.cpus().reduce((a, c) => ({ idle: a.idle + c.times.idle, all: a.all + c.times.user + c.times.nice + c.times.sys + c.times.idle + c.times.irq }), { idle: 0, all: 0 });
    const cpuNow = async () => {
      const m = Object.fromEntries((await cdp.send("Performance.getMetrics")).metrics.map((x) => [x.name, x.value]));
      let procs = [];
      try {
        procs = (await cdp.send("SystemInfo.getProcessInfo")).processInfo;
      } catch {
        /* not available */
      }
      const byType = {};
      for (const p of procs) byType[p.type] = (byType[p.type] ?? 0) + p.cpuTime;
      return { t: m.Timestamp, task: m.TaskDuration, script: m.ScriptDuration, byType, host: hostTimes() };
    };
    const cpuDelta = (a, z) => {
      const dt = z.t - a.t;
      const out = { hostBusy: round((1 - (z.host.idle - a.host.idle) / (z.host.all - a.host.all)) * 100, 1), mainBusy: round(((z.task - a.task) / dt) * 100, 1), mainScript: round(((z.script - a.script) / dt) * 100, 1) };
      for (const k of new Set([...Object.keys(a.byType), ...Object.keys(z.byType)])) out[`proc.${k}`] = round((((z.byType[k] ?? 0) - (a.byType[k] ?? 0)) / dt) * 100, 1);
      return out;
    };
    // CIV5_BENCH_PROFILE=idle-far,endturns (or `all`): the V8 CPU profile of those phases (it slows the frame: profile runs
    // are for the audit, not for numbers). Saved as `<logs>/bench-<mode>-<phase>.cpuprofile` (DevTools opens it) with the
    // top self and inclusive times in `bench-<mode>-<phase>.prof.txt`; the wasm names need `CIV5_WASM_NAMES=1` at build.
    const profiled = (process.env.CIV5_BENCH_PROFILE ?? "").split(",").filter(Boolean);
    const measure = async (name, fn) => {
      const prof = profiled.includes(name) || profiled.includes("all");
      if (prof) {
        await cdp.send("Profiler.enable");
        await cdp.send("Profiler.setSamplingInterval", { interval: 200 });
        await cdp.send("Profiler.start");
      }
      // The slow-frame record of the client (frames over 33 ms and where they spent it, `window.__civ5stats.slow_frames`,
      // published once a second): the ones of this phase are those with a `seq` above the last one seen before it.
      const slowSeq = await page.evaluate(() => Math.max(0, ...((window.__civ5stats?.slow_frames ?? []).map((f) => f.seq))));
      const c0 = await cpuNow();
      const from = await now(page);
      await fn();
      const to = await now(page);
      const c1 = await cpuNow();
      if (prof) {
        const { profile } = await cdp.send("Profiler.stop");
        const base = path.join(o.logDir, `bench-${mode}-${name}`);
        fs.writeFileSync(`${base}.cpuprofile`, JSON.stringify(profile));
        fs.writeFileSync(`${base}.prof.txt`, profileSummary(profile));
      }
      await sleep(1300);
      const slowFrames = await page.evaluate((seq) => (window.__civ5stats?.slow_frames ?? []).filter((f) => f.seq > seq), slowSeq);
      const data = await drain(page);
      phases[name] = { ...phaseRecord(data, from, to), from, to, cpuPct: cpuDelta(c0, c1), slowFrames, raster: rasterPerCell(data.samples, from, to) };
      const r = phases[name];
      const draws = r.stats?.draws?.p50;
      if (slowFrames.length) {
        const worst = slowFrames.reduce((a, b) => (b.ms > a.ms ? b : a));
        console.log(`      ${slowFrames.length} slow frame(s) (>33 ms); worst ${Math.round(worst.ms)} ms turn ${worst.turn}: ${worst.scopes.slice(0, 8).map(([n, ms]) => `${n} ${Math.round(ms)}`).join(", ")}`);
      }
      console.log(`  [${mode}] ${name.padEnd(13)} ${r.seconds ?? "-"} s  p50 ${r.fpsP50} fps  1% low ${r.fps1pLow}  frame p50 ${r.frameMsP50} ms p99 ${r.frameMsP99} max ${r.frameMsMax}  draws ${draws ?? "-"}  passSum ${r.stats?.passSumMs?.p50 ?? "-"} ms`);
    };
    await measure("idle-default", () => sleep(phaseS * 1000));
    if (o.screenshot !== false) await page.screenshot({ path: path.join(o.logDir, `bench-${mode}-default.png`) }).catch(() => {});
    await wheel(page, 25, 400, 60);
    await sleep(2000);
    await measure("idle-far", () => sleep(phaseS * 1000));
    if (o.screenshot !== false) await page.screenshot({ path: path.join(o.logDir, `bench-${mode}-far.png`) }).catch(() => {});
    await measure("pan-far", () => pan(page));
    await wheel(page, 8, -400);
    await sleep(2000);
    await measure("pan-mid", () => pan(page));
    await measure("zoom", async () => {
      for (let i = 0; i < 3; i++) {
        await wheel(page, 10, 400, 150);
        await wheel(page, 10, -400, 150);
      }
    });
    // Back to the far view, then the end turns: every turn is timed and its worst frame kept.
    await wheel(page, 25, 400, 60);
    await sleep(2000);
    const turns = [];
    await measure("endturns", async () => {
      const n = o.benchTurns ?? 8;
      for (let i = 0; i < n; i++) {
        const cur = (await state(page)).turn;
        const a = await now(page);
        const wall0 = Date.now();
        await act(page, { kind: "endTurn", force: true });
        await until(page, (x) => x.turn > cur && x.status === "human", o.turnTimeoutMs, `turn after ${cur}`);
        const wallMs = Date.now() - wall0;
        await sleep(2000);
        const z = await now(page);
        const data = await drain(page);
        const f = frames(data.ts, a, z);
        turns.push({ turn: cur, wallMs, frameMsMax: f.frameMsMax, frameMsP99: f.frameMsP99, over33ms: f.over33ms });
      }
    });
    phases["endturns"].turns = turns;
    const fatal = (await state(page))?.fatal ?? null;
    scene.fatal = fatal;
    const data = await drain(page);
    scene.finalStats = data.samples.at(-1)?.v ?? null;
    // The slow-state probe: the CPU raster of the field cells over the whole run (most cells build in the load) and the
    // frame CPU ms of the idle view. `slowTier` true: the launch ran about twice as slow on the CPU (an efficiency core;
    // its numbers are not comparable); null: nothing to tell.
    const all = rasterPerCell(data.samples, -Infinity, Infinity);
    const after = rasterPerCell(data.samples, phases["idle-default"]?.from ?? 0, Infinity);
    scene.rasterMsPerCell = { all: all?.ms ?? null, cells: all?.cells ?? 0, afterLoad: after?.ms ?? null, afterLoadCells: after?.cells ?? 0 };
    scene.idleCpuMs = phases["idle-default"]?.stats?.["frame.cpu_ms"]?.p50 ?? null;
    const slowBy = [];
    if (all?.ms != null && all.ms > SLOW_TIER_RASTER_MS) slowBy.push("raster");
    if (scene.idleCpuMs != null && scene.idleCpuMs > SLOW_TIER_IDLE_CPU_MS) slowBy.push("idleCpu");
    scene.slowTier = all?.ms == null && scene.idleCpuMs == null ? null : slowBy.length > 0;
    scene.slowBy = slowBy;
    scene.doc = docState(data.samples, 0, Infinity);
    scene.cpuPin = b.cpuPin;
    console.log(`  [${mode}] slow-state probe: field raster ${scene.rasterMsPerCell.all ?? "-"} ms/cell (${scene.rasterMsPerCell.cells} cells), idle frame CPU ${scene.idleCpuMs ?? "-"} ms, cpu pin ${JSON.stringify(scene.cpuPin)}: ${scene.slowTier ? `SLOW (${slowBy.join(", ")})` : scene.slowTier === false ? "fast" : "unknown"}`);
    scene.refreshHz = phases["idle-default"]?.fpsP50 ?? null;
  } catch (e) {
    error = e;
  }
  const errors = b.errors.slice();
  await b.close();
  return { phases, scene, error, errors };
}

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "bench", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  if (!o.headed) console.log("note: bench without --headed runs on SwiftShader: the numbers mean nothing (use --headed)");
  if (o.lateUrl && (!o.lateNick || !o.lateToken)) {
    check("options", false, "--late-url needs --late-nick and --late-token");
    result.durationMs = Date.now() - started;
    return result;
  }
  const modes = (o.benchModes ?? "capped,uncapped").split(",").map((x) => x.trim()).filter(Boolean);
  const meta = {
    label: o.benchLabel ?? null,
    commit: git(ctx.root, "rev-parse", "--short", "HEAD"),
    branch: git(ctx.root, "rev-parse", "--abbrev-ref", "HEAD"),
    dirty: !!git(ctx.root, "status", "--porcelain", "--", "../../crates", "../../apps", "../../tools/playtest"),
    startedAt: new Date().toISOString(),
    scene: o.lateUrl ? "late (real core)" : `heavy fixture: ${o.benchUnits ?? 1500} units, ${o.benchCities ?? 80} cities, Huge, ${(o.benchAi ?? 6) + 1} players`,
    headed: !!o.headed,
    profile: o.profile,
    preset: o.preset ?? "high",
    gfx: o.gfx ?? "default",
    ui: benchUi().ui,
    size: o.benchSize ?? "1920x1080",
    host: { cpu: os.cpus()[0]?.model, cores: os.cpus().length, ramGb: Math.round(os.totalmem() / 2 ** 30), os: `${os.type()} ${os.release()}` },
    phaseS: o.benchPhaseS ?? 10,
    turns: o.benchTurns ?? 8,
  };
  const out = { meta, modes: {}, scene: {}, turns: {} };
  for (const mode of modes) {
    console.log(`  mode ${mode}`);
    const r = await runMode(ctx, mode);
    out.modes[mode] = r.phases;
    out.scene[mode] = r.scene;
    out.turns[mode] = r.phases["endturns"]?.turns;
    result.consoleErrors.push(...r.errors);
    check(`${mode}: all phases ran`, !r.error && Object.keys(r.phases).length === 7, r.error ? r.error.message : `${Object.keys(r.phases).length} phases`);
    check(`${mode}: no fatal`, !r.scene.fatal, r.scene.fatal ?? "");
    // On WebGPU the pages must be baked from the GPU vertices: the CPU path there is a fallback (a compute pipeline
    // that failed) whose numbers are not the ones measured.
    if (r.scene.gpu?.webgpu) {
      check(`${mode}: terrain bake path is gpu`, r.scene.bakePath === "gpu", r.scene.bakePath === "cpu" ? "WebGPU fell back to the CPU page vertices (see the console warning \"terrain bake: page vertices on the CPU\")" : `terrain.vertex_path ${r.scene.bakePath ?? "missing"}`);
    }
    if (r.error) result.error = r.error.stack ?? String(r.error);
  }
  meta.finishedAt = new Date().toISOString();
  fs.writeFileSync(path.join(o.logDir, "bench.json"), JSON.stringify(out, null, 2));
  result.bench = out;
  // The report's number table gets the headline per phase; the full records are in bench.json.
  for (const [mode, phases] of Object.entries(out.modes)) {
    for (const [name, r] of Object.entries(phases)) {
      result.numbers[`${mode}/${name}`] = `p50 ${r.fpsP50} fps, 1% low ${r.fps1pLow}, frame p50 ${r.frameMsP50} ms p99 ${r.frameMsP99}, draws ${r.stats?.draws?.p50 ?? "-"}, tris ${r.stats?.triangles?.p50 ?? "-"}, passSum ${r.stats?.passSumMs?.p50 ?? "-"} ms`;
    }
  }
  result.numbers.scene = out.scene;
  check("no console errors", result.consoleErrors.length === 0, `${result.consoleErrors.length} error(s)${result.consoleErrors[0] ? `: ${result.consoleErrors[0]}` : ""}`);
  result.pass = result.checks.every((c) => c.pass);
  result.durationMs = Date.now() - started;
  console.log(`  bench.json: ${path.join(o.logDir, "bench.json")}`);
  return result;
}
