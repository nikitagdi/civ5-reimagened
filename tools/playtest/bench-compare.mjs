// Compares a bench result with a baseline (B3 perf-bench, AC-B2):
//   node tools/playtest/bench-compare.mjs <current bench.json> [baseline bench.json] [--md <out.md>] [--all]
// The baseline defaults to tools/playtest/baselines/b3-baseline.json. Prints the key metrics per mode and phase
// (FPS p50, 1% low, frame ms, draws, triangles, pass sum) and exits 1 when one is worse than its tolerance
// (5 to 10 %); `--all` also lists the unchanged rows. Counters (terrain.*, gpu_mem.*, ram.*, net.*) are in the
// phase records of the json for a closer look: `--counters` prints their p50 side by side.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { compare, compareMd, metric } from "./lib/bench.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const flag = (f) => args.includes(f);
const val = (f) => {
  const i = args.indexOf(f);
  return i >= 0 ? args[i + 1] : null;
};
const files = args.filter((a, i) => !a.startsWith("--") && args[i - 1] !== "--md");
if (!files[0]) {
  console.error("usage: node bench-compare.mjs <current bench.json> [baseline bench.json] [--md out.md] [--all] [--counters]");
  process.exit(2);
}
const read = (f) => JSON.parse(fs.readFileSync(f, "utf8"));
const cur = read(files[0]);
const basePath = files[1] ?? path.join(here, "baselines", "b3-baseline.json");
const base = read(basePath);
const c = compare(base, cur);
const rows = flag("--all") ? c.rows : c.rows.filter((r) => r.verdict !== "same");
console.log(`baseline ${base.meta?.label ?? basePath} (${base.meta?.commit}) vs current ${cur.meta?.label ?? files[0]} (${cur.meta?.commit})`);
for (const r of rows) console.log(`  ${r.verdict.padEnd(6)} ${r.mode}/${r.phase} ${r.metric}: ${r.base} -> ${r.cur} (${r.delta > 0 ? "+" : ""}${r.delta}%)`);
if (flag("--counters")) {
  for (const [mode, phases] of Object.entries(cur.modes)) {
    for (const [name, rec] of Object.entries(phases)) {
      const keys = Object.keys(rec.stats ?? {}).filter((k) => /^(counters\.)?(terrain|gpu_mem|ram|net|assets|storage)\./.test(k) || /^counters\.(terrain|gpu_mem|ram|net|assets|storage)/.test(k));
      for (const k of keys) console.log(`  ${mode}/${name} ${k}: ${metric(base.modes?.[mode]?.[name] ?? {}, `stats.${k}.p50`) ?? "-"} -> ${metric(rec, `stats.${k}.p50`)}`);
    }
  }
}
console.log(c.regressions.length ? `REGRESSION: ${c.regressions.length} metric(s) worse than tolerance` : "OK: no regression beyond tolerance");
const md = val("--md");
if (md) fs.writeFileSync(md, compareMd(base, cur, c));
process.exit(c.regressions.length ? 1 : 0);
