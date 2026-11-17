// Requires an attached real game with a city and enough culture to open its first branch.
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
  const browser = await openPage({ headed: opts.headed, logDir: opts.logDir, viewport: { width: 1366, height: 768 }, consoleFile: "ui2-screen-policies.log" });
  try {
    const page = browser.page;
    const nick = "policy-test";
    await page.goto(appUrl(server.url, opts, { ui: "v2:engraved", nick, manual: "1" }));
    const first = await until(page, s => s.screen === "login" || s.screen === "play", 90000, "login or game");
    if (first.screen === "login") await act(page, { kind: "login", nick, password: DEV_PASSWORD });
    await until(page, s => s.screen === "play" && s.turn !== null, 90000, "prepared game");
    if (!await clickPart(page, "top.menu.policies")) throw new Error("policy menu missing");
    const before = await waitFor(page, p => texts(p, "policies.name").length >= 9, "real branches");
    if (!before.ui.windows.includes("policies")) throw new Error("policy window not registered");
    const rects = await page.evaluate(() => window.__civ5test.ui2Rects(["policies.unlock"]));
    const row = rects.filter(r => r.keys && !r.node.includes("#")).sort((a,b) => a.y-b.y || a.x-b.x)[0];
    if (!row) throw new Error("branch button missing");
    await page.mouse.click(row.x + row.w/2, row.y + row.h/2);
    await waitFor(page, p => Number(p.ui.state["policies.confirm"]) >= 0, "branch confirmation (fixture needs culture)");
    await clickPart(page, "policies.confirm");
    await waitFor(page, p => texts(p, "policies.status")[0] !== texts(before, "policies.status")[0], "branch adoption changes server data");
    await page.keyboard.press("Escape");
    await waitFor(page, p => !p.ui.windows.includes("policies"), "close");
    return { scenario: "ui2-screen-policies", pass: true, checks: [{ name: "real catalog, confirmation, adoption and close", pass: true }] };
  } finally { await browser.close(); }
}
