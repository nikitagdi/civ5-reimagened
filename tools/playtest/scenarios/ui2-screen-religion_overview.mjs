// Requires an attached real game awaiting an religion_overview choice.
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
  const browser = await openPage({ headed: opts.headed, logDir: opts.logDir, viewport: { width: 1366, height: 768 }, consoleFile: "ui2-screen-religion_overview.log" });
  try {
    const page = browser.page;
    const nick = "religion_overview-test";
    await page.goto(appUrl(server.url, opts, { ui: "v2:engraved", nick, manual: "1" }));
    const first = await until(page, s => s.screen === "login" || s.screen === "play", 90000, "login or game");
    if (first.screen === "login") await act(page, { kind: "login", nick, password: DEV_PASSWORD });
    await until(page, s => s.screen === "play" && s.turn !== null, 90000, "prepared game");
    // SIMPLIFIED(SIMP-369): the lead must expose the existing window intent to the test port.
    await page.evaluate(() => {
      if (!window.__civ5test.ui2Intent) throw new Error("Needs ui2Intent test-port adapter for window.open");
      window.__civ5test.ui2Intent("window.open", {window: "religion_overview"});
    });
    await waitFor(page, p => texts(p, "religion-overview.status").length > 0, "real faith status");
    const before = await waitFor(page, p => texts(p, "religion-overview.auto-label").length > 0, "auto-purchase label");
    await clickPart(page, "religion-overview.auto");
    const rects = await page.evaluate(() => window.__civ5test.ui2Rects(["religion-overview.option"]));
    const row = rects.filter(r => r.keys.includes("save_prophet") && !r.node.includes("#"))[0];
    if (!row) throw new Error("fixture must offer Save for a Great Prophet");
    await page.mouse.click(row.x + row.w/2, row.y + row.h/2);
    await waitFor(page, p => texts(p, "religion-overview.auto-label")[0] !== texts(before, "religion-overview.auto-label")[0], "server-confirmed auto-purchase label");
    await page.keyboard.press("Escape");
    await waitFor(page, p => !p.ui.windows.includes("religion_overview"), "close");
    return { scenario: "ui2-screen-religion_overview", pass: true, checks: [{ name: "real catalog, confirmation and religion_overview adoption", pass: true }] };
  } finally { await browser.close(); }
}
