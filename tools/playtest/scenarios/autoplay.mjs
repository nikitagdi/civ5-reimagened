// `autoplay` (playtest §3.2, task H6): a real game in Chromium through the server (`?autostart=1`, the client has no
// brain, net-client NC9), played N turns through the test
// hooks (`?test=1`) the way a player does it on the screens (`lib/policy.mjs`). Checks: no fatal,
// no console errors, the turns advance, no stuck turn, no Lua error in the journal. Numbers: time
// to map, the wall time of each turn (end-turn press to the next human turn) and the rivals'
// share of it; the auto-choice (SIMP-002) and auto-decline (SIMP-003) journal lines seen.
import { openPage, state } from "../lib/browser.mjs";
import { startServerGame } from "../lib/game.mjs";
import { createPlayer, mergeTurnTimes } from "../lib/policy.mjs";
import { JournalLog } from "../lib/journal.mjs";
import { stats, round } from "../lib/report.mjs";

export async function run(ctx) {
  const o = ctx.opts;
  const seed = ctx.seed ?? o.seed;
  const ai = ctx.ai ?? o.ai;
  const name = ctx.name ?? "autoplay";
  const started = Date.now();
  const result = { scenario: name, pass: false, logs: o.logDir, checks: [], numbers: {}, turns: [] };
  const b = await openPage({ headed: o.headed, logDir: o.logDir });
  const { page } = b;
  const journal = new JournalLog();
  const player = createPlayer(page, { turnTimeoutMs: o.turnTimeoutMs, verbose: o.verbose, observe: (s) => journal.add(s) });
  let error = null;
  let first = null;
  let last = null;
  try {
    first = await startServerGame(page, ctx, { nick: "autoplay", ai, seed });
    last = await player.play(first.turn + o.turns);
  } catch (e) {
    error = e;
  }

  // Numbers, also after a failure.
  let metrics = null;
  try {
    metrics = await page.evaluate(() => window.__civ5test?.metrics() ?? null);
    last = (await state(page)) ?? last;
    journal.add(last);
  } catch {
    /* the page is gone */
  }
  result.turns = player.turns;
  result.stuck = player.stuck;
  mergeTurnTimes(result.turns, metrics);
  const st = metrics?.stats ?? {};
  const mapAt = st.timeToMapMs ?? st.timing?.mapAtMs ?? null;
  const startAt = metrics?.startGameAtMs ?? null;
  const played = first && last ? last.turn - first.turn : 0;
  result.numbers = {
    seed,
    rivals: ai,
    firstTurn: first?.turn ?? null,
    lastTurn: last?.turn ?? null,
    turnsPlayed: played,
    timeToMapFromStartMs: mapAt !== null && startAt !== null ? round(mapAt - startAt) : null,
    timeToMapFromPageMs: round(mapAt),
    wasmInstantiateMs: round(st.timing?.wasmInstantiateMs ?? null),
    turnWallMs: stats(result.turns.map((t) => t.wallMs)),
    turnAiMs: stats(result.turns.map((t) => t.aiMs)),
    citiesAtEnd: last?.cities?.length ?? null,
    unitsAtEnd: last?.units?.length ?? null,
    rivalCitiesAtEnd: last?.rivalCities ?? null,
    policy: player.policy,
    journal: journal.summary(),
    frames: metrics?.frames ?? null,
  };
  result.digest = await page.evaluate(() => window.__civ5test?.digest() ?? null).catch(() => null);
  result.journalTail = last?.journal ?? [];
  result.journalLines = journal.lines();
  const fatal = error?.fatal ?? last?.fatal ?? null;
  result.consoleErrors = b.errors.slice();
  const check = (n, pass, detail) => result.checks.push({ name: n, pass: !!pass, detail });
  check("no fatal error", !fatal, fatal ?? "");
  check("no console errors", b.errors.length === 0, `${b.errors.length} error(s)`);
  check("map on screen", first !== null, first ? `turn ${first.turn}` : error?.message);
  check("turns advanced", played >= o.turns || last?.status === "over", `${played} of ${o.turns} turns${last?.status === "over" ? " (game over)" : ""}`);
  check("no stuck turn (STUCK)", !result.stuck, result.stuck ? `turn ${result.stuck.turn}: ${JSON.stringify(result.stuck.blocker)} ${result.stuck.endTurnNote ?? ""}` : "");
  const lua = journal.luaErrors();
  check("no Lua errors in the journal", lua.length === 0, lua.length ? lua.slice(0, 3).join(" | ") : `${journal.size()} journal lines seen`);
  check("no scenario error", !error || !!fatal, error && !fatal ? error.message : "");
  if (error) result.error = error.stack ?? String(error);
  result.pass = result.checks.every((c) => c.pass);
  result.durationMs = Date.now() - started;
  if (o.screenshot) await page.screenshot({ path: `${o.logDir}/last.png` }).catch(() => {});
  await b.close();
  return result;
}
