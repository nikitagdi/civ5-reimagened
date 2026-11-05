// `background` (net-client NC12, N4.7; spike): a game tab that is hidden for minutes.
// Visible, headed Chrome (the system's Chrome, or CHROMIUM_PATH): alice's game tab is hidden by bringing another
// tab of the same window to the front, for `--bg-minutes` (10); "Memory Saver" is switched on or off in the
// profile (`--bg-memory-saver on|off`); `--bg-mode frozen` also freezes the hidden tab through the DevTools
// (`Page.setWebLifecycleState`), the way Chrome freezes a tab that stays hidden. Bob plays in a headless
// browser and is the clock of the server: the turns end by a fixed timer (10 s), so the view changes all the
// time while alice's tab is hidden.
//
// What it measures (numbers, no pass/fail beyond "the tab came back with the right view and no error"):
// - the freshness of the hidden tab's view: for every turn the server starts (bob sees it), how long until
//   alice's client has it (`window.__civ5stats.netTurn`, which the safety timer publishes);
// - the gaps of that timer (how slow the browser makes it), the lifecycle events of the tab (`freeze`, `resume`,
//   `visibilitychange`), the line of the client (`Online`/`Lost`...), the attaches it made;
// - the return: the time until frames run again and until the screen shows the server's turn.
// Run by hand: node tools/playtest/run.mjs --scenario background --headed --web-root ... [--bg-minutes 10]
import { openPage, act, until, state } from "../lib/browser.mjs";
import { launchChrome } from "../lib/chrome.mjs";

const PASSWORD = "dev";
const CHROME = "C:/Program Files/Google/Chrome/Application/chrome.exe";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const pct = (xs, p) => (xs.length ? [...xs].sort((a, b) => a - b)[Math.min(xs.length - 1, Math.floor((xs.length * p) / 100))] : null);

/** Alice's browser: the system's Chrome with its own profile (Memory Saver on or off), its first tab for the game and another tab to come in front. */
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

