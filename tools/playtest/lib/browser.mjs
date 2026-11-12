// Chromium through the pinned Playwright (package.json), with the page's console collected.
// The browsers are Playwright's own (`%LOCALAPPDATA%\ms-playwright`, revision of the pinned
// version); CHROMIUM_PATH points at another executable.
import { execFile } from "node:child_process";
import { createWriteStream, existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

const PIN_SCRIPT = path.join(path.dirname(fileURLToPath(import.meta.url)), "pin-pcores.ps1");

/**
 * Pins the processes of the Chrome executable `exe` to the performance cores (Windows with a hybrid CPU: the cores of
 * the highest efficiency class). Without it the scheduler may keep the renderer's main thread on an efficiency core for
 * a whole launch: every frame about twice as slow. `CIV5_NO_CPU_PIN=1`
 * turns it off. Resolves to {pinned, mask, cores: [logical processor indexes]} or {skip: reason}.
 */
export function pinPerformanceCores(exe) {
  if (process.platform !== "win32") return Promise.resolve({ skip: "not Windows" });
  if (process.env.CIV5_NO_CPU_PIN === "1") return Promise.resolve({ skip: "CIV5_NO_CPU_PIN=1" });
  return new Promise((resolve) => {
    execFile("powershell", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", PIN_SCRIPT, "-Exe", exe], { windowsHide: true }, (err, stdout) => {
      const out = String(stdout ?? "").trim();
      const m = /^pinned (\d+) mask 0x([0-9A-F]+)/i.exec(out);
      if (m) {
        const mask = BigInt(`0x${m[2]}`);
        const cores = [];
        for (let i = 0n; i < 64n; i++) if ((mask >> i) & 1n) cores.push(Number(i));
        resolve({ pinned: Number(m[1]), mask: `0x${m[2]}`, cores });
      } else resolve({ skip: out || String(err?.message ?? "no output") });
    });
  });
}

/**
 * The executable whose processes the browser of `openPage` runs in. Playwright starts a headless Chromium from its
 * separate headless shell (`chromium_headless_shell-<rev>/.../chrome-headless-shell.exe`), not from the
 * `chrome.exe` that `chromium.executablePath()` names; pinning that one would pin no process of a headless run.
 * `CHROMIUM_PATH` is used as it is.
 * @param {boolean} headed
 */
export function launchedExecutable(headed) {
  if (process.env.CHROMIUM_PATH) return process.env.CHROMIUM_PATH;
  const full = chromium.executablePath();
  return headed ? full : (headlessShellOf(full) ?? full);
}

/** The headless shell of the same Playwright revision as the full Chromium `full`, or null when it is not installed. */
export function headlessShellOf(full) {
  const m = /^(.*[\\/])chromium-(\d+)[\\/]chrome-([a-z0-9-]+)[\\/]/i.exec(full);
  if (!m) return null;
  const [, base, rev, platform] = m;
  const name = process.platform === "win32" ? "chrome-headless-shell.exe" : "chrome-headless-shell";
  const shell = path.join(`${base}chromium_headless_shell-${rev}`, `chrome-headless-shell-${platform}`, name);
  return existsSync(shell) ? shell : null;
}

/** Features always disabled: Windows occlusion tracking (a covered window counts as hidden), the intensive wake-up
 * throttling of background pages and EcoQoS (efficiency mode) for background renderers. With the switches below a page
 * runs at full speed when the user works in another window while a playtest or the bench runs. */
export const DISABLED_FEATURES = ["CalculateNativeWinOcclusion", "IntensiveWakeUpThrottling", "UseEcoQoSForBackgroundProcess"];

/** Playwright's own `--disable-features` (1.60.0, chromiumSwitches.ts): ours comes after it on the command line and
 * Chrome keeps the last one, so its list is repeated in ours. */
const PLAYWRIGHT_DISABLED_FEATURES = [
  "AvoidUnnecessaryBeforeUnloadCheckSync", "BoundaryEventDispatchTracksNodeRemoval", "DestroyProfileOnBrowserClose",
  "DialMediaRouteProvider", "GlobalMediaControls", "HttpsUpgrades", "LensOverlay", "MediaRouter", "PaintHolding",
  "ThirdPartyStoragePartitioning", "Translate", "AutoDeElevate", "RenderDocument", "OptimizationHints",
];

