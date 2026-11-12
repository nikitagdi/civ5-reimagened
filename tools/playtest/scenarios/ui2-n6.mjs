// `ui2-n6`: where UI v2 skin scripts run, measured. The page `ui2/bench/bench.html` runs one skin workload (the HUD of
// mockup A in draft form over a generated late game on a Huge map: 200 city banners, 150 notifications, the production
// window with 40 options and the skin's own sort) on two hosts with the same contract: the production Web Worker
// (`ui2/host.js`) and the page's main thread (`ui2/bench/main-arm.js`). Per arm:
// - idle: 10 s without changes: no message, no script call;
// - turns: 20 turn deltas 2 s apart, each followed by three timer ticks: main-thread time of the frames that post or
//   drain (the worker arm's overhead; the main arm's scripts included), the worker's handler time (`busy_ms`), and
//   the frames between a delta and the answer that acknowledges it (N3: at most one);
// - feed: 10 filter events a second for 10 s;
// - heap after the turns (`measureUserAgentSpecificMemory`, by realm: needs the cross-origin isolated page, `--threads`);
// - GC pauses: the turns again, fast, under a trace (`v8`, `devtools.timeline`), peak pause per thread;
// - the Rust supervisor (`Ui2ScriptSelftest`) over the same host: a 200-row delta probe with its main-thread counter
//   `ui2.js.main_ms`, then a handler that loops for 5 s (detection, kill, restart, the longest page frame).
// The result carries the filled table and the decision of the rule; `pass` means the measurement ran cleanly, not
// that the thresholds are met.
import { statSync } from "node:fs";
import path from "node:path";
import { openPage } from "../lib/browser.mjs";

const GC_EVENTS = new Set([
  "MinorGC",
  "MajorGC",
  "V8.GCScavenger",
  "V8.GCFinalizeMC",
  "V8.GCFinalizeMCReduceMemory",
  "V8.GCIncrementalMarking",
  "V8.GCIncrementalMarkingStart",
  "V8.GCIncrementalMarkingFinalize",
  "V8.GCCompactor",
  "V8.GCMinorMS",
  "V8.GC_SCAVENGER",
  "V8.GC_MARK_COMPACTOR",
  "V8.GC_MINOR_MARK_SWEEPER",
]);

/** Peak and total GC pause per thread of the trace (main thread of the page, dedicated workers), ms. */
export function gcPauses(buffer) {
  const trace = JSON.parse(buffer.toString("utf8"));
  const events = Array.isArray(trace) ? trace : trace.traceEvents;
  const names = new Map();
  for (const e of events) if (e.ph === "M" && e.name === "thread_name") names.set(`${e.pid}:${e.tid}`, e.args?.name ?? "");
  const by = {};
  for (const e of events) {
    if (e.ph !== "X" || !GC_EVENTS.has(e.name) || typeof e.dur !== "number") continue;
    const t = names.get(`${e.pid}:${e.tid}`) ?? "";
    const kind = t === "CrRendererMain" ? "main" : /DedicatedWorker/.test(t) ? "worker" : null;
    if (!kind) continue;
    const ms = e.dur / 1000;
    const b = (by[kind] ??= { peakMs: 0, peakEvent: null, events: 0, totalMs: 0 });
    b.events += 1;
    if (e.name === "MinorGC" || e.name === "MajorGC") b.totalMs += ms;
    if (ms > b.peakMs) {
      b.peakMs = ms;
      b.peakEvent = e.name;
    }
  }
  for (const b of Object.values(by)) {
    b.peakMs = Math.round(b.peakMs * 1000) / 1000;
    b.totalMs = Math.round(b.totalMs * 1000) / 1000;
  }
  return by;
}

