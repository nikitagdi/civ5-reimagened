// What the scenarios do to the server and the channel on purpose (net-test 4.5, 4.7): kill and freeze a worker, kill
// the gateway, drain it, wait for the game to come back, read the autosave ring from disk, and the random chaos of
// `soak` (a seeded schedule of channel changes, drops and worker kills). The checks stay in the scenarios.
import { readdirSync, statSync } from "node:fs";
import path from "node:path";
import { rng } from "./stats.mjs";

export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** Polls `pred` until it gives a truthy value; that value, or `null` after `ms`. */
export async function until(pred, ms, stepMs = 100) {
  const t0 = Date.now();
  for (;;) {
    let v = null;
    try {
      v = await pred();
    } catch {
      // the gateway may be down for a moment: ask again
    }
    if (v) return v;
    if (Date.now() - t0 >= ms) return null;
    await sleep(stepMs);
  }
}

/** `metrics().games[id]`, or `null` while the gateway does not answer. */
export const gameNow = (server, game) => server.admin.game(game).catch(() => null);

/**
 * Waits until the game runs on a worker that started after `startsBefore` starts. Returns `{ms, g}` (ms from `t0`,
 * a `performance.now()` mark) or `null`.
 */
export async function runningAgain(server, game, startsBefore, t0, ms = 60000) {
  const g = await until(async () => {
    const x = await gameNow(server, game);
    return x && x.status === "Running" && x.worker.starts > startsBefore ? x : null;
  }, ms, 50);
  return g ? { ms: performance.now() - t0, g } : null;
}

/** The saves of a game on disk, newest first: `{seq, turn, kind, mtimeMs}` (the ring and the keep directory). */
export function savesOnDisk(server, game) {
  const out = [];
  for (const [sub, kind] of [["ring", "Ring"], ["keep", "Keep"]]) {
    const dir = path.join(server.stateDir, "games", String(game), sub);
    let names = [];
    try {
      names = readdirSync(dir);
    } catch {
      continue;
    }
    for (const f of names) {
      const m = /^(\d+)-t(\d+)\.c5sv$/.exec(f);
      if (!m) continue;
      try {
        out.push({ seq: Number(m[1]), turn: Number(m[2]), kind, mtimeMs: statSync(path.join(dir, f)).mtimeMs, file: `${sub}/${f}` });
      } catch {
        // renamed or removed from the ring in the meantime
      }
    }
  }
  return out.sort((a, b) => b.seq - a.seq);
}

/**
 * A crash of the worker: a real `kill -9` (`signal` KILL) or a freeze (STOP) through the test hook. Returns what the
 * scenario needs to judge the recovery.
 */
export async function crashWorker(ctx, game, signal = "KILL", { within = 60000 } = {}) {
  const { server } = ctx;
  const before = await server.admin.game(game);
  const newest = savesOnDisk(server, game)[0] ?? null;
  const killedAtWall = Date.now();
  const r = await server.killWorker(game, signal);
  const t0 = performance.now();
  const back = await runningAgain(server, game, before.worker.starts, t0, within);
  return {
    hook: r,
    before,
    after: back?.g ?? null,
    recoverMs: back?.ms ?? null,
    // The age of the newest save when the worker died: the most orders that can be lost (kill9, 60 s).
    saveAgeS: newest ? (killedAtWall - newest.mtimeMs) / 1000 : null,
    saveTurn: newest?.turn ?? null,
    turnAtKill: before.turn,
    pid: before.worker.pid,
  };
}

/**
 * `kill -9` of the gateway alone, then a new gateway on the same state dir. Returns when the game runs again.
 * `pids` are the workers of the old gateway: how long they live on after it (they save on EOF) is measured.
 */
