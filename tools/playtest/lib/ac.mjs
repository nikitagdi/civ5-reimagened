// The acceptance table of the report (playtest §3.3): every acceptance criterion with its
// priority, how the build checks it (BUILD's own column), what this run checked automatically and
// with what result. Status:
// - pass / fail: the automatic part this run did (the scenario checks named in `evidence`);
// - not-covered: nothing of it ran here (a manual H10 step, a review, a unit test, `perf`).
// `manual` says what is left to the manual acceptance even when the
// automatic part passed.
// Thresholds are the criteria's own; they are not weakened here.

/** [id, priority, criterion (short), BUILD check column, evaluator key | null, manual part (H10) | ""]. */
const ROWS = [
  ["AC-0.1", "Must", "every B1 simplification in the register with all fields", "review", null, "review of the simplification register"],
  ["AC-0.2", "Must", "simp_check passes (stage accept)", "auto", "simpAccept", ""],
  ["AC-0.3", "Must", "interim UI only in interim-ui; ui crate without egui; auto-choices switchable", "auto (deps graph) + review", null, "cargo xtask deps + review"],
  ["AC-0.4", "Must", "module specs cite SIMP numbers", "review", null, "review"],
  ["AC-1.1", "Must", "clean clone to game in <= 5 documented commands", "manual", null, "H10 step 1"],
  ["AC-1.2", "Must", "start screen: New game / Continue; civ, AI count, difficulty, seed", "manual", "lobby", "H10 step 2 (the choices on screen)"],
  ["AC-1.3", "Must", "New game to map <= 30 s", "auto (timing)", "timeToMap", "H8 perf (headed Chrome) is the reference"],
  ["AC-2.1", "Must", "map from the original map script; same seed, same map", "auto", null, "unit/integration tests (cargo xtask test, real core in WSL)"],
  ["AC-2.2", "Should", "map matches civhost plot by plot", "auto", null, "comparison tool"],
  ["AC-2.3", "Must", "original texture per terrain type", "comparison", null, "H10 step 3"],
  ["AC-2.4", "Must", "hills and mountains raised as in the original", "manual", null, "H10 step 3"],
  ["AC-2.5", "Must", "forest/jungle models; marsh/oasis/flood plains/ice", "comparison", null, "H10 step 3"],
  ["AC-2.6", "Must", "rivers on hex edges", "auto + manual", null, "unit tests + H10 step 3"],
  ["AC-2.7", "Must", "revealed resources as icons", "manual", null, "H10 step 3"],
  ["AC-2.8", "Should", "improvements and roads visible", "manual", null, "H10"],
  ["AC-2.9", "Must", "borders in civ colours", "manual", null, "H10"],
  ["AC-2.10", "Must", "fog of war from the brain's visibility", "auto", null, "unit tests (cargo xtask test)"],
  ["AC-2.11", "Should", "map wraps along X", "manual", null, "H10"],
  ["AC-2.12", "Should", "hex grid toggled with G", "manual", null, "H10"],
  ["AC-3.1", "Must", "camera tilt, FOV, clipping as the original", "auto (params) + comparison", null, "unit tests + H10"],
  ["AC-3.2", "Must", "exponential zoom, range, keys, zoom to cursor", "auto (params) + comparison", null, "unit tests + H10"],
  ["AC-3.3", "Must", "drag, arrows, C centres", "manual", null, "H10 step 3"],
  ["AC-3.4", "Should", "edge scrolling, inertia", "manual", null, "H10"],
  ["AC-3.5", "Must", ">= 30 FPS on Duel, also during the AI turn", "auto (timing)", "fps", "H8 perf (headed, GPU); headless FPS is not meaningful"],
  ["AC-4.1", "Must", "original unit models and scale; stubs listed", "auto (report) + manual", null, "asset pipeline report + H10"],
  ["AC-4.2", "Should", "unit formations, figures lost with HP", "comparison", null, "H10"],
  ["AC-4.3", "Must", "unit flag, HP bar, civilian+military both visible", "manual", null, "H10"],
  ["AC-4.4", "Must", "city model and banner", "manual", null, "H10"],
  ["AC-4.5", "Should", "city model by era and style", "comparison", null, "H10"],
  ["AC-4.6", "Must", "barbarian camps and ruins visible", "manual", null, "H10"],
  ["AC-5.1", "Must", "click selects a unit; unit panel", "manual", "unitPanel", "H10 step 4 (highlight, panel contents)"],
  ["AC-5.2", "Must", "right click moves, also multi-turn", "auto + manual", "move", "H10 (multi-turn path)"],
  ["AC-5.3", "Should", "hover path with turn marks", "manual", null, "H10"],
  ["AC-5.4", "Must", "melee attack and visible result", "auto + manual", null, "H10 step 5 (no fight is scripted here)"],
  ["AC-5.5", "Should", "\"Declare war?\" on attacking a civ at peace", "manual", null, "H10"],
  ["AC-5.6", "Must/Should", "ranged attack (unit / city)", "manual", null, "H10 step 5"],
  ["AC-5.7", "Must", "Found city at once; disabled where forbidden", "auto + manual", "foundCity", "H10 (disabled case)"],
  ["AC-5.8", "Must", "worker Automate", "manual", null, "H10 step 4"],
  ["AC-5.9", "Must", "Skip, Fortify/Sleep, Explore", "manual", null, "H10"],
  ["AC-5.10", "Must", "selection moves to the next unit needing orders", "manual", null, "H10"],
  ["AC-5.11", "Must", "city window and production choice", "auto + manual", "cityWindow", "H10 (window contents)"],
  ["AC-5.12", "Should", "purchase with gold in the city window", "manual", "purchase", "H10 if not bought here"],
  ["AC-5.13", "Must", "captured city decided automatically, journaled", "auto", null, "no capture is scripted in the scenarios; H10 step 5"],
  ["AC-6.1", "Must", "end turn button/Enter, blockers, Shift+Enter", "auto + manual", "endTurn", "H10 (Enter, Shift+Enter keys)"],
  ["AC-6.2", "Must", "other choices by the AI, never stuck, journaled (100 turns autoplay)", "auto", "autoChoices", ""],
  ["AC-6.3", "Must", "AI diplomacy offers declined automatically", "auto", "diplo", ""],
  ["AC-6.4", "Must", "live map and \"rivals moving\" indicator during the AI turn", "manual", "aiIndicator", "H10 (camera moves during the AI turn)"],
  ["AC-6.5", "Must", "AI turn median <= 1 s, max <= 3 s by turn 100", "auto (timing)", "aiTime", "H8 perf (headed) is the reference"],
  ["AC-6.6", "Must", "AI results visible", "manual", null, "H10"],
  ["AC-7.1", "Must", "top bar contents", "manual", null, "H10"],
  ["AC-7.2", "Must", "journal of turn events", "manual", "journal", "H10 step 7 (event kinds)"],
  ["AC-7.3", "Should", "journal click centres the camera", "manual", null, "H10 (camera state is not in the test port)"],
  ["AC-7.4", "Should", "plot tooltip on hover", "manual", "tooltip", "H10 (contents)"],
  ["AC-7.5", "Must", "names from the game's localisation", "manual", null, "H10"],
  ["AC-8.1", "Must", "autosave each turn; reload + Continue = same turn and view; 20 more turns", "auto", "saveload", "H10 step 6"],
  ["AC-8.2", "Should", "incompatible autosave: clear error, tab alive", "auto", "badSave", ""],
  ["AC-9.1", "Must", "manual game, 1 AI, Duel, >= 50 turns", "manual", null, "H10 step 5"],
  ["AC-9.2", "Must", "autoplay 100 turns x 3 seeds x {1, 2 AI} without errors", "auto", "matrix", ""],
  ["AC-9.3", "Must", "0 Lua errors beyond the original's", "auto (log)", "lua", ""],
  ["AC-9.4", "Must", "Playwright smoke: new game, 10 turns, reload, Continue (headless)", "auto", "smokeReload", ""],
  ["AC-9.5", "Must", "core errors in journal and console; fatal = message, not white screen", "auto", "fatal", ""],
];

