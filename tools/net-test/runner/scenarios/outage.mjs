// `outage` (N3.2, N1.5, net-test 4.5, NT-D18): the network goes away for `cut_s` seconds (10 in the full preset) and
// comes back. One human bot (through the proxy) and one AI. (The game stands while its only human is away: the turn
// waits for him, the timer stops, game-server 4.7; so what the bot has to catch up with is the stream itself.) Four ways to lose
// the link, one after
// the other, on the same game:
//   blackhole   the proxy holds every byte both ways, the sockets stay open (a phone in a tunnel);
//   reset       the connections are reset (RST) and new ones are held;
//   half-open   the way down is dead, the way up is alive (a one-way link);
//   server-close  the gateway closes the socket (the test hook `close_socket`) and new connections are held.
// Judged for each: the view of the bot is current again (its hash equals the gateway's seat hash) within
// `outage.recover_ms` (2 s) of the moment the network is back. The ways that end the socket (reset, server-close)
// need a new connection: the runner makes the bot reconnect at the moment the network is back (the bot of `ctl` has
// no policy of its own), which is the best a probing client can do; the real probe policy and the browser client
// are pending. An order given while the bot knows it has no connection is refused at once and never reaches the
// gateway (no queue, NT-D18). At the end the ledger is clean: no order twice, none open.
import { PROFILES } from "../lib/link.mjs";

export const meta = {
  id: "outage",
  ac: ["AC-N3.2", "AC-N1.5"],
  needs: ["admin", "hooks", "proxy"],
  core: ["fixture", "real"],
  hashMismatchAllowed: true,
  allowErrors: [/^rejected: /, /closed/i, /connect/i, /timeout/i],
  quick: { cut_s: 12 },
  full: { cut_s: 12 },
};

const WAYS = ["blackhole", "reset", "half-open", "server-close"];

export async function run(ctx) {
  const { server, expect, opts } = ctx;
  const cutS = opts.cut_s;
    expect("the cut lasts at least the threshold's time", cutS >= ctx.th.value("outage.cut_s"), `${cutS} s (${ctx.th.text("outage.cut_s", " s")})`);
  const recoverBound = ctx.th.value("outage.recover_ms");
  const link = await ctx.proxyLink();
  await link.set({ ...PROFILES.clean, rtt_ms: 40 });

  const H = ctx.bots.spawn({ nick: "outbot" });
  await H.connect({ url: link.wsUrl });
  await H.enter({ role: "host", humans: 1, ai_slots: [1] }, { lobby_timeout_ms: 120000 });
  const st0 = await H.state();
  const game = st0.attached.game;
  const player = st0.attached.player;
  const unit = st0.mine[0];
  const gw = async () => (await server.admin.game(game)).seats[String(player)].view_hash;
  const { asBig } = await import("../lib/json.mjs");

  // The view of the bot is current: its hash equals what the gateway holds for the seat (read before and after).
  const isDown = async () => (await H.send("wait", { until: { conn: "down" }, timeout_ms: 20 })).met;
  const currentWithin = async (limitMs) => {
    const t0 = performance.now();
    while (performance.now() - t0 < limitMs) {
      const a = await gw();
      const s = await H.state().catch(() => null);
      const b = await gw();
      const h = s?.view?.hash;
      if (h !== undefined && !(await isDown()) && (asBig(h) === asBig(a) || asBig(h) === asBig(b))) return performance.now() - t0;
      await ctx.sleep(40);
    }
    return null;
  };

  const numbers = { ways: {} };
  for (const way of WAYS) {
    await H.wait({ conn: "attached" }, 30000);
    const c0 = (await server.admin.game(game)).cmds;
    // ---- the cut
    if (way === "blackhole") await link.blackhole(cutS * 1000);
    if (way === "half-open") await link.halfOpen("down", cutS * 1000);
    if (way === "reset") {
      await link.blackhole(cutS * 1000);
      await link.reset();
    }
    if (way === "server-close") {
      await link.blackhole(cutS * 1000);
      const f = await server.admin.fault(game, player, "close_socket");
      expect(`${way}: the hook closed the socket`, f.ok, `HTTP ${f.status}`);
    }
    const tCut = performance.now();
    // Does the bot know it has no connection? Then an order is refused on the spot.
    await ctx.sleep(1500);
    const knows = await isDown();
    let refused = null;
    if (knows) {
      refused = false;
      try {
        await H.cmd({ UnitSkip: { unit } });
      } catch (e) {
        refused = /not connected/i.test(String(e));
      }
      expect(`${way}: an order without a connection is refused at once, no queue`, refused === true, `${refused}`);
    }
    // ---- wait for the end of the cut
    await ctx.sleep(Math.max(0, cutS * 1000 - (performance.now() - tCut)) + 300);
    // ---- the network is back
    const tBack = performance.now();
    if (await isDown()) await H.reconnect(true);
    const took = await currentWithin(Math.max(recoverBound * 4, 8000));
    expect(`${way}: the view is current within ${recoverBound} ms of the network coming back`, took !== null && took <= recoverBound, `${took === null ? "not current in " + Math.max(recoverBound * 4, 8000) + " ms" : took.toFixed(0) + " ms"} (${ctx.th.text("outage.recover_ms", " ms")})`);
    const c1 = (await server.admin.game(game)).cmds;
    if (refused) expect(`${way}: the refused order never reached the gateway`, c1.received === c0.received, `received ${c0.received} -> ${c1.received}`);
    // Between the ways: the bot is attached again (reconnect if the bot gave up on its own).
    numbers.ways[way] = { recover_ms: took === null ? null : Math.round(took), knew_it_was_down: knows, order_refused: refused, since_back_ms: Math.round(performance.now() - tBack) };
    await ctx.sleep(500);
  }
  // A client that gives up and a probe policy of its own, and the real client in a browser, come with net-client 4.7.
  ctx.pending("the bot with the probing socket policy (knob reconnect=probe) waits for net-client NC-D17: the reconnect after reset/server-close is made by the runner at the moment the network is back");
  ctx.pending("the same cuts for the real client in a browser (net-client 4.7) need the browser scenarios (N5.8)");
  ctx.pending("blackhole/half-open: an order given while the client does not yet know the link is dead is not refused by the bot (it needs the client's own liveness check, net-client 4.7); the order waits in the channel");

  const fin = await H.stats();
  expect("ledger: no order answered twice, none open", fin.ledger.double_outcome === 0 && fin.ledger.open === 0, JSON.stringify(fin.ledger));
  const m = await server.admin.metrics();
  numbers.sessions = m.server.sessions;
  return { numbers };
}
