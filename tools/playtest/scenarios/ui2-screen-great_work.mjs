import { openPage, act, until, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { clickPart } from "./ui2-v1.mjs";

export async function run({ server, opts }) {
  if (!opts.attach) throw new Error("great_work requires --attach to a real-core server");
  const browser = await openPage({ headed: opts.headed, logDir: opts.logDir, viewport: { width: 1366, height: 768 }, consoleFile: "great-work.log" });
  const result = { scenario: "ui2-screen-great_work", pass: false, checks: [] };
  try {
    const page = browser.page, nick = "viewer-test";
    await page.goto(appUrl(server.url, opts, { ui: "v2:engraved", nick, autostart: "1", ai: "2", seed: "7", manual: "1" }));
    const first = await until(page, s => s.screen === "login" || s.screen === "play", 90000, "login or game");
    if (first.screen === "login") await act(page, { kind: "login", nick, password: DEV_PASSWORD });
    await until(page, s => s.screen === "play" && s.turn !== null, 90000, "game");
    await page.evaluate(() => window.__civ5test.ui2Intent({ name: "window.open", args: { window: "culture" } }));
    await page.evaluate(() => window.__civ5test.ui2Intent({ name: "window.open", args: { window: "great_work", a: -1 } }));
    await page.waitForFunction(() => window.__civ5test.ui2()?.texts.some(text => text.part === "great-work.name" && text.text.length > 0), null, { timeout: 30000 });
    result.checks.push({ name: "preview contains real work metadata", pass: true });
    if (!await clickPart(page, "great-work.close")) throw new Error("Viewer close button is missing");
    await page.waitForFunction(() => { const p = window.__civ5test.ui2(); return p?.ui.windows.includes("culture") && !p.ui.windows.includes("great_work"); });
    result.checks.push({ name: "closing viewer preserves culture overview", pass: true });
    result.pass = result.checks.every(check => check.pass);
    return result;
  } finally { await browser.close(); }
}
