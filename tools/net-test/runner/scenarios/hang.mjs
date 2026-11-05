// `hang` (N5.2, game-server 4.10, net-test 4.5): the worker is frozen (`SIGSTOP`, so it sends no frame, not even a
// heartbeat). The gateway must declare the silence a hang after `hang_s` (30 s; the quick preset shortens it to 8 s),
// kill the worker and bring the game back on a new one within `hang_s` + 10 s. Linux only (signals): on Windows the
// scenario is skipped. Three human bots and one AI; the freeze is at turn `kill_at`. After it the bots finish their
// turns, every order has one outcome, `worker.hang_kills` is 1 and `crashes` 1.
import { until } from "../lib/chaos.mjs";
import { botChecks, budgetFor, finishAll, gatewayGame, nicksOf, startGame } from "../lib/play.mjs";

export const meta = {
  id: "hang",
  ac: ["AC-N5.2"],
  needs: ["admin", "hooks", "linux"],
  core: ["fixture", "real"],
  allowErrors: [/^rejected: /, /^order \d+ was lost/],
  quick: { turns: 20, kill_at: 5, serverConfig: { hangS: 8 } },
  full: { turns: 20, kill_at: 5, serverConfig: { hangS: 30 } },
};

export async function run(ctx) {
  const { server, expect, opts } = ctx;
  const hangS = opts.preset === "full" ? 30 : 8;
  const turns = opts.turns;
  const names = nicksOf("hangbot", 3);
  const running = startGame(ctx, names, { turns, humans: 3, ai_slots: [3], knobs: { hold_ms: 150 }, turn_timeout_ms: 180000 });
  const game = await until(async () => ((await server.admin.games()).games ?? []).find((x) => x.status === "Running")?.id ?? null, 60000, 50);
  if (!expect("the game started", game !== null, "no running game")) return { numbers: {} };
  const before = await until(async () => {
    const g = await server.admin.game(game);
    return g && g.turn >= opts.kill_at ? g : null;
  }, 120000, 20);
  if (!expect(`the game reached turn ${opts.kill_at}`, !!before, "it did not")) return { numbers: {} };

  const stopped = await server.killWorker(game, "STOP");
  const t0 = performance.now();
  expect("the hook froze the worker", stopped.ok, `HTTP ${stopped.status}`);
  const back = await until(async () => {
    const g = await server.admin.game(game);
    return g && g.status === "Running" && g.worker.starts > before.worker.starts ? g : null;
  }, (hangS + 40) * 1000, 100);
  const took = (performance.now() - t0) / 1000;
  expect(`the gateway killed the frozen worker and brought the game back within ${hangS} s + 10 s`, !!back && took <= hangS + 10, `${back ? took.toFixed(1) + " s" : "not back"}`);
  expect("the hang is not reported early (not before hang_s - 1 s of silence)", !!back && took >= hangS - 1, `${took.toFixed(1)} s against hang_s ${hangS}`);
  expect("the gateway counted one hang kill and one crash", back && back.worker.hang_kills === 1 && back.worker.crashes === 1, `hang_kills ${back?.worker.hang_kills}, crashes ${back?.worker.crashes}`);
  expect("the worker is a new process", back && back.worker.pid !== before.worker.pid, `pid ${before.worker.pid} -> ${back?.worker.pid}`);

  const finished = await finishAll(running, budgetFor(turns, 15000) + hangS * 1000);
  if (!expect("all bots finished in time", finished !== null, "budget over")) return { numbers: {} };
  const onlyExpected = (f) => (f.result.errors ?? []).every((e) => meta.allowErrors.some((re) => re.test(e)));
  const results = botChecks(expect, names, finished.map((f) => ({ ...f, code: f.code === 0 || (f.code === 1 && onlyExpected(f)) ? 0 : f.code })), turns, { tag: "after the hang" });
  const g = await gatewayGame(ctx, game, results);
  results.forEach((r, i) => expect(`bot ${names[i]}: every order has one outcome, none open`, r.ledger.double_outcome === 0 && r.ledger.open === 0, JSON.stringify(r.ledger)));
  expect("gateway: no fog violation", !!g && g.fog_violations === 0, `${g?.fog_violations}`);
  return { numbers: { hang_s: hangS, back_s: Math.round(took * 10) / 10, turn_at_freeze: before.turn, turn_after: back?.turn } };
}
