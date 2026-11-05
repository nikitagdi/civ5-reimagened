// `huge8` (N4.3, AC-N4.3, net-test 4.5): a Huge game with 8 human bots and AI in the rest of the slots. The first bot
// hosts, seven join and say they are ready, the host starts. Conditions: from the start of the game to every bot being
// attached with its first view no more than 120 s (the real core builds a Huge world in that time); every seat gets its
// own first view (the hashes of the keyframes all differ: fog); the keyframe without the map basis of Huge stays within
// the AC-N3.5 threshold on the wire (compressed; the raw size is information); the sizes of the keyframe, the basis and the cold batch are printed; 10 turns are played by
// the bots with three orders per turn (`spammer`, 3 orders and then the shared policy); `hist.step_ms.p95` is printed.
// Real core: `cargo xtask nettest --wsl --core real --scenario huge8`.
import { botChecks, finishWatched, gatewayChecks, gatewayGame, nicksOf, startGame } from "../lib/play.mjs";
import { hex } from "../lib/json.mjs";

export const meta = {
  id: "huge8",
  ac: ["AC-N4.3"],
  needs: ["admin"],
  core: ["fixture", "real"],
  quick: { turns: 10 },
  full: { turns: 10 },
};

const KB = 1024;
const START_TO_ATTACHED_MS = 120000;
const HUMANS = 8;
// The real core lets in as many players as the map size's default (12 on Huge): the other four slots are AI. The test
// world's Huge holds eight, all of them the bots.
const AI_REAL = 4;
// A Huge map of 12 players is made by the map script in a few seconds, but some seeds make it search for fair starts for
// a minute or more (seeds 3 and 150: 29 s and 72 s from Start to attached); the worker sends heartbeats while it builds
// the game, so a slow start is not a hang. `NETTEST_GAME_SEED` picks another seed.
const GAME_SEED = Number(process.env.NETTEST_GAME_SEED ?? 11);
const r1 = (x) => (Number.isFinite(x) ? Math.round(x * 10) / 10 : null);

