// The game screen of a server session, played through the test port the way a player plays it.
import { act, until, settle, appUrl, INTERIM } from "./browser.mjs";

/** The password of the dev server (`--dev`); the page has no token the first time. */
export const DEV_PASSWORD = "dev";

/**
 * Starts a game through the server the way a player does with `?autostart=1`: sign in (typed once, the dev server
 * wants its password) and the quick start (a room of its own, the open slots to the AI, Start). Resolves with the
 * state at the first human turn on the game screen.
 * @param {import("playwright").Page} page
 * @param {{server: {url: string}, opts: object}} ctx
 * @param {{nick: string, ai?: number, seed?: number, size?: string, mode?: string, civ?: string, extra?: Record<string, string>}} p
 */
export async function startServerGame(page, ctx, p) {
  const o = ctx.opts;
  const q = { ...INTERIM, nick: p.nick, autostart: "1", ai: String(p.ai ?? o.ai), size: p.size ?? "duel", seed: String(p.seed ?? o.seed), ...(p.extra ?? {}) };
  if (p.mode) q.mode = p.mode;
  const civ = p.civ ?? o.civ;
  if (civ) q.civ = civ;
  await page.goto(appUrl(ctx.server.url, o, q));
  return signInAndWait(page, ctx, p.nick);
}

/** After a page load with `autostart`: the sign-in window if it shows, then the first human turn on the map. */
export async function signInAndWait(page, ctx, nick) {
  const o = ctx.opts;
  const first = await until(page, (x) => x.screen === "login" || x.screen === "play", o.lobbyTimeoutMs, "sign-in window or the game");
  if (first.screen === "login") await act(page, { kind: "login", nick, password: DEV_PASSWORD });
  return until(page, (x) => x.screen === "play" && x.status === "human", o.mapTimeoutMs, "map on screen");
}

/** The plots around a plot to try for a move (the map's terrain is not known to the scenario). */
const AROUND = [[1, 0], [-1, 0], [0, 1], [0, -1], [1, 1], [-1, -1], [1, -1], [-1, 1], [2, 0], [-2, 0]];

/**
 * One turn of a player on the game screen: select an own unit that has moves, right-click a plot beside it
 * until the server lets it go there, wait for the order to settle, end the turn. Returns what was seen.
 * @returns {Promise<{unit: number, from: number[], to: number[], pictureMs: number, answerMs: number, tried: number}|null>}
 */
export async function moveAUnit(page, s) {
  for (const u of s.units.filter((x) => x.movesLeft > 0)) {
    let tried = 0;
    for (const [dx, dy] of AROUND) {
      tried++;
      await act(page, { kind: "selectUnit", id: u.id });
      const t0 = Date.now();
      await act(page, { kind: "click", x: u.x + dx, y: u.y + dy, secondary: true });
      const at = (x) => x.units.find((y) => y.id === u.id);
      // The picture of the order comes with the next frame, before the server's answer (NC-D6).
      const shown = await settle(page, (x) => at(x) && (at(x).x !== u.x || at(x).y !== u.y), 1500);
      const pictureMs = Date.now() - t0;
      if (!shown || !at(shown) || (at(shown).x === u.x && at(shown).y === u.y)) {
        // Nothing moved: the order was refused (water, a mountain, rivals) and no picture was made, or it came
        // off. Let it settle, and try the next plot.
        await settle(page, (x) => x.net?.inflight === 0, 3000);
        continue;
      }
      // The order settles: the unit stays where the picture put it (no rollback), nothing is left in flight.
      const settled = await until(page, (x) => x.net?.inflight === 0 && x.net?.overlay === 0, 10000, "the order settles");
      const answerMs = Date.now() - t0;
      const end = at(settled);
      if (end && (end.x !== at(shown).x || end.y !== at(shown).y)) continue; // rolled back: another plot
      return { unit: u.id, from: [u.x, u.y], to: [at(shown).x, at(shown).y], pictureMs, answerMs, tried };
    }
  }
  return null;
}

