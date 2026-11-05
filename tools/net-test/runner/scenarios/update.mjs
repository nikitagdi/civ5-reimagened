// `update` (N5.5, game-server 4.12, net-test 4.5), the local form: the drain and the start again of the gateway with a
// game in progress, as `cargo xtask deploy` does it on a host. Three human bots and one AI play `turns` turns. At turn
// `drain_at` the runner calls `POST /admin/drain?exit=1` and, while it runs, tries a new WebSocket upgrade by hand:
//   - the upgrade is answered 503 (with `Retry-After`) while the drain runs;
//   - the call comes back (the games are saved) within 25 s and the gateway process ends with exit code 0;
//   - the bots' sockets are closed with code 1012 ("service restart");
//   - the newest save is of the turn the game was at (nothing lost: the drain saves, unlike a crash);
//   - a new gateway on the same state dir brings the game back, the bots return and finish their turns, their views
//     equal the gateway's, every order has one outcome, nothing is open.
// Against a deployed service (`--attach`, `cargo xtask deploy`) the scenario is skipped: it needs the process.
import { probeUpgrade, savesOnDisk, until } from "../lib/chaos.mjs";
import { botChecks, budgetFor, finishAll, gatewayGame, nicksOf, startGame } from "../lib/play.mjs";
import { asBig } from "../lib/json.mjs";

export const meta = {
  id: "update",
  ac: ["AC-N5.5"],
  needs: ["admin", "hooks"],
  core: ["fixture", "real"],
  attach: false,
  allowErrors: [/^rejected: /, /^order \d+ was lost/, /^closed:/, /^reconnect failed/, /connect/i],
  quick: { turns: 30, drain_at: 8 },
  full: { turns: 60, drain_at: 20 },
};

export async function run(ctx) {
  const { server, expect, opts } = ctx;
  const turns = opts.turns;
  ctx.restarts = 1;
  const names = nicksOf("updbot", 3);
  const running = startGame(ctx, names, { turns, humans: 3, ai_slots: [3], knobs: { hold_ms: 80 }, turn_timeout_ms: 120000 });
  const game = await until(async () => ((await server.admin.games()).games ?? []).find((x) => x.status === "Running")?.id ?? null, 60000, 50);
  if (!expect("the game started", game !== null, "no running game in 60 s")) return { numbers: {} };
  const atDrain = await until(async () => {
    const g = await server.admin.game(game);
    return g && g.turn >= opts.drain_at ? g : null;
  }, 120000, 20);
  if (!expect(`the game reached turn ${opts.drain_at}`, !!atDrain, "it did not")) return { numbers: {} };

  // ---- the drain
  server.expectedStop = true; // the exit is asked for
  const t0 = performance.now();
  const drain = server.drain(true).catch((e) => ({ ok: false, status: 0, error: String(e) }));
  // The 503 is only visible while the drain runs: ask until it answers (or the process is gone).
  let probe = null;
  for (let i = 0; i < 40 && !probe; i++) {
    const p = await probeUpgrade(server);
    if (p && p.status === 503) probe = p;
    else await ctx.sleep(25);
  }
  const answer = await drain;
  const drainMs = performance.now() - t0;
  expect("a new WebSocket upgrade during the drain is answered 503", !!probe, probe ? `503, Retry-After ${probe.retryAfter}` : "never saw a 503 (the drain may be too quick to catch: see numbers)");
  if (probe) expect("the 503 carries Retry-After", probe.retryAfter !== null, `${probe.retryAfter}`);
  expect("the drain call came back with the games saved within 25 s", (answer.ok ?? false) && drainMs <= 25000, `${answer.ok ? "ok" : JSON.stringify(answer)} in ${(drainMs / 1000).toFixed(2)} s`);
  const ended = await until(() => (server.alive() ? null : true), 10000, 50);
  expect("the gateway process ended", !!ended, "still alive 10 s after the drain");
  expect("the exit code is 0", server.lastExit?.code === 0, JSON.stringify(server.lastExit));
  const newest = savesOnDisk(server, game)[0];
  expect("the newest save is of the turn the drain found the game at (or later)", !!newest && newest.turn >= atDrain.turn, `save of turn ${newest?.turn}, the game was at turn ${atDrain.turn}`);

  // ---- the new gateway
  await server.start();
  const back = await until(async () => {
    const g = await server.admin.game(game);
    return g && g.status === "Running" ? g : null;
  }, 60000, 50);
  expect("the new gateway brings the game back", !!back, "not running in 60 s");
  expect("nothing is lost: the game is at the turn it was drained at (or later)", !!back && back.turn >= atDrain.turn, `turn ${atDrain.turn} -> ${back?.turn}`);

  const finished = await finishAll(running, budgetFor(turns, 15000));
  if (!expect("all bots finished in time (they came back)", finished !== null, `${budgetFor(turns, 15000) / 1000} s budget`)) return { numbers: {} };
  const onlyExpected = (f) => (f.result.errors ?? []).every((e) => meta.allowErrors.some((re) => re.test(e)));
  const results = botChecks(expect, names, finished.map((f) => ({ ...f, code: f.code === 0 || (f.code === 1 && onlyExpected(f)) ? 0 : f.code })), turns, { tag: "after the update" });
  const g = await gatewayGame(ctx, game, results);
  if (!expect("the new gateway knows the game", !!g, `game ${game}`)) return { numbers: {} };
  results.forEach((r, i) => {
    const gw = g.seats?.[String(r.player)]?.view_hash;
    expect(`bot ${names[i]}'s view equals the gateway's`, gw !== undefined && asBig(gw) === asBig(r.final.view_hash), "");
    expect(`bot ${names[i]} came back`, (r.reconnects ?? []).length >= 1, `${(r.reconnects ?? []).length} reconnects`);
    expect(`bot ${names[i]}: every order has one outcome, none open`, r.ledger.double_outcome === 0 && r.ledger.open === 0, JSON.stringify(r.ledger));
  });
  // The close code: the bots note it in their timelines.
  let closed1012 = 0;
  for (const n of names) {
    const text = await ctx.host.readFile(`${ctx.logDir}/bots/${n}/timeline.jsonl`).catch(() => "");
    if (text.split("\n").some((l) => l.includes('"ev":"closed"') && l.includes('"code":1012'))) closed1012++;
  }
  expect("the bots' sockets were closed with code 1012", closed1012 === names.length, `${closed1012} of ${names.length}`);
  expect("gateway: no fog violation", g.fog_violations === 0, `${g.fog_violations}`);
  return { numbers: { drain_s: Math.round(drainMs) / 1000, turn_at_drain: atDrain.turn, turn_after: back?.turn, save_turn: newest?.turn, saw_503: !!probe, closed_1012: closed1012 } };
}
