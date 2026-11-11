// `ui2-script`: the UI v2 script workers in the browser, without a game. The page `ui2-selftest.html` runs the Rust
// supervisor (`Ui2ScriptSelftest` of app-web) over the page's workers (`ui2/host.js`, `ui2/worker.js`, `ui2/api.js`)
// and a small model of its own; the scenario starts a skin context with scripts of its own and checks:
// - echo: the worker says `ready`; a model write reaches the script, whose state write comes back as `out` and
//   lands in the model; a markup event does the same.
import { openPage } from "../lib/browser.mjs";

/** The skin script of the run. */
const MAIN = [
  "ui.model.on('top.gold', (g) => ui.state.set('n', g * 2));",
  "ui.on('ping', (a) => ui.state.set('sort', 'pong:' + a));",
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
