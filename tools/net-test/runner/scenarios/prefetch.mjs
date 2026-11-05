// `prefetch` (N5.8, AC-N2.3, net-test 4.6): the contexts of the human's units come to the client before it selects them.
// The real client in a browser, one AI, a channel of RTT 200 ms (the proxy):
//   - `selects` selections of units one after another (the panel with the unit's orders is ready each time) send no
//     query at all (`prefetch.queries` = 0): the contexts were prefetched by the focus;
//   - after the view changed (the turn went on) the first selection is ready within RTT + 50 ms
//     (`prefetch.first_select_over_rtt_ms`): a context that is not there yet costs at most one query.
// The time is the page's own (requestAnimationFrame): from the frame that took the click to the frame with the panel.
import { createPlayer } from "../../../playtest/lib/policy.mjs";
import { clientStats, openPage, queriesSent, startBrowserGame, state, until } from "../lib/browser.mjs";

export const meta = {
  id: "prefetch",
  ac: ["AC-N2.3"],
  needs: ["browser", "web", "proxy"],
  core: ["fixture", "real"],
  quick: { rtt_ms: 200, selects: 20, changes: 2 },
  full: { rtt_ms: 200, selects: 20, changes: 4 },
};

/** `selectUnit` of `id`, timed by the page: ms from the frame that took the click to the frame with the unit's panel. */
async function timedSelect(page, id, waitMs = 5000) {
  return page.evaluate(
    ({ id, waitMs }) =>
      new Promise((resolve) => {
        const T = window.__civ5test;
        const t0 = performance.now();
        const n = T.act({ kind: "selectUnit", id });
        let tAct = null;
        let ctx = null;
        const step = () => {
          const s = T.state();
          const now = performance.now();
          if (tAct === null && s.applied >= n) {
            tAct = now;
            ctx = s.net?.ctx ?? null;
          }
          if (tAct !== null && s.selectedUnit === id && s.unitPanel && !s.unitPanel.loading) {
            resolve({ ok: true, ms: Math.round(now - tAct), ctx });
            return;
          }
          if (now - t0 > waitMs) {
            resolve({ ok: false, ms: null, ctx });
            return;
          }
          requestAnimationFrame(step);
        };
        requestAnimationFrame(step);
      }),
    { id, waitMs },
  );
}

export async function run(ctx) {
  const { expect, opts } = ctx;
  const maxQueries = ctx.th.value("prefetch.queries");
  const overRtt = ctx.th.value("prefetch.first_select_over_rtt_ms");
  const link = await ctx.proxyLink();
  // A real GPU when there is a window to show: the software GL of the headless browser draws a frame in 100+ ms, which
  // is the client's pump interval, so every round trip it measures grows by that.
  const b = await openPage({ headed: !!ctx.caps.headed, logDir: ctx.logDir });
  const { page } = b;
  const numbers = {};
  try {
    let s = await startBrowserGame(ctx, page, { nick: "prefetch", ai: 1, seed: 11, link });
    await link.set({ rtt_ms: opts.rtt_ms, jitter_ms: 0 });
    await until(page, (x) => (x.net?.rttMs ?? 0) >= opts.rtt_ms * 0.8, 15000, "the client measures the slow channel").catch(() => {});
    numbers.client_rtt_ms = (await state(page))?.net?.rttMs ?? null;
    const rtt = Math.max(numbers.client_rtt_ms ?? 0, opts.rtt_ms);

    // ---- selections in a row: no query
    s = await state(page);
    const own = s.units.map((u) => u.id);
    expect("the human has units to select", own.length >= 2, `${own.length} unit(s)`);
    // Time for the focus to reach the server and the contexts to come back.
    await ctx.sleep(rtt * 4);
    const q0 = queriesSent(await clientStats(page));
    const times = [];
    const cx = [];
    for (let i = 0; i < opts.selects; i++) {
      const r = await timedSelect(page, own[i % own.length]);
      if (!r.ok) cx.push("timeout");
      else {
        times.push(r.ms);
        cx.push(r.ctx);
      }
    }
    const c1 = await clientStats(page);
    const sent = queriesSent(c1) - q0;
    numbers.selects = { n: times.length, ms_max: Math.max(...times, 0), ms_p50: [...times].sort((a, b) => a - b)[Math.floor(times.length / 2)] ?? null, ctx: [...new Set(cx)] };
    expect(`${opts.selects} selections: every panel was ready`, times.length === opts.selects, `${times.length}/${opts.selects}`);
    expect(`${opts.selects} selections send ${maxQueries} queries`, sent === maxQueries, `${sent} sent (${JSON.stringify(c1.queries)}), contexts at select: fresh ${c1.ctxFreshAtSelect}, stale ${c1.ctxStaleAtSelect}`);
    // The rare selection that has no context yet waits for the one the server pushes (a Focus is not a query).
    expect(`every panel is ready within RTT + ${overRtt} ms`, numbers.selects.ms_max <= rtt + overRtt, `max ${numbers.selects.ms_max} ms, p50 ${numbers.selects.ms_p50} ms, RTT ${rtt}`);

    // ---- the first selection after the view changed
    const player = createPlayer(page, { turnTimeoutMs: 120000, observe: () => {} });
    const firsts = [];
    for (let k = 0; k < opts.changes; k++) {
      const before = await state(page);
      const last = await player.play(before.turn + 1);
      if (player.stuck || last.status === "over") break;
      const now = await until(page, (x) => x.status === "human" && x.turn > before.turn, 60000, "the next human turn");
      const id = now.units.find((u) => u.movesLeft > 0)?.id ?? now.units[0]?.id;
      if (id === undefined) break;
      const r = await timedSelect(page, id, 8000);
      firsts.push({ turn: now.turn, ms: r.ms, ok: r.ok, ctx: r.ctx });
    }
    numbers.first_select = firsts;
    const worst = Math.max(...firsts.map((f) => f.ms ?? Infinity), 0);
    expect("a selection was timed after each change", firsts.length === opts.changes && firsts.every((f) => f.ok), JSON.stringify(firsts));
    expect(`the first selection after a change is ready within RTT + ${overRtt} ms`, firsts.length > 0 && worst <= rtt + overRtt, `worst ${worst} ms, RTT ${rtt}`);
    const fatal = (await state(page)).fatal;
    expect("no fatal error in the client", !fatal, fatal ?? "");
  } finally {
    expect("no console errors in the page", b.errors.length === 0, b.errors.slice(0, 2).join(" | "));
    await page.screenshot({ path: `${ctx.logDir}/prefetch.png` }).catch(() => {});
    await b.close();
  }
  return { numbers };
}