const byName = (results, name) => results.filter((r) => r.scenario === name || r.scenario.startsWith(`${name}[`));

/** The checks of `scenario` whose names start with one of `prefixes`, as one verdict. */
function checks(results, scenario, prefixes) {
  const rs = byName(results, scenario);
  if (rs.length === 0) return { status: "not-covered", evidence: `${scenario} not run` };
  const found = [];
  const missing = [];
  for (const p of prefixes) {
    const hits = rs.flatMap((r) => (r.checks ?? []).filter((c) => c.name.startsWith(p)).map((c) => ({ ...c, scenario: r.scenario })));
    if (hits.length === 0) missing.push(p);
    found.push(...hits);
  }
  const failed = found.filter((c) => !c.pass);
  const status = missing.length || failed.length ? "fail" : "pass";
  const evidence = [
    ...found.map((c) => `${c.scenario}: ${c.pass ? "ok" : "FAIL"} ${c.name}${c.detail ? ` (${String(c.detail).slice(0, 140)})` : ""}`),
    ...missing.map((p) => `${scenario}: check "${p}" did not run (the scenario stopped earlier)`),
  ].join("; ");
  return { status, evidence };
}

const all = (...vs) => {
  if (vs.some((v) => v.status === "not-covered")) {
    return { status: vs.some((v) => v.status === "fail") ? "fail" : "not-covered", evidence: vs.map((v) => v.evidence).join("; ") };
  }
  return { status: vs.some((v) => v.status === "fail") ? "fail" : "pass", evidence: vs.map((v) => v.evidence).join("; ") };
};

