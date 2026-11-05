// `join` (N4.5, net-test 4.5): a bot takes an AI seat of a game in progress over a channel of RTT 200.
// Duel, two human bots (A, B) and two AIs; A and B play in the background (`autoplay`), the joining bot C:
//   - joins its AI seat: the time from `JoinGame` to the first view is <= 10 s; the first batch is `MapBasis` then
//     `Keyframe` (a cold entry: C has no basis in memory); the sizes of the basis, the keyframe and the batch are
//     printed (no threshold on them: NP-D22);
//   - the return variant: C leaves (`GameCtl::Leave`), takes its own seat again with `claim_basis: own`: the batch has a
//     `Keyframe` and no `MapBasis`, and it is smaller than the cold batch;
//   - the seat is taken: a second bot on C's seat gets a LobbyError;
//   - a nick that sits in another game (game 2, one human and one AI, hosted by a third bot) cannot take an AI seat
//     of game 2: LobbyError;
//   - the answer pause of the gateway's safe point (`hist.safe_point_wait_ms`) in the join window is no more than the
//     base (its median before the join) + 1 s.
// The AI phase of the test world lasts 2 x 300 ms (`fixture_test.ai_step_delay_ms`), so the join lands in an AI phase
// part of the time; the scenario does not force it (pending). A and B send their orders in burst pace, not one per second: the pause is read
// from the gateway's histogram instead of from their own latencies.
export const meta = {
  id: "join",
  ac: ["AC-N4.5"],
  needs: ["admin", "hooks", "proxy"],
  core: ["fixture", "real"],
  serverConfig: { fixtureTest: { ai_step_delay_ms: 300 } },
  allowErrors: [/^rejected: /],
  quick: { turns: 6 },
  full: { turns: 30 },
};

import { bigintReplacer } from "../lib/json.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function until(pred, ms) {
  const t0 = Date.now();
  for (;;) {
    const v = await pred();
    if (v) return v;
    if (Date.now() - t0 > ms) return v;
    await sleep(50);
  }
}

