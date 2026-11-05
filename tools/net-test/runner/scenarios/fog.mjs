// `fog` (N1.3, net-test 4.5): 4 bots, two of them `warmonger` so that the empires meet; 100 turns. The gateway
// (worker audit) and every bot (client audit) find 0 violations. The control (NT-D10): a second server whose test
// world leaks on purpose (`worker.fixture_test.leak_fog`: an unseen unit and the live building of an unseen city in
// place of the last seen) must be caught by both auditors, or the scenario fails as a "blind auditor". The map basis
// is on the audit's white list: it is sent for plots nobody discovered, and that is not a violation.
import { botChecks, budgetFor, finishAll, gatewayChecks, gatewayGame, nicksOf, startGame } from "../lib/play.mjs";

export const meta = {
  id: "fog",
  ac: ["AC-N1.3"],
  needs: ["admin", "hooks"],
  core: ["fixture", "real"],
  quick: { turns: 30, bots: 4, control_turns: 10 },
  full: { turns: 100, bots: 4, control_turns: 10 },
};

export async function run(ctx) {
  const { server, expect, opts } = ctx;
  const turns = opts.turns ?? 100;
  const n = opts.bots ?? 4;
  const names = nicksOf("fogbot", n);

  // ---- the clean run: two warmongers, two peaceful players
  const personas = names.map((_, i) => (i % 2 === 0 ? "warmonger" : "autoplay"));
  const running = startGame(ctx, names, { turns, humans: n, persona: personas, persona_args: personas.map((p) => (p === "warmonger" ? { war_turn: 5 } : undefined)), settings: { turn_mode: "Simultaneous" } });
  const finished = await finishAll(running, budgetFor(turns, 15000));
  if (!expect("all bots finished in time", finished !== null, `${budgetFor(turns, 15000) / 1000} s budget`)) return { numbers: {} };
  const results = botChecks(expect, names, finished, turns);
  const game = results[0].play.game;
  const g = await gatewayGame(ctx, game, results);
  if (!expect("the gateway knows the game", !!g, `game ${game}`)) return { numbers: {} };
  gatewayChecks(expect, g, results, names);
  const checked = results.reduce((s, r) => s + r.fog.checked, 0);
  expect("the bots' audit looked at the messages", checked > 0, `${checked} checks`);
  const wars = results.reduce((s, r) => s + (r.cmds.applied ?? 0), 0);

  // ---- the control: the leak has to be found by both auditors
  const ctl = await ctx.extraServer("control", { fixtureTest: { leak_fog: true } });
  const cnames = nicksOf("leakbot", 2);
  const crun = startGame(ctx, cnames, { bots: ctl.bots, turns: opts.control_turns ?? 10, humans: 2, persona: ["warmonger", "autoplay"], persona_args: [{ war_turn: 3 }, undefined] });
  const cfin = await finishAll(crun, budgetFor(opts.control_turns ?? 10, 15000));
  if (!expect("control: the bots finished in time", cfin !== null, "budget over")) return { numbers: { game } };
  const cres = cfin.map((f) => f.result);
  const cgame = cres[0].play?.game;
  const cg = cgame !== undefined ? await ctl.server.admin.game(cgame) : null;
  expect("control: the worker's audit found the leak (gateway fog_violations > 0)", !!cg && cg.fog_violations > 0, `${cg?.fog_violations}`);
  const botFound = cres.reduce((s, r) => s + r.fog.violations, 0);
  expect("control: the bots' audit found the leak (fog.violations > 0)", botFound > 0, `${botFound} in ${cres.map((r) => r.fog.checked).join(", ")} checks`);

  return {
    numbers: {
      game,
      turns,
      fog_checks: results.map((r) => ({ nick: r.nick, checked: r.fog.checked, violations: r.fog.violations })),
      orders_applied: wars,
      control: { game: cgame, gateway_violations: cg?.fog_violations, bot_violations: botFound },
    },
  };
}