const MB = (b) => (b === null || b === undefined ? null : Math.round((b / 1048576) * 100) / 100);
const fmt = (v, unit = "") => (v === null || v === undefined ? "n/a" : `${v}${unit}`);

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "ui2-n6", pass: false, logs: o.logDir, checks: [], numbers: {} };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const b = await openPage({
    headed: o.headed,
    logDir: o.logDir,
    // Memory by realm right away (no wait for the next GC), and exact `performance.memory` as the fallback.
    // The renderer must be locked to the site for memory by realm.
    chromeArgs: ["--enable-blink-features=ForceEagerMeasureMemory", "--enable-precise-memory-info", "--site-per-process"],
  });
  const { page, browser } = b;
  const n = result.numbers;
  try {
    await page.goto(`${ctx.server.url}/ui2/bench/bench.html`);
    await page.waitForFunction(() => !!window.__n6, null, { timeout: 30000 });
    n.info = await page.evaluate(() => window.__n6.info());
    n.run = {
      headed: !!o.headed,
      profile: o.profile ?? "debug",
      chrome: browser.version(),
      angle: b.args.find((a) => a.startsWith("--use-angle")) ?? (o.headed ? "default (headed)" : null),
      cpuPin: b.cpuPin,
    };
    // Not a failure of the measurement: without memory by realm the worker's heap is not attributed (the table says so).
    n.memoryByRealm = { isolated: n.info.isolated, measure: n.info.measure, whyNot: n.info.whyNot };

    // ---- the heap of a worker with no skin
    n.emptyWorker = await page.evaluate(() => window.__n6.emptyWorker());

    // ---- the workload on each arm
    const cdp = await page.context().newCDPSession(page);
    const mainHeap = async () => {
      await cdp.send("HeapProfiler.collectGarbage");
      return (await cdp.send("Runtime.getHeapUsage")).usedSize;
    };
    n.workload = {};
    for (const arm of ["worker", "main"]) {
      const before = await mainHeap();
      const w = await page.evaluate((a) => window.__n6.workload(a), arm);
      w.cdpMainHeapBefore = before;
      w.cdpMainHeapAfter = await mainHeap();
      n.workload[arm] = w;
      check(
        `${arm}: the workload ran without errors and the mirror kept in step`,
        !w.turns.errors.length && !w.feed.errors.length && w.turns.logErrors === 0 && w.feed.logErrors === 0 && w.turns.unanswered === 0,
        JSON.stringify({ errors: [...w.turns.errors, ...w.feed.errors], logErrors: w.turns.logErrors + w.feed.logErrors, first: w.turns.firstLogError ?? w.feed.firstLogError, unanswered: w.turns.unanswered }),
      );
      check(`${arm}: idle sends nothing and calls no script`, w.idle.posts === 0 && w.idle.outs === 0, JSON.stringify(w.idle));
      check(`${arm}: every turn and tick was answered`, w.turns.n3.n === 80, `${w.turns.n3.n} answered updates`);
    }

    // ---- GC pauses: the turns again under a trace
    n.gc = {};
    for (const arm of ["worker", "main"]) {
      await browser.startTracing(page, { categories: ["devtools.timeline", "v8", "disabled-by-default-v8.gc"] });
      const pass = await page.evaluate((a) => window.__n6.gcPass(a), arm);
      const buf = await browser.stopTracing();
      n.gc[arm] = { pauses: gcPauses(buf), turnsWork: pass.work, workerBusy: pass.workerBusy };
    }

    // ---- the Rust supervisor over each host: probe and hang
    n.rust = {};
    for (const arm of ["worker", "main"]) {
      n.rust[arm] = await page.evaluate((a) => window.__n6.rust(a), arm);
    }
    const hw = n.rust.worker.hang;
    check("worker: the hang is detected, the worker killed and running again", hw.detectMs !== null && hw.killMs !== null && hw.runningAgainMs !== null && hw.answersAfter, JSON.stringify(hw));
    check("main: the context answers after the hang", n.rust.main.hang.answersAfter, JSON.stringify(n.rust.main.hang));

    // ---- sizes
    const ui2 = path.join(o.webRoot ?? ctx.webRoot ?? "target/web", "ui2");
    const size = (f) => {
      try {
        return statSync(path.join(ui2, f)).size;
      } catch {
        return null;
      }
    };
    n.sizes = { skinBytes: n.info.skinBytes, apiJs: size("api.js"), workerJs: size("worker.js"), hostJs: size("host.js") };

    n.table = table(n);
    n.decision = decide(n.table);
    // The module's start finds no game canvas on this page and says so: not an error of the bench.
    const errors = b.errors.filter((e) => !/page has no <canvas/.test(e));
    result.pass = result.checks.every((c) => c.pass) && errors.length === 0;
    if (errors.length) result.error = `console errors: ${errors.slice(0, 3).join(" | ")}`;
  } catch (e) {
    result.error = e.stack ?? String(e);
  } finally {
    result.durationMs = Date.now() - started;
    await b.close();
  }
  if (n.table) {
    console.log("ui2-n6 result table:");
    for (const r of n.table) console.log(`  ${r.metric}: main ${r.main} | worker ${r.worker} | limit ${r.limit} | ${r.verdict}`);
    console.log(`ui2-n6 decision: ${n.decision?.text}`);
  }
  return result;
}

