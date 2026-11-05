// `perf` (playtest §3.2, task H8; AC-1.3, AC-3.5, AC-6.5): numbers from a VISIBLE Chrome with the
// real GPU (run it with --headed; headless has no GPU and the FPS means nothing). Duel (one AI),
// seed 1, the policy of `lib/policy.mjs` plays like in `autoplay`.
//
// The game runs through the server (`?autostart=1`, the client has no brain, net-client NC9); the Start of a run is
// the moment the sign-in is done (the quick start script runs by itself after it).
// - AC-1.3: `--perf-runs` (3) times a fresh browser: from that Start to the first frame with the terrain and its
//   textures settled (`__civ5stats.timeToMapMs`); also measured from outside (Start to the stat appearing).
// - AC-3.5: in the last run, after the map: `--perf-idle-s` (10) s of idle at the default zoom,
//   then `--perf-turns` (100) turns. The page counts every `requestAnimationFrame` (the app renders
//   in the same callbacks); frames are split into idle / the rivals' turns / the rest of the play.
// - AC-6.5: per turn, the time the game worker was busy for the end-turn (its own "host idle after N
//   steps, X ms" line: AI turn plus the next turn's start, the autosave excluded), next to the wall
//   time (end-turn request to the next human turn) and the time the rivals were seen moving.
// Thresholds: 30 s, 30 FPS, median 1 s / max 3 s.
import { openPage, state, act, until, appUrl, INTERIM } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { createPlayer, mergeTurnTimes } from "../lib/policy.mjs";
import { stats, round } from "../lib/report.mjs";

export const LIMITS = { timeToMapMs: 30000, fps: 30, aiMedianMs: 1000, aiMaxMs: 3000 };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

function pct(sorted, p) {
  if (!sorted.length) return null;
  return sorted[Math.min(sorted.length - 1, Math.max(0, Math.ceil((p / 100) * sorted.length) - 1))];
}

/** Frame statistics of the rAF timestamps `ts` that fall into `ranges` ([from, to] each; null = all). */
export function frameStats(ts, ranges) {
  const inside = (t) => !ranges || ranges.some(([a, b]) => t > a && t <= b);
  const d = [];
  const at = [];
  for (let i = 1; i < ts.length; i++) {
    if (inside(ts[i])) {
      d.push(ts[i] - ts[i - 1]);
      at.push(ts[i]);
    }
  }
  if (d.length < 2) return { frames: d.length, seconds: round(d.reduce((a, b) => a + b, 0) / 1000), meanFps: null, medianFps: null, p5Fps: null, minFps: null, over33ms: 0 };
  const sorted = d.slice().sort((a, b) => a - b);
  const total = d.reduce((a, b) => a + b, 0);
  // The worst one-second window: the fewest frames in any 1 s of consecutive frames.
  let worst1s = null;
  // (Only for one contiguous range: the frames of separate windows are not a continuous second.)
  for (let i = 0, j = 0; (!ranges || ranges.length === 1) && i < at.length; i++) {
    while (at[i] - at[j] > 1000) j++;
    if (at[i] - at[0] >= 1000) {
      const f = ((i - j + 1) * 1000) / Math.max(at[i] - at[j], 1);
      worst1s = worst1s === null ? f : Math.min(worst1s, f);
    }
  }
  return {
    frames: d.length,
    seconds: round(total / 1000),
    meanFps: round((d.length * 1000) / total),
    medianFps: round(1000 / pct(sorted, 50)),
    p5Fps: round(1000 / pct(sorted, 95)),
    minFps: round(1000 / sorted[sorted.length - 1]),
    worst1sFps: round(worst1s),
    maxFrameMs: round(sorted[sorted.length - 1]),
    p99FrameMs: round(pct(sorted, 99)),
    over33ms: d.filter((x) => x > 1000 / 30).length,
  };
}

