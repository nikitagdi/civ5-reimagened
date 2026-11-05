// `soak` (N6.1, net-test 4.5, 4.7): the endurance run. Six human bots and one AI play `turns` turns (200 in the full
// preset, thresholds `soak.bots`, `soak.turns`) through the proxy, while a seeded chaos (`lib/chaos.mjs`, the same
// schedule for the same `--seed`) changes the channel (delay, jitter, stalls), resets the connections, blackholes and cuts
// one direction, and now and then kills the worker. The run is judged by:
//   - the game finishes: every bot plays all its turns (the ones that were cut off come back and go on);
//   - the stuck-turn detector: the turn counter of the game is sampled every second; a turn that does not move for
//     `stuck_s` outside the time the chaos is known to hold the game up is a failure, with the sample log kept;
//   - the common checks: no order answered twice or left open, no hash mismatch, no fog violation, no ERROR in the log;
//   - leaks: sampled every 5 s, the open sessions, the resident memory of the gateway and the worker (Windows: `tasklist`;
//     Linux: `/proc`), the saves on disk. Checked: at the end no more sessions are open than there are bots, the saves of the
//     ring stay at most 10 + the keep files. The memory trend is reported (first and last third, KiB per turn): BUILD
//     gives no number for it yet, so no bound is checked (pending).
// `history.jsonl` in the output directory gets one line per run: commit, preset, seed, result, the key numbers.
import { appendFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import path from "node:path";
import { savesOnDisk, startChaos, until } from "../lib/chaos.mjs";
import { PROFILES } from "../lib/link.mjs";
import { botChecks, budgetFor, finishAll, gatewayGame, nicksOf, startGame } from "../lib/play.mjs";
import { median, slope, summary } from "../lib/stats.mjs";

export const meta = {
  id: "soak",
  ac: ["AC-N6.1"],
  needs: ["admin", "hooks", "proxy"],
  core: ["fixture", "real"],
  allowErrors: [/^rejected: /, /^order \d+ was lost/, /closed/i, /connect/i, /timeout/i, /reconnect/i],
  // The quick form is for the regression run: a few turns, the same chaos in a shorter rhythm.
  quick: { turns: 20, stuck_s: 30, every_s: [2, 4], kill_every_s: 12 },
  full: { turns: 200, stuck_s: 60, every_s: [3, 9], kill_every_s: 90 },
};

export async function run(ctx) {
  const { server, expect, opts } = ctx;
  const nBots = ctx.opts.bots ?? ctx.th.value("soak.bots");
  const turns = opts.turns;
  if (opts.preset === "full") expect(`the full run is ${ctx.th.value("soak.turns")} turns with ${ctx.th.value("soak.bots")} bots`, turns === ctx.th.value("soak.turns") && nBots === ctx.th.value("soak.bots"), `${nBots} bots, ${turns} turns`);
  if (opts.link === "netem") ctx.pending("netem (real packet loss) path: needs the WSL build with the netem plumbing; this run used the netproxy");
  const link = await ctx.proxyLink();
  await link.set(PROFILES.latency);
  const names = nicksOf("soakbot", nBots);
  const started = Date.now();
  // The rest of the eight slots of the size are AIs: the room starts only when every open slot is taken.
  const aiSlots = Array.from({ length: 8 - nBots }, (_, i) => nBots + i);
  const settings = { world_size: opts.core === "real" ? "WORLDSIZE_STANDARD" : "WORLDSIZE_HUGE", turn_mode: "Simultaneous" };
  const running = startGame(ctx, names, { turns, humans: nBots, ai_slots: aiSlots, settings, knobs: { hold_ms: 15 }, url: link.wsUrl, turn_timeout_ms: 180000, lobby_timeout_ms: 180000 });
  const game = await until(async () => ((await server.admin.games()).games ?? []).find((x) => x.status === "Running")?.id ?? null, 120000, 100);
  if (!expect("the game started", game !== null, "no running game in 120 s")) return { numbers: {} };

  // The chaos starts when every bot has left the lobby for the game (a reset in the lobby phase makes a mission send
  // its lobby commands again to a server that has already seated it: "you are in a game").
  await until(async () => ((await server.admin.game(game))?.turn ?? 0) >= 2 && Object.values((await server.admin.game(game)).seats).filter((x) => x.nick).length === nBots, 60000, 100);

  const chaos = startChaos(ctx, { link, game, seed: ctx.opts.seed, everyS: opts.every_s, killEveryS: opts.kill_every_s });

  // ---- samplers: the stuck-turn detector (1 s) and the leaks (5 s)
  const turnLog = [];
  const stuck = [];
  let lastTurn = -1;
  let lastMoveAt = Date.now();
  const turnTimer = setInterval(async () => {
    try {
      const g = await server.admin.game(game);
      if (!g) return;
      turnLog.push({ t: Date.now() - started, turn: g.turn, status: g.status });
      if (g.turn !== lastTurn) {
        lastTurn = g.turn;
        lastMoveAt = Date.now();
      } else if (g.status === "Running" && Date.now() - lastMoveAt > opts.stuck_s * 1000 && Date.now() > chaos.busyUntil() + 5000 && !stuck.some((s) => s.turn === g.turn)) {
        stuck.push({ turn: g.turn, since_ms: Date.now() - lastMoveAt, at_ms: Date.now() - started, chaos_tail: chaos.log.slice(-4) });
      }
    } catch {
      // the gateway may answer late during a chaos event
    }
  }, 1000);
  const leaks = [];
  const leakTimer = setInterval(async () => {
    try {
      const m = await server.admin.metrics();
      const g = m.games[String(game)];
      const gwRss = m.server.mem.gateway_rss_kb ?? (server.pid ? await ctx.host.rssKb(server.pid) : null);
      const wRss = g?.worker.rss_kb ?? (g?.worker.pid ? await ctx.host.rssKb(g.worker.pid) : null);
      const saves = savesOnDisk(server, game);
      leaks.push({ t: Date.now() - started, turn: g?.turn ?? null, sessions: m.server.sessions.open, gateway_rss_kb: gwRss, worker_rss_kb: wRss, saves: saves.length, ring: saves.filter((s) => s.kind === "Ring").length });
    } catch {
      // as above
    }
  }, 5000);

  const budget = budgetFor(turns, 20000) + 120000;
  const finished = await finishAll(running, budget);
  clearInterval(turnTimer);
  clearInterval(leakTimer);
  await chaos.stop();
  const wall = (Date.now() - started) / 1000;
  const kinds = {};
  for (const e of chaos.log) kinds[e.kind] = (kinds[e.kind] ?? 0) + 1;

  expect("the stuck-turn detector found no turn stuck", stuck.length === 0, stuck.length ? JSON.stringify(stuck[0]) : `${turnLog.length} samples, stuck_s ${opts.stuck_s}`);
  if (!expect("all bots finished in time", finished !== null, `${budget / 1000} s budget; turn log tail ${JSON.stringify(turnLog.slice(-3))}`)) {
    return { numbers: { chaos: kinds, stuck, wall_s: wall, turn_log_tail: turnLog.slice(-5) } };
  }
  const onlyExpected = (f) => (f.result.errors ?? []).every((e) => meta.allowErrors.some((re) => re.test(e)));
  const results = botChecks(expect, names, finished.map((f) => ({ ...f, code: f.code === 0 || (f.code === 1 && onlyExpected(f)) ? 0 : f.code })), turns, { tag: "soak", slack: (r) => (r.reconnects ?? []).length });
  const g = await gatewayGame(ctx, game, results);
  results.forEach((r, i) => expect(`bot ${names[i]}: every order has one outcome, none open`, r.ledger.double_outcome === 0 && r.ledger.open === 0, JSON.stringify(r.ledger)));
  expect(`the game is at turn ${turns} or later`, !!g && g.turn >= turns, `turn ${g?.turn}`);
  expect("gateway: no fog violation", !!g && g.fog_violations === 0, `${g?.fog_violations}`);
  // (a worker killed by the chaos a moment ago may still be coming back)
  const running2 = await until(async () => ((await server.admin.game(game))?.status === "Running" ? true : null), 30000, 100);
  expect("the game still runs", !!running2, g?.status);

  // ---- leaks
  await ctx.sleep(1500);
  const m = await server.admin.metrics();
  expect("no more sessions are open than there are bots", m.server.sessions.open <= nBots, `${m.server.sessions.open} open, ${nBots} bots`);
  const maxRing = Math.max(0, ...leaks.map((l) => l.ring));
  expect("the autosave ring never held more than 10 files", maxRing <= 10, `at most ${maxRing}`);
  const trend = (key) => {
    const v = leaks.filter((l) => typeof l[key] === "number");
    if (v.length < 6) return null;
    const third = Math.floor(v.length / 3);
    const first = median(v.slice(0, third).map((l) => l[key]));
    const last = median(v.slice(-third).map((l) => l[key]));
    return { first_kb: first, last_kb: last, kb_per_min: Math.round(slope(v.map((l) => l.t / 60000), v.map((l) => l[key]))) };
  };
  const rss = { gateway: trend("gateway_rss_kb"), worker: trend("worker_rss_kb") };
  ctx.pending("memory trend: BUILD N6.1 gives no bound for it, the numbers (rss.gateway, rss.worker) are reported without a check");

  const lat = results.map((r) => r.latency_ms.p95).filter((x) => typeof x === "number");
  const numbers = {
    bots: nBots,
    turns,
    wall_s: Math.round(wall),
    turns_per_min: Math.round((turns / wall) * 600) / 10,
    chaos_events: kinds,
    worker_restarts: (g?.worker.starts ?? 1) - 1,
    order_p95_ms: summary(lat),
    reconnects: results.map((r) => (r.reconnects ?? []).length),
    stuck: stuck.length,
    sessions_open_end: m.server.sessions.open,
    sessions_lost_timeout_total: m.server.sessions.lost_timeout_total,
    rss,
    ring_max: maxRing,
  };

  // ---- history.jsonl, one line per run, in the output directory
  try {
    let commit = "unknown";
    try {
      commit = execFileSync("git", ["rev-parse", "--short", "HEAD"], { cwd: ctx.root, encoding: "utf8" }).trim();
    } catch {
      // not a checkout
    }
    const out = path.resolve(ctx.logDir, "..", "..");
    const line = { at: new Date().toISOString(), commit, preset: opts.preset, core: opts.core, seed: opts.seed, scenario: "soak", turns, bots: nBots, ok: stuck.length === 0, numbers };
    appendFileSync(path.join(out, "history.jsonl"), JSON.stringify(line) + "\n");
  } catch (e) {
    ctx.pending(`history.jsonl could not be written: ${e}`);
  }
  return { numbers, details: { turn_log: turnLog.filter((_, i) => i % 5 === 0), chaos: chaos.log, leaks } };
}