/**
 * Whether a headless Chrome draws on SwiftShader (the CPU) instead of the GPU: `o.swiftshader`, `CIV5_SWIFTSHADER=1`,
 * or not Windows (WSL and CI have no GPU).
 * @param {{swiftshader?: boolean}} o
 */
export function headlessOnSwiftShader(o) {
  return !!o.swiftshader || process.env.CIV5_SWIFTSHADER === "1" || process.platform !== "win32";
}

/**
 * The Chrome switches of `openPage` (exported for the bench's record). Chrome keeps only one `--disable-features`, so
 * those of `o.chromeArgs` are merged with `DISABLED_FEATURES` into one.
 * @param {{headed?: boolean, uncapped?: boolean, swiftshader?: boolean, chromeArgs?: string[]}} o
 */
export function chromeSwitches(o) {
  const args = ["--no-sandbox"];
  if (!o.headed) {
    // Headless on Windows: WebGL2 on the GPU through ANGLE D3D11 (SwiftShader stays the fallback of a machine
    // without one). SwiftShader made a UI boot take minutes; it stays for pixel-stable runs (`--swiftshader`).
    if (headlessOnSwiftShader(o)) args.push("--enable-unsafe-swiftshader", "--use-angle=swiftshader");
    else args.push("--use-angle=d3d11", "--enable-unsafe-swiftshader");
  }
  // No throttling of a window that is covered, unfocused or in the background (the perf scenarios measure frame times).
  args.push("--disable-renderer-backgrounding", "--disable-background-timer-throttling", "--disable-backgrounding-occluded-windows");
  // `uncapped`: no vsync and no frame rate limit, so a frame costs what the CPU or the GPU needs (the bench scenario).
  if (o.uncapped) args.push("--disable-frame-rate-limit", "--disable-gpu-vsync");
  const features = new Set([...PLAYWRIGHT_DISABLED_FEATURES, ...DISABLED_FEATURES]);
  for (const a of o.chromeArgs ?? []) {
    if (a.startsWith("--disable-features=")) for (const f of a.slice("--disable-features=".length).split(",")) f && features.add(f);
    else args.push(a);
  }
  args.push(`--disable-features=${[...features].join(",")}`);
  return args;
}

/**
 * Opens a page; returns { browser, page, args, console: [{type, text, at}], errors: [text], close() }.
 * Errors are console errors (page and workers) and uncaught page errors.
 * A new browser per call: a fresh context, so the origin's storage (OPFS) starts empty.
 * @param {{headed?: boolean, uncapped?: boolean, chromeArgs?: string[], logDir: string, consoleFile?: string, viewport?: {width: number, height: number}}} o
 */
export async function openPage(o) {
  const args = chromeSwitches(o);
  const browser = await chromium.launch({
    headless: !o.headed,
    executablePath: process.env.CHROMIUM_PATH || undefined,
    args,
  });
  const context = await browser.newContext({ viewport: o.viewport ?? { width: 1280, height: 720 }, deviceScaleFactor: 1 });
  const page = await context.newPage();
  // The performance cores for every process of this browser; again at each load (a navigation can start a new
  // renderer process). `cpuPin` holds the last result.
  const exe = launchedExecutable(!!o.headed);
  const handle = { cpuPin: null };
  const pin = async () => (handle.cpuPin = { exe: path.basename(exe), ...(await pinPerformanceCores(exe)) });
  await pin();
  page.on("load", () => {
    pin().catch(() => {});
  });
  const log = createWriteStream(path.join(o.logDir, o.consoleFile ?? "console.log"));
  const t0 = Date.now();
  const lines = [];
  const errors = [];
  const record = (source, type, text) => {
    const at = Date.now() - t0;
    lines.push({ source, type, text, at });
    log.write(`[${(at / 1000).toFixed(1)}s ${source} ${type}] ${text}\n`);
    // A headed Chrome asks for /favicon.ico (headless does not); the app has none: not an app error.
    if (type === "error" && !/\/favicon\.ico\)?$/.test(text)) errors.push(`${source}: ${text}`);
  };
  // The page's console event carries the workers' messages too (the game worker's included).
  page.on("console", (m) => record("page", m.type(), m.type() === "error" && m.location()?.url ? `${m.text()} (${m.location().url})` : m.text()));
  page.on("pageerror", (e) => record("page", "error", `uncaught: ${e.message}`));
  page.on("crash", () => record("page", "error", "page crashed"));
  return {
    browser,
    page,
    args,
    /** Pins the browser's processes to the performance cores now; resolves to the result (see `pinPerformanceCores`). */
    pin,
    get cpuPin() {
      return handle.cpuPin;
    },
    console: lines,
    errors,
    /** Epoch ms of the console clock: a line's `at` + `t0` is its arrival time (Date.now()). */
    t0,
    async close() {
      await browser.close().catch(() => {});
      log.end();
    },
  };
}

