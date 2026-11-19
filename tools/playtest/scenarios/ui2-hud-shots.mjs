// `ui2-hud-shots`: the HUD of the engraved skin in its states, live against a real-core server, for a side by side
// look with the approved design:
//   node tools/playtest/run.mjs --scenario ui2-hud-shots --attach http://127.0.0.1:8870 --out <dir>
// Env: UI2_SHOT (path prefix for the screenshots: <prefix>-hud-1920.png, -hud-1366.png, -hud-menu.png, -hud-chat.png,
// -hud-unit-<n>.png, -hud-city-banners.png, -hud-research-choose.png).
import { openPage, act, until, appUrl, state } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { ensureCity } from "../lib/screen-city.mjs";
import { partBox, clickPart } from "./ui2-v1.mjs";
import { probe, texts, waitFor, intent } from "./ui2-screen-top_panel.mjs";

export async function run({ server, opts }) {
  if (!opts.attach) throw new Error("ui2-hud-shots requires --attach to a real-core server");
  const prefix = process.env.UI2_SHOT;
  const browser = await openPage({ headed: opts.headed, logDir: opts.logDir, viewport: { width: 1920, height: 1080 }, consoleFile: "ui2-hud-shots.log" });
  const result = { scenario: "ui2-hud-shots", pass: false, checks: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const shot = async (page, label, clip) => {
    if (prefix) await page.screenshot({ path: `${prefix}-${label}.png`, ...(clip ? { clip } : {}) });
  };
  try {
    const page = browser.page;
    const nick = `shots${Date.now() % 1000000}`;
    await page.goto(appUrl(server.url, opts, { ui: "v2:engraved", nick, autostart: "1", ai: "2", size: "small", seed: "7", mode: "simultaneous", manual: "1" }));
    const first = await until(page, s => s.screen === "login" || s.screen === "play", 90000, "login or game");
    if (first.screen === "login") await act(page, { kind: "login", nick, password: DEV_PASSWORD });
    await until(page, s => s.screen === "play" && s.turn !== null, 90000, "running game");
    await waitFor(page, p => p?.state === "running" && p.frames > 10, "UI v2");
    // The game's pictures come with the UI set, which loads after the skin: wait for it and for their textures.
    const pt0 = Date.now();
    let pc = {};
    while (Date.now() - pt0 < 90000) {
      pc = (await page.evaluate(() => (window.__civ5test?.ui2 ? window.__civ5test.ui2() : null)).catch(() => null))?.counters ?? {};
      if (pc["ui2.assets.set"] === 1 && pc["ui2.assets.textures"] > 0 && pc["ui2.assets.resident"] === pc["ui2.assets.textures"]) break;
      await page.waitForTimeout(250);
    }
    check("the game's pictures are drawn", pc["ui2.assets.set"] === 1 && pc["ui2.assets.resident"] === pc["ui2.assets.textures"],
      JSON.stringify({ ms: Date.now() - pt0, set: pc["ui2.assets.set"], textures: pc["ui2.assets.textures"], resident: pc["ui2.assets.resident"], missed: pc["ui2.assets.missed"] }));
    await page.waitForTimeout(1000);
    // Before the first city: nothing is researched, the research block asks for a choice.
    check("the research block asks for a choice before the first research", !!await partBox(page, "top.research.pick"));
    await ensureCity(page);
    await intent(page, "window.close", { window: "city" });
    await intent(page, "window.close", { window: "production" });
    await waitFor(page, p => texts(p, "top.yield.science").length > 0, "HUD back");
    await page.waitForTimeout(1500);
    await shot(page, "hud-research-choose");
    // A unit selected: its flag on the map.
    if (await clickPart(page, "unit-flag")) await waitFor(page, p => texts(p, "unit.title").length > 0, "selected unit");
    // The pointer leaves the map objects: no tooltip over the banner.
    await page.mouse.move(1200, 760);
    await page.waitForTimeout(1500);
    await shot(page, "hud-1920");
    const banner = await partBox(page, "city-banner");
    if (banner) await shot(page, "hud-city-banners", { x: Math.max(0, banner.x - 40), y: Math.max(0, banner.y - 30), width: Math.min(1920, banner.w + 80), height: banner.h + 60 });
    check("a city banner is on the map", !!banner, JSON.stringify(banner));
    // Every own unit in turn: the panel of each (the worker's has the most orders).
    const flags = ((await page.evaluate(() => window.__civ5test.ui2Rects(["unit-flag"])).catch(() => [])) ?? [])
      .filter(r => r.part === "unit-flag" && r.w > 20 && r.h > 40).slice(0, 4);
    result.units = ((await state(page)).units ?? []).map(u => u.name);
    let i = 0;
    for (const f of flags) {
      await page.mouse.click(f.x + f.w / 2, f.y + 14);
      await page.waitForTimeout(1200);
      const box = await partBox(page, "unit");
      if (box) await shot(page, `hud-unit-${i}`, { x: 0, y: Math.max(0, box.y - 120), width: 760, height: 1080 - Math.max(0, box.y - 120) });
      i += 1;
    }
    await clickPart(page, "top.menu.fold");
    await page.waitForTimeout(600);
    const military = await partBox(page, "top.menu.military");
    check("the more menu opens", !!military);
    // The pointer goes down into the menu, as a player's would: the button's tooltip leaves.
    if (military) await page.mouse.move(military.cx, military.cy + 60);
    await page.waitForTimeout(600);
    await shot(page, "hud-menu");
    await clickPart(page, "top.menu.fold");
    await clickPart(page, "top.chat");
    await page.waitForTimeout(600);
    const field = await partBox(page, "chat.field");
    check("the chat dropdown opens", !!field);
    // A line in the log: the player writes to all.
    if (field) {
      await page.mouse.click(field.cx, field.cy);
      await page.keyboard.type("Anyone up for a research pact");
      await page.keyboard.press("Enter");
      await page.mouse.move(field.cx, field.cy - 30);
      await page.waitForTimeout(1500);
    }
    await shot(page, "hud-chat");
    await clickPart(page, "top.chat");
    await page.setViewportSize({ width: 1366, height: 768 });
    await page.waitForTimeout(1500);
    await shot(page, "hud-1366");
    const p = await probe(page);
    result.texts = { strip: ["top.yield.science", "top.yield.gold", "top.yield.trade", "top.yield.happiness", "top.yield.golden-age", "top.yield.culture", "top.yield.faith", "top.turn", "top.research", "helm.state", "end-turn.queue", "unit.title"].map(k => [k, texts(p, k)]) };
    check("no browser errors", browser.errors.filter(e => !e.includes("/pkg-mt/")).length === 0, browser.errors.slice(0, 3).join(" | "));
    result.pass = result.checks.every(c => c.pass);
    return result;
  } finally { await browser.close(); }
}