/** GPU and graphics API as the page sees them (once per scenario). */
async function gpuInfo(page) {
  return page.evaluate(async () => {
    const out = { userAgent: navigator.userAgent };
    try {
      const a = await navigator.gpu?.requestAdapter();
      const i = a?.info;
      out.webgpu = a ? { vendor: i?.vendor, architecture: i?.architecture, device: i?.device, description: i?.description, isFallbackAdapter: a.isFallbackAdapter } : null;
    } catch (e) {
      out.webgpu = `error: ${e}`;
    }
    try {
      const gl = document.createElement("canvas").getContext("webgl2");
      const ext = gl?.getExtension("WEBGL_debug_renderer_info");
      out.webgl2 = gl ? { renderer: gl.getParameter(ext ? ext.UNMASKED_RENDERER_WEBGL : gl.RENDERER) } : null;
    } catch (e) {
      out.webgl2 = `error: ${e}`;
    }
    return out;
  });
}

/** The display's frame rate as the page sees it: rAF on a quiet page for 1.5 s. */
async function refreshRate(page) {
  return page.evaluate(
    () =>
      new Promise((ok) => {
        const t = [];
        const f = (ts) => {
          t.push(ts);
          if (ts - t[0] < 1500) requestAnimationFrame(f);
          else ok(Math.round(((t.length - 1) * 1000) / (ts - t[0])));
        };
        requestAnimationFrame(f);
      }),
  );
}

/** One fresh browser: sign-in, the quick start, wait for the map. Returns the load numbers and the open browser. */
async function loadRun(ctx, index, total) {
  const o = ctx.opts;
  const seed = ctx.seed ?? o.seed;
  const b = await openPage({ headed: o.headed, logDir: o.logDir, consoleFile: `console-run${index + 1}.log` });
  const { page } = b;
  const run = { run: index + 1 };
  try {
    const t0 = Date.now();
    const nick = `perf${index + 1}`;
    await page.goto(appUrl(ctx.server.url, o, { ...INTERIM, stats: "1", nick, autostart: "1", ai: "1", size: "duel", seed: String(seed), ...(o.civ ? { civ: o.civ } : {}) }));
    const login = await until(page, (s) => s.screen === "login" || s.screen === "play", o.lobbyTimeoutMs, "sign-in window");
    run.pageToLobbyMs = Date.now() - t0;
    if (index === 0) run.gpu = await gpuInfo(page);
    if (index === 0) run.refreshHz = await refreshRate(page);
    const click = Date.now();
    if (login.screen === "login") await act(page, { kind: "login", nick, password: DEV_PASSWORD });
    // The map: terrain with its textures (the app's own milestone).
    let fallbackEnd = false;
    for (;;) {
      const m = await page.evaluate(() => ({ s: window.__civ5stats ?? null, shown: window.__civ5test?.state()?.screen }));
      if (m.s?.timeToMapMs !== undefined) break;
      // A server session may not publish the app's milestone: the first human turn on the map is the end then.
      if (m.shown === "play" && (await state(page))?.status === "human") {
        fallbackEnd = true;
        break;
      }
      if (Date.now() - click > o.mapTimeoutMs) throw new Error(`timeout after ${o.mapTimeoutMs} ms: time to map (screen ${m.shown})`);
      await sleep(20);
    }
    run.externalMs = Date.now() - click;
    const m = await page.evaluate(() => window.__civ5test.metrics());
    const st = m.stats ?? {};
    const tm = st.timing ?? {};
    // `startGameAtMs` is the Start click of a local game; a server session has none: the numbers count from the
    // moment the sign-in was done (the same clock, the page's `performance.now()`).
    const startAt = m.startGameAtMs ?? (await page.evaluate((sinceMs) => performance.now() - sinceMs, Date.now() - click));
    run.startGameAtMs = round(startAt);
    run.timeToMapMs = st.timeToMapMs !== undefined ? round(st.timeToMapMs - startAt) : run.externalMs;
    run.milestone = st.timeToMapMs !== undefined ? "app" : "first human turn on the map (outside)";
    run.timeToTerrainMs = tm.terrainAtMs !== undefined ? round(tm.terrainAtMs - startAt) : null;
    run.timeToWorldMs = tm.worldAtMs !== undefined ? round(tm.worldAtMs - startAt) : null;
    run.timing = Object.fromEntries(Object.entries(tm).map(([k, v]) => [k, round(v)]));
    run.screen = (await state(page))?.screen;
  } catch (e) {
    run.error = e.stack ?? String(e);
  }
  return { run, b };
}

