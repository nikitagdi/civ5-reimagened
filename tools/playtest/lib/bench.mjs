// Helpers of the `bench` scenario (B3 perf-bench): the in-page probe (rAF stamps + a 250 ms sampler of
// `window.__civ5stats`), the per-phase summary and the comparison with a baseline.
//
// The summary is generic on purpose: every numeric leaf of `__civ5stats` (top level, `passes.*`,
// `counters.*`, `timing.*`, and whatever the other B3 streams add: `terrain.*`, `gpu_mem.*`, `ram.*`,
// `net.*`, `assets.*`, `storage.*`) is summarised per phase as {p50, p95, max, last}, so a new counter shows
// up in the reports without a change here.

export const round = (x, d = 2) => (typeof x === "number" && Number.isFinite(x) ? Math.round(x * 10 ** d) / 10 ** d : x);

export function pct(sorted, p) {
  if (!sorted.length) return null;
  return sorted[Math.min(sorted.length - 1, Math.max(0, Math.ceil((p / 100) * sorted.length) - 1))];
}

/** Starts the probe in the page: `window.__bench = { ts: [rAF stamps], samples: [{t, v: {flat numeric stats}}] }`. */
export async function installProbe(page) {
  await page.evaluate(() => {
    if (window.__bench) return;
    const B = (window.__bench = { ts: [], samples: [] });
    const tick = (ts) => {
      B.ts.push(ts);
      requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
    const flat = (o, p, out) => {
      for (const [k, v] of Object.entries(o ?? {})) {
        if (typeof v === "number" && Number.isFinite(v)) out[`${p}${k}`] = v;
        else if (v && typeof v === "object" && !Array.isArray(v)) flat(v, `${p}${k}.`, out);
      }
      return out;
    };
    setInterval(() => {
      const v = flat(window.__civ5stats, "", {});
      const m = performance.memory;
      if (m) v["js.heap_used"] = m.usedJSHeapSize;
      // Whether the window had the focus and the page was visible (a covered or unfocused bench window may be throttled).
      v["doc.focus"] = document.hasFocus() ? 1 : 0;
      v["doc.visible"] = document.visibilityState === "visible" ? 1 : 0;
      B.samples.push({ t: performance.now(), v });
    }, 250);
  });
}

export const now = (page) => page.evaluate(() => performance.now());

const drained = new WeakMap();

/** Pulls the probe's data out of the page: only what is new since the last call crosses (the whole run would be
 * serialised on the page's main thread each time, which stalled it for 400+ ms late in a run: a frame the page itself
 * never saw); the cumulative data is kept here. */
export async function drain(page) {
  const have = drained.get(page) ?? { ts: [], samples: [] };
  const fresh = await page.evaluate(([a, b]) => ({ ts: window.__bench.ts.slice(a), samples: window.__bench.samples.slice(b) }), [have.ts.length, have.samples.length]);
  for (const t of fresh.ts) have.ts.push(t);
  for (const s of fresh.samples) have.samples.push(s);
  drained.set(page, have);
  return { ts: have.ts, samples: have.samples };
}

/** Frame time statistics of the rAF stamps inside [from, to]. FPS figures come from frame times: p50 FPS = 1000 / p50 ms,
 * 1% low = 1000 / p99 ms, 0.1% low = 1000 / p99.9 ms. */
export function frames(ts, from, to) {
  const d = [];
  for (let i = 1; i < ts.length; i++) if (ts[i] > from && ts[i] <= to) d.push(ts[i] - ts[i - 1]);
  if (d.length < 2) return { frames: d.length };
  const s = d.slice().sort((a, b) => a - b);
  const total = d.reduce((a, b) => a + b, 0);
  const worst = s.slice(Math.floor(s.length * 0.99));
  return {
    frames: d.length,
    seconds: round(total / 1000),
    fpsMean: round((d.length * 1000) / total, 1),
    fpsP50: round(1000 / pct(s, 50), 1),
    fps1pLow: round(1000 / pct(s, 99), 1),
    fps1pLowAvg: round(1000 / (worst.reduce((a, b) => a + b, 0) / worst.length), 1),
    fps01pLow: round(1000 / pct(s, 99.9), 1),
    frameMsP50: round(pct(s, 50)),
    frameMsP95: round(pct(s, 95)),
    frameMsP99: round(pct(s, 99)),
    frameMsMax: round(s[s.length - 1]),
    over6_9ms: d.filter((x) => x > 6.95).length,
    over16_7ms: d.filter((x) => x > 16.7).length,
    over33ms: d.filter((x) => x > 33.4).length,
  };
}

/** {name: {p50, p95, max, last, n}} of every stats field sampled inside [from, to]. */
export function statsOf(samples, from, to) {
  const cols = new Map();
  let last = null;
  for (const s of samples) {
    if (s.t <= from || s.t > to) continue;
    last = s;
    for (const [k, v] of Object.entries(s.v)) {
      if (!cols.has(k)) cols.set(k, []);
      cols.get(k).push(v);
    }
  }
  const out = {};
  for (const [k, xs] of cols) {
    const sorted = xs.slice().sort((a, b) => a - b);
    out[k] = { p50: round(pct(sorted, 50), 3), p95: round(pct(sorted, 95), 3), max: round(sorted[sorted.length - 1], 3), last: round(last.v[k], 3), n: xs.length };
  }
  // The pass table summed: the GPU time of the frame when the passes are GPU timings, else the CPU encode time.
  const sums = samples.filter((s) => s.t > from && s.t <= to).map((s) => Object.entries(s.v).filter(([k]) => k.startsWith("passes.")).reduce((a, [, v]) => a + v, 0)).filter((x) => x > 0);
  if (sums.length) {
    const sorted = sums.sort((a, b) => a - b);
    out["passSumMs"] = { p50: round(pct(sorted, 50), 3), p95: round(pct(sorted, 95), 3), max: round(sorted[sorted.length - 1], 3), last: round(sorted[sorted.length - 1], 3), n: sums.length };
  }
  return out;
}

/** Share of the samples in [from, to] where the window had the focus (`document.hasFocus()`) and the page was visible
 * (`document.visibilityState`); null without samples. */
export function docState(samples, from, to) {
  const xs = samples.filter((s) => s.t > from && s.t <= to && s.v["doc.focus"] !== undefined);
  if (!xs.length) return null;
  const share = (k) => round(xs.filter((s) => s.v[k] === 1).length / xs.length, 2);
  return { focused: share("doc.focus"), visible: share("doc.visible"), samples: xs.length };
}

/** One phase record: the frame statistics, the stats summary and the focus/visibility of [from, to]. */
export function phaseRecord(data, from, to) {
  return { ...frames(data.ts, from, to), stats: statsOf(data.samples, from, to), doc: docState(data.samples, from, to) };
}

// ---------------------------------------------------------------- comparison

/** Metrics that decide pass/fail of a comparison: [path in a phase record, "higher"|"lower" is better, tolerance]. */
export const KEY_METRICS = [
  ["fpsP50", "higher", 0.05],
  ["fps1pLow", "higher", 0.1],
  ["frameMsP50", "lower", 0.05],
  ["frameMsP99", "lower", 0.1],
  ["stats.draws.p50", "lower", 0.05],
  ["stats.triangles.p50", "lower", 0.05],
  ["stats.passSumMs.p50", "lower", 0.1],
];

/** Looks up "stats.passes.Terrain.p50" style paths: the stats keys contain dots themselves. */
export function metric(phase, path) {
  if (path.startsWith("stats.")) {
    const rest = path.slice(6);
    const cut = rest.lastIndexOf(".");
    return phase.stats?.[rest.slice(0, cut)]?.[rest.slice(cut + 1)];
  }
  return phase[path];
}

/** Compares two bench results ({phases: {name: record}}); returns {rows, regressions}. */
export function compare(base, cur) {
  const rows = [];
  const regressions = [];
  for (const [mode, phases] of Object.entries(cur.modes ?? {})) {
    for (const [name, rec] of Object.entries(phases)) {
      const b = base.modes?.[mode]?.[name];
      if (!b) continue;
      for (const [path, better, tol] of KEY_METRICS) {
        const x = metric(b, path);
        const y = metric(rec, path);
        if (typeof x !== "number" || typeof y !== "number") continue;
        const delta = x === 0 ? 0 : (y - x) / x;
        const worse = better === "higher" ? delta < -tol : delta > tol;
        const better_ = better === "higher" ? delta > tol : delta < -tol;
        const row = { mode, phase: name, metric: path, base: x, cur: y, delta: round(delta * 100, 1), verdict: worse ? "WORSE" : better_ ? "better" : "same" };
        rows.push(row);
        if (worse) regressions.push(row);
      }
    }
  }
  return { rows, regressions };
}

export function compareMd(base, cur, c) {
  const out = [`# Bench comparison`, "", `Baseline: ${base.meta?.label ?? "?"} (${base.meta?.commit ?? "?"}); current: ${cur.meta?.label ?? "?"} (${cur.meta?.commit ?? "?"}).`, ""];
  out.push("| mode | phase | metric | baseline | current | Δ% | |", "|---|---|---|---:|---:|---:|---|");
  for (const r of c.rows) out.push(`| ${r.mode} | ${r.phase} | ${r.metric} | ${r.base} | ${r.cur} | ${r.delta} | ${r.verdict} |`);
  out.push("", c.regressions.length ? `**${c.regressions.length} regression(s)** beyond tolerance.` : "No regression beyond tolerance.");
  return out.join("\n");
}

// ---------------------------------------------------------------- report

const f = (x) => (x === undefined || x === null ? "-" : x);

/** Markdown of a bench result: per mode the phase table, the pass table (p50 ms), the counters, the end turns. */
export function reportMd(b) {
  const m = b.meta ?? {};
  const out = [`# Bench ${m.label ?? ""}`.trim(), ""];
  out.push(`- commit \`${f(m.commit)}\` on \`${f(m.branch)}\`${m.dirty ? " (dirty tree)" : ""}; ${m.startedAt}`);
  out.push(`- scene: ${m.scene}; preset ${m.preset}, gfx ${m.gfx}, window ${m.size}, ${m.headed ? "headed" : "headless"}, wasm profile ${f(m.profile)}`);
  out.push(`- machine: ${m.host?.cpu}, ${m.host?.cores} threads, ${m.host?.ramGb} GB, ${m.host?.os}`);
  for (const [mode, sc] of Object.entries(b.scene ?? {})) {
    const g = sc.gpu?.webgpu;
    out.push(`- ${mode}: GPU ${g ? `${g.vendor} ${g.architecture} ${g.device ?? ""}` : "?"}, Chrome ${f(sc.chrome)}, units ${f(sc.units)}, cities ${f(sc.cities)}, players ${f(sc.players)}, time to play ${f(sc.timeToPlayMs)} ms, settle ${f(sc.settleMs)} ms`);
  }
  out.push("");
  for (const [mode, phases] of Object.entries(b.modes ?? {})) {
    out.push(`## ${mode}`, "");
    out.push("| phase | s | FPS p50 | FPS p50 range over runs | host CPU % | 1% low | 0.1% low | frame ms p50 | p99 | max | >6.9 ms | >16.7 ms | draws p50 | triangles p50 | passes Σ p50 ms |", "|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|");
    for (const [name, r] of Object.entries(phases)) {
      out.push(`| ${name} | ${f(r.seconds)} | ${f(r.fpsP50)} | ${r.spread?.fpsP50 ? r.spread.fpsP50.join("…") : "-"} | ${f(r.cpuPct?.hostBusy)} | ${f(r.fps1pLow)} | ${f(r.fps01pLow)} | ${f(r.frameMsP50)} | ${f(r.frameMsP99)} | ${f(r.frameMsMax)} | ${f(r.over6_9ms)}/${f(r.frames)} | ${f(r.over16_7ms)} | ${f(r.stats?.draws?.p50)} | ${f(r.stats?.triangles?.p50)} | ${f(r.stats?.passSumMs?.p50)} |`);
    }
    out.push("");
    const names = Object.keys(phases);
    const cpuKeys = [...new Set(names.flatMap((n) => Object.keys(phases[n].cpuPct ?? {})))];
    if (cpuKeys.length) {
      out.push("CPU, % of one core during the phase (`mainBusy`/`mainScript`: the page's main thread task / script time; `proc.*`: all processes of that type, CDP SystemInfo):", "", `| | ${names.join(" | ")} |`, `|---|${names.map(() => "---:").join("|")}|`);
      for (const k of cpuKeys) out.push(`| ${k} | ${names.map((n) => f(phases[n].cpuPct?.[k])).join(" | ")} |`);
      out.push("");
    }
    const passes = [...new Set(names.flatMap((n) => Object.keys(phases[n].stats ?? {}).filter((k) => k.startsWith("passes."))))];
    passes.sort((a, c) => (phases["pan-far"]?.stats?.[c]?.p50 ?? 0) - (phases["pan-far"]?.stats?.[a]?.p50 ?? 0));
    out.push("Pass time p50, ms (what `?stats=1` calls `passes`):", "", `| pass | ${names.join(" | ")} |`, `|---|${names.map(() => "---:").join("|")}|`);
    for (const p of passes) out.push(`| ${p.slice(7)} | ${names.map((n) => f(phases[n].stats?.[p]?.p50)).join(" | ")} |`);
    out.push("");
    const keys = [...new Set(names.flatMap((n) => Object.keys(phases[n].stats ?? {}).filter((k) => /^(counters\.(terrain|gpu_mem|ram|net|assets|storage|anim|overlay)|terrain\.|gpu_mem\.|ram\.|net\.|assets\.|storage\.|js\.)/.test(k))))];
    if (keys.length) {
      out.push("Counters p50 (max in the last column of the heaviest phase):", "", `| counter | ${names.join(" | ")} |`, `|---|${names.map(() => "---:").join("|")}|`);
      for (const k of keys) out.push(`| ${k} | ${names.map((n) => f(phases[n].stats?.[k]?.p50)).join(" | ")} |`);
      out.push("");
    }
    const t = b.turns?.[mode];
    if (t?.length) {
      out.push("End turns:", "", "| turn | wall ms | worst frame ms | p99 frame ms | frames > 33 ms in 2 s after |", "|---:|---:|---:|---:|---:|");
      for (const x of t) out.push(`| ${x.turn} | ${x.wallMs} | ${x.frameMsMax} | ${x.frameMsP99} | ${x.over33ms} |`);
      out.push("");
    }
  }
  return out.join("\n");
}