/** `window.__civ5test.state()`, or null before the hooks exist and while the page navigates. */
export async function state(page) {
  try {
    return await page.evaluate(() => (window.__civ5test ? window.__civ5test.state() : null));
  } catch (e) {
    // A reload (the error screen's Reload, `page.reload`) destroys the context under the call.
    if (/context was destroyed|navigat|Target closed|frame was detached/i.test(e.message)) return null;
    throw e;
  }
}

/** `ui=interim`: the interim egui game panels (B1/B2) instead of the original UI, the default of the game screen. */
export const INTERIM = { ui: "interim" };

/**
 * The game page, with the hooks (`test=1`). The client has no brain of its own (net-client NC9): it plays
 * through the server (`cargo xtask dev`), so a scripted game is the quick start of the address (`autostart=1`,
 * `nick`, `ai`, `size`, `seed`, `mode` in `extra`).
 * @param {string} base server URL
 * @param {{gfx?: string, preset?: string}} o
 * @param {Record<string, string>} [extra]
 */
export function appUrl(base, o, extra = {}) {
  // The game screen is the original UI (EUI) unless `ui` says otherwise; B1/B2 scenarios drive the interim egui
  // screens through the test port and pass `ui: "interim"` (the `INTERIM` constant).
  const query = new URLSearchParams({ test: "1", ...extra });
  if (o.gfx) query.set("gfx", o.gfx);
  if (o.preset) query.set("preset", o.preset);
  return `${base}/?${query}`;
}

/** A same-origin page without the game (no worker holds the OPFS files): `<base>/__playtest_blank`. */
export async function gotoBlank(page, base) {
  const url = `${base}/__playtest_blank`;
  await page.route(url, (r) => r.fulfill({ contentType: "text/html", body: "<!doctype html><title>blank</title>" }));
  await page.goto(url);
}

/**
 * The autosave slots in the page origin's OPFS (call on the blank page): per file its size, and
 * the `C5SV` header's sequence number and turn (offsets 12 and 20) when it has one.
 */
export async function readSlots(page) {
  return page.evaluate(async () => {
    const root = await navigator.storage.getDirectory();
    const out = [];
    for await (const [name, handle] of root.entries()) {
      if (handle.kind !== "file") continue;
      const bytes = new Uint8Array(await (await handle.getFile()).arrayBuffer());
      const v = new DataView(bytes.buffer);
      const c5sv = bytes.length >= 24 && String.fromCharCode(...bytes.slice(0, 4)) === "C5SV";
      out.push({
        name,
        size: bytes.length,
        seq: c5sv ? Number(v.getBigUint64(12, true)) : null,
        turn: c5sv ? v.getInt32(20, true) : null,
        version: c5sv ? v.getUint32(4, true) : null,
      });
    }
    return out.sort((a, b) => a.name.localeCompare(b.name));
  });
}

/**
 * Changes one OPFS file in place (call on the blank page): `how` = "damage" flips the last byte
 * (the payload checksum no longer matches), "version" sets the `C5SV` format version to 99.
 * Retries while the closing worker still holds the file.
 */
export async function tamperSlot(page, name, how) {
  return page.evaluate(
    async ({ name, how }) => {
      const root = await navigator.storage.getDirectory();
      const handle = await root.getFileHandle(name);
      const bytes = new Uint8Array(await (await handle.getFile()).arrayBuffer());
      if (how === "damage") bytes[bytes.length - 1] ^= 0xff;
      else if (how === "version") new DataView(bytes.buffer).setUint32(4, 99, true);
      else throw new Error(`unknown tamper ${how}`);
      let last = null;
      for (let i = 0; i < 50; i++) {
        try {
          const w = await handle.createWritable();
          await w.write(bytes);
          await w.close();
          return { name, how, size: bytes.length };
        } catch (e) {
          last = e;
          await new Promise((r) => setTimeout(r, 100));
        }
      }
      throw new Error(`cannot write ${name}: ${last}`);
    },
    { name, how },
  );
}

