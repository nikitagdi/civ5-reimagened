// M2 join-time baseline: opens the quick-start page of a running server N times (a fresh browser each time) and
// measures the time from navigation to the sign-in window, to the game screen, and to the first human turn on the
// map, plus window.__civ5stats (timeToMapMs, timing.*), the transfer sizes of the wasm/js, and WebGPU availability.
//   node tools/playtest/m2/join-baseline.mjs --url http://127.0.0.1:8090 [--runs 3] [--size small] [--ai 2] [--seed 7]
//        [--headed] [--out D:\wo6\infra\baseline.json] [--gfx webgl2|webgpu] [--tag b3] [--timeout-ms 300000]
// The server needs the real core and an asset dir for a real game: tools/playtest/m2/server.ps1.
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { openPage, state, appUrl, INTERIM, act, until } from "../lib/browser.mjs";

const a = process.argv.slice(2);
const opt = (k, d) => { const i = a.indexOf(k); return i >= 0 ? a[i + 1] : d; };
const url = opt("--url", "http://127.0.0.1:8090").replace(/\/+$/, "");
const runs = Number(opt("--runs", "3"));
const size = opt("--size", "small");
const ai = opt("--ai", "2");
const seed = opt("--seed", "7");
const headed = a.includes("--headed");
const gfx = opt("--gfx", null);
const tag = opt("--tag", "run");
const timeout = Number(opt("--timeout-ms", "300000"));
const out = path.resolve(opt("--out", `baseline-${tag}.json`));
const logDir = path.resolve(path.dirname(out), `baseline-${tag}-logs`);
mkdirSync(logDir, { recursive: true });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const results = [];

for (let i = 1; i <= runs; i++) {
  const nick = `base${tag.replace(/\W/g, "")}${i}${Date.now() % 10000}`;
  const b = await openPage({ headed, logDir, consoleFile: `console-${i}.log` });
  const r = { run: i, nick };
  try {
    const t0 = Date.now();
    await b.page.goto(appUrl(url, { gfx }, { ...INTERIM, nick, autostart: "1", ai, size, seed, mode: "simultaneous" }));
    r.loadMs = Date.now() - t0;
    let s = null, seenLogin = false, loggedIn = false, playAt = null;
    while (Date.now() - t0 < timeout) {
      s = await state(b.page);
      if (s?.screen === "login") {
        if (!seenLogin) { seenLogin = true; r.loginWindowMs = Date.now() - t0; }
        if (!loggedIn) { await act(b.page, { kind: "login", nick, password: "dev" }); loggedIn = true; r.loginSentMs = Date.now() - t0; }
      }
      if (s?.screen === "play" && playAt === null) { playAt = Date.now() - t0; r.gameScreenMs = playAt; }
      if (s?.screen === "play" && s.status === "human") { r.firstHumanTurnMs = Date.now() - t0; break; }
      await sleep(100);
    }
    if (r.firstHumanTurnMs === undefined) throw new Error(`no map within ${timeout} ms (last screen ${s?.screen}/${s?.status})`);
    await sleep(3000); // let the per-second stats publish
    r.stats = await b.page.evaluate(() => {
      const st = window.__civ5stats ?? {};
      const res = performance.getEntriesByType("resource").filter((e) => /\.(wasm|js)(\?|$)/.test(e.name)).map((e) => ({ name: e.name.split("/").slice(-2).join("/"), transfer: e.transferSize, encoded: e.encodedBodySize, decoded: e.decodedBodySize, ms: Math.round(e.duration) }));
      return { timeToMapMs: st.timeToMapMs, timing: st.timing, fps: st.fps, counters: st.counters, net: st.net, keys: Object.keys(st), resources: res, wasmTotal: res.filter((x) => x.name.endsWith(".wasm")) };
    });
    r.gpu = await b.page.evaluate(async () => {
      let adapter = null;
      try { adapter = navigator.gpu ? !!(await navigator.gpu.requestAdapter()) : false; } catch { adapter = false; }
      const c = document.createElement("canvas").getContext("webgl2");
      const dbg = c?.getExtension("WEBGL_debug_renderer_info");
      return { webgpuAvailable: !!navigator.gpu, webgpuAdapter: adapter, webgl2Renderer: dbg ? c.getParameter(dbg.UNMASKED_RENDERER_WEBGL) : null, crossOriginIsolated: self.crossOriginIsolated };
    });
    r.consoleErrors = b.errors.slice(0, 5);
    r.state = { turn: s.turn, players: s.players };
  } catch (e) {
    r.error = e.message;
  }
  await b.close();
  results.push(r);
  console.log(JSON.stringify({ run: i, loadMs: r.loadMs, loginWindowMs: r.loginWindowMs, gameScreenMs: r.gameScreenMs, firstHumanTurnMs: r.firstHumanTurnMs, timeToMapMs: r.stats?.timeToMapMs, error: r.error }));
}
writeFileSync(out, JSON.stringify({ url, size, ai, seed, headed, gfx, tag, at: new Date().toISOString(), results }, null, 1));
const ok = results.filter((r) => r.firstHumanTurnMs !== undefined);
const med = (f) => { const v = ok.map(f).filter((x) => typeof x === "number").sort((x, y) => x - y); return v.length ? v[Math.floor(v.length / 2)] : null; };
console.log(`median over ${ok.length}/${runs}: gameScreenMs ${med((r) => r.gameScreenMs)}, firstHumanTurnMs ${med((r) => r.firstHumanTurnMs)}, timeToMapMs ${med((r) => r.stats?.timeToMapMs)}`);
console.log(`written ${out}`);
process.exit(ok.length === runs ? 0 : 1);
