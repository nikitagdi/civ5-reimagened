// `reload` (N5.8, AC-N3.3, net-test 4.6): the tab is reloaded in the middle of a game, the cache is warm. The sign-in
// window does not show (the client enters with its token), the game screen is back within `reload.ms` (5 s) from
// the reload and its view hash is the gateway's hash of that seat. Each reload is timed from the call to the state
// "screen play and the view hash of the gateway"; the marks of the client's own start (`__civ5stats.timing`) are
// printed. The reload is repeated `reloads` times; every one has to be in time.
import { asBig } from "../lib/json.mjs";
import { gatewaySeat, openPage, startBrowserGame, state } from "../lib/browser.mjs";

export const meta = {
  id: "reload",
  ac: ["AC-N3.3"],
  needs: ["browser", "web", "admin"],
  core: ["fixture", "real"],
  quick: { reloads: 2 },
  full: { reloads: 4 },
};

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export async function run(ctx) {
  const { expect, opts } = ctx;
  const limit = ctx.th.value("reload.ms");
  // A real GPU when there is a window to show (headed): the software GL of the headless browser needs about 12 s to tear
  // the first game page down, before the new document runs a line (measured: `reload()` 12.8 s headless, 0.04 s headed).
  // Without a window the time is judged from the moment the new document's scripts start, and the teardown is printed.
  const headed = !!ctx.caps.headed;
  const b = await openPage({ headed, logDir: ctx.logDir });
  const { page } = b;
  const numbers = { reloads: [], headed };
  try {
    await startBrowserGame(ctx, page, { nick: "reload", ai: 1, seed: 5 });
    const seat = await gatewaySeat(ctx, "reload");
    if (!expect("the gateway has the client's seat", !!seat)) return { numbers };
    // Let the game settle (the first diffs, the autosave of the turn) before the first reload.
    await sleep(2000);
    for (let i = 0; i < opts.reloads; i++) {
      const screens = new Set();
      const t0 = Date.now();
      await page.reload();
      let hit = null;
      let last = null;
      while (Date.now() - t0 < limit * 3) {
        const s = await state(page);
        if (s) {
          screens.add(s.screen);
          last = s;
          if (s.screen === "play" && s.net?.viewHash) {
            const g = (await ctx.server.admin.game(seat.game))?.seats?.[String(seat.player)];
            if (g && asBig(g.view_hash) === BigInt(`0x${s.net.viewHash}`)) {
              hit = { ms: Date.now() - t0, hash: s.net.viewHash };
              break;
            }
          }
        }
        await sleep(25);
      }
      const timing = await page.evaluate(() => window.__civ5stats?.timing ?? null).catch(() => null);
      // Where the time went before the page's own code ran (the document, the scripts) and what was slow.
      const nav = await page.evaluate(() => {
        const n = performance.getEntriesByType("navigation")[0];
        const slow = performance.getEntriesByType("resource").map((r) => ({ name: r.name.replace(/^https?:\/\/[^/]+/, ""), start: Math.round(r.startTime), ms: Math.round(r.duration), size: r.transferSize })).sort((a, b) => b.ms - a.ms).slice(0, 4);
        const script = performance.getEntriesByType("resource").find((r) => r.name.endsWith("/bootstrap.js"));
        return n ? { scriptStart: script ? Math.round(script.startTime) : 0, domContentLoaded: Math.round(n.domContentLoadedEventEnd), slow } : null;
      }).catch(() => null);
      numbers.reloads.push({ ms: hit?.ms ?? null, screens: [...screens], timing, nav });
      // Headed: the whole reload. Headless: from the first line of the new document (the teardown is the browser's).
      const judged = hit ? hit.ms - (headed ? 0 : nav?.scriptStart ?? 0) : null;
      numbers.reloads.at(-1).judged_ms = judged;
      expect(`reload ${i + 1}: the game is on screen with the gateway's view hash within ${limit} ms`, judged !== null && judged <= limit, hit ? `${judged} ms${headed ? "" : ` (${hit.ms} ms in all, ${nav?.scriptStart ?? "?"} ms of teardown before the new page ran)`}` : `no match in ${Date.now() - t0} ms; last screen ${last?.screen}, hash ${last?.net?.viewHash}`);
      expect(`reload ${i + 1}: no sign-in window (the token is used)`, !screens.has("login"), `screens seen: ${[...screens].join(", ")}`);
      await sleep(500);
    }
    const fatal = (await state(page))?.fatal;
    expect("no fatal error in the client", !fatal, fatal ?? "");
  } finally {
    expect("no console errors in the page", b.errors.length === 0, b.errors.slice(0, 2).join(" | "));
    await b.close();
  }
  return { numbers };
}
