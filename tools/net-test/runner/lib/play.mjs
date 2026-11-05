// Helpers for scenarios that play one server game with autonomous bots (`net-bot play`), the pattern of `m1`:
// the first bot hosts a room (AI in the slots after the humans), the others join and say they are ready, every
// bot plays its turns, then the copies of the views are compared with the gateway's.
import { asBig, hex } from "./json.mjs";

export const sum = (o) => Object.values(o ?? {}).reduce((s, n) => s + Number(n), 0);

/**
 * The end of `h`'s turn on a world where a turn cannot be ended while a unit waits for an order (the real core: plain
 * and forced ends are refused, `FORCE_END_TURN` lets only one unit go): every unit of the seat is skipped, then the
 * forced end. The test world takes it as it is. Cities and research need no order in the scenarios that use this.
 */
export async function endTurn(h) {
  for (const unit of (await h.state()).mine ?? []) {
    const id = await h.cmd({ UnitSkip: { unit } });
    await h.wait({ result_id: id }, 10000);
  }
  return h.cmd({ EndTurn: { force: true } });
}
export const nicksOf = (prefix, n) => Array.from({ length: n }, (_, i) => `${prefix}${i + 1}`);

/**
 * Starts `names.length` missions: the first bot hosts, the rest join.
 * @param {object} ctx
 * @param {string[]} names
 * @param {{turns: number, humans?: number, ai_slots?: number[], settings?: object, persona?: string | string[],
 *   persona_args?: object | object[], knobs?: object | object[], pace?: string, url?: string, extra?: object,
 *   turn_timeout_ms?: number, lobby_timeout_ms?: number}} o
 */
export function startGame(ctx, names, o) {
  const each = (v, i) => (Array.isArray(v) ? v[i] : v);
  return names.map((nick, i) =>
    (o.bots ?? ctx.bots).play(nick, {
      turns: o.turns,
      pace: o.pace ?? "burst",
      persona: each(o.persona, i) ?? "autoplay",
      ...(each(o.persona_args, i) ? { persona_args: each(o.persona_args, i) } : {}),
      ...(each(o.knobs, i) ? { knobs: each(o.knobs, i) } : {}),
      lobby_timeout_ms: o.lobby_timeout_ms ?? 120000,
      turn_timeout_ms: o.turn_timeout_ms ?? 120000,
      ...(o.url ? { url: o.url } : {}),
      lobby:
        i === 0
          ? { role: "host", humans: o.humans ?? names.length, ai_slots: o.ai_slots ?? [], ...(o.host_slot != null ? { host_slot: o.host_slot } : {}), ...(o.settings ? { settings: o.settings } : {}) }
          : { role: "guest", host: names[0], slots: (o.humans ?? names.length) + (o.ai_slots ?? []).length },
      ...(o.extra ?? {}),
    }),
  );
}

