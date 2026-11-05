// `target/playtest/report.json` and `report.md` (playtest §3.3): scenarios, results, numbers.
import { writeFileSync } from "node:fs";
import path from "node:path";

export function stats(values) {
  const v = values.filter((x) => typeof x === "number" && Number.isFinite(x)).sort((a, b) => a - b);
  if (v.length === 0) return { n: 0, median: null, max: null, mean: null };
  const mid = Math.floor(v.length / 2);
  const median = v.length % 2 ? v[mid] : (v[mid - 1] + v[mid]) / 2;
  const mean = v.reduce((a, b) => a + b, 0) / v.length;
  return { n: v.length, median: round(median), max: round(v[v.length - 1]), mean: round(mean) };
}

export function round(x) {
  return typeof x === "number" ? Math.round(x * 10) / 10 : x;
}

const fmt = (x, unit = "") => (x === null || x === undefined ? "-" : `${x}${unit}`);

function scenarioMd(r) {
  const out = [`## ${r.scenario}: ${r.pass ? "PASS" : "FAIL"}`, ""];
  out.push(`Duration: ${fmt(round(r.durationMs / 1000), " s")}. Logs: \`${r.logs}\`.`, "");
  if (r.checks?.length) {
    out.push("| Check | Result | Detail |", "|---|---|---|");
    for (const c of r.checks) out.push(`| ${c.name} | ${c.pass ? "pass" : "FAIL"} | ${c.detail ?? ""} |`);
    out.push("");
  }
  if (r.numbers) {
    out.push("| Number | Value |", "|---|---|");
    for (const [k, v] of Object.entries(r.numbers)) out.push(`| ${k} | ${typeof v === "object" ? JSON.stringify(v) : fmt(v)} |`);
    out.push("");
  }
  if (r.turns?.length) {
    out.push("<details><summary>Per turn</summary>", "", "| Turn | Wall ms | AI ms | Actions | Forced |", "|---|---|---|---|---|");
    for (const t of r.turns) out.push(`| ${t.turn} | ${fmt(round(t.wallMs))} | ${fmt(round(t.aiMs))} | ${t.actions ?? ""} | ${t.forced ? "yes" : ""} |`);
    out.push("", "</details>", "");
  }
  if (r.steps?.length) {
    out.push("<details><summary>Steps</summary>", "", "```", ...r.steps.map((s) => JSON.stringify(s)), "```", "", "</details>", "");
  }
  if (r.journalLines?.length) {
    out.push(`<details><summary>Journal lines seen (${r.journalLines.length})</summary>`, "", "```", ...r.journalLines.slice(0, 200), "```", "", "</details>", "");
  }
  if (r.error) out.push("Error:", "", "```", r.error, "```", "");
  if (r.consoleErrors?.length) {
    out.push("Console errors (first 20):", "", "```", ...r.consoleErrors.slice(0, 20), "```", "");
  }
  return out.join("\n");
}

/** Writes both files to `outDir`; returns their paths. */
export function writeReport(outDir, report) {
  const json = path.join(outDir, "report.json");
  const md = path.join(outDir, "report.md");
  writeFileSync(json, JSON.stringify(report, null, 2));
  const lines = [
    "# Playtest report",
    "",
    `Started ${report.startedAt}; build bundle \`${report.webRoot}\`; Playwright ${report.playwright}; ${report.headed ? "headed" : "headless"} Chromium.`,
    "",
    "| Scenario | Result |",
    "|---|---|",
    ...report.scenarios.map((s) => `| ${s.scenario} | ${s.pass ? "PASS" : "FAIL"} |`),
    "",
    ...report.scenarios.map(scenarioMd),
  ];
  if (report.ac?.length) {
    const cell = (s) => String(s ?? "").replace(/\|/g, "\\|").replace(/\n/g, " ");
    const c = report.acCounts;
    lines.push(
      "## Acceptance criteria",
      "",
      "Status is what this run checked automatically: **pass** / **fail**, or **not-covered** when nothing of it ran here. " +
        "\"Manual (H10)\" is what the manual acceptance still has to look at.",
      "",
    );
    if (c) {
      lines.push(
        `All: ${c.all.pass} pass, ${c.all.fail} fail, ${c.all["not-covered"]} not-covered. ` +
          `Must: ${c.must.pass} pass, ${c.must.fail} fail, ${c.must["not-covered"]} not-covered.`,
        "",
      );
      const fails = report.ac.filter((a) => a.status === "fail").map((a) => a.id);
      if (fails.length) lines.push(`Failed: ${fails.join(", ")}.`, "");
    }
    lines.push("| AC | Priority | Criterion | BUILD check | Checked by | Status | Evidence | Manual (H10) |", "|---|---|---|---|---|---|---|---|");
    for (const a of report.ac) {
      const status = a.status === "fail" ? "**fail**" : a.status;
      lines.push(`| ${a.id} | ${a.priority} | ${cell(a.criterion)} | ${cell(a.method)} | ${cell(a.by)} | ${status} | ${cell(a.evidence)} | ${cell(a.manual)} |`);
    }
    lines.push("");
  }
  writeFileSync(md, lines.join("\n"));
  return { json, md };
}
