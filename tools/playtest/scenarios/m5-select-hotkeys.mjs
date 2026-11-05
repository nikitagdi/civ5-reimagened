// `m5-select-hotkeys` (live reports 2026-10-30): in the original UI (the default game screen) a unit is selected by mouse
// and gets the engine's selection ring, the exe's hot keys work, and the camera does not stick to the mouse.
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8900 --scenario m5-select-hotkeys [--headed]
// Server: one real-core server with the plain game data (tools/playtest/m5/serve-one.sh). A new game against one AI:
//  1. a click on the map on our warrior's plot selects it (InGame.lua ClickSelect -> UI.LocationSelect): the head
//     selected unit is the warrior, `__civ5stats.m2.ring_x/ring_y` is its plot (the ring was drawn there) and the pixels
//     around the plot change;
//  2. after that press and release on the map the camera does not follow the mouse;
//  3. F (the exe's hot key: CvGame::GetAction over GameInfoActions) fortifies the warrior (`m2.hotkeys` counts it);
//  4. a click on the unit flag of an other own unit selects that unit (SerialEventUnitFlagSelected);
//  5. the left arrow (WorldView.lua's camera event) moves the camera;
//  ui.lua_errors == 0 and no console errors.
import { openPage, act, until, appUrl } from "../lib/browser.mjs";
import { uiDriver } from "../lib/ui-driver.mjs";
import { enterGame } from "../lib/steps-woj.mjs";
import { decode } from "../lib/png.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const stats = (page) => page.evaluate(() => {
  const s = window.__civ5stats ?? {};
  return { ui: s.ui ?? null, replica: s.replica ?? null, exe: s.exe ?? null, m2: s.m2 ?? null, camera: s.camera ?? null };
});

