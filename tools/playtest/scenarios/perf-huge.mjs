// `perf-huge` (world-objects WOB9, R4.9): the "Huge, late game" scene through the server (the `autostart`
// path): a Huge map with the most AI players the fixture takes, `--perf-turns` (30) turns played by the end-turn
// button so cities, roads and units pile up, then the camera zoomed out over the map for `--perf-idle-s` (10) s.
// Reports the frame rate (rAF) and, from the `?stats=1` console tables, the budget of every object subsystem
// (draw calls, instances, bytes of instance data; the last `budget` block) and the pass table. If the
// original's `LateGameViewBench.log` (Documents\My Games\Sid Meier's Civilization 5\Logs\, read only) is
// there its lines are listed next to ours. Use --headed for FPS that mean something (real GPU).
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { openPage, act, until, appUrl, INTERIM } from "../lib/browser.mjs";
import { round } from "../lib/report.mjs";
import { frameStats } from "./perf.mjs";

const PASSWORD = "dev";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** The last `budget` block of the console: [{name, calls, instances, bytes}]. */
export function lastBudget(lines) {
  let rows = [];
  for (const l of lines) {
    const found = [...l.text.matchAll(/^budget (\S+)\s+calls (\d+) instances (\d+) bytes (\d+)$/gm)];
    if (found.length) rows = found.map((m) => ({ name: m[1], calls: +m[2], instances: +m[3], bytes: +m[4] }));
  }
  return rows;
}

function benchLog() {
  const p = path.join(os.homedir(), "Documents", "My Games", "Sid Meier's Civilization 5", "Logs", "LateGameViewBench.log");
  try {
    return { path: p, lines: fs.readFileSync(p, "utf8").split(/\r?\n/).filter(Boolean).slice(-40) };
  } catch {
    return { path: p, lines: null };
  }
}

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const turns = o.perfTurns ?? 30;
  const secs = o.perfIdleS ?? 10;
  const result = { scenario: "perf-huge", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const n = result.numbers;
  n.headed = !!o.headed;
  const b = await openPage({ headed: o.headed, logDir: o.logDir });
  let error = null;
  try {
    const url = appUrl(ctx.server.url, o, { ...INTERIM, test: "1", nick: "erin", autostart: "1", ai: "6", size: "huge", seed: String(o.seed ?? 7), stats: "1" });
    await b.page.goto(url);
    let s = await until(b.page, (x) => x.screen === "login" || x.screen === "play", 60000, "sign-in window or the game");
    if (s.screen === "login") await act(b.page, { kind: "login", nick: "erin", password: PASSWORD });
    s = await until(b.page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 180000, "the huge game");
    n.civilizations = s.players;
    const first = s.turn;
    for (let t = first; t < first + turns; t++) {
      await until(b.page, (x) => x.turn === t && x.status === "human", 120000, `turn ${t}`);
      await act(b.page, { kind: "endTurn", force: true });
    }
    s = await until(b.page, (x) => x.turn === first + turns && x.status === "human", 120000, "last turn");
    n.turnsPlayed = turns;
    n.cities = s.cities?.length ?? null;
    // Zoom out over the map and let it run.
    await b.page.mouse.move(640, 360);
    for (let i = 0; i < 12; i++) await b.page.mouse.wheel(0, 400);
    await sleep(2000);
    await b.page.evaluate(() => {
      const P = (window.__perf = { ts: [] });
      const tick = (ts) => {
        P.ts.push(ts);
        requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
    });
    await sleep(secs * 1000);
    const ts = await b.page.evaluate(() => window.__perf.ts);
    n.fps = frameStats(ts, null);
    n.passes = await b.page.evaluate(() => window.__civ5stats?.passes ?? null);
    n.budget = lastBudget(b.console);
    const total = n.budget.find((r) => r.name === "total");
    check("budget table published", !!total, total ? `${total.calls} calls, ${total.instances} instances, ${(total.bytes / 1048576).toFixed(2)} MiB` : "no budget lines in the console");
    for (const r of n.budget) console.log(`  budget ${r.name.padEnd(10)} calls ${r.calls} instances ${r.instances} bytes ${r.bytes}`);
    console.log(`  fps over ${n.fps.seconds} s: median ${n.fps.medianFps}, p5 ${n.fps.p5Fps}, min ${n.fps.minFps} (${n.headed ? "headed" : "headless: SwiftShader, the FPS mean nothing"})`);
    n.bench = benchLog();
    console.log(n.bench.lines ? `  LateGameViewBench.log (last ${n.bench.lines.length} lines):\n    ${n.bench.lines.join("\n    ")}` : `  LateGameViewBench.log not found at ${n.bench.path}`);
    if (o.screenshot) await b.page.screenshot({ path: `${o.logDir}/perf-huge.png` }).catch(() => {});
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
