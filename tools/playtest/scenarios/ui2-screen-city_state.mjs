// Requires an attached game with a met city-state and enough gold for its small gift.
import { openPage, act, until, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { clickPart } from "./ui2-v1.mjs";

export async function run({ server, opts }) {
  if (!opts.attach) throw new Error("city_state requires --attach with a known city-state and gold");
  const browser = await openPage({ headed: opts.headed, logDir: opts.logDir, viewport: { width: 1366, height: 768 }, consoleFile: "city-state.log" });
  const result = { scenario: "ui2-screen-city_state", pass: false, checks: [] };
  try {
    const page = browser.page, nick = "minor-test";
    await page.goto(appUrl(server.url, opts, { ui: "v2:engraved", nick, autostart: "1", ai: "2", seed: "7", manual: "1" }));
    const first = await until(page, s => s.screen === "login" || s.screen === "play", 90000, "login or game");
    if (first.screen === "login") await act(page, { kind: "login", nick, password: DEV_PASSWORD });
    await until(page, s => s.screen === "play", 90000, "game");
    await page.evaluate(() => window.__civ5test.ui2Intent({ name: "window.open", args: { window: "city_state" } }));
    await page.waitForFunction(([positive, unavailable]) => window.__civ5test.ui2()?.texts.some(t => t.text && (t.part === positive || t.part === unavailable)), ["city-state.influence","city-state.reason"], {timeout:30000});
    const unavailable = await page.evaluate(part => window.__civ5test.ui2()?.texts.filter(t => t.part === part).map(t => t.text).join(" "), "city-state.reason");
    if (unavailable) {
      if (!/no known city-state/i.test(unavailable)) throw new Error("Unexpected unavailable state");
      result.checks.push({name:"server reports unavailable game state",pass:true});
      result.pass=true; return result;
    }
    await page.waitForFunction(() => window.__civ5test.ui2()?.texts.some(item => item.part === "city-state.influence"), null, { timeout: 30000 });
    const before = await page.evaluate(() => window.__civ5test.ui2().texts.find(item => item.part === "city-state.influence").text);
    await clickPart(page, "city-state.gifts");
    const rows = await page.evaluate(() => window.__civ5test.ui2Rects(["city-state.gold-gift"]));
    const row = rows.filter(item => !item.node.includes("#") && item.w > 0 && item.h > 0).sort((a, b) => a.y - b.y)[0];
    if (!row) throw new Error("No gold gift row");
    await page.mouse.click(row.x + row.w / 2, row.y + row.h / 2);
    await page.waitForFunction(old => window.__civ5test.ui2()?.texts.some(item => item.part === "city-state.influence" && item.text !== old), before, { timeout: 30000 });
    result.checks.push({ name: "gold gift changes server influence", pass: true });
    // The tribute menu shows the core's numeric pressure (its breakdown is the buttons' tooltip).
    await clickPart(page, "city-state.gifts-back");
    await clickPart(page, "city-state.tribute");
    await page.waitForFunction(() => window.__civ5test.ui2()?.texts.some(t => t.part === "city-state.pressure" && /-?\d/.test(t.text)), null, { timeout: 30000 });
    result.checks.push({ name: "tribute menu shows the numeric pressure", pass: true });
    await page.keyboard.press("Escape");
    await page.waitForFunction(() => !window.__civ5test.ui2()?.ui.windows.includes("city_state"));
    result.pass = true;
    return result;
  } finally { await browser.close(); }
}
