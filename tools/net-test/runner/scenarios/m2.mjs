// `m2`, milestone M2 (net-test 4.5, game-host-mp 11 item 2): the same as `m1` on the real core. Two humans and two
// AIs play 50 turns on the smallest map of the server's list that holds four (Duel on the test world; the AI phase is
// real); the turns count from the first view (the test world starts at turn 1, the real core at 0). Then the game is saved and stopped (the seats stay bound to the
// nicks, game-server 4.8), the gateway is killed and started again (a new process), the humans log in with their
// tokens: the game wakes from the save in a new worker, they get their seats back and play 5 more turns. Run it as `cargo xtask nettest --wsl --core real --scenario m2` (the real core is Linux only). With
// `--core fixture` the same steps run on the test world, which checks the scenario itself.
import { asBig, hex } from "../lib/json.mjs";

export const meta = {
  id: "m2",
  milestone: "M2",
  ac: [],
  needs: ["admin"],
  core: ["real", "fixture"],
  quick: { turns: 10, post_turns: 3 },
  full: { turns: 50, post_turns: 5 },
};

const sum = (o) => Object.values(o ?? {}).reduce((s, n) => s + Number(n), 0);
const N = 2;

/** Waits for all the missions of a phase; `null` on the time budget. */
async function finish(running, budgetMs) {
  let timer;
  const r = await Promise.race([
    Promise.all(running.map((x) => x.done)),
    new Promise((resolve) => {
      timer = setTimeout(() => resolve(null), budgetMs);
    }),
  ]);
  clearTimeout(timer);
  return r;
}

function botChecks(expect, names, finished, turns, tag) {
  const results = finished.map((f) => f.result);
  results.forEach((r, i) => {
    const who = `${tag}: bot ${names[i]}`;
    expect(`${who} exited cleanly`, finished[i].code === 0, `exit ${finished[i].code}`);
    expect(`${who} played ${turns} turns`, r.play?.done === "turns" && r.turns_played === turns, `done ${r.play?.done}, turns_played ${r.turns_played}`);
    expect(`${who} checked hashes and none differed`, r.hash.checked > 0 && r.hash.mismatch === 0, `checked ${r.hash.checked}, mismatch ${r.hash.mismatch}`);
    expect(`${who} never asked for a keyframe`, sum(r.need_keyframe) === 0, JSON.stringify(r.need_keyframe));
    expect(`${who} found no fog violation`, r.fog.violations === 0, `${r.fog.violations} of ${r.fog.checked} checks`);
    expect(`${who} has no lobby errors`, (r.lobby_errors ?? []).length === 0, (r.lobby_errors ?? []).join("; "));
  });
  return results;
}

async function gameSeats(server, game) {
  for (let i = 0; i < 50; i++) {
    const g = await server.admin.game(game);
    if (g) return g;
    await new Promise((r) => setTimeout(r, 100));
  }
  return null;
}

