// `m3-tech-policy-religion` (M3 WO-B): K9 tech tree, K10 social policies, K11 pantheon on the original UI (EUI, the default game screen)
// and a real-core game. Needs the real-core server (WSL, `tools/playtest/m2/server.ps1`) and `--attach <url>`:
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8720 --scenario m3-tech-policy-religion [--gfx webgl2] [--headed]
// Faith and culture: the human gets them from a DEV gamedata copy (a block appended to lekmod_policies.lua: turn 3 ->
// +30 faith, +60 culture; see docs of WO-B, SIMP-exe-100); the server must run with `-GameData` of that copy.
// Flow (by mouse at 1600x900): enter -> tech tree opens (top panel science button) -> a tech is chosen -> the tree closes ->
// turns until faith/culture arrive -> a pantheon is founded from the popup -> a policy is adopted -> checks.
// Checks: __civ5stats.ui.lua_errors == 0, the tech was sent (replica.net_sent), the player's research changed,
// a pantheon belief and a policy are on the replica (read through `window.__civ5m3` Lua probes in `?luadbg`).
import { openPage, act, until, state, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { uiDriver } from "../lib/ui-driver.mjs";

// Lua of `?luadbg=`: the production of a new city (the production choice is the city view, K4/WO-A) and PROBES that print
// one line `M3PROBE key=value ...` to the console when the replica's state of this scenario changes.
const DEBUG_LUA = `
print("M3PROBE boot")
local function probe(tag)
  local pid = Game.GetActivePlayer()
  local p = Players[pid]
  if not p then return end
  local pan = -1
  pcall(function() pan = p:GetBeliefInPantheon() end)
  local pol = -1
  pcall(function() pol = p:GetNumPolicies() end)
  print(string.format("M3PROBE %s research=%d faith=%d culture=%d pantheon=%d policies=%d freepol=%d turn=%d", tag,
    p:GetCurrentResearch(), p:GetFaith(), p:GetJONSCulture(), pan, pol, p.GetNumFreePolicies and p:GetNumFreePolicies() or -1, Game.GetGameTurn()))
end
Events.SerialEventResearchDirty.Add(function() probe("research") end)
Events.SerialEventGameDataDirty.Add(function() probe("data") end)
Events.ActivePlayerTurnStart.Add(function() probe("turnstart") end)
GameEvents.PlayerAdoptPolicy.Add(function(pid, pol) print("M3PROBE adopt player=" .. pid .. " policy=" .. pol) end)
GameEvents.PantheonFounded.Add(function(pid, city, belief) print("M3PROBE pantheon player=" .. pid .. " belief=" .. belief) end)
-- Popups the game raises itself later (or never in a short game) are opened on the turns 5-8 to see that they load and draw.
Events.ActivePlayerTurnStart.Add(function()
  local turn = Game.GetGameTurn()
  local pid = Game.GetActivePlayer()
  local pop
  if turn == 5 then pop = { Type = ButtonPopupTypes.BUTTONPOPUP_RELIGION_OVERVIEW, Data1 = 0 }
  elseif turn == 6 then pop = { Type = ButtonPopupTypes.BUTTONPOPUP_FAITH_GREAT_PERSON, Data1 = pid }
  elseif turn == 7 then pop = { Type = ButtonPopupTypes.BUTTONPOPUP_CHOOSE_IDEOLOGY, Data1 = pid }
  elseif turn == 8 then pop = { Type = ButtonPopupTypes.BUTTONPOPUP_TECH_AWARD, Data1 = pid, Data3 = GameInfoTypes.TECH_AGRICULTURE } end
  if pop then print("M3PROBE popup turn=" .. turn); Events.SerialEventGameMessagePopup(pop) end
end)
Events.SerialEventCityCreated.Add(function(hex, owner, id)
  local p = Players[owner]
  local city = p and p:GetCityByID(id)
  if city and owner == Game.GetActivePlayer() then
    pcall(function() Game.CityPushOrder(city, 0, GameInfoTypes.UNIT_WARRIOR, 0, false, true) end)
  end
end)
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

// Pixels of the original UI at 1600x900.
const END_TURN = [1423, 682];
const FOUND_CITY = [463, 860];
const FORTIFY = [255, 865];
const SCIENCE_BUTTON = [20, 12];
const POTTERY = [565, 211];
const TREE_CLOSE = [57, 806];
const BELIEF_FIRST = [150, 167];
const CONFIRM_YES = [800, 438];
const POLICY_NOTIFICATION = [1565, 630];
const ADOPT_TRADITION = [411, 252];
const POLICY_CLOSE = [404, 808];
const NOTIFICATION = [1555, 615];
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
  const result = { scenario: "m3-tech-policy-religion", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const base = o.attach ?? ctx.server?.url;
  if (!base) {
    check("options", false, "m3-tech-policy-religion needs --attach <url of the real-core server>");
    return result;
  }
  const b = await openPage({ headed: o.headed, logDir: o.logDir, viewport: { width: 1600, height: 900 } });
  const shot = async (name) => {
    for (let k = 0; k < 3; k++) {
      try { await b.page.screenshot({ path: `${o.logDir}/${name}.png`, timeout: 120000 }); return; } catch (e) { console.log(`  shot ${name} failed: ${e.message.split("\n")[0]}`); }
    }
  };
  const click = async ([x, y]) => { await b.page.mouse.move(x, y); await b.page.mouse.click(x, y); };
  // The last `M3PROBE <tag> research=.. faith=.. ...` line of the page's console (the probes of DEBUG_LUA).
  const lastProbe = () => {
    for (let i = b.console.length - 1; i >= 0; i--) {
      const m = /M3PROBE (\w+) research=(-?\d+) faith=(-?\d+) culture=(-?\d+) pantheon=(-?\d+) policies=(-?\d+) freepol=(-?\d+) turn=(-?\d+)/.exec(b.console[i].text);
      if (m) return { tag: m[1], research: +m[2], faith: +m[3], culture: +m[4], pantheon: +m[5], policies: +m[6], freepol: +m[7], turn: +m[8] };
    }
    return null;
  };
  const probe = (key) => lastProbe()?.[key] ?? null;
  // Escape on the bare map opens the game menu (WO-G, as in the original); the menu is modal and blocks END TURN.
  // Escape here only closes what is open: when it opened the game menu instead, a second Escape closes it.
  const ui = uiDriver(b);
  const gameMenuUp = async () => (await ui.dump("#top")).some((r) => /GameMenu.*MainMenuButton$/.test(r.path));
  const closeGameMenu = async () => {
    for (let k = 0; k < 3 && (await gameMenuUp()); k++) { await b.page.keyboard.press("Escape"); await sleep(1500); }
  };
  const escape = async () => { await b.page.keyboard.press("Escape"); await sleep(1500); await closeGameMenu(); };
  // END TURN; a unit that waits for orders is selected by the button: fortify it and try again.
  const endTurn = async () => {
    const at = (await stats(b.page)).replica?.turn ?? 0;
    for (let tries = 0; tries < 4; tries++) {
      await closeGameMenu();
      await click(END_TURN);
      const t0 = Date.now();
      while (Date.now() - t0 < (tries === 0 ? 30000 : 12000)) {
        await sleep(1000);
        if (((await stats(b.page)).replica?.turn ?? 0) > at) { await sleep(2500); return true; }
      }
      await click(FORTIFY);
      await sleep(4000);
    }
    throw new Error(`the turn did not advance from ${at}`);
  };
  let error = null;
  try {
    let t0 = 0;
    for (let attempt = 0; ; attempt++) {
      const nick = `m3b${(Date.now() + attempt) % 1000000}`;
      t0 = Date.now();
      await b.page.goto(appUrl(base, o, { nick, autostart: "1", ai: "2", size: "small", seed: "7", mode: "simultaneous", manual: "1", stats: "1", luadbg: process.env.M3_LUADBG ?? DEBUG_LUA }));
      try {
        const s = await until(b.page, (x) => x.screen === "login" || x.screen === "play", 90000, "sign-in window or the game");
        if (s.screen === "login") await act(b.page, { kind: "login", nick, password: DEV_PASSWORD });
        await until(b.page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 60000, "the game");
        break;
      } catch (e) {
        if (attempt >= 2) throw e;
      }
    }
    const upAt = Date.now();
    let st = null;
    for (;;) {
      st = await stats(b.page);
      if (st.m2?.phase === 2) break;
      if (st.m2?.phase === -1) throw new Error("M2 failed (phase -1)");
      if (Date.now() - upAt > 420000) throw new Error(`M2 is not up after 420 s: ${JSON.stringify(st.m2)}`);
      await sleep(500);
    }
    await sleep(4000);
    await shot("m3-00-enter");
    st = await stats(b.page);
    check("ui.lua_errors == 0 after the start", st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
    if (process.env.M3_QUICK) {
      await click(SCIENCE_BUTTON);
      await sleep(Number(process.env.M3_QUICK) * 1000);
      throw new Error("M3_QUICK: stopped after the start");
    }
    // END TURN while units wait for orders selects the settler; its panel's button founds the city.
    await click(END_TURN);
    await sleep(6000);
    const citiesBefore = (await stats(b.page)).exe?.entity_cities ?? 0;
    await click(FOUND_CITY);
    await sleep(10000);
    st = await stats(b.page);
    check("a city was founded", (st.exe?.entity_cities ?? 0) === citiesBefore + 1, `cities ${citiesBefore} -> ${st.exe?.entity_cities}`);
    await shot("m3-01-city");

    // K9: the tech tree (the science button of the top panel).
    await click(SCIENCE_BUTTON);
    await sleep(3000);
    await shot("m3-02-tech-tree");
    if (process.env.M3_HOLD) await sleep(Number(process.env.M3_HOLD) * 1000);
    st = await stats(b.page);
    check("ui.lua_errors == 0 with the tree open", st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);

    // A tech is chosen: Pottery (an available tech other than the current research).
    const researchBefore = probe("research");
    const sentBefore = st.replica?.net_sent ?? 0;
    await click(POTTERY);
    await sleep(5000);
    await shot("m3-03-tech-chosen");
    st = await stats(b.page);
    const researchAfter = probe("research");
    check("a tech is chosen in the tree (research changed, a message sent)", researchAfter !== null && researchAfter !== researchBefore && (st.replica?.net_sent ?? 0) > sentBefore,
      `research ${researchBefore} -> ${researchAfter}, net_sent ${sentBefore} -> ${st.replica?.net_sent}`);
    await click(TREE_CLOSE);
    await sleep(1500);
    await shot("m3-04-tree-closed");

    // Turns until the faith and culture of the dev gamedata have arrived (turn 3).
    let guard = 0;
    while ((probe("faith") ?? 0) < 10 && guard++ < 12) {
      await endTurn();
      await shot(`m3-05-turn-${guard}`);
    }
    // The notifications (pantheon, policy) come at the start of the next turn.
    await endTurn();
    await sleep(6000);
    await shot("m3-06-notifications");
    result.numbers.afterTurns = lastProbe();

    // K11: the pantheon notification (the round button on the right) opens the pantheon popup.
    await click(NOTIFICATION);
    await sleep(3000);
    await shot("m3-07-pantheon-popup");
    st = await stats(b.page);
    check("ui.lua_errors == 0 with the pantheon popup", st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
    await click(BELIEF_FIRST);
    await sleep(2000);
    await shot("m3-08-pantheon-confirm");
    await click(CONFIRM_YES);
    await sleep(8000);
    await shot("m3-09-pantheon-founded");
    check("a pantheon is founded", (probe("pantheon") ?? -1) >= 0, `pantheon belief ${probe("pantheon")}`);

    // K10: the policy notification opens the policy popup.
    await click(POLICY_NOTIFICATION);
    await sleep(4000);
    await shot("m3-10-policy-popup");
    st = await stats(b.page);
    check("ui.lua_errors == 0 with the policy popup", st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
    await click(ADOPT_TRADITION);
    await sleep(4000);
    await shot("m3-11-policy-adopt");
    await click(CONFIRM_YES);
    await sleep(8000);
    await shot("m3-12-policy-adopted");
    check("a policy is adopted", (probe("policies") ?? 0) >= 1, `policies ${probe("policies")}`);
    await click(POLICY_CLOSE);
    await sleep(2500);
    await shot("m3-12b-policy-closed");
    await escape();
    await shot("m3-12c-after-escape");

    // The other windows of the cluster, opened by the Lua of the probes on the turns 5-8.
    for (const [turn, name] of [[5, "religion-overview"], [6, "faith-great-person"], [7, "ideology"], [8, "tech-award"]]) {
      await endTurn();
      await sleep(4000);
      await shot(`m3-13-${name}`);
      st = await stats(b.page);
      check(`ui.lua_errors == 0 after the ${name} popup event of turn ${turn} (the popup itself is not verified: the event from Lua did not open it)`, st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
      await escape();
    }
    st = await stats(b.page);
    check("replica.diverged == 0", st.replica?.diverged === 0, `${st.replica?.diverged}`);
  } catch (e) {
    error = e;
    await shot("failure");
    const last = await state(b.page).catch(() => null);
    if (last) result.numbers.lastScreen = `${last.screen}/${last.net?.phase}`;
    result.numbers.lastStats = await stats(b.page).catch(() => null);
  }
  result.consoleErrors.push(...b.errors);
  await b.close();
  check("no console errors", result.consoleErrors.length === 0, `${result.consoleErrors.length} error(s)${result.consoleErrors[0] ? `: ${result.consoleErrors[0]}` : ""}`);
  if (error) {
    check("no scenario error", false, error.message);
    result.error = error.stack ?? String(error);
  }
  result.pass = result.checks.every((c) => c.pass);
  result.durationMs = Date.now() - started;
  return result;
}
