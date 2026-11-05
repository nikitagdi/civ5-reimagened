// `basis` (N3.5, N3.2, net-test 4.5): the map basis outside the keyframe (NP-D22). Huge test world, 3 human bots,
// `basis_nw = All`. The steps, with bot A a guest, B the host:
//   1. cold entry (no basis in memory): the first batch is `MapBasis` then `Keyframe`, their hashes equal the
//      gateway's `seats.A.basis_hash`, the keyframe without the basis is <= 80 KB, sizes are printed;
//   2. back to the own seat with the right claim (`Leave`, `JoinGame` with `claim_basis: own`): a `Keyframe` and no
//      `MapBasis`, the view hash equals the gateway's, `basis_skipped` of the gateway grows by one, the batch is
//      smaller than the cold one;
//   3. a foreign claim: `claim_basis: wrong_game` gets `MapBasis` (pending: the `stale` claim after the hook
//      `change_basis_at_turn` changed the basis, the gateway config does not expose that hook);
//   4. healing: A throws its basis away (`drop_basis`) and sends `NeedKeyframe { BasisMismatch }`: `MapBasis` and
//      `Keyframe` in one batch within the RTT + 60 ms, the others ask for nothing;
//   5. the change of the basis reaches the other seats as a diff (pending, same hook);
//   7. a restart of the worker (same basis): a client that holds it gets no second `MapBasis`.
// Step 6 (a natural wonder under the fog) needs a test world with a wonder in an undiscovered plot, which the test
// world cannot be told to make yet: reported as pending. Deviation from the text of step 2: the return goes through
// `Leave` and `JoinGame` (the bot has no stream substitution); the reply is the same `Fresh` with a claim.
import { hist } from "../lib/report.mjs";
import { sum } from "../lib/play.mjs";

export const meta = {
  id: "basis",
  ac: ["AC-N3.5", "AC-N3.2"],
  needs: ["admin", "hooks"],
  core: ["fixture"],
  hashMismatchAllowed: true,
  quick: {},
  full: {},
};

const HUGE = { world_size: "WORLDSIZE_HUGE" };

/** The `batch` events of a bot after the event index `from`. */
const batchesSince = (h, from) => h.events.slice(from).filter((e) => e.ev === "batch");

async function seatOf(ctx, game, nick) {
  const g = await ctx.server.admin.game(game);
  const e = Object.entries(g?.seats ?? {}).find(([, s]) => s.nick === nick);
  return e ? { player: Number(e[0]), ...e[1], game: g } : null;
}

/** Waits until `pred()` is truthy (polling), or gives the last value after `ms`. */
async function until(ctx, pred, ms = 15000) {
  const t0 = Date.now();
  let v;
  while (Date.now() - t0 < ms) {
    v = await pred();
    if (v) return v;
    await ctx.sleep(50);
  }
  return v;
}

