// `m5-fix-meter` (live reports 2026-10-30): (1) the unit flag tooltip shows the hovered unit (not the type's default text);
// (2) the city screen's citizen growth meter fills from the bottom of the portrait circle.
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8970 --scenario m5-fix-meter
import { openPage, act, until, appUrl } from "../lib/browser.mjs";
import { uiDriver } from "../lib/ui-driver.mjs";
import { enterGame } from "../lib/steps-woj.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const stats = (page) => page.evaluate(() => {
  const s = window.__civ5stats ?? {};
  return { ui: s.ui ?? null, replica: s.replica ?? null, m2: s.m2 ?? null, camera: s.camera ?? null };
});

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "m5-fix-meter", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => { result.checks.push({ name, pass: !!pass, detail: detail ?? "" }); console.log(`  ${pass ? "ok  " : "FAIL"} ${name}${detail ? ` (${detail})` : ""}`); };
  const base = o.attach ?? ctx.server?.url;
  if (!base) { check("options", false, "m5-fix-meter needs --attach <url of a real-core server>"); return result; }
  const view = { width: 1600, height: 900 };
  const b = await openPage({ headed: o.headed, logDir: o.logDir, consoleFile: "console.log", viewport: view });
  const dir = process.env.SHOTS ?? o.logDir;
  const shot = async (name) => { try { await b.page.screenshot({ path: `${dir}/${name}.png` }); } catch { /* gone */ } };
  const num = result.numbers;
  try {
    await enterGame(b, o, base, { autostart: "1", ai: "1", size: "small", seed: "7", mode: "simultaneous", manual: "1" }, "mmeter", { act, until, appUrl }, stats);
    const ui = uiDriver(b);
    await sleep(3000);
    const units = (await ui.lua(`
      local p = Players[Game.GetActivePlayer()]
      for u in p:Units() do print(TAG .. u:GetID() .. " " .. GameInfo.Units[u:GetUnitType()].Type .. " " .. u:GetX() .. " " .. u:GetY()) end`))
      .map((l) => { const [id, type, x, y] = l.split(" "); return { id: Number(id), type, x: Number(x), y: Number(y) }; });
    num.units = units;
    const s = (await b.page.evaluate(() => window.__civ5stats?.camera?.ui_scale)) ?? 1;
    // 1. Tooltips of two flags of different units.
    const seen = new Map();
    for (const u of units.slice(0, 4)) {
      await ui.lua(`UI.ClearSelectionList() UI.LookAt(Map.GetPlot(${u.x}, ${u.y}))`);
      await sleep(2000);
      const flags = (await ui.dump("UnitFlagManager")).filter((r) => r.cls.includes("Button") && /\/Button$/.test(r.path) && r.w > 0);
      const mid = { x: view.width / 2, y: view.height / 2 };
      const d = (r) => Math.hypot((r.x + r.w / 2) * s - mid.x, (r.y + r.h / 2) * s - mid.y);
      const flag = flags.sort((p, q) => d(p) - d(q))[0];
      if (!flag) continue;
      await b.page.mouse.move(5, 5);
      await sleep(300);
      await b.page.mouse.move(Math.round((flag.x + flag.w / 2) * s), Math.round((flag.y + flag.h / 2) * s));
      await sleep(1200);
      await b.page.mouse.move(Math.round((flag.x + flag.w / 2) * s) + 1, Math.round((flag.y + flag.h / 2) * s) + 1);
      await sleep(1200);
      const rows = await ui.dump("");
      const tip = rows.filter((r) => /Tool/i.test(r.path) && r.text).map((r) => r.text).join(" | ");
      seen.set(u.type, tip);
      await shot(`tip-${u.type}`);
    }
    num.tips = Object.fromEntries(seen);
    const texts = [...seen.values()];
    check("a unit tooltip is shown", texts.length > 0 && texts.every((t) => t.length > 0), JSON.stringify(num.tips));
    check("the tooltip is not the default text", texts.every((t) => !/Brief description/.test(t)), JSON.stringify(num.tips));
    check("tooltips of different units differ", new Set(texts).size === texts.length && texts.length > 1, `${new Set(texts).size}/${texts.length}`);

    // 2. City screen meter: found a city by mouse, open its screen, the citizen meter is filled 30 % from the bottom.
    await b.page.mouse.move(1000, 700);
    const settler = units.find((u) => u.type === "UNIT_SETTLER");
    await ui.lua(`UI.SelectUnit(Players[Game.GetActivePlayer()]:GetUnitByID(${settler.id}))`);
    await sleep(1500);
    num.found = await ui.unitAction(/FOUND/);
    await sleep(4000);
    await ui.lua(`local c = Players[Game.GetActivePlayer()]:GetCapitalCity() if c then UI.DoSelectCityAtPlot(c:Plot()) end`);
    await sleep(4000);
    check("the city screen is open", await ui.cityScreenShown(), `found ${num.found}`);
    await ui.lua(`local m = LookUpControl("/InGame/CityView/PeopleMeter") if m then m:SetPercent(0.3) end print(TAG .. tostring(m))`);
    await sleep(800);
    await b.page.mouse.move(1000, 700);
    await shot("city-meter");
    {
      const st = await stats(b.page);
      check("ui.lua_errors == 0", st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
    }
  } catch (e) {
    await shot("failure");
    check("no scenario error", false, e.message);
    result.error = e.stack ?? String(e);
  }
  result.consoleErrors.push(...b.errors);
  await b.close();
  result.pass = result.checks.every((c) => c.pass);
  result.durationMs = Date.now() - started;
  return result;
}
