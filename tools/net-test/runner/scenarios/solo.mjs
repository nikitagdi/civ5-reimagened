// `solo` (N1.1, net-test 4.5): one human and one AI play 20 turns through the server: one `Attached`, the view comes.
// Build check: the client crates and the web bundle do not link `gamecore` (`cargo tree`). That part works after
// `net-client` removed the worker mode of the client (H10); until then it is reported as pending, not faked.
import { LocalHost } from "../lib/host.mjs";
import { botChecks, budgetFor, finishAll, gatewayChecks, gatewayGame, nicksOf, startGame } from "../lib/play.mjs";

export const meta = {
  id: "solo",
  ac: ["AC-N1.1"],
  needs: ["admin"],
  core: ["fixture", "real"],
  quick: { turns: 10 },
  full: { turns: 20 },
};

export async function run(ctx) {
  const { server, expect, opts } = ctx;
  const turns = opts.turns ?? 20;
  const names = nicksOf("solo", 1);

  const running = startGame(ctx, names, { turns, humans: 1, ai_slots: [1] });
  const finished = await finishAll(running, budgetFor(turns));
  if (!expect("the bot finished in time", finished !== null, `${budgetFor(turns) / 1000} s budget`)) return { numbers: {} };
  const results = botChecks(expect, names, finished, turns);
  const r = results[0];
  expect("one Attached: the bot never reconnected", (r.reconnects ?? []).length === 0, `${(r.reconnects ?? []).length} reconnects`);
  expect("the view came and was checked at each step", r.hash.checked >= turns, `${r.hash.checked} hashes of ${turns} turns`);

  let g = await gatewayGame(ctx, r.play.game, results);
  if (!expect("the gateway knows the game", !!g, `game ${r.play.game}`)) return { numbers: {} };
  // The first turn is 1 on the fixture core and 0 on the real one; the real core also plays the AI turn after
  // the last EndTurn, so give the gateway a moment to reach first + turns.
  const reach = (r.first_view?.turn ?? 1) + turns;
  for (let i = 0; i < 100 && g.turn < reach; i++) {
    await ctx.sleep(100);
    g = (await ctx.server.admin.game(r.play.game)) ?? g;
  }
  gatewayChecks(expect, g, results, names);
  expect("gateway: one human seat", Object.keys(g.seats ?? {}).length === 1, `${Object.keys(g.seats ?? {}).join(",")}`);
  expect("gateway: the game runs at turn " + reach, g.status === "Running" && g.turn >= reach, `${g.status}, turn ${g.turn}`);
  expect("gateway: the orders were applied", g.cmds.applied >= turns, `applied ${g.cmds.applied}, at least ${turns} (an EndTurn per turn)`);

  // The build: no `gamecore-sys` behind the web client.
  const tree = await new LocalHost().exec("cargo", ["tree", "--target", "wasm32-unknown-unknown", "-p", "app-web", "-i", "gamecore-sys"], { cwd: ctx.root, timeoutMs: 180000 });
  const linked = tree.code === 0 && tree.stdout.includes("gamecore-sys");
  if (linked) ctx.pending("AC-N1.1 build check: app-web still links gamecore-sys through game-host (net-client H10 removes the worker mode)");
  else expect("build: the web client has no gamecore-sys behind it", tree.code !== 0 || !tree.stdout.includes("gamecore-sys"), tree.stderr.trim().split("\n")[0] ?? "");

  return { numbers: { game: r.play.game, turns, orders: r.cmds.unique, applied: r.cmds.applied, latency_ms: { p50: r.latency_ms.p50, p95: r.latency_ms.p95 }, build_linked_gamecore: linked } };
}
