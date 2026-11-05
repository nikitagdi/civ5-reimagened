// `m1`, milestone M1 (net-test 4.5, game-server 11 item 1): N bots (2 by default) play one server game on the
// test world for 20 turns. The first bot hosts a room (the last slot is an AI, so the AI phase runs), the others
// join and say they are ready, the host starts. Every bot keeps its own copy of the view from the stream and
// checks every hash. At the end the copies equal what the gateway holds (`seats.<p>.view_hash`), nothing was
// lost, expired or healed by a keyframe, and neither the worker's fog audit nor the bots' own found a leak.
import { asBig, hex } from "../lib/json.mjs";

export const meta = {
  id: "m1",
  milestone: "M1",
  ac: [],
  needs: ["admin"],
  core: ["fixture"],
  quick: { turns: 20, bots: 2 },
  full: { turns: 20, bots: 2 },
};

const sum = (o) => Object.values(o ?? {}).reduce((s, n) => s + Number(n), 0);

export async function run(ctx) {
  const { server, bots, expect, opts } = ctx;
  const n = opts.bots ?? 2;
  const turns = opts.turns ?? 20;
  const names = Array.from({ length: n }, (_, i) => `m1bot${i + 1}`);
  const common = { turns, pace: "burst", persona: "autoplay", lobby_timeout_ms: 60000, turn_timeout_ms: 60000 };

  const running = names.map((nick, i) =>
    bots.play(nick, {
      ...common,
      // The host: a room with the AI in the first slot after the humans; the others join its room.
      lobby: i === 0 ? { role: "host", humans: n, ai_slots: [n] } : { role: "guest", host: names[0] },
    }),
  );
  const budgetMs = 60000 + turns * 10000;
  let timer;
  const finished = await Promise.race([
    Promise.all(running.map((r) => r.done)),
    new Promise((resolve) => {
      timer = setTimeout(() => resolve(null), budgetMs);
    }),
  ]);
  clearTimeout(timer);
  if (!expect("all bots finished in time", finished !== null, `${(budgetMs / 1000).toFixed(0)} s budget`)) return { numbers: {} };

  const results = finished.map((f) => f.result);
  results.forEach((r, i) => {
    expect(`bot ${names[i]} exited cleanly`, finished[i].code === 0, `exit ${finished[i].code}`);
    expect(`bot ${names[i]} played ${turns} turns`, r.play?.done === "turns" && r.turns_played === turns, `done ${r.play?.done}, turns_played ${r.turns_played}`);
    expect(`bot ${names[i]} checked hashes and none differed`, r.hash.checked > 0 && r.hash.mismatch === 0, `checked ${r.hash.checked}, mismatch ${r.hash.mismatch}`);
    expect(`bot ${names[i]} never asked for a keyframe`, sum(r.need_keyframe) === 0, JSON.stringify(r.need_keyframe));
    expect(`bot ${names[i]} found no fog violation`, r.fog.violations === 0, `${r.fog.violations} of ${r.fog.checked} checks${r.fog.examples.length ? ": " + JSON.stringify(r.fog.examples[0]) : ""}`);
    expect(`bot ${names[i]} has no lobby errors`, (r.lobby_errors ?? []).length === 0, (r.lobby_errors ?? []).join("; "));
  });

  // The copies equal what the gateway holds. The gateway publishes a seat's view a moment after it sends it.
  const game = results[0].play.game;
  expect("all bots sit in one game", results.every((r) => r.play.game === game), results.map((r) => r.play.game).join(", "));
  let g = null;
  const agree = (gm) => gm && results.every((r) => gm.seats?.[String(r.player)]?.view_hash !== undefined && asBig(gm.seats[String(r.player)].view_hash) === asBig(r.final.view_hash));
  for (let i = 0; i < 50; i++) {
    g = await server.admin.game(game);
    if (agree(g)) break;
    await ctx.sleep(100);
  }
  expect("the gateway knows the game", !!g, `game ${game}`);
  if (!g) return { numbers: {} };
  results.forEach((r, i) => {
    const gw = g.seats?.[String(r.player)]?.view_hash;
    expect(`bot ${names[i]}'s view hash equals the gateway's`, gw !== undefined && asBig(gw) === asBig(r.final.view_hash), `bot ${hex(r.final.view_hash)}, gateway ${gw === undefined ? "?" : hex(gw)}`);
  });
  const basis = new Set(results.map((r) => String(r.final.basis_hash)));
  expect("all bots hold the same map basis", basis.size === 1, [...basis].join(", "));
  const views = new Set(results.map((r) => String(r.final.view_hash)));
  expect("the views differ (fog): each seat has its own", views.size === results.length, `${views.size} different of ${results.length}`);

  expect("gateway: nothing lost (cmds.lost)", g.cmds.lost === 0, `${g.cmds.lost}`);
  expect("gateway: nothing expired (cmds.expired)", g.cmds.expired === 0, `${g.cmds.expired}`);
  const gwNeed = sum(g.views.need_keyframe);
  expect("gateway: no keyframe was asked for (views.need_keyframe)", gwNeed === 0, JSON.stringify(g.views.need_keyframe));
  expect("gateway: no wantfull and no worker hash failure", g.views.wantfull === 0 && g.views.hash_fail_worker === 0, `wantfull ${g.views.wantfull}, hash_fail_worker ${g.views.hash_fail_worker}`);
  expect("gateway: fog_violations is 0", g.fog_violations === 0, `${g.fog_violations}`);
  expect("gateway: the worker never crashed", g.worker.crashes === 0, `${g.worker.crashes}`);
  expect("gateway: the orders were applied", g.cmds.applied >= n * turns, `applied ${g.cmds.applied}, at least ${n * turns} (one EndTurn per bot per turn)`);
  expect("gateway: the game is running at turn " + (turns + 1), g.status === "Running" && g.turn >= turns + 1, `${g.status}, turn ${g.turn}`);

  const metrics = await server.admin.metrics();
  expect("gateway: one game running", metrics.server.games.running === 1, `${metrics.server.games.running}`);
  expect("gateway: no session was lost by timeout", metrics.server.sessions.lost_timeout_total === 0, `${metrics.server.sessions.lost_timeout_total}`);

  const kb = (b) => Math.round((b / 1024) * 10) / 10;
  return {
    numbers: {
      game,
      turns,
      gateway_cmds: g.cmds,
      gateway_views: { keyframes: g.views.keyframes, diffs: g.views.diffs, basis_sent: g.views.basis_sent, need_keyframe: g.views.need_keyframe },
      bots: results.map((r) => ({
        nick: r.nick,
        player: r.player,
        view_hash: hex(r.final.view_hash),
        orders: r.cmds.unique,
        applied: r.cmds.applied,
        rejected: r.cmds.rejected,
        latency_ms: { p50: r.latency_ms.p50, p95: r.latency_ms.p95 },
        rtt_ms_p50: r.rtt_median,
        down_kb: kb(r.bytes.down_body),
        keyframe_max_kb: kb(r.bytes.keyframes.max_bytes),
        basis_kb: kb(r.bytes.basis.max_bytes),
        queries_sent: r.queries_sent,
        contexts_used: r.contexts_used,
        hashes_checked: r.hash.checked,
        fog_checked: r.fog.checked,
      })),
    },
  };
}
