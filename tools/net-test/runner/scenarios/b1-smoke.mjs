// `b1-smoke` (N5.8, AC-N6.2, net-test 4.5, NT-D14): the B1 `smoke` of the playtest through the server, in the browser, as a
// player goes: sign in, a room of one's own with one AI seat, Start, `turns` turns played with the playtest's policy
// (`policy.mjs`, read only), a reload of the tab, and `after` more turns after it. Per phase: no stuck turn, no fatal
// error, no console error, the journal has lines, and after the reload the game is the same game (no new room, the same
// turn or later, the gateway's hash of the seat equals the client's).
import { createPlayer } from "../../../playtest/lib/policy.mjs";
import { JournalLog } from "../../../playtest/lib/journal.mjs";
import { asBig } from "../lib/json.mjs";
import { act, gatewaySeat, openPage, pageUrl, settle, state, until } from "../lib/browser.mjs";

export const meta = {
  id: "b1-smoke",
  ac: ["AC-N6.2"],
  needs: ["browser", "web", "admin"],
  core: ["fixture", "real"],
  quick: { turns: 10, after: 3 },
  full: { turns: 10, after: 5 },
};

export async function run(ctx) {
  const { expect, opts } = ctx;
  const nick = "smoker";
  const b = await openPage({ headed: false, logDir: ctx.logDir });
  const { page } = b;
  const journal = new JournalLog();
  const seenEnd = new Map();
  const observe = (s) => {
    journal.add(s);
    if (s?.endTurn) seenEnd.set(s.endTurn, s.status);
  };
  const player = createPlayer(page, { turnTimeoutMs: 120000, observe });
  const numbers = {};
  try {
    // ---- sign in, the room, Start
    await page.goto(pageUrl(ctx, { nick }));
    const first = await until(page, (x) => x.screen === "login" || x.screen === "lobby", 60000, "sign-in window or the lobby");
    if (first.screen === "login") await act(page, { kind: "login", nick, password: ctx.server.password });
    await until(page, (x) => x.screen === "lobby", 30000, "the lobby");
    await act(page, { kind: "createRoom" });
    const room = await until(page, (x) => x.screen === "room", 15000, "the room");
    await act(page, { kind: "roomSetting", field: "name", value: "B1 smoke" });
    await act(page, { kind: "roomSetting", field: "minorCivs", value: 0 });
    const open = (room.net?.room?.slots ?? []).filter((x) => x.who === "open");
    let ai = false;
    for (const slot of open) {
      await act(page, { kind: "slot", index: slot.index, occupant: ai ? "closed" : "ai" });
      ai = true;
    }
    await until(page, (x) => x.net?.room?.canStart === true, 15000, "Start is on");
    await act(page, { kind: "start" });
    const s0 = await signInAndWaitPlay(page);
    observe(s0);
    expect("the game is on screen after Start", s0.screen === "play", `turn ${s0.turn}`);
    const seat = await gatewaySeat(ctx, nick);
    expect("the gateway has the client's seat", !!seat);

    // ---- turns
    const t0 = s0.turn;
    const mid = await player.play(t0 + opts.turns);
    observe(mid);
    expect(`${opts.turns} turns played`, mid.turn >= t0 + opts.turns, `turn ${t0} -> ${mid.turn}`);
    expect("no stuck turn", !player.stuck, player.stuck ? JSON.stringify(player.stuck) : "");
    expect("the end-turn button changed its text", seenEnd.size >= 2, [...seenEnd.keys()].join("; "));
    expect("the journal has lines", journal.size() > 0, `${journal.size()} line(s)`);
    numbers.policy = player.policy;

    // ---- reload, same game
    const gameBefore = (await gatewaySeat(ctx, nick))?.game;
    const screens = new Set();
    await page.reload();
    let back = null;
    const tReload = Date.now();
    while (Date.now() - tReload < 30000 && !back) {
      const s = await state(page);
      if (s) {
        screens.add(s.screen);
        if (s.screen === "play" && s.status === "human") back = s;
      }
      await ctx.sleep(50);
    }
    expect("after the reload the game is back without the sign-in window", !!back && !screens.has("login"), `screens: ${[...screens].join(", ")}; ${Date.now() - tReload} ms`);
    if (!back) return { numbers };
    numbers.reload_ms = Date.now() - tReload;
    const seat2 = await gatewaySeat(ctx, nick);
    expect("the same game and seat after the reload", seat2?.game === gameBefore && seat2?.player === seat?.player, `game ${gameBefore} -> ${seat2?.game}`);
    expect("the turn is the same or later", back.turn >= mid.turn, `turn ${mid.turn} -> ${back.turn}`);
    const gh = seat2?.seat?.view_hash;
    const settledState = await settle(page, (x) => !!x.net?.viewHash && gh !== undefined && asBig(gh) === BigInt(`0x${x.net.viewHash}`), 5000);
    expect("the client's view hash equals the gateway's after the reload", !!settledState?.net?.viewHash && asBig(gh) === BigInt(`0x${settledState.net.viewHash}`), `client ${settledState?.net?.viewHash}`);

    // ---- on
    const end = await player.play(back.turn + opts.after);
    observe(end);
    expect(`${opts.after} more turns after the reload`, end.turn >= back.turn + opts.after && !player.stuck, `turn ${back.turn} -> ${end.turn}${player.stuck ? `; stuck ${JSON.stringify(player.stuck)}` : ""}`);
    const fatal = end.fatal ?? (await state(page))?.fatal;
    expect("no fatal error", !fatal, fatal ?? "");
    numbers.turns = { start: t0, before_reload: mid.turn, end: end.turn };
    numbers.journal = journal.summary();
  } finally {
    expect("no console errors in the page", b.errors.length === 0, b.errors.slice(0, 2).join(" | "));
    await page.screenshot({ path: `${ctx.logDir}/b1-smoke.png` }).catch(() => {});
    await b.close();
  }
  return { numbers };
}

/** From Start to the first human turn on the map (the loading screens in between). */
function signInAndWaitPlay(page) {
  return until(page, (x) => x.screen === "play" && x.status === "human", 300000, "the map");
}
