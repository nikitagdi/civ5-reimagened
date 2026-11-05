// `m3-corners-chat` (WO-C, milestone M3): the corners of the original UI (EUI, the default game screen) in a real two-player game:
// K4 info corner (unit / city / resource / great people lists, tech panel), K5 diplomacy corner, player list and chat,
// K6 minimap, K7 "Your turn", other players' turn, MP turn panel and MP player list.
// Needs the real-core server (WSL, `tools/playtest/m2/server.ps1`) and `--attach <url>`:
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8730 --scenario m3-corners-chat [--gfx webgl2] [--headed]
// Flow: alice hosts a room (simultaneous turns, 2 humans), bob joins, both start -> both clients run the original UI ->
// screenshots of the corners -> alice types a chat line into the chat panel (mouse + keyboard) -> it arrives at bob (his
// `exe.chat_in` counter and the line in his console) -> a click on the minimap moves the camera -> the MP list.
// Checks: ui.lua_errors == 0 on both clients, replica.diverged == 0, the chat line arrived, the minimap click moved the
// camera. Screenshots go to the log dir.
import { openPage, act, until, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function stats(page) {
  return page.evaluate(() => {
    const s = window.__civ5stats ?? {};
    return { ui: s.ui ?? null, replica: s.replica ?? null, exe: s.exe ?? null, m2: s.m2 ?? null, camera: s.camera ?? null };
  });
}

// The city view (K8, WO-A) is not here yet: production is pushed by a Lua snippet of `?luadbg=` as in m2-k1k3.
const PRODUCTION_LUA = `
Events.SerialEventCityCreated.Add(function(hex, owner, id)
  local p = Players[owner]
  local city = p and p:GetCityByID(id)
  if city and owner == Game.GetActivePlayer() then
    pcall(function() Game.CityPushOrder(city, 0, GameInfoTypes.UNIT_WARRIOR, 0, false, true) end)
  end
end)
-- a city whose queue ran empty gets a warrior again at the start of the turn
Events.ActivePlayerTurnStart.Add(function()
  local p = Players[Game.GetActivePlayer()]
  pcall(function()
    for city in p:Cities() do
      if city:GetOrderQueueLength() == 0 then
        Game.CityPushOrder(city, 0, GameInfoTypes.UNIT_WARRIOR, 0, false, true)
      end
    end
  end)
end)`;

// What the turn events do, for the log (K7: "Your turn", the turn panel).
const TURN_LUA = `
print('M3 turn hooks loaded')
Events.ActivePlayerTurnStart.Add(function() print('M3 ActivePlayerTurnStart turn ' .. Game.GetGameTurn()) end)
Events.ActivePlayerTurnEnd.Add(function() print('M3 ActivePlayerTurnEnd') end)
Events.RemotePlayerTurnStart.Add(function() print('M3 RemotePlayerTurnStart') end)
Events.RemotePlayerTurnEnd.Add(function() print('M3 RemotePlayerTurnEnd') end)
Events.NewGameTurn.Add(function(t) print('M3 NewGameTurn ' .. t) end)`;

// Pixels of the original UI at 1600x900 (read off the screenshots).
const PX = {
  chatEntry: [1400, 184],
  minimapPoint: [1423, 762],
  endTurn: [1423, 682],
  foundCity: [463, 860],
  fortify: [255, 865],
  kickX: [1326, 306],
};

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "m3-corners-chat", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const base = o.attach ?? ctx.server?.url;
  if (!base) {
    check("options", false, "m3-corners-chat needs --attach <url of the real-core server>");
    return result;
  }
  const pages = [];
  const open = async (nick) => {
    const b = await openPage({ headed: o.headed, logDir: o.logDir, consoleFile: `console-${nick}.log`, viewport: { width: 1600, height: 900 } });
    pages.push([nick, b]);
    await b.page.goto(appUrl(base, o, { nick, stats: "1", luadbg: o.luadbg ?? PRODUCTION_LUA + TURN_LUA }));
    return b;
  };
  const shot = async (b, name, clip) => {
    try {
      await b.page.bringToFront().catch(() => {});
      await b.page.screenshot({ path: `${o.logDir}/${name}.png`, timeout: 90000, ...(clip ? { clip } : {}) });
    } catch (e) {
      console.log(`  (screenshot ${name} failed: ${String(e.message).split(String.fromCharCode(10))[0]})`);
    }
  };
  const signIn = async (b, nick) => {
    const s = await until(b.page, (x) => x.screen === "login" || x.screen === "lobby", 60000, `${nick}: sign-in window or lobby`);
    if (s.screen === "login") await act(b.page, { kind: "login", nick, password: DEV_PASSWORD });
    return until(b.page, (x) => x.screen === "lobby", 30000, `${nick}: the lobby`);
  };
  const m2Up = async (b, nick) => {
    const t0 = Date.now();
    for (;;) {
      const st = await stats(b.page);
      if (st.m2?.phase === 2) return st;
      if (st.m2?.phase === -1) throw new Error(`${nick}: M2 failed (phase -1)`);
      if (Date.now() - t0 > 180000) throw new Error(`${nick}: M2 is not up after 180 s: ${JSON.stringify(st.m2)}`);
      await sleep(500);
    }
  };
  let error = null;
  try {
    const nickA = `ca${Date.now() % 100000}`;
    const nickB = `cb${Date.now() % 100000}`;
    // ---- alice hosts, bob joins
    const alice = await open(nickA);
    let s = await signIn(alice, nickA);
    await act(alice.page, { kind: "createRoom" });
    s = await until(alice.page, (x) => x.screen === "room", 15000, "the room");
    const roomName = `m3 corners ${nickA}`;
    await act(alice.page, { kind: "roomSetting", field: "name", value: roomName });
    await until(alice.page, (x) => x.net?.room?.name === roomName, 10000, "the room name");
    await act(alice.page, { kind: "roomSetting", field: "turnMode", value: "simultaneous" });
    await act(alice.page, { kind: "roomSetting", field: "minorCivs", value: 0 });
    const bob = await open(nickB);
    s = await signIn(bob, nickB);
    s = await until(bob.page, (x) => x.net?.rooms?.some((r) => r.name === roomName), 15000, "the room in bob's list");
    await act(bob.page, { kind: "joinRoom", room: s.net.rooms.find((r) => r.name === roomName).id });
    await until(bob.page, (x) => x.screen === "room", 15000, "bob in the room");
    s = await until(alice.page, (x) => x.net?.room?.slots.some((y) => y.nick === nickB), 15000, "alice sees bob");
    await act(bob.page, { kind: "ready", value: true });
    s = await until(alice.page, (x) => x.net?.room?.slots.find((y) => y.nick === nickB)?.ready === true, 15000, "bob ready");
    for (const slot of s.net.room.slots.filter((y) => y.who === "open")) await act(alice.page, { kind: "slot", index: slot.index, occupant: "closed" });
    await until(alice.page, (x) => x.net?.room?.slots.every((y) => y.who !== "open") && x.net.room.canStart === true, 15000, "Start is on");
    const t0 = Date.now();
    await act(alice.page, { kind: "start" });
    for (const [nick, b] of [[nickA, alice], [nickB, bob]]) await until(b.page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 120000, `${nick} in the game`);
    const stA = await m2Up(alice, nickA);
    const stB = await m2Up(bob, nickB);
    result.numbers.toUiMs = Date.now() - t0;
    await sleep(5000);
    await shot(alice, "01-alice-start");
    await shot(bob, "01-bob-start");
    for (const [nick, b] of [[nickA, alice], [nickB, bob]]) {
      const st = await stats(b.page);
      result.numbers[`m2_${nick}`] = { m2: st.m2, exe: st.exe, replica: st.replica };
      check(`${nick}: M2 is up and live`, st.m2?.phase === 2 && st.replica?.live === 1, JSON.stringify({ m2: st.m2, live: st.replica?.live }));
      check(`${nick}: ui.lua_errors == 0 after the start`, st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
    }
    // ---- K5 chat: alice types a line into the chat panel; it arrives at bob
    const line = `hello from ${nickA} ${Date.now() % 10000}`;
    const chatIn = async (b) => (await stats(b.page)).m2?.chat_in ?? 0;
    const bobBefore = await chatIn(bob);
    await alice.page.mouse.move(...PX.chatEntry);
    await alice.page.mouse.click(...PX.chatEntry);
    await sleep(300);
    await alice.page.keyboard.type(line, { delay: 20 });
    await sleep(300);
    await shot(alice, "02-alice-chat-typed");
    await alice.page.keyboard.press("Enter");
    let got = false;
    for (let i = 0; i < 40 && !got; i++) {
      await sleep(500);
      got = (await chatIn(bob)) > bobBefore;
    }
    await sleep(1500);
    const bobLog = bob.console.map((l) => l.text).join("\n");
    check("chat: the line sent by alice arrives at bob (his chat_in grows)", got, `bob chat_in ${bobBefore} -> ${await chatIn(bob)}`);
    check("chat: bob has the text of the line", bobLog.includes(line), line);
    check("chat: alice sent one line", ((await stats(alice.page)).m2?.chat_sent ?? 0) >= 1, `${(await stats(alice.page)).m2?.chat_sent}`);
    await shot(alice, "03-alice-chat-sent");
    await shot(bob, "03-bob-chat-received");

    // ---- K6 minimap: a click moves the camera
    const cam = async (b) => (await stats(b.page)).camera ?? {};
    const c0 = await cam(alice);
    await alice.page.mouse.move(...PX.minimapPoint);
    await alice.page.mouse.click(...PX.minimapPoint);
    let c1 = c0;
    let moved = false;
    for (let i = 0; i < 20 && !moved; i++) {
      await sleep(500);
      c1 = await cam(alice);
      moved = Object.keys(c0).some((k) => /^m\d+$/.test(k) && Math.abs((c1[k] ?? 0) - c0[k]) > 1e-4);
    }
    const clicks = (await stats(alice.page)).m2?.minimap_clicks ?? 0;
    check("minimap: a click on it reaches the exe API (minimap_clicks)", clicks >= 1, `${clicks}`);
    check("minimap: the click moved the camera", moved, moved ? "view_proj changed" : "view_proj unchanged");
    await shot(alice, "04-alice-minimap-clicked");
    await shot(alice, "04-alice-minimap-zoom", { x: 1250, y: 650, width: 350, height: 250 });
    // ---- K4 info corner: the pulldown at the top left lists tech / units / cities / resources / great people
    await bob.page.mouse.move(22, 50);
    await bob.page.mouse.click(22, 50);
    await sleep(2000);
    await shot(bob, "05-bob-infocorner-pulldown");
    for (const [name, y] of [["tech", 233], ["units", 278], ["cities", 323], ["resources", 368], ["great-people", 413]]) {
      await bob.page.mouse.click(152, y);
      await sleep(2500);
      await shot(bob, `05-bob-infocorner-${name}`);
      const st = await stats(bob.page);
      check(`K4 ${name}: ui.lua_errors == 0`, st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
      await bob.page.mouse.click(22, 50);
      await sleep(1500);
    }
    await bob.page.mouse.click(152, 188); // the first entry closes the corner again
    await sleep(1500);
    // ---- K7 turn flow: both players end the turn (END TURN selects the units that need orders: found the city, fortify)
    const turnOf = async (b) => (await stats(b.page)).replica?.turn ?? 0;
    // END TURN while units wait for orders selects one: found the city with the first, fortify the others. In a
    // simultaneous game the turn goes on when BOTH have ended, so the clients take their rounds together.
    const t0turn = await turnOf(alice);
    let shotWaiting = false;
    for (let round = 0; round < 24 && (await turnOf(alice)) <= t0turn; round++) {
      for (const b of [alice, bob]) {
        await b.page.mouse.click(...PX.endTurn);
        await sleep(2000);
      }
      for (const b of [alice, bob]) {
        await b.page.mouse.click(...(round === 0 ? PX.foundCity : PX.fortify));
        await sleep(round === 0 ? 4000 : 2000);
      }
      if (round === 3 && !shotWaiting) {
        shotWaiting = true;
        await shot(alice, "06-alice-ended-waiting");
        await shot(bob, "06-bob-waiting");
      }
    }
    const endedA = (await stats(alice.page)).m2?.turn_complete;
    const endedB = (await stats(bob.page)).m2?.turn_complete;
    for (let i = 0; i < 200 && (await turnOf(alice)) <= t0turn; i++) await sleep(300);
    await sleep(400);
    await shot(alice, "07-alice-new-turn-banner");
    await shot(bob, "07-bob-new-turn-banner");
    check("turn flow: both players ended the turn and the turn advanced", (await turnOf(alice)) > t0turn && (await turnOf(bob)) > t0turn, `alice ended ${endedA}, bob ended ${endedB}, turn ${t0turn} -> ${await turnOf(alice)}/${await turnOf(bob)}`);
    await sleep(6000);
    await shot(alice, "08-alice-turn2");
    for (const [nick, b] of [[nickA, alice], [nickB, bob]]) {
      const st = await stats(b.page);
      check(`${nick}: ui.lua_errors == 0 after the turn`, st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
      check(`${nick}: replica.diverged == 0`, st.replica?.diverged === 0, `${st.replica?.diverged}`);
    }
    // ---- K7 MP list: the host kicks the other player (the X of his row asks for a confirmation, Enter accepts)
    await alice.page.mouse.click(...PX.kickX);
    await sleep(5000);
    await shot(alice, "09-alice-confirm-kick");
    await alice.page.keyboard.press("Enter");
    await sleep(4000);
    const kicksSent = (await stats(alice.page)).m2?.kicks_sent ?? 0;
    check("MP list: the host's kick of bob went out (Matchmaking.KickPlayer -> the server)", kicksSent >= 1, `kicks_sent ${kicksSent}`);
    await shot(alice, "09-alice-after-kick");
    {
      const st = await stats(alice.page);
      check(`${nickA}: ui.lua_errors == 0 at the end`, st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
    }
    void stA;
    void stB;
    if (o.explore) {
      // Exploration: hold the clients for the developer (`--explore 600` seconds).
      await sleep(Number(o.explore) * 1000);
    }
  } catch (e) {
    error = e;
    for (const [nick, b] of pages) await shot(b, `failure-${nick}`);
    for (const [nick, b] of pages) result.numbers[`lastStats_${nick}`] = await stats(b.page).catch(() => null);
  }
  for (const [, b] of pages) result.consoleErrors.push(...b.errors);
  for (const [, b] of pages) await b.close();
  check("no console errors", result.consoleErrors.length === 0, `${result.consoleErrors.length} error(s)${result.consoleErrors[0] ? `: ${result.consoleErrors[0]}` : ""}`);
  if (error) {
    check("no scenario error", false, error.message);
    result.error = error.stack ?? String(error);
  }
  result.pass = result.checks.every((c) => c.pass);
  result.durationMs = Date.now() - started;
  return result;
}
