// `m5-mp-trade-kick` (live reports 2026-10-30): (1) a trade between two humans reaches the other player, who accepts it, and the deal
// is applied on both clients; (2) the host kicks the other human from the MP player list (the X button -> ConfirmKick -> Accept): the seat
// goes to the AI and the host's UI stays responsive.
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8950 --scenario m5-mp-trade-kick [--gfx webgl2]
// Needs a real-core server with DEV game data in which every human gets 300 gold once and meets the other humans on its first turn
// (a PlayerDoTurn hook appended to lekmod_policies.lua, as tools/playtest/m4/make-dev-gamedata.ps1 does). Two clients (alice hosts, bob
// joins), no AI or city-state seats.
import { openPage, act, until, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { uiDriver } from "../lib/ui-driver.mjs";
import { turnDriver } from "../lib/turns-woe.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function stats(page) {
  return page.evaluate(() => {
    const s = window.__civ5stats ?? {};
    return { ui: s.ui ?? null, replica: s.replica ?? null, exe: s.exe ?? null, m2: s.m2 ?? null };
  });
}

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "m5-mp-trade-kick", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => { result.checks.push({ name, pass: !!pass, detail: detail ?? "" }); console.log(`  ${pass ? "ok  " : "FAIL"} ${name}${detail ? ` (${detail})` : ""}`); };
  const base = o.attach ?? ctx.server?.url;
  if (!base) { check("options", false, "needs --attach <url of the real-core server>"); return result; }
  const view = { width: 1600, height: 900 };
  const pages = [];
  const open = async (nick) => {
    const b = await openPage({ headed: o.headed, logDir: o.logDir, consoleFile: `console-${nick}.log`, viewport: view });
    b.nick = nick;
    pages.push(b);
    await b.page.goto(appUrl(base, o, { nick, ui: "original", stats: "1", manual: "1" }), { timeout: 180000 });
    return b;
  };
  const shot = async (b, name) => { try { await b.page.bringToFront().catch(() => {}); await b.page.screenshot({ path: `${o.logDir}/${b.nick}-${name}.png`, timeout: 90000 }); } catch { /* gone */ } };
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
  /** Polls a Lua expression printed with TAG until `ok(line)` or the time is up; returns the last line. */
  const waitLua = async (ui, code, ok, ms = 60000) => {
    const t0 = Date.now();
    let last = "";
    for (;;) {
      [last] = await ui.lua(code);
      if (ok(last ?? "")) return last;
      if (Date.now() - t0 > ms) return last;
      await sleep(1000);
    }
  };
  const need = async (ui, re, what) => {
    for (let i = 0; i < 30; i++) {
      const r = (await ui.dump("")).find((x) => re.test(x.path));
      if (r) return r;
      await sleep(500);
    }
    throw new Error(`${what}: no control matches ${re}`);
  };
  let error = null;
  try {
    const nickA = `mpta${Date.now() % 100000}`;
    const nickB = `mptb${Date.now() % 100000}`;
    const alice = await open(nickA);
    await signIn(alice);
    await act(alice.page, { kind: "createRoom" });
    await until(alice.page, (x) => x.screen === "room", 15000, "the room");
    const roomName = `mp trade ${nickA}`;
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
    await act(alice.page, { kind: "start" });
    for (const b of [alice, bob]) await until(b.page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 120000, `${b.nick} in the game`);
    await Promise.all([m2Up(alice), m2Up(bob)]);
    await sleep(6000);
    const uiA = uiDriver(alice), uiB = uiDriver(bob);
    const idA = Number((await uiA.lua(`print(TAG .. Game.GetActivePlayer())`))[0]);
    const idB = Number((await uiB.lua(`print(TAG .. Game.GetActivePlayer())`))[0]);
    result.numbers.seats = { alice: idA, bob: idB };

    // ---- 0: one turn on both (the DEV hook gives the gold and the meeting on the first turn) ----
    const tdA = turnDriver(alice, uiA, (n) => shot(alice, n)), tdB = turnDriver(bob, uiB, (n) => shot(bob, n));
    await Promise.all([tdA.endTurn(), tdB.endTurn()]);
    const goldOf = async (ui, p) => Number((await ui.lua(`print(TAG .. Players[${p}]:GetGold())`))[0]);
    const met = await uiA.lua(`print(TAG .. tostring(Teams[Players[${idA}]:GetTeam()]:IsHasMet(Players[${idB}]:GetTeam())))`);
    const gA0 = await goldOf(uiA, idA), gB0 = await goldOf(uiB, idB);
    check("turn 1: the humans met and have gold", met[0] === "true" && gA0 >= 100 && gB0 >= 100, `met ${met[0]}, gold ${gA0} / ${gB0}`);

    // ---- 1, 2: alice proposes in the trade window (Events.OpenPlayerDealScreenEvent: the click on bob's leader), bob gets the proposal and
    // its notification, opens it (as the notification does) and accepts. First a declaration of friendship (gold between civilizations needs
    // one, CvDeal::IsPossibleToTradeItem), then 50 gold.
    const roundTrip = async (label, items, n) => {
      await uiA.lua(`Events.OpenPlayerDealScreenEvent(${idB})`);
      await sleep(2500);
      const [cnt] = await uiA.lua(`local d = UI.GetScratchDeal() ${items} print(TAG .. d:GetNumItems())`);
      check(`${label}: the items are on alice's table`, Number(cnt) > 0, cnt);
      await shot(alice, `${n}1-${label}-proposing`);
      await uiA.click(await need(uiA, /SimpleDiplo.*\/ProposeButton$/, `${label}: alice's Propose`));
      const proposed = await waitLua(uiB, `print(TAG .. tostring(UI.ProposedDealExists(${idA}, ${idB})))`, (l) => l === "true", 60000);
      check(`${label}: bob's replica holds alice's proposal (UI.ProposedDealExists)`, proposed === "true", proposed);
      const note = await waitLua(uiB, `local p = Players[${idB}] local n = 0 local want = Locale.ConvertTextKey("TXT_KEY_DEAL_OFFERED") for i = 0, p:GetNumNotifications() - 1 do if p:GetNotificationSummaryStr(i) == want then n = n + 1 end end print(TAG .. n)`, (l) => Number(l) > 0, 20000);
      check(`${label}: bob has the 'deal proposed' notification`, Number(note) > 0, note);
      const has = await uiA.lua(`print(TAG .. tostring(UI.HasMadeProposal(${idA})))`);
      check(`${label}: alice's UI.HasMadeProposal names bob`, Number(has[0]) === idB, has[0]);
      await shot(bob, `${n}2-${label}-received`);
      await uiB.lua(`Events.OpenPlayerDealScreenEvent(${idA})`);
      await sleep(2500);
      await shot(bob, `${n}3-${label}-bob-window`);
      await uiB.click(await need(uiB, /SimpleDiplo.*\/ProposeButton$/, `${label}: bob's Accept`));
      const gone = await waitLua(uiA, `print(TAG .. tostring(UI.ProposedDealExists(${idA}, ${idB})))`, (l) => l === "false", 60000);
      check(`${label}: the proposal is resolved on alice's replica`, gone === "false", gone);
      await sleep(3000);
    };
    await roundTrip("friendship", `d:AddDeclarationOfFriendship(${idA}) d:AddDeclarationOfFriendship(${idB})`, 1);
    const dof = [...(await uiA.lua(`print(TAG .. tostring(Players[${idA}]:IsDoF(${idB})) .. "/" .. tostring(Players[${idB}]:IsDoF(${idA})))`)),
      ...(await uiB.lua(`print(TAG .. tostring(Players[${idA}]:IsDoF(${idB})) .. "/" .. tostring(Players[${idB}]:IsDoF(${idA})))`))];
    check("friendship: the declaration is made on both clients", dof.every((x) => x === "true/true"), dof.join(" "));
    await roundTrip("gold", `d:AddGoldTrade(${idA}, 50)`, 2);
    const gA1 = await goldOf(uiA, idA), gB1 = await goldOf(uiB, idB);
    const gB1onA = await goldOf(uiA, idB), gA1onB = await goldOf(uiB, idA);
    result.numbers.gold = { before: [gA0, gB0], after: [gA1, gB1] };
    check("gold: the deal is applied, alice -50, bob +50", gA1 === gA0 - 50 && gB1 === gB0 + 50, `alice ${gA0} -> ${gA1}, bob ${gB0} -> ${gB1}`);
    check("gold: both clients agree on both golds", gB1onA === gB1 && gA1onB === gA1, `alice sees bob ${gB1onA}, bob sees alice ${gA1onB}`);
    await shot(alice, "04-deal-done");
    for (const b of [alice, bob]) {
      const st = await stats(b.page);
      check(`${b.nick}: ui.lua_errors == 0, replica.diverged == 0 after the trade`, st.ui?.lua_errors === 0 && st.replica?.diverged === 0, `${st.ui?.lua_errors} / ${st.replica?.diverged}`);
    }

    // ---- 3: the host kicks bob from the MP player list ----
    const kick = await need(uiA, /MPList.*\/KickButton$/, "alice: the kick button of the MP list");
    await uiA.click(kick);
    await sleep(2000);
    await shot(alice, "05-confirm-kick");
    const accept = await need(uiA, /ConfirmKick.*\/AcceptButton$/, "alice: the kick confirmation");
    check("the kick confirmation is on the screen", !!accept, accept.path);
    await uiA.click(accept);
    const ai = await waitLua(uiA, `print(TAG .. tostring(Players[${idB}]:IsHuman()))`, (l) => l === "false", 90000);
    check("bob's seat goes to the AI (Players[bob]:IsHuman() false on alice)", ai === "false", ai);
    await sleep(3000);
    await shot(alice, "06-after-kick");
    // The host's UI answers: a Lua call returns and a click opens the game menu (Esc) and closes it.
    const alive = await uiA.lua(`print(TAG .. "alive " .. Game.GetGameTurn())`);
    check("alice's UI answers after the kick", /^alive \d+/.test(alive[0] ?? ""), alive[0]);
    const modal = await uiA.dump("ConfirmKick");
    check("the confirmation is gone", !modal.some((r) => /AcceptButton$/.test(r.path)), `${modal.length} rows`);
    const st = await stats(alice.page);
    check("alice: ui.lua_errors == 0, replica.diverged == 0 at the end", st.ui?.lua_errors === 0 && st.replica?.diverged === 0, `${st.ui?.lua_errors} / ${st.replica?.diverged}`);
    const at = await tdA.turnNow();
    await tdA.endTurn();
    check("alice: a turn passes after the kick", (await tdA.turnNow()) > at, `${at} -> ${await tdA.turnNow()}`);
  } catch (e) {
    error = e;
    for (const b of pages) await shot(b, "failure");
  }
  for (const b of pages) result.consoleErrors.push(...b.errors);
  for (const b of pages) await b.close();
  if (error) { check("no scenario error", false, error.message); result.error = error.stack ?? String(error); }
  result.pass = result.checks.every((x) => x.pass);
  result.durationMs = Date.now() - started;
  return result;
}
