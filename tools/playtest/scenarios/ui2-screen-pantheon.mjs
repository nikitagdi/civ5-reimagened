// Requires an attached real game awaiting an pantheon choice.
import { openPage, act, until, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { clickPart } from "./ui2-v1.mjs";

const texts = (p, part) => (p?.texts ?? []).filter(t => t.part === part).map(t => t.text);
async function waitFor(page, predicate, description) {
  const deadline = Date.now() + 30000;
  while (Date.now() < deadline) {
    const probe = await page.evaluate(() => window.__civ5test.ui2());
    if (predicate(probe)) return probe;
    await page.waitForTimeout(150);
  }
  throw new Error(`Timed out: ${description}`);
}

export async function run({ server, opts }) {
  if (!opts.attach) throw new Error("requires --attach to a prepared real-core game");
  const browser = await openPage({ headed: opts.headed, logDir: opts.logDir, viewport: { width: 1366, height: 768 }, consoleFile: "ui2-screen-pantheon.log" });
  try {
    const page = browser.page;
    const nick = "pantheon-test";
    await page.goto(appUrl(server.url, opts, { ui: "v2:engraved", nick, manual: "1" }));
    const first = await until(page, s => s.screen === "login" || s.screen === "play", 90000, "login or game");
    if (first.screen === "login") await act(page, { kind: "login", nick, password: DEV_PASSWORD });
    await until(page, s => s.screen === "play" && s.turn !== null, 90000, "prepared game");
    // SIMPLIFIED(SIMP-369): the lead must expose the existing window intent to the test port.
    await page.evaluate(() => {
      if (!window.__civ5test.ui2Intent) throw new Error("Needs ui2Intent test-port adapter for window.open");
      window.__civ5test.ui2Intent("window.open", {window: "pantheon"});
    });
    await waitFor(page, p => texts(p, "pantheon.name").length > 0, "real pantheon catalog");
    await clickPart(page, "pantheon.row");
    await waitFor(page, p => Number(p.ui.state["pantheon.confirm"]) >= 0, "confirmation");
    await clickPart(page, "pantheon.confirm");
    await waitFor(page, p => !p.ui.windows.includes("pantheon"), "successful choice closes window");
    await page.keyboard.press("Escape");
    await waitFor(page, p => !p.ui.windows.includes("pantheon"), "close");
    return { scenario: "ui2-screen-pantheon", pass: true, checks: [{ name: "real catalog, confirmation and pantheon adoption", pass: true }] };
  } finally { await browser.close(); }
}