/** Mean absolute difference (0..255) of two PNG screenshots in the box (x, y, w, h). */
function boxDiff(a, b, box) {
  const A = decode(a), B = decode(b);
  let sum = 0, n = 0;
  for (let y = box.y; y < box.y + box.h; y++) {
    for (let x = box.x; x < box.x + box.w; x++) {
      const i = (y * A.width + x) * A.channels, j = (y * B.width + x) * B.channels;
      for (let c = 0; c < 3; c++) sum += Math.abs(A.px[i + c] - B.px[j + c]);
      n += 3;
    }
  }
  return n ? sum / n : 0;
}

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "m5-select-hotkeys", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => { result.checks.push({ name, pass: !!pass, detail: detail ?? "" }); console.log(`  ${pass ? "ok  " : "FAIL"} ${name}${detail ? ` (${detail})` : ""}`); };
  const base = o.attach ?? ctx.server?.url;
  if (!base) { check("options", false, "m5-select-hotkeys needs --attach <url of a real-core server>"); return result; }
  const view = { width: 1600, height: 900 };
  const b = await openPage({ headed: o.headed, logDir: o.logDir, consoleFile: "console.log", viewport: view });
  const shot = async (name) => { try { await b.page.screenshot({ path: `${o.logDir}/${name}.png` }); } catch { /* gone */ } };
  const num = result.numbers;
  try {
    await enterGame(b, o, base, { autostart: "1", ai: "1", size: "small", seed: "7", mode: "simultaneous", manual: "1" }, "msel", { act, until, appUrl }, stats);
    let st = await stats(b.page);
    check("the original UI is up and live", st.m2?.phase === 2 && st.replica?.live === 1, JSON.stringify(st.m2));
    const ui = uiDriver(b);
    await sleep(3000);
    const units = (await ui.lua(`
      local p = Players[Game.GetActivePlayer()]
      for u in p:Units() do print(TAG .. u:GetID() .. " " .. GameInfo.Units[u:GetUnitType()].Type .. " " .. u:GetX() .. " " .. u:GetY()) end`))
      .map((l) => { const [id, type, x, y] = l.split(" "); return { id: Number(id), type, x: Number(x), y: Number(y) }; });
    num.units = units;
    const warrior = units.find((u) => u.type === "UNIT_WARRIOR") ?? units.find((u) => !/SETTLER|WORKER/.test(u.type)) ?? units[0];
    const other = units.find((u) => u !== warrior && (u.x !== warrior.x || u.y !== warrior.y));
    if (!warrior) throw new Error(`no unit of ours: ${JSON.stringify(units)}`);
    // Nothing selected, the warrior's plot in the middle of the screen.
    const centre = async (u) => {
      await ui.lua(`UI.ClearSelectionList() UI.LookAt(Map.GetPlot(${u.x}, ${u.y}))`);
      await sleep(2500);
    };
    await centre(warrior);
    st = await stats(b.page);
    check("nothing selected: no ring", st.m2?.ring_x === -1, `ring ${st.m2?.ring_x},${st.m2?.ring_y}`);
    const at = { x: Math.round(view.width / 2), y: Math.round(view.height / 2) + 12 };
    const box = { x: at.x - 70, y: at.y - 40, w: 140, h: 80 };
    await b.page.mouse.move(at.x, at.y);
    await sleep(800);
    const before = await b.page.screenshot();
    await shot("01-before");

    // 1. Click on the map.
    await b.page.mouse.down();
    await sleep(80);
    await b.page.mouse.up();
    await sleep(2500);
    const [head] = await ui.lua(`local u = UI.GetHeadSelectedUnit() print(TAG .. (u and (u:GetID() .. " " .. u:GetX() .. " " .. u:GetY()) or "none"))`);
    check("a click on the map selects the unit of the plot", head === `${warrior.id} ${warrior.x} ${warrior.y}`, `head ${head}, warrior ${warrior.id} at ${warrior.x},${warrior.y}`);
    st = await stats(b.page);
    check("the selection ring is drawn on its plot", st.m2?.ring_x === warrior.x && st.m2?.ring_y === warrior.y, `ring ${st.m2?.ring_x},${st.m2?.ring_y}`);
    const after = await b.page.screenshot();
    await shot("02-selected");
    num.ringPixelDiff = Math.round(boxDiff(before, after, box) * 100) / 100;
    check("the pixels around the selected plot change", num.ringPixelDiff > 1.5, `mean |diff| ${num.ringPixelDiff} in ${JSON.stringify(box)}`);

    // 2. The camera does not follow the mouse after the press and release.
    const focus = (s) => [s.camera?.focus_x ?? NaN, s.camera?.focus_z ?? NaN];
    const f0 = focus(await stats(b.page));
    for (let i = 1; i <= 10; i++) { await b.page.mouse.move(at.x + i * 30, at.y + i * 15); await sleep(40); }
    await sleep(1500);
    const f1 = focus(await stats(b.page));
    const moved = Math.hypot(f1[0] - f0[0], f1[1] - f0[1]);
    check("the camera stays when the mouse moves after a click", moved < 0.01, `focus moved ${moved.toFixed(3)}`);

    // 3. F fortifies (the exe's hot key).
    const hk0 = st.m2?.hotkeys ?? 0;
    await b.page.keyboard.press("f");
    let activity = "";
    for (let i = 0; i < 20; i++) {
      await sleep(500);
      [activity] = await ui.lua(`local u = Players[Game.GetActivePlayer()]:GetUnitByID(${warrior.id}) print(TAG .. tostring(u:GetActivityType()) .. " " .. tostring(u:GetFortifyTurns()) .. " " .. tostring(u:IsFortifyable()))`);
      if (!/^0 /.test(activity)) break;
    }
    st = await stats(b.page);
    check("F is the exe's hot key", (st.m2?.hotkeys ?? 0) > hk0, `hotkeys ${hk0} -> ${st.m2?.hotkeys}`);
    check("F changes the unit's state (fortify)", !/^0 /.test(activity), `activity fortifyTurns fortifyable: ${activity}`);
    await shot("03-fortified");

    // 4. A click on an other own unit's flag selects it.
    if (other) {
      await centre(other);
      const flags = (await ui.dump("UnitFlagManager")).filter((r) => r.cls.includes("Button") && /\/Button$/.test(r.path) && r.w > 0);
      const mid = { x: view.width / 2, y: view.height / 2 };
      const s = (await b.page.evaluate(() => window.__civ5stats?.camera?.ui_scale)) ?? 1;
      const dist = (r) => Math.hypot((r.x + r.w / 2) * s - mid.x, (r.y + r.h / 2) * s - mid.y);
      const flag = flags.sort((p, q) => dist(p) - dist(q))[0];
      num.flagCount = flags.length;
      if (flag) await ui.click(flag);
      await sleep(2500);
      const [h2] = await ui.lua(`local u = UI.GetHeadSelectedUnit() print(TAG .. (u and u:GetID() or "none"))`);
      check("a click on a unit flag selects that unit", h2 === String(other.id), `head ${h2}, unit ${other.id} (${other.type}), flag ${flag?.path}`);
      st = await stats(b.page);
      check("the ring moves to that unit", st.m2?.ring_x === other.x && st.m2?.ring_y === other.y, `ring ${st.m2?.ring_x},${st.m2?.ring_y}`);
      await shot("04-flag");
    } else {
      check("a second unit on an other plot for the flag click", false, JSON.stringify(units));
    }

    // 5. The left arrow moves the camera.
    const g0 = focus(await stats(b.page));
    await b.page.keyboard.down("ArrowLeft");
    await sleep(900);
    await b.page.keyboard.up("ArrowLeft");
    await sleep(1500);
    const g1 = focus(await stats(b.page));
    check("the left arrow moves the camera west", g1[0] < g0[0] - 1, `focus_x ${g0[0].toFixed(1)} -> ${g1[0].toFixed(1)}`);

    st = await stats(b.page);
    check("ui.lua_errors == 0", st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
    num.m2 = st.m2;
  } catch (e) {
    await shot("failure");
    check("no scenario error", false, e.message);
    result.error = e.stack ?? String(e);
  }
  result.consoleErrors.push(...b.errors);
  await b.close();
  check("no console errors", b.errors.length === 0, `${b.errors.length} error(s)${b.errors[0] ? `: ${b.errors[0]}` : ""}`);
  result.pass = result.checks.every((c) => c.pass);
  result.durationMs = Date.now() - started;
  return result;
}
