// `lobby` (lobby-ui 4.11, AC-N4.1, AC-N4.2; net-client N4.4): the screens of a server session, driven through
// the test port the way a player drives them: sign in (a wrong password, then the right one), the lobby, a room
// (settings, slots, "Ready", Start) with two players, then the game itself: both players play 5 turns on the
// game screen (select a unit, move it, end the turn) with no `NeedKeyframe` and no console error; a quick
// start, [Sign out]. The page is the plain one (no `fixture`, no `real`): it talks to the server on the same
// address, which runs the test world.
//
// The server refuses 10 wrong passwords from one address in 10 minutes and then blocks it, and a refused
// first socket is sometimes retried by the client (it ends with code 1006, no close frame, before the refusal
// is read): every page that has no token costs one or two refusals. So the scenario has few pages and signs out the page that is not yet in a
// game.
import { openPage, state, act, until } from "../lib/browser.mjs";
import { moveAUnit } from "../lib/game.mjs";

/** The password of `--dev`. */
const PASSWORD = "dev";

/** A player's page on the server's own address, with the test hooks. */
async function player(ctx, nick) {
  const b = await openPage({ headed: ctx.opts.headed, logDir: ctx.opts.logDir, consoleFile: `console-${nick}.log` });
  await b.page.goto(`${ctx.server.url}/?test=1&ui=interim&nick=${encodeURIComponent(nick)}`);
  return b;
}

/** The sign-in window (a wrong password first when asked: the server slows every refusal down, so once), then the lobby. */
async function signIn(b, nick, check, wrongFirst = false) {
  let s = await until(b.page, (x) => x.screen === "login" || x.screen === "lobby", 60000, `${nick}: sign-in window or lobby`);
  if (s.screen === "login") {
    check(`sign-in: the window opens without a red line before anything is typed (${nick})`, !s.net.loginError, s.net.loginError ?? "");
    if (wrongFirst) {
      await act(b.page, { kind: "login", nick, password: "not-the-password" });
      s = await until(b.page, (x) => x.screen === "login" && x.net?.loginError, 30000, `${nick}: the refusal`);
      check(`sign-in: a wrong password is refused with its text (${nick})`, s.net.loginError === "Wrong server password.", s.net.loginError);
    }
    await act(b.page, { kind: "login", nick, password: PASSWORD });
  }
  return until(b.page, (x) => x.screen === "lobby", 30000, `${nick}: the lobby`);
}

/** Until the page is in the game (the view and the catalog came). */
const inGame = (b, what) => until(b.page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 90000, what);

