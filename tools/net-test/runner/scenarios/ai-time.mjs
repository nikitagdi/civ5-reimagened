// `ai-time` (N5.4, AC-N5.4, net-test 4.5): how long the AI phase takes on the real core, `hist.ai_phase_ms` of the gateway.
// The human bot plays the shared policy (`autoplay`: the real core refuses an end of turn while a unit waits for an order,
// so an `idle` bot that only ends turns cannot play there); the AI phase is what is measured. Three parts:
//   A. Duel, 1 bot and 1 AI: the median of the AI phase <= `ai.duel.median_s`;
//   B. Standard, 8 civilizations (1 bot and 7 AI): the median <= `ai.std8.median_s`, the maximum <= `ai.std8.max_s`;
//   C. a late Huge game, from the save `make-late-save` made (Huge, 2 bots, turn 150), 20 more turns: the median
//      <= `ai.huge.median_s`. Without the save, or when the save is from another core build, the part is pending and says
//      to run `make-late-save` again (a save is only good for the core it was made with).
// The thresholds are provisional (BUILD N5.4); a number over its threshold is a failed check, never loosened. A debug
// build of the server is slower than a release one in the gateway only; the AI itself is the game core.
// Real core only: `cargo xtask nettest --wsl --core real --scenario ai-time [--full]`.
import path from "node:path";
import { botChecks, finishWatched, gatewayChecks, gatewayGame, nicksOf, startGame } from "../lib/play.mjs";
import { lateSaveDir, readLateMeta, restoreLateSave } from "../lib/latesave.mjs";

export const meta = {
  id: "ai-time",
  ac: ["AC-N5.4"],
  needs: ["admin"],
  core: ["real"],
  quick: { duel_turns: 10, std_turns: 10, huge_turns: 5 },
  full: { duel_turns: 100, std_turns: 100, huge_turns: 20 },
};

const s = (ms) => ms / 1000;
const r3 = (x) => Math.round(x * 1000) / 1000;

/** The AI phase numbers of a game once its bots are done: waits until the gateway has the last AI phase. */
async function aiPhase(ctx, game, turns) {
  let g = null;
  for (let i = 0; i < 300; i++) {
    g = await ctx.server.admin.game(game);
    if (g && (g.hist?.ai_phase_ms?.n ?? 0) >= turns) break;
    await ctx.sleep(200);
  }
  return g;
}

async function freshGame(ctx, o) {
  const { expect } = ctx;
  const names = nicksOf(o.prefix, 1);
  const budget = 120000 + o.turns * o.perTurnMs;
  const running = startGame(ctx, names, {
    turns: o.turns,
    humans: 1,
    ai_slots: Array.from({ length: o.ai }, (_, i) => 1 + i),
    settings: { world_size: o.size },
    persona: "autoplay",
    pace: "burst",
    lobby_timeout_ms: 300000,
    turn_timeout_ms: 300000,
  });
  const { finished, fault } = await finishWatched(ctx, running, budget);
  if (!expect(`${o.label}: the bot finished in time`, finished !== null, fault ?? `${budget / 1000} s budget`)) return null;
  const results = botChecks(expect, names, finished, o.turns, { tag: o.label });
  const game = results[0].play?.game;
  const g0 = await gatewayGame(ctx, game, results);
  if (!expect(`${o.label}: the gateway knows the game`, !!g0, `game ${game}`)) return null;
  const g = (await aiPhase(ctx, game, o.turns)) ?? g0;
  gatewayChecks(expect, g, results, names);
  return { game, g, results };
}

function judge(ctx, label, g, turns, medianKey, maxKey) {
  const { expect } = ctx;
  const a = g.hist?.ai_phase_ms ?? {};
  expect(`${label}: the AI phase was measured (at least ${turns} phases)`, (a.n ?? 0) >= turns, `${a.n ?? 0} phases`);
  expect(`${label}: median AI phase <= ${ctx.th.text(medianKey, " s")}`, (a.n ?? 0) > 0 && s(a.p50) <= ctx.th.value(medianKey), `median ${r3(s(a.p50 ?? 0))} s (p95 ${r3(s(a.p95 ?? 0))} s, max ${r3(s(a.max ?? 0))} s, n ${a.n ?? 0})`);
  if (maxKey) expect(`${label}: maximum AI phase <= ${ctx.th.text(maxKey, " s")}`, s(a.max ?? 0) <= ctx.th.value(maxKey), `max ${r3(s(a.max ?? 0))} s`);
  return { ai_phase_ms: a, step_ms: g.hist?.step_ms ?? null };
}