/** Removes every OPFS entry of the origin (call on the blank page); returns how many. */
export async function clearOpfs(page) {
  return page.evaluate(async () => {
    const root = await navigator.storage.getDirectory();
    const names = [];
    for await (const [name] of root.entries()) names.push(name);
    for (const n of names) await root.removeEntry(n, { recursive: true });
    return names.length;
  });
}

/** Waits until a console line (page or worker) matches `re`; returns it. */
export async function waitConsole(b, re, timeoutMs, from = 0) {
  const start = Date.now();
  for (;;) {
    const hit = b.console.slice(from).find((l) => re.test(l.text));
    if (hit) return hit;
    if (Date.now() - start > timeoutMs) throw new Error(`timeout after ${timeoutMs} ms: console line ${re}`);
    await new Promise((r) => setTimeout(r, 50));
  }
}

/** Queues an action; resolves once the client has taken it (returns the state then). */
export async function act(page, action, timeoutMs = 10000) {
  const n = await page.evaluate((a) => window.__civ5test.act(a), action);
  return until(page, (s) => s.applied >= n, timeoutMs, `action ${action.kind} not taken`);
}

/** Polls the state until `pred(state)` holds; returns that state. Throws on timeout or a fatal. */
export async function until(page, pred, timeoutMs, what = "condition") {
  const start = Date.now();
  let s = null;
  for (;;) {
    s = await state(page);
    if (s && pred(s)) return s;
    if (s && s.fatal) {
      const e = new Error(`fatal: ${s.fatal}`);
      e.fatal = s.fatal;
      throw e;
    }
    if (Date.now() - start > timeoutMs) {
      const e = new Error(`timeout after ${timeoutMs} ms: ${what} (screen ${s?.screen}, turn ${s?.turn}, status ${s?.status})`);
      e.timeout = true;
      e.state = s;
      throw e;
    }
    await new Promise((r) => setTimeout(r, 50));
  }
}

/** Like `until`, but returns the last state instead of throwing on a timeout. */
export async function settle(page, pred, timeoutMs) {
  try {
    return await until(page, pred, timeoutMs);
  } catch (e) {
    if (e.timeout) return e.state;
    throw e;
  }
}

/**
 * The numbers of the performance panel (perf-hud, AC-H7) as `window.__civ5stats` has them, normalized:
 * `{ fps, frame: {fps, p50_ms, p99_ms, low1_fps, cpu_ms, gpu_ms|null, frames}, verdict, passes: {Name: {gpu_ms|null, cpu_ms,
 * draws, triangles, instances, pipelines, bind_groups}}, gpuMem: {cluster: bytes, total}, ram: {...}, groups: {prefix: {name: n}},
 * counters: {"prefix.name": n}, timing }`, or null while the page has not published (the URL needs `stats: "1"` or
 * `hud: "1"` in `appUrl`'s `extra`; it publishes once per second).
 */
export async function stats(page) {
  const s = await page.evaluate(() => (window.__civ5stats ? JSON.parse(JSON.stringify(window.__civ5stats)) : null));
  if (!s || !s.frame) return null;
  const nullIfNeg = (v) => (v < 0 ? null : v);
  const passes = {};
  for (const [name, p] of Object.entries(s.passes_full ?? {})) passes[name] = { ...p, gpu_ms: nullIfNeg(p.gpu_ms) };
  return {
    fps: s.fps ?? s.frame.fps,
    frame: { ...s.frame, gpu_ms: nullIfNeg(s.frame.gpu_ms) },
    verdict: s.verdict,
    passes,
    gpuMem: s.gpu_mem ?? {},
    ram: s.ram ?? {},
    groups: s.groups ?? {},
    counters: s.counters ?? {},
    timing: s.timing ?? {},
  };
}

/** Waits until `stats(page)` has published (up to `timeoutMs`) and returns it. */
export async function waitStats(page, timeoutMs = 10000) {
  const until = Date.now() + timeoutMs;
  for (;;) {
    const s = await stats(page).catch(() => null);
    if (s) return s;
    if (Date.now() > until) throw new Error("window.__civ5stats was not published (add stats=1 or hud=1 to the URL)");
    await new Promise((r) => setTimeout(r, 200));
  }
}
