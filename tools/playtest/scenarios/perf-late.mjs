// `perf-late` (AC-R8.2 on the real core): the frame rate of a late Huge game (turn 150+) from the stored save
// (`assets-out/saves/late-huge`, made by net-test `make-late-save`), seen by one of its humans in a visible Chrome.
// Unlike the other scenarios it does not start the server: the real core runs in WSL (the gamecore is a Linux
// library) on a copy of the save's state dir, and the scenario attaches to it:
//   --late-url http://127.0.0.1:8099 --late-nick late1 --late-token <file with the token of the nick>
// (`cargo xtask playtest --scenario perf-late --headed --release --late-url ... --late-nick ... --late-token ...`;
// the xtask still starts a fixture server that this scenario ignores.) The token goes to the page's
// `localStorage` (`civ5.token.<nick>`, as the sign-in leaves it), so no password is needed.
// Measured, each over `--perf-idle-s` (10) s of requestAnimationFrame stamps: the start view (as the game opens), then
// a pan (the mouse wheel at the default zoom and a drag across the map, `--scroll-s` seconds). Limits: 60 FPS at the
// start, 30 FPS in the late Huge game (AC-R8.2).
import fs from "node:fs";
import { openPage, state, appUrl, INTERIM } from "../lib/browser.mjs";
import { frameStats } from "./perf.mjs";
import { lastBudget } from "./perf-huge.mjs";

export const LIMITS = { startFps: 60, lateFps: 30 };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "perf-late", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const n = result.numbers;
  n.headed = !!o.headed;
  if (!o.lateUrl || !o.lateNick || !o.lateToken) {
    check("options", false, "perf-late needs --late-url, --late-nick and --late-token");
    result.durationMs = Date.now() - started;
    return result;
  }
  const token = fs.readFileSync(o.lateToken, "utf8").trim();
  const secs = o.perfIdleS ?? 10;
  const scrollS = o.scrollS ?? 8;
  const b = await openPage({ headed: o.headed, logDir: o.logDir, viewport: { width: 1920, height: 1080 } });
  let error = null;
  try {
    await b.page.addInitScript(([nick, tok]) => {
      try {
        localStorage.setItem("civ5.nick", nick);
        localStorage.setItem(`civ5.token.${nick}`, tok);
      } catch {
        /* storage blocked */
      }
    }, [o.lateNick, token]);
    await b.page.goto(appUrl(o.lateUrl, o, { ...INTERIM, nick: o.lateNick, stats: "1" }));
    const t0 = Date.now();
    let s = null;
    let last = "";
    while (Date.now() - t0 < 240000) {
      s = await state(b.page);
      const line = s ? `${s.screen}/${s.net?.phase}/turn ${s.turn}` : "no hooks";
      if (line !== last) console.log(`  state: ${line}`);
      last = line;
      if (s && s.screen === "play" && s.net?.phase === "play" && s.turn !== null) break;
      await sleep(500);
    }
    check("the late game opens", s && s.screen === "play" && s.turn !== null, last);
    if (!s || s.screen !== "play") throw new Error(`no game: ${last}`);
    n.turn = s.turn;
    n.civilizations = s.players;
    n.timeToPlayMs = Date.now() - t0;
    // Let the terrain and the objects settle (streaming), then count frames.
    await sleep(8000);
    const gpu = await b.page.evaluate(() => ({ dpr: window.devicePixelRatio, w: innerWidth, h: innerHeight }));
    n.viewport = gpu;
    await b.page.evaluate(() => {
      const P = (window.__perf = { ts: [] });
      const tick = (ts) => {
        P.ts.push(ts);
        requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
    });
    const mark = async () => b.page.evaluate(() => performance.now());
    const a0 = await mark();
    await sleep(secs * 1000);
    const a1 = await mark();
    if (o.screenshot !== false) await b.page.screenshot({ path: `${o.logDir}/perf-late-start.png` }).catch(() => {});
    // Pan at the default zoom: the arrow keys reach the canvas when it has the focus (a click on the ground gives it).
    await b.page.mouse.click(960, 540);
    await b.page.evaluate(() => {
      const c = document.querySelector("canvas");
      if (c) {
        if (c.tabIndex < 0) c.tabIndex = 0;
        c.focus();
      }
    });
    const p0 = await mark();
    const panEnd = Date.now() + scrollS * 1000;
    const keys = ["ArrowRight", "ArrowDown", "ArrowLeft", "ArrowUp"];
    for (let k = 0; Date.now() < panEnd; k++) {
      await b.page.keyboard.down(keys[k % 4]);
      await sleep(Math.max(250, (scrollS * 1000) / 8));
      await b.page.keyboard.up(keys[k % 4]);
    }
    const p1 = await mark();
    // Zoomed out over the map (the wheel), the heaviest view: the same pan.
    for (let i = 0; i < 8; i++) await b.page.mouse.wheel(0, 400);
    await sleep(2000);
    const z0 = await mark();
    const zoomEnd = Date.now() + scrollS * 1000;
    for (let k = 0; Date.now() < zoomEnd; k++) {
      await b.page.keyboard.down(keys[k % 4]);
      await sleep(Math.max(250, (scrollS * 1000) / 8));
      await b.page.keyboard.up(keys[k % 4]);
    }
    const z1 = await mark();
    const ts = await b.page.evaluate(() => window.__perf.ts);
    n.start = frameStats(ts, [[a0, a1]]);
    n.pan = frameStats(ts, [[p0, p1]]);
    n.panZoomedOut = frameStats(ts, [[z0, z1]]);
    console.log(`  pan zoomed out: median ${n.panZoomedOut.medianFps} fps, p5 ${n.panZoomedOut.p5Fps}, min ${n.panZoomedOut.minFps}`);
    n.passes = await b.page.evaluate(() => window.__civ5stats?.passes ?? null);
    n.budget = lastBudget(b.console);
    console.log(`  start view: median ${n.start.medianFps} fps, p5 ${n.start.p5Fps}, min ${n.start.minFps} (${n.start.frames} frames, ${n.start.seconds} s)`);
    console.log(`  pan: median ${n.pan.medianFps} fps, p5 ${n.pan.p5Fps}, min ${n.pan.minFps} (${n.pan.frames} frames, ${n.pan.seconds} s)`);
    for (const r of n.budget) console.log(`  budget ${r.name.padEnd(10)} calls ${r.calls} instances ${r.instances} bytes ${r.bytes}`);
    if (o.screenshot !== false) await b.page.screenshot({ path: `${o.logDir}/perf-late.png` }).catch(() => {});
    if (n.headed) {
      check("start view >= 60 FPS (median)", n.start.medianFps >= LIMITS.startFps, `${n.start.medianFps}`);
      check("late Huge pan >= 30 FPS (median)", n.pan.medianFps >= LIMITS.lateFps, `${n.pan.medianFps}`);
      check("late Huge pan zoomed out >= 30 FPS (median)", n.panZoomedOut.medianFps >= LIMITS.lateFps, `${n.panZoomedOut.medianFps}`);
    }
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
