// `fix-ui4` (live reports 2026-10-30): fog (plot queries, floating text) and combat preview, original UI on a real-core game.
// 1. SerialEventMouseOverHex carries the plot's grid position: EnemyUnitPanel (Map.GetPlot(hexX, hexY)) shows the preview of the
//    plot under the mouse (the other player's unit is made visible in the replica only, Alt stands for war).
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8940 --scenario fix-ui4 [--out <dir>]
import { openPage, act, until, appUrl } from "../lib/browser.mjs";
import { uiDriver } from "../lib/ui-driver.mjs";
import { enterGame } from "../lib/steps-woj.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function stats(page) {
  return page.evaluate(() => {
    const s = window.__civ5stats ?? {};
    return { ui: s.ui ?? null, replica: s.replica ?? null, exe: s.exe ?? null, m2: s.m2 ?? null };
  });
}

export async function run(ctx) {
  const o = ctx.opts;
  const result = { scenario: "fix-ui4", pass: false, logs: o.logDir, checks: [], numbers: {} };
  const check = (name, pass, detail) => { result.checks.push({ name, pass: !!pass, detail: detail ?? "" }); console.log(`  ${pass ? "ok  " : "FAIL"} ${name}${detail ? ` (${detail})` : ""}`); };
  const base = o.attach ?? ctx.server?.url;
  const shots = process.env.FIX_SHOTS ?? o.logDir;
  const view = { width: 1600, height: 900 };
  const b = await openPage({ headed: o.headed, logDir: o.logDir, consoleFile: "console.log", viewport: view });
  const shot = async (name) => { try { await b.page.screenshot({ path: `${shots}/${name}.png` }); } catch { /* gone */ } };
  try {
    await enterGame(b, o, base, { autostart: "1", ai: "1", size: "small", seed: "7", mode: "simultaneous", manual: "1" }, "fu4", { act, until, appUrl }, stats);
    const ui = uiDriver(b);
    const one = async (code) => (await ui.lua(code))[0];
    const counts = await one(`
      local team = Game.GetActiveTeam()
      local n, rev, vis, cityRev, cityUnrev, dbg = 0, 0, 0, 0, 0, tostring(Game.IsDebugMode())
      local w, h = Map.GetGridSize()
      for y = 0, h - 1 do for x = 0, w - 1 do
        local p = Map.GetPlot(x, y)
        n = n + 1
        if p:IsRevealed(team, false) then rev = rev + 1 end
        if p:IsVisible(team, false) then vis = vis + 1 end
        if p:IsCity() then if p:IsRevealed(team, false) then cityRev = cityRev + 1 else cityUnrev = cityUnrev + 1 end end
      end end
      print(TAG .. "team=" .. team .. " plots=" .. n .. " revealed=" .. rev .. " visible=" .. vis .. " cities revealed=" .. cityRev .. " unrevealed=" .. cityUnrev .. " debug=" .. dbg)`);
    console.log("  counts:", counts);
    result.numbers.counts = counts;
    // An enemy (any other player's) unit on a plot that is not visible: what the plot queries answer.
    const probe = await one(`
      local team = Game.GetActiveTeam()
      local me = Game.GetActivePlayer()
      local out = {}
      for i = 0, GameDefines.MAX_CIV_PLAYERS - 1 do
        local pl = Players[i]
        if pl and pl:IsAlive() and i ~= me then
          for u in pl:Units() do
            local p = u:GetPlot()
            if p and not p:IsVisible(team, false) then out[#out + 1] = "unit@" .. p:GetX() .. "," .. p:GetY() .. " rev=" .. tostring(p:IsRevealed(team, false)) end
            if #out >= 3 then break end
          end
          for c in pl:Cities() do
            local p = c:Plot()
            out[#out + 1] = "city@" .. p:GetX() .. "," .. p:GetY() .. " rev=" .. tostring(p:IsRevealed(team, false)) .. " vis=" .. tostring(p:IsVisible(team, false))
          end
        end
      end
      print(TAG .. table.concat(out, " | "))`);
    console.log("  probe:", probe);
    result.numbers.probe = probe;
    const calls = await ui.lua(`
      local me = Game.GetActivePlayer()
      local mine, theirs
      for u in Players[me]:Units() do if u:IsCombatUnit() and not mine then mine = u end end
      for i = 0, GameDefines.MAX_CIV_PLAYERS - 1 do
        local pl = Players[i]
        if pl and pl:IsAlive() and i ~= me and not theirs then for u in pl:Units() do if u:IsCombatUnit() and not theirs then theirs = u end end end
      end
      if not mine or not theirs then print(TAG .. "no pair " .. tostring(mine) .. " " .. tostring(theirs)) return end
      local from, to = mine:GetPlot(), theirs:GetPlot()
      local function t(name, f) local ok, r = pcall(f) print(TAG .. name .. " " .. (ok and tostring(r) or ("ERR " .. tostring(r)))) end
      t("flagicon", function() return table.concat({UI.GetUnitFlagIcon(theirs)}, ",") end)
      t("portrait", function() return table.concat({UI.GetUnitPortraitIcon(theirs)}, ",") end)
      t("MaxMoves", function() return theirs:MaxMoves() end)
      t("IsInvisible", function() return theirs:IsInvisible(Game.GetActiveTeam(), false) end)
      t("GetBaseCombat", function() return theirs:GetBaseCombatStrength() end)
      t("IsHasPromotion", function() return theirs:IsHasPromotion(0) end)
      t("GetBaseRanged", function() return mine:GetBaseRangedCombatStrength() end)
      t("GetMaxAttackStrength", function() return mine:GetMaxAttackStrength(from, to, theirs) end)
      t("GetMaxDefenseStrength", function() return theirs:GetMaxDefenseStrength(to, mine) end)
      t("GetFireSupportUnit", function() return mine:GetFireSupportUnit(theirs:GetOwner(), to:GetX(), to:GetY()) end)
      t("GetCombatDamage", function() return mine:GetCombatDamage(1000, 1000, mine:GetDamage(), false, false, false) end)
      t("GetDamage", function() return mine:GetDamage() end)
      t("IsRanged", function() return tostring(mine:IsRanged()) .. tostring(mine:IsEmbarked()) .. tostring(mine:IsRangeAttackOnlyInDomain()) .. tostring(mine:IsCityAttackOnly()) end)
      t("GetCombatPrediction", function() return Game.GetCombatPrediction(mine, theirs) end)
      t("GetEmbarkedUnitDefense", function() return theirs:GetEmbarkedUnitDefense() end)
      t("GetRangedSupportFire", function() return theirs:IsRangedSupportFire() end)
      t("GetMaxRanged", function() return theirs:GetMaxRangedCombatStrength(mine, nil, false, false) end)
      t("GetRangeCombatDamage", function() return mine:GetRangeCombatDamage(theirs, nil, false) end)
      t("GameDefines", function() return tostring(GameDefines.MOVE_DENOMINATOR) end)
      t("IsAtWar", function() return tostring(Teams[Game.GetActiveTeam()]:IsAtWar(theirs:GetTeam())) end)
      t("GetCombatPlot", function() return tostring(mine:GetPlot():GetX()) .. "->" .. to:GetX() end)
    `);
    console.log("  calls:", calls.join(" || "));
    result.numbers.calls = calls;
    // Combat preview: make the other player's unit visible in the replica only (a probe), Alt shows the panel at peace.
    const pos = await one(`
      local me = Game.GetActivePlayer()
      local mine, theirs
      for u in Players[me]:Units() do if u:IsCombatUnit() and not mine then mine = u end end
      for i = 0, GameDefines.MAX_CIV_PLAYERS - 1 do
        local pl = Players[i]
        if pl and pl:IsAlive() and i ~= me and not theirs then for u in pl:Units() do if u:IsCombatUnit() and not theirs then theirs = u end end end
      end
      local p = theirs:GetPlot()
      local ok, e = pcall(function() p:ChangeVisibilityCount(Game.GetActiveTeam(), 1, -1, false, true) end)
      UI.SelectUnit(mine)
      __exe_camera_place(p:GetX(), p:GetY(), 0.5, 0.5, -1)
      print(TAG .. p:GetX() .. " " .. p:GetY() .. " vis=" .. tostring(p:IsVisible(Game.GetActiveTeam(), false)) .. " " .. tostring(ok) .. " " .. tostring(e) .. " alt=" .. tostring(UIManager:GetAlt()))`);
    console.log("  preview setup:", pos);
    await sleep(1500);
    await b.page.keyboard.down("Alt");
    await b.page.mouse.move(view.width / 2 - 200, view.height / 2 - 100);
    await sleep(300);
    await b.page.mouse.move(view.width / 2, view.height / 2);
    await sleep(1500);
    const hov = await one(`local x, y = UI.GetMouseOverHex() print(TAG .. x .. " " .. y .. " alt=" .. tostring(UIManager:GetAlt()))`);
    console.log("  hover:", hov);
    const panel = (await ui.dump("")).filter((r) => /Enemy|CombatResults|UnitPortrait/i.test(r.path));
    console.log("  enemy rows total:", panel.length, panel.filter((r) => /Panel$|MyCombatResultsStack|TheirCombatResultsStack|UnitName|Strength/.test(r.path)).slice(0, 10).map((r) => `${r.path} hid=${r.hidden}`).join(" | "));
    console.log("  enemy panel rows:", panel.slice(0, 12).map((r) => `${r.path.split("/").slice(-2).join("/")} hid=${r.hidden} ${r.text.slice(0, 30)}`).join(" | "));
    const trace = await ui.lua(`
      local x, y = UI.GetMouseOverHex()
      local pPlot = Map.GetPlot(x, y)
      local h = UI.GetHeadSelectedUnit()
      print(TAG .. "head=" .. tostring(h and h:GetName()) .. " plot=" .. x .. "," .. y)
      if not (pPlot and h) then return end
      local iTeam = Game.GetActiveTeam()
      print(TAG .. "combat=" .. tostring(h:IsCombatUnit()) .. " rev=" .. tostring(pPlot:IsRevealed(iTeam, false)) .. " city=" .. tostring(pPlot:IsCity()) .. " vis=" .. tostring(pPlot:IsVisible(iTeam, false)) .. " cityonly=" .. tostring(h:IsCityAttackOnly()) .. " alt=" .. tostring(UIManager:GetAlt()))
      local n = pPlot:GetNumUnits()
      for i = 0, n do
        local u = pPlot:GetUnit(i)
        if u then print(TAG .. "unit " .. i .. " " .. u:GetName() .. " inv=" .. tostring(u:IsInvisible(iTeam, false)) .. " dom=" .. u:GetDomainType() .. "/" .. h:GetDomainType() .. " base=" .. u:GetBaseCombatStrength() .. " owner=" .. u:GetOwner() .. " war=" .. tostring(Teams[iTeam]:IsAtWar(u:GetTeam()))) end
      end
      print(TAG .. "panel hidden=" .. tostring((function() local ok, c = pcall(LookUpControl, "/InGame/WorldView/EnemyUnitPanel") return ok and c and c:IsHidden() end)()))
    `);
    console.log("  trace:", trace.join(" || "));
    const shown = await one(`local ok, c = pcall(LookUpControl, "/InGame/WorldView/EnemyUnitPanel") print(TAG .. tostring(ok and c and not c:IsHidden()))`);
    check("hover with Alt over another player's unit shows the combat preview (EnemyUnitPanel)", shown === "true", String(shown));
    await shot("preview");
    await b.page.keyboard.up("Alt");
    const st = await stats(b.page);
    console.log("  ui stats:", JSON.stringify(st.ui).slice(0, 300));
    await shot("loaded");
    result.pass = result.checks.every((c) => c.pass);
  } catch (e) {
    console.log("  error", e);
    result.error = String(e);
  } finally {
    await b.close?.();
  }
  return result;
}
