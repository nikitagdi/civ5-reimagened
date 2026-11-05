// The bots of a scenario (`ctx.bots`, net-test 4.4): `net-bot ctl` processes driven line by line (JSON on
// stdin and stdout, net-test 4.2). Requests carry a `req` tag; answers have it back; lines with `ev` are
// the bot's events and go to `timeline`. A scenario stops its bots (`stop()` returns `result.json`);
// the runner stops the rest before the common checks.
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { createInterface } from "node:readline";
import { parse as parseJson } from "./json.mjs";

const PASSWORD_ENV = "NETTEST_PASSWORD";

/**
 * @param {{host: import("./host.mjs").LocalHost, bot: string, server: {wsUrl: string, password: string} | null,
 *   logDir: string, seed: number, verbose?: boolean}} o
 */
export function createBots(o) {
  const handles = [];
  const finished = [];

  function spawnBot(opts = {}) {
    if (!o.bot) throw new Error("net-bot is not built (--bot <net-bot executable>)");
    if (!o.server) throw new Error("the scenario has no server to connect to");
    const nick = opts.nick ?? `bot${handles.length + 1}`;
    const dir = path.join(o.logDir, "bots", nick);
    mkdirSync(dir, { recursive: true });
    const resultPath = path.join(dir, "result.json");
    const timelinePath = path.join(dir, "timeline.jsonl");
    const child = o.host.spawn(o.bot, ["ctl", "--result", o.host.path(resultPath), "--timeline", o.host.path(timelinePath)], {
      stdio: ["pipe", "pipe", "pipe"],
      env: { ...process.env, [PASSWORD_ENV]: o.server.password ?? "" },
    });
    let stderr = "";
    child.stderr.on("data", (d) => (stderr += d));
    const events = [];
    const pending = new Map();
    let tag = 0;
    let exited = false;
    const lines = createInterface({ input: child.stdout });
    lines.on("line", (line) => {
      let msg;
      try {
        msg = parseJson(line);
      } catch {
        events.push({ ev: "garbage", line });
        return;
      }
      if (msg.req !== undefined && pending.has(msg.req)) {
        const { resolve, reject, op } = pending.get(msg.req);
        pending.delete(msg.req);
        if (msg.ok) resolve(msg);
        else reject(new Error(`bot ${nick}: ${op}: ${msg.error}`));
      } else if (msg.ev) events.push(msg);
    });
    // A write to a bot that has exited raises EPIPE on stdin; the write callback rejects that request.
    child.stdin.on("error", () => {});
    child.on("exit", (code) => {
      exited = true;
      for (const { reject, op } of pending.values()) reject(new Error(`bot ${nick} exited (code ${code}) during ${op}: ${stderr.trim()}`));
      pending.clear();
    });

    const send = (op, fields = {}) =>
      new Promise((resolve, reject) => {
        if (exited) return reject(new Error(`bot ${nick} is gone`));
        const req = ++tag;
        pending.set(req, { resolve, reject, op });
        child.stdin.write(JSON.stringify({ op, req, ...fields }) + "\n", (err) => {
          if (err && pending.delete(req)) reject(new Error(`bot ${nick} is gone during ${op}: ${err.message}`));
        });
      });

    let result = null;
    const h = {
      nick,
      events,
      send,
      // The token file is read and written by the bot, which may live in WSL: the path goes through the host.
      connect: (extra = {}) => send("connect", { url: o.server.wsUrl, nick, password_env: PASSWORD_ENV, ...extra, token_file: o.host.path(extra.token_file ?? path.join(dir, "token")) }),
      lobby: (cmd) => send("lobby", { cmd }),
      /** The lobby part of a mission (`{role: "host", humans, ai_slots, settings}`, `{role: "guest", host}`, `{role: "load"}`, `{role: "join", game, player}`): answers when the bot sits in a game. */
      enter: (lobby, extra = {}) => send("enter", { lobby, ...extra }),
      /** Starts the autonomous policy inside this bot (`persona`, `turns`, `pace`); wait for it with `wait({autoplay: 1})`. */
      autoplay: (opts = {}) => send("autoplay", opts),
      cmd: (command, extra = {}) => send("cmd", { command, ...extra }).then((r) => r.cmd_id),
      focus: (units, city = null) => send("focus", { units, city }),
      query: (q) => send("query", { q }).then((r) => r.query_id),
      chat: (to, text) => send("chat", { to, text }),
      gamectl: (ctl) => send("gamectl", { ctl }),
      needKeyframe: (reason) => send("need_keyframe", { reason }),
      /** Waits for a condition (`{turn: 5}`, `{phase: "Humans"}`, `{may_act: true}`, `{result_id: 3}`, ...). Throws on a timeout. */
      async wait(until, timeoutMs = 30000) {
        const r = await send("wait", { until, timeout_ms: timeoutMs });
        if (!r.met) throw new Error(`bot ${nick}: timed out after ${timeoutMs} ms waiting for ${JSON.stringify(until)} (turn ${r.state.turn}, phase ${r.state.phase})`);
        return r.state;
      },
      state: () => send("state").then((r) => r.state),
      knob: (name, value) => send("knob", { name, value }),
      drop: () => send("drop"),
      reconnect: (resume = true) => send("reconnect", { resume }),
      stats: () => send("stats").then((r) => r.result),
      /** Ends the bot; returns its `result.json`. */
      async stop() {
        if (result) return result;
        if (!exited) {
          try {
            result = (await send("quit")).result;
          } catch {
            // the bot died; its result file may still be there
          }
          child.stdin.end();
          await new Promise((r) => {
            if (exited) return r();
            child.once("exit", r);
            setTimeout(() => {
              child.kill();
              r();
            }, 5000);
          });
        }
        result ??= { nick, errors: [`bot ${nick} ended without a result`], hash: {}, ledger: {}, fog: {} };
        finished.push(result);
        return result;
      },
    };
    handles.push(h);
    return h;
  }

  /**
   * An autonomous bot (`net-bot play`): `mission` is the mission JSON (lobby part, turns, pace, persona, knobs).
   * Returns `{nick, done}`; `done` resolves with `{code, result, stderr}` when the bot is finished. Its
   * `result.json` is recorded for the common checks.
   */
  function play(nick, mission) {
    if (!o.bot) throw new Error("net-bot is not built (--bot <net-bot executable>)");
    const dir = path.join(o.logDir, "bots", nick);
    mkdirSync(dir, { recursive: true });
    const missionPath = path.join(dir, "mission.json");
    const resultPath = path.join(dir, "result.json");
    const body = {
      url: o.server.wsUrl,
      nick,
      password_env: PASSWORD_ENV,
      token_file: o.host.path(path.join(dir, "token")),
      seed: o.seed,
      ...mission,
    };
    writeFileSync(missionPath, JSON.stringify(body, null, 2));
    const child = o.host.spawn(
      o.bot,
      ["play", "--mission", o.host.path(missionPath), "--result", o.host.path(resultPath), "--timeline", o.host.path(path.join(dir, "timeline.jsonl"))],
      { stdio: ["ignore", "pipe", "pipe"], env: { ...process.env, [PASSWORD_ENV]: o.server.password ?? "" } },
    );
    let stderr = "";
    child.stderr.on("data", (d) => (stderr += d));
    child.stdout.on("data", () => {}); // the result is read from the file
    const done = new Promise((resolve) => {
      child.on("exit", async (code) => {
        let result = null;
        try {
          result = parseJson(await o.host.readFile(resultPath));
        } catch {
          result = { nick, errors: [`bot ${nick} left no result (exit ${code}): ${stderr.trim()}`], hash: {}, ledger: {}, fog: {} };
        }
        finished.push(result);
        resolve({ code, result, stderr });
      });
    });
    const h = { nick, play: true, done, kill: () => child.kill(), exited: () => child.exitCode !== null };
    handles.push(h);
    return h;
  }

  return {
    spawn: spawnBot,
    play,
    /** `n` bots `prefix1..n`, connected. */
    async fleet(n, opts = {}) {
      const out = [];
      for (let i = 1; i <= n; i++) {
        const b = spawnBot({ ...opts, nick: `${opts.prefix ?? "bot"}${i}` });
        await b.connect();
        out.push(b);
      }
      return out;
    },
    results: () => finished,
    async stopAll() {
      for (const h of handles) {
        if (h.play) {
          // A mission bot that is still running at the end of a scenario is stopped, not waited for.
          if (!h.exited()) {
            h.kill();
            await h.done;
          }
        } else await h.stop();
      }
    },
  };
}
