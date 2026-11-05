// `optimistic` (N5.8, AC-N2.2, net-test 4.6): the picture of an order does not wait for the server. The real client in
// a browser plays alone against one AI over a channel of RTT 200 ms (the proxy). A move by right click:
//   - the unit stands on the new plot within one frame of the frame that took the click (`optimistic.max_frames`),
//     while the server's answer comes a round trip later (the client's own order time is at least 0.8 of the RTT);
//   - the order settles where the picture put it (no rollback);
//   - on a refusal (the server's test hook `reject_next_cmd`) the view returns to the server's: the unit is back on
//     its plot, the layer counted a rollback, and the journal has a line that says why.
// The frames are counted by the page itself (requestAnimationFrame), not by the runner's polling.
import { AROUND, act, clientStats, orderTimesSince, startBrowserGame, state, timedMove, openPage, until, settle, gatewaySeat } from "../lib/browser.mjs";

export const meta = {
  id: "optimistic",
  ac: ["AC-N2.2"],
  needs: ["browser", "web", "proxy", "hooks", "admin"],
  core: ["fixture", "real"],
  quick: { rtt_ms: 200, tries: 14 },
  full: { rtt_ms: 200, tries: 30 },
};

/** Waits until nothing is in flight and the layer holds no picture. */
const settled = (page, ms = 15000) => until(page, (x) => x.net?.inflight === 0 && x.net?.overlay === 0, ms, "the order settles");

export async function run(ctx) {
  const { expect, opts } = ctx;
  const maxFrames = ctx.th.value("optimistic.max_frames");
  const link = await ctx.proxyLink();
  // A real GPU when there is a window to show: the software GL of the headless browser draws a frame in 100+ ms, which
  // is the client's pump interval, so every round trip it measures grows by that.
  const b = await openPage({ headed: !!ctx.caps.headed, logDir: ctx.logDir });
  const { page } = b;
  const numbers = {};
  try {
    // The game is started over a clean channel; the slow one is switched on when the map is up.
    let s = await startBrowserGame(ctx, page, { nick: "optim", ai: 1, seed: 7, link });
    await link.set({ rtt_ms: opts.rtt_ms, jitter_ms: 0 });
    const seat = await gatewaySeat(ctx, "optim");
    expect("the gateway has the client's seat", !!seat, JSON.stringify(seat?.seat?.nick));
    if (!seat) return { numbers };
    // The round trip as the client measures it (its pings) must show the slow channel.
    s = await until(page, (x) => (x.net?.rttMs ?? 0) >= opts.rtt_ms * 0.8, 15000, "the client measures the slow channel").catch(() => null);
    numbers.client_rtt_ms = (await state(page))?.net?.rttMs ?? null;

    const units = () => state(page).then((x) => x.units.filter((u) => u.movesLeft > 0));
    // One order on a plot beside a unit, tried over the plots around until one is a legal move.
    const tryMoves = async (label) => {
      for (const u of await units()) {
        for (const [dx, dy] of AROUND) {
          // The right click orders the selected unit.
          // The picture of an order needs the unit's context (NC-D6: no picture on a stale one); it comes a round trip
          // after the selection, as long as a player takes to move the mouse.
          await act(page, { kind: "selectUnit", id: u.id });
          await settle(page, (x) => x.net?.ctx === "fresh", 3000);
          const before = await state(page);
          const ring0 = ((await clientStats(page))?.orderMs ?? []).length;
          const m = await timedMove(page, u.id, u.x + dx, u.y + dy);
          if (!m.picture) {
            await settled(page).catch(() => {});
            continue;
          }
          const j0 = before.journal ?? [];
          const after = await settled(page);
          const at = after.units.find((y) => y.id === u.id);
          const stayed = at && at.x === m.to[0] && at.y === m.to[1];
          // A plot the server refuses (water, rivals) is not a move for the tests that need an accepted one.
          if (label !== "rejected" && !stayed) continue;
          return { unit: u.id, from: m.from, to: m.to, m, stayed, after, ring0, j0, label };
        }
      }
      return null;
    };

    // ---- a normal order: the picture at once, the answer a round trip later
    const ok = await tryMoves("move");
    if (!expect("a move was found (a unit that can go to a neighbouring plot)", !!ok)) return { numbers };
    numbers.picture_frames = ok.m.frames;
    numbers.picture_ms = ok.m.pictureMs;
    expect(`the unit is on the new plot within ${maxFrames} frame (page frames from the click to the picture)`, ok.m.frames <= maxFrames, `${ok.m.frames} frame(s), ${ok.m.pictureMs} ms`);
    const times = await orderTimesSince(page, ok.ring0);
    const answerMs = times.length ? times[times.length - 1] : null;
    numbers.answer_ms = answerMs;
    expect("the server's answer comes a round trip after the picture", answerMs !== null && answerMs >= opts.rtt_ms * 0.8 && answerMs > ok.m.pictureMs, `answer ${answerMs} ms, picture ${ok.m.pictureMs} ms, RTT ${opts.rtt_ms}`);
    expect("the order settles where the picture put it (applied, no rollback)", ok.stayed, `unit ${ok.unit}: ${ok.from} -> ${ok.m.to}`);
    const c1 = await clientStats(page);
    expect("the layer drew the picture and no order was rolled back", c1.layerRecords >= 1 && c1.layerRollbacks === 0, `records ${c1.layerRecords}, rollbacks ${c1.layerRollbacks}`);

    // ---- a refused order: the view returns, the journal says why
    await ctx.server.admin.fault(seat.game, seat.player, "reject_next_cmd");
    const rb0 = c1.layerRollbacks;
    const rej = await tryMoves("rejected");
    if (!expect("a second move was found for the refusal", !!rej)) return { numbers };
    numbers.reject_picture_frames = rej.m.frames;
    expect(`the refused order also showed its picture within ${maxFrames} frame`, rej.m.frames <= maxFrames, `${rej.m.frames} frame(s)`);
    expect("the unit is back on its plot (the view is the server's again)", !rej.stayed, `unit ${rej.unit} was ${rej.from}, picture ${rej.m.to}, now ${(() => { const q = rej.after.units.find((y) => y.id === rej.unit); return q ? `${q.x},${q.y}` : "gone"; })()}`);
    const c2 = await clientStats(page);
    expect("the layer counted the rollback", c2.layerRollbacks === rb0 + 1 && c2.orders.rejected >= 1, `rollbacks ${rb0} -> ${c2.layerRollbacks}, rejected ${c2.orders.rejected}`);
    const fin = await settle(page, (x) => (x.journal ?? []).some((l) => !rej.j0.includes(l) && /refus|reject|cannot|can't|not allowed|test hook/i.test(l)), 5000);
    const line = (fin?.journal ?? []).find((l) => !rej.j0.includes(l) && /refus|reject|cannot|can't|not allowed|test hook/i.test(l));
    numbers.journal_line = line ?? null;
    expect("the journal has a line with the reason", !!line, line ?? `journal tail: ${(fin?.journal ?? []).slice(-3).join(" | ")}`);
    // The fault was single-shot: the next order goes through.
    const again = await tryMoves("after");
    expect("the order after the refusal goes through", !!again && again.stayed, again ? `${again.from} -> ${again.m.to}` : "no move");
    const fatal = (await state(page)).fatal;
    expect("no fatal error in the client", !fatal, fatal ?? "");
  } finally {
    expect("no console errors in the page", b.errors.length === 0, b.errors.slice(0, 2).join(" | "));
    await page.screenshot({ path: `${ctx.logDir}/optimistic.png` }).catch(() => {});
    await b.close();
  }
  return { numbers };
}
