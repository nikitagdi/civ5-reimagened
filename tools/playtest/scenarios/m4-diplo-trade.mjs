// `m4-diplo-trade` (WO-D, milestone M4): diplomacy of the original UI (K12) in its MP form, by mouse: the leader of an AI in the
// diplomacy list -> the leader screen -> Trade -> an item on the table -> propose -> the AI accepts -> the deal is made (our gold
// went to the AI as a gift); then the city-state window and the declare-war popup open.
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8810 --scenario m4-diplo-trade [--gfx webgl2]
// The server must run the DEV game data of `tools/playtest/m4/make-dev-gamedata.ps1` (the humans meet every major on turn 2 and get +300 gold).
// Controls are found with the dump of `lib/ui-driver.mjs`, not by pixels. Checks: ui.lua_errors == 0, a deal was proposed and made.
import { openPage, act, until, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { uiDriver, BLOCK } from "../lib/ui-driver.mjs";

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
  const result = { scenario: "m4-diplo-trade", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const base = o.attach ?? ctx.server?.url;
  if (!base) { check("options", false, "m4-diplo-trade needs --attach <url of the real-core server>"); return result; }
  const b = await openPage({ headed: o.headed, logDir: o.logDir, viewport: { width: 1920, height: 1080 } });
  const shot = async (name) => { try { await b.page.screenshot({ path: `${o.logDir}/${name}.png` }); } catch { /* gone */ } };
  let error = null;
  try {
    for (let attempt = 0; ; attempt++) {
      const nick = `m4d${(Date.now() + attempt) % 1000000}`;
      await b.page.goto(appUrl(base, o, { nick, autostart: "1", ai: "2", size: "small", seed: "7", mode: "simultaneous", stats: "1", ...(process.env.M3_NOMT ? { mt: "0" } : {}) }));
      try {
        const s = await until(b.page, (x) => x.screen === "login" || x.screen === "play", 90000, "sign-in window or the game");
        if (s.screen === "login") await act(b.page, { kind: "login", nick, password: DEV_PASSWORD });
        await until(b.page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 60000, "the game");
        break;
      } catch (e) { if (attempt >= 2) throw e; }
    }
    const upAt = Date.now();
    for (;;) {
      const st = await stats(b.page);
      if (st.m2?.phase === 2) break;
      if (st.m2?.phase === -1) throw new Error("M2 failed (phase -1)");
      if (Date.now() - upAt > 180000) throw new Error("M2 is not up");
      await sleep(500);
    }
    await sleep(4000);
    const ui = uiDriver(b);
    // found the city, then end turns until turn 3 (the dev data makes the AIs met and gives gold on turn 2)
    await ui.clickEndTurn(); await sleep(9000);
    await ui.unitAction(/FOUND/); await sleep(12000);
    let at = (await stats(b.page)).replica?.turn ?? 0;
    while (at < 3) {
      let advanced = false;
      for (let step = 0; step < 24 && !advanced; step++) {
        const bl = await ui.blocking();
        if (bl.type === BLOCK.NONE) {
          await ui.clickEndTurn();
          for (let i = 0; i < 40 && !advanced; i++) { await sleep(1000); advanced = ((await stats(b.page)).replica?.turn ?? 0) > at; }
          continue;
        }
        await ui.handleBlock(bl);
        await sleep(2500);
        advanced = ((await stats(b.page)).replica?.turn ?? 0) > at;
      }
      if (!advanced) throw new Error(`the turn did not advance from ${at}`);
      at += 1;
      await sleep(3000);
    }
    const [ai] = await ui.lua(`for i = 0, GameDefines.MAX_MAJOR_CIVS - 1 do local p = Players[i] if p and p:IsAlive() and not p:IsHuman() and not p:IsMinorCiv() and i ~= Game.GetActivePlayer() then print(TAG .. i) break end end`);
    const [emb0] = await ui.lua(`print(TAG .. tostring(Teams[Players[${ai}]:GetTeam()]:HasEmbassyAtTeam(Game.GetActiveTeam())))`);
    const [gold0] = await ui.lua(`print(TAG .. Players[Game.GetActivePlayer()]:GetGold())`);
    result.numbers.ai = ai; result.numbers.gold0 = Number(gold0);
    check("an AI major is met", ai !== undefined);
    await shot("d1-before");

    // diagnostics: are the K12 contexts there, and does the DLL answer DoBeginDiploWithHuman with a leader message?
    result.numbers.diag = await ui.lua(`for _, n in ipairs({"LeaderHeadRoot","DiscussionDialog","DiploTrade","DeclareWarPopup","CityStateDiploPopup"}) do local c = LookUpControl("/InGame/BulkUI/" .. n) or LookUpControl("/InGame/" .. n) print(TAG .. n .. "=" .. tostring(c ~= nil)) end
Events.AILeaderMessage.Add(function(p, s, t, a, d) print(TAG .. "AILeaderMessage " .. tostring(p) .. " " .. tostring(s) .. " " .. tostring(t)) end)
print(TAG .. "busy=" .. tostring(IsGameCoreBusy and IsGameCoreBusy()) .. " rootup=" .. tostring(UI.GetLeaderHeadRootUp()))
`);
    await sleep(3000);
    // the leader of the AI in the diplomacy list (the civ list of the notification panel / diplo corner)
    const rows = await ui.dump("");
    result.numbers.civListRows = rows.filter((r) => /Civ|Diplo|Leader/i.test(r.path) && /Button/.test(r.cls)).slice(0, 25).map((r) => `${r.path} ${r.cls} ${r.text}`);
    result.numbers.listRows = rows.filter((r) => /Genghis|Nyatsimba/.test(r.text)).map((r) => `${r.path} ${r.cls} ${r.x},${r.y} ${r.w}x${r.h} ${r.disabled ? "D" : ""}`);
    const dbtn = rows.find((r) => /\/DiploCornerStack\/DiploButton$/.test(r.path));
    if (dbtn) { await ui.click(dbtn); await sleep(4000); await shot("d1b-diplo-overview"); }
    const rows2 = await ui.dump("");
    result.numbers.diploRows = rows2.filter((r) => /Diplo|Relationship|Overview|Leader/i.test(r.path) && /Button|Label/.test(r.cls) && !/DiploCornerStack|TopPanel|LeaderHeadRoot/.test(r.path)).slice(0, 40).map((r) => `${r.path} ${r.cls} ${r.x},${r.y} ${r.text}`);
    const leader = rows2.find((r) => /^\/InGame\/.*MajorCivScrollPanel\/ButtonStack\/DiploButton$/.test(r.path) && r.cls === "Button" && !r.disabled) ?? null;
    check("the Diplomacy Overview lists an AI leader", !!leader, leader?.path ?? "none");
    if (leader) await ui.click(leader);
    await sleep(5000);
    result.numbers.afterClick = await ui.lua(`local c = LookUpControl("/LeaderHeadRoot") print(TAG .. "hidden=" .. tostring(c and c:IsHidden()) .. " up=" .. tostring(UI.GetLeaderHeadRootUp()))`);
    await shot("d2-leader-screen");
    let st = await stats(b.page);
    check("ui.lua_errors == 0 with the leader screen", st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
    const trade = await ui.find("/LeaderHeadRoot", (r) => /\/TradeButton$/.test(r.path) && !r.disabled);
    check("the leader screen has a Trade button", !!trade);
    result.numbers.prio = await ui.lua(`print(TAG .. tostring(PopupPriority.LeaderHead) .. " " .. tostring(PopupPriority.LeaderTrade) .. " " .. tostring(PopupPriority.Current))`);
    result.numbers.topBefore = (await ui.dump("#top")).slice(0, 4).map((r) => r.path);
    if (trade) {
      await ui.lua(`LuaEvents.OpenAILeaderDiploTrade.Add(function() print(TAG .. "OpenAILeaderDiploTrade fired") end) Events.LeavingLeaderViewMode.Add(function() print(TAG .. "LeavingLeaderViewMode") end)`);
      await ui.click(trade);
      result.numbers.topAfter = (await ui.dump("#top")).slice(0, 4).map((r) => r.path);
      result.numbers.poll = [];
      for (let i = 0; i < 6; i++) { result.numbers.poll.push(await ui.lua(`local c = LookUpControl("/LeaderHeadRoot/DiploTrade") local l = LookUpControl("/LeaderHeadRoot") print(TAG .. "trade hidden=" .. tostring(c:IsHidden()) .. " root hidden=" .. tostring(l:IsHidden()))`)); await sleep(700); }
    }
    await sleep(1500);
    result.numbers.afterTrade = await ui.lua(`for _, p in ipairs({"/LeaderHeadRoot","/LeaderHeadRoot/DiploTrade","/LeaderHeadRoot/DiploTrade/DiploTrade","/DiploTrade"}) do local c = LookUpControl(p) print(TAG .. p .. " " .. tostring(c) .. " hidden=" .. tostring(c and c:IsHidden())) end`);
    result.numbers.tradeRows = (await ui.dump("/DiploTrade")).filter((r) => /Pocket|Table/.test(r.path)).slice(0, 30).map((r) => `${r.path} ${r.cls} ${r.x},${r.y} ${r.w}x${r.h} ${r.disabled ? "D" : ""}`);
    await shot("d3-trade-table");
    // our gold into the pocket (a gift the AI accepts), then propose
    const gold = await ui.find("/LeaderHeadRoot/DiploTrade", (r) => /\/UsPocketAllowEmbassy$/.test(r.path) && !r.disabled);
    check("the table has our Accept Embassy item (gold is closed to a first deal without a friendship, as in BNW)", !!gold, gold?.path ?? "none");
    if (gold) { await ui.click(gold); await sleep(1500); }
    await shot("d4-offer");
    const propose = await ui.find("/LeaderHeadRoot/DiploTrade", (r) => /\/ProposeButton$/.test(r.path));
    check("the table has a Propose button", !!propose);
    if (propose) await ui.click(propose);
    await sleep(8000);
    await shot("d5-after-propose");
    const [emb1] = await ui.lua(`print(TAG .. tostring(Teams[Players[${ai}]:GetTeam()]:HasEmbassyAtTeam(Game.GetActiveTeam())))`);
    const [gold1] = await ui.lua(`print(TAG .. Players[Game.GetActivePlayer()]:GetGold())`);
    result.numbers.gold1 = Number(gold1);
    st = await stats(b.page);
    check("the deal was made (the AI got an embassy with us)", emb0 === "false" && emb1 === "true", `embassy ${emb0} -> ${emb1}`);
    check("ui.lua_errors == 0 after the deal", st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
    check("replica.diverged == 0", st.replica?.diverged === 0, `${st.replica?.diverged}`);

    // leave the leader screen: BACK on the table, Goodbye on the leader screen
    for (const [root, re] of [["/LeaderHeadRoot/DiploTrade", /\/CancelButton$/], ["/LeaderHeadRoot/RootOptions", /\/GoodByeButton$/]]) {
      const btn = await ui.find(root, (r) => re.test(r.path) && !r.disabled);
      if (btn) { await ui.click(btn); await sleep(2500); }
    }
    await shot("d5b-left");
    result.numbers.leftTop = (await ui.dump("#top")).slice(0, 2).map((r) => r.path);

    // the city-state window and the declare-war popup
    const popups = [
      ["city-state", `local cs for i = GameDefines.MAX_MAJOR_CIVS, GameDefines.MAX_CIV_PLAYERS - 1 do local p = Players[i] if p and p:IsAlive() and p:IsMinorCiv() then cs = i break end end if cs then Events.SerialEventGameMessagePopup{ Type = ButtonPopupTypes.BUTTONPOPUP_CITY_STATE_DIPLO, Data1 = cs } else print("M4 no city-state") end`, "/CityStateDiploPopup"],
      ["declare-war", `Events.SerialEventGameMessagePopup{ Type = ButtonPopupTypes.BUTTONPOPUP_DECLAREWARMOVE, Data1 = Players[${ai}]:GetTeam(), Option1 = true }`, "/DeclareWarPopup"],
    ];
    for (const [name, code, path] of popups) {
      await ui.lua(code);
      await sleep(5000);
      const shown = (await ui.dump(path)).length > 0;
      await shot(`d6-${name}`);
      check(`the ${name} window opens`, shown);
      await b.page.keyboard.press("Escape");
      await sleep(2500);
      st = await stats(b.page);
      check(`${name}: ui.lua_errors == 0`, st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
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
