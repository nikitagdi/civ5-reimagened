// `pipeline` (N2.4, net-test 4.5): the next order can be given without waiting for the answer to the previous one.
// A channel of RTT 200 ms; the bot sends 20 orders (Sleep, Fortify, Skip) to its units one after another with no
// waiting. All 20 answers arrive within RTT + 400 ms (not 20 x RTT = 4 s), and no more than 32 are in the air at
// once (the bot's own window). With the `window` knob the bot sends 70: the worker queue holds 64 and the rest is
// `Rejected { TooMany }`; the server lives. A burst the worker drains faster than it arrives has no excess, and the
// excess is then reported as pending (the AI phase of the test world cannot be held open yet).
export const meta = {
  id: "pipeline",
  ac: ["AC-N2.4"],
  needs: ["admin", "proxy"],
  core: ["fixture", "real"],
  quick: { rtt_ms: 200, orders: 20 },
  full: { rtt_ms: 200, orders: 20 },
};

export async function run(ctx) {
  const { server, expect, opts } = ctx;
  const rttSet = opts.rtt_ms ?? 200;
  const count = opts.orders ?? 20;
  const link = await ctx.proxyLink();
  await link.set({ rtt_ms: rttSet });
  const H = ctx.bots.spawn({ nick: "pipebot" });
  await H.connect({ url: link.wsUrl });
  await H.enter({ role: "host", humans: 1, ai_slots: [1] }, { lobby_timeout_ms: 120000 });
  const st = await H.state();
  const units = st.mine;
  expect("the bot has units to order", units.length > 0, `${units.length} units`);

  // The measured RTT of the channel (the bot's pings): the thresholds are against it, not against the set value.
  let rtt = null;
  for (let i = 0; i < 100 && rtt === null; i++) {
    rtt = (await H.stats()).rtt_median ?? null;
    if (rtt === null) await ctx.sleep(200);
  }
  expect("the bot measured the RTT of the channel", rtt !== null, `${rtt} ms (set ${rttSet})`);
  const bound = (rtt ?? rttSet) + 400;

  const kinds = [(u) => ({ UnitSleep: { unit: u } }), (u) => ({ UnitFortify: { unit: u } }), (u) => ({ UnitSkip: { unit: u } })];
  const mark = H.events.length;
  const ids = [];
  const t0 = performance.now();
  for (let i = 0; i < count; i++) ids.push(await H.cmd(kinds[i % 3](units[i % units.length])));
  const tSent = performance.now() - t0;
  await Promise.all(ids.map((id) => H.wait({ result_id: id }, 30000)));
  const total = performance.now() - t0;
  const results = H.events.slice(mark).filter((e) => e.ev === "result");
  const finals = new Map();
  for (const e of results) finals.set(e.id, e);
  const worst = Math.max(...ids.map((id) => finals.get(id)?.ms ?? Infinity));
  expect(`all ${count} answers came`, ids.every((id) => finals.has(id)), `${finals.size} of ${count}`);
  expect(`all ${count} answers within RTT + 400 ms = ${bound.toFixed(0)} ms (not ${count} x RTT)`, total <= bound && worst <= bound, `${total.toFixed(0)} ms in all, the slowest order ${worst.toFixed(0)} ms, sending took ${tSent.toFixed(0)} ms`);
  expect(`the orders did not wait one for another (total < ${count} x RTT / 2)`, total < (count * (rtt ?? rttSet)) / 2, `${total.toFixed(0)} ms against ${(count * (rtt ?? rttSet)).toFixed(0)} ms`);

  // The window: 70 orders in the air.
  await H.knob("window", true);
  const g0 = (await server.admin.game(st.attached.game)).cmds;
  const mark2 = H.events.length;
  const ids2 = [];
  for (let i = 0; i < 70; i++) ids2.push(await H.cmd(kinds[i % 3](units[i % units.length])));
  await Promise.all(ids2.map((id) => H.wait({ result_id: id }, 60000)));
  const by = new Map();
  for (const e of H.events.slice(mark2).filter((x) => x.ev === "result")) by.set(e.id, e);
  const tooMany = ids2.filter((id) => String(by.get(id)?.outcome ?? "").includes("TooMany")).length;
  const g1 = (await server.admin.game(st.attached.game)).cmds;
  expect("window: all 70 orders ended with one outcome each", ids2.every((id) => by.has(id)), `${by.size} of 70`);
  expect("window: the gateway counted the same TooMany as the bot", g1.rejected.TooMany - g0.rejected.TooMany === tooMany, `bot ${tooMany}, gateway ${g1.rejected.TooMany - g0.rejected.TooMany}`);
  const m = await server.admin.metrics();
  expect("window: the server is alive and the game runs", m.server.games.running === 1, `${m.server.games.running}`);
  if (tooMany === 0) ctx.pending("window: no order was refused with TooMany: the worker drains the 70 as fast as they come; the queue can only be filled in the AI phase (not holdable in the test world yet)");
  else expect("window: the excess over 64 queued orders is Rejected(TooMany)", tooMany <= 6, `${tooMany}`);

  return { numbers: { rtt_set_ms: rttSet, rtt_measured_ms: rtt, orders: count, send_ms: Math.round(tSent), all_answers_ms: Math.round(total), slowest_ms: worst, bound_ms: bound, window_too_many: tooMany } };
}
