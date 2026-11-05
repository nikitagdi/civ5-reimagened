// `fix-path-hover` (live reports 2026-10-30): the original UI on a real-core game, by mouse.
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8930 --scenario fix-path-hover [--out <dir>]
// 1. Right drag of a selected unit: while the right button is held over a plot the movement range (MovementRangeBorder), the path
//    (the renderer's path pieces) and the turn markers of PathHelpManager.lua show; the release moves the unit there.
// 2. The mouse resting on a plot shows the plot help tooltip of PlotHelpManager.lua (TheBox's tip).
// 3. ui.lua_errors == 0. Screenshots go to $FIX_SHOTS (default <logDir>).
import { openPage, act, until, appUrl } from "../lib/browser.mjs";
import { uiDriver } from "../lib/ui-driver.mjs";
import { enterGame } from "../lib/steps-woj.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function stats(page) {
  return page.evaluate(() => {
    const s = window.__civ5stats ?? {};
    return { ui: s.ui ?? null, replica: s.replica ?? null, exe: s.exe ?? null, m2: s.m2 ?? null, render: s.render ?? null, path: s.path ?? null };
  });
}

export async function run(ctx) {
  const o = ctx.opts;
  const result = { scenario: "fix-path-hover", pass: false, logs: o.logDir, checks: [], numbers: {} };
  const check = (name, pass, detail) => { result.checks.push({ name, pass: !!pass, detail: detail ?? "" }); console.log(`  ${pass ? "ok  " : "FAIL"} ${name}${detail ? ` (${detail})` : ""}`); };
  const base = o.attach ?? ctx.server?.url;
  const shots = process.env.FIX_SHOTS ?? o.logDir;
  const view = { width: 1600, height: 900 };
  const b = await openPage({ headed: o.headed, logDir: o.logDir, consoleFile: "console.log", viewport: view });
  const shot = async (name) => { try { await b.page.screenshot({ path: `${shots}/${name}.png` }); } catch { /* gone */ } };
  try {
    await enterGame(b, o, base, { autostart: "1", ai: "1", size: "small", seed: "7", mode: "simultaneous", manual: "1" }, "fph", { act, until, appUrl }, stats);
    const ui = uiDriver(b);
    const one = async (code) => (await ui.lua(code))[0];
    // A land unit with moves, and a plot two steps away it can walk to.
    const pick = await one(`
      local p = Players[Game.GetActivePlayer()]
      local best
      for u in p:Units() do
        if u:GetDomainType() == DomainTypes.DOMAIN_LAND and u:MovesLeft() > 0 and (not best or u:IsCombatUnit()) then best = u end
      end
      if not best then print(TAG .. "none") return end
      UI.SelectUnit(best)
      local ux, uy = best:GetX(), best:GetY()
      local tx, ty
      for dx = -3, 3 do for dy = -3, 3 do
        local q = Map.GetPlot(ux + dx, uy + dy)
        if not tx and q and Map.PlotDistance(ux, uy, q:GetX(), q:GetY()) == 2 and not q:IsWater() and not q:IsMountain()
          and not q:IsImpassable() and q:GetNumUnits() == 0 and q:IsRevealed(Game.GetActiveTeam(), false) then
          tx, ty = q:GetX(), q:GetY()
        end
      end end
      print(TAG .. best:GetID() .. " " .. ux .. " " .. uy .. " " .. tostring(tx) .. " " .. tostring(ty))`);
    check("a unit to move", pick && pick !== "none", pick);
    const [uid, ux, uy, tx, ty] = pick.split(" ").map(Number);
    // The target plot in the middle of the screen.
    await ui.lua(`__exe_camera_place(${tx}, ${ty}, 0.5, 0.5, -1)`);
    await sleep(1500);
    const X = view.width / 2, Y = view.height / 2;
    await b.page.mouse.move(X - 40, Y);
    await sleep(200);
    await b.page.mouse.move(X, Y);
    await sleep(300);
    const over = await one(`local x, y = UI.GetMouseOverHex() print(TAG .. x .. " " .. y)`);
    check("the mouse is over the target plot", over === `${tx} ${ty}`, `${over} vs ${tx} ${ty}`);
    await ui.lua(`__fph_range, __fph_hl = 0, 0
      Events.AddUnitMoveHexRangeHex.Add(function() __fph_range = __fph_range + 1 end)
      Events.SerialEventHexHighlight.Add(function(h, on, c, s) if s == "MovementRangeBorder" and on then __fph_hl = __fph_hl + 1 end end)`);
    await b.page.mouse.down({ button: "right" });
    await sleep(200);
    await b.page.mouse.move(X + 3, Y + 2);
    await sleep(1500);
    const held = await stats(b.page);
    const shown = await b.page.evaluate(() => window.__civ5stats);
    result.numbers.held = { path: held.path, m2: held.m2, render_path: shown?.render?.["path.steps"] ?? null };
    const pathSteps = shown?.counters?.["path.steps"] ?? null;
    const markers = await one(`
      local n = 0
      local ok, c = pcall(LookUpControl, "/InGame/WorldView/PathHelpManager/Container")
      print(TAG .. tostring(ok and c and not c:IsHidden()))`);
    await shot("rdrag-held");
    try { await b.page.screenshot({ path: `${shots}/rdrag-held-zoom.png`, clip: { x: X - 350, y: Y - 200, width: 700, height: 400 } }); } catch { /* gone */ }
    const pathKeys = [];
    const walk = (o, p) => { for (const [k, v] of Object.entries(o ?? {})) { if (v && typeof v === "object") walk(v, `${p}${k}.`); else if (/path|highlight/i.test(k)) pathKeys.push(`${p}${k}=${v}`); } };
    walk(shown, "");
    result.numbers.pathKeys = pathKeys;
    console.log("  stats:", pathKeys.join(" "));
    check("right drag: the turn markers are shown (PathHelpManager Container)", markers === "true", markers);
    const range = await one(`print(TAG .. __fph_range .. " " .. __fph_hl)`);
    const [rangeN, hlN] = range.split(" ").map(Number);
    check("right drag: the movement range is announced and highlighted (MovementRangeBorder)", rangeN > 1 && hlN > 1, `range hexes ${rangeN}, highlights ${hlN}`);
    check("right drag: the renderer draws a path", pathSteps >= 2, `path.steps=${pathSteps}`);
    await b.page.mouse.up({ button: "right" });
    await sleep(4000);
    const after = await one(`local u = Players[Game.GetActivePlayer()]:GetUnitByID(${uid}) print(TAG .. u:GetX() .. " " .. u:GetY())`);
    await shot("rdrag-released");
    check("right release: the unit moved", after !== `${ux} ${uy}`, `${ux} ${uy} -> ${after} (target ${tx} ${ty})`);
    const st2 = await b.page.evaluate(() => window.__civ5stats);
    check("right release: no path drawn", st2?.counters?.["path.steps"] === 0, `${st2?.counters?.["path.steps"]}`);
    // Hover: rest on a plot.
    await b.page.mouse.move(X - 120, Y + 60);
    await sleep(300);
    await b.page.mouse.move(X - 121, Y + 61);
    await sleep(2500);
    const diag = await ui.lua(`
      for _, p in ipairs({"/InGame/PlotHelpManager/TheBox", "/InGame/EUI_context/PlotHelpManager/TheBox"}) do
        local ok, c = pcall(LookUpControl, p)
        print(TAG .. p .. " " .. tostring(ok and c and c:IsToolTipEnabled()) .. " hidden=" .. tostring(ok and c and c:IsHidden()))
      end
      print(TAG .. "tip1=" .. tostring(OptionsManager.GetTooltip1Seconds()) .. " mouse=" .. table.concat({UI.GetMouseOverHex()}, ","))`);
    console.log("  hover diag:", diag.join(" | "));
    const tip = await ui.dump("");
    console.log("  tooltip-ish rows:", tip.filter((r) => /tool|tip|HexDetails|TheBox/i.test(r.path)).slice(0, 10).map((r) => `${r.path} ${r.cls} ${r.text.slice(0, 40)}`).join(" | "));
    const tipRows = tip.filter((r) => /ToolTip|HexDetails/i.test(r.path) && r.cls === "Label" && r.text.trim().length > 0);
    await shot("hover-tooltip");
    check("hover: the plot help tooltip shows", tipRows.length > 0, tipRows.slice(0, 3).map((r) => `${r.path}: ${r.text.slice(0, 60)}`).join(" | "));
    // Ancient ruins (report only): the nearest hut a unit reaches this turn; what the UI is told when it is entered.
    const hut = await one(`
      __fph_alerts = {}
      Events.GameplayAlertMessage.Add(function(t) table.insert(__fph_alerts, tostring(t)) end)
      Events.AddPopupTextEvent.Add(function(x, y, t) table.insert(__fph_alerts, "popuptext:" .. tostring(t)) end)
      Events.SerialEventGameMessagePopup.Add(function(i) table.insert(__fph_alerts, "popup:" .. tostring(i.Type)) end)
      local p = Players[Game.GetActivePlayer()]
      local hutId = GameInfoTypes.IMPROVEMENT_GOODY_HUT
      for u in p:Units() do
        if u:GetDomainType() == DomainTypes.DOMAIN_LAND and u:MovesLeft() > 0 then
          for dx = -2, 2 do for dy = -2, 2 do
            local q = Map.GetPlot(u:GetX() + dx, u:GetY() + dy)
            if q and q:GetImprovementType() == hutId and Map.PlotDistance(u:GetX(), u:GetY(), q:GetX(), q:GetY()) <= 2 then
              UI.SelectUnit(u)
              Game.SelectionListMove(q, false, false, false)
              print(TAG .. q:GetX() .. " " .. q:GetY())
              return
            end
          end end
        end
      end
      print(TAG .. "none")`);
    if (hut && hut !== "none") {
      await sleep(5000);
      const [hx, hy] = hut.split(" ").map(Number);
      const res = await ui.lua(`print(TAG .. "improvement=" .. Map.GetPlot(${hx}, ${hy}):GetImprovementType()) for _, a in ipairs(__fph_alerts) do print(TAG .. a) end`);
      await ui.lua(`__exe_camera_place(${hx}, ${hy}, 0.5, 0.5, -1)`);
      await sleep(1500);
      await shot("ruins-entered");
      result.numbers.ruins = res;
      console.log("  ruins:", res.join(" | "));
    } else {
      console.log("  ruins: no hut in reach");
    }
    const audio = await b.page.evaluate(() => window.__civ5stats?.audio ?? window.__civ5stats?.groups?.audio ?? null);
    console.log("  audio:", JSON.stringify(audio));
    result.numbers.audio = audio;
    const st = await stats(b.page);
    check("ui.lua_errors == 0", st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
  } catch (e) {
    check("no scenario error", false, e.message);
    result.error = e.stack ?? String(e);
    await shot("error");
  } finally {
    await b.close?.();
  }
  result.pass = result.checks.every((c) => c.pass);
  return result;
}
