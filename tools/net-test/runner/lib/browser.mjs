// The browser side of the net-test scenarios: one Chromium per client through the
// playtest's own `openPage` (read only, not changed), the game started the way a player starts it (`?autostart=1`,
// the sign-in typed once), and a few readings that need the page's clock (frames, the clock of the order).
import { act, openPage, settle, state, until } from "../../../playtest/lib/browser.mjs";

export { act, openPage, settle, state, until };

/**
 * The address of the game page. With a `link` (a channel from `ctx.proxyLink()`) the page itself comes through the
 * channel, so the page, its assets and the socket share one origin (the client takes its asset base from the socket's
 * address) and the profile of the channel works on all of them. The password never goes into the address.
 * @param {{server: {url: string}}} ctx
 * @param {Record<string, string>} q
 * @param {{url: string}} [link]
 */
export function pageUrl(ctx, q, link) {
  const query = new URLSearchParams({ test: "1", ...q });
  return `${link?.url ?? ctx.server.url}/?${query}`;
}

/** After a page load: the sign-in window if it shows (a token does without it), then the first human turn on the map. */
export async function signInAndWait(ctx, page, nick, { timeoutMs = 120000, mapTimeoutMs = 300000 } = {}) {
  const first = await until(page, (x) => x.screen === "login" || x.screen === "play", timeoutMs, "sign-in window or the game");
  if (first.screen === "login") await act(page, { kind: "login", nick, password: ctx.server.password });
  return until(page, (x) => x.screen === "play" && x.status === "human", mapTimeoutMs, "map on screen");
}

/**
 * A game for one human: the quick start of the address (a room of its own, the open slots to the AI, Start).
 * Resolves with the state at the first human turn.
 * @param {{nick: string, ai?: number, size?: string, seed?: number, mode?: string, link?: {url: string}}} p
 */
export async function startBrowserGame(ctx, page, p) {
  const q = { nick: p.nick, autostart: "1", ai: String(p.ai ?? 1), size: p.size ?? "duel", seed: String(p.seed ?? 1) };
  if (p.mode) q.mode = p.mode;
  await page.goto(pageUrl(ctx, q, p.link));
  return signInAndWait(ctx, page, p.nick);
}

/** The game the server runs for `nick` and the player number of that nick (admin metrics). */
export async function gatewaySeat(ctx, nick) {
  const m = await ctx.server.admin.metrics();
  for (const [id, g] of Object.entries(m.games ?? {})) {
    for (const [player, seat] of Object.entries(g.seats ?? {})) if (seat.nick === nick) return { game: Number(id), player: Number(player), seat, g };
  }
  return null;
}

const sum = (o) => Object.values(o ?? {}).reduce((s, x) => s + x, 0);

/** `window.__civ5test.metrics().client` (the counters of the client), or null. */
export async function clientStats(page) {
  return page.evaluate(() => window.__civ5test?.metrics()?.client ?? null).catch(() => null);
}

/** Queries the client sent so far (all kinds). */
export const queriesSent = (c) => sum(c?.queries);

/**
 * One order given by a right click on a plot, timed by the page's own frames: `frames` is the number of frames
 * between the frame that took the click and the frame that shows the unit on the new plot (the picture of the order,
 * NC-D6); `pictureMs` the same in time; `answerMs` the time to the server's final answer (the client's own ring of
 * order times). `picture` is false when nothing moved within `waitMs` (the client refuses at once: water, rivals).
 */
export async function timedMove(page, unit, x, y, waitMs = 1500) {
  return page.evaluate(
    ({ id, x, y, waitMs }) =>
      new Promise((resolve) => {
        const T = window.__civ5test;
        const at = () => T.state().units.find((u) => u.id === id);
        const u0 = at();
        const ring0 = (T.metrics().client?.orderMs ?? []).length;
        const sent0 = T.metrics().client?.orders?.sent ?? 0;
        const t0 = performance.now();
        const n = T.act({ kind: "click", x, y, secondary: true });
        let tAct = null;
        let fAct = null;
        const step = () => {
          const s = T.state();
          const now = performance.now();
          if (tAct === null && s.applied >= n) {
            tAct = now;
            fAct = s.frames;
          }
          const u = at();
          if (tAct !== null && u && u0 && (u.x !== u0.x || u.y !== u0.y)) {
            resolve({ picture: true, frames: s.frames - fAct, pictureMs: Math.round(now - tAct), ordersSent: (T.metrics().client?.orders?.sent ?? 0) - sent0, ring0, from: [u0.x, u0.y], to: [u.x, u.y] });
            return;
          }
          if (now - t0 > waitMs) {
            resolve({ picture: false, frames: null, pictureMs: null, ordersSent: (T.metrics().client?.orders?.sent ?? 0) - sent0, ring0, from: u0 ? [u0.x, u0.y] : null, to: null });
            return;
          }
          requestAnimationFrame(step);
        };
        requestAnimationFrame(step);
      }),
    { id: unit, x, y, waitMs },
  );
}

/** The order times (ms) the client measured since the ring had `ring0` entries (the ring keeps 256). */
export async function orderTimesSince(page, ring0) {
  const c = await clientStats(page);
  const ring = c?.orderMs ?? [];
  return ring.length >= ring0 ? ring.slice(ring0) : ring.slice(-1);
}

/** The plots to try beside a unit for a move (the terrain is not known to the scenario). */
export const AROUND = [[1, 0], [-1, 0], [0, 1], [0, -1], [1, 1], [-1, -1], [1, -1], [-1, 1], [2, 0], [-2, 0]];
