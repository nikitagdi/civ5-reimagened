// `m5-leave-rejoin` (live reports 2026-10-30): (1) after a reload and rejoin the players panel (MPList) shows the other human
// as connected; (2) "Exit to main menu" leaves no world under the lobby and the next game draws its map.
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8940 --scenario m5-leave-rejoin [--gfx webgl2]
// Needs a real-core server with the plain game data (`tools/playtest/m5/serve-one.sh`). Two clients (alice hosts, bob joins) in one
// room with two AI seats; bob reloads the page and rejoins (Network.IsPlayerConnected on both pages), then leaves through the Esc
// menu to the lobby (the lobby screenshot must not show the world; the renderer reports no terrain cells), then quick-starts a new
// game (terrain cells, units, ui.lua_errors == 0).
import { openPage, act, until, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { uiDriver } from "../lib/ui-driver.mjs";

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
  const result = { scenario: "m5-leave-rejoin", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => { result.checks.push({ name, pass: !!pass, detail: detail ?? "" }); console.log(`  ${pass ? "ok  " : "FAIL"} ${name}${detail ? ` (${detail})` : ""}`); };
  const base = o.attach ?? ctx.server?.url;
  if (!base) { check("options", false, "needs --attach <url of the real-core server>"); return result; }
  const view = { width: 1600, height: 900 };
  const pages = [];
  const open = async (nick) => {
    const b = await openPage({ headed: o.headed, logDir: o.logDir, consoleFile: `console-${nick}.log`, viewport: view });
    b.nick = nick;
    pages.push(b);
    await b.page.goto(appUrl(base, o, { nick, ui: "original", stats: "1" }), { timeout: 180000 });
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
  /** The connection state of every seat as the original UI's Lua sees it. */
  const conn = async (b) => {
    const ui = uiDriver(b);
    const [l] = await ui.lua(`local t = {} for i = 0, 3 do local ok, pc = pcall(function() return Players[i]:IsConnected() end) t[#t + 1] = i .. "=" .. tostring(Network.IsPlayerConnected(i)) .. (ok and ("/pc" .. tostring(pc)) or "/pcERR") end print(TAG .. "local=" .. tostring(Matchmaking.GetLocalID()) .. " " .. table.concat(t, " "))`);
    return l;
  };
  const cells = async (b) => (await until(b.page, () => true, 1000)).terrainCells ?? null;
  let error = null;
  try {
    // ---- 0: one human against AIs: the own seat is connected before and after a reload ----
    const solo = await open(`m5ls${Date.now() % 100000}`);
    await solo.page.goto(appUrl(base, o, { nick: solo.nick, ui: "original", stats: "1", autostart: "1", ai: "2", size: "small", seed: "7", mode: "simultaneous" }), { timeout: 180000 });
    const soloIn = async () => {
      const s0 = await until(solo.page, (x) => x.screen === "login" || x.screen === "play", 90000, "solo: sign-in or the game");
      if (s0.screen === "login") await act(solo.page, { kind: "login", nick: solo.nick, password: DEV_PASSWORD });
      await until(solo.page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 120000, "solo: in the game");
      await m2Up(solo);
      await sleep(5000);
    };
    await soloIn();
    const own = (t) => { const m = /local=(\d+)/.exec(t); return !!m && new RegExp(`${m[1]}=true`).test(t); };
    let sc = await conn(solo);
    check("solo: the own seat is connected", own(sc), sc);
    await solo.page.reload({ timeout: 180000 });
    await soloIn();
    sc = await conn(solo);
    check("solo after the reload: the own seat is connected", own(sc), sc);
    await shot(solo, "00-solo-after-reload");
    await solo.close();
    pages.splice(pages.indexOf(solo), 1);

    const nickA = `m5la${Date.now() % 100000}`;
    const nickB = `m5lb${Date.now() % 100000}`;
    const alice = await open(nickA);
    await signIn(alice);
    await act(alice.page, { kind: "createRoom" });
    await until(alice.page, (x) => x.screen === "room", 15000, "the room");
    const roomName = `m5 leave ${nickA}`;
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

    // ---- 1: reload bob's page and rejoin ----
    const both = (t) => /0=true/.test(t) && /1=true/.test(t);
    let c = await conn(bob);
    check("bob: both humans connected before the reload", both(c), c);
    await shot(bob, "01-before-reload");
    await bob.page.reload({ timeout: 180000 });
    const back = await until(bob.page, (x) => x.screen === "login" || x.screen === "play" || x.screen === "lobby", 90000, "bob after the reload");
    if (back.screen === "login") await act(bob.page, { kind: "login", nick: nickB, password: DEV_PASSWORD });
    await until(bob.page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 120000, "bob back in the game");
    await m2Up(bob);
    await sleep(6000);
    c = await conn(bob);
    check("bob after the reload: both humans connected", both(c), c);
    c = await conn(alice);
    check("alice after bob's reload: both humans connected", both(c), c);
    await shot(bob, "02-after-reload");
    let st = await stats(bob.page);
    check("bob: ui.lua_errors == 0 after the reload", st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);

    // ---- 2: leave to the lobby by the Esc menu ----
    const ui = uiDriver(bob);
    const need = async (re, what) => {
      for (let i = 0; i < 20; i++) {
        const r = (await ui.dump("")).find((x) => re.test(x.path));
        if (r) return r;
        await sleep(500);
      }
      console.log("  . top-level rows:", (await ui.dump("")).filter((x) => /Menu|Popup/.test(x.path)).map((x) => x.path).slice(0, 12).join(" | "));
      throw new Error(`${what}: no control matches ${re}`);
    };
    await bob.page.keyboard.press("Escape");
    await sleep(2500);
    await ui.click(await need(/GameMenu.*MainMenuButton$/, "Exit to main menu"));
    await sleep(2000);
    await ui.click(await need(/GameMenu.*ExitConfirm.*\/Yes$/, "exit confirmation Yes"));
    await until(bob.page, (x) => x.screen === "lobby", 60000, "bob in the lobby");
    await sleep(3000);
    await shot(bob, "03-lobby");
    const lobbyCells = await cells(bob);
    check("lobby: the renderer draws no terrain cells", !lobbyCells || lobbyCells.visible === 0, JSON.stringify(lobbyCells));

    // ---- 2b: Return to the game from the lobby ("Return"): the seat of the other human shows connected ----
    const lob = await until(bob.page, (x) => x.net?.games?.some((g) => g.seats.some((y) => y.yours)), 20000, "bob's seat in the lobby list");
    const mine = lob.net.games.find((g) => g.seats.some((y) => y.yours));
    await act(bob.page, { kind: "takeSeat", game: mine.id, player: mine.seats.find((y) => y.yours).player });
    await until(bob.page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 120000, "bob back by Return");
    await m2Up(bob);
    await sleep(6000);
    c = await conn(bob);
    check("bob after Return: both humans connected", both(c), c);
    c = await conn(alice);
    check("alice after bob's Return: both humans connected", both(c), c);
    await shot(bob, "03b-returned");
    st = await stats(bob.page);
    check("bob after Return: ui.lua_errors == 0", st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
    for (let k = 0; k < 3; k++) {
      await bob.page.keyboard.press("Escape");
      await sleep(2500);
      const rows = (await ui.dump("")).filter((x) => /GameMenu/.test(x.path));
      console.log(`  . Esc ${k + 1}: ${rows.length} GameMenu rows`);
      await shot(bob, `03c-esc${k + 1}`);
      if (rows.length) break;
    }
    await ui.click(await need(/GameMenu.*MainMenuButton$/, "Exit to main menu"));
    await sleep(2000);
    await ui.click(await need(/GameMenu.*ExitConfirm.*\/Yes$/, "exit confirmation Yes"));
    await until(bob.page, (x) => x.screen === "lobby", 60000, "bob in the lobby again");
    await sleep(2000);

    // ---- 3: a new game (bob gives up his seat in the old one first: a seat blocks a room of his own) ----
    st = await stats(bob.page);
    check("bob in the lobby again: ui.lua_errors == 0", st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
    await act(bob.page, { kind: "giveUpSeat" });
    await act(bob.page, { kind: "confirmGiveUp", yes: true });
    await sleep(1500);
    await act(bob.page, { kind: "createRoom" });
    await until(bob.page, (x) => x.screen === "room", 15000, "bob's own room");
    await act(bob.page, { kind: "roomSetting", field: "minorCivs", value: 0 });
    const rm = await until(bob.page, (x) => x.net?.room?.slots?.some((y) => y.who !== "open"), 15000, "the room slots");
    const openSlots = rm.net.room.slots.filter((y) => y.who === "open");
    for (const [i, slot] of openSlots.entries()) await act(bob.page, { kind: "slot", index: slot.index, occupant: i === 0 ? "ai" : "closed" });
    await until(bob.page, (x) => x.net?.room?.canStart === true, 15000, "Start is on");
    await act(bob.page, { kind: "start" });
    await until(bob.page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 120000, "bob in the new game");
    await m2Up(bob);
    await sleep(8000);
    await shot(bob, "04-new-game");
    const g2 = await until(bob.page, (x) => x.terrainCells && x.terrainCells.visible > 0 && x.terrainCells.unpaged === 0, 60000, "the new game's terrain cells").then((x) => x.terrainCells, () => null);
    check("new game: terrain cells visible", !!g2 && g2.visible > 0, JSON.stringify(g2));
    st = await stats(bob.page);
    check("new game: ui.lua_errors == 0", st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
    check("new game: units in the replica", (st.exe?.entity_units ?? 1) > 0, JSON.stringify(st.exe?.entity_units));
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
