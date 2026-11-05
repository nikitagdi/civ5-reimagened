// `m5-map-sync` (live report 2026-10-30): under the original UI the 3D map must follow the game. The unit and city models the
// render bridge gave the renderer (`mapUnits` / `mapCities` of the test port, [owner, id, x, y]) are compared with the replica
// (`Players[active]:Units()` / `:Cities()`) after: the start, founding the first city with the settler panel's button, a move by
// mouse (right click on a neighbour plot, aimed with the camera of `__civ5stats.camera`), and END TURN. Before the fix the server
// views (which feed the bridge) stayed at the start of the turn: the settler stood where the city was, warriors at their old plots.
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8910 --scenario m5-map-sync [--gfx webgl2]
// Checks: the models match the replica at every step, ui.lua_errors == 0, replica.diverged == 0; after the clicks a quiet pause
// and a UI order make the replica verify a CHECK of the server (the right click's Game.SetEverRightClickMoved diverged it,
// SIMP-exe-66). Screenshots go to the log dir.
import { openPage, act, until, state, appUrl } from "../lib/browser.mjs";
import { uiDriver } from "../lib/ui-driver.mjs";
import { turnDriver } from "../lib/turns-woe.mjs";
import { woeSteps } from "../lib/steps-woe.mjs";
import { wojSteps, enterGame } from "../lib/steps-woj.mjs";

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
  const result = { scenario: "m5-map-sync", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => { result.checks.push({ name, pass: !!pass, detail: detail ?? "" }); console.log(`  ${pass ? "ok  " : "FAIL"} ${name}${detail ? ` (${detail})` : ""}`); };
  const base = o.attach ?? ctx.server?.url;
  if (!base) { check("options", false, "m5-map-sync needs --attach <url of a real-core server>"); return result; }
  const view = { width: 1600, height: 900 };
  const b = await openPage({ headed: o.headed, logDir: o.logDir, consoleFile: "console.log", viewport: view });
  const shot = async (name) => { try { await b.page.screenshot({ path: `${o.logDir}/${name}.png` }); } catch { /* gone */ } };
  const num = result.numbers;
  let error = null;
  try {
    await enterGame(b, o, base, { autostart: "1", ai: "1", size: "duel", seed: "7", mode: "simultaneous", manual: "1" }, "msync", { act, until, appUrl }, stats);
    const ui = uiDriver(b);
    const woe = woeSteps(b, ui, shot);
    const woj = wojSteps(b, ui, woe, view, shot);
    const td = turnDriver(b, ui, shot, { research: woj.researchViaTree, policy: woj.adoptViaWindow, block: woj.block });
    const [me] = await ui.lua("print(TAG .. Game.GetActivePlayer())");
    const active = Number(me);

    /** The replica's units and cities of the active player: [[id, x, y, type]], [[id, x, y]]. */
    const replica = async () => {
      const lines = await ui.lua(`
        local p = Players[Game.GetActivePlayer()]
        for u in p:Units() do print(TAG .. "u " .. u:GetID() .. " " .. u:GetX() .. " " .. u:GetY() .. " " .. GameInfo.Units[u:GetUnitType()].Type) end
        for c in p:Cities() do print(TAG .. "c " .. c:GetID() .. " " .. c:GetX() .. " " .. c:GetY()) end`);
      const units = [], cities = [];
      for (const l of lines) {
        const [k, id, x, y, t] = l.split(" ");
        (k === "u" ? units : cities).push([Number(id), Number(x), Number(y), t]);
      }
      return { units, cities };
    };
    /** The models of the active player that the render bridge gave the renderer. */
    const drawn = async () => {
      const s = await state(b.page);
      const own = (rows) => (rows ?? []).filter((r) => r[0] === active).map(([, id, x, y]) => [id, x, y]);
      return { units: own(s?.mapUnits), cities: own(s?.mapCities) };
    };
    const key = (rows) => rows.map(([id, x, y]) => `${id}@${x},${y}`).sort().join(" ");
    /** Waits (up to 20 s: the server's view round trip) until the models match the replica; one check. */
    const same = async (what) => {
      let r, d;
      for (let i = 0; i < 40; i++) {
        r = await replica();
        d = await drawn();
        if (key(r.units) === key(d.units) && key(r.cities) === key(d.cities)) break;
        await sleep(500);
      }
      const ok = key(r.units) === key(d.units) && key(r.cities) === key(d.cities);
      check(`the map shows what the replica has ${what}`, ok, ok ? `units ${key(d.units)}; cities ${key(d.cities) || "-"}` : `replica units [${key(r.units)}] cities [${key(r.cities)}]; map units [${key(d.units)}] cities [${key(d.cities)}]`);
      return { r, d, ok };
    };
    const errors = async (what) => {
      const s = await stats(b.page);
      check(`ui.lua_errors == 0 ${what}`, s.ui?.lua_errors === 0, `${s.ui?.lua_errors}`);
    };

    await same("at the start");
    await shot("01-start");

    // The first city, by the settler panel's button (END TURN selects the settler).
    const before = (await replica()).cities.length;
    await ui.clickEndTurn();
    await sleep(6000);
    const t = await ui.unitAction(/FOUND/);
    check("the settler panel has a found-city button", !!t, `${t}`);
    for (let i = 0; i < 40 && (await replica()).cities.length === before; i++) await sleep(500);
    const founded = await same("after founding the city (no settler model, a city model)");
    check("a city was founded", founded.r.cities.length === before + 1, `cities ${before} -> ${founded.r.cities.length}`);
    check("the settler model is gone", !founded.d.units.some(([id]) => !founded.r.units.some(([rid]) => rid === id)), key(founded.d.units));
    await shot("02-city-founded");
    await errors("after the city");

    // A move by mouse: the unit is selected, a right click on a free neighbour plot (the camera of `__civ5stats.camera` aims it).
    if (await ui.cityScreenShown()) { await ui.closeCityScreen(); await sleep(2000); }
    const [pick] = await ui.lua(`
      local p = Players[Game.GetActivePlayer()]
      ;(function() for u in p:Units() do
        if u:GetMoves() > 0 and u:IsCombatUnit() then
          for d = 0, 5 do
            local q = Map.PlotDirection(u:GetX(), u:GetY(), d)
            if q and not q:IsWater() and not q:IsImpassable() and not q:IsMountain() and q:GetNumUnits() == 0 and not q:IsCity() then
              UI.SelectUnit(u) UI.LookAt(u:GetPlot(), 0)
              local w = HexToWorld(ToHexFromGrid({ x = q:GetX(), y = q:GetY() }))
              print(TAG .. u:GetID() .. " " .. q:GetX() .. " " .. q:GetY() .. " " .. w.x .. " " .. w.y .. " " .. w.z)
              return
            end
          end
        end
      end end)()`);
    check("a combat unit with a free neighbour plot", !!pick, pick ?? "none");
    if (pick) {
      await sleep(2500); // the camera goes to the unit
      const [uid, tx, ty] = pick.split(" ").map(Number);
      const [, , , wx, wy, wz] = pick.split(" ").map(Number);
      const from = (await replica()).units.find(([id]) => id === uid);
      const cam = (await stats(b.page)).camera;
      const m = Array.from({ length: 16 }, (_, i) => cam[`m${i}`]);
      // view_proj (column-major) * (x, z, -y, 1): the projector of the world anchors (m2.rs).
      const v = [wx, wz, -wy, 1];
      const c = [0, 1, 2, 3].map((r) => m[r] * v[0] + m[4 + r] * v[1] + m[8 + r] * v[2] + m[12 + r] * v[3]);
      const px = ((c[0] / c[3]) * 0.5 + 0.5) * cam.width;
      const py = (1 - ((c[1] / c[3]) * 0.5 + 0.5)) * cam.height;
      num.move = { unit: uid, to: [tx, ty], px: Math.round(px), py: Math.round(py) };
      await b.page.mouse.move(px, py);
      await sleep(400);
      await b.page.mouse.click(px, py, { button: "right" });
      let moved = null;
      for (let i = 0; i < 20 && !moved; i++) {
        await sleep(500);
        const u = (await replica()).units.find(([id]) => id === uid);
        if (u && from && (u[1] !== from[1] || u[2] !== from[2])) moved = u;
      }
      num.move.byMouse = !!moved;
      if (!moved) {
        // The right click did not reach the unit (input routing is another work item): the same message by the call WorldView.lua makes.
        await ui.lua(`local q = Map.GetPlot(${tx}, ${ty}) Game.SelectionListMove(q, false, false, false)`);
        for (let i = 0; i < 20 && !moved; i++) {
          await sleep(500);
          const u = (await replica()).units.find(([id]) => id === uid);
          if (u && u[1] === tx && u[2] === ty) moved = u;
        }
      }
      check(`the unit moved (${num.move.byMouse ? "right click" : "SelectionListMove"})`, !!moved, moved ? `unit ${uid} at ${moved[1]},${moved[2]}` : `unit ${uid} stood`);
      await same("after the move");
      await shot("03-moved");
      await errors("after the move");
    }

    // The city's first production in the city screen (as m5-acceptance); the order the screen sends if the click did not take.
    if ((await ui.blocking()).type === 2) {
      await ui.clickEndTurn();
      await sleep(5000);
      woj.note(`city screen: ${await woj.chooseInOpenScreen().catch((e) => e.message)}`);
      await sleep(2000);
      if (await ui.cityScreenShown()) await ui.closeCityScreen();
      if ((await ui.blocking()).type === 2) {
        await ui.lua(`for c in Players[Game.GetActivePlayer()]:Cities() do Game.CityPushOrder(c, OrderTypes.ORDER_TRAIN, GameInfoTypes.UNIT_WARRIOR, false, false, true) end`);
        num.productionByOrder = true;
        await sleep(3000);
      }
    }
    // The replica checks the server's digest only when the player is idle (replica::machine, `prepare`): after a quiet pause a UI
    // order without input (a warrior appended to the queue, the message the city screen sends) brings a batch whose CHECK is
    // verified, so whatever the UI did to the replica's game so far shows here (live 2026-10-30: WorldView.lua's right click
    // set Game.SetEverRightClickMoved in the replica only, `record 2: digest ...`).
    {
      const runBefore = (await stats(b.page)).replica?.checks_run ?? 0;
      await sleep(12000);
      await ui.lua(`for c in Players[Game.GetActivePlayer()]:Cities() do Game.CityPushOrder(c, OrderTypes.ORDER_TRAIN, GameInfoTypes.UNIT_WARRIOR, false, false, true) end`);
      let st = null;
      for (let i = 0; i < 20; i++) {
        await sleep(500);
        st = (await stats(b.page)).replica;
        if ((st?.checks_run ?? 0) > runBefore) break;
      }
      num.checksRun = [runBefore, st?.checks_run];
      check("a CHECK of the server ran after the UI's clicks", (st?.checks_run ?? 0) > runBefore, `checks_run ${runBefore} -> ${st?.checks_run}`);
      check("replica.diverged == 0 after the CHECK", st?.diverged === 0, `${st?.diverged}`);
    }
    // END TURN: the new turn's models.
    const at = await td.turnNow();
    await td.endTurn();
    check("the turn advanced", (await td.turnNow()) > at, `${at} -> ${await td.turnNow()}`);
    await same("after END TURN");
    await shot("04-next-turn");
    const st = await stats(b.page);
    check("ui.lua_errors == 0 at the end", st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
    check("replica.diverged == 0 at the end", st.replica?.diverged === 0, `${st.replica?.diverged}`);
    num.handled = td.handled;
  } catch (e) {
    error = e;
    await shot("failure");
  }
  result.consoleErrors.push(...b.errors);
  await b.close();
  check("no console errors", b.errors.length === 0, `${b.errors.length} error(s)${b.errors[0] ? `: ${b.errors[0]}` : ""}`);
  if (error) { check("no scenario error", false, error.message); result.error = error.stack ?? String(error); }
  result.pass = result.checks.every((c) => c.pass);
  result.durationMs = Date.now() - started;
  return result;
}
