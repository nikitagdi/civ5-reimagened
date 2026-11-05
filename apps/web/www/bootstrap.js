// Fetches .wasm, times instantiation, passes metrics to wasm (S5).
//
// B3 render-mt, threads by default (the user, 2026-10-29): the threaded build of `./pkg-mt/` loads when
// the client can run it (`crossOriginIsolated` from the gateway's COOP/COEP, `SharedArrayBuffer`, a wasm
// shared memory validates, two cores), and the workers of the jobs pool start on its module and shared
// memory (`jobs-worker.js`). Otherwise, or with `?mt=0`, the single-threaded build of `./pkg/` is the
// fallback. `?mt=N` sets the number of workers. The mode goes to the console and `window.__civ5threads`;
// the app counts the workers that joined as `app.threads` (0: single).
// The log ring first: it must be there before the module logs its first line, and it
// keeps the lines through a trap and a reload (`clientlog.js`).
import "./clientlog.js";
import init, { on_loaded } from "./pkg/app_web.js";

// ---------------------------------------------------------------------------------------------- crash report
//
// A wasm trap (an allocation that fails, a stack overflow, an abort; a panic of a jobs worker, which has no
// overlay) bypasses the panic hook of the page. wasm-bindgen then marks the module terminated and every later
// callback (frame, input, timers, ResizeObserver) throws "Module terminated" (live report 2026-10-31: 600 of
// them, the first error lost). Here the FIRST error is kept (from the glue's hook, `__civ5OnTrap`, patched in
// by `cargo xtask`, SIMP-exe-690; from a worker's message; from the page's error events), with the size of the
// wasm memory, the JS heap, the turn and the game; it goes to the console once, over the canvas (the overlay of
// `show_crash`) and to the server (`POST /client-report`). The handlers that would call into the dead module
// are cut off (SIMP-exe-691) and the later errors are kept out of the console.

const crash = {
  first: null,
  extra: [],
  recent: [],
  terminated: false,
  done: false,
  quiet: 0,
  memory: null,
  workers: [],
};

/** A plain record of an error (or of anything thrown). */
function errorInfo(e, where) {
  const s = (v, n) => String(v ?? "").slice(0, n);
  return {
    where,
    name: s(e && e.name ? e.name : typeof e, 80),
    message: s(e && e.message !== undefined ? e.message : e, 1000),
    stack: s(e && e.stack, 3000),
    // Absolute UTC unix ms, as every log line (`ts`), so the crash lines up with the server's log by time.
    ts: Date.now(),
  };
}

/** Keeps `info` as the first error of the crash, or among the next few distinct ones. */
function noteError(info) {
  // A cause (a trap seen by the glue, a worker's message) comes before a mere sign of the dead module.
  if (!crash.first || (crash.first.sign && !info.sign)) {
    if (crash.first) crash.extra.unshift(crash.first);
    crash.first = info;
    return;
  }
  const same = (a) => a.message === info.message && a.where === info.where;
  if (crash.extra.length < 6 && !same(crash.first) && !crash.extra.some(same)) crash.extra.push(info);
}

/** The module is dead: the report is made a moment later (a worker's message may still be on its way). */
function moduleTerminated() {
  if (crash.terminated) return;
  crash.terminated = true;
  setTimeout(finishCrash, 400);
}

const isTerminated = (msg) => /Module terminated/.test(String(msg ?? ""));

// The glue (main thread) calls this when it marks the module terminated: `e` is the error. When another
// thread had marked it already (`already`), the page's trap is a consequence (wasm-bindgen stops a thread of
// a dead module at its next call into JS): the worker's own message, on its way, is the cause.
globalThis.__civ5OnTrap = (e, already) => {
  const info = errorInfo(e, already ? "page: stopped (the module was already terminated)" : "page: wasm trap");
  noteError(already ? { ...info, sign: true } : info);
  moduleTerminated();
};