/** chrome://discards lists the tabs and their lifecycle (Active, Frozen, Discarded). */
async function discards(chrome) {
  let tab = null;
  try {
    tab = await chrome.tab("chrome://discards");
    await sleep(2000);
    const text = await tab.evaluate(() => document.body.innerText);
    return text.split("\n").filter((l) => /127\.0\.0\.1|Frozen|Discarded|Active|Visible|Hidden/i.test(l)).slice(0, 14);
  } catch (e) {
    return [`chrome://discards: ${e.message}`];
  } finally {
    await tab?.close().catch(() => {});
  }
}
export async function run(ctx) {
  const started = Date.now();
  const minutes = ctx.opts.bgMinutes ?? 10;
  const mode = ctx.opts.bgMode ?? "plain";
  const memorySaver = (ctx.opts.bgMemorySaver ?? "off") === "on";
  const result = { scenario: "background", pass: false, logs: ctx.opts.logDir, checks: [], numbers: { minutes, mode, memorySaver }, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  let alice = null;
  let bob = null;
  let error = null;
  try {
    alice = await openAlice(memorySaver);
    result.numbers.browser = alice.chrome.exe;
    result.numbers.version = alice.chrome.version;
    bob = await openPage({ headed: false, logDir: ctx.opts.logDir, consoleFile: "console-bob.log" });
    const base = ctx.server.url;
    await alice.game.goto(`${base}/?test=1&ui=interim&nick=alice`);
    await bob.page.goto(`${base}/?test=1&ui=interim&nick=bob`);
    const A = alice.game;
    const B = bob.page;
    for (const [nick, p] of [["alice", A], ["bob", B]]) {
      const s = await until(p, (x) => x.screen === "login" || x.screen === "lobby", 60000, `${nick}: sign-in`);
      if (s.screen === "login") await act(p, { kind: "login", nick, password: PASSWORD });
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
    let s = await until(A, (x) => x.net?.room?.slots.find((y) => y.nick === "bob")?.ready === true, 15000, "bob ready");
    for (const slot of s.net.room.slots.filter((y) => y.who === "open")) await act(A, { kind: "slot", index: slot.index, occupant: "closed" });
    await until(A, (x) => x.net?.room?.canStart === true, 15000, "Start is on");
    await act(A, { kind: "start" });
    await until(A, (x) => x.screen === "play" && x.turn !== null, 90000, "alice in the game");
    await until(B, (x) => x.screen === "play" && x.turn !== null, 90000, "bob in the game");

    // ---- visible baseline: the view follows the server, frames run
    await sleep(25000);
    const sample = async () => {
      const [a, b] = await Promise.all([
        Promise.race([A.evaluate(() => { const st = window.__civ5stats ?? {}; return { visible: document.visibilityState, perf: performance.now(), at: st.netAtMs ?? null, turn: st.netTurn ?? null, line: st.netLine ?? null, attaches: st.netAttaches ?? null, bytes: st.netBytesIn ?? null, revision: st.netRevision ?? null, frames: window.__civ5test.state().frames }; }), sleep(8000).then(() => "timeout")]).catch((e) => `error ${e.message}`),
        state(B).catch(() => null),
      ]);
      return { at: Date.now(), a, serverTurn: b?.turn ?? null };
    };
    const base0 = await sample();
    check("visible: alice's client has the server's turn", typeof base0.a === "object" && base0.a.turn >= (base0.serverTurn ?? 0) - 1, JSON.stringify(base0.a).slice(0, 200));
    const screenshotDir = ctx.opts.logDir;
    if (ctx.opts.screenshot) await A.screenshot(`${screenshotDir}/background-before.png`).catch(() => {});
    result.numbers.discardsBefore = await discards(alice.chrome);

    // ---- hide the game tab
    await alice.other.activate();
    await sleep(1000);
    const hiddenAt = Date.now();
    const vis = await A.evaluate(() => document.visibilityState).catch(() => "?");
    check("hidden: the game tab is hidden (the other tab is in front)", vis === "hidden", `visibilityState ${vis}`);
    if (mode === "frozen") {
      await A.setLifecycle("frozen").catch((e) => (result.numbers.freezeError = e.message));
    }
    // The server's turn changes are seen by bob (his page is visible): their times are the reference.
    const serverTurns = new Map(); // turn -> time bob first showed it
    const aliceSeen = new Map(); // turn -> time alice's client first had it (by a sample)
    const samples = [];
    const endAt = hiddenAt + minutes * 60 * 1000;
    let unanswered = 0;
    while (Date.now() < endAt) {
      const x = await sample();
      samples.push(x);
      if (x.serverTurn !== null && !serverTurns.has(x.serverTurn)) serverTurns.set(x.serverTurn, x.at);
      if (typeof x.a === "object" && x.a.turn !== null) for (let t = x.a.turn; t >= 1; t--) { if (aliceSeen.has(t)) break; aliceSeen.set(t, x.at); }
      if (typeof x.a !== "object") unanswered++;
      await sleep(1000);
    }
    // How long after the server started turn t did alice's client have it?
    const delays = [];
    for (const [t, at] of serverTurns) if (t > (base0.serverTurn ?? 0) && aliceSeen.has(t)) delays.push(Math.max(0, aliceSeen.get(t) - at));
    const answered = samples.filter((x) => typeof x.a === "object");
    const gaps = [];
    for (let i = 1; i < answered.length; i++) if (answered[i].a.at !== null && answered[i - 1].a.at !== null && answered[i].a.at !== answered[i - 1].a.at) gaps.push(answered[i].a.at - answered[i - 1].a.at);
    const lines = new Set(answered.map((x) => x.a.line));
    const last = answered.at(-1);
    result.numbers.hidden = {
      serverTurnsSeen: serverTurns.size,
      delaysMs: { n: delays.length, p50: pct(delays, 50), p90: pct(delays, 90), max: delays.length ? Math.max(...delays) : null },
      timerGapMs: { n: gaps.length, p50: pct(gaps, 50), p90: pct(gaps, 90), max: gaps.length ? Math.max(...gaps) : null },
      samples: samples.length,
      unanswered,
      lineValuesSeen: [...lines],
      attachesFirstLast: [answered[0]?.a.attaches ?? null, last?.a.attaches ?? null],
      bytesFirstLast: [answered[0]?.a.bytes ?? null, last?.a.bytes ?? null],
      lastLag: last ? (last.serverTurn ?? 0) - (last.a.turn ?? 0) : null,
    };
    result.numbers.discardsAfter = await discards(alice.chrome);

    // ---- bring it back
    if (mode === "frozen") await A.setLifecycle("active").catch(() => {});
    const t0 = Date.now();
    await A.activate();
    const framesAt = (await A.evaluate(() => window.__civ5test.state().frames).catch(() => 0)) ?? 0;
    let framesResumed = null;
    let viewCurrent = null;
    while (Date.now() - t0 < 30000 && (framesResumed === null || viewCurrent === null)) {
      const a = await state(A).catch(() => null);
      const b = await state(B).catch(() => null);
      if (a && framesResumed === null && a.frames > framesAt + 2) framesResumed = Date.now() - t0;
      if (a && b && a.turn !== null && b.turn !== null && a.turn >= b.turn && viewCurrent === null) viewCurrent = Date.now() - t0;
      await sleep(50);
    }
    // A tab that is still hidden after the activation means the window itself is covered or minimised (the run says
    // nothing about the tab then): said in the check, not left to a null.
    const visAfter = await A.evaluate(() => document.visibilityState).catch(() => "?");
    result.numbers.return = { framesResumedMs: framesResumed, viewCurrentMs: viewCurrent, visibilityAfter: visAfter };
    check("return: the tab is visible again after the activation (the window is not covered)", visAfter === "visible", `visibilityState ${visAfter}`);
    check("return: frames run again and the screen shows the server's turn", framesResumed !== null && viewCurrent !== null, JSON.stringify(result.numbers.return));
    const fin = await state(A);
    const m = await A.evaluate(() => window.__civ5test.metrics()).catch(() => null);
    result.numbers.final = { turn: fin?.turn, screen: fin?.screen, conn: fin?.net?.conn, fresh: m?.client?.attachedFresh, replay: m?.client?.attachedReplay, needKeyframe: m?.client?.needKeyframe?.total, streamBreaks: m?.client?.streamBreaks };
    result.numbers.lifecycle = await A.evaluate(() => (window.__bg || []).slice(-40)).catch(() => []);
    check("return: the client is in the game, no keyframe was asked for, no stream broke", fin?.screen === "play" && (m?.client?.needKeyframe?.total ?? 1) === 0 && (m?.client?.streamBreaks ?? 1) === 0, JSON.stringify(result.numbers.final));
    if (ctx.opts.screenshot) await A.screenshot(`${screenshotDir}/background-after.png`).catch(() => {});
    result.numbers.series = samples.filter((_, i) => i % 15 === 0).map((x) => ({ t: Math.round((x.at - hiddenAt) / 1000), server: x.serverTurn, alice: typeof x.a === "object" ? x.a.turn : x.a, ageMs: typeof x.a === "object" && x.a.at !== null ? Math.round(x.a.perf - x.a.at) : null, line: typeof x.a === "object" ? x.a.line : null }));
  } catch (e) {
    error = e;
  }
  if (alice) {
    result.consoleErrors.push(...alice.game.errors);
    await alice.chrome.close().catch(() => {});
  }
  if (bob) {
    result.consoleErrors.push(...bob.errors);
    await bob.close();
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
