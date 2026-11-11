// The worker arm of the script host measurement (bench.html, playtest scenario `ui2-n6`).
//
// The production path: `ui2/host.js` (one Web Worker per context running `ui2/worker.js`, which loads `ui2/api.js`
// and takes the forbidden globals away). The bench loop or the Rust supervisor posts the frame text and drains the
// answers in a later frame; the skin's handlers run on the worker's thread. This module adds what only this arm has:
// counting what crosses the thread boundary, and the heap of the workers.

/** The host of `ui2/host.js`, taken when the page loaded it (bench.html swaps `window.__civ5ui2` between arms). */
let production = null;

/** Remember the production host; call once, right after `ui2/host.js` ran. */
export function takeProductionHost() {
  if (!production) production = window.__civ5ui2 || null;
  if (!production) throw new Error("ui2/host.js is not loaded");
  return production;
}

/**
 * The production host wrapped with counters: `stats()` -> {posts, postBytes, drained, drainedBytes, postMs}
 * (`postMs`: time spent in `postMessage`, the structured clone of the text included).
 */
export function createWorkerHost() {
  const h = takeProductionHost();
  let posts = 0;
  let postBytes = 0;
  let drained = 0;
  let drainedBytes = 0;
  let postMs = 0;
  return {
    spawn: () => h.spawn(),
    post(id, text) {
      const t0 = performance.now();
      const ok = h.post(id, text);
      postMs += performance.now() - t0;
      posts += 1;
      postBytes += text.length;
      return ok;
    },
    drain() {
      const q = h.drain();
      drained += q.length;
      for (const m of q) drainedBytes += m[2].length;
      return q;
    },
    terminate: (id) => h.terminate(id),
    stats() {
      return { posts, postBytes, drained, drainedBytes, postMs };
    },
  };
}

let measurable = null;
let unavailable = null;

/**
 * Whether the page can attribute memory to its workers: the page must be cross-origin isolated and its renderer locked
 * to the site, or Chrome refuses with a SecurityError. Tried once.
 */
export async function canMeasureWorkers() {
  if (measurable !== null) return measurable;
  measurable = false;
  if (typeof performance.measureUserAgentSpecificMemory !== "function" || globalThis.crossOriginIsolated !== true) {
    unavailable = "no measureUserAgentSpecificMemory or not cross-origin isolated";
    return false;
  }
  try {
    await performance.measureUserAgentSpecificMemory();
    measurable = true;
  } catch (e) {
    unavailable = String(e && e.message ? e.message : e);
  }
  return measurable;
}

/** Why the page cannot attribute memory to its workers, or null. */
export function whyNotMeasurable() {
  return unavailable;
}

/**
 * The page's memory by realm: {total, window, workers: [{url, bytes}], other} in bytes, from
 * `performance.measureUserAgentSpecificMemory()` (which collects garbage first). Null where the page cannot measure.
 */
export async function measureRealms() {
  if (!(await canMeasureWorkers())) return null;
  const r = await performance.measureUserAgentSpecificMemory();
  const out = { total: r.bytes, window: 0, workers: [], other: 0 };
  for (const b of r.breakdown) {
    const at = b.attribution || [];
    const w = at.find((x) => x.scope === "DedicatedWorkerGlobalScope");
    if (w) out.workers.push({ url: w.url, bytes: b.bytes });
    else if (at.some((x) => x.scope === "Window")) out.window += b.bytes;
    else out.other += b.bytes;
  }
  return out;
}

/**
 * The heap of a worker that loaded `ui2/worker.js` and `ui2/api.js` and runs no skin (one context with no files):
 * {bytes, workers} or null where the page cannot measure. Call it while no other worker runs; the worker is terminated
 * afterwards.
 */
export async function emptyWorkerBytes(init) {
  if (!(await canMeasureWorkers())) return null;
  const h = takeProductionHost();
  const id = h.spawn();
  h.post(id, JSON.stringify(Object.assign({}, init, { files: [] })));
  const t0 = performance.now();
  // Wait for `ready`, then let the worker settle.
  for (;;) {
    const q = h.drain();
    if (q.some((m) => m[0] === id && m[2].includes('"ready"'))) break;
    if (performance.now() - t0 > 10000) break;
    await new Promise((r) => setTimeout(r, 20));
  }
  await new Promise((r) => setTimeout(r, 200));
  const after = await measureRealms();
  h.terminate(id);
  const bytes = after.workers.length ? Math.max(...after.workers.map((w) => w.bytes)) : null;
  return { bytes, workers: after.workers };
}
