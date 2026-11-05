// `idem` (N1.5, net-test 4.5): an order is never applied twice and never lost without a word. One human bot and one
// AI, driven order by order:
//   (a) an order sent twice in a row: one outcome, `cmds.applied` of the gateway +1, the repeat answered or dropped;
//   (b) an order sent and the link dropped at once; after the resume the bot sends it again: one outcome, applied +1;
//   (d) 70 orders in the air at once (the `window` knob): the server lives, every order ends with one outcome. That
//       64 of them wait in the worker's queue and the rest is `Rejected { TooMany }` needs the AI phase slowed by
//       `fixture_test.ai_step_delay_ms`, which the test world does not implement yet (pending);
//   (c') the id of an order from before the ring of 64: `Rejected { Expired }`;
//   (c) orders in flight when the worker is killed (`kill-worker`): each ends with exactly one outcome (applied or
//       `Lost`), nothing stays open, the game goes on.
// In all of them `ledger.double_outcome == 0` (but the one the bot sees by reusing an old id in c') and `ledger.open == 0`
// after the quiet. The check of (c) in the seat's
// journal (the `System` line with the number of rolled-back orders against `saves.last.applied_upto`) is pending.
export const meta = {
  id: "idem",
  ac: ["AC-N1.5"],
  needs: ["admin", "hooks"],
  core: ["fixture", "real"],
  // Reusing the id of an answered order (c') is a second answer for the bot's ledger: that one is expected.
  allowErrors: [/second, different final outcome/],
  allowDoubleOutcome: 1,
  quick: {},
  full: {},
};

/** Final outcome texts by order id from the `result` events of the bot (the last one per id). */
function outcomes(h, fromIndex = 0) {
  const by = new Map();
  for (const e of h.events.slice(fromIndex)) if (e.ev === "result") by.set(e.id, [...(by.get(e.id) ?? []), String(e.outcome)]);
  return by;
}

const finalOf = (list) => list.filter((o) => o !== "Queued").at(-1) ?? "Queued";

