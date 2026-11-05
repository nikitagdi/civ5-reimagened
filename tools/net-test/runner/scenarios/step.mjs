// `step` (N2.5, net-test 4.5): how long one game step takes on the server, measured by the gateway (`hist.step_ms`).
// Two games one after the other on the same server, simultaneous mode, `burst` pace (the orders come as fast as the
// bots can send them, so the steps follow one another). Part A: Duel, 4 human bots, no AI. Part B: Huge, 8 human bots,
// at most 15 turns. After each part the gateway must have measured at least `turns` steps, the 95th percentile of the
// step time must stay within its budget (`step.p95_ms.duel4` for Duel, `step.p95_ms.huge8` for Huge), and the worker
// must not have crashed. The budgets are provisional (BUILD N2.5): a failing check is a measured number over the
// budget, and is reported as it is.
import { botChecks, budgetFor, finishAll, gatewayGame, nicksOf, startGame } from "../lib/play.mjs";

export const meta = {
  id: "step",
  ac: ["AC-N2.5"],
  needs: ["admin"],
  core: ["fixture", "real"],
  quick: { turns: 15 },
  full: { turns: 40 },
};

async function part(ctx, o) {
  const { expect } = ctx;
  const names = nicksOf(o.prefix, o.humans);
  const budget = budgetFor(o.turns, 15000);
  const running = startGame(ctx, names, {
    turns: o.turns,
    humans: o.humans,
    ai_slots: [],
    settings: { world_size: o.size, turn_mode: "Simultaneous" },
    pace: "burst",
  });
  const finished = await finishAll(running, budget);
  if (!expect(`${o.label}: all bots finished in time`, finished !== null, `${budget / 1000} s budget`)) return null;
  const results = botChecks(expect, names, finished, o.turns, { tag: o.label });

  const game = results[0].play?.game;
  const g = await gatewayGame(ctx, game, results);
  if (!expect(`${o.label}: the gateway knows the game`, !!g, `game ${game}`)) return null;

  const st = g.hist?.step_ms ?? {};
  const limit = ctx.th.value(o.thKey);
  const budgetText = ctx.th.text(o.thKey, " ms");
  expect(`${o.label}: the steps were measured (at least ${o.turns})`, (st.n ?? 0) >= o.turns, `${st.n ?? 0} steps`);
  expect(`${o.label}: p95 step <= ${budgetText}`, Number.isFinite(st.p95) && st.p95 <= limit, `p95 ${st.p95 ?? "?"} ms, budget ${budgetText}`);
  expect(`${o.label}: the worker never crashed`, g.worker?.crashes === 0, `${g.worker?.crashes}`);

  return { game, turns: o.turns, step_ms: g.hist?.step_ms ?? null, ai_phase_ms: g.hist?.ai_phase_ms ?? null };
}

export async function run(ctx) {
  const turns = ctx.opts.turns ?? 40;
  const numbers = {};

  const a = await part(ctx, { label: "A Duel 4", prefix: "stepd", humans: 4, size: "WORLDSIZE_DUEL", turns, thKey: "step.p95_ms.duel4" });
  if (a) numbers.part_a = a;

  const b = await part(ctx, { label: "B Huge 8", prefix: "steph", humans: 8, size: "WORLDSIZE_HUGE", turns: Math.min(turns, 15), thKey: "step.p95_ms.huge8" });
  if (b) numbers.part_b = b;

  return { numbers };
}
