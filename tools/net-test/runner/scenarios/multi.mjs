// `multi` (N5.1, N5.7, net-test 4.5): three games at once on one gateway (`max_games = 3`), two human bots and one AI in
// each, and a fourth host who is refused.
//   - limit: with three games running, a fourth room cannot be started ("too many games" as a LobbyError), the three
//     games are not touched by it;
//   - independence: while they play, the runner spoils one view of game C (the gateway hook `corrupt_hash`); only game
//     C asks for a keyframe, games A and B ask for none and their bots see no hash mismatch;
//   - counters and logs by game: every game has its own entry in the metrics with its own turn and orders (the applied
//     orders of a game are at least the end-turns of its bots), its own `logs/game-<id>.jsonl` in the state dir, its own
//     worker process (three different pids), and the views of the bots equal the gateway's for their own game.
// `kill -9` is not in it (it is `kill9`); against a deployed service the hook part would be left out.
import { existsSync, statSync } from "node:fs";
import path from "node:path";
import { until } from "../lib/chaos.mjs";
import { budgetFor, finishAll, gatewayChecks, gatewayGame, nicksOf, startGame, sum } from "../lib/play.mjs";

export const meta = {
  id: "multi",
  ac: ["AC-N5.1"],
  needs: ["admin", "hooks"],
  core: ["fixture", "real"],
  hashMismatchAllowed: true,
  allowErrors: [/^rejected: /, /a batch missing or repeated/],
  serverConfig: { maxGames: 3 },
  quick: { turns: 25 },
  full: { turns: 60 },
};

export async function run(ctx) {
  const { server, expect, opts } = ctx;
  const turns = opts.turns;
  const groups = ["A", "B", "C"].map((x) => nicksOf(`multi${x}`, 2));
  const running = groups.map((names) => startGame(ctx, names, { turns, humans: 2, ai_slots: [2], knobs: { hold_ms: 120 }, turn_timeout_ms: 120000 }));

  const three = await until(async () => {
    const m = await server.admin.metrics();
    return Object.values(m.games).filter((g) => g.status === "Running").length === 3 ? m : null;
  }, 60000, 50);
  if (!expect("three games run at once", !!three, "not three Running games in 60 s")) return { numbers: {} };
  const idOf = (nick) => Object.entries(three.games).find(([, g]) => Object.values(g.seats).some((s) => s.nick === nick))?.[0];
  const ids = groups.map((n) => idOf(n[0]));
  expect("each room became its own game", new Set(ids).size === 3 && ids.every(Boolean), ids.join(", "));
  const pids = ids.map((id) => three.games[id]?.worker.pid);
  expect("the three games have three worker processes", new Set(pids).size === 3 && pids.every(Boolean), pids.join(", "));

  // ---- the limit
  const F = ctx.bots.spawn({ nick: "multiF" });
  await F.connect();
  let refused = false;
  try {
    await F.enter({ role: "host", humans: 1, ai_slots: [1] }, { lobby_timeout_ms: 8000 });
  } catch {
    refused = true;
  }
  const errs = (await F.stats()).lobby_errors ?? [];
  expect("a fourth room cannot be started", refused || errs.length > 0, JSON.stringify(errs));
  expect("the refusal says why (too many games)", errs.some((e) => /too many games/i.test(e)), JSON.stringify(errs));
  const after4 = await server.admin.metrics();
  expect("the three games are still three, and run", Object.values(after4.games).filter((g) => g.status === "Running").length === 3, "");

  // ---- independence: a bad diff for one seat of game C
  const gidC = ids[2];
  const seatC = Object.entries(after4.games[gidC]?.seats ?? {}).find(([, s]) => s.nick === groups[2][1])?.[0];
  const gotTo = await until(async () => (((await server.admin.game(gidC))?.turn ?? 0) >= Math.floor(turns / 3) ? true : null), 60000, 20);
  expect("game C reached a third of its turns", !!gotTo, "");
  const fault = await server.admin.fault(Number(gidC), Number(seatC), "corrupt_hash");
  expect("the hook spoiled one view in game C", fault.ok, `HTTP ${fault.status}`);

  const all = await finishAll(running.flat(), budgetFor(turns, 15000));
  if (!expect("all six bots finished in time", all !== null, `${budgetFor(turns, 15000) / 1000} s budget`)) return { numbers: {} };
  const byGame = [all.slice(0, 2), all.slice(2, 4), all.slice(4, 6)];
  const numbers = { games: {}, refusal: errs };
  for (let k = 0; k < 3; k++) {
    const tag = `game ${"ABC"[k]}`;
    const res = byGame[k].map((f) => f.result);
    res.forEach((r, i) => {
      expect(`${tag}: bot ${groups[k][i]} played ${turns} turns`, r.play?.done === "turns" && r.turns_played === turns, `done ${r.play?.done}, turns_played ${r.turns_played}`);
      expect(`${tag}: bot ${groups[k][i]} sits in game ${ids[k]}`, String(r.play?.game) === String(ids[k]), `${r.play?.game}`);
      expect(`${tag}: bot ${groups[k][i]} found no fog violation`, r.fog.violations === 0, `${r.fog.violations}`);
    });
    const g = await gatewayGame(ctx, ids[k], res);
    if (!expect(`${tag}: the gateway knows it`, !!g, "")) continue;
    gatewayChecks(expect, g, res, groups[k], { dirty: k === 2 });
    expect(`${tag}: its own turn and orders in its own counters (applied >= end-turns of its bots)`, g.turn >= turns && g.cmds.applied >= 2 * turns, `turn ${g.turn}, applied ${g.cmds.applied}`);
    const need = sum(g.views.need_keyframe);
    if (k < 2) {
      expect(`${tag}: no keyframe was asked for (the fault in game C does not reach it)`, need === 0, JSON.stringify(g.views.need_keyframe));
      res.forEach((r, i) => expect(`${tag}: bot ${groups[k][i]} saw no hash mismatch`, r.hash.mismatch === 0, `${r.hash.mismatch}`));
    } else {
      expect(`${tag}: the spoiled view was noticed and healed (a keyframe was asked for)`, need >= 1, JSON.stringify(g.views.need_keyframe));
    }
    const log = path.join(server.stateDir, "logs", `game-${ids[k]}.jsonl`);
    expect(`${tag}: it has its own log logs/game-${ids[k]}.jsonl`, existsSync(log) && statSync(log).size > 0, existsSync(log) ? `${statSync(log).size} B` : "missing");
    numbers.games[ids[k]] = { turn: g.turn, applied: g.cmds.applied, need_keyframe: need, step_p95_ms: g.hist.step_ms.p95 };
  }
  const end = await server.admin.metrics();
  expect("no game was lost, the server counts three", end.server.games.running === 3, `${end.server.games.running}`);
  return { numbers };
}