/** Waits for all the missions; `null` when the budget is over. */
export async function finishAll(running, budgetMs) {
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

/**
 * Waits like `finishAll`, and also watches the gateway: a game that turns `Faulted` (its worker died or did not come up)
 * ends the wait at once, the bots are stopped, and `fault` says what happened. Without it the bots would wait for the
 * game until their own timeouts. Returns `{finished, fault}`: `finished` is `null` on the budget or a fault.
 */
export async function finishWatched(ctx, running, budgetMs) {
  let fault = null;
  let polling = true;
  const watch = (async () => {
    while (polling) {
      try {
        const m = await ctx.server.admin.metrics();
        const bad = Object.entries(m.games ?? {}).find(([, g]) => /fault/i.test(String(g.status)));
        if (bad || (m.server?.games?.faulted ?? 0) > 0) {
          fault = bad ? `game ${bad[0]} is ${bad[1].status}` : `${m.server.games.faulted} game(s) faulted (the worker did not come up or died)`;
          running.forEach((r) => r.kill());
          return;
        }
      } catch {
        // the admin API answers a moment after the server is up
      }
      await ctx.sleep(1000);
    }
  })();
  const finished = await finishAll(running, budgetMs);
  polling = false;
  await watch;
  return { finished: fault ? null : finished, fault };
}

/** The time a scenario gives its bots for `turns` turns (generous: a stuck game is a failure, not a hang). */
export const budgetFor = (turns, perTurnMs = 10000) => 60000 + turns * perTurnMs;

/**
 * The checks every bot of a finished game passes (the ones of `m1`).
 * `opts.slack(r)`: how many turns a bot may have missed in its own count (chaos).
 * `opts.hashOk`: the bot corrupts its own view on purpose, so mismatches and keyframe requests are not errors.
 */
export function botChecks(expect, names, finished, turns, opts = {}) {
  const results = finished.map((f) => f.result);
  const tag = opts.tag ? `${opts.tag}: ` : "";
  results.forEach((r, i) => {
    const who = `${tag}bot ${names[i]}`;
    expect(`${who} exited cleanly`, finished[i].code === 0, `exit ${finished[i].code}`);
    // `opts.slack(r)`: turns the bot may not have counted (an end-turn answered while its link was reset, soak)
    const slack = opts.slack ? opts.slack(r) : 0;
    // `opts.over`: turns played twice after a rollback the scenario allows (a crash restores an earlier save).
    const over = opts.over ?? 0;
    expect(`${who} played ${turns} turns`, r.play?.done === "turns" && r.turns_played <= turns + over && r.turns_played >= turns - slack, `done ${r.play?.done}, turns_played ${r.turns_played}${slack ? ` (slack ${slack})` : ""}${over ? ` (over ${over})` : ""}`);
    if (!opts.hashOk) {
      expect(`${who} checked hashes and none differed`, r.hash.checked > 0 && r.hash.mismatch === 0, `checked ${r.hash.checked}, mismatch ${r.hash.mismatch}`);
      expect(`${who} never asked for a keyframe`, sum(r.need_keyframe) === 0, JSON.stringify(r.need_keyframe));
    }
    expect(`${who} found no fog violation`, opts.fogOk || r.fog.violations === 0, `${r.fog.violations} of ${r.fog.checked} checks${r.fog.examples?.length ? ": " + JSON.stringify(r.fog.examples[0]) : ""}`);
    expect(`${who} has no lobby errors`, (r.lobby_errors ?? []).length === 0, (r.lobby_errors ?? []).join("; "));
  });
  return results;
}

/** The gateway's counters of the game once its seats hold the final hashes of the bots (it publishes a moment after it sends). */
export async function gatewayGame(ctx, game, results, tries = 50) {
  const agree = (gm) => gm && results.every((r) => gm.seats?.[String(r.player)]?.view_hash !== undefined && asBig(gm.seats[String(r.player)].view_hash) === asBig(r.final.view_hash));
  let g = null;
  for (let i = 0; i < tries; i++) {
    g = await ctx.server.admin.game(game);
    if (agree(g)) break;
    await ctx.sleep(100);
  }
  return g;
}

/**
 * Waits until a save of the game at `turn` or later is on disk (the admin API lists it). The worker writes its saves
 * one after the other; on a slow disk (the state dir on a Windows drive under WSL) they lag behind a burst of turns,
 * and a stop that comes first would take the newest ones away. Returns the newest save, or `null` on the time budget.
 */
export async function waitForSave(ctx, game, turn, ms = 300000) {
  const t0 = Date.now();
  for (;;) {
    const detail = await ctx.server.admin.get(`/admin/games/${game}`).catch(() => null);
    const latest = (detail?.saves ?? []).reduce((m, s) => (!m || s.seq > m.seq ? s : m), null);
    if (latest && latest.turn >= turn) return latest;
    if (Date.now() - t0 > ms) return null;
    await ctx.sleep(250);
  }
}

/** Checks of the gateway side that hold for a clean game. */
export function gatewayChecks(expect, g, results, names, o = {}) {
  results.forEach((r, i) => {
    const gw = g.seats?.[String(r.player)]?.view_hash;
    expect(`bot ${names[i]}'s view hash equals the gateway's`, gw !== undefined && asBig(gw) === asBig(r.final.view_hash), `bot ${hex(r.final.view_hash)}, gateway ${gw === undefined ? "?" : hex(gw)}`);
  });
  if (!o.dirty) {
    expect("gateway: nothing lost, nothing expired", g.cmds.lost === 0 && g.cmds.expired === 0, `lost ${g.cmds.lost}, expired ${g.cmds.expired}`);
    expect("gateway: no keyframe was asked for", sum(g.views.need_keyframe) === 0, JSON.stringify(g.views.need_keyframe));
    expect("gateway: no wantfull and no worker hash failure", g.views.wantfull === 0 && g.views.hash_fail_worker === 0, `wantfull ${g.views.wantfull}, hash_fail_worker ${g.views.hash_fail_worker}`);
  }
  if (!o.leakOk) expect("gateway: fog_violations is 0", g.fog_violations === 0, `${g.fog_violations}`);
  expect("gateway: the worker never crashed", g.worker.crashes === 0, `${g.worker.crashes}`);
}
