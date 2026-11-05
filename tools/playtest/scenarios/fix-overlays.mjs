// `fix-overlays` (live reports 2026-10-30): the original UI on a real-core game, by mouse.
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8930 --scenario fix-overlays [--out <dir>]
// 1. A right drag of a selected unit to a plot a few turns away: the renderer draws the exe's path (a centre per node, six
//    connectors per edge, the mark at the end, looks by turn 1/2/3+) and PathHelpManager.lua numbers the turns.
// 2. The minimap panel's "Yield Icons" and "Resource Icons" boxes: YieldIconManager.lua gets ShowHexYield and builds its
//    anchors, ResourceIconManager.lua gets the resource icons and shows its container.
// 3. ui.lua_errors == 0. Screenshots go to $FIX_SHOTS (default <logDir>).
import { openPage, act, until, appUrl } from "../lib/browser.mjs";
import { uiDriver } from "../lib/ui-driver.mjs";
import { enterGame } from "../lib/steps-woj.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function stats(page) {
  return page.evaluate(() => {
    const s = window.__civ5stats ?? {};
    return { ui: s.ui ?? null, replica: s.replica ?? null, exe: s.exe ?? null, m2: s.m2 ?? null, render: s.render ?? null };
  });
}

export async function run(ctx) {
  const o = ctx.opts;
  const result = { scenario: "fix-overlays", pass: false, logs: o.logDir, checks: [], numbers: {} };
  const check = (name, pass, detail) => { result.checks.push({ name, pass: !!pass, detail: detail ?? "" }); console.log(`  ${pass ? "ok  " : "FAIL"} ${name}${detail ? ` (${detail})` : ""}`); };
  const base = o.attach ?? ctx.server?.url;
  const shots = process.env.FIX_SHOTS ?? o.logDir;
  const view = { width: 1600, height: 900 };
  const b = await openPage({ headed: o.headed, logDir: o.logDir, consoleFile: "console.log", viewport: view });
  const shot = async (name, clip) => { try { await b.page.screenshot({ path: `${shots}/${name}.png`, ...(clip ? { clip } : {}) }); } catch { /* gone */ } };
  try {
    await enterGame(b, o, base, { autostart: "1", ai: "1", size: "small", seed: "7", mode: "simultaneous", manual: "1" }, "fov", { act, until, appUrl }, stats);
    const ui = uiDriver(b);
    const one = async (code) => (await ui.lua(code))[0];
    // A land unit and the farthest revealed land plot it can reach within 4 plots (a path of several turns).
    const pick = await one(`
      local p = Players[Game.GetActivePlayer()]
      local best
      for u in p:Units() do
        if u:GetDomainType() == DomainTypes.DOMAIN_LAND and u:MovesLeft() > 0 and (not best or u:IsCombatUnit()) then best = u end
      end
      if not best then print(TAG .. "none") return end
      UI.SelectUnit(best)
      local ux, uy = best:GetX(), best:GetY()
      local tx, ty, td = nil, nil, 0
      for dx = -5, 5 do for dy = -5, 5 do
        local q = Map.GetPlot(ux + dx, uy + dy)
        if q then
          local d = Map.PlotDistance(ux, uy, q:GetX(), q:GetY())
          if d <= 4 and d > td and not q:IsWater() and not q:IsMountain() and not q:IsImpassable() and q:GetNumUnits() == 0
            and q:IsRevealed(Game.GetActiveTeam(), false) then
            tx, ty, td = q:GetX(), q:GetY(), d
          end
        end
      end end
      print(TAG .. best:GetID() .. " " .. ux .. " " .. uy .. " " .. tostring(tx) .. " " .. tostring(ty) .. " " .. td)`);
    check("a unit to move", pick && pick !== "none", pick);
    const [, ux, uy, tx, ty] = pick.split(" ").map(Number);
    // The middle of unit and target in the middle of the screen; the target is found by the mouse on the screen.
    await ui.lua(`__exe_camera_place(${Math.round((ux + tx) / 2)}, ${Math.round((uy + ty) / 2)}, 0.5, 0.5, -1)`);
    await sleep(1500);
    // Find the screen point of the target plot: walk the mouse over a grid until the exe reports the plot.
    let at = null;
    for (let y = 150; y < view.height - 150 && !at; y += 25) {
      for (let x = 200; x < view.width - 200 && !at; x += 25) {
        await b.page.mouse.move(x, y);
        await sleep(20);
        const h = await one(`local x, y = UI.GetMouseOverHex() print(TAG .. x .. " " .. y)`);
        if (h === `${tx} ${ty}`) at = { x, y };
      }
    }
    check("the target plot is on the screen", !!at, JSON.stringify(at));
    if (at) {
      await b.page.mouse.down({ button: "right" });
      await sleep(200);
      await b.page.mouse.move(at.x + 2, at.y + 1);
      await sleep(1500);
      const s = await b.page.evaluate(() => window.__civ5stats?.counters ?? {});
      check("right drag: the renderer draws a path", (s["path.steps"] ?? 0) >= 2, `path.steps=${s["path.steps"]}`);
      await shot("path-held");
      await shot("path-held-zoom", { x: Math.max(0, at.x - 500), y: Math.max(0, at.y - 300), width: 1000, height: 600 });
      await b.page.mouse.up({ button: "right" });
      await sleep(1500);
    }
    // 2. The map options.
    await ui.lua(`__fov = { yields = 0, res = 0 }
      Events.ShowHexYield.Add(function(x, y, on) if on then __fov.yields = __fov.yields + 1 end end)
      Events.SerialEventRawResourceIconCreated.Add(function() __fov.res = __fov.res + 1 end)`);
    const boxes = await ui.dump("Show");
    const box = (id) => boxes.find((r) => r.path.endsWith(`/${id}`));
    console.log("  boxes:", boxes.filter((r) => /ShowYield|ShowResources/.test(r.path)).map((r) => `${r.path} ${r.x},${r.y} ${r.w}x${r.h}`).join(" | "));
    for (const id of ["ShowYield", "ShowResources"]) {
      const r = box(id);
      if (r && r.w > 0) {
        await b.page.mouse.click(r.x + 10, r.y + r.h / 2);
      } else {
        await ui.lua(id === "ShowYield" ? "UI.SetYieldVisibleMode(true)" : "UI.SetResourceVisibleMode(true)");
      }
      await sleep(800);
    }
    await sleep(2000);
    const counts = await one(`
      local function kids(path)
        local ok, c = pcall(LookUpControl, path)
        if not (ok and c) then return "nil" end
        return tostring(not c:IsHidden())
      end
      print(TAG .. __fov.yields .. " " .. __fov.res .. " " .. kids("/InGame/ResourceIconManager/ResourceIconContainer"))`);
    console.log("  icons:", counts);
    const [ny, nr, resShown] = counts.split(" ");
    check("yield icons: ShowHexYield reached the Lua", Number(ny) > 0, `ShowHexYield(true) ${ny}`);
    check("resource icons: the container is shown", resShown === "true", resShown);
    const resRows = (await ui.dump("ResourceIconManager")).filter((r) => r.cls === "Image");
    check("resource icons: icons are on the screen", resRows.length > 0, `${resRows.length} images`);
    result.numbers.icons = { yields: Number(ny), resourcesAfterToggle: Number(nr) };
    const pt = await stats(b.page);
    result.numbers.frame = { ui: pt.ui, m2: pt.m2 };
    await shot("icons");
    await ui.lua(`__exe_camera_place(${ux}, ${uy}, 0.5, 0.5, -1)`);
    await sleep(1500);
    await shot("icons-unit");
    await shot("icons-unit-zoom", { x: 300, y: 150, width: 1000, height: 600 });
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