/** The rows of the result table: values as text, and pass flags per arm. */
function table(n) {
  const w = n.workload.worker;
  const m = n.workload.main;
  const worst = (a, b) => (a === null ? b : b === null ? a : Math.max(a, b));
  const rows = [];
  const row = (metric, main, worker, limit, okMain, okWorker, note) => {
    const verdict = `main ${okMain === null ? "n/a" : okMain ? "pass" : "FAIL"}, worker ${okWorker === null ? "n/a" : okWorker ? "pass" : "FAIL"}`;
    rows.push({ metric, main, worker, limit, okMain, okWorker, verdict, note });
  };
  // JS in a frame with changes, judged on the turn frames (the heavy ones; the timer ticks in between would dilute
  // the p95) and the feed events: the main arm's page-thread frame (scripts inside), the worker arm's handler time
  // per batch on the worker. The p95 over every frame with work is shown beside it.
  const mainJs = worst(m.turns.turnWork.p95, m.feed.work.p95);
  const workerJs = worst(w.turns.turnBusy.p95, w.feed.workerBusy.p95);
  row(
    "JS in a frame with changes, p95, ms",
    `${fmt(mainJs)} (all frames with work ${fmt(m.turns.work.p95)}; turn max ${fmt(m.turns.turnWork.max)})`,
    `${fmt(workerJs)} on the worker (all batches ${fmt(w.turns.workerBusy.p95)}; turn max ${fmt(w.turns.turnBusy.max)})`,
    "<= 1.5",
    mainJs !== null && mainJs <= 1.5,
    workerJs !== null && workerJs <= 1.5,
    "turn frames (20) and feed events (100); main: page thread per frame; worker: handler time per batch",
  );
  const over = worst(w.turns.turnWork.p95, w.feed.work.p95);
  row(
    "main-thread overhead, p95, ms",
    "n/a (the scripts run there)",
    `${fmt(over)} (all frames with work ${fmt(w.turns.work.p95)}; turn max ${fmt(w.turns.turnWork.max)})`,
    "<= 1.0",
    null,
    over !== null && over <= 1.0,
    "worker arm, JS only: drain + parse of the answers + post of the frame text; the Rust side is the next row",
  );
  const rustOver = n.rust.worker.probe.mainMs.p95;
  row("main-thread overhead through the Rust supervisor (200-row probe), p95, ms", fmt(n.rust.main.probe.mainMs.p95), fmt(rustOver), "<= 1.0 (worker)", null, rustOver !== null && rustOver <= 1.0, "ui2.js.main_ms: drain + post of the Rust supervisor; main includes the scripts");
  const skinMain = m.heap.windowGrowth;
  const skinWorker = (w.heap.workers ?? []).length ? Math.max(...w.heap.workers.map((x) => x.bytes)) : null;
  const empty = n.emptyWorker?.bytes ?? null;
  row(
    "heap, MB (empty worker / skin)",
    `- / ${fmt(MB(skinMain))} (growth of the page heap; page ${fmt(MB(m.heap.window))})`,
    skinWorker === null ? `not attributed here (${n.memoryByRealm?.whyNot ?? "no memory by realm"})` : `${fmt(MB(empty))} / ${fmt(MB(skinWorker))}`,
    "<= 48",
    skinMain === null ? null : MB(m.heap.window) <= 48,
    skinWorker === null ? null : MB(skinWorker) <= 48,
    `${m.heap.how}; the same api.js mirror holds the same model in either realm`,
  );
  const gcMain = n.gc.main.pauses.main?.peakMs ?? null;
  const gcWorker = n.gc.worker.pauses.worker?.peakMs ?? null;
  const gcWorkerPage = n.gc.worker.pauses.main?.peakMs ?? null;
  row("GC peak pause, ms", fmt(gcMain), `${fmt(gcWorker)} (worker thread; page thread ${fmt(gcWorkerPage)})`, "<= 2", gcMain === null ? null : gcMain <= 2, gcWorker === null ? null : gcWorker <= 2, "trace of 20 fast turns; the page thread also carries the bench's own host loop");
  const kb = Math.round((n.sizes.skinBytes / 1024) * 100) / 100;
  row("skin JS, KB", fmt(kb), fmt(kb), "<= 750", kb <= 750, kb <= 750, `the bench skin; api.js ${n.sizes.apiJs} B + worker.js ${n.sizes.workerJs} B + host.js ${n.sizes.hostJs} B are the host's`);
  const share = (x) => (x.n ? Math.round((x.withinOne / x.n) * 10000) / 100 : null);
  const n3m = share(m.turns.n3);
  const n3w = Math.min(share(w.turns.n3) ?? 100, share(w.feed.n3) ?? 100);
  row("N3: updates answered within 1 frame, %", fmt(n3m), fmt(n3w), ">= 99", n3m === null ? null : n3m >= 99, n3w >= 99, "frames between the delta's post and the drain of its acknowledging answer");
  const hm = n.rust.main.hang;
  const hw = n.rust.worker.hang;
  const hangOk = (h) => h.detectMs !== null && h.detectMs <= 400 && h.runningAgainMs !== null && h.runningAgainMs <= 1500 && h.longestFrameMs !== null && h.longestFrameMs <= 33;
  row(
    "hang: detection / restart / longest page frame, ms",
    `${fmt(hm.detectMs)} / ${fmt(hm.runningAgainMs)} / ${fmt(hm.longestFrameMs)}`,
    `${fmt(hw.detectMs)} / ${fmt(hw.runningAgainMs)} / ${fmt(hw.longestFrameMs)}`,
    "250 / <= 1500 / <= 33",
    hangOk(hm),
    hangOk(hw),
    "a handler that loops for 5 s; the Rust watchdog marks busy after 250 ms without an answer and kills at 1000 ms",
  );
  return rows;
}

/** The decision rule of the spec. */
function decide(rows) {
  const by = Object.fromEntries(rows.map((r) => [r.metric.split(",")[0], r]));
  const workerFails = rows.filter((r) => r.okWorker === false).map((r) => r.metric);
  const n6 = ["JS in a frame with changes", "heap", "GC peak pause", "skin JS"];
  const mainPassesN6 = n6.every((k) => by[k]?.okMain !== false);
  if (!workerFails.length) return { rule: 1, text: "worker: every threshold is met (the preliminary choice stands)", workerFails };
  const onlyOverheadOrN3 = workerFails.every((m) => /overhead|N3/.test(m));
  if (onlyOverheadOrN3 && mainPassesN6) {
    return { rule: 2, text: `main thread passes N6 and the worker misses only ${workerFails.join("; ")}: a question to the operator (the main thread cannot interrupt a hung script)`, workerFails };
  }
  return { rule: 3, text: `the worker misses ${workerFails.join("; ")}${mainPassesN6 ? "" : " and the main thread misses N6 too"}: cut the skin budgets and measure again`, workerFails };
}