export async function crashGateway(ctx, game) {
  const { server, host } = ctx;
  const m0 = await server.admin.metrics();
  const g0 = m0.games[String(game)];
  const pids = Object.values(m0.games).map((g) => g.worker.pid).filter(Boolean);
  const saved0 = savesOnDisk(server, game)[0] ?? null;
  const killedAt = Date.now();
  await server.killGateway();
  // The workers see EOF on their stdin, save and leave.
  const gone = await until(async () => {
    for (const p of pids) if (await host.pidAlive(p)) return null;
    return true;
  }, 20000, 100);
  const workersGoneMs = Date.now() - killedAt;
  const saved1 = savesOnDisk(server, game)[0] ?? null;
  const t0 = performance.now();
  await server.start();
  const back = await runningAgain(server, game, -1, t0, 60000);
  return { pids, workersGone: !!gone, workersGoneMs, savedBefore: saved0, savedAfter: saved1, killedAt, turnBefore: g0.turn, back: back?.g ?? null, backMs: back?.ms ?? null };
}

/** An upgrade request for `/ws` made by hand: the status the gateway answers (503 while it drains), or `null`. */
export async function probeUpgrade(server) {
  const http = await import("node:http");
  return new Promise((resolve) => {
    const u = new URL(server.url);
    const req = http.request(
      { host: u.hostname, port: u.port, path: "/ws", headers: { Connection: "Upgrade", Upgrade: "websocket", "Sec-WebSocket-Version": "13", "Sec-WebSocket-Key": "dGhlIHNhbXBsZSBub25jZQ==" } },
      (res) => {
        res.resume();
        resolve({ status: res.statusCode, retryAfter: res.headers["retry-after"] ?? null });
      },
    );
    req.on("upgrade", (res, socket) => {
      socket.destroy();
      resolve({ status: res.statusCode, retryAfter: null });
    });
    req.on("error", () => resolve(null));
    req.setTimeout(3000, () => {
      req.destroy();
      resolve(null);
    });
    req.end();
  });
}

/**
 * The random chaos of `soak` (net-test 4.7): every few seconds one event from a seeded list, the schedule is
 * the same for the same `--seed`. Events: a new channel profile (delay, jitter, stall), a reset of all connections
 * (RST), a blackhole of 0.5-3 s, a cut of one direction, and (rarely) a kill of the worker. Returns
 * `{stop(), log, busyUntil()}`: `busyUntil` is the wall time until which the last event may keep the game from moving.
 */
export function startChaos(ctx, { link, game, seed, everyS = [3, 9], workerKills = true, killEveryS = 90 }) {
  const r = rng(seed ^ 0x5eed);
  const log = [];
  let stopped = false;
  let busyUntil = 0;
  let lastKill = Date.now();
  const note = (kind, detail = {}) => log.push({ t: Date.now(), kind, ...detail });
  const events = [
    async () => {
      const p = { rtt_ms: r.int(0, 300), jitter_ms: r.int(0, 40), stall_p: r.pick([0, 0, 0.01, 0.03]) };
      await link.set(p);
      note("profile", p);
    },
    async () => {
      await link.reset();
      busyUntil = Date.now() + 12000;
      note("reset");
    },
    async () => {
      const ms = r.int(500, 3000);
      await link.blackhole(ms);
      busyUntil = Date.now() + ms + 3000;
      note("blackhole", { ms });
    },
    async () => {
      const ms = r.int(500, 2500);
      const dir = r.pick(["up", "down"]);
      await link.halfOpen(dir, ms);
      busyUntil = Date.now() + ms + 3000;
      note("half-open", { dir, ms });
    },
  ];
  const loop = (async () => {
    while (!stopped) {
      await sleep(r.int(everyS[0], everyS[1]) * 1000);
      if (stopped) break;
      try {
        if (workerKills && game && Date.now() - lastKill > killEveryS * 1000) {
          lastKill = Date.now();
          const g = await ctx.server.admin.game(game);
          if (g?.status === "Running") {
            await ctx.server.killWorker(game, "KILL");
            busyUntil = Date.now() + 30000;
            note("kill-worker", { turn: g.turn });
          }
        } else await r.pick(events)();
      } catch (e) {
        note("error", { error: String(e) });
      }
    }
  })();
  return {
    log,
    busyUntil: () => busyUntil,
    async stop() {
      stopped = true;
      await loop;
      // A clean channel for the rest of the run.
      await link.clear().catch(() => {});
    },
  };
}