window.addEventListener("error", (ev) => {
  const e = ev.error ?? ev.message;
  const msg = ev.error && ev.error.message !== undefined ? ev.error.message : ev.message;
  if (isTerminated(msg)) {
    // The module was dead before anything here saw why (a worker trapped, or a frame caught the error).
    if (!crash.first) noteError({ ...errorInfo(e, "page: an export refused (the module was already terminated)"), sign: true });
    moduleTerminated();
    if (crash.quiet++ > 0) ev.preventDefault();
    return;
  }
  if (crash.terminated) {
    ev.preventDefault();
    return;
  }
  crash.recent.push(errorInfo(e, "page: error event"));
  if (crash.recent.length > 3) crash.recent.shift();
});

window.addEventListener("unhandledrejection", (ev) => {
  const r = ev.reason;
  if (isTerminated(r && r.message)) {
    if (!crash.first) noteError({ ...errorInfo(r, "page: a promise of the dead module"), sign: true });
    moduleTerminated();
    if (crash.quiet++ > 0) ev.preventDefault();
    return;
  }
  if (crash.terminated) {
    ev.preventDefault();
    return;
  }
  crash.recent.push(errorInfo(r, "page: unhandled rejection"));
  if (crash.recent.length > 3) crash.recent.shift();
});

/** What the report says besides the errors. */
function crashContext() {
  const stats = window.__civ5stats ?? {};
  const num = (v) => (typeof v === "number" && Number.isFinite(v) ? v : null);
  let wasmMB = null;
  try {
    if (crash.memory) wasmMB = Math.round(crash.memory.buffer.byteLength / 1048576);
  } catch (_) {}
  const pm = performance.memory;
  return {
    wasmMB,
    wasmMBLastSeen: num(stats.ctxWasmMB),
    jsHeapMB: pm ? Math.round(pm.usedJSHeapSize / 1048576) : null,
    jsHeapLimitMB: pm ? Math.round(pm.jsHeapSizeLimit / 1048576) : null,
    turn: num(stats.ctxTurn),
    game: num(stats.ctxGame),
    threads: window.__civ5threads ? `${window.__civ5threads.mode} ${window.__civ5threads.workers}` : null,
    uptimeS: Math.round(performance.now() / 1000),
    hidden: document.hidden,
    build: new URL(import.meta.url).pathname,
    ua: navigator.userAgent,
    deviceMemoryGB: navigator.deviceMemory ?? null,
    cores: navigator.hardwareConcurrency ?? null,
    panic: typeof window.__civ5crash === "string" ? window.__civ5crash.slice(0, 1500) : null,
  };
}

/** The overlay of `show_crash` (log.rs, same id) when the module did not show it: plain DOM, no wasm. */
function crashOverlay(title, detail, reportText) {
  let panel = document.getElementById("civ5-crash");
  const el = (tag, style, text) => {
    const e = document.createElement(tag);
    e.setAttribute("style", style);
    e.textContent = text;
    return e;
  };
  if (!panel) {
    panel = el(
      "div",
      "position:fixed;inset:0;z-index:2147483647;display:flex;flex-direction:column;align-items:center;justify-content:center;gap:14px;padding:24px;box-sizing:border-box;background:rgba(5,8,15,0.94);color:#e6ebf2;font:15px/1.5 system-ui,sans-serif;text-align:center",
      "",
    );
    panel.id = "civ5-crash";
    panel.setAttribute("role", "alert");
    panel.append(
      el("div", "font-size:20px;font-weight:600", title),
      el(
        "pre",
        "max-width:min(900px,100%);max-height:40vh;overflow:auto;margin:0;padding:10px 12px;background:#0d1320;border:1px solid #26324a;border-radius:6px;color:#aab6c8;font:12px/1.4 ui-monospace,monospace;text-align:left;white-space:pre-wrap;word-break:break-word",
        detail,
      ),
      el(
        "div",
        "color:#8a96aa;font-size:13px",
        "A short report went to the server. Your game goes on on the server: reload to come back.",
      ),
    );
    const reload = el(
      "a",
      "padding:8px 22px;border-radius:6px;background:#2f6fd6;color:#fff;text-decoration:none;font-weight:600",
      "Reload",
    );
    reload.href = location.href;
    panel.append(reload);
    document.body.append(panel);
  }
  const copy = el(
    "button",
    "padding:6px 16px;border-radius:6px;border:1px solid #26324a;background:#0d1320;color:#e6ebf2;cursor:pointer",
    "Copy the crash report",
  );
  copy.onclick = () => {
    navigator.clipboard?.writeText(reportText).then(
      () => (copy.textContent = "Copied"),
      () => (copy.textContent = "Copy failed: see the console (F12)"),
    );
  };
  panel.append(copy);
  // The full report: the logs of this and the earlier page loads, with a note, to the server (`clientlog.js`).
  if (window.__civ5log?.openReport) {
    const full = el(
      "button",
      "padding:6px 16px;border-radius:6px;border:1px solid #2f6fd6;background:#2f6fd6;color:#fff;cursor:pointer",
      "Send a full report",
    );
    full.onclick = () => window.__civ5log.openReport();
    panel.append(full);
  }
}

