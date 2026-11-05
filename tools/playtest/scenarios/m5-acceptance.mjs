// `m5-acceptance` (M5 WO-J, AC-S3): the acceptance scenario of the original UI (EUI, `?ui=original`) on a real-core game, by mouse only.
// Two modes (M5_MODE=ai|mp|both, default both; `--turns` is the length of mode "ai", default 50; M5_MP_TURNS the one of mode "mp", default 8):
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8870 --attach-mp http://127.0.0.1:8872 --scenario m5-acceptance [--gfx webgl2]
// Servers (WSL, `tools/playtest/m5/serve.sh`): 8870 runs the DEV game data of `tools/playtest/m5/make-dev-gamedata-woj.ps1` (the M4 WO-E data:
// on turn 3 all majors get Printing Press + Animal Husbandry, meet, a city of another civ stands near our capital, we get a Caravan and a
// spy slot; plus +30 faith, +40 culture, +200 gold), 8872 the plain game data (two humans would run the turn-3 block twice).
// Mode "ai": one human against 2 AIs, 50 turns. The first city is founded with the settler panel's button, every production is chosen in
// the city screen, every research in the tech tree, policies in the policy window, the pantheon in its popup, the religion overview is
// opened from the faith string, a deal with the AI is made in the trade window, a proposal is made and a vote cast in the World Congress,
// a spy is moved from the espionage overview, a trade route is made from the caravan's window. END TURN blocks are handled the way a player
// does (`GetEndTurnBlockingType`). Checks: ui.lua_errors == 0, replica.diverged == 0 after every window and at the end; numbers: the turn
// times (END TURN click -> the new turn on the replica), the main-thread pause of the turn change, the bytes per turn.
// Mode "mp": two clients (alice hosts, bob joins) in one room on one server; both found a city, chat both ways, turns pass on both.
import { openPage, act, until, state, appUrl } from "../lib/browser.mjs";
import { uiDriver } from "../lib/ui-driver.mjs";
import { turnDriver } from "../lib/turns-woe.mjs";
import { woeSteps } from "../lib/steps-woe.mjs";
import { wojSteps, enterGame } from "../lib/steps-woj.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function stats(page) {
  return page.evaluate(() => {
    const s = window.__civ5stats ?? {};
    return { ui: s.ui ?? null, replica: s.replica ?? null, exe: s.exe ?? null, m2: s.m2 ?? null };
  });
}
const median = (a) => { const s = [...a].sort((x, y) => x - y); return s.length ? (s.length % 2 ? s[(s.length - 1) / 2] : (s[s.length / 2 - 1] + s[s.length / 2]) / 2) : null; };
const mean = (a) => (a.length ? a.reduce((x, y) => x + y, 0) / a.length : null);
const round = (x, d = 0) => (x === null ? null : Math.round(x * 10 ** d) / 10 ** d);

/** Counts what a client receives: WebSocket frames and the HTTP bodies of `/replica/{hash}` (the snapshots / patches of a turn). */
function traffic(page) {
  const t = { ws_rx: 0, ws_tx: 0, http_rx: 0, http_n: 0 };
  page.on("websocket", (ws) => {
    ws.on("framereceived", (f) => { t.ws_rx += f.payload.length; });
    ws.on("framesent", (f) => { t.ws_tx += f.payload.length; });
  });
  page.on("response", async (res) => {
    if (!res.url().includes("/replica/")) return;
    t.http_n += 1;
    const cl = Number(res.headers()["content-length"] ?? 0);
    t.http_rx += cl || (await res.body().then((x) => x.length).catch(() => 0));
  });
  return t;
}
const rxOf = (t) => t.ws_rx + t.http_rx;

/** Per-turn numbers of a client: wraps `td.endTurn()`; the turn times come from the driver, the rest from the page stats. */
function turnMeter(page, t) {
  const rows = [];
  return {
    rows,
    async around(fn) {
      const rx0 = rxOf(t), tx0 = t.ws_tx;
      const turn = await fn();
      const st = await stats(page);
      rows.push({ turn, rx: rxOf(t) - rx0, tx: t.ws_tx - tx0, switch_ms: st.replica?.turn_switch_ms ?? null, wait_ms: st.replica?.turn_wait_ms ?? null, mem_mb: st.replica?.memory_mb ?? null });
      return turn;
    },
  };
}

