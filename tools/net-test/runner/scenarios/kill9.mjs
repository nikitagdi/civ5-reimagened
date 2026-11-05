// `kill9` (N5.2, game-server 4.10, net-test 4.5): the worker dies in the middle of a game. Three human bots and one AI
// play `turns` turns; when the game is at turn `kill_at` the runner sends a real `kill -9` to the worker (the test hook
// `kill-worker`, on Windows `taskkill /F`). The gateway has to:
//   - bring the game back on a new worker within `kill9.recover_s` (30 s) of the kill (`restart_ms` of its own metrics
//     is printed next to the runner's own clock);
//   - lose no more than `kill9.lost_orders_max_s` (60 s) of orders: the newest save on disk was written at most that
//     long before the kill (turn-start saves make it a few seconds);
//   - answer every order of the bots exactly once: applied, rejected or `Lost`, nothing open at the end
//     (`ledger.double_outcome == 0` and `ledger.open == 0` are common checks);
//   - let the bots play on to the end: all of them finish their turns and their views equal the gateway's.
// The game's crash counter says 1, the restart counter 2. Pending: the `System` line in the seat journal ("the game was
// restored from turn T; N of your orders were lost"), which the bot does not read yet.
import { crashWorker, until } from "../lib/chaos.mjs";
import { botChecks, budgetFor, finishAll, gatewayGame, nicksOf, startGame } from "../lib/play.mjs";
import { asBig } from "../lib/json.mjs";

export const meta = {
  id: "kill9",
  ac: ["AC-N5.2"],
  needs: ["admin", "hooks"],
  core: ["fixture", "real"],
  // The orders in flight when the worker dies end as Rejected(Restarting) or Lost: the bot says so in its errors (and
  // exits with 1 for it); that is the outcome the spec asks for, not a fault.
  allowErrors: [/^rejected: /, /^order \d+ was lost/],
  quick: { turns: 30, kill_at: 8 },
  full: { turns: 60, kill_at: 20 },
};

// The save thread of the worker needs about 1.4 s for a save of the real core (1.5 MB packed from 14 MB, debug build,
// WSL disk). Bots that outrun it (a turn of the real core with one AI takes 0.4 s) leave the newest save many turns
// behind the game, and a kill then rolls back far more than "a few turns" for reasons that are not the recovery's. So on
// the real core a bot pauses 2.5 s before an order and a turn takes longer than a save.
const HOLD_REAL_MS = 2500;

export async function run(ctx) {
  const { server, expect, opts } = ctx;
  const turns = opts.turns;
  const names = nicksOf("killbot", 3);
  // A pause before each order keeps the game slow enough for the kill to land in the middle of it.
  const running = startGame(ctx, names, { turns, humans: 3, ai_slots: [3], knobs: { hold_ms: ctx.opts.core === "real" ? HOLD_REAL_MS : 80 }, turn_timeout_ms: 120000 });

  // The game exists as soon as the first bot is attached.
  const game = await until(async () => {
    const list = (await server.admin.games()).games ?? [];
    const g = list.find((x) => x.status === "Running");
    return g ? g.id : null;
  }, 60000, 50);
  if (!expect("the game started", game !== null, "no running game in 60 s")) return { numbers: {} };
  const atKill = await until(async () => {
    const g = await server.admin.game(game);
    return g && g.turn >= opts.kill_at ? g : null;
  }, 120000, 20);
  if (!expect(`the game reached turn ${opts.kill_at}`, !!atKill, "it did not")) return { numbers: {} };

  const crash = await crashWorker(ctx, game, "KILL", { within: 60000 });
  expect("the hook killed the worker", crash.hook.ok, `HTTP ${crash.hook.status}`);
  const recoverBound = ctx.th.value("kill9.recover_s");
  expect(`the game runs on a new worker within ${recoverBound} s`, crash.recoverMs !== null && crash.recoverMs <= recoverBound * 1000, `${crash.recoverMs === null ? "not back in 60 s" : (crash.recoverMs / 1000).toFixed(2) + " s"} (${ctx.th.text("kill9.recover_s", " s")})`);
  expect("the worker is a new process", crash.after && crash.after.worker.pid !== crash.pid, `pid ${crash.pid} -> ${crash.after?.worker.pid}`);
  const lostBound = ctx.th.value("kill9.lost_orders_max_s");
  expect(`the newest save is at most ${lostBound} s older than the kill`, crash.saveAgeS !== null && crash.saveAgeS <= lostBound, `${crash.saveAgeS === null ? "no save on disk" : crash.saveAgeS.toFixed(1) + " s"} (${ctx.th.text("kill9.lost_orders_max_s", " s")}); save of turn ${crash.saveTurn}, the kill at turn ${crash.turnAtKill}`);
  expect("the game goes back no more than a few turns", crash.after && crash.after.turn >= crash.turnAtKill - 3, `turn ${crash.turnAtKill} -> ${crash.after?.turn}`);
  expect("the gateway counted the crash", crash.after && crash.after.worker.crashes === 1 && crash.after.worker.starts === 2, `crashes ${crash.after?.worker.crashes}, starts ${crash.after?.worker.starts}`);

  const finished = await finishAll(running, budgetFor(turns, 15000));
  if (!expect("all bots finished in time", finished !== null, `${budgetFor(turns, 15000) / 1000} s budget`)) return { numbers: { crash: summaryOf(crash) } };
  const onlyExpected = (f) => (f.result.errors ?? []).every((e) => meta.allowErrors.some((re) => re.test(e)));
  const results = botChecks(ctx.expect, names, finished.map((f) => ({ ...f, code: f.code === 0 || (f.code === 1 && onlyExpected(f)) ? 0 : f.code })), turns, { tag: "after the kill", over: 3 });
  const g = await gatewayGame(ctx, game, results);
  if (!expect("the gateway knows the game", !!g, `game ${game}`)) return { numbers: { crash: summaryOf(crash) } };
  results.forEach((r, i) => {
    const gw = g.seats?.[String(r.player)]?.view_hash;
    expect(`bot ${names[i]}'s view equals the gateway's`, gw !== undefined && asBig(gw) === asBig(r.final.view_hash), "");
    expect(`bot ${names[i]}: every order has one outcome, none open`, r.ledger.double_outcome === 0 && r.ledger.open === 0, JSON.stringify(r.ledger));
  });
  expect("gateway: no fog violation", g.fog_violations === 0, `${g.fog_violations}`);
  expect("gateway: the game runs", g.status === "Running", g.status);
  ctx.pending("the System line of the seat journal (restored from turn T, N orders lost) is not read yet");

  const lost = results.reduce((s, r) => s + (r.cmds.lost ?? 0), 0);
  return { numbers: { crash: summaryOf(crash), orders_lost: lost, gateway_lost: g.cmds.lost, restart_ms_gateway: g.worker.restart_ms } };
}

const summaryOf = (c) => ({ recover_s: c.recoverMs === null ? null : Math.round(c.recoverMs) / 1000, save_age_s: c.saveAgeS === null ? null : Math.round(c.saveAgeS * 10) / 10, turn_at_kill: c.turnAtKill, turn_after: c.after?.turn ?? null, save_turn: c.saveTurn });