export async function run(ctx) {
  const { server, expect } = ctx;
  const H = ctx.bots.spawn({ nick: "idembot" });
  await H.connect();
  await H.enter({ role: "host", humans: 1, ai_slots: [1] }, { lobby_timeout_ms: 120000 });
  const st0 = await H.state();
  const game = st0.attached.game;
  const unit = st0.mine[0];
  const units = st0.mine;
  const skip = (u = unit) => ({ UnitSkip: { unit: u } });
  const gw = async () => (await server.admin.game(game)).cmds;
  const quiet = async (ms = 30000) => {
    const t0 = Date.now();
    for (;;) {
      const s = await H.state();
      if (s.open_orders === 0) return true;
      if (Date.now() - t0 > ms) return false;
      await ctx.sleep(50);
    }
  };
  const numbers = {};

  // ---- (a) the same order twice
  {
    const c0 = await gw();
    await H.knob("dup_cmds", 1);
    const id = await H.cmd(skip());
    await H.wait({ result_id: id }, 15000);
    await H.knob("dup_cmds", 0);
    await ctx.sleep(300);
    const c1 = await gw();
    const out = outcomes(H).get(id) ?? [];
    expect("a: one final outcome for the order sent twice", out.filter((o) => o !== "Queued").length === 1, JSON.stringify(out));
    expect("a: the gateway applied it once (applied + 1)", c1.applied === c0.applied + 1, `${c0.applied} -> ${c1.applied}`);
    const repeats = c1.dup_answered + c1.dup_pending_dropped - c0.dup_answered - c0.dup_pending_dropped;
    expect("a: the repeat was answered or dropped, not applied (dup + 1)", repeats === 1, `dup_answered ${c0.dup_answered} -> ${c1.dup_answered}, dup_pending_dropped ${c0.dup_pending_dropped} -> ${c1.dup_pending_dropped}`);
    numbers.a = { applied: c1.applied - c0.applied, repeats };
  }

  // ---- (b) the link drops at once; after the resume the order goes again
  {
    const c0 = await gw();
    const mark = H.events.length;
    const id = await H.cmd(skip(units[1] ?? unit));
    await H.drop();
    await ctx.sleep(200);
    await H.reconnect(true);
    await H.wait({ conn: "attached" }, 30000);
    await H.wait({ result_id: id }, 30000);
    await quiet();
    await ctx.sleep(200);
    const c1 = await gw();
    const out = outcomes(H, mark).get(id) ?? [];
    expect("b: one final outcome after the drop and the resume", out.filter((o) => o !== "Queued").length === 1, JSON.stringify(out));
    expect("b: the order was applied once (applied + 1)", c1.applied === c0.applied + 1, `${c0.applied} -> ${c1.applied}`);
    numbers.b = { applied: c1.applied - c0.applied, outcomes: out };
  }

  // ---- (d) 70 orders in the air
  {
    await H.knob("window", true);
    const mark = H.events.length;
    const ids = [];
    for (let i = 0; i < 70; i++) ids.push(await H.cmd(skip(units[i % units.length])));
    const calm = await quiet(60000);
    expect("d: every order ended (nothing open after the quiet)", calm, calm ? "quiet" : "orders still open after 60 s");
    const by = outcomes(H, mark);
    const finals = ids.map((id) => finalOf(by.get(id) ?? []));
    expect("d: every order has exactly one final outcome", ids.every((id) => (by.get(id) ?? []).filter((o) => o !== "Queued").length === 1), "an order without one outcome, or with two");
    const queued = ids.filter((id) => (by.get(id) ?? []).includes("Queued")).length;
    const tooMany = finals.filter((f) => f.includes("TooMany")).length;
    const m = await server.admin.metrics();
    expect("d: the server is alive and the game runs", m.server.games.running === 1, `${m.server.games.running} running`);
    await H.knob("window", false);
    numbers.d = { queued, too_many: tooMany };
    ctx.pending("d: 64 Queued and the rest TooMany need the AI phase held open by fixture_test.ai_step_delay_ms, which game-host does not implement (the AI phase lasts 1 ms)");
  }

  // ---- (c') an id from before the ring of 64
  {
    const c0 = await gw();
    await H.wait({ phase: "Humans" }, 60000);
    await H.cmd(skip(), { id: 1 });
    await ctx.sleep(500);
    const c1 = await gw();
    expect("c': an id older than the ring is Rejected(Expired): the gateway counted it", c1.rejected.Expired - c0.rejected.Expired === 1, `${c0.rejected.Expired} -> ${c1.rejected.Expired}`);
  }

  // ---- (c) the worker dies with orders in flight
  {
    await H.wait({ phase: "Humans" }, 60000);
    const startsBefore = (await server.admin.game(game)).worker.starts;
    const mark = H.events.length;
    const ids = [];
    for (let i = 0; i < 12; i++) ids.push(await H.cmd(skip(units[i % units.length])));
    const killed = await server.admin.killWorker(game, "KILL");
    expect("c: the hook kills the worker", killed.ok, `HTTP ${killed.status}`);
    const back = await (async () => {
      const t0 = Date.now();
      while (Date.now() - t0 < 60000) {
        const g = await server.admin.game(game);
        if (g.status === "Running" && g.worker.starts > startsBefore) return g;
        await ctx.sleep(100);
      }
      return null;
    })();
    expect("c: the worker came back", !!back, back ? `starts ${back.worker.starts}` : "no restart in 60 s");
    const calm = await quiet(60000);
    expect("c: nothing stays open after the restart", calm, "orders still open");
    const by = outcomes(H, mark);
    const lost = ids.filter((id) => finalOf(by.get(id) ?? []) === "Lost").length;
    expect("c: every order of the burst has exactly one final outcome (applied, rejected or Lost)", ids.every((id) => (by.get(id) ?? []).filter((o) => o !== "Queued").length === 1), JSON.stringify(ids.map((id) => by.get(id))));
    numbers.c = { sent: ids.length, lost, restarts: back?.worker.starts };
    ctx.pending("c: the System line of the seat's journal (rolled-back orders against saves.last.applied_upto) is not read yet");
  }

  const fin = await H.stats();
  expect("ledger: only the answer to the reused id was a second one, nothing is open", fin.ledger.double_outcome === 1 && fin.ledger.open === 0, JSON.stringify(fin.ledger));
  const g = await server.admin.game(game);
  expect("the game runs on after all of it", g.status === "Running", g.status);
  return { numbers };
}