function summarize(td, meter) {
  const ms = td.timings.map((x) => x.ms);
  const rx = meter.rows.map((r) => r.rx);
  return {
    turns: ms.length,
    turnMs: { median: median(ms), worst: ms.length ? Math.max(...ms) : null, mean: round(mean(ms)) },
    switchMs: { median: median(meter.rows.map((r) => r.switch_ms ?? 0)), worst: Math.max(0, ...meter.rows.map((r) => r.switch_ms ?? 0)) },
    rxBytesPerTurn: { median: median(rx), mean: round(mean(rx)), worst: rx.length ? Math.max(...rx) : null },
    txBytesPerTurn: { median: median(meter.rows.map((r) => r.tx)), worst: Math.max(0, ...meter.rows.map((r) => r.tx)) },
    memoryMb: meter.rows.length ? meter.rows[meter.rows.length - 1].mem_mb : null,
    perTurn: meter.rows,
  };
}

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "m5-acceptance", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => { result.checks.push({ name, pass: !!pass, detail: detail ?? "" }); console.log(`  ${pass ? "ok  " : "FAIL"} ${name}${detail ? ` (${detail})` : ""}`); };
  const mode = process.env.M5_MODE ?? "both";
  const baseAi = o.attach ?? ctx.server?.url;
  const baseMp = o.attachMp ?? baseAi;
  if (!baseAi) { check("options", false, "m5-acceptance needs --attach <url of the real-core server with the dev game data> [--attach-mp <url>]"); return result; }
  try {
    if (mode !== "mp") await modeAi({ o, result, check, base: baseAi });
  } catch (e) { check("mode ai: no scenario error", false, e.message); result.error = e.stack ?? String(e); }
  try {
    if (mode !== "ai") await modeMp({ o, result, check, base: baseMp });
  } catch (e) { check("mode mp: no scenario error", false, e.message); result.error ??= e.stack ?? String(e); }
  result.pass = result.checks.every((c) => c.pass);
  result.durationMs = Date.now() - started;
  return result;
}

