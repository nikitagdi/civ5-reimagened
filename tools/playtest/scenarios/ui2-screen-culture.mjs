// A new game exercises populated city data, offers and visible move refusals.
import { openPage, act, until, state, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { FOUND_CITY } from "../lib/policy.mjs";
import { clickPart } from "./ui2-v1.mjs";

const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
const probe = page => page.evaluate(() => window.__civ5test.ui2());
const texts = (p, part) => (p?.texts ?? []).filter(item => item.part === part).map(item => item.text);
async function waitFor(page, predicate, description) {
  const deadline = Date.now() + 30000;
  while (Date.now() < deadline) {
    const value = await probe(page);
    if (predicate(value)) return value;
    await pause(150);
  }
  throw new Error(`Timed out waiting for ${description}`);
}

export async function run({ server, opts }) {
  if (!opts.attach) throw new Error("culture requires --attach to a real-core server");
  const result = { scenario: "ui2-screen-culture", pass: false, checks: [] };
  const browser = await openPage({ headed: opts.headed, logDir: opts.logDir, viewport: { width: 1366, height: 768 }, consoleFile: "culture.log" });
  try {
    const page = browser.page;
    const nick = "culture-test";
    await page.goto(appUrl(server.url, opts, { ui: "v2:engraved", nick, autostart: "1", ai: "2", seed: "7", size: "small", manual: "1" }));
    const first = await until(page, s => s.screen === "login" || s.screen === "play", 90000, "login or play");
    if (first.screen === "login") await act(page, { kind: "login", nick, password: DEV_PASSWORD });
    await until(page, s => s.screen === "play" && s.turn !== null, 90000, "running game");
    const world = await state(page);
    if (!world.cities?.length) {
      const settler = world.units.find(unit => /settler/i.test(unit.name));
      if (!settler) throw new Error("A settler is required to found a city");
      await act(page, { kind: "selectUnit", id: settler.id });
      await act(page, { kind: "unitAction", action: FOUND_CITY });
      await until(page, s => s.cities?.length > 0, 30000, "first city");
    }
    await page.evaluate(() => window.__civ5test.ui2Intent({ name: "window.open", args: { window: "culture", a: 0 } }));
    const root = await waitFor(page, p => texts(p, "culture.city-name").length > 0, "real culture cities");
    result.checks.push({ name: "culture displays own cities", pass: root.ui.windows.includes("culture") });
    await clickPart(page, "culture.tab.swap");
    await waitFor(page, p => p.ui.windows.includes("culture"), "swap tab");
    const rects = await page.evaluate(() => window.__civ5test.ui2Rects(["culture.swap-action"]));
    result.checks.push({ name: "swap action has visible geometry", pass: rects.some(rect => rect.w > 0 && rect.h > 0) });
    // Clearing an offer is legal even in a new game with no great works.
    await page.evaluate(() => window.__civ5test.ui2Intent({ name: "screen.act", args: { screen: "culture", action: "offer", a: 1, b: -1 } }));
    await pause(500);
    await page.evaluate(() => window.__civ5test.ui2Intent({ name: "screen.act", args: { screen: "culture", action: "move", a: -1, b: -1 } }));
    await waitFor(page, p => texts(p, "culture.error").some(text => text.includes("slot")), "visible refusal for an unavailable move");
    result.checks.push({ name: "unavailable move displays the server reason", pass: true });
    await clickPart(page, "culture.tab.victory");
    await clickPart(page, "culture.tab.influence");
    // The influence tab's texts (levels, trends and their tooltips) are resolved game text.
    const shown = ((await page.evaluate(() => window.__civ5test.ui2()))?.texts ?? []).filter(t => (t.part ?? "").startsWith("culture"));
    result.checks.push({ name: "influence tab shows resolved text", pass: shown.every(t => !/TXT_KEY_/.test(t.text)) });
    await page.keyboard.press("Escape");
    await waitFor(page, p => !p.ui.windows.includes("culture"), "closed culture");
    result.checks.push({ name: "Escape closes culture", pass: true });
    result.pass = result.checks.every(check => check.pass);
    return result;
  } finally { await browser.close(); }
}
