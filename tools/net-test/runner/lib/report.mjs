// `report.json` and `report.md` (net-test 4.8): the B1 format. Scenario table (PASS / FAIL / SKIP with the
// reason), checks and numbers per scenario, gateway counters before and after, the AC table.
import { writeFileSync } from "node:fs";
import path from "node:path";
import { bigintReplacer } from "./json.mjs";

export function round(x) {
  return typeof x === "number" ? Math.round(x * 10) / 10 : x;
}

/** `{n, min, p50, p95, p99, max, mean}` of a list of numbers (the `H` of the bot's result). */
export function hist(values) {
  const v = values.filter((x) => typeof x === "number" && Number.isFinite(x)).sort((a, b) => a - b);
  if (v.length === 0) return { n: 0, min: null, p50: null, p95: null, p99: null, max: null, mean: null };
  const at = (p) => v[Math.min(v.length - 1, Math.ceil((p / 100) * v.length) - 1)];
  return {
    n: v.length,
    min: round(v[0]),
    p50: round(at(50)),
    p95: round(at(95)),
    p99: round(at(99)),
    max: round(v[v.length - 1]),
    mean: round(v.reduce((a, b) => a + b, 0) / v.length),
  };
}

const fmt = (x, unit = "") => (x === null || x === undefined ? "-" : `${x}${unit}`);
const cell = (s) => String(s ?? "").replace(/\|/g, "\\|").replace(/\n/g, " ");

export const statusText = (r) =>
  r.status === "skip" ? `SKIP (${r.skip})` : r.status === "pass" ? (r.pending?.length ? `PASS (pending: ${r.pending.join("; ")})` : "PASS") : "FAIL";

function scenarioMd(r) {
  const out = [`## ${r.id}: ${statusText(r)}`, ""];
  if (r.status === "skip") return out.concat(["Not run: " + r.skip, ""]).join("\n");
  out.push(`Duration: ${fmt(round(r.durationMs / 1000), " s")}. Logs: \`${r.logs}\`.`, "");
  if (r.params) out.push(`Parameters: ${JSON.stringify(r.params)}.`, "");
  if (r.checks?.length) {
    out.push("| Check | Result | Detail |", "|---|---|---|");
    for (const c of r.checks) out.push(`| ${cell(c.name)} | ${c.pass ? "pass" : "FAIL"} | ${cell(c.detail)} |`);
    out.push("");
  }
  if (r.numbers && Object.keys(r.numbers).length) {
    out.push("| Number | Value |", "|---|---|");
    for (const [k, v] of Object.entries(r.numbers)) out.push(`| ${cell(k)} | ${cell(typeof v === "object" && v !== null ? JSON.stringify(v, bigintReplacer) : fmt(v))} |`);
    out.push("");
  }
  if (r.details) out.push("<details><summary>Details</summary>", "", "```", typeof r.details === "string" ? r.details : JSON.stringify(r.details, bigintReplacer, 1), "```", "", "</details>", "");
  if (r.counters) {
    out.push("<details><summary>Gateway counters (before / after)</summary>", "", "```", JSON.stringify(r.counters, bigintReplacer, 1), "```", "", "</details>", "");
  }
  if (r.error) out.push("Error:", "", "```", r.error, "```", "");
  return out.join("\n");
}

/** Writes both files to `outDir`; returns their paths. */
export function writeReport(outDir, report) {
  const json = path.join(outDir, "report.json");
  const md = path.join(outDir, "report.md");
  writeFileSync(json, JSON.stringify(report, bigintReplacer, 2));
  const e = report.env;
  const lines = [
    "# Net-test report",
    "",
    `Started ${report.startedAt}; profile ${e.profile}; mode ${e.mode}; core ${e.core}; link ${e.link}; ` +
      `server build ${e.serverBuild}; core build ${e.coreBuild}; protocol ${e.protocol}; commit ${e.commit}; ` +
      `host ${e.host}; seed ${e.seed}; preset ${e.preset}; acceptance ${e.acceptance ? "yes" : "no"}.`,
    "",
    "Capabilities: " + Object.entries(report.capabilities).map(([k, v]) => `${k}=${v ? "yes" : "no"}`).join(", ") + ".",
    "",
    "| Scenario | Result | Time |",
    "|---|---|---|",
    ...report.scenarios.map((s) => `| ${s.id} | ${statusText(s)} | ${s.status === "skip" ? "-" : fmt(round(s.durationMs / 1000), " s")} |`),
    "",
    ...report.scenarios.map(scenarioMd),
  ];
  if (report.link) {
    lines.push("## Link", "", "```", JSON.stringify(report.link, null, 1), "```", "");
  }
  if (report.ac?.length) {
    const c = report.acCounts;
    lines.push(
      "## Acceptance criteria (track N)",
      "",
      "Status is what this run checked automatically: **pass** / **fail**, or **not-covered** when nothing of it ran here " +
        "(skipped, manual or no scenario yet). \"Manual\" is what the manual acceptance still has to look at.",
      "",
      `All: ${c.all.pass} pass, ${c.all.fail} fail, ${c.all["not-covered"]} not-covered. ` +
        `Must: ${c.must.pass} pass, ${c.must.fail} fail, ${c.must["not-covered"]} not-covered.`,
      "",
      "| AC | Priority | Criterion | BUILD check | Checked by | Status | Evidence | Manual |",
      "|---|---|---|---|---|---|---|---|",
    );
    for (const a of report.ac) {
      const status = a.status === "fail" ? "**fail**" : a.status;
      lines.push(`| ${a.id} | ${a.priority} | ${cell(a.criterion)} | ${cell(a.method)} | ${cell(a.by.join(", "))} | ${status} | ${cell(a.evidence)} | ${cell(a.manual)} |`);
    }
    lines.push("");
  }
  writeFileSync(md, lines.join("\n"));
  return { json, md };
}
