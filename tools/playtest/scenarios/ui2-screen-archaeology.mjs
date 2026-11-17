// Attach to a save with an unresolved dig to exercise confirmation and the final choice.
import { openPage, act, until, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { clickPart } from "./ui2-v1.mjs";

export async function run({ server, opts }) {
  if (!opts.attach) throw new Error("archaeology requires --attach to a real-core server with an unresolved excavation");
  const browser = await openPage({ headed: opts.headed, logDir: opts.logDir, viewport: { width: 1366, height: 768 }, consoleFile: "archaeology.log" });
  const result = { scenario: "ui2-screen-archaeology", pass: false, checks: [] };
  try {
    const page = browser.page, nick = "dig-test";
    await page.goto(appUrl(server.url, opts, { ui: "v2:engraved", nick, autostart: "1", ai: "2", seed: "7", manual: "1" }));
    const first = await until(page, s => s.screen === "login" || s.screen === "play", 90000, "login or game");
    if (first.screen === "login") await act(page, { kind: "login", nick, password: DEV_PASSWORD });
    await until(page, s => s.screen === "play", 90000, "game");
    await page.evaluate(() => window.__civ5test.ui2Intent({ name: "window.open", args: { window: "archaeology" } }));
    await page.waitForFunction(([positive, unavailable]) => window.__civ5test.ui2()?.texts.some(t => t.text && (t.part === positive || t.part === unavailable)), ["archaeology.choice-title","archaeology.idle"], {timeout:30000});
    const unavailable = await page.evaluate(part => window.__civ5test.ui2()?.texts.filter(t => t.part === part).map(t => t.text).join(" "), "archaeology.idle");
    if (unavailable) {
      if (!/no completed excavation/i.test(unavailable)) throw new Error("Unexpected unavailable state");
      result.checks.push({name:"server reports unavailable game state",pass:true});
      result.pass=true; return result;
    }
    await page.waitForFunction(() => window.__civ5test.ui2()?.texts.some(text => text.part === "archaeology.choice-title"), null, { timeout: 30000 });
    const rows = await page.evaluate(() => window.__civ5test.ui2Rects(["archaeology.choice"]));
    const row = rows.find(item => !item.node.includes("#") && item.w > 0 && item.h > 0);
    if (!row) throw new Error("No excavation choice has visible geometry");
    await page.mouse.click(row.x + row.w / 2, row.y + row.h / 2);
    await page.waitForFunction(() => window.__civ5test.ui2()?.ui.state["archaeology.confirm"] > 0);
    await page.keyboard.press("Escape");
    await page.waitForFunction(() => window.__civ5test.ui2()?.ui.state["archaeology.confirm"] === 0);
    result.checks.push({ name: "Escape cancels confirmation and keeps excavation", pass: true });
    await page.mouse.click(row.x + row.w / 2, row.y + row.h / 2);
    if (!await clickPart(page, "archaeology.yes")) throw new Error("Confirmation button is missing");
    await page.waitForFunction(() => !window.__civ5test.ui2()?.ui.windows.includes("archaeology"), null, { timeout: 30000 });
    result.checks.push({ name: "accepted choice resolves the pending excavation", pass: true });
    result.pass = result.checks.every(check => check.pass);
    return result;
  } finally { await browser.close(); }
}