export async function run(ctx) {
  const started = Date.now();
  const result = { scenario: "lobby", pass: false, logs: ctx.opts.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const pages = [];
  const open = async (nick) => {
    const b = await player(ctx, nick);
    pages.push([nick, b]);
    return b;
  };
  let error = null;
  try {
    // ---- alice hosts a room
    const alice = await open("alice");
    const a = alice.page;
    let s = await signIn(alice, "alice", check, true);
    check("lobby: the three lists and the line", Array.isArray(s.net.rooms) && Array.isArray(s.net.games) && Array.isArray(s.net.saves) && s.net.conn.startsWith("Online"), `line "${s.net.conn}"`);

    await act(a, { kind: "createRoom" });
    s = await until(a, (x) => x.screen === "room", 15000, "the room");
    const room = s.net.room;
    check("room: alice is the host in the first slot", room.meHost && room.slots[0]?.nick === "alice" && room.slots[0]?.me, `host ${room.host}, ${room.slots.length} slots`);
    check("room: Start waits for a second civilization", room.canStart === false && /at least two/i.test(room.whyNot ?? ""), room.whyNot ?? "");
    const before = room.name;
    await act(a, { kind: "roomSetting", field: "name", value: "Friday night" });
    s = await until(a, (x) => x.net?.room?.name === "Friday night", 10000, "the new name, echoed by the server");
    check("room: a setting goes out and comes back from the server", before !== "Friday night", `${before} -> ${s.net.room.name}`);
    await act(a, { kind: "roomSetting", field: "turnMode", value: "simultaneous" });
    await act(a, { kind: "roomSetting", field: "minorCivs", value: 0 });
    s = await until(a, (x) => x.net?.room?.turnMode === "simultaneous" && x.net.room.minorCivs === 0, 10000, "two quick edits");
    check("room: two quick edits in a row both stay", true, `${s.net.room.turnMode}, ${s.net.room.minorCivs} city-states`);

    // ---- bob joins from the list
    const bob = await open("bob");
    const b = bob.page;
    s = await signIn(bob, "bob", check);
    s = await until(b, (x) => x.net?.rooms?.some((r) => r.name === "Friday night"), 15000, "the room in bob's list");
    const listed = s.net.rooms.find((r) => r.name === "Friday night");
    check("lobby: bob sees the room with its host", listed.host === "alice" && /^1\//.test(listed.players), `${listed.name} by ${listed.host}, ${listed.players}`);
    await act(b, { kind: "joinRoom", room: listed.id });
    s = await until(b, (x) => x.screen === "room", 15000, "bob in the room");
    check("room: bob is a guest in a slot", !s.net.room.meHost && s.net.room.slots.some((x) => x.me && x.nick === "bob"), `slots ${s.net.room.slots.map((x) => x.who).join(",")}`);
    s = await until(a, (x) => x.net?.room?.slots.some((y) => y.nick === "bob"), 15000, "alice sees bob");
    check("room: the host sees who is not ready", /waiting for: bob/i.test(s.net.room.whyNot ?? ""), s.net.room.whyNot ?? "");
    await act(b, { kind: "ready", value: true });
    s = await until(a, (x) => x.net?.room?.slots.find((y) => y.nick === "bob")?.ready === true, 15000, "bob ready");

    // The open slots are settled by the host (the server wants it), then Start.
    for (const slot of s.net.room.slots.filter((y) => y.who === "open")) {
      await act(a, { kind: "slot", index: slot.index, occupant: "closed" });
    }
    s = await until(a, (x) => x.net?.room?.slots.every((y) => y.who !== "open") && x.net.room.slots.find((y) => y.nick === "bob")?.ready, 15000, "the slots settled and bob still ready");
    check("room: Start is on when everybody is ready", s.net.room.canStart === true, s.net.room.whyNot ?? "ready");
    if (ctx.opts.screenshot) await a.screenshot({ path: `${ctx.opts.logDir}/lobby-room.png` }).catch(() => {});
    await act(a, { kind: "start" });
    const sa = await inGame(alice, "alice in the game");
    const sb = await inGame(bob, "bob in the game");
    check("start: both players reach the game", sa.turn === sb.turn, `alice turn ${sa.turn}, bob turn ${sb.turn}; ${sa.players} civilizations`);
    const stats = await a.evaluate(() => window.__civ5stats?.timing ?? {});
    result.numbers.timing = { attachedAtMs: stats.attachedAtMs ?? null, keyframeAtMs: stats.keyframeAtMs ?? null };
    check("loading: the stage times are in window.__civ5stats.timing", stats.attachedAtMs > 0 && stats.keyframeAtMs >= stats.attachedAtMs, JSON.stringify(result.numbers.timing));
    if (ctx.opts.screenshot) await a.screenshot({ path: `${ctx.opts.logDir}/lobby-alice-game.png` }).catch(() => {});
    check("game screen: the end-turn button, the units and the city are there and orders are taken", !!sa.endTurn && sa.units.length > 0 && sa.cities.length > 0 && sa.net.ordersEnabled === true, `button "${sa.endTurn}", ${sa.units.length} units, ${sa.cities.length} city, line "${sa.net.conn}"`);

    // ---- the game: both players play 5 turns on the game screen
    const TURNS = ctx.opts.lobbyTurns ?? 5;
    const first = sa.turn;
    const moves = { alice: [], bob: [] };
    const turnMs = [];
    for (let t = first; t < first + TURNS; t++) {
      const t0 = Date.now();
      for (const [nick, b] of [["alice", alice], ["bob", bob]]) {
        const now = await until(b.page, (x) => x.turn === t && x.status === "human", 60000, `${nick}: turn ${t}`);
        const moved = await moveAUnit(b.page, now);
        if (moved) moves[nick].push(moved);
        check(`turn ${t}: ${nick} moves a unit and the order settles`, !!moved, moved ? `unit ${moved.unit} ${moved.from} -> ${moved.to}, picture ${moved.pictureMs} ms, settled ${moved.answerMs} ms, ${moved.tried} plot(s) tried` : "no unit could move");
        if (nick === "alice" && t === first + 2 && ctx.opts.screenshot) await b.page.screenshot({ path: `${ctx.opts.logDir}/lobby-alice-turn.png` }).catch(() => {});
        await act(b.page, { kind: "endTurn", force: true });
      }
      const [na, nb] = await Promise.all([alice, bob].map((b) => until(b.page, (x) => x.turn === t + 1 && x.status === "human", 60000, `turn ${t + 1}`)));
      turnMs.push(Date.now() - t0);
      check(`turn ${t}: both players are in turn ${t + 1}`, na.turn === nb.turn, `alice ${na.turn}, bob ${nb.turn}, ${turnMs.at(-1)} ms`);
    }
    result.numbers.turnMs = turnMs;
    result.numbers.moves = moves;
    for (const [nick, b] of [["alice", alice], ["bob", bob]]) {
      const m = await b.page.evaluate(() => window.__civ5test.metrics());
      const c = m.client;
      const s2 = await state(b.page);
      check(`game: ${nick} never asked for a keyframe (no need_keyframe)`, c && c.needKeyframe.total === 0, c ? JSON.stringify(c.needKeyframe) : "no client metrics");
      check(`game: ${nick} took the diffs of ${TURNS} turns and the orders were all answered`, c && c.diffs > 0 && c.orders.sent > 0 && c.orders.applied + c.orders.rejected === c.orders.sent && s2.net.inflight === 0 && s2.net.overlay === 0, c ? `${c.diffs} diffs, orders sent ${c.orders.sent} applied ${c.orders.applied} rejected ${c.orders.rejected}, in flight ${s2.net.inflight}, layer ${s2.net.overlay}` : "");
      check(`game: ${nick}'s line stayed up`, s2.net.conn.startsWith("Online") && c.streamBreaks === 0, `line "${s2.net.conn}", ${c.streamBreaks} broken stream(s), ${c.attachedFresh} fresh attach(es)`);
      result.numbers[`orderMs_${nick}`] = c.orderMs;
    }
    if (ctx.opts.screenshot) await a.screenshot({ path: `${ctx.opts.logDir}/lobby-alice-after.png` }).catch(() => {});

    // ---- sign out returns to the sign-in window (carl, not yet in a game), then a quick start of one player
    const carl = await open("carl");
    s = await signIn(carl, "carl", check);
    if (ctx.opts.screenshot) await carl.page.screenshot({ path: `${ctx.opts.logDir}/lobby-list.png` }).catch(() => {});
    await act(carl.page, { kind: "signOut" });
    s = await until(carl.page, (x) => x.screen === "login", 30000, "carl back at the sign-in window");
    check("sign out: the sign-in window is back", s.net.phase === "login", `phase ${s.net.phase}`);
    await act(carl.page, { kind: "login", nick: "carl", password: PASSWORD });
    await until(carl.page, (x) => x.screen === "lobby", 30000, "carl in the lobby again");
    await act(carl.page, { kind: "quickStart" });
    const sc = await inGame(carl, "carl in a quick game");
    check("quick start: a game against the AI without the room window", sc.net.phase === "play" && sc.players >= 2, `${sc.players} civilizations, turn ${sc.turn}`);
  } catch (e) {
    error = e;
    // The screen the scenario was on, for the log.
    for (const [nick, b] of pages) {
      const last = await state(b.page).catch(() => null);
      if (last) result.numbers[`lastScreen_${nick}`] = `${last.screen}/${last.net?.phase}${last.net?.loginError ? ` (${last.net.loginError})` : ""}${last.fatal ? ` fatal: ${last.fatal}` : ""}`;
    }
  }
  for (const [nick, b] of pages) {
    result.consoleErrors.push(...b.errors.map((x) => `${nick}: ${x}`));
    await b.close();
  }
  check("no console errors", result.consoleErrors.length === 0, `${result.consoleErrors.length} error(s)${result.consoleErrors[0] ? `: ${result.consoleErrors[0]}` : ""}`);
  if (error) {
    check("no scenario error", false, error.message);
    result.error = error.stack ?? String(error);
  }
  result.pass = result.checks.every((c) => c.pass);
  result.durationMs = Date.now() - started;
  return result;
}
