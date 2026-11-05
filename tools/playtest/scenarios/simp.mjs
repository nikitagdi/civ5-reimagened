// `simp` (playtest §3.2, task H7; AC-0.2, AC-6.2 journal part): the register check of the
// workspace (`tools/simp_check.py`) in both stages, and in a game through the server
// (`?autostart=1`, net-client NC9; the server's fixture core makes no auto-choices, a real core does): the
// auto-choices the AI makes for the human are journal lines tagged with their SIMP number, and
// every number the journal shows is an open entry of the register.
// The scenario passes on the `dev` stage (what the modules must keep green); the `accept` stage
// (AC-0.2, run by H10) is reported as its own check.
import { spawnSync } from "node:child_process";
import { existsSync, writeFileSync } from "node:fs";
import path from "node:path";
import { openPage } from "../lib/browser.mjs";
import { startServerGame } from "../lib/game.mjs";
import { createPlayer } from "../lib/policy.mjs";
import { JournalLog } from "../lib/journal.mjs";

/** `--simp-check`, else `tools/simp_check.py` in the nearest folder above the repository. */
export function findSimpCheck(repoRoot, given) {
  if (given) return path.resolve(given);
  let dir = path.dirname(repoRoot);
  for (;;) {
    const p = path.join(dir, "tools", "simp_check.py");
    if (existsSync(p)) return p;
    const up = path.dirname(dir);
    if (up === dir) return null;
    dir = up;
  }
}

function simpCheck(script, stage, logDir) {
  const r = spawnSync(process.env.PYTHON || "python", [script, "--stage", stage, "--json"], { cwd: path.dirname(path.dirname(script)), encoding: "utf8", windowsHide: true });
  writeFileSync(path.join(logDir, `simp_check-${stage}.json`), r.stdout ?? "");
  if (r.stderr) writeFileSync(path.join(logDir, `simp_check-${stage}.stderr.txt`), r.stderr);
  let json = null;
  try {
    json = JSON.parse(r.stdout);
  } catch {
    /* not JSON */
  }
  const findings = json?.findings ?? [];
  return {
    stage,
    exitCode: r.status,
    error: r.error?.message ?? null,
    errors: findings.filter((f) => f.level === "error"),
    warnings: findings.filter((f) => f.level !== "error"),
    summary: json?.summary ?? [],
  };
}

const fmt = (fs) => fs.map((f) => `${f.code} ${f.path}:${f.line} ${f.message}`).join("; ");

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "simp", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const script = findSimpCheck(path.resolve(ctx.root, "../.."), o.simpCheck);
  let open = null;
  if (!script) {
    check("simp_check.py found", false, "no tools/simp_check.py above the repository; pass --simp-check <path>");
  } else {
    const dev = simpCheck(script, "dev", o.logDir);
    const accept = simpCheck(script, "accept", o.logDir);
    result.simpCheck = { script, dev: { ...dev, summary: undefined }, accept: { ...accept, summary: undefined } };
    check("simp_check --stage dev: 0 errors", dev.exitCode === 0 && dev.errors.length === 0, `exit ${dev.exitCode}, ${dev.errors.length} errors, ${dev.warnings.length} warnings${dev.warnings.length ? `: ${fmt(dev.warnings)}` : ""}${dev.error ? `; ${dev.error}` : ""}`);
    result.accept = { pass: accept.exitCode === 0 && accept.errors.length === 0, detail: `exit ${accept.exitCode}, ${accept.errors.length} errors${accept.errors.length ? `: ${fmt(accept.errors)}` : ""}` };
    open = new Map(dev.summary.map((s) => [s.id, s.status]));
    result.numbers.register = { entries: dev.summary.length, open: dev.summary.filter((s) => s.status === "открыто").length };
    result.numbers.acceptStage = result.accept;
  }

  // In the game.
  const b = await openPage({ headed: o.headed, logDir: o.logDir });
  const { page } = b;
  const journal = new JournalLog();
  const player = createPlayer(page, { turnTimeoutMs: o.turnTimeoutMs, observe: (s) => journal.add(s) });
  let error = null;
  try {
    const first = await startServerGame(page, ctx, { nick: "simp" });
    journal.add(await player.play(first.turn + o.simpTurns));
  } catch (e) {
    error = e;
  }
  const tagged = journal.bySimp();
  const numbers = Object.keys(tagged).map((n) => `SIMP-${n}`);
  const auto002 = tagged["002"] ?? [];
  // On the server path the humans choose for themselves and the server's fixture core makes no auto-choice (only the
  // real core does, SIMP-002): with none made, the check holds when the journal is alive and carries no auto-choice
  // line at all; the lines of a real core must carry the tag.
  const autoLike = journal.lines().filter((l) => /^T-?\d+: (Research|Free tech|Policy|Ideology|Pantheon|Religion|Promotion)/i.test(l));
  check("auto-choices in the journal carry SIMP-002", auto002.length > 0 || (autoLike.length === 0 && journal.size() > 0), auto002.length ? `${auto002.length} lines, e.g. ${auto002.slice(0, 2).join(" | ")}` : `none in ${journal.size()} lines (the fixture core makes no auto-choices)`);
  // An auto-choice reads "Research ...", "Policy ...", ... (the core's `what`): such a line without
  // its tag is an auto-choice that lost its number.
  const untagged = journal.lines().filter((l) => /^T-?\d+: (Research|Free tech|Policy|Ideology|Pantheon|Religion|Promotion)\b/i.test(l) && !/\(auto, SIMP-\d{3}\)$/.test(l));
  check("no auto-choice line without its SIMP number", untagged.length === 0, untagged.slice(0, 3).join(" | ") || "none");
  if (open) {
    const unknown = numbers.filter((n) => open.get(n) !== "открыто");
    check("journal SIMP numbers are open register entries", unknown.length === 0, `${numbers.join(", ") || "none"}${unknown.length ? `; not open: ${unknown.map((n) => `${n} (${open.get(n) ?? "absent"})`).join(", ")}` : ""}`);
  }
  check("no scenario error", !error, error ? error.message : "");
  check("no console errors", b.errors.length === 0, `${b.errors.length} error(s)`);
  result.consoleErrors = b.errors.slice();
  result.journalLines = journal.lines();
  result.numbers.journal = journal.summary();
  if (error) result.error = error.stack ?? String(error);
  await b.close();
  result.pass = result.checks.every((c) => c.pass);
  result.durationMs = Date.now() - started;
  return result;
}
