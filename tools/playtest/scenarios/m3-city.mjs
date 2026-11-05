// `m3-city` (WO-A, milestone M3): the city screen (K8) and the popup host (K18) of the original UI, by mouse.
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8710 --scenario m3-city [--gfx webgl2] [--headed]
// The controls are found by path / class / text in the screen dump and clicked in the middle of their rectangle
// (`lib/ui-driver.mjs`), not by pixels. Flow at 1600x900: enter -> END TURN (selects the settler) -> found a city -> open the city (its banner) -> pick the
// production in the city screen by mouse -> screenshots (next to golden S3) -> close. Checks: ui.lua_errors == 0.
import { openPage, act, until, state, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { uiDriver } from "../lib/ui-driver.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function stats(page) {
  return page.evaluate(() => {
    const s = window.__civ5stats ?? {};
    return { ui: s.ui ?? null, replica: s.replica ?? null, exe: s.exe ?? null, m2: s.m2 ?? null, camera: s.camera ?? null };
  });
}

// K18: the event popups the exe's InterfaceBuddy releases (UI.AddPopup -> SerialEventGameMessagePopup). Each is added by
// a Lua snippet (`window.__civ5test.lua`), shown, shot, closed with Escape; the layouts of GenericPopup come first.
const NATURAL_WONDER_LUA = `
local nx, ny
for i = 0, Map.GetNumPlots() - 1 do
  local p = Map.GetPlotByIndex(i)
  local f = p and p:GetFeatureType() if f and f >= 0 and GameInfo.Features[f].NaturalWonder then nx, ny = p:GetX(), p:GetY() break end
end
if nx then UI.AddPopup({ Type = ButtonPopupTypes.BUTTONPOPUP_NATURAL_WONDER_REWARD, Data1 = nx, Data2 = ny, Data3 = 30, Option1 = true }) else print("M3 no natural wonder on the map") end`;
const POPUPS = [
  ["probe", `local function hid(n) local c = LookUpControl("/InGame/"..n) return c and tostring(c:IsHidden()) or "none" end Events.SerialEventGameMessagePopup.Add(function(p) print("M3 hidden?", p.Type, hid("GenericPopup"), hid("TextPopup"), hid("NewEraPopup"), hid("GoldenAgePopup")) end) Events.SerialEventGameMessagePopup.Add(function(p) print("M3 popup event", p.Type) end) Events.SerialEventGameMessagePopupShown.Add(function(p) print("M3 shown", p.Type) end) print("M3 up", UI.IsPopupUp(), ButtonPopupTypes.BUTTONPOPUP_TEXT)`],
  ["confirm-command", `local ai for i = 0, 300 do if GameInfoActions[i] and GameInfoActions[i].TextKey then ai = i break end end print("M3 action", ai) UI.AddPopup({ Type = ButtonPopupTypes.BUTTONPOPUP_CONFIRMCOMMAND, Data1 = ai, Option1 = false })`],
  ["text", `UI.AddPopup({ Type = ButtonPopupTypes.BUTTONPOPUP_TEXT, Data1 = 400, Text = "TXT_KEY_POPUP_YES", Option1 = true })`],
  ["new-era", `UI.AddPopup({ Type = ButtonPopupTypes.BUTTONPOPUP_NEW_ERA, Data1 = GameInfoTypes.ERA_CLASSICAL })`],
  ["natural-wonder", NATURAL_WONDER_LUA],
  ["goody-hut", `UI.AddPopup({ Type = ButtonPopupTypes.BUTTONPOPUP_GOODY_HUT_REWARD, Data1 = GameInfoTypes.GOODY_GOLD, Data2 = 60 })`],
  ["barbarian-camp", `UI.AddPopup({ Type = ButtonPopupTypes.BUTTONPOPUP_BARBARIAN_CAMP_REWARD, Data1 = 45 })`],
  ["golden-age", `UI.AddPopup({ Type = ButtonPopupTypes.BUTTONPOPUP_GOLDEN_AGE_REWARD, Data1 = 0 })`],
  ["great-person-reward", `UI.AddPopup({ Type = ButtonPopupTypes.BUTTONPOPUP_GREAT_PERSON_REWARD, Data1 = GameInfoTypes.UNIT_SCIENTIST, Data2 = (function() for c in Players[Game.GetActivePlayer()]:Cities() do return c:GetID() end end)() })`],
  // (the wonder splash only shows after the start turn: WonderPopup.lua returns on turn 0)
  ["rename-city", `for c in Players[Game.GetActivePlayer()]:Cities() do UI.SelectCity(c) break end UI.AddPopup({ Type = ButtonPopupTypes.BUTTONPOPUP_RENAME_CITY, Data1 = 0 })`],
];

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "m3-city", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const base = o.attach ?? ctx.server?.url;
  if (!base) { check("options", false, "m3-city needs --attach <url of the real-core server>"); return result; }
  const b = await openPage({ headed: o.headed, logDir: o.logDir, viewport: { width: 1600, height: 900 } });
  const shot = async (name) => { try { await b.page.screenshot({ path: `${o.logDir}/${name}.png` }); } catch { /* gone */ } };
  let error = null;
  try {
    for (let attempt = 0; ; attempt++) {
      const nick = `m3a${(Date.now() + attempt) % 1000000}`;
      await b.page.goto(appUrl(base, o, { nick, autostart: "1", ai: "2", size: "small", seed: "7", mode: "simultaneous", stats: "1", ...(process.env.M3_NOMT ? { mt: "0" } : {}), ...(process.env.M3_LUADBG ? { luadbg: process.env.M3_LUADBG } : {}) }));
      try {
        const s = await until(b.page, (x) => x.screen === "login" || x.screen === "play", 90000, "sign-in window or the game");
        if (s.screen === "login") await act(b.page, { kind: "login", nick, password: DEV_PASSWORD });
        await until(b.page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 60000, "the game");
        break;
      } catch (e) { if (attempt >= 2) throw e; }
    }
    const upAt = Date.now();
    let st;
    for (;;) {
      st = await stats(b.page);
      if (st.m2?.phase === 2) break;
      if (st.m2?.phase === -1) throw new Error("M2 failed (phase -1)");
      if (Date.now() - upAt > 180000) throw new Error(`M2 is not up: ${JSON.stringify(st.m2)}`);
      await sleep(500);
    }
    await sleep(4000);
    st = await stats(b.page);
    check("the UI is up", st.m2?.phase === 2 && st.replica?.live === 1, JSON.stringify(st.m2));
    check("ui.lua_errors == 0 after the start", st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
    if (!process.env.M3_POPUPS_ONLY) {
    const ui = uiDriver(b);
    await ui.clickEndTurn();
    await sleep(9000);
    const citiesBefore = st.exe?.entity_cities ?? 0;
    check("the settler has a found-city button", !!(await ui.unitAction(/FOUND/)));
    await sleep(12000);
    st = await stats(b.page);
    check("a city was founded", (st.exe?.entity_cities ?? 0) === citiesBefore + 1, `cities ${citiesBefore} -> ${st.exe?.entity_cities}`);
    await shot("c1-city-founded");
    // The "Choose production" notification (it takes the END TURN button's place) opens the city screen.
    await ui.clickEndTurn();
    await sleep(5000);
    await shot("c2-city-screen");
    check("the city screen opened", await ui.cityScreenShown());
    st = await stats(b.page);
    check("ui.lua_errors == 0 with the city screen open", st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
    result.numbers.afterOpen = st;
    // The production list (left, UNITS): click "Warrior".
    const picked = await ui.pickProduction();
    check("a production was picked in the list", !!picked, picked);
    await sleep(4000);
    await shot("c3-production-chosen");
    st = await stats(b.page);
    check("ui.lua_errors == 0 after the production click", st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
    // The screen closes by itself after the pick (AutoClose); else its RETURN button.
    if (await ui.cityScreenShown()) await ui.closeCityScreen();
    await sleep(3000);
    check("the city screen is closed", !(await ui.cityScreenShown()));
    const [pop] = await ui.lua(`print(TAG .. tostring(UI.IsPopupUp()))`);
    check("no popup is left open after the city screen", pop === "false", pop);
    await shot("c4-city-closed");
    st = await stats(b.page);
    check("ui.lua_errors == 0 after the city screen is closed", st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);

    }
    // K18: the popups.
    for (const [name, lua] of POPUPS.filter(([n]) => (process.env.M3_ONLY ? process.env.M3_ONLY.split(",").includes(n) : n !== "probe"))) {
      await b.page.evaluate((code) => window.__civ5test.lua(code), lua);
      await sleep(8000);
      await b.page.evaluate(() => window.__civ5test.lua(`local function hid(n) local c = LookUpControl("/InGame/"..n) return c and tostring(c:IsHidden()) or "none" end print("M3 state", UI.IsPopupUp(), hid("GenericPopup"), hid("TextPopup"), hid("SetCityName"), hid("NewEraPopup"))`));
      await sleep(5000);
      await shot(`p-${name}`);
      await b.page.keyboard.press("Enter");
      await sleep(6000);
      st = await stats(b.page);
      check(`popup ${name}: ui.lua_errors == 0`, st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
    }
  } catch (e) {
    error = e;
    await shot("failure");
  }
  result.consoleErrors.push(...b.errors);
  await b.close();
  check("no console errors", result.consoleErrors.length === 0, `${result.consoleErrors.length} error(s)${result.consoleErrors[0] ? `: ${result.consoleErrors[0]}` : ""}`);
  if (error) { check("no scenario error", false, error.message); result.error = error.stack ?? String(error); }
  result.pass = result.checks.every((c) => c.pass);
  result.durationMs = Date.now() - started;
  return result;
}
