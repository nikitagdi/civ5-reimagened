// `weak` (N3.1, net-test 4.5): a weak channel between the bots and the server. The netproxy adds a round trip of 200 ms
// and holds back 3 percent of the chunks for 200 to 1000 ms (the stall model of the proxy, the way a TCP
// retransmission holds bytes back; real packet loss is `netem`). Two human bots and one AI play on it in `burst`
// pace. The game must be played through: the bots' latency p95 stays under the budget, no bot is dropped on a false
// timeout (per bot and at the gateway), nothing is lost and no view leaks. The control (NT-D10): the channel must
// really have been stalled, or the run proves nothing.
import { PROFILES } from "../lib/link.mjs";
import { botChecks, budgetFor, finishAll, gatewayGame, startGame } from "../lib/play.mjs";

export const meta = {
  id: "weak",
  ac: ["AC-N3.1"],
  needs: ["admin", "proxy"],
  core: ["fixture", "real"],
  quick: { turns: 10 },
  full: { turns: 30 },
};

export async function run(ctx) {
  if (ctx.opts.link === "netem") ctx.pending("netem (real packet loss) path: needs the WSL build with netem plumbing; this run used the netproxy stall model");

  const { server, expect } = ctx;
  const turns = ctx.opts.turns ?? 30;
  const names = ["weakbot1", "weakbot2"];

  const link = await ctx.proxyLink();
  await link.set(PROFILES.weak);

  const budget = budgetFor(turns, 15000);
  const running = startGame(ctx, names, { turns, humans: 2, ai_slots: [2], pace: "burst", url: link.wsUrl });
  const finished = await finishAll(running, budget);
  if (!expect("all bots finished in time", finished !== null, `${budget / 1000} s budget`)) return { numbers: {} };
  const results = botChecks(expect, names, finished, turns, { tag: "weak" });

  const p95Limit = ctx.th.value("weak.p95_ms");
  const dropLimit = ctx.th.value("weak.false_drops");
  results.forEach((r, i) => {
    const who = `bot ${names[i]}`;
    const lat = r.latency_ms ?? {};
    expect(`${who}: latency was measured`, (lat.n ?? 0) > 0, `n ${lat.n ?? 0}`);
    expect(`${who}: latency p95 <= ${ctx.th.text("weak.p95_ms", " ms")}`, Number.isFinite(lat.p95) && lat.p95 <= p95Limit, `p95 ${lat.p95 ?? "?"} ms`);
    const drops = (r.reconnects ?? []).length;
    expect(`${who}: no false drop (reconnects <= ${dropLimit})`, drops <= dropLimit, `${drops} reconnects`);
  });

  const game = results[0].play?.game;
  const m = await server.admin.metrics();
  const lostTimeout = m.server?.sessions?.lost_timeout_total;
  expect(`gateway: sessions lost by timeout <= ${dropLimit}`, lostTimeout <= dropLimit, `${lostTimeout}`);

  const g = await gatewayGame(ctx, game, results);
  if (!expect("the gateway knows the game", !!g, `game ${game}`)) return { numbers: {} };
  expect("gateway: nothing lost (cmds.lost)", g.cmds?.lost === 0, `${g.cmds?.lost}`);
  expect("gateway: fog_violations is 0", g.fog_violations === 0, `${g.fog_violations}`);

  // The control: the channel was really stalled.
  const st = await link.stats();
  const addedMax = Math.max(st.added_delay_max_ms?.up ?? 0, st.added_delay_max_ms?.down ?? 0);
  expect("control: the channel was stalled (added delay max >= 400 ms: a 200 ms stall on top of the 100 ms one-way delay)", addedMax >= 400, `${addedMax} ms`);
  results.forEach((r, i) => {
    if (r.rtt_median === null || r.rtt_median === undefined) return;
    expect(`control: bot ${names[i]} rtt_median >= 150 ms`, r.rtt_median >= 150, `${r.rtt_median} ms`);
  });

  return {
    numbers: {
      game,
      turns,
      link_added_delay_max_ms: st.added_delay_max_ms,
      bots: results.map((r) => ({ nick: r.nick, latency_ms: r.latency_ms, rtt_median: r.rtt_median, reconnects: (r.reconnects ?? []).length })),
    },
  };
}
