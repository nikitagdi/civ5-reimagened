// Attach to a Congress session with unspent delegates and a yes/no resolution.
import { openPage, act, until, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { clickPart } from "./ui2-v1.mjs";

export async function run({ server, opts }) {
  if (!opts.attach) throw new Error("congress requires --attach to an active voting session");
  const browser = await openPage({ headed: opts.headed, logDir: opts.logDir, viewport: { width: 1366, height: 768 }, consoleFile: "congress.log" });
  const result = { scenario: "ui2-screen-congress", pass: false, checks: [] };
  try {
    const page = browser.page, nick = "congress-test";
    await page.goto(appUrl(server.url, opts, { ui: "v2:engraved", nick, manual: "1" }));
    const first = await until(page, s => s.screen === "login" || s.screen === "play", 90000, "login or game");
    if (first.screen === "login") await act(page, { kind: "login", nick, password: DEV_PASSWORD });
    await until(page, s => s.screen === "play", 90000, "game");
    await page.evaluate(() => window.__civ5test.ui2Intent({ name: "window.open", args: { window: "congress" } }));
    await page.waitForFunction(() => window.__civ5test.ui2()?.texts.some(item => item.part === "congress.proposal-name"), null, { timeout: 30000 });
    const before = await page.evaluate(() => window.__civ5test.ui2().texts.filter(item => item.part === "congress.votes").map(item => item.text));
    if (!await clickPart(page, "congress.vote-up")) throw new Error("An available yes/no resolution is required");
    await page.waitForFunction(old => JSON.stringify(window.__civ5test.ui2()?.texts.filter(item => item.part === "congress.votes").map(item => item.text)) !== JSON.stringify(old), before, { timeout: 30000 });
    result.checks.push({ name: "draft vote appears after server action", pass: true });
    await clickPart(page, "congress.submit-votes");
    await clickPart(page, "congress.confirm");
    await page.waitForFunction(() => !window.__civ5test.ui2()?.ui.windows.includes("congress"), null, { timeout: 30000 });
    result.checks.push({ name: "submission consumes the voting opportunity", pass: true });
    result.pass = true;
    return result;
  } finally { await browser.close(); }
}
