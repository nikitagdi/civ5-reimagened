// `background` (N5.8, AC-N3.4, net-test 4.6): a game tab that stays in the background for 10 minutes, in a visible
// Chrome (the system's, or CHROMIUM_PATH). Alice plays in the headed Chrome: her game tab is hidden by bringing another
// tab of the window to the front (Playwright's pages are always "visible", so this browser is driven by
// `playtest/lib/chrome.mjs`). Bob is a headless client; the room is simultaneous with a fixed 10 s timer, so the turns
// go by the timer and the view changes all the time while alice's tab is hidden. Conditions:
//   - the gateway's `conn` of alice's seat is `online` at every look (every 5 s) and `lost_timeout_total` did not grow;
//   - after the return the view is current within `background.recover_ms` (2 s): alice's view hash equals the gateway's;
//   - no keyframe was asked for and no stream broke (the queue of batches was taken in, not replayed past the
//     threshold); the numbers of the queue and of the catch-up go to the report.
// If Chrome froze or discarded the tab (`document.wasDiscarded`, a `freeze` event) that is written down and the return
// is judged as a reload (AC-N3.3), not as the 2 s catch-up. Minutes: `--bg-minutes` (default 10). Memory Saver is on.
import { launchChrome } from "../../../playtest/lib/chrome.mjs";
import { asBig } from "../lib/json.mjs";
import { act, openPage, state, until } from "../lib/browser.mjs";

export const meta = {
  id: "background",
  ac: ["AC-N3.4"],
  needs: ["browser", "web", "headed", "admin"],
  core: ["fixture", "real"],
  quick: { minutes: 10, memory_saver: true },
  full: { minutes: 10, memory_saver: true },
};

const CHROME = "C:/Program Files/Google/Chrome/Application/chrome.exe";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const pct = (xs, p) => (xs.length ? [...xs].sort((a, b) => a - b)[Math.min(xs.length - 1, Math.floor((xs.length * p) / 100))] : null);

async function openAlice(memorySaver) {
  const exe = process.env.CHROMIUM_PATH || CHROME;
  const prefs = { performance_tuning: { high_efficiency_mode: { state: memorySaver ? 1 : 0, enabled: memorySaver }, memory_saver_mode_aggressiveness: memorySaver ? 2 : 0 } };
  const chrome = await launchChrome({ exe, prefs, args: ["--disable-features=CalculateNativeWinOcclusion", ...(memorySaver ? ["--enable-features=HighEfficiencyModeAvailable"] : [])] });
  const game = await chrome.tab();
  // The lifecycle as the page sees it (kept in the page, read on demand: a frozen page cannot answer).
  await game.addInitScript(`(() => {
    const log = (window.__bg = window.__bg || []);
    const add = (what) => log.push([Math.round(performance.now()), what, document.visibilityState]);
    for (const e of ["visibilitychange", "freeze", "resume"]) document.addEventListener(e, () => add(e));
    for (const e of ["pagehide", "pageshow", "online", "offline"]) window.addEventListener(e, () => add(e));
    add("init");
  })()`);
  const other = await chrome.tab("about:blank");
  await game.activate();
  return { chrome, game, other };
}