const autoplayRuns = (results) => byName(results, "autoplay");
/** The `perf` result, if the run had one (a visible Chrome is the reference for AC-1.3 / 3.5 / 6.5). */
const perfRun = (results) => byName(results, "perf")[0] ?? null;

const EVAL = {
  simpAccept(results) {
    const r = byName(results, "simp")[0];
    if (!r) return { status: "not-covered", evidence: "simp not run" };
    if (!r.accept) return { status: "fail", evidence: "simp_check did not run" };
    return { status: r.accept.pass ? "pass" : "fail", evidence: `simp: simp_check --stage accept: ${r.accept.detail}` };
  },
  lobby: (rs) => all(checks(rs, "smoke", ["screens:"]), checks(rs, "saveload", ["no game and no save before", "the lobby offers [Take]", "the lobby lists the save"])),
  timeToMap(results) {
    // The reference: `perf` in a visible Chrome with the GPU (H8); the headless numbers otherwise.
    const p = perfRun(results);
    if (p?.numbers?.timeToMapMs?.runs?.length) {
      const t = p.numbers.timeToMapMs;
      const ok = t.runs.length >= 3 && t.max <= 30000 && !p.numbers.loads?.some((r) => r.error);
      return { status: ok ? "pass" : "fail", evidence: `perf (${p.numbers.bundle ?? "?"} bundle, ${p.numbers.headed ? "headed" : "headless"}, ${p.numbers.gfx}): Start click to the map with textures, ${t.runs.length} runs: ${t.runs.map((x) => `${Math.round(x)} ms`).join(", ")}; max ${Math.round(t.max)} ms <= 30000` };
    }
    const vals = [...autoplayRuns(results), ...byName(results, "smoke")]
      .map((r) => ({ s: r.scenario, ms: r.numbers?.timeToMapFromStartMs }))
      .filter((x) => typeof x.ms === "number");
    if (!vals.length) return { status: "not-covered", evidence: "no time to map measured" };
    const max = Math.max(...vals.map((x) => x.ms));
    return { status: max <= 30000 ? "pass" : "fail", evidence: `headless SwiftShader, Start to map with textures: ${vals.map((x) => `${x.s} ${Math.round(x.ms)} ms`).join(", ")}; max ${Math.round(max)} ms <= 30000` };
  },
  unitPanel: (rs) => checks(rs, "smoke", ["unit panel of a selected unit"]),
  move: (rs) => checks(rs, "smoke", ["move by right click"]),
  foundCity: (rs) => checks(rs, "smoke", ["Found city offered", "city founded"]),
  cityWindow: (rs) => checks(rs, "smoke", ["city window with builds", "production choice"]),
  purchase(rs) {
    const r = byName(rs, "smoke")[0];
    if (!r) return { status: "not-covered", evidence: "smoke not run" };
    const v = checks(rs, "smoke", r.purchase === "bought" ? ["purchase with gold"] : ["purchase buttons disabled"]);
    if (r.purchase !== "bought" && v.status === "pass") return { status: "not-covered", evidence: `nothing affordable at the start, only the disabled state: ${v.evidence}` };
    return v;
  },
  endTurn: (rs) => all(checks(rs, "smoke", ["end-turn button states", "no stuck turn"]), checks(rs, "autoplay", ["no stuck turn"])),
  autoChoices(rs) {
    const runs = autoplayRuns(rs);
    const v = all(checks(rs, "autoplay", ["no stuck turn", "turns advanced"]), checks(rs, "simp", ["auto-choices in the journal", "no auto-choice line without"]));
    const longest = Math.max(0, ...runs.map((r) => r.numbers?.turnsPlayed ?? 0));
    if (v.status === "pass" && longest < 100) return { status: "not-covered", evidence: `the checks passed but the longest autoplay was ${longest} of 100 turns: ${v.evidence}` };
    return { ...v, evidence: `longest autoplay ${longest} turns; ${v.evidence}` };
  },
  diplo(rs) {
    const runs = autoplayRuns(rs);
    if (!runs.length) return { status: "not-covered", evidence: "autoplay not run" };
    const n = runs.reduce((a, r) => a + (r.numbers?.journal?.simpTagged?.["SIMP-003"] ?? 0), 0);
    const stuck = runs.filter((r) => r.stuck);
    if (stuck.length) return { status: "fail", evidence: `stuck: ${stuck.map((r) => r.scenario).join(", ")}` };
    if (n === 0) return { status: "not-covered", evidence: `no AI offer to the human came in ${runs.map((r) => `${r.scenario} ${r.numbers?.turnsPlayed ?? 0} turns`).join(", ")} (no SIMP-003 line); the turns did not stall` };
    return { status: "pass", evidence: `${n} auto-declined offers (SIMP-003 lines), no stuck turn` };
  },
  aiIndicator(rs) {
    const r = byName(rs, "smoke")[0];
    if (!r) return { status: "not-covered", evidence: "smoke not run" };
    const seen = r.numbers?.aiTurnIndicatorSeen;
    return seen
      ? { status: "pass", evidence: `end-turn button during the rivals' turn: ${(r.numbers.endTurnStates ?? []).filter((x) => x.endsWith("[ai]")).join("; ")}` }
      : { status: "not-covered", evidence: "the rivals' turns were too short to see the indicator in the polls" };
  },
  fps(results) {
    const p = perfRun(results);
    if (!p) return { status: "not-covered", evidence: "perf not run (headless FPS is not meaningful)" };
    if (!p.numbers?.headed) return { status: "not-covered", evidence: "perf ran headless (SwiftShader): the FPS is not meaningful" };
    const v = checks(results, p.scenario, ["fps idle", "fps during the AI turns"]);
    const f = p.numbers.fps;
    return { ...v, evidence: `perf (${p.numbers.bundle ?? "?"} bundle, ${p.numbers.gfx}, display ${p.numbers.refreshHz} Hz${f ? `; idle median ${f.idle.medianFps} / p5 ${f.idle.p5Fps} / min ${f.idle.minFps}; AI turns median ${f.aiTurns.medianFps} / p5 ${f.aiTurns.p5Fps} / min ${f.aiTurns.minFps}` : ""}): ${v.evidence}` };
  },
  aiTime(results) {
    const p = perfRun(results);
    if (p?.numbers?.aiTurn?.all?.n) {
      const a = p.numbers.aiTurn;
      const v = checks(results, p.scenario, ["AI turn median", "AI turn max", "turns advanced"]);
      return { status: v.status, evidence: `perf (${p.numbers.bundle ?? "?"} bundle, ${p.numbers.headed ? "headed" : "headless"}; ${a.source}): turns ${p.numbers.firstTurn}-${p.numbers.lastTurn}: median ${a.all.median} ms, max ${a.all.max} ms; last 10 turns: median ${a.last10.median}, max ${a.last10.max}; wall time median ${a.wallMs.median}, max ${a.wallMs.max}; thresholds 1000 / 3000 ms` };
    }
    const runs = autoplayRuns(results).filter((r) => r.numbers?.turnWallMs?.n);
    if (!runs.length) return { status: "not-covered", evidence: "autoplay not run" };
    // The wall time of a turn (end-turn press to the next human turn) bounds the AI turn from above.
    const bad = runs.filter((r) => r.numbers.turnWallMs.median > 1000 || r.numbers.turnWallMs.max > 3000);
    return {
      status: bad.length ? "fail" : "pass",
      evidence: `headless, whole turn wall time (an upper bound of the AI turn): ${runs.map((r) => `${r.scenario}: median ${r.numbers.turnWallMs.median} ms, max ${r.numbers.turnWallMs.max} ms over ${r.numbers.turnWallMs.n} turns`).join("; ")}; thresholds 1000 / 3000 ms`,
    };
  },
  journal: (rs) => checks(rs, "smoke", ["journal has lines"]),
  tooltip: (rs) => checks(rs, "smoke", ["plot tooltip on hover"]),
  saveload(rs) {
    const v = checks(rs, "saveload", ["played", "the server saved", "the lobby offers [Take]", "same turn after", "same digest after", "same view hash after", "same visible", "turns after the load", "no stuck turn after", "no fatal", "no console errors"]);
    const after = byName(rs, "saveload")[0]?.numbers?.turnsAfterLoad ?? 0;
    if (v.status === "pass" && after < 20) return { status: "not-covered", evidence: `only ${after} of 20 turns after the load: ${v.evidence}` };
    return v;
  },
  badSave: (rs) => checks(rs, "saveload", ["damaged save:", "no usable save:"]),
  matrix(results) {
    const runs = autoplayRuns(results);
    if (!runs.length) return { status: "not-covered", evidence: "autoplay not run" };
    const need = [1, 2, 3].flatMap((s) => [1, 2].map((a) => `${s}/${a}`));
    const ok = new Set(runs.filter((r) => r.pass && (r.numbers?.turnsPlayed ?? 0) >= 100).map((r) => `${r.numbers.seed}/${r.numbers.rivals}`));
    const failed = runs.filter((r) => !r.pass);
    const ran = runs.map((r) => `seed ${r.numbers?.seed} AI ${r.numbers?.rivals}: ${r.pass ? "pass" : "FAIL"}, ${r.numbers?.turnsPlayed ?? 0} turns`).join("; ");
    if (failed.length) return { status: "fail", evidence: ran };
    const missing = need.filter((k) => !ok.has(k));
    return missing.length ? { status: "not-covered", evidence: `matrix incomplete (missing seed/AI ${missing.join(", ")} at 100 turns): ${ran}` } : { status: "pass", evidence: ran };
  },
  lua: (rs) => all(checks(rs, "autoplay", ["no Lua errors in the journal", "no console errors"])),
  smokeReload: (rs) => all(checks(rs, "saveload", ["played", "same digest after", "no console errors"]), checks(rs, "smoke", ["screens:", "no console errors"])),
  fatal: (rs) => checks(rs, "fatal", ["catalogError:", "catalogBytes:", "badMessage:", "no uncaught page error"]),
};

/** The table for `results` (the scenario results of this run). */
export function acTable(results) {
  return ROWS.map(([id, priority, criterion, method, key, manual]) => {
    const v = key ? EVAL[key](results) : { status: "not-covered", evidence: "" };
    const other = /^review/.test(method) ? "review" : /auto/.test(method) ? "unit test / tool (not run by playtest)" : "manual (H10)";
    const by = key ? `scenario${manual ? " + manual (H10)" : ""}` : other;
    return { id, priority, criterion, method, by, status: v.status, evidence: v.evidence || "", manual: manual || "" };
  });
}

/** { pass, fail, "not-covered" } counts, overall and for Must only. */
export function acCounts(table) {
  const count = (rows) => rows.reduce((a, r) => ((a[r.status] = (a[r.status] ?? 0) + 1), a), { pass: 0, fail: 0, "not-covered": 0 });
  return { all: count(table), must: count(table.filter((r) => r.priority.startsWith("Must"))) };
}
