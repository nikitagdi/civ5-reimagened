// `gateway-kill` (N5.2, N5.5, game-server 4.12, net-test 4.5): `kill -9` of the gateway in the middle of a game. Three
// human bots and one AI play `turns` turns through the runner. At turn `kill_at` the gateway process alone is killed
// (its worker is not touched). Then:
//   - the worker sees EOF on its stdin, saves and leaves by itself (all worker processes gone within 20 s);
//   - a new gateway on the same state dir brings the game back from the newest save (`Running`, within 60 s);
//   - the bots come back with their tokens (a new stream: `Fresh`) and finish all their turns;
//   - their views equal the new gateway's, every order has one outcome, nothing is open.
// Orders in flight when the gateway died get `Lost` or are sent again by the bot after the resume.
import { crashGateway, until } from "../lib/chaos.mjs";
import { botChecks, budgetFor, finishAll, gatewayGame, nicksOf, startGame } from "../lib/play.mjs";
import { asBig } from "../lib/json.mjs";

export const meta = {
  id: "gateway-kill",
  ac: ["AC-N5.2"],
  needs: ["admin", "hooks"],
  core: ["fixture", "real"],
  allowErrors: [/^rejected: /, /^order \d+ was lost/, /^closed:/, /^reconnect failed/, /connect/i],
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
  ctx.restarts = 1; // the new gateway below is requested
  const names = nicksOf("gwkill", 3);
  const running = startGame(ctx, names, { turns, humans: 3, ai_slots: [3], knobs: { hold_ms: ctx.opts.core === "real" ? HOLD_REAL_MS : 80 }, turn_timeout_ms: 120000 });
  const game = await until(async () => ((await server.admin.games()).games ?? []).find((x) => x.status === "Running")?.id ?? null, 60000, 50);
  if (!expect("the game started", game !== null, "no running game in 60 s")) return { numbers: {} };
  const atKill = await until(async () => {
    const g = await server.admin.game(game);
    return g && g.turn >= opts.kill_at ? g : null;
  }, 120000, 20);
  if (!expect(`the game reached turn ${opts.kill_at}`, !!atKill, "it did not")) return { numbers: {} };

  const k = await crashGateway(ctx, game);
  expect("the worker processes ended by themselves after the gateway died", k.workersGone, `${k.workersGoneMs} ms (pids ${k.pids.join(",")})`);
  expect("the workers saved on EOF (a newer save than before the kill)", k.savedAfter && (!k.savedBefore || k.savedAfter.seq >= k.savedBefore.seq), `seq ${k.savedBefore?.seq} -> ${k.savedAfter?.seq}`);
  expect("the new gateway brings the game back from the save", !!k.back, k.back ? `turn ${k.turnBefore} -> ${k.back.turn} in ${(k.backMs / 1000).toFixed(2)} s` : "not running in 60 s");
  expect("the game goes back no more than a few turns", k.back && k.back.turn >= k.turnBefore - 3, `turn ${k.turnBefore} -> ${k.back?.turn}`);

  const finished = await finishAll(running, budgetFor(turns, 15000));
  if (!expect("all bots finished in time (they came back)", finished !== null, `${budgetFor(turns, 15000) / 1000} s budget`)) return { numbers: {} };
  const onlyExpected = (f) => (f.result.errors ?? []).every((e) => meta.allowErrors.some((re) => re.test(e)));
  const results = botChecks(expect, names, finished.map((f) => ({ ...f, code: f.code === 0 || (f.code === 1 && onlyExpected(f)) ? 0 : f.code })), turns, { tag: "after the kill", over: 3 });
  const g = await gatewayGame(ctx, game, results);
  if (!expect("the new gateway knows the game", !!g, `game ${game}`)) return { numbers: {} };
  results.forEach((r, i) => {
    const gw = g.seats?.[String(r.player)]?.view_hash;
    expect(`bot ${names[i]}'s view equals the gateway's`, gw !== undefined && asBig(gw) === asBig(r.final.view_hash), "");
    expect(`bot ${names[i]} came back at least once`, (r.reconnects ?? []).length >= 1, `${(r.reconnects ?? []).length} reconnects`);
    expect(`bot ${names[i]}: every order has one outcome, none open`, r.ledger.double_outcome === 0 && r.ledger.open === 0, JSON.stringify(r.ledger));
  });
  expect("gateway: no fog violation", g.fog_violations === 0, `${g.fog_violations}`);
  return { numbers: { workers_gone_ms: k.workersGoneMs, back_s: Math.round(k.backMs) / 1000, turn_before: k.turnBefore, turn_after: k.back?.turn } };
}
