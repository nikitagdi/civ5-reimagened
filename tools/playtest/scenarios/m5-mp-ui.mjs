// `m5-mp-ui` (live reports 2026-10-30, fork b4/fix-mp-ui): in a real 2-human simultaneous game
//  A. bob ends his turn: alice's Lua sees Events.RemotePlayerTurnEnd (the bottom queue MPTurnPanel and MPList listen to it) and
//     Players[bob]:HasReceivedNetTurnComplete();
//  B. with alice's "Multiplayer Auto End Turn" on (OptionsManager + CommitGameOptions -> civ5.options.MultiplayerAutoEndTurn = "1"),
//     her turn ends by itself once nothing blocks it (Network.HasSentNetTurnComplete without a click on END TURN);
//  C. alice kicks bob from the MP list: Events.MultiplayerGamePlayerDisconnected / MultiplayerGamePlayerUpdated fire on alice,
//     Players[bob]:IsHuman() is false and the kick button is gone;
//  D. bob founds his first city: alice's camera focus does not move.
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8960 --scenario m5-mp-ui
// Server: one real-core server, plain game data (tools/playtest/m5/serve-one.sh). Two clients (alice hosts, bob joins), no other seats.
import { openPage, act, until, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { uiDriver } from "../lib/ui-driver.mjs";
import { turnDriver } from "../lib/turns-woe.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function stats(page) {
  return page.evaluate(() => {
    const s = window.__civ5stats ?? {};
    return { ui: s.ui ?? null, replica: s.replica ?? null, exe: s.exe ?? null, m2: s.m2 ?? null, camera: s.camera ?? null };
  });
}

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "m5-mp-ui", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
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
  const waitLua = async (ui, code, ok, ms = 60000) => {
    const t0 = Date.now();
    let last = "";
    for (;;) {
      [last] = await ui.lua(code);
      if (ok(last ?? "")) return last;
      if (Date.now() - t0 > ms) return last;
      await sleep(700);
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
  const cam = async (b) => { const c = (await stats(b.page)).camera; return c ? { x: c.focus_x, z: c.focus_z, level: c.level } : null; };
  /** One line `sent=.. turn=.. rcA=.. rcB=..`. */
  const hand = async (ui, ids) => (await ui.lua(`local t = { "sent=" .. tostring(Network.HasSentNetTurnComplete()), "turn=" .. Game.GetGameTurn() }
    for _, i in ipairs({${ids.join(",")}}) do t[#t + 1] = "rc" .. i .. "=" .. tostring(Players[i]:HasReceivedNetTurnComplete()) end
    print(TAG .. table.concat(t, " "))`))[0];
  const field = (line, k) => (new RegExp(`${k}=(\\S+)`).exec(line ?? "") ?? [])[1];
  /** Clears every blocker of the seat the way a player does (the turn driver's handlers), without pressing END TURN when none blocks. */
  const clearAll = async (td, ui) => {
    for (let i = 0; i < 10; i++) {
      if (await ui.cityScreenShown()) { // founding a city opens the city screen: pick production there, close it
        if ((await ui.dump("/CityView/QueueBackground/Queue/SelectionScrollPanel/SelectionStacks/UnitButtonStack/Button")).length) { console.log(`  . production: ${await ui.pickProduction()}`); await sleep(3000); }
        for (let k = 0; k < 4 && (await ui.cityScreenShown()); k++) { await ui.closeCityScreen(); await sleep(1500); }
      }
      const bl = await td.clearOrdinary();
      if (bl.type === -1) return true;
      console.log(`  . blocker ${bl.type}: ${await ui.handleBlock(bl)}`);
      await sleep(1500);
    }
    return (await ui.blocking()).type === -1;
  };
  const SKIP_ALL = `for u in Players[Game.GetActivePlayer()]:Units() do UI.SelectUnit(u) Game.SelectionListGameNetMessage(GameMessageTypes.GAMEMESSAGE_PUSH_MISSION, GameInfoTypes.MISSION_SKIP, 0, 0, 0, false, false) end UI.ClearSelectionList()`;
  const FOUND = `local done = false
    for u in Players[Game.GetActivePlayer()]:Units() do
      if not done and u:IsFound() then
        UI.SelectUnit(u)
        Game.SelectionListGameNetMessage(GameMessageTypes.GAMEMESSAGE_PUSH_MISSION, GameInfoTypes.MISSION_FOUND, 0, 0, 0, false, false)
        UI.ClearSelectionList()
        done = true
      end
    end
    print(TAG .. tostring(done))`;
  let error = null;
  try {
    const nickA = `mpua${Date.now() % 100000}`;
    const nickB = `mpub${Date.now() % 100000}`;
    const alice = await open(nickA);
    await signIn(alice);
    await act(alice.page, { kind: "createRoom" });
    await until(alice.page, (x) => x.screen === "room", 15000, "the room");
    const roomName = `mp ui ${nickA}`;
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
    const tdA = turnDriver(alice, uiA, (n) => shot(alice, n)), tdB = turnDriver(bob, uiB, (n) => shot(bob, n));
    const ids = [idA, idB];

    // Alice's listeners: counts and the arguments of the events.
    const lis = await uiA.lua(`
      MPUI = { rte = 0, rteArgs = "", disc = 0, discArgs = "", upd = 0 }
      Events.RemotePlayerTurnEnd.Add(function(...) MPUI.rte = MPUI.rte + 1 MPUI.rteArgs = MPUI.rteArgs .. table.concat({...}, ",") .. ";" end)
      Events.MultiplayerGamePlayerDisconnected.Add(function(...) MPUI.disc = MPUI.disc + 1 MPUI.discArgs = MPUI.discArgs .. table.concat({...}, ",") .. ";" end)
      Events.MultiplayerGamePlayerUpdated.Add(function(...) MPUI.upd = MPUI.upd + 1 end)
      print(TAG .. "ok")`);
    check("alice: the listeners are installed", lis[0] === "ok", lis[0]);
    const counts = async () => (await uiA.lua(`print(TAG .. MPUI.rte .. "|" .. MPUI.rteArgs .. "|" .. MPUI.disc .. "|" .. MPUI.discArgs .. "|" .. MPUI.upd)`))[0].split("|");
    const turn1 = Number(field(await hand(uiA, ids), "turn"));

    // ---- turn 1: alice founds a city and clears her blockers; bob skips his units ----
    check("alice: the settler founds a city", (await uiA.lua(FOUND))[0] === "true");
    await sleep(3000);
    check("alice: nothing blocks END TURN", await clearAll(tdA, uiA));
    await uiB.lua(SKIP_ALL);
    await sleep(1500);
    check("bob: nothing blocks END TURN", await clearAll(tdB, uiB));
    const [c0] = [await counts()];
    check("A: alice has seen no RemotePlayerTurnEnd yet", Number(c0[0]) === 0, c0.join("|"));
    await shot(alice, "01-alice-before-bob-ends");

    // ---- A: bob ends his turn ----
    await uiB.clickEndTurn();
    const hA = await waitLua(uiA, `local t = {} t[1] = tostring(Players[${idB}]:HasReceivedNetTurnComplete()) print(TAG .. t[1] .. "|" .. MPUI.rte)`, (l) => l.startsWith("true"), 30000);
    const [rcB, rteN] = hA.split("|");
    await sleep(1500);
    const cA = await counts();
    result.numbers.A = { rc: rcB, rte: cA[0], args: cA[1] };
    check("A: alice's replica has bob's turn complete (HasReceivedNetTurnComplete)", rcB === "true", hA);
    check("A: Events.RemotePlayerTurnEnd fired on alice (>= 1)", Number(cA[0]) >= 1, `count ${cA[0]}, args ${cA[1]}`);
    check("A: alice has not ended her turn herself", field(await hand(uiA, ids), "sent") === "false");
    const strip = (await uiA.dump("MPTurnPanel")).map((r) => `${r.path.split("/").slice(-2).join("/")}:${r.text.trim()}`).filter((x) => !x.endsWith(":"));
    result.numbers.A.mpTurnPanel = strip.slice(0, 20);
    console.log(`  . MPTurnPanel rows: ${strip.slice(0, 12).join(" | ")}`);
    await shot(alice, "02-alice-bob-done");

    // ---- alice ends the turn by the button: turn 2 ----
    await tdA.endTurn();
    await until(bob.page, (x) => (x.turn ?? 0) >= turn1 + 1, 60000, "bob on turn 2");
    const turn2 = Number(field(await hand(uiA, ids), "turn"));
    check("turn 2 started on alice", turn2 === turn1 + 1, `${turn1} -> ${turn2}`);

    // ---- B: the option on; D: bob founds his first city meanwhile ----
    const opt = await uiA.lua(`OptionsManager.SetMultiplayerAutoEndTurnEnabled_Cached(true) OptionsManager.CommitGameOptions() print(TAG .. "set")`);
    const stored = await alice.page.evaluate(() => { try { return localStorage.getItem("civ5.options.MultiplayerAutoEndTurn"); } catch (e) { return `err ${e}`; } });
    check("B: the option is stored (civ5.options.MultiplayerAutoEndTurn == \"1\")", stored === "1", `${opt[0]} / ${stored}`);
    const camBefore = await cam(alice);
    const sentAtStart = field(await hand(uiA, ids), "sent");
    const t0 = Date.now();
    check("D: bob's settler founds a city", (await uiB.lua(FOUND))[0] === "true");
    // alice: no click, no orders; poll for her own turn complete
    let sentAt = null, hl = "";
    for (let i = 0; i < 60 && sentAt === null; i++) {
      hl = await hand(uiA, ids);
      if (field(hl, "sent") === "true" || Number(field(hl, "turn")) > turn2) sentAt = Date.now() - t0;
      else await sleep(500);
    }
    result.numbers.B = { sentAtStart, autoEndAfterMs: sentAt, line: hl };
    check("B: alice's turn ends by itself, no click (HasSentNetTurnComplete)", sentAt !== null && field(hl, "turn") === String(turn2) && field(hl, "sent") === "true", `sent at start ${sentAtStart}; after ${sentAt} ms; ${hl}`);
    await shot(alice, "03-alice-auto-ended");
    await sleep(4000);
    const camAfter = await cam(alice);
    result.numbers.D = { before: camBefore, after: camAfter };
    const bobCities = Number((await uiB.lua(`print(TAG .. Players[Game.GetActivePlayer()]:GetNumCities())`))[0]);
    const aliceSeesBobCity = Number((await uiA.lua(`print(TAG .. Players[${idB}]:GetNumCities())`))[0]);
    check("D: bob has a city, alice's replica too", bobCities === 1 && aliceSeesBobCity === 1, `bob ${bobCities}, alice sees ${aliceSeesBobCity}`);
    const moved = camBefore && camAfter ? Math.hypot(camAfter.x - camBefore.x, camAfter.z - camBefore.z) : NaN;
    check("D: alice's camera did not jump when bob founded a city", moved < 0.5, `focus ${JSON.stringify(camBefore)} -> ${JSON.stringify(camAfter)}, moved ${moved}`);
    await shot(alice, "04-alice-after-bob-city");
    await shot(bob, "04-bob-city");

    // bob finishes the turn: the turn advances (alice already ended hers)
    check("bob: nothing blocks END TURN", await clearAll(tdB, uiB));
    for (let i = 0; i < 6 && (await uiB.cityScreenShown()); i++) { await uiB.closeCityScreen(); await sleep(1500); }
    await shot(bob, "04-bob-before-end");
    await uiB.clickEndTurn();
    await until(alice.page, (x) => (x.turn ?? 0) >= turn2 + 1, 90000, "turn 3 on alice");
    await sleep(3000);
    check("the turn advanced without alice pressing END TURN", Number(field(await hand(uiA, ids), "turn")) === turn2 + 1);

    // ---- C: alice kicks bob ----
    const before = await counts();
    const kick = await need(uiA, /MPList.*\/KickButton$/, "alice: the kick button of the MP list");
    await uiA.click(kick);
    await sleep(2000);
    await shot(alice, "05-confirm-kick");
    const accept = await need(uiA, /ConfirmKick.*\/AcceptButton$/, "alice: the kick confirmation");
    await uiA.click(accept);
    const ai = await waitLua(uiA, `print(TAG .. tostring(Players[${idB}]:IsHuman()))`, (l) => l === "false", 90000);
    check("C: Players[bob]:IsHuman() is false on alice", ai === "false", ai);
    await sleep(3000);
    const after = await counts();
    result.numbers.C = { before, after };
    check("C: Events.MultiplayerGamePlayerDisconnected fired on alice for bob", Number(after[2]) > Number(before[2]) && after[3].split(";").some((a) => a.split(",")[0] === String(idB)), `count ${before[2]} -> ${after[2]}, args ${after[3]}`);
    check("C: Events.MultiplayerGamePlayerUpdated fired on alice", Number(after[4]) > Number(before[4]), `count ${before[4]} -> ${after[4]}`);
    const kicks = (await uiA.dump("MPList")).filter((r) => /\/KickButton$/.test(r.path));
    check("C: the kick button is gone from the MP list", kicks.length === 0, `${kicks.length} kick rows`);
    await shot(alice, "06-after-kick");
    const alive = await uiA.lua(`print(TAG .. "alive " .. Game.GetGameTurn())`);
    check("alice's UI answers after the kick", /^alive \d+/.test(alive[0] ?? ""), alive[0]);
    for (const b of [alice, bob]) {
      const st = await stats(b.page);
      result.numbers[`stats_${b.nick}`] = { lua_errors: st.ui?.lua_errors, diverged: st.replica?.diverged };
      if (b === alice) check(`${b.nick}: ui.lua_errors == 0, replica.diverged == 0`, st.ui?.lua_errors === 0 && st.replica?.diverged === 0, `${st.ui?.lua_errors} / ${st.replica?.diverged}`);
    }
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
