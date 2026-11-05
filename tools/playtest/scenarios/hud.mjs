// `hud` (build B3, perf-hud, AC-H1..H7, H9): the performance panel and its JSON through the server game.
// A duel with `?stats=1&hud=1`: the panel is up from the start (screenshot `hud.png`), `window.__civ5stats` carries
// the frame summary, the pass table with draw calls, GPU memory by cluster and RAM; then F3 hides the panel and
// the CPU time of the frame with the panel hidden is compared with the one shown (AC-H9: shown <= hidden + 0.5 ms).
// Headless Chromium has no GPU: the GPU numbers are SwiftShader's; only the presence of the keys is checked there.
import { openPage, act, until, appUrl, INTERIM, waitStats, stats } from "../lib/browser.mjs";

const PASSWORD = "dev";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "hud", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const n = result.numbers;
  const b = await openPage({ headed: o.headed, logDir: o.logDir });
  let error = null;
  try {
    const url = appUrl(ctx.server.url, o, { ...INTERIM, test: "1", nick: "erin", autostart: "1", seed: String(o.seed ?? 3), stats: "1", hud: "1" });
    await b.page.goto(url);
    let s = await until(b.page, (x) => x.screen === "login" || x.screen === "play", 60000, "sign-in window or the game");
    if (s.screen === "login") await act(b.page, { kind: "login", nick: "erin", password: PASSWORD });
    await until(b.page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 120000, "the game");
    await sleep(4000);

    // Panel up: the JSON of the same numbers.
    const shown = await waitStats(b.page, 15000);
    n.shown = { frame: shown.frame, verdict: shown.verdict, passes: Object.keys(shown.passes).length, gpuMemTotal: shown.gpuMem.total, ramLive: shown.ram.heap_live, wasm: shown.ram.wasm_linear, jsHeap: shown.ram.js_heap_used ?? null };
    check("frame summary (fps, p50, 1% low, CPU)", shown.frame.fps > 0 && shown.frame.p50_ms > 0 && shown.frame.low1_fps > 0 && shown.frame.cpu_ms >= 0, JSON.stringify(shown.frame));
    check("verdict is cpu / gpu / vsync / unknown", ["cpu", "gpu", "vsync", "unknown"].includes(shown.verdict), shown.verdict);
    check("pass table has draw calls, instances, switches", (shown.passes.Main?.draws ?? 0) > 0 && "pipelines" in (shown.passes.Main ?? {}) && "bind_groups" in (shown.passes.Main ?? {}), JSON.stringify(shown.passes.Main ?? null));
    check("GPU memory by cluster", shown.gpuMem.total > 0 && "terrain_pages_64" in shown.gpuMem && "render_targets" in shown.gpuMem, `total ${shown.gpuMem.total}`);
    check("RAM: heap, wasm linear memory", shown.ram.heap_live > 0 && shown.ram.wasm_linear > 0, `heap ${shown.ram.heap_live}, wasm ${shown.ram.wasm_linear}`);
    check("JS heap (Chrome)", shown.ram.js_heap_used > 0, String(shown.ram.js_heap_used));
    check("counters by prefix are grouped", Object.keys(shown.groups).includes("gpu_mem") && Object.keys(shown.groups).includes("ram") && Object.keys(shown.groups).includes("frame"), Object.keys(shown.groups).join(","));
    if (o.screenshot !== false) await b.page.screenshot({ path: `${o.logDir}/hud.png` }).catch(() => {});

    // CPU time of the frame, panel shown against hidden (median of the 10 s window, read after it filled).
    await sleep(11000);
    const withPanel = (await stats(b.page)).frame.cpu_ms;
    await b.page.keyboard.press("F3");
    await sleep(11000);
    const hidden = (await stats(b.page)).frame.cpu_ms;
    n.cpuMs = { shown: withPanel, hidden };
    n.panelOverheadMs = Number((withPanel - hidden).toFixed(3));
    console.log(`  frame CPU: panel shown ${withPanel.toFixed(3)} ms, hidden ${hidden.toFixed(3)} ms, difference ${n.panelOverheadMs} ms`);
    // Headless Chromium draws with SwiftShader (frames take seconds): the difference is noise there, so it is enforced only with a real GPU.
    const usable = (await stats(b.page)).frame.fps >= 20;
    check("AC-H9: the shown panel costs < 0.5 ms of CPU per frame", !usable || n.panelOverheadMs < 0.5, usable ? `${n.panelOverheadMs} ms` : "not measurable here (software rendering); run with --headed");
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