export async function run(ctx) {
  const { expect, opts, server } = ctx;
  const minutes = opts.bgMinutes ?? opts.minutes ?? 10;
  const limit = ctx.th.value("background.recover_ms");
  const numbers = { minutes, memorySaver: !!opts.memory_saver };
  let alice = null;
  let bob = null;
  try {
    alice = await openAlice(!!opts.memory_saver);
    numbers.browser = alice.chrome.version;
    bob = await openPage({ headed: false, logDir: ctx.logDir, consoleFile: "console-bob.log" });
    const A = alice.game;
    const B = bob.page;
    const base = server.url;
    await A.goto(`${base}/?test=1&nick=alice`);
    await B.goto(`${base}/?test=1&nick=bob`);
    for (const [nick, p] of [["alice", A], ["bob", B]]) {
      const s = await until(p, (x) => x.screen === "login" || x.screen === "lobby", 60000, `${nick}: sign-in`);
      if (s.screen === "login") await act(p, { kind: "login", nick, password: server.password });
      await until(p, (x) => x.screen === "lobby", 30000, `${nick}: the lobby`);
    }
    // The room: simultaneous turns of 10 s, two humans, the rest closed.
    await act(A, { kind: "createRoom" });
    await until(A, (x) => x.screen === "room", 15000, "the room");
    await act(A, { kind: "roomSetting", field: "turnMode", value: "simultaneous" });
    await act(A, { kind: "roomSetting", field: "minorCivs", value: 0 });
    await act(A, { kind: "roomSetting", field: "timer", value: "fixed:10" });
    await act(A, { kind: "roomSetting", field: "name", value: "Background" });
    const listed = await until(B, (x) => x.net?.rooms?.some((r) => r.name === "Background"), 15000, "the room in bob's list");
    await act(B, { kind: "joinRoom", room: listed.net.rooms.find((r) => r.name === "Background").id });
    await until(B, (x) => x.screen === "room", 15000, "bob in the room");
    await act(B, { kind: "ready", value: true });
    const s = await until(A, (x) => x.net?.room?.slots.find((y) => y.nick === "bob")?.ready === true, 15000, "bob ready");
    for (const slot of s.net.room.slots.filter((y) => y.who === "open")) await act(A, { kind: "slot", index: slot.index, occupant: "closed" });
    await until(A, (x) => x.net?.room?.canStart === true, 15000, "Start is on");
    await act(A, { kind: "start" });
    await until(A, (x) => x.screen === "play" && x.turn !== null, 120000, "alice in the game");
    await until(B, (x) => x.screen === "play" && x.turn !== null, 120000, "bob in the game");
    await sleep(15000);

    const find = async () => {
      const m = await server.admin.metrics();
      for (const [id, g] of Object.entries(m.games ?? {})) {
        for (const [player, seat] of Object.entries(g.seats ?? {})) if (seat.nick === "alice") return { game: id, player, seat, lost: m.server?.sessions?.lost_timeout_total ?? null };
      }
      return null;
    };
    const gw0 = await find();
    expect("the gateway has alice's seat", !!gw0 && gw0.seat.conn === "online", `${JSON.stringify(gw0?.seat?.conn)}`);
    if (!gw0) return { numbers };
    const sample = async () => {
      const [a, b] = await Promise.all([
        Promise.race([A.evaluate(() => { const st = window.__civ5stats ?? {}; return { visible: document.visibilityState, perf: performance.now(), at: st.netAtMs ?? null, turn: st.netTurn ?? null, line: st.netLine ?? null, frames: window.__civ5test.state().frames }; }), sleep(8000).then(() => "timeout")]).catch((e) => `error ${e.message}`),
        state(B).catch(() => null),
      ]);
      return { at: Date.now(), a, serverTurn: b?.turn ?? null };
    };
    const base0 = await sample();
    expect("visible: alice's client has the server's turn", typeof base0.a === "object" && base0.a.turn >= (base0.serverTurn ?? 0) - 1, JSON.stringify(base0.a).slice(0, 160));

    // ---- hide the game tab
    await alice.other.activate();
    await sleep(1000);
    const hiddenAt = Date.now();
    const vis = await A.evaluate(() => document.visibilityState).catch(() => "?");
    expect("hidden: the game tab is hidden (the other tab is in front)", vis === "hidden", `visibilityState ${vis}`);
    const endAt = hiddenAt + minutes * 60 * 1000;
    const serverTurns = new Map();
    const aliceSeen = new Map();
    const conns = new Map();
    const samples = [];
    let nextGw = 0;
    let unanswered = 0;
    while (Date.now() < endAt) {
      const x = await sample();
      samples.push(x);
      if (x.serverTurn !== null && !serverTurns.has(x.serverTurn)) serverTurns.set(x.serverTurn, x.at);
      if (typeof x.a === "object" && x.a.turn !== null) for (let t = x.a.turn; t >= 1; t--) { if (aliceSeen.has(t)) break; aliceSeen.set(t, x.at); }
      if (typeof x.a !== "object") unanswered++;
      if (Date.now() >= nextGw) {
        nextGw = Date.now() + 5000;
        const g = await find().catch(() => null);
        const c = g?.seat?.conn ?? "none";
        conns.set(c, (conns.get(c) ?? 0) + 1);
      }
      await sleep(1000);
    }
    const delays = [];
    for (const [t, at] of serverTurns) if (t > (base0.serverTurn ?? 0) && aliceSeen.has(t)) delays.push(Math.max(0, aliceSeen.get(t) - at));
    numbers.hidden = { serverTurnsSeen: serverTurns.size, delaysMs: { n: delays.length, p50: pct(delays, 50), p90: pct(delays, 90), max: delays.length ? Math.max(...delays) : null }, samples: samples.length, unanswered };
    numbers.gatewayConn = Object.fromEntries(conns);
    expect("the gateway's conn of alice's seat was Online at every look", conns.size === 1 && conns.has("online"), JSON.stringify(numbers.gatewayConn));
    expect("the game went on (the server's turns changed while the tab was hidden)", serverTurns.size >= Math.floor(minutes * 60 / 10 / 2), `${serverTurns.size} turn(s) seen`);

    // ---- bring it back
    const t0 = Date.now();
    await A.activate();
    const framesAt = (await A.evaluate(() => window.__civ5test.state().frames).catch(() => 0)) ?? 0;
    let framesResumed = null;
    let current = null;
    while (Date.now() - t0 < 30000 && (framesResumed === null || current === null)) {
      const a = await state(A).catch(() => null);
      if (a && framesResumed === null && a.frames > framesAt + 2) framesResumed = Date.now() - t0;
      if (a && a.net?.viewHash && current === null) {
        const g = await find().catch(() => null);
        if (g && asBig(g.seat.view_hash) === BigInt(`0x${a.net.viewHash}`)) current = Date.now() - t0;
      }
      await sleep(50);
    }
    const visAfter = await A.evaluate(() => document.visibilityState).catch(() => "?");
    numbers.return = { framesResumedMs: framesResumed, viewCurrentMs: current, visibilityAfter: visAfter };
    // Still hidden after the activation: the window itself is covered or minimised, and the run says nothing.
    expect("the tab is visible again after the activation (the window is not covered)", visAfter === "visible", `visibilityState ${visAfter}`);
    const discarded = await A.evaluate(() => document.wasDiscarded).catch(() => null);
    const life = await A.evaluate(() => (window.__bg || []).filter((e) => e[1] === "freeze" || e[1] === "resume" || e[1] === "pageshow").length).catch(() => null);
    numbers.lifecycle = { wasDiscarded: discarded, freezeResumeEvents: life };
    expect(`the view is current within ${limit} ms of the return (the view hash equals the gateway's)`, current !== null && current <= limit, `${current} ms`);
    const gw1 = await find();
    expect("no session was lost to a timeout while the tab was hidden", gw1 && gw1.lost === gw0.lost, `lost_timeout_total ${gw0.lost} -> ${gw1?.lost}`);
    const m = await A.evaluate(() => window.__civ5test.metrics()).catch(() => null);
    const c = m?.client;
    numbers.queue = { batches: c?.batches, diffs: c?.diffs, keyframes: c?.keyframes, attachedFresh: c?.attachedFresh, attachedReplay: c?.attachedReplay, needKeyframe: c?.needKeyframe?.total, streamBreaks: c?.streamBreaks, bytesIn: c?.bytesIn };
    expect("the client is in the game, no keyframe was asked for, no stream broke", (await state(A))?.screen === "play" && (c?.needKeyframe?.total ?? 1) === 0 && (c?.streamBreaks ?? 1) === 0, JSON.stringify(numbers.queue));
    numbers.series = samples.filter((_, i) => i % 15 === 0).map((x) => ({ t: Math.round((x.at - hiddenAt) / 1000), server: x.serverTurn, alice: typeof x.a === "object" ? x.a.turn : x.a, line: typeof x.a === "object" ? x.a.line : null }));
  } finally {
    const errs = [...(alice?.game.errors ?? []), ...(bob?.errors ?? [])];
    expect("no console errors", errs.length === 0, `${errs.length} error(s)${errs[0] ? `: ${errs[0]}` : ""}`);
    await alice?.chrome.close().catch(() => {});
    await bob?.close().catch(() => {});
  }
  return { numbers };
}
