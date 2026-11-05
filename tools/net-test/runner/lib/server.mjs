// The server under test (NT-D6): the real `civ5-server` (gateway and worker process) with `test_hooks` and
// `[worker] audit_fog = true`, on a temporary state dir. Or, with `--attach`, an already running server
// that the runner only measures (no hooks, NT-D16).
import { randomBytes } from "node:crypto";
import { createWriteStream, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { createAdmin } from "./admin.mjs";

/** `server.toml` of a run. Only keys the gateway knows (it refuses unknown ones). */
export function serverToml(o) {
  const lines = [`test_hooks = ${o.hooks ? "true" : "false"}`];
  if (o.hostLostS) lines.push(`host_lost_s = ${o.hostLostS}`);
  if (o.maxGames) lines.push(`max_games = ${o.maxGames}`);
  if (o.hangS) lines.push(`hang_s = ${o.hangS}`);
  if (o.listenHost) lines.push("allow_plain_nonlocal = true");
  if (o.gamedataDir) lines.push(`gamedata_dir = "${o.gamedataDir}"`);
  lines.push("", "[worker]", `audit_fog = ${o.auditFog ? "true" : "false"}`);
  if (o.autosaveMinIntervalS) lines.push("", "[worker.autosave]", `min_interval_s = ${o.autosaveMinIntervalS}`, "ring = 10", "keep_every = 10");
  if (o.hooks && o.fixtureTest) {
    lines.push("", "[worker.fixture_test]");
    for (const [k, v] of Object.entries(o.fixtureTest)) lines.push(`${k} = ${v}`);
  }
  return lines.join("\n") + "\n";
}

const NEWLINE = String.fromCharCode(10);
const ANSI = new RegExp(String.fromCharCode(27) + "\\[[0-9;]*m", "g");

async function waitHealthz(base, child, timeoutMs, logPath) {
  const start = Date.now();
  let last = "";
  while (Date.now() - start < timeoutMs) {
    if (child && child.exitCode !== null) throw new Error(`server exited early (code ${child.exitCode})`);
    try {
      const r = await fetch(`${base}/healthz`);
      if (r.status === 200) return;
      last = `HTTP ${r.status}`;
    } catch (e) {
      last = e.message;
    }
    // The Describe pass failed: the lobby stays closed for good, no use in waiting (the real core missing, say).
    let failed;
    try {
      failed = logPath && readFileSync(logPath, "utf8").split(NEWLINE).find((l) => l.includes("the Describe pass failed"));
    } catch {
      failed = undefined;
    }
    if (failed) {
      const text = failed.replace(ANSI, "").trim();
      throw Object.assign(new Error(`the server could not start: ${text}`), { describeFailed: text });
    }
    await new Promise((r) => setTimeout(r, 150));
  }
  throw new Error(`/healthz did not turn 200 within ${timeoutMs} ms (${last})`);
}

/**
 * Starts the gateway. The ports are picked by the runner (`freePort`): two parallel runs never meet on the
 * default 8080/9090. Returns the handle scenarios use (`ctx.server`).
 * @param {{host: import("./host.mjs").LocalHost, exe: string, core: string, logDir: string, hooks?: boolean,
 *   auditFog?: boolean, hostLostS?: number, maxGames?: number, autosaveMinIntervalS?: number, fixtureTest?: object,
 *   keepState?: boolean}} o
 */
export async function startServer(o) {
  const host = o.host;
  mkdirSync(o.logDir, { recursive: true });
  const stateDir = path.join(o.logDir, "state");
  // A run starts on a clean state dir (the games of an earlier run would come back otherwise).
  if (!o.keepState) rmSync(stateDir, { recursive: true, force: true });
  const port = await host.freePort();
  let adminPort = await host.freePort();
  while (adminPort === port) adminPort = await host.freePort();
  const password = randomBytes(12).toString("base64url");
  const tomlPath = path.join(o.logDir, "server.toml");
  writeFileSync(tomlPath, serverToml({ ...o, hooks: o.hooks ?? true, auditFog: o.auditFog ?? true, gamedataDir: o.gamedataDir ? host.path(o.gamedataDir) : undefined }));
  const logPath = path.join(o.logDir, "server.log");
  const log = createWriteStream(logPath, { flags: "w" });
  // `listenHost`: the address the gateway listens on (`0.0.0.0` when the bots come through a netem veth pair).
  const listenHost = o.listenHost ?? "127.0.0.1";
  const url = `http://127.0.0.1:${port}`;
  const adminBase = `http://127.0.0.1:${adminPort}`;

  let child = null;
  const handle = {
    attached: false,
    url,
    wsUrl: `ws://127.0.0.1:${port}/ws`,
    port,
    adminPort,
    stateDir,
    password,
    logPath,
    starts: 0,
    /** Restarts nobody asked for would show here (common check). */
    unrequestedExit: false,
    expectedStop: false,
    /** `{code, signal}` of the last process of the gateway that ended. */
    lastExit: null,
    get pid() {
      return child?.pid ?? null;
    },
    alive: () => child !== null && child.exitCode === null,
    admin: createAdmin({
      via: "http",
      base: adminBase,
      // The token file appears with the first start of the gateway.
      token: async () => {
        const f = path.join(stateDir, "admin.token");
        for (let i = 0; i < 50; i++) {
          try {
            return readFileSync(f, "utf8");
          } catch {
            await new Promise((r) => setTimeout(r, 100));
          }
        }
        throw new Error(`no admin token at ${f}`);
      },
    }),
    log: () => {
      try {
        return readFileSync(logPath, "utf8");
      } catch {
        return "";
      }
    },
    async start() {
      const args = [
        "--config", host.path(tomlPath),
        "--core", o.core,
        "--listen", `${listenHost}:${port}`,
        "--admin-listen", `127.0.0.1:${adminPort}`,
        "--state-dir", host.path(stateDir),
        // The web client and its assets, for the browser scenarios (`--web-root`, `--assets`).
        ...(o.webRoot ? ["--web-root", host.path(o.webRoot)] : []),
        ...(o.assetsRoot ? ["--assets-root", host.path(o.assetsRoot)] : []),
      ];
      child = host.spawn(o.exe, args, {
        stdio: ["ignore", "pipe", "pipe"],
        env: { ...process.env, CIV5_PASSWORD: password, RUST_LOG: process.env.RUST_LOG ?? "info" },
      });
      handle.starts++;
      handle.expectedStop = false;
      child.stdout.pipe(log, { end: false });
      child.stderr.pipe(log, { end: false });
      handle.lastExit = null;
      child.on("exit", (code, signal) => {
        handle.lastExit = { code, signal };
        if (!handle.expectedStop) handle.unrequestedExit = true;
      });
      try {
        await waitHealthz(url, child, 60000, logPath);
      } catch (e) {
        await handle.kill();
        throw e;
      }
    },
    /** Stops the gateway and everything under it (the end of a scenario, a restart for a test of the lobby). */
    async kill() {
      handle.expectedStop = true;
      await host.killTree(child);
    },
    /**
     * `kill -9` of the gateway alone (NT contract `killGateway`, game-server 4.12): the worker is not touched, it
     * gets EOF on its stdin, saves and exits by itself. (`kill()` takes the whole tree down, workers too.)
     */
    async killGateway() {
      handle.expectedStop = true;
      await host.killProcess(child, `--listen ${listenHost}:${port}`);
    },
    async stop() {
      await handle.kill();
      log.end();
    },
    /** Stop and start again on the same ports and state dir. */
    async restart() {
      await handle.kill();
      await handle.start();
    },
    /** `POST /admin/drain` (game-server 4.12); with `exit` the process ends 0 when the games are saved. */
    drain: (exit = false) => handle.admin.drain(exit),
    /** `kill-worker` test hook: a real signal (`KILL`, `STOP`, `CONT`) to the worker of the game. */
    killWorker: (game, signal = "KILL") => handle.admin.killWorker(game, signal),
  };
  await handle.start();
  return handle;
}

/** A server the runner did not start (`--attach`): measured, never driven (no hooks, NT-D16). */
export function attachServer({ url, admin, password }) {
  const ws = url.replace(/^http/, "ws").replace(/\/$/, "") + "/ws";
  return {
    attached: true,
    url,
    wsUrl: ws,
    password,
    admin,
    starts: 0,
    unrequestedExit: false,
    alive: () => true,
    log: () => "",
    stop: async () => {},
    kill() {
      throw new Error("an attached server is not driven by the runner");
    },
  };
}