export async function run(ctx) {
  const { expect, server, opts } = ctx;
  const numbers = {};

  // ---- A. Duel
  const duelTurns = opts.duel_turns ?? 100;
  const duel = await freshGame(ctx, { label: "A Duel", prefix: "aitd", ai: 1, size: "WORLDSIZE_DUEL", turns: duelTurns, perTurnMs: 20000 });
  if (duel) numbers.duel = { game: duel.game, turns: duelTurns, ...judge(ctx, "A Duel", duel.g, duelTurns, "ai.duel.median_s") };
  if (duel) await server.admin.stop(duel.game);

  // ---- B. Standard, 8 civilizations
  const stdTurns = opts.std_turns ?? 100;
  const std = await freshGame(ctx, { label: "B Standard 8", prefix: "aits", ai: 7, size: "WORLDSIZE_STANDARD", turns: stdTurns, perTurnMs: 30000 });
  if (std) numbers.standard8 = { game: std.game, turns: stdTurns, ...judge(ctx, "B Standard 8", std.g, stdTurns, "ai.std8.median_s", "ai.std8.max_s") };
  if (std) await server.admin.stop(std.game);

  // ---- C. a late Huge game from the save of `make-late-save`
  const hugeTurns = opts.huge_turns ?? 20;
  const dir = lateSaveDir(ctx.root);
  const saved = readLateMeta(dir);
  const metrics = await server.admin.metrics();
  if (!saved) {
    ctx.pending(`C late Huge: no save of a late game at ${dir}; run 'cargo xtask nettest --wsl --core real --scenario make-late-save --full' first`);
    return { numbers };
  }
  if (saved.core_build !== (metrics.core_build ?? null)) {
    ctx.pending(`C late Huge: the save is from core build ${saved.core_build}, this server runs ${metrics.core_build}; run make-late-save again`);
    return { numbers };
  }
  const names = saved.nicks;
  await server.kill();
  restoreLateSave({ dir, stateDir: server.stateDir, botsDir: path.join(ctx.logDir, "bots"), nicks: names });
  await server.start();
  ctx.restarts = (ctx.restarts ?? 0) + 1;
  const before = await server.admin.metrics();
  expect("C: the restored server runs no game yet", before.server.games.running === 0, `${before.server.games.running}`);

  // The bots come back with their tokens: the game wakes from the save. One of them plays `idle`, the other too.
  const common = { turns: hugeTurns, pace: "burst", persona: "autoplay", lobby_timeout_ms: 300000, turn_timeout_ms: 600000 };
  const running = names.map((nick) => ctx.bots.play(nick, common));
  const budget = 300000 + hugeTurns * 60000;
  const { finished, fault } = await finishWatched(ctx, running, budget);
  if (!expect("C: both bots finished in time", finished !== null, fault ?? `${budget / 1000} s budget`)) return { numbers };
  const results = botChecks(expect, names, finished, hugeTurns, { tag: "C" });
  const game = results[0].play?.game;
  expect("C: the bots sit in the game of the save", game === saved.game, `game ${game}, the save's game ${saved.game}`);
  const g0 = await gatewayGame(ctx, game, results);
  if (!expect("C: the gateway knows the game", !!g0, `game ${game}`)) return { numbers };
  // Phases of the woken game: all of them come after the wake (the metrics of the gateway start with the process).
  const g = (await aiPhase(ctx, game, hugeTurns)) ?? g0;
  gatewayChecks(expect, g, results, names);
  expect("C: the game went on from the save's turn", g.turn >= saved.turn + hugeTurns, `turn ${g.turn}, the save was at ${saved.turn}`);
  numbers.huge_late = { game, from_turn: saved.turn, turns: hugeTurns, save_core_build: saved.core_build, ...judge(ctx, "C late Huge", g, hugeTurns, "ai.huge.median_s") };
  return { numbers };
}