export async function run(ctx) {
  const { server, expect, opts } = ctx;
  const turns = opts.turns ?? 6;
  const numbers = {};
  const link = await ctx.proxyLink();
  await link.set({ rtt_ms: 200 });

  // ---- the game: two humans and two AIs on Duel (A and B direct; the joining bot C comes over the link)
  const A = ctx.bots.spawn({ nick: "joinA" });
  const B = ctx.bots.spawn({ nick: "joinB" });
  await A.connect();
  await B.connect();
  await Promise.all([
    B.enter({ role: "host", humans: 2, ai_slots: [2, 3] }, { lobby_timeout_ms: 120000 }),
    A.enter({ role: "guest", host: "joinB", slots: 4 }, { lobby_timeout_ms: 120000 }),
  ]);
  const game = (await B.state()).attached.game;
  const turn0 = (await B.state()).turn; // 1 on the test world, 0 on the real core
  const g0 = await server.admin.game(game);
  // The per-game metrics list only the seats that have stats; the games list has every seat of the snapshot (AI too).
  const listed0 = (await server.admin.games()).games.find((x) => x.id === game);
  const aiSeats = (listed0?.seats ?? []).filter((s) => s.control === "Ai").map((s) => s.player);
  expect("the game has two AI seats", aiSeats.length === 2, JSON.stringify((listed0?.seats ?? []).map((s) => [s.player, s.control, s.nick]), bigintReplacer));
  const seat = aiSeats[0];
  const basePause = g0.hist.safe_point_wait_ms;

  await A.autoplay({ turns, persona: "autoplay", pace: "burst" });
  await B.autoplay({ turns, persona: "autoplay", pace: "burst" });
  await B.wait({ turn: 3 }, 120000);

  // ---- C joins its AI seat over RTT 200
  const C = ctx.bots.spawn({ nick: "joinC" });
  await C.connect({ url: link.wsUrl });
  await C.wait({ conn: "lobby" }, 30000);
  const t0 = performance.now();
  await C.enter({ role: "join", game, player: seat }, { lobby_timeout_ms: 60000 });
  const joinMs = performance.now() - t0;
  const turnAtJoin = (await C.state()).turn;
  numbers.join_ms = Math.round(joinMs);
  expect("C is on its AI seat within 10 s at RTT 200", joinMs <= 10000, `${joinMs.toFixed(0)} ms (RTT ${link.nominalRtt} ms)`);
  expect("C is attached to the seat it asked for", (await C.state()).attached?.player === seat, `player ${(await C.state()).attached?.player}`);
  const cold = C.events.find((e) => e.ev === "batch" && e.kinds?.includes("Keyframe"));
  expect("cold entry: the first batch is MapBasis then Keyframe", !!cold && cold.kinds[0] === "MapBasis" && cold.kinds.includes("Keyframe"), JSON.stringify(cold?.kinds));
  const cs = await C.stats();
  numbers.cold = {
    basis_bytes: cs.bytes.basis.max_bytes,
    keyframe_bytes: cs.bytes.keyframes.max_bytes,
    cold_batch_bytes: cs.bytes.cold_batches.max_bytes,
  };
  const gMid = await server.admin.game(game);
  const base = basePause?.p50 ?? 0;
  const windowMax = gMid.hist.safe_point_wait_ms.max;
  numbers.safe_point_wait_ms = { base_p50: base, window_max: windowMax };
  expect("the guests' answer pause in the join window is no more than the base + 1 s", windowMax <= base + 1000, `window max ${windowMax} ms, base p50 ${base} ms`);

  // ---- the return variant: Leave, then the own seat again with the basis in hand
  await C.gamectl("Leave");
  await C.wait({ conn: "lobby" }, 20000);
  await C.knob("claim_basis", "own");
  const mark = C.events.length;
  await C.enter({ role: "join", game, player: seat }, { lobby_timeout_ms: 60000 });
  const back = C.events.slice(mark).find((e) => e.ev === "batch" && e.kinds?.includes("Keyframe"));
  expect("the return with the basis: a Keyframe and no MapBasis", !!back && !back.kinds.includes("MapBasis"), JSON.stringify(back?.kinds));
  expect("the return batch is smaller than the cold one", !!back && back.bytes < cold.bytes, `${back?.bytes} B against ${cold?.bytes} B`);
  numbers.return_batch_bytes = back?.bytes ?? null;

  // ---- negatives: the seat is taken; a nick that sits in another game
  const D = ctx.bots.spawn({ nick: "joinD" });
  await D.connect({ url: link.wsUrl });
  await D.wait({ conn: "lobby" }, 30000);
  const taken = await D.enter({ role: "join", game, player: seat }, { lobby_timeout_ms: 20000 }).then(() => null, (e) => String(e));
  expect("a second player on the taken seat gets a LobbyError", !!taken && /lobby error/.test(taken), taken ?? "entered the seat");

  const X = ctx.bots.spawn({ nick: "joinX" });
  await X.connect();
  await X.enter({ role: "host", humans: 1, ai_slots: [1] }, { lobby_timeout_ms: 120000 });
  const game2 = (await X.state()).attached.game;
  const before = (await A.stats()).lobby_errors.length;
  await A.lobby({ JoinGame: { game: game2, player: 1, basis: null } });
  const refused = await until(async () => (await A.stats()).lobby_errors.length > before, 10000);
  const errs = (await A.stats()).lobby_errors.slice(before);
  expect("a nick that sits in another game cannot take an AI seat of it: LobbyError", !!refused && errs.length === 1, JSON.stringify(errs));

  // ---- the rest of the game: C ends the turns it still owes (its seat is human now), up to the turn A and B ended
  // (The real core's seat has a city and units that want orders before a turn can end: the bot's own policy plays them, up to the turn where A and B stop: 	urn0 + turns.)
  const left = Math.max(1, turn0 + turns - (await C.state()).turn);
  await C.autoplay({ turns: left, persona: "autoplay", pace: "burst" });
  await C.wait({ autoplay: 1 }, 180000).catch(() => null);
  await A.wait({ autoplay: 1 }, 180000).catch(() => null);
  await B.wait({ autoplay: 1 }, 180000).catch(() => null);
  const g = await server.admin.game(game);
  expect("the game runs on after the join", g.status === "Running" && g.turn >= turnAtJoin, `${g.status} turn ${g.turn}`);
  numbers.game = { game, seat, turn_at_join: turnAtJoin, turn_end: g.turn };

  ctx.pending("a join forced into the AI phase: the AI phase is held open 2 x 300 ms in this scenario, but the join time is not synchronised to it");
  return { numbers };
}
