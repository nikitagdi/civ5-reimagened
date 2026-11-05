// `cold-start` (AC-R8.4, R5.10): the time from opening the page to the map with its art, on an empty cache and on
// a warm one, with the network held to 100 Mbit/s (Chrome's network emulation through the DevTools protocol). A duel
// through the server (the `autostart` path). One browser: the first page of a fresh context is the cold start
// (nothing cached, OPFS empty); a second page in the same context is the warm one (the HTTP cache and the
// page's own storage hold what the first loaded). "The map with its art" is: the play screen, nothing loading, every
// terrain cell in view with its page, and the renderer settled for 2 s.
// The numbers: seconds to the map and the megabytes that crossed the (emulated) wire, cold and warm. The limits are the
// provisional ones of AC-R8.4: 60 s cold, 15 s warm (constants here: the user may change them later).
import { chromium } from "playwright";
import { createWriteStream } from "node:fs";
import { join } from "node:path";
import { until, act, appUrl, INTERIM, headlessOnSwiftShader } from "../lib/browser.mjs";

export const LIMITS = { coldS: 60, warmS: 15 };
const MBIT = 100;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function load(context, ctx, label, logDir) {
  const o = ctx.opts;
  const page = await context.newPage();
  const errors = [];
  const log = createWriteStream(join(logDir, `cold-start-${label}.console.log`));
  let t0 = Date.now();
  page.on("console", (m) => {
    // Milliseconds since the page was asked for: the per-stage numbers are read off these stamps.
    log.write(`${String(Date.now() - t0).padStart(6)} [${m.type()}] ${m.text()}\n`);
    // A headed Chrome asks for /favicon.ico; the app has none: not an app error.
    if (m.type() === "error" && !/favicon\.ico/.test(m.location()?.url ?? "")) errors.push(m.text());
  });
  page.on("pageerror", (e) => errors.push(`uncaught: ${e.message}`));
  const cdp = await context.newCDPSession(page);
  await cdp.send("Network.enable");
  let wire = 0;
  let requests = 0;
  let cached = 0;
  cdp.on("Network.loadingFinished", (e) => {
    wire += e.encodedDataLength;
    requests++;
  });
  cdp.on("Network.requestServedFromCache", () => cached++);
  await cdp.send("Network.emulateNetworkConditions", { offline: false, latency: 20, downloadThroughput: (MBIT * 1e6) / 8, uploadThroughput: (MBIT * 1e6) / 8 });
  // Frames the page drew (rAF callbacks), for the frame time of the load in the state lines.
  await page.addInitScript(() => {
    window.__csFrames = 0;
    const tick = () => {
      window.__csFrames++;
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  });
  t0 = Date.now();
  await page.goto(appUrl(ctx.server.url, { gfx: o.gfx }, { ...INTERIM, nick: "cold", autostart: "1", ai: "1", size: "duel", seed: String(o.seed) }));
  let s = await until(page, (x) => x.screen === "login" || x.screen === "play", 120000, "sign-in window or the game");
  if (s.screen === "login") await act(page, { kind: "login", nick: "cold", password: "dev" });
  const ready = (x) => x.screen === "play" && !x.loading && x.terrainCells && x.terrainCells.visible > 0 && x.terrainCells.unpaged === 0;
  // Ready, and still ready 2 s later (the pages and the models come a few at a time).
  let at = null;
  // The state over time (a line per change), next to the console stamps.
  let last = "";
  const watch = setInterval(async () => {
    const x = await page.evaluate(() => (window.__civ5test ? { ...window.__civ5test.state(), frames: window.__csFrames } : null)).catch(() => null);
    if (!x) return;
    const line = `screen ${x.screen} loading ${x.loading} cells ${x.terrainCells ? `${x.terrainCells.visible - x.terrainCells.unpaged}/${x.terrainCells.visible}` : "-"}`;
    if (line !== last) log.write(`${String(Date.now() - t0).padStart(6)} [state] ${line} frames ${x.frames}\n`);
    last = line;
  }, 250);
  for (;;) {
    s = await until(page, ready, 300000, `the map with its art (${label})`);
    const mark = Date.now();
    await sleep(2000);
    const again = await page.evaluate(() => (window.__civ5test ? window.__civ5test.state() : null));
    if (again && ready(again)) {
      at = mark;
      break;
    }
  }
  clearInterval(watch);
  const out = { seconds: (at - t0) / 1000, megabytes: wire / 1048576, requests, fromCache: cached, errors };
  await page.close();
  log.end();
  return out;
}

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "cold-start", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const browser = await chromium.launch({
    headless: !o.headed,
    executablePath: process.env.CHROMIUM_PATH || undefined,
    args: o.headed ? ["--no-sandbox", "--disable-renderer-backgrounding", "--disable-background-timer-throttling"] : ["--no-sandbox", "--enable-unsafe-swiftshader", headlessOnSwiftShader({}) ? "--use-angle=swiftshader" : "--use-angle=d3d11"],
  });
  let error = null;
  try {
    const context = await browser.newContext({ viewport: { width: 1920, height: 1080 }, deviceScaleFactor: 1 });
    const cold = await load(context, ctx, "cold", o.logDir);
    const warm = await load(context, ctx, "warm", o.logDir);
    result.numbers = { networkMbit: MBIT, cold, warm, limits: LIMITS };
    result.consoleErrors.push(...cold.errors, ...warm.errors);
    check("AC-R8.4: cold start", cold.seconds <= LIMITS.coldS, `${cold.seconds.toFixed(1)} s, ${cold.megabytes.toFixed(1)} MB over ${cold.requests} requests at ${MBIT} Mbit/s; limit ${LIMITS.coldS} s (provisional)`);
    check("AC-R8.4: warm start", warm.seconds <= LIMITS.warmS, `${warm.seconds.toFixed(1)} s, ${warm.megabytes.toFixed(1)} MB over ${warm.requests} requests (${warm.fromCache} from the HTTP cache: the assets come from Cache Storage, which this counter does not see; the wire carries the wasm only); limit ${LIMITS.warmS} s (provisional)`);
  } catch (e) {
    error = e;
  }
  await browser.close().catch(() => {});
  check("no console errors", result.consoleErrors.length === 0, `${result.consoleErrors.length} error(s)${result.consoleErrors[0] ? `: ${result.consoleErrors[0]}` : ""}`);
  if (error) {
    check("no scenario error", false, error.message);
    result.error = error.stack ?? String(error);
  }
  result.pass = result.checks.every((c) => c.pass);
  result.durationMs = Date.now() - started;
  return result;
}