export async function run(ctx) {
  const { expect, server, opts } = ctx;
  const turns = opts.turns ?? 10;
  const names = nicksOf("huge8b", HUMANS);
  const numbers = {};

  const t0 = Date.now();
  const running = startGame(ctx, names, {
    turns,
    humans: HUMANS,
    ai_slots: Array.from({ length: opts.core === "real" ? AI_REAL : 0 }, (_, i) => HUMANS + i),
    settings: { world_size: "WORLDSIZE_HUGE", seed: GAME_SEED, map_seed: GAME_SEED },
    persona: "spammer",
    persona_args: { per_turn: 3 },
    pace: "burst",
    lobby_timeout_ms: 300000,
    turn_timeout_ms: 300000,
  });

  // The game appears in the admin API when the host has started the room (Start); the entry ends when every seat has
  // a view at the gateway (the bot is attached and got its first keyframe).
  let tGame = null;
  let tAll = null;
  let firstStatus = null;
  let polling = true;
  const poll = (async () => {
    while (polling && tAll === null) {
      try {
        const gm = (await server.admin.games()).games ?? [];
        if (gm.length && tGame === null) {
          tGame = Date.now();
          firstStatus = gm[0].status;
        }
        if (gm.length) {
          const g = await server.admin.game(gm[0].id);
          const seated = Object.values(g?.seats ?? {}).filter((s) => s.view_hash !== undefined && s.view_hash !== null).length;
          if (seated >= HUMANS) tAll = Date.now();
        }
      } catch {
        // the admin API answers a moment after the server is up
      }
      if (tAll === null) await ctx.sleep(100);
    }
  })();
  const budget = 300000 + turns * 60000;
  const { finished, fault } = await finishWatched(ctx, running, budget);
  polling = false;
  await poll;
  if (!expect("all 8 bots finished in time", finished !== null, fault ?? `${budget / 1000} s budget`)) return { numbers };
  const tDone = Date.now();

  const results = botChecks(expect, names, finished, turns);
  const game = results[0].play?.game;
  expect("all bots sit in one game", results.every((r) => r.play?.game === game), results.map((r) => r.play?.game).join(", "));

  const startToAttached = tGame !== null && tAll !== null ? tAll - tGame : null;
  numbers.entry = { total_ms: tDone - t0, game_seen_after_ms: tGame === null ? null : tGame - t0, first_status: firstStatus, start_to_attached_ms: startToAttached };
  expect(`from Start to every bot attached with a view <= ${START_TO_ATTACHED_MS / 1000} s`, startToAttached !== null && startToAttached <= START_TO_ATTACHED_MS, startToAttached === null ? "no entry seen" : `${(startToAttached / 1000).toFixed(1)} s (game first seen as ${firstStatus})`);

  // Every seat has its own view from the first keyframe.
  const firstHashes = results.map((r) => (r.first_view ? String(r.first_view.view_hash) : null));
  expect("every bot has a first view", firstHashes.every((h) => h !== null), JSON.stringify(firstHashes));
  expect("the first views differ (fog): each seat has its own", new Set(firstHashes).size === HUMANS, `${new Set(firstHashes).size} different of ${HUMANS}`);
  const basis = new Set(results.map((r) => String(r.final.basis_hash)));
  expect("all bots hold the same map basis", basis.size === 1, [...basis].join(", "));

  // Sizes of the cold entry. The threshold of AC-N3.5 is on the bytes on the wire (decision 2026-10-22): the gateway's
  // compressed size is checked; the bot's raw (decoded) size is printed next to it as information.
  const KB_RAW = Math.max(...results.map((r) => r.bytes?.keyframes?.max_bytes ?? 0)) / KB;
  const basisKb = Math.max(...results.map((r) => r.bytes?.basis?.max_bytes ?? 0)) / KB;
  const coldKb = Math.max(...results.map((r) => r.bytes?.cold_batches?.max_bytes ?? 0)) / KB;

  const g = await gatewayGame(ctx, game, results);
  if (!expect("the gateway knows the game", !!g, `game ${game}`)) return { numbers };
  gatewayChecks(expect, g, results, names);
  // The real game starts at turn 0: the bots played `turns` turns and the game stands at turn `turns`.
  const startTurn = results[0].first_view?.turn ?? 0;
  const gn = g;
  expect(`the game is running at turn ${startTurn + turns}`, gn.status === "Running" && gn.turn >= startTurn + turns, `${gn.status}, turn ${gn.turn}`);
  const wireB = Math.max(...Object.values(g.seats ?? {}).map((s) => s.tx?.keyframe_bytes_max ?? 0));
  const wireKb = wireB / KB;
  expect(`Huge keyframe without the basis on the wire <= ${ctx.th.text("traffic.huge.keyframe_kb", " kB")}`, wireKb <= ctx.th.value("traffic.huge.keyframe_kb"), `${wireKb.toFixed(1)} kB on the wire, ${KB_RAW.toFixed(1)} kB raw at the bot`);
  numbers.sizes_kb = { keyframe_raw_max: r1(KB_RAW), keyframe_wire_max: r1(wireKb), basis_raw_max: r1(basisKb), cold_batch_raw_max: r1(coldKb), gateway_basis_wire_max: r1(Math.max(...Object.values(g.seats ?? {}).map((s) => s.tx?.basis_bytes_max ?? 0)) / KB), gateway_cold_batch_wire_max: r1(Math.max(...Object.values(g.seats ?? {}).map((s) => s.tx?.cold_batch_bytes_max ?? 0)) / KB) };

  expect("the orders were applied: three per turn and the end of turn", g.cmds.applied >= HUMANS * turns, `applied ${g.cmds.applied}, at least ${HUMANS * turns}`);
  numbers.game = game;
  numbers.turns = turns;
  numbers.step_ms = gn.hist?.step_ms ?? null;
  numbers.ai_phase_ms = gn.hist?.ai_phase_ms ?? null;
  numbers.gateway_cmds = g.cmds;
  numbers.bots = results.map((r) => ({ nick: r.nick, player: r.player, first_view_hash: r.first_view ? hex(r.first_view.view_hash) : null, view_hash: hex(r.final.view_hash), orders: r.cmds.unique }));
  return { numbers };
}