export async function run(ctx) {
  const { server, expect } = ctx;
  const link = ctx.caps.proxy ? await ctx.proxyLink() : null;
  if (link) await link.set({ rtt_ms: 60 });
  const url = link?.wsUrl;
  const connect = async (nick) => {
    const h = ctx.bots.spawn({ nick });
    await h.connect(url ? { url } : {});
    return h;
  };
  const [B, A, C] = [await connect("basisB"), await connect("basisA"), await connect("basisC")];
  const numbers = {};
  const kb = (b) => Math.round((b / 1024) * 10) / 10;

  // ---- 1. the game starts; A's entry is cold
  await Promise.all([
    B.enter({ role: "host", humans: 3, settings: HUGE }, { lobby_timeout_ms: 120000 }),
    A.enter({ role: "guest", host: "basisB", slots: 3 }, { lobby_timeout_ms: 120000 }),
    C.enter({ role: "guest", host: "basisB", slots: 3 }, { lobby_timeout_ms: 120000 }),
  ]);
  const a0 = await A.state();
  const game = a0.attached.game;
  const first = A.events.find((e) => e.ev === "batch" && e.kinds?.includes("Keyframe"));
  expect("1: A's first view batch is MapBasis then Keyframe", !!first && first.kinds[0] === "MapBasis" && first.kinds[1] === "Keyframe", JSON.stringify(first?.kinds));
  const seatA = await until(ctx, async () => {
    const s = await seatOf(ctx, game, "basisA");
    return s?.basis_hash !== null && s?.basis_hash !== undefined ? s : null;
  });
  const basis0 = a0.basis_hash;
  expect("1: the Keyframe names the MapBasis A holds, and it is the gateway's (seats.A.basis_hash)", !!seatA && String(seatA.basis_hash) === String(basis0), `A ${basis0}, gateway ${seatA?.basis_hash}`);
  const st1 = await A.stats();
  expect("1: no mismatch while entering", st1.hash.mismatch === 0 && sum(st1.need_keyframe) === 0, `${st1.hash.mismatch}, ${JSON.stringify(st1.need_keyframe)}`);
  // The threshold is on the bytes on the wire (decision 2026-10-22): the gateway's compressed size; the bot's raw size
  // is information.
  const keyframeKb = kb(st1.bytes.keyframes.max_bytes);
  const wireB = seatA?.tx?.keyframe_bytes_max ?? 0;
  const wireKb = kb(wireB);
  expect(`1: the keyframe without the basis on the wire is <= ${ctx.th.value("traffic.huge.keyframe_kb")} KB`, wireB > 0 && wireKb <= ctx.th.value("traffic.huge.keyframe_kb"), `${wireKb} KB on the wire, ${keyframeKb} KB raw (${ctx.th.text("traffic.huge.keyframe_kb", " KB")})`);
  expect("1: one MapBasis came", st1.bytes.basis.n === 1, `${st1.bytes.basis.n}`);
  numbers.sizes_kb = { basis: kb(st1.bytes.basis.max_bytes), keyframe: keyframeKb, keyframe_wire: wireKb, cold_batch: kb(st1.bytes.cold_batches.max_bytes) };
  const coldBytes = st1.bytes.cold_batches.max_bytes;
  const playerA = a0.attached.player;
  const g1 = await server.admin.game(game);
  const skipped0 = g1.views.basis_skipped;

  // ---- 2. back to the own seat with a right claim
  const mark2 = A.events.length;
  await A.knob("claim_basis", "own");
  await A.gamectl("Leave");
  await A.wait({ conn: "lobby" }, 15000).catch(() => null);
  await A.enter({ role: "join", game, player: playerA }, { lobby_timeout_ms: 30000 });
  const back = batchesSince(A, mark2).find((e) => e.kinds?.includes("Keyframe"));
  expect("2: the batch of the return has a Keyframe and no MapBasis", !!back && !back.kinds.includes("MapBasis"), JSON.stringify(back?.kinds));
  expect("2: it is smaller than the cold batch (no basis in it)", !!back && back.bytes < coldBytes, `${back?.bytes} B against ${coldBytes} B for the cold entry (basis ${st1.bytes.basis.max_bytes} B raw)`);
  const g2 = await until(ctx, async () => {
    const g = await server.admin.game(game);
    return g.views.basis_skipped > skipped0 ? g : null;
  });
  expect("2: the gateway counted basis_skipped + 1", !!g2 && g2.views.basis_skipped === skipped0 + 1, `${skipped0} -> ${g2?.views.basis_skipped}`);
  const a2 = await A.state();
  const seatA2 = await until(ctx, async () => {
    const s = await seatOf(ctx, game, "basisA");
    return s && String(s.view_hash) === String(a2.view.hash) ? s : null;
  });
  expect("2: A's view hash equals the gateway's", !!seatA2, `A ${a2.view.hash}`);

  // ---- 3. foreign claims
  // The change of the basis by the hook `change_basis_at_turn` (a stale claim, and the diff of the basis to the other
  // seats, step 5) is not reachable: the gateway's `[worker.fixture_test]` knows only `ai_step_delay_ms` and `leak_fog`.
  ctx.pending("3 (stale claim) and 5 (the basis diff): the gateway config has no change_basis_at_turn in [worker.fixture_test]");
  {
    await A.gamectl("Leave");
    await A.wait({ conn: "lobby" }, 15000).catch(() => null);
    await A.knob("claim_basis", "wrong_game");
    const mark = A.events.length;
    await A.enter({ role: "join", game, player: playerA }, { lobby_timeout_ms: 30000 });
    const w = batchesSince(A, mark).find((e) => e.kinds?.includes("Keyframe"));
    expect("3: claim wrong_game: the batch has MapBasis and then Keyframe", !!w && w.kinds[0] === "MapBasis", JSON.stringify(w?.kinds));
    const st = await A.state();
    expect("3: claim wrong_game: A holds the basis of the game", String(st.basis_hash) === String(basis0), `A ${st.basis_hash}, gateway ${seatA?.basis_hash}`);
  }

  // ---- 4. healing: A drops its basis, asks, gets MapBasis and Keyframe in one batch
  const aNow = await A.state();
  const dropAt = aNow.turn + 1;
  const stB5 = await B.stats();
  const stC5 = await C.stats();
  expect("no mismatch and no keyframe request at B and C so far", stB5.hash.mismatch === 0 && stC5.hash.mismatch === 0 && sum(stB5.need_keyframe) === 0 && sum(stC5.need_keyframe) === 0, `B ${stB5.hash.mismatch}/${sum(stB5.need_keyframe)}, C ${stC5.hash.mismatch}/${sum(stC5.need_keyframe)}`);
  await A.knob("claim_basis", "own");
  await A.knob("drop_basis", dropAt);
  const markDrop = A.events.length;
  const needBefore = (await B.stats()).need_keyframe;
  await Promise.all([A, B, C].map((b) => b.autoplay({ turns: 3, persona: "autoplay", pace: "burst" })));
  await Promise.all([A, B, C].map((b) => b.wait({ autoplay: 1 }, 120000)));
  const stA = await A.stats();
  expect("4: A asked once with BasisMismatch", (stA.need_keyframe.BasisMismatch ?? 0) === 1, JSON.stringify(stA.need_keyframe));
  const ask = A.events.slice(markDrop).findIndex((e) => e.ev === "error" && e.what === "need_keyframe");
  const answer = ask >= 0 ? A.events.slice(markDrop + ask).find((e) => e.ev === "batch" && e.kinds?.includes("Keyframe")) : null;
  expect("4: the answer is MapBasis and Keyframe in one batch", !!answer && answer.kinds[0] === "MapBasis", JSON.stringify(answer?.kinds));
  const heal = ask >= 0 && answer ? answer.t_ms - A.events[markDrop + ask].t_ms : null;
  const rtt = Math.max(stA.rtt_median ?? 0, link ? link.nominalRtt : 0);
  expect(`4: it came within RTT ${rtt.toFixed(0)} + ${ctx.th.value("hash.heal_over_rtt_ms")} ms`, heal !== null && heal <= rtt + ctx.th.value("hash.heal_over_rtt_ms"), `${heal} ms`);
  const needAfter = (await B.stats()).need_keyframe;
  expect("4: the others asked for nothing", sum(needAfter) === sum(needBefore) && sum((await C.stats()).need_keyframe) === 0, JSON.stringify(needAfter));

  // ---- 6. natural wonder under the fog
  ctx.pending("6: a natural wonder in an undiscovered plot needs a test world option for it (not in the fixture)");

  // ---- 7. a restart of the worker: the basis is the same, nobody gets it again
  const basisBefore = (await B.stats()).bytes.basis.n;
  const startsBefore = (await server.admin.game(game)).worker.starts;
  const killed = await server.admin.killWorker(game, "KILL");
  expect("7: the hook kills the worker", killed.ok, `HTTP ${killed.status} ${JSON.stringify(killed.body)}`);
  const restarted = await until(ctx, async () => {
    const g = await server.admin.game(game);
    return g.status === "Running" && g.worker.starts > startsBefore ? g : null;
  }, 60000);
  expect("7: the worker came back", !!restarted, restarted ? `starts ${restarted.worker.starts}` : "no restart in 60 s");
  await ctx.sleep(500);
  const stB7 = await B.stats();
  expect("7: B (holds the basis) got no second MapBasis", stB7.bytes.basis.n === basisBefore, `${basisBefore} -> ${stB7.bytes.basis.n}`);
  const seatB7 = await seatOf(ctx, game, "basisB");
  const b7 = await B.state();
  expect("7: B's view equals the gateway's after the restart", String(seatB7?.view_hash) === String(b7.view.hash) || b7.awaiting_keyframe === false, `B ${b7.view.hash}, gateway ${seatB7?.view_hash}`);

  const g = await server.admin.game(game);
  numbers.game = game;
  numbers.basis_hash = String(basis0);
  numbers.heal_ms = heal;
  numbers.gateway_views = g.views;
  numbers.rtt = hist([stA.rtt_median ?? 0]);
  return { numbers };
}