/** SIMPLIFIED(SIMP-exe-691): cuts the dead module off the page: the canvases are replaced by copies without listeners (input, the
 *  ResizeObserver), the workers stop. The frame loop stops by itself (its callback throws before it asks for
 *  the next frame); what is left (timers, window events) throws quietly. */
function cutOff() {
  for (const c of document.querySelectorAll("canvas")) {
    try {
      c.replaceWith(c.cloneNode(false));
    } catch (_) {}
  }
  for (const w of crash.workers) {
    try {
      w.terminate();
    } catch (_) {}
  }
}

function finishCrash() {
  if (crash.done) return;
  crash.done = true;
  const report = {
    kind: "wasm-terminated",
    first: crash.first,
    extra: crash.extra,
    recent: crash.recent,
    ...crashContext(),
  };
  window.__civ5crashReport = report;
  const text = JSON.stringify(report);
  console.error("civ5 crash report:", text);
  // Into the log ring and to storage before the overlay (the full report and the next page load read it from there).
  try {
    window.__civ5log?.log(1, "crash.report", text.slice(0, 3500), { kind: report.kind });
    window.__civ5log?.flush(true);
  } catch (_) {}
  const f = crash.first ?? { where: "?", name: "?", message: "?" };
  const detail = [
    `${f.where}: ${f.name}: ${f.message}`,
    ...crash.extra.map((x) => `then ${x.where}: ${x.name}: ${x.message}`),
    report.panic ? `panic: ${report.panic}` : null,
    `wasm memory ${report.wasmMB ?? "?"} MB, JS heap ${report.jsHeapMB ?? "?"} MB, turn ${report.turn ?? "?"}, game ${report.game ?? "?"}`,
  ]
    .filter(Boolean)
    .join("\n");
  cutOff();
  crashOverlay("The game stopped because of an internal error.", detail, text);
  try {
    fetch(new URL("/client-report", location.href), {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: text.slice(0, 16000),
      keepalive: true,
    }).catch(() => {});
  } catch (_) {}
}

const status = document.getElementById("civ5-status");
const setStatus = (text) => {
  if (status) status.textContent = text;
};

const mtParam = new URLSearchParams(location.search).get("mt");

/** A module with a shared memory (`memory 1 1 shared`): the engine runs wasm threads. */
const SHARED_MEMORY_PROBE = new Uint8Array([0, 97, 115, 109, 1, 0, 0, 0, 5, 4, 1, 3, 1, 1]);

/** Why this client takes the single-threaded build, or `null` when it can run the threaded one. */
function singleReason() {
  if (mtParam === "0") return "?mt=0";
  if (self.crossOriginIsolated !== true) return "not crossOriginIsolated (no COOP/COEP)";
  if (typeof SharedArrayBuffer === "undefined") return "no SharedArrayBuffer";
  try {
    if (!WebAssembly.validate(SHARED_MEMORY_PROBE)) return "no wasm threads (shared memory)";
  } catch {
    return "no wasm threads (shared memory)";
  }
  if ((navigator.hardwareConcurrency || 1) < 2) return "one core";
  return null;
}

