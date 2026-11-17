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
    await page.goto(appUrl(server.url, opts, { ui: "v2:engraved", nick, manual: "1" }));
    const first = await until(page, s => s.screen === "login" || s.screen === "play", 90000, "login or game");
    if (first.screen === "login") await act(page, { kind: "login", nick, password: DEV_PASSWORD });
    await until(page, s => s.screen === "play", 90000, "game");
    await page.evaluate(() => window.__civ5test.ui2Intent({ name: "window.open", args: { window: "city_state" } }));
    await page.waitForFunction(() => window.__civ5test.ui2()?.texts.some(item => item.part === "city-state.influence"), null, { timeout: 30000 });
    const before = await page.evaluate(() => window.__civ5test.ui2().texts.find(item => item.part === "city-state.influence").text);
    await clickPart(page, "city-state.gifts");
    const rows = await page.evaluate(() => window.__civ5test.ui2Rects(["city-state.gold-gift"]));
    const row = rows.filter(item => !item.node.includes("#") && item.w > 0 && item.h > 0).sort((a, b) => a.y - b.y)[0];
    if (!row) throw new Error("No gold gift row");
    await page.mouse.click(row.x + row.w / 2, row.y + row.h / 2);
    await page.waitForFunction(old => window.__civ5test.ui2()?.texts.some(item => item.part === "city-state.influence" && item.text !== old), before, { timeout: 30000 });
    result.checks.push({ name: "gold gift changes server influence", pass: true });
    await page.keyboard.press("Escape");
    await page.waitForFunction(() => !window.__civ5test.ui2()?.ui.windows.includes("city_state"));
    result.pass = true;
    return result;
  } finally { await browser.close(); }
}
