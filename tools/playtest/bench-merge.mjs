// Merges several bench runs of the same setup into one record (the median of every number) for a steadier baseline:
//   node tools/playtest/bench-merge.mjs <out.json> <run1.json> <run2.json> ...
// Per phase the spread of the key metrics over the runs is kept in `spread` ({metric: [min, max]}); `meta.runs` is the count.
import fs from "node:fs";
import { KEY_METRICS, metric, round } from "./lib/bench.mjs";

const [out, ...files] = process.argv.slice(2);
if (!out || files.length < 1) {
  console.error("usage: node bench-merge.mjs <out.json> <run1.json> [run2.json ...]");
  process.exit(2);
}
const runs = files.map((f) => JSON.parse(fs.readFileSync(f, "utf8")));
// A launch on V8's baseline code (`scene.<mode>.slowTier`) is left out of the median of its mode, unless every run of
// the mode is one; `meta.slowTierExcluded` lists them as "file:mode".
const slowTierExcluded = [];
for (const mode of new Set(runs.flatMap((r) => Object.keys(r.modes ?? {})))) {
  const slow = runs.filter((r) => r.scene?.[mode]?.slowTier === true);
  if (slow.length === 0 || slow.length === runs.filter((r) => r.modes?.[mode]).length) continue;
  for (const r of slow) {
    delete r.modes[mode];
    if (r.turns) delete r.turns[mode];
    slowTierExcluded.push(`${files[runs.indexOf(r)]}:${mode}`);
  }
}
const med = (xs) => {
  const s = xs.slice().sort((a, b) => a - b);
  const m = Math.floor(s.length / 2);
  return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2;
};
function merge(vals) {
  const first = vals.find((v) => v !== undefined);
  if (typeof first === "number") return round(med(vals.filter((v) => typeof v === "number")), 3);
  if (Array.isArray(first)) return first.map((_, i) => merge(vals.map((v) => v?.[i])));
  if (first && typeof first === "object") {
    const o = {};
    for (const k of new Set(vals.flatMap((v) => Object.keys(v ?? {})))) o[k] = merge(vals.map((v) => v?.[k]));
    return o;
  }
  return first;
}
const merged = merge(runs);
merged.meta = { ...runs[0].meta, runs: runs.length, commits: [...new Set(runs.map((r) => r.meta?.commit))], slowTierExcluded };
for (const [mode, phases] of Object.entries(merged.modes ?? {})) {
  for (const [name, rec] of Object.entries(phases)) {
    rec.spread = {};
    for (const [path] of KEY_METRICS) {
      const xs = runs.map((r) => metric(r.modes?.[mode]?.[name] ?? {}, path)).filter((x) => typeof x === "number");
      if (xs.length) rec.spread[path] = [Math.min(...xs), Math.max(...xs)];
    }
    rec.hostBusyPerRun = runs.map((r) => r.modes?.[mode]?.[name]?.cpuPct?.hostBusy ?? null);
  }
}
fs.writeFileSync(out, JSON.stringify(merged, null, 2));
console.log(`merged ${runs.length} runs into ${out}`);