// ------------------------------------------------------------------------------------------------------------------ mode "ai"
async function modeAi({ o, result, check, base }) {
  const turns = o.turns ?? 50;
  const view = { width: 1600, height: 900 };
  const b = await openPage({ headed: o.headed, logDir: o.logDir, consoleFile: "console-ai.log", viewport: view });
  const shot = async (name) => { try { await b.page.screenshot({ path: `${o.logDir}/ai-${name}.png` }); } catch { /* gone */ } };
  const t = traffic(b.page);
  const num = (result.numbers.ai = {});
  let error = null;
  let diag = null; // {td, ui} for the failure report
  try {
    const t0 = Date.now();
    await enterGame(b, o, base, { autostart: "1", ai: "2", size: "small", seed: "7", mode: "simultaneous", manual: "1", ...(process.env.M5_NOMT ? { mt: "0" } : {}) }, "m5a", { act, until, appUrl }, stats);
    num.toUiMs = Date.now() - t0;
    let st = await stats(b.page);
    check("ai: the UI is up and live", st.m2?.phase === 2 && st.replica?.live === 1, JSON.stringify(st.m2));
    check("ai: ui.lua_errors == 0 after the start", st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
    const errors = async (what) => {
      const s = await stats(b.page);
      check(`ai: ui.lua_errors == 0 ${what}`, s.ui?.lua_errors === 0, `${s.ui?.lua_errors}`);
      return s;
    };
    const ui = uiDriver(b);
    const woe = woeSteps(b, ui, shot);
    const woj = wojSteps(b, ui, woe, view, shot);
    const td = turnDriver(b, ui, shot, { research: woj.researchViaTree, policy: woj.adoptViaWindow, block: woj.block });
    const meter = turnMeter(b.page, t);
    diag = { td, ui };
    const turn = (n) => meter.around(() => td.endTurn());

    // The first city: END TURN selects the settler, its panel's button founds the city.
    await ui.clickEndTurn();
    await sleep(9000);
    const citiesBefore = st.exe?.entity_cities ?? 0;
    check("ai: the settler panel has a found-city button", !!(await ui.unitAction(/FOUND/)));
    await sleep(12000);
    st = await stats(b.page);
    check("ai: a city was founded", (st.exe?.entity_cities ?? 0) === citiesBefore + 1, `cities ${citiesBefore} -> ${st.exe?.entity_cities}`);
    await shot("01-city-founded");
    // The city screen: its production (also the END TURN block of the following turns).
    await ui.clickEndTurn();
    await sleep(5000);
    check("ai: the city screen opens", await ui.cityScreenShown());
    await shot("02-city-screen");
    woj.note(`city screen: ${await woj.chooseInOpenScreen()}`);
    await errors("after the city screen");

    // The tech tree.
    const r0 = await woj.researchViaTree();
    woj.note(r0);
    check("ai: a tech was chosen in the tree", /^research: TECH_/.test(r0), r0);
    await shot("03-tech-tree");
    await errors("after the tech tree");

    // Turns until the dev block (turn 3) has run.
    while ((await td.turnNow()) < 4) await turn();
    await sleep(3000);
    const [probe] = await ui.lua(`
      local p = Players[Game.GetActivePlayer()]
      local lg = Game.GetActiveLeague()
      local ncar = 0 for u in p:Units() do if u:GetUnitType() == GameInfoTypes.UNIT_CARAVAN then ncar = ncar + 1 end end
      print(TAG .. string.format("turn=%d league=%s spies=%d caravans=%d faith=%d culture=%d gold=%d", Game.GetGameTurn(), tostring(lg ~= nil and not lg:IsNone()), #p:GetEspionageSpies(), ncar, p:GetFaith(), p:GetJONSCulture(), p:GetGold()))`);
    num.afterDevBlock = probe;
    check("ai: the dev game data ran (league, a spy, a caravan, faith, culture)", /league=true/.test(probe) && /spies=1/.test(probe) && /caravans=1/.test(probe) && /faith=[3-9]\d/.test(probe), probe);
    await errors("after the dev block");

    // K11: the pantheon from its notification; K10: a policy from the policy window; religion overview.
    const belief = await woj.foundPantheon();
    check("ai: a pantheon was founded in its popup", belief >= 0, `belief ${belief}`);
    await shot("04-pantheon");
    await errors("after the pantheon");
    const pol = await woj.adoptViaWindow();
    woj.note(pol);
    check("ai: a policy was adopted in the policy window", /: adopted/.test(pol), pol);
    await shot("05-policy");
    await errors("after the policy");
    const rel = await woj.openReligionOverview();
    check("ai: the religion overview opens from the faith string", rel);
    await errors("after the religion overview");

    // K12: a deal with the AI in the trade window.
    const [ai] = await ui.lua(`for i = 0, GameDefines.MAX_MAJOR_CIVS - 1 do local p = Players[i] if p and p:IsAlive() and not p:IsHuman() and not p:IsMinorCiv() and i ~= Game.GetActivePlayer() then print(TAG .. i) break end end`);
    await ui.lua("UI.ClearSelectionList()"); // the unit panel covers the leader screen's buttons
    await sleep(2000);
    const deal = await woj.dealWithAI(Number(ai));
    check("ai: a deal with the AI was made in the trade window (it got our embassy)", deal.made, JSON.stringify(deal));
    check("ai: the diplomacy overview is closed after the deal", await woe.hidden("DiploOverview"));
    await shot("06-deal");
    await errors("after the deal");

    // K13-K15: proposal, spy, trade route.
    const prop = await woe.propose();
    check("ai: a proposal was made in the World Congress", prop.remaining === "0" && prop.proposals.some((l) => l.startsWith("0:")), JSON.stringify(prop));
    await errors("after the proposal");
    const spy = await woe.spyMove("Beshbalik");
    check("ai: a spy was moved to a city from the espionage overview", /^10,19,/.test(spy), spy);
    await errors("after the spy");
    const tr = await woe.tradeRoute("Beshbalik");
    check("ai: a trade route was made from the caravan's window and is listed", tr.routes > 0 && tr.listed, JSON.stringify(tr));
    await errors("after the trade route");

    // The turns up to `turns`; the congress vote is cast when the session comes (it takes turns of the 50).
    const vote = async () => {
      const v = await woe.vote({ ...td, endTurn: () => turn(), turnNow: td.turnNow, clearOrdinary: td.clearOrdinary });
      check("ai: a vote was cast in the World Congress", v.after !== `${v.before},true`, JSON.stringify(v));
      await errors("after the vote");
    };
    await vote();
    while ((await td.turnNow()) < turns) {
      await turn();
      const at = await td.turnNow();
      if (at % 10 === 0) { await shot(`10-turn-${at}`); await errors(`at turn ${at}`); }
    }
    num.turnsPlayed = await td.turnNow();
    st = await stats(b.page);
    check(`ai: the game reached turn ${turns}`, (await td.turnNow()) >= turns, `turn ${await td.turnNow()}`);
    check("ai: ui.lua_errors == 0 at the end", st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
    check("ai: replica.diverged == 0 at the end", st.replica?.diverged === 0, `${st.replica?.diverged}`);
    Object.assign(num, summarize(td, meter));
    num.final = { ui: st.ui, replica: st.replica };
    num.handled = td.handled;
    num.notes = woe.notes.concat(woj.notes);
    const simp = Object.entries(st.exe ?? {}).filter(([k]) => k.startsWith("simp.")).map(([k, v]) => `${k.slice(5)}=${v}`);
    num.simpCalls = simp;
    console.log(`  ai: turn time median ${num.turnMs.median} ms, worst ${num.turnMs.worst} ms; rx/turn median ${num.rxBytesPerTurn.median} B, mean ${num.rxBytesPerTurn.mean} B; switch worst ${num.switchMs.worst} ms`);
  } catch (e) {
    error = e;
    await shot("failure");
    const last = await state(b.page).catch(() => null);
    if (last) num.lastScreen = `${last.screen}/${last.net?.phase}`;
    num.lastStats = await stats(b.page).catch(() => null);
    if (diag) {
      num.handled = diag.td.handled.slice(-12);
      num.cityState = await diag.ui.lua("for c in Players[Game.GetActivePlayer()]:Cities() do print(TAG .. 'queue=' .. c:GetOrderQueueLength() .. ' prod=' .. tostring(c:GetProductionNameKey()) .. ' resistance=' .. tostring(c:IsResistance()) .. ' blocking=' .. tostring(Players[Game.GetActivePlayer()]:GetEndTurnBlockingType())) end").catch((e) => String(e));
      num.cityViewButtons = await diag.ui.dump("/CityView").then((r) => r.filter((x) => /Button/.test(x.cls) && !/PlotButton/.test(x.path)).slice(0, 25).map((x) => `${x.path.slice(-60)} ${x.x},${x.y} ${x.w}x${x.h}${x.disabled ? " D" : ""} ${x.text.slice(0, 20)}`)).catch(() => null);
    }
  }
  result.consoleErrors.push(...b.errors);
  await b.close();
  check("ai: no console errors", b.errors.length === 0, `${b.errors.length} error(s)${b.errors[0] ? `: ${b.errors[0]}` : ""}`);
  if (error) { check("ai: no scenario error", false, error.message); result.error = error.stack ?? String(error); }
}

// ------------------------------------------------------------------------------------------------------------------ mode "mp"
async function modeMp({ o, result, check, base }) {
  const turns = Number(process.env.M5_MP_TURNS ?? 8);
  const view = { width: 1600, height: 900 };
  const num = (result.numbers.mp = {});
  const pages = [];
  const open = async (nick) => {
    const b = await openPage({ headed: o.headed, logDir: o.logDir, consoleFile: `console-mp-${nick}.log`, viewport: view });
    b.nick = nick;
    b.traffic = traffic(b.page);
    pages.push(b);
    await b.page.goto(appUrl(base, o, { nick, ui: "original", stats: "1", manual: "1", ...(process.env.M5_NOMT ? { mt: "0" } : {}) }), { timeout: 180000 });
    return b;
  };
  const shot = async (b, name) => { try { await b.page.bringToFront().catch(() => {}); await b.page.screenshot({ path: `${o.logDir}/mp-${b.nick}-${name}.png`, timeout: 90000 }); } catch { /* gone */ } };
  const signIn = async (b) => {
    const s = await until(b.page, (x) => x.screen === "login" || x.screen === "lobby", 60000, `${b.nick}: sign-in window or lobby`);
    if (s.screen === "login") await act(b.page, { kind: "login", nick: b.nick, password: DEV_PASSWORD });
    return until(b.page, (x) => x.screen === "lobby", 30000, `${b.nick}: the lobby`);
  };
  const m2Up = async (b) => {
    const t0 = Date.now();
    for (;;) {
      const st = await stats(b.page);
      if (st.m2?.phase === 2) return st;
      if (st.m2?.phase === -1) throw new Error(`${b.nick}: M2 failed (phase -1)`);
      if (Date.now() - t0 > 300000) throw new Error(`${b.nick}: M2 is not up: ${JSON.stringify(st.m2)}`);
      await sleep(500);
    }
  };
  let error = null;
  try {
    const nickA = `m5pa${Date.now() % 100000}`;
    const nickB = `m5pb${Date.now() % 100000}`;
    const alice = await open(nickA);
    await signIn(alice);
    await act(alice.page, { kind: "createRoom" });
    await until(alice.page, (x) => x.screen === "room", 15000, "the room");
    const roomName = `m5 acceptance ${nickA}`;
    await act(alice.page, { kind: "roomSetting", field: "name", value: roomName });
    await until(alice.page, (x) => x.net?.room?.name === roomName, 10000, "the room name");
    await act(alice.page, { kind: "roomSetting", field: "turnMode", value: "simultaneous" });
    await act(alice.page, { kind: "roomSetting", field: "minorCivs", value: 0 });
    const bob = await open(nickB);
    let s = await signIn(bob);
    s = await until(bob.page, (x) => x.net?.rooms?.some((r) => r.name === roomName), 15000, "the room in bob's list");
    await act(bob.page, { kind: "joinRoom", room: s.net.rooms.find((r) => r.name === roomName).id });
    await until(bob.page, (x) => x.screen === "room", 15000, "bob in the room");
    await until(alice.page, (x) => x.net?.room?.slots.some((y) => y.nick === nickB), 15000, "alice sees bob");
    await act(bob.page, { kind: "ready", value: true });
    s = await until(alice.page, (x) => x.net?.room?.slots.find((y) => y.nick === nickB)?.ready === true, 15000, "bob ready");
    for (const slot of s.net.room.slots.filter((y) => y.who === "open")) await act(alice.page, { kind: "slot", index: slot.index, occupant: "closed" });
    await until(alice.page, (x) => x.net?.room?.slots.every((y) => y.who !== "open") && x.net.room.canStart === true, 15000, "Start is on");
    const t0 = Date.now();
    await act(alice.page, { kind: "start" });
    for (const b of [alice, bob]) await until(b.page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 120000, `${b.nick} in the game`);
    await Promise.all([m2Up(alice), m2Up(bob)]);
    num.toUiMs = Date.now() - t0;
    await sleep(5000);
    const clients = [];
    for (const b of [alice, bob]) {
      const st = await stats(b.page);
      check(`mp ${b.nick}: the UI is up and live`, st.m2?.phase === 2 && st.replica?.live === 1, JSON.stringify(st.m2));
      check(`mp ${b.nick}: ui.lua_errors == 0 after the start`, st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
      const ui = uiDriver(b);
      const woe = woeSteps(b, ui, (n) => shot(b, n));
      const woj = wojSteps(b, ui, woe, view, (n) => shot(b, n));
      const td = turnDriver(b, ui, (n) => shot(b, n), { research: woj.researchViaTree, policy: woj.adoptViaWindow, block: woj.block });
      clients.push({ b, ui, woe, woj, td, meter: turnMeter(b.page, b.traffic) });
    }
    // Both found their city (END TURN selects the settler, the panel's button founds it).
    await Promise.all(clients.map(async (c) => {
      const before = (await stats(c.b.page)).exe?.entity_cities ?? 0;
      await c.ui.clickEndTurn();
      await sleep(9000);
      const t = await c.ui.unitAction(/FOUND/);
      await sleep(12000);
      const after = (await stats(c.b.page)).exe?.entity_cities ?? 0;
      check(`mp ${c.b.nick}: a city was founded with the settler panel (cities ${before} -> ${after})`, !!t && after > before, `${t}`);
    }));
    for (const c of clients) await shot(c.b, "01-founded");

    // Chat both ways: the chat entry (mouse + keyboard) -> the other client's `exe.chat_in` and console.
    const chatIn = async (b) => (await stats(b.page)).m2?.chat_in ?? 0;
    for (const [from, to] of [[alice, bob], [bob, alice]]) {
      const line = `hello from ${from.nick} ${Date.now() % 10000}`;
      const before = await chatIn(to);
      const ui = clients.find((c) => c.b === from).ui;
      const entry = await ui.find("", (x) => /ChatEntry$/.test(x.path) && x.cls.includes("EditBox"));
      if (!entry) throw new Error(`${from.nick}: no chat entry on the screen`);
      await ui.click(entry);
      await sleep(300);
      await from.page.keyboard.type(line, { delay: 20 });
      await sleep(300);
      await from.page.keyboard.press("Enter");
      let got = false;
      for (let i = 0; i < 40 && !got; i++) { await sleep(500); got = (await chatIn(to)) > before; }
      await sleep(1500);
      check(`mp chat: the line of ${from.nick} arrives at ${to.nick}`, got && to.console.map((l) => l.text).join("\n").includes(line), `chat_in ${before} -> ${await chatIn(to)}`);
    }
    await shot(alice, "02-chat");
    await shot(bob, "02-chat");

    // Turns pass on both clients (simultaneous: the turn goes on when both ended).
    for (let i = 0; i < turns; i++) {
      const at = await clients[0].td.turnNow();
      await Promise.all(clients.map((c) => c.meter.around(() => c.td.endTurn())));
      check(`mp: turn ${at} -> ${at + 1} on both clients`, (await clients[0].td.turnNow()) > at && (await clients[1].td.turnNow()) > at);
    }
    for (const c of clients) {
      const st = await stats(c.b.page);
      check(`mp ${c.b.nick}: ui.lua_errors == 0 at the end`, st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
      check(`mp ${c.b.nick}: replica.diverged == 0 at the end`, st.replica?.diverged === 0, `${st.replica?.diverged}`);
      num[c.b.nick] = { ...summarize(c.td, c.meter), final: { ui: st.ui, replica: st.replica }, handled: c.td.handled };
      await shot(c.b, "03-end");
    }
    const ta = await clients[0].td.turnNow(), tb = await clients[1].td.turnNow();
    check("mp: both clients are on the same turn", ta === tb, `${ta} / ${tb}`);
    console.log(`  mp: turn time median ${num[nickA].turnMs.median} ms (alice) / ${num[nickB].turnMs.median} ms (bob); rx/turn median ${num[nickA].rxBytesPerTurn.median} / ${num[nickB].rxBytesPerTurn.median} B`);
  } catch (e) {
    error = e;
    for (const b of pages) await shot(b, "failure");
    for (const b of pages) num[`lastStats_${b.nick}`] = await stats(b.page).catch(() => null);
  }
  for (const b of pages) result.consoleErrors.push(...b.errors);
  for (const b of pages) await b.close();
  check("mp: no console errors", pages.every((b) => b.errors.length === 0), pages.map((b) => `${b.nick} ${b.errors.length}${b.errors[0] ? `: ${b.errors[0]}` : ""}`).join("; "));
  if (error) { check("mp: no scenario error", false, error.message); result.error ??= error.stack ?? String(error); }
}
