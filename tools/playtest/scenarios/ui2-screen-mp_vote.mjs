// Attach to a game with an active proposal on which the test seat can vote.
import { openPage, act, until, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { clickPart } from "./ui2-v1.mjs";

export async function run({ server, opts }) {
  if (!opts.attach) throw new Error("mp_vote requires --attach to an active proposal");
  const browser = await openPage({ headed: opts.headed, logDir: opts.logDir, viewport: { width: 1366, height: 768 }, consoleFile: "mp-vote.log" });
  const result = { scenario: "ui2-screen-mp_vote", pass: false, checks: [] };
  try {
    const page = browser.page, nick = "vote-test";
    await page.goto(appUrl(server.url, opts, { ui: "v2:engraved", nick, manual: "1" }));
    const first = await until(page, s => s.screen === "login" || s.screen === "play", 90000, "login or game");
    if (first.screen === "login") await act(page, { kind: "login", nick, password: DEV_PASSWORD });
    await until(page, s => s.screen === "play", 90000, "game");
    await page.evaluate(() => window.__civ5test.ui2Intent({ name: "window.open", args: { window: "mp_vote", a: 0 } }));
    await page.waitForFunction(() => window.__civ5test.ui2()?.texts.some(item => item.part === "mp-vote.proposal-title"), null, { timeout: 30000 });
    if (!await clickPart(page, "mp-vote.yes")) throw new Error("Vote yes is unavailable");
    await page.waitForFunction(() => { const p = window.__civ5test.ui2(); return p?.texts.some(item => item.part === "mp-vote.vote-state" && item.text === "yes") || !p?.ui.windows.includes("mp_vote"); }, null, { timeout: 30000 });
    result.checks.push({ name: "vote is recorded or the proposal resolves", pass: true });
    result.pass = true;
    return result;
  } finally { await browser.close(); }
}
