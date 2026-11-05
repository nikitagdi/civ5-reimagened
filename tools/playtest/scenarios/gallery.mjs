// `gallery`: the full-set test world (the server's `CIV5_FIXTURE_GALLERY`: one unit of every type and one landmark of
// every row of the asset report directory, the map revealed for good with `CIV5_FIXTURE_REVEAL_ALL`) in the browser,
// on the backend `--gfx` names (`webgl2` runs the GL path of wgpu: its limits, 16 texture units, must hold for every
// pipeline the full set draws). Checks the page does not fail (no console error, no fatal) and that frames keep coming;
// reports the frame rate over `--perf-idle-s` seconds (use --headed for numbers that mean something) and the pass
// table of `?stats=1` (`window.__civ5stats.passes`). Not in `all`; the folder comes from `--gallery <dir>` (default:
// the assets directory).
//   cargo xtask playtest --scenario gallery --assets <dir> [--gallery <report dir>] [--gfx webgl2] [--headed]
import { openPage, act, until, appUrl, INTERIM } from "../lib/browser.mjs";
import { frameStats } from "./perf.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const secs = o.perfIdleS ?? 10;
  const result = { scenario: "gallery", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const n = result.numbers;
  n.headed = !!o.headed;
  n.gfx = o.gfx ?? "default";
  const b = await openPage({ headed: o.headed, logDir: o.logDir });
  let error = null;
  try {
    // CIV5_PLAYTEST_DISABLE=Forest,Unit: `?disable=` (the [DisableSystems] names), to find which subsystem costs the frame time.
    const extra = { ...INTERIM, nick: "gallery", autostart: "1", ai: "1", size: "duel", seed: String(o.seed ?? 1), stats: "1" };
    if (process.env.CIV5_PLAYTEST_DISABLE) extra.disable = process.env.CIV5_PLAYTEST_DISABLE;
    await b.page.goto(appUrl(ctx.server.url, o, extra));
    let s = await until(b.page, (x) => x.screen === "login" || x.screen === "play", o.lobbyTimeoutMs, "sign-in window or the game");
    if (s.screen === "login") await act(b.page, { kind: "login", nick: "gallery", password: "dev" });
    s = await until(b.page, (x) => x.screen === "play" && x.status === "human", o.mapTimeoutMs, "the map");
    n.units = s.units?.length ?? null;
    // CIV5_PLAYTEST_PROFILE=<seconds>: the V8 profile of that many seconds from here (CDP), the top self times by function:
    // where the frame time goes (the wasm, or the GL calls its imports make).
    const profileS = Number(process.env.CIV5_PLAYTEST_PROFILE ?? 0);
    let cdp = null;
    if (profileS > 0) {
      // CIV5_PLAYTEST_PROFILE_AFTER=<seconds>: the profile starts that long after the map (the late frames, not the load).
      await sleep(Number(process.env.CIV5_PLAYTEST_PROFILE_AFTER ?? 0) * 1000);
      cdp = await b.page.context().newCDPSession(b.page);
      await cdp.send("Profiler.enable");
      await cdp.send("Profiler.setSamplingInterval", { interval: 1000 });
      await cdp.send("Profiler.start");
      await sleep(profileS * 1000);
      const { profile } = await cdp.send("Profiler.stop");
      const self = new Map();
      const dt = profile.timeDeltas;
      const byId = new Map(profile.nodes.map((nd) => [nd.id, nd]));
      profile.samples.forEach((id, i) => {
        const nd = byId.get(id);
        const key = `${nd.callFrame.functionName || "(anonymous)"} ${nd.callFrame.url.split("/").pop()}:${nd.callFrame.lineNumber}`;
        self.set(key, (self.get(key) ?? 0) + (dt[i] ?? 0));
      });
      const total = [...self.values()].reduce((a, v) => a + v, 0);
      n.profile = [...self.entries()].sort((a, c) => c[1] - a[1]).slice(0, 25).map(([k, v]) => ({ fn: k, ms: Math.round(v / 1000), pct: Math.round((v / total) * 100) }));
      for (const r of n.profile) console.log(`  prof ${String(r.ms).padStart(7)} ms ${String(r.pct).padStart(3)}%  ${r.fn}`);
      // Inclusive time of the functions (wasm names only): who calls the expensive ones.
      const parent = new Map();
      for (const nd of profile.nodes) for (const c of nd.children ?? []) parent.set(c, nd.id);
      const incl = new Map();
      profile.samples.forEach((id, i) => {
        const seen = new Set();
        for (let cur = id; cur !== undefined; cur = parent.get(cur)) {
          const f = byId.get(cur).callFrame;
          const key = `${f.functionName || "(anonymous)"}`.replace(/::h[0-9a-f]{16}$/, "");
          if (!seen.has(key)) {
            seen.add(key);
            incl.set(key, (incl.get(key) ?? 0) + (dt[i] ?? 0));
          }
        }
      });
      for (const [k, v] of [...incl.entries()].sort((a, c) => c[1] - a[1]).slice(0, 45)) console.log(`  incl ${String(Math.round(v / 1000)).padStart(7)} ms  ${k.slice(0, 150)}`);
    }
    await sleep(3000);
    // Frame rate of the idle view (rAF timestamps, as the perf scenarios do).
    await b.page.evaluate(() => {
      const P = (window.__perf = { ts: [] });
      const tick = (ts) => {
        P.ts.push(ts);
        requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
    });
    // CIV5_PLAYTEST_ANIM=1: the `anim.*` counters of `?stats=1` every 10 s (clip bakes: one-time load or rebake thrash).
    const animEvery = process.env.CIV5_PLAYTEST_ANIM ? 10 : secs;
    for (let t = 0; t < secs; t += animEvery) {
      await sleep(Math.min(animEvery, secs - t) * 1000);
      if (process.env.CIV5_PLAYTEST_ANIM) {
        const st = await b.page.evaluate(() => {
          const flat = (o, p = "", out = {}) => {
            for (const [k, v] of Object.entries(o ?? {})) {
              if (v && typeof v === "object") flat(v, `${p}${k}.`, out);
              else if (`${p}${k}`.includes("anim")) out[`${p}${k}`] = v;
            }
            return out;
          };
          return flat(window.__civ5stats);
        });
        console.log(`  anim @${t + animEvery} s: ${JSON.stringify(st)}`);
      }
    }
    const ts = await b.page.evaluate(() => window.__perf.ts);
    n.fps = frameStats(ts, null);
    // Windows of 10 s: the first ones carry the build of the revealed map, the last ones the steady state.
    n.windows = [];
    for (let from = ts[0] ?? 0; from < (ts.at(-1) ?? 0); from += 10000) {
      const w = frameStats(ts, [[from, from + 10000]]);
      n.windows.push({ fromS: Math.round((from - (ts[0] ?? 0)) / 1000), frames: w.frames, medianFps: w.medianFps, p5Fps: w.p5Fps, minFps: w.minFps, maxFrameMs: w.maxFrameMs });
    }
    for (const w of n.windows) console.log(`  window ${String(w.fromS).padStart(4)} s: ${w.frames} frames, median ${w.medianFps} fps, p5 ${w.p5Fps}, min ${w.minFps}, longest frame ${w.maxFrameMs} ms`);
    n.passes = await b.page.evaluate(() => window.__civ5stats?.passes ?? null);
    const fatal = (await b.page.evaluate(() => window.__civ5test.state()))?.fatal ?? null;
    check("no fatal error", !fatal, fatal ?? "");
    check("frames keep coming", ts.length >= 3, `${ts.length} frames in ${secs} s: median ${n.fps.medianFps} fps, p5 ${n.fps.p5Fps}, min ${n.fps.minFps}`);
    console.log(`  fps over ${n.fps.seconds} s: median ${n.fps.medianFps}, p5 ${n.fps.p5Fps}, min ${n.fps.minFps} (${n.gfx}, ${n.headed ? "headed" : "headless: SwiftShader, the FPS mean nothing"})`);
    if (n.passes) for (const p of Object.entries(n.passes)) console.log(`  pass ${p[0]}: ${JSON.stringify(p[1])}`);
    if (o.screenshot) await b.page.screenshot({ path: `${o.logDir}/gallery.png` }).catch(() => {});
  } catch (e) {
    error = e;
  }
  result.consoleErrors.push(...b.errors);
  await b.close();
  check("no console errors", result.consoleErrors.length === 0, `${result.consoleErrors.length} error(s)${result.consoleErrors[0] ? `: ${result.consoleErrors[0]}` : ""}`);
  if (error) {
    check("no scenario error", false, error.message);
    result.error = error.stack ?? String(error);
  }
  result.pass = result.checks.every((c) => c.pass);
  result.durationMs = Date.now() - started;
  return result;
}
