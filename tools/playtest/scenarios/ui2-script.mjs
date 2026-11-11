// `ui2-script`: the UI v2 script workers in the browser, without a game. The page `ui2-selftest.html` runs the Rust
// supervisor (`Ui2ScriptSelftest` of app-web) over the page's workers (`ui2/host.js`, `ui2/worker.js`, `ui2/api.js`)
// and a small model of its own; the scenario starts a skin context with scripts of its own and checks:
// - echo: the worker says `ready`; a model write reaches the script, whose state write comes back as `out` and
//   lands in the model; a markup event does the same;
// - isolation: the forbidden globals are gone in the worker; a handler that throws is switched off with one log
//   line (also when it is called again) and the page goes on; a handler that hangs gets its worker killed after
//   1000 ms and a new one starts from the model as it is, without stalling the page.
import { openPage } from "../lib/browser.mjs";

/** The skin script of the run. */
const MAIN = [
  "ui.model.on('top.gold', (g) => ui.state.set('n', g * 2));",
  "ui.on('ping', (a) => ui.state.set('sort', 'pong:' + a));",
  "ui.on('boom', () => { throw new Error('boom'); });",
  "ui.on('hang', () => { for (;;) {} });",
  "const gone = (n) => typeof self[n] === 'undefined' && typeof Object.getPrototypeOf(self)[n] === 'undefined';",
  "ui.state.set('open', ['fetch', 'XMLHttpRequest', 'WebSocket', 'importScripts', 'indexedDB', 'caches', 'postMessage'].every(gone));",
].join("\n");

async function waitFor(page, fn, arg, timeoutMs, what) {
  const start = Date.now();
  for (;;) {
    const v = await page.evaluate(fn, arg);
    if (v) return { value: v, ms: Date.now() - start };
    if (Date.now() - start > timeoutMs) throw new Error(`timed out after ${timeoutMs} ms: ${what}`);
    await page.waitForTimeout(20);
  }
}

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "ui2-script", pass: false, logs: o.logDir, checks: [], numbers: {} };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const b = await openPage({ headed: o.headed, logDir: o.logDir });
  const { page } = b;
  try {
    await page.goto(`${ctx.server.url}/ui2-selftest.html`);
    const boot = await waitFor(page, () => window.__ui2self ?? null, null, 30000, "the selftest page");
    check("the page has the worker host and the supervisor", !boot.value.error, boot.value.error ?? "");
    if (boot.value.error) throw new Error(boot.value.error);

    // ---- part 1: echo
    await page.evaluate((code) => window.__ui2self.start([{ path: "js/main.js", code }], ["top"]), MAIN);
    const ready = await waitFor(page, () => window.__ui2self.status() === "running", null, 10000, "ready");
    check("the worker says ready", true, `${ready.ms} ms`);
    result.numbers.readyMs = ready.ms;
    await page.evaluate(() => window.__ui2self.write("top.gold", 21));
    const echo = await waitFor(page, () => window.__ui2self.get("state.n") === 42, null, 5000, "state.n = 42");
    check("a model write comes back as the script's state", true, `${echo.ms} ms`);
    result.numbers.echoMs = echo.ms;
    await page.evaluate(() => window.__ui2self.event("ping", 7));
    const ping = await waitFor(page, () => window.__ui2self.get("state.sort") === "pong:7", null, 5000, "state.sort");
    check("a markup event reaches its handler", true, `${ping.ms} ms`);
    const outs = await page.evaluate(() => window.__ui2self.answers.filter((a) => a.state.length).length);
    check("the supervisor got the answers as out messages", outs >= 2, `${outs} answers with state`);

    // ---- part 2: isolation
    check("the forbidden globals are gone in the worker", (await page.evaluate(() => window.__ui2self.get("state.open"))) === true);
    const boomLines = () =>
      page.evaluate(() => window.__ui2self.answers.flatMap((a) => a.log).filter((l) => /on\(boom\) disabled: boom/.test(l[1])).length);
    await page.evaluate(() => window.__ui2self.event("boom", null));
    await waitFor(page, () => window.__ui2self.answers.some((a) => a.log.some((l) => /on\(boom\)/.test(l[1]))), null, 5000, "the boom line");
    await page.evaluate(() => window.__ui2self.event("boom", null));
    await page.evaluate(() => window.__ui2self.write("top.gold", 5));
    await waitFor(page, () => window.__ui2self.get("state.n") === 10, null, 5000, "echo after the exception");
    const booms = await boomLines();
    check("an exception switches its handler off with one log line, the rest goes on", booms === 1, `${booms} lines`);

    await page.evaluate(() => window.__ui2self.gaps());
    const hangAt = await page.evaluate(() => performance.now());
    await page.evaluate(() => window.__ui2self.event("hang", null));
    const busy = await waitFor(page, () => window.__ui2self.status() === "busy", null, 3000, "busy");
    const killed = await waitFor(
      page,
      () => window.__ui2self.answers.find((a) => a.log.some((l) => /skin killed after 1000 ms/.test(l[1]))),
      null,
      5000,
      "the kill",
    );
    const back = await waitFor(page, () => window.__ui2self.status() === "running", null, 15000, "running again");
    const t = await page.evaluate(() => performance.now());
    const killedAfter = killed.value.at - hangAt;
    result.numbers.hang = { busyMs: busy.ms, killedMs: Math.round(killedAfter), runningAgainMs: Math.round(t - hangAt) };
    check("a hung handler makes the context busy, then its worker is killed", killedAfter >= 1000 && killedAfter < 2000, JSON.stringify(result.numbers.hang));
    await page.evaluate(() => window.__ui2self.write("top.gold", 50));
    await waitFor(page, () => window.__ui2self.get("state.n") === 100, null, 5000, "echo after the restart");
    check("the new worker runs the scripts on the current model", true);
    const restarts = await page.evaluate(() => window.__ui2self.answers.at(-1).stats.restarts);
    check("one restart is counted", restarts === 1, `${restarts}`);
    const gap = await page.evaluate(() => window.__ui2self.gaps());
    result.numbers.longestFrameGapMs = Math.round(gap);
    check("the page does not stall while the worker hangs", gap < 250, `${Math.round(gap)} ms between frames`);

    // The module's start finds no game canvas on this page and says so: not an error of the scripts.
    const errors = b.errors.filter((e) => !/page has no <canvas/.test(e));
    result.pass = result.checks.every((c) => c.pass) && errors.length === 0;
    if (errors.length) result.error = `console errors: ${errors.slice(0, 3).join(" | ")}`;
  } catch (e) {
    result.error = e.stack ?? String(e);
  } finally {
    result.durationMs = Date.now() - started;
    await page.screenshot({ path: `${o.logDir}/ui2-script.png` }).catch(() => {});
    await b.close();
  }
  return result;
}
