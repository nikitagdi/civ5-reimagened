// Admin API client (game-server 4.13): `GET /admin/metrics` and `/admin/games`, every request with the
// `X-Admin-Token` header. Two ways in: HTTP on loopback (a server the runner started) and
// `docker exec <container> civ5-server admin GET <path>` (the deployed service: the admin port is not
// published, NT-D16).

import { parse } from "./json.mjs";

/** @param {{via: "http", base: string, token: () => Promise<string>} | {via: "docker", context: string, container: string, host: import("./host.mjs").LocalHost}} cfg */
export function createAdmin(cfg) {
  async function httpGet(path, withToken = true) {
    const headers = {};
    if (withToken) headers["X-Admin-Token"] = (await cfg.token()).trim();
    const r = await fetch(cfg.base + path, { headers });
    const text = await r.text();
    if (!r.ok) throw new Error(`admin GET ${path}: HTTP ${r.status}`);
    return parse(text);
  }

  function dockerExec(method, path) {
    const args = [];
    if (cfg.context) args.push("--context", cfg.context);
    args.push("exec", cfg.container, "civ5-server", "admin", method, path);
    return cfg.host.exec("docker", args, { timeoutMs: 30000 });
  }

  async function dockerGet(path) {
    const r = await dockerExec("GET", path);
    if (r.code !== 0) throw new Error(`admin GET ${path} via docker: exit ${r.code}: ${r.stderr.trim()}`);
    return parse(r.stdout);
  }

  const get = (path) => (cfg.via === "docker" ? dockerGet(path) : httpGet(path));

  /** `POST path`; returns `{status, body}` and never throws on a non-2xx status. The docker route has no status
   * code (the CLI exits 1 on a non-2xx answer: `status` is then 0) and sends no body. */
  async function post(path, body) {
    if (cfg.via === "docker") {
      if (body !== undefined) throw new Error("POST with a body needs the HTTP admin API");
      const r = await dockerExec("POST", path);
      let parsed = r.stdout.trim() || null;
      try {
        parsed = parsed ? parse(parsed) : null;
      } catch {
        // a plain text answer stays text
      }
      return { status: r.code === 0 ? 200 : 0, ok: r.code === 0, body: parsed };
    }
    const headers = { "X-Admin-Token": (await cfg.token()).trim() };
    if (body !== undefined) headers["content-type"] = "application/json";
    const r = await fetch(cfg.base + path, { method: "POST", headers, body: body === undefined ? undefined : JSON.stringify(body) });
    const text = await r.text();
    let parsed = null;
    try {
      parsed = text ? parse(text) : null;
    } catch {
      parsed = text;
    }
    return { status: r.status, ok: r.ok, body: parsed };
  }

  return {
    via: cfg.via,
    get,
    post,
    /** `POST /admin/games/{id}/save`: a save now (a worker checkpoint). */
    save: (id) => post(`/admin/games/${id}/save`),
    /** `POST /admin/games/{id}/stop`: save and stop the game; its worker ends. */
    stop: (id) => post(`/admin/games/${id}/stop`),
    /** `POST /admin/games/{id}/close`: save and close for good (running or asleep): seats and nicks free, saves stay. */
    close: (id) => post(`/admin/games/${id}/close`),
    restart: (id) => post(`/admin/games/${id}/restart`),
    /** `POST /admin/games/{id}/purge`: close the game if it is open, delete its saves, autosaves and record. */
    purge: (id) => post(`/admin/games/${id}/purge`),
    /** `POST /admin/purge?nick_prefix=..`: the same for every game that is not running and whose humans all have
     * such a nick (the old `smoke*` leftovers). */
    purgeByNick: (prefix) => post(`/admin/purge?nick_prefix=${encodeURIComponent(prefix)}`),
    kick: (id, player) => post(`/admin/games/${id}/kick?player=${player}`),
    drain: (exit = false) => post(`/admin/drain${exit ? "?exit=1" : ""}`),
    /** Test hooks (`test_hooks = true`): a signal to the worker, a fault for a seat. */
    killWorker: (id, signal = "KILL") => post(`/admin/games/${id}/kill-worker?signal=${signal}`),
    fault: (id, seat, kind, value) => post(`/admin/games/${id}/fault`, { seat, kind, ...(value === undefined ? {} : { value }) }),
    /** Closes every game that is not in `keep` (a set of ids from an earlier `games()`); returns the ids closed. A test
     * cleans up its own games: a game nobody is in is closed by the server after 5 minutes anyway, but not left to it. */
    closeGamesExcept: async (keep) => {
      const closed = [];
      for (const g of (await get("/admin/games")).games ?? []) {
        if (keep.has(g.id) || !(await post(`/admin/games/${g.id}/close`)).ok) continue;
        closed.push(g.id);
      }
      return closed;
    },
    /** `GET /admin/metrics` (`schema: 1`). */
    metrics: () => get("/admin/metrics"),
    /** `GET /admin/games`. */
    games: () => get("/admin/games"),
    /** `metrics().games[id]`. */
    game: async (id) => (await get("/admin/metrics")).games?.[String(id)] ?? null,
    /** Status of a request without the token (the admin API must refuse it); HTTP only. */
    statusWithoutToken: async (path = "/admin/metrics") => {
      if (cfg.via !== "http") throw new Error("statusWithoutToken needs the HTTP admin API");
      return (await fetch(cfg.base + path)).status;
    },
  };
}
