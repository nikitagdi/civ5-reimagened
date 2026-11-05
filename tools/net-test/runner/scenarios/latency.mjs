// `latency` (N2.1, net-test 4.5): what an order costs over a slow channel. A channel of RTT 200 +- 20 ms (the proxy;
// on `netem` the same with the real qdisc), one human bot and one AI. The bot gives `orders` orders one after another,
// each after the answer to the previous one, and the time from the send to the final outcome is taken from the bot's
// own `result` events. The bound is against the RTT the bot measured on its pings, not the one that was set:
//   p50 <= RTT + 60 ms, p95 <= RTT + 150 ms (thresholds `latency.*`, provisional).
// Control (NT-D10): the same orders over a clean channel (RTT 0) must give p50 <= 60 ms, the slow channel must cost
// at least 0.8 of the RTT on top of that, or the stand measures nothing.
import { PROFILES } from "../lib/link.mjs";
import { summary } from "../lib/stats.mjs";

export const meta = {
  id: "latency",
  ac: ["AC-N2.1"],
  needs: ["admin", "proxy"],
  core: ["fixture", "real"],
  quick: { orders: 120, control_orders: 40 },
  full: { orders: 500, control_orders: 100 },
};

/** `count` orders in turn, each waiting for its answer; the times of the final outcomes (ms) and how many were applied. */
async function giveOrders(ctx, H, units, count) {
  const kinds = [(u) => ({ UnitSleep: { unit: u } }), (u) => ({ UnitFortify: { unit: u } }), (u) => ({ UnitSkip: { unit: u } })];
  const mark = H.events.length;
  const ids = [];
  for (let i = 0; i < count; i++) {
    const id = await H.cmd(kinds[i % 3](units[i % units.length]));
    ids.push(id);
    await H.wait({ result_id: id }, 30000);
  }
  const finals = new Map();
  for (const e of H.events.slice(mark)) if (e.ev === "result" && String(e.outcome) !== "Queued") finals.set(e.id, e);
  const times = ids.map((id) => finals.get(id)?.ms).filter((x) => typeof x === "number");
  const applied = ids.filter((id) => String(finals.get(id)?.outcome ?? "").startsWith("Applied")).length;
  return { times, applied, sent: ids.length };
}

export async function run(ctx) {
  const { expect, opts } = ctx;
  const link = await ctx.proxyLink();
  if (opts.link === "netem") ctx.pending("netem (real qdisc) path: needs the WSL build with the netem plumbing; this run used the netproxy");
  await link.set({ rtt_ms: 0, jitter_ms: 0 });

  const H = ctx.bots.spawn({ nick: "latbot" });
  await H.connect({ url: link.wsUrl });
  await H.enter({ role: "host", humans: 1, ai_slots: [1] }, { lobby_timeout_ms: 120000 });
  const units = (await H.state()).mine;
  expect("the bot has units to order", units.length > 0, `${units.length} units`);

  // ---- control: a clean channel
  const clean = await giveOrders(ctx, H, units, opts.control_orders);
  const cleanSum = summary(clean.times);
  expect(`control: over a clean channel p50 is at most 60 ms`, cleanSum.p50 !== null && cleanSum.p50 <= 60, `p50 ${cleanSum.p50} ms of ${cleanSum.n} orders`);

  // ---- the slow channel
  await link.set(PROFILES.latency);
  await ctx.sleep(1500); // the pings of the bot measure the new RTT
  const rttBefore = (await H.stats()).rtt_median ?? null;
  const slow = await giveOrders(ctx, H, units, opts.orders);
  const slowSum = summary(slow.times);
  const rtt = (await H.stats()).rtt_median ?? rttBefore ?? link.nominalRtt;
  expect("the bot measured the RTT of the channel", rtt !== null && rtt >= 150, `${rtt} ms measured (set ${link.nominalRtt} ms)`);
  expect(`most of the ${slow.sent} orders were applied (the times are of real round trips)`, slow.applied >= 0.8 * slow.sent, `${slow.applied} of ${slow.sent} applied`);
  expect(`the answers of ${slowSum.n} orders were timed`, slowSum.n >= 0.95 * slow.sent, `${slowSum.n} of ${slow.sent}`);
  const b50 = rtt + ctx.th.value("latency.p50_over_rtt_ms");
  const b95 = rtt + ctx.th.value("latency.p95_over_rtt_ms");
  expect(`p50 <= RTT + ${ctx.th.value("latency.p50_over_rtt_ms")} ms = ${b50.toFixed(0)} ms`, slowSum.p50 <= b50, `p50 ${slowSum.p50} ms (${ctx.th.text("latency.p50_over_rtt_ms", " ms")})`);
  expect(`p95 <= RTT + ${ctx.th.value("latency.p95_over_rtt_ms")} ms = ${b95.toFixed(0)} ms`, slowSum.p95 <= b95, `p95 ${slowSum.p95} ms (${ctx.th.text("latency.p95_over_rtt_ms", " ms")})`);
  expect("control: the slow channel costs at least 0.8 of the RTT more than the clean one", slowSum.p50 - cleanSum.p50 >= 0.8 * rtt, `${slowSum.p50} - ${cleanSum.p50} ms against RTT ${rtt} ms`);

  return { numbers: { rtt_set_ms: link.nominalRtt, rtt_measured_ms: rtt, orders: slow.sent, slow_ms: slowSum, clean_ms: cleanSum, applied: slow.applied } };
}
