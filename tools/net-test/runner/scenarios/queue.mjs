// `queue` (N4.11, net-test 4.5): orders given while the AI plays. Duel, one human bot and one AI, the `window` knob
// (70 orders in the air at once; the worker holds 64 and the rest is `Rejected { TooMany }`).
// Checked here:
//   - every one of the 70 orders ends with exactly one final outcome (`Queued` is not final);
//   - the gateway counted the same `TooMany` as the bot (`cmds.rejected.TooMany`), and the server is alive.
//   - the AI phase held open by `fixture_test.ai_step_delay_ms` (1.5 s per step): H ends its turn, then sends 70 orders;
//     32 (the gateway window per seat; the worker itself would hold 64) get `Queued` and, after the phase, their outcome;
//     the other 38 are `Rejected(TooMany)`, the gateway counts the same.
//     the bot names the turn it sees, so after the phase every queued order is `Rejected(TurnChanged)` (the TurnChanged
//     path is checked this way).
export const meta = {
  id: "queue",
  ac: ["AC-N4.11"],
  needs: ["admin", "hooks"],
  core: ["fixture", "real"],
  serverConfig: { fixtureTest: { ai_step_delay_ms: 1500 } },
  quick: { orders: 70 },
  full: { orders: 70 },
};

export async function run(ctx) {
  const { server, expect, opts } = ctx;
  const count = opts.orders ?? 70;
  const numbers = {};
  const H = ctx.bots.spawn({ nick: "queuebot" });
  await H.connect();
  await H.enter({ role: "host", humans: 1, ai_slots: [1] }, { lobby_timeout_ms: 120000 });
  const st = await H.state();
  const game = st.attached.game;
  const units = st.mine;
  expect("the bot has units to order", units.length > 0, `${units.length} units`);

  const g0 = (await server.admin.game(game)).cmds;
  await H.knob("window", true);
  const mark = H.events.length;
  const ids = [];
  for (let i = 0; i < count; i++) ids.push(await H.cmd({ UnitSkip: { unit: units[i % units.length] } }));
  await Promise.all(ids.map((id) => H.wait({ result_id: id }, 60000)));
  await H.knob("window", false);

  const by = new Map();
  for (const e of H.events.slice(mark)) if (e.ev === "result") by.set(e.id, [...(by.get(e.id) ?? []), String(e.outcome)]);
  const finalOf = (list) => list.filter((o) => o !== "Queued").at(-1) ?? "Queued";
  const oneEach = ids.every((id) => (by.get(id) ?? []).filter((o) => o !== "Queued").length === 1);
  expect(`all ${count} orders end with exactly one final outcome`, oneEach, JSON.stringify(ids.filter((id) => (by.get(id) ?? []).filter((o) => o !== "Queued").length !== 1).map((id) => [id, by.get(id)])));

  const tooMany = ids.filter((id) => finalOf(by.get(id) ?? []).includes("TooMany")).length;
  const queued = ids.filter((id) => (by.get(id) ?? []).includes("Queued")).length;
  const g1 = (await server.admin.game(game)).cmds;
  expect("the gateway counted the same TooMany as the bot", g1.rejected.TooMany - g0.rejected.TooMany === tooMany, `bot ${tooMany}, gateway ${g1.rejected.TooMany - g0.rejected.TooMany}`);
  const m = await server.admin.metrics();
  expect("the server is alive and the game runs", m.server.games.running === 1, `${m.server.games.running} running`);
  numbers.burst = { orders: count, too_many: tooMany, queued };

  // The AI phase held open (ai_step_delay_ms, two steps): H ends its turn and sends the burst inside the window.
  await H.wait({ phase: "Humans" }, 60000);
  const turn0 = (await H.state()).turn;
  const g2 = (await server.admin.game(game)).cmds;
  const endId = await H.cmd({ EndTurn: { force: false } });
  await H.wait({ phase: "Ai" }, 20000);
  const mark2 = H.events.length;
  const ids2 = [];
  await H.knob("window", true);
  for (let i = 0; i < count; i++) ids2.push(await H.cmd({ UnitSkip: { unit: units[i % units.length] } }));
  await Promise.all([endId, ...ids2].map((id) => H.wait({ result_id: id }, 60000)));
  await H.wait({ phase: "Humans" }, 60000);
  await H.knob("window", false);
  const by2 = new Map();
  for (const e of H.events.slice(mark2)) if (e.ev === "result") by2.set(e.id, [...(by2.get(e.id) ?? []), String(e.outcome)]);
  const finals2 = ids2.map((id) => (by2.get(id) ?? []).filter((o) => o !== "Queued"));
  expect("AI phase: every order ends with exactly one final outcome", finals2.every((f) => f.length === 1), JSON.stringify(finals2.filter((f) => f.length !== 1)));
  const queued2 = ids2.filter((id) => (by2.get(id) ?? []).includes("Queued")).length;
  const tooMany2 = finals2.filter((f) => f[0]?.includes("TooMany")).length;
  expect("AI phase: the gateway window (32 per seat) holds 32 and refuses the rest with TooMany", queued2 === 32 && tooMany2 === count - 32, `queued ${queued2}, TooMany ${tooMany2}`);
  expect("AI phase: a queued order gets Queued first and its outcome after", ids2.filter((id) => (by2.get(id) ?? [])[0] === "Queued").every((id) => (by2.get(id) ?? []).length === 2), "a Queued without its outcome");
  const changed = ids2.filter((id) => (by2.get(id) ?? []).includes("Queued") && (by2.get(id) ?? []).at(-1) === "Rejected(TurnChanged)").length;
  expect("AI phase: the queued orders name the old turn and end Rejected(TurnChanged)", changed === queued2, `${changed} of ${queued2}`);
  const g3 = (await server.admin.game(game)).cmds;
  expect("AI phase: the gateway counted the same TooMany", g3.rejected.TooMany - g2.rejected.TooMany === tooMany2, `bot ${tooMany2}, gateway ${g3.rejected.TooMany - g2.rejected.TooMany}`);
  expect("AI phase: the turn went on", (await H.state()).turn > turn0, `turn ${turn0} -> ${(await H.state()).turn}`);
  numbers.ai_window = { queued: queued2, too_many: tooMany2 };

  return { numbers };
}