export async function run(ctx) {
  const { server, bots, expect, opts } = ctx;
  const turns = opts.turns ?? 50;
  const post = opts.post_turns ?? 5;
  const names = Array.from({ length: N }, (_, i) => `m2bot${i + 1}`);
  const numbers = {};
  ctx.restarts = 1; // the runner's "not restarted" check knows about the one restart here

  // ---- the map from the server's pick lists (`Describe`): the smallest size that holds two humans and two AIs
  // (Duel on the test world; the real core's Duel holds two players), else the largest size, with what AIs fit.
  const probe = ctx.bots.spawn({ nick: "m2probe" });
  await probe.connect();
  await probe.wait({ conn: "lobby" }, 20000);
  const sizes = (await probe.state()).world_sizes ?? [];
  await probe.stop();
  if (!expect("the server lists its map sizes", sizes.length > 0, JSON.stringify(sizes))) return { numbers };
  const size = sizes.find((s) => s.max_players >= N + 2) ?? sizes.reduce((a, b) => (b.max_players > a.max_players ? b : a));
  const aiSlots = Array.from({ length: Math.max(0, Math.min(2, size.max_players - N)) }, (_, i) => N + i);
  numbers.map = { world_size: size.id, max_players: size.max_players, default_players: size.default_players, ai: aiSlots.length };

  // ---- phase A: two humans, the AIs, `turns` turns on the real AI.
  const common = { turns, pace: "burst", persona: "autoplay", lobby_timeout_ms: 120000, turn_timeout_ms: 120000 };
  const runningA = names.map((nick, i) =>
    bots.play(nick, {
      ...common,
      lobby: i === 0
        ? { role: "host", humans: N, ai_slots: aiSlots, settings: { world_size: size.id } }
        : { role: "guest", host: names[0], slots: N + aiSlots.length },
    }),
  );
  const finishedA = await finish(runningA, 120000 + turns * 20000);
  if (!expect("A: all bots finished in time", finishedA !== null, `${(120000 + turns * 20000) / 1000} s budget`)) return { numbers };
  const resA = botChecks(expect, names, finishedA, turns, "A");
  const game = resA[0].play.game;
  expect("A: all bots sit in one game", resA.every((r) => r.play.game === game), resA.map((r) => r.play.game).join(", "));

  // The gateway agrees with the bots.
  let g = null;
  const agree = (gm) => gm && resA.every((r) => gm.seats?.[String(r.player)]?.view_hash !== undefined && asBig(gm.seats[String(r.player)].view_hash) === asBig(r.final.view_hash));
  for (let i = 0; i < 50; i++) {
    g = await gameSeats(server, game);
    if (agree(g)) break;
    await ctx.sleep(100);
  }
  if (!expect("A: the gateway knows the game", !!g, `game ${game}`)) return { numbers };
  resA.forEach((r, i) => {
    const gw = g.seats?.[String(r.player)]?.view_hash;
    expect(`A: bot ${names[i]}'s view hash equals the gateway's`, gw !== undefined && asBig(gw) === asBig(r.final.view_hash), `bot ${hex(r.final.view_hash)}, gateway ${gw === undefined ? "?" : hex(gw)}`);
  });
  // The test world starts at turn 1, the real core at turn 0: the turns count from the first view.
  const firstTurn = resA[0].first_view?.turn ?? 1;
  expect(`A: the AI phase ran every turn: the game is at turn ${firstTurn + turns}`, g.status === "Running" && g.turn >= firstTurn + turns, `${g.status}, turn ${g.turn} (first view at turn ${firstTurn})`);
  expect("A: nothing lost, nothing expired", g.cmds.lost === 0 && g.cmds.expired === 0, `lost ${g.cmds.lost}, expired ${g.cmds.expired}`);
  expect("A: no keyframe asked for, no hash failure of the worker", sum(g.views.need_keyframe) === 0 && g.views.hash_fail_worker === 0, JSON.stringify(g.views.need_keyframe));
  expect("A: fog_violations is 0", g.fog_violations === 0, `${g.fog_violations}`);
  expect("A: the worker never crashed", g.worker.crashes === 0, `${g.worker.crashes}`);
  expect("A: the orders were applied", g.cmds.applied >= N * turns, `applied ${g.cmds.applied}, at least ${N * turns}`);
  const lastTurn = g.turn;
  const hashesBefore = Object.fromEntries(resA.map((r) => [r.nick, String(r.final.view_hash)]));
  numbers.phase_a = { game, turns, gateway_cmds: g.cmds, step_ms: g.hist?.step_ms ?? null, ai_phase_ms: g.hist?.ai_phase_ms ?? null };

  // ---- save, stop, a new process.
  const saved = await server.admin.save(game);
  expect("save: the admin API takes the save", saved.ok, `HTTP ${saved.status} ${JSON.stringify(saved.body)}`);
  let onDisk = null;
  for (let i = 0; i < 100 && !onDisk; i++) {
    const detail = await server.admin.get(`/admin/games/${game}`).catch(() => null);
    onDisk = detail?.saves?.find((s) => s.turn === lastTurn) ?? null;
    if (!onDisk) await ctx.sleep(100);
  }
  expect(`save: a save of turn ${lastTurn} is on disk`, !!onDisk, onDisk ? JSON.stringify(onDisk) : "no save of that turn after 10 s");
  const stopped = await server.admin.stop(game);
  expect("stop: the admin API stops the game", stopped.ok, `HTTP ${stopped.status} ${JSON.stringify(stopped.body)}`);
  const oldPid = g.worker?.pid ?? null;
  let down = false;
  for (let i = 0; i < 100 && !down; i++) {
    const m = await server.admin.metrics().catch(() => null);
    down = !!m && (m.server?.games?.running ?? 1) === 0;
    if (!down) await ctx.sleep(100);
  }
  expect("stop: no game is running", down, "games.running did not fall to 0 in 10 s");
  const gatewayPid = server.pid;
  await server.restart();
  expect("restart: the gateway is a new process", server.pid !== gatewayPid, `pid ${gatewayPid} -> ${server.pid}`);
  const after = await server.admin.metrics();
  expect("restart: the new process has no running game", after.server.games.running === 0, `${after.server.games.running}`);

  // ---- phase B: the same nicks log in with their tokens (no lobby part): the game wakes, `post` turns.
  const runningB = names.map((nick) => bots.play(nick, { ...common, turns: post }));
  const finishedB = await finish(runningB, 120000 + post * 20000);
  if (!expect("B: all bots finished in time", finishedB !== null, `${(120000 + post * 20000) / 1000} s budget`)) return { numbers };
  const resB = botChecks(expect, names, finishedB, post, "B");
  const game2 = resB[0].play.game;
  expect("B: all bots sit in the game they left", resB.every((r) => r.play.game === game), resB.map((r) => r.play.game).join(", "));
  resB.forEach((r, i) => {
    const first = r.first_view;
    expect(`B: bot ${names[i]}'s first view is the turn of the save`, first?.turn === lastTurn, `view turn ${first?.turn}, save turn ${lastTurn}`);
    expect(`B: bot ${names[i]}'s first view equals the last one before the stop`, first && String(first.view_hash) === hashesBefore[r.nick], `before ${hex(BigInt(hashesBefore[r.nick]))}, after ${first ? hex(first.view_hash) : "?"}`);
  });
  const g2 = await gameSeats(server, game2);
  if (!expect("B: the gateway knows the loaded game", !!g2, `game ${game2}`)) return { numbers };
  expect("B: the woken game runs at turn " + (lastTurn + post), g2.status === "Running" && g2.turn >= lastTurn + post, `${g2.status}, turn ${g2.turn}`);
  expect("B: nothing lost, nothing expired", g2.cmds.lost === 0 && g2.cmds.expired === 0, `lost ${g2.cmds.lost}, expired ${g2.cmds.expired}`);
  expect("B: fog_violations is 0 and the worker never crashed", g2.fog_violations === 0 && g2.worker.crashes === 0, `fog ${g2.fog_violations}, crashes ${g2.worker.crashes}`);
  expect("B: the worker is a new process", oldPid === null || g2.worker?.pid !== oldPid, `pid ${oldPid} -> ${g2.worker?.pid}`);
  numbers.phase_b = { game: game2, turns: post, from_save: onDisk, gateway_cmds: g2.cmds };
  numbers.bots = resB.map((r) => ({ nick: r.nick, player: r.player, view_hash: hex(r.final.view_hash), latency_ms: { p50: r.latency_ms.p50, p95: r.latency_ms.p95 } }));
  return { numbers };
}