/** The threaded glue (the default), or `null` when the client cannot run it (`window.__civ5threads.reason`). */
async function threadedGlue() {
  const reason = singleReason();
  if (reason) {
    window.__civ5threads = { mode: "single", reason, workers: 0, joined: () => 0 };
    return null;
  }
  try {
    return await import("./pkg-mt/app_web.js");
  } catch (err) {
    const why = err && err.message ? err.message : String(err);
    window.__civ5threads = { mode: "single", reason: `no threaded build: ${why}`, workers: 0, joined: () => 0 };
    return null;
  }
}

function startWorkers(glue) {
  const wanted = mtParam && /^\d+$/.test(mtParam) ? Number(mtParam) : Math.min(8, (navigator.hardwareConcurrency || 2) - 1);
  const module = glue.jobs_module();
  const memory = glue.jobs_memory();
  for (let i = 0; i < wanted; i++) {
    const w = new Worker(new URL("./jobs-worker.js", import.meta.url), { type: "module", name: `jobs-${i}` });
    w.onerror = (e) => {
      console.error(`jobs worker ${i}:`, e.message ?? e);
      noteError(errorInfo(e.error ?? e.message, `jobs-${i}: error event`));
    };
    // The worker's first error (`jobs-worker.js`): its trap terminates the module for the page too.
    w.onmessage = ({ data }) => {
      if (!data || !data.civ5trap) return;
      const t = data.civ5trap;
      console.error(`jobs worker ${i} trapped: ${t.name}: ${t.message}\n${t.stack}`);
      const message = t.panic ? `${t.message} after ${t.panic}` : t.message;
      noteError({ ...errorInfo(null, t.where), name: t.name, message, stack: t.stack, sign: t.sign === true });
      moduleTerminated();
    };
    crash.workers.push(w);
    w.postMessage({ module, memory });
  }
  return wanted;
}

async function main() {
  const glue = await threadedGlue();
  const dir = glue ? "./pkg-mt/" : "./pkg/";
  const wasmUrl = new URL(`${dir}app_web_bg.wasm`, import.meta.url);
  const tFetch = performance.now();
  const response = await fetch(wasmUrl);
  if (!response.ok) {
    throw new Error(`failed to download ${wasmUrl}: HTTP ${response.status}`);
  }
  // Compile while the body streams in (compileStreaming), so the compile hides behind the download.
  // The exact size comes from a clone's body read, which runs concurrently with the compile.
  let module;
  let size = 0;
  const sizeTask = response.clone().arrayBuffer().then((b) => { size = b.byteLength; });
  try {
    module = await WebAssembly.compileStreaming(response);
  } catch (err) {
    console.warn("bootstrap: compileStreaming failed, compiling from bytes:", err);
    const again = await fetch(wasmUrl);
    module = await WebAssembly.compile(await again.arrayBuffer());
  }
  await sizeTask;
  // An old localStorage token becomes the session cookie before the app opens its socket (auth.js, `ready`).
  try {
    await window.__civ5auth?.ready;
  } catch (_) {
    // the page goes on without it: the login form shows
  }
  const downloadMs = performance.now() - tFetch;
  const t0 = performance.now();
  if (glue) {
    await glue.default({ module_or_path: module });
    crash.memory = glue.jobs_memory();
  } else {
    const exports = await init({ module_or_path: module });
    crash.memory = exports.memory ?? null;
  }
  const instantiateMs = performance.now() - t0;
  if (glue) {
    const n = startWorkers(glue);
    window.__civ5threads = { mode: "threads", reason: null, workers: n, joined: () => glue.jobs_workers() };
    console.info(`bootstrap: mode threads, ${n} workers`);
    glue.on_loaded(size, instantiateMs, downloadMs);
  } else {
    console.info(`bootstrap: mode single (${window.__civ5threads?.reason ?? "fallback"})`);
    on_loaded(size, instantiateMs, downloadMs);
  }
  setStatus("");
}

main().catch((err) => {
  console.error("bootstrap:", err);
  setStatus(`load error: ${err && err.message ? err.message : err}`);
});