/** The rivals' turn windows from the page's state log: [{turn, from, to}] in page ms. */
function aiWindows(log) {
  const wins = [];
  let human = null;
  let open = null;
  for (const [t, status, turn] of log) {
    if (status === "human") {
      if (open) {
        wins.push({ turn: open.turn, from: open.from, to: t });
        open = null;
      }
      human = { t, turn };
    } else if (status === "ai" && !open) {
      open = { from: t, turn: human?.turn ?? null };
    }
  }
  return wins;
}

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const runs = o.perfRuns ?? 3;
  const turns = o.perfTurns ?? 100;
  const idleS = o.perfIdleS ?? 10;
  const result = { scenario: ctx.name ?? "perf", pass: false, logs: o.logDir, checks: [], numbers: {}, turns: [], consoleErrors: [] };
  const check = (n, pass, detail) => result.checks.push({ name: n, pass: !!pass, detail: detail ?? "" });
  const n = result.numbers;
  n.headed = !!o.headed;
  n.gfx = o.gfx ?? "default (WebGPU, WebGL2 fallback)";
  n.bundle = ctx.bundle ?? null;
  let error = null;
  const loads = [];
  let main = null;
  try {
    for (let i = 0; i < runs; i++) {
      const { run: r, b } = await loadRun(ctx, i, runs);
      loads.push(r);
      console.log(`  load ${r.run}: ${r.error ? `ERROR ${r.error.split("\n")[0]}` : `Start to map ${r.timeToMapMs} ms (terrain ${r.timeToTerrainMs} ms, outside ${r.externalMs} ms)`}`);
      result.consoleErrors.push(...b.errors);
      if (i === runs - 1 && !r.error) main = { r, b };
      else await b.close();
    }
  } catch (e) {
    error = e;
  }
  n.loads = loads;
  n.gpu = loads[0]?.gpu ?? null;
  n.refreshHz = loads[0]?.refreshHz ?? null;
  const toMap = loads.filter((r) => !r.error).map((r) => r.timeToMapMs);
  n.timeToMapMs = { runs: toMap, max: toMap.length ? Math.max(...toMap) : null, median: stats(toMap).median };
  check("time to map (each run)", toMap.length === runs && toMap.every((x) => x <= LIMITS.timeToMapMs), `${toMap.map((x) => `${Math.round(x)} ms`).join(", ")}; limit ${LIMITS.timeToMapMs} ms`);

  if (main) {
    const { b } = main;
    const { page } = b;
    const player = createPlayer(page, { turnTimeoutMs: o.turnTimeoutMs, verbose: o.verbose });
    let first = null;
    let last = null;
    try {
      first = await until(page, (s) => s.screen === "play" && s.status === "human", o.mapTimeoutMs, "human turn");
      await page.mouse.move(640, 360); // the middle of the map: no edge scrolling, no hover target to track
      await page.evaluate(() => {
        const P = (window.__perf = { ts: [], log: [], longtasks: [], idleFrom: performance.now(), idleTo: null });
        const api = window.__civ5test;
        const orig = api.state;
        api.state = function () {
          const s = orig.apply(this, arguments);
          P.log.push([performance.now(), s.status, s.turn]);
          return s;
        };
        const tick = (ts) => {
          P.ts.push(ts);
          requestAnimationFrame(tick);
        };
        requestAnimationFrame(tick);
        try {
          new PerformanceObserver((l) => {
            for (const e of l.getEntries()) P.longtasks.push([e.startTime, e.duration]);
          }).observe({ entryTypes: ["longtask"] });
        } catch {
          /* no longtask support */
        }
      });
      await sleep(idleS * 1000); // idle, default zoom, nothing polled
      await page.evaluate(() => (window.__perf.idleTo = performance.now()));
      // AC-R8.6: the time of each pass of the frame (`?stats=1`), GPU time where the adapter gives
      // it, else CPU time; published once per second.
      n.passes = await page.evaluate(() => window.__civ5stats?.passes ?? null);
      check("pass times published", n.passes && Object.keys(n.passes).length >= 13, n.passes ? Object.entries(n.passes).map(([k, v]) => `${k} ${Number(v).toFixed(2)}`).join(", ") : "no __civ5stats.passes");
      last = await player.play(first.turn + turns);
    } catch (e) {
      error = e;
    }
    let metrics = null;
    let data = null;
    try {
      metrics = await page.evaluate(() => window.__civ5test?.metrics() ?? null);
      last = (await state(page)) ?? last;
      data = await page.evaluate(() => ({ ...window.__perf, origin: performance.timeOrigin }));
    } catch {
      /* the page is gone */
    }
    result.turns = player.turns;
    result.stuck = player.stuck;
    mergeTurnTimes(result.turns, metrics);

    // AC-3.5: frames.
    if (data) {
      const wins = aiWindows(data.log);
      const ai = frameStats(data.ts, wins.map((w) => [w.from, w.to]));
      const idle = frameStats(data.ts, [[data.idleFrom, data.idleTo ?? data.idleFrom]]);
      const playAll = frameStats(data.ts, [[data.idleTo ?? 0, Infinity]]);
      n.fps = { idle, aiTurns: ai, playAll, aiWindows: wins.length, aiWindowSeconds: round(wins.reduce((s, w) => s + (w.to - w.from), 0) / 1000) };
      const lt = data.longtasks;
      n.longTasks = {
        total: lt.length,
        over100ms: lt.filter(([, d]) => d > 100).length,
        inAiTurns: lt.filter(([t]) => wins.some((w) => t >= w.from && t <= w.to)).length,
        maxMs: lt.length ? round(Math.max(...lt.map(([, d]) => d))) : null,
      };
      n.refreshHzNote = "frame rates are capped by the display refresh rate";

      // AC-6.5: the worker-side busy time of each turn, from the worker's own console line.
      const busy = b.console
        .map((l) => ({ at: l.at + b.t0, m: /host idle after (\d+) steps, (\d+) ms/.exec(l.text) }))
        .filter((x) => x.m)
        .map((x) => ({ at: x.at, steps: Number(x.m[1]), ms: Number(x.m[2]) }));
      const save = b.console.map((l) => /autosave of turn (\d+).*: (\d+) ms/.exec(l.text)).filter(Boolean).map((m) => ({ turn: Number(m[1]), ms: Number(m[2]) }));
      n.busyLinesSeen = busy.length;
      const epoch = (t) => data.origin + t;
      // The worker prints its line right after the new turn's world went out, so the line of turn
      // T's end-turn is the one that arrives closest to the page first seeing the human's turn T+1
      // (the page polls every 50 ms; turns can be as short as 100 ms, so a wider window would
      // pick up the neighbours' lines). A cross-check: the wall time and the seen-moving time.
      const firstHuman = new Map();
      for (const [t, status, turn] of data.log) if (status === "human" && !firstHuman.has(turn)) firstHuman.set(turn, t);
      for (const t of result.turns) {
        const w = wins.find((x) => x.turn === t.turn);
        t.aiWindowMs = w ? round(w.to - w.from) : null;
        const f = firstHuman.get(t.turn + 1);
        t.workerMs = null;
        if (f !== undefined) {
          const near = busy.filter((x) => x.at >= epoch(f) - 150 && x.at <= epoch(f) + 60).sort((a, c) => Math.abs(a.at - epoch(f)) - Math.abs(c.at - epoch(f)));
          t.workerMs = near.length ? near[0].ms : null;
        }
        t.autosaveMs = save.find((s) => s.turn === t.turn + 1)?.ms ?? null;
      }
    }
    const rows = result.turns.filter((t) => !result.stuck || t.turn !== result.stuck.turn);
    // One source for all turns: the worker's own busy time when it covers (nearly) every turn,
    // else the time the rivals were seen moving, else the wall time (an upper bound).
    const covered = (f) => rows.filter((t) => typeof f(t) === "number").length >= 0.9 * rows.length;
    const field = covered((t) => t.workerMs) ? "workerMs" : covered((t) => t.aiMs) ? "aiMs" : "wallMs";
    const pick = (t) => t[field];
    const src = () => ({ workerMs: "worker", aiMs: "aiMs", wallMs: "wall" })[field];
    const series = (f) => rows.filter((t) => t.turn >= first.turn).map(f);
    const tail = rows.filter((t) => t.turn > (last?.turn ?? 0) - 11);
    n.turnsPlayed = first && last ? last.turn - first.turn : 0;
    n.firstTurn = first?.turn ?? null;
    n.lastTurn = last?.turn ?? null;
    n.aiTurn = {
      source: [...new Set(rows.map(src))].join("+"),
      all: stats(series(pick)),
      last10: stats(tail.map(pick)),
      workerMs: stats(series((t) => t.workerMs)),
      seenMovingMs: stats(series((t) => t.aiMs)),
      wallMs: stats(series((t) => t.wallMs)),
      wallLast10: stats(tail.map((t) => t.wallMs)),
      worst: rows.map((t) => ({ turn: t.turn, ms: round(pick(t)) })).filter((x) => x.ms !== null && x.ms !== undefined).sort((a, c) => c.ms - a.ms).slice(0, 5),
      byTenTurns: Array.from({ length: Math.ceil(n.turnsPlayed / 10) }, (_, i) => {
        const from = (first?.turn ?? 0) + i * 10;
        const s = stats(rows.filter((t) => t.turn >= from && t.turn < from + 10).map(pick));
        return { turns: `${from}-${from + 9}`, median: s.median, max: s.max };
      }),
    };
    n.autosaveMs = stats(result.turns.map((t) => t.autosaveMs));
    n.citiesAtEnd = last?.cities?.length ?? null;
    n.unitsAtEnd = last?.units?.length ?? null;
    n.rivalCitiesAtEnd = last?.rivalCities ?? null;
    n.policy = player.policy;

    const fatal = error?.fatal ?? last?.fatal ?? null;
    result.consoleErrors.push(...b.errors);
    check("no fatal error", !fatal, fatal ?? "");
    check("no console errors", b.errors.length === 0, `${b.errors.length} error(s)`);
    check("turns advanced", n.turnsPlayed >= turns || last?.status === "over", `${n.turnsPlayed} of ${turns} turns`);
    check("no stuck turn (STUCK)", !result.stuck, result.stuck ? `turn ${result.stuck.turn}` : "");
    if (n.fps) {
      const f = n.fps;
      // Without --headed the frames come from SwiftShader (no GPU): the numbers are kept, the limit is not enforced
      // (AC-3.5 stays not-covered in the AC table, see ac.mjs).
      const fpsCheck = (name, ok, detail) => check(name, ok || !o.headed, o.headed ? detail : `${detail} [headless: limit not enforced]`);
      fpsCheck("fps idle (default zoom)", f.idle.p5Fps !== null && f.idle.p5Fps >= LIMITS.fps && f.idle.medianFps >= LIMITS.fps, `${f.idle.frames} frames in ${f.idle.seconds} s: median ${f.idle.medianFps}, p5 ${f.idle.p5Fps}, min ${f.idle.minFps} fps (display ${n.refreshHz} Hz); limit ${LIMITS.fps}`);
      fpsCheck("fps during the AI turns", f.aiTurns.p5Fps !== null && f.aiTurns.p5Fps >= LIMITS.fps && f.aiTurns.medianFps >= LIMITS.fps, f.aiTurns.p5Fps === null ? "no frames seen in the rivals' turns" : `${f.aiWindows} turns, ${f.aiTurns.frames} frames in ${f.aiTurns.seconds} s: median ${f.aiTurns.medianFps}, p5 ${f.aiTurns.p5Fps}, min ${f.aiTurns.minFps} fps; limit ${LIMITS.fps}`);
    } else check("fps idle (default zoom)", !o.headed, "no frame data");
    const a = n.aiTurn;
    check("AI turn median", a.all.n > 0 && a.all.median <= LIMITS.aiMedianMs, `${a.source}: median ${a.all.median} ms over ${a.all.n} turns (turns ${n.firstTurn}-${n.lastTurn}); last 10: ${a.last10.median} ms; limit ${LIMITS.aiMedianMs} ms`);
    check("AI turn max", a.all.n > 0 && a.all.max <= LIMITS.aiMaxMs, `${a.source}: max ${a.all.max} ms (last 10: ${a.last10.max} ms); limit ${LIMITS.aiMaxMs} ms; worst ${a.worst.map((x) => `t${x.turn} ${x.ms}`).join(", ")}`);
    if (o.screenshot) await page.screenshot({ path: `${o.logDir}/last.png` }).catch(() => {});
    await b.close();
  } else {
    check("play session", false, "no run reached the map");
  }
  check("no scenario error", !error, error ? error.message : "");
  if (error) result.error = error.stack ?? String(error);
  result.pass = result.checks.every((c) => c.pass);
  result.durationMs = Date.now() - started;
  return result;
}
