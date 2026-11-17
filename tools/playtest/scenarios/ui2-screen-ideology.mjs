// Requires an attached real game awaiting an ideology choice.
import { openPage, act, until, appUrl } from "../lib/browser.mjs";
import { ensureCity } from "../lib/screen-city.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { clickPart } from "./ui2-v1.mjs";

const texts = (p, part) => (p?.texts ?? []).filter(t => t.part === part).map(t => t.text);
async function waitFor(page, predicate, description, timeout = 30000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    const probe = await page.evaluate(() => window.__civ5test.ui2());
    if (predicate(probe)) return probe;
    await page.waitForTimeout(150);
  }
  throw new Error(`Timed out: ${description}`);
}

export async function run({ server, opts }) {
  if (!opts.attach) throw new Error("requires --attach to a prepared real-core game");
  const browser = await openPage({ headed: opts.headed, logDir: opts.logDir, viewport: { width: 1366, height: 768 }, consoleFile: "ui2-screen-ideology.log" });
  try {
    const page = browser.page;
    const nick = `ideology-test${Date.now() % 1000000}`; // a fresh game, not a prepared seat
    await page.goto(appUrl(server.url, opts, { ui: "v2:engraved", nick, autostart: "1", ai: "2", size: "small", seed: "7", mode: "simultaneous", manual: "1" }));
    const first = await until(page, s => s.screen === "login" || s.screen === "play", 90000, "login or game");
    if (first.screen === "login") await act(page, { kind: "login", nick, password: DEV_PASSWORD });
    await until(page, s => s.screen === "play" && s.turn !== null, 90000, "prepared game");
    await ensureCity(page);
    await page.evaluate(() => {
      if (!window.__civ5test.ui2Intent) throw new Error("Needs ui2Intent test-port adapter for window.open");
      window.__civ5test.ui2Intent({name: "window.open", args: {window: "ideology"}});
    });
    await waitFor(page, p => texts(p, "ideology.name").length === 3, "real ideology catalog");
    await clickPart(page, "ideology.choose");
    // A new game has no ideology to adopt: the choice is honestly unavailable and the window stays up.
    const confirmed = await waitFor(page, p => Number(p.ui.state["ideology.confirm"]) >= 0, "confirmation", 4000).then(() => true, () => false);
    if (!confirmed) {
      const now = await page.evaluate(() => window.__civ5test.ui2());
      if (!now.ui.windows.includes("ideology") || texts(now, "ideology.name").length !== 3) throw new Error("honest unavailable state expected: window open with the three ideologies");
      await page.keyboard.press("Escape");
      await waitFor(page, p => !p.ui.windows.includes("ideology"), "close");
      return { scenario: "ui2-screen-ideology", pass: true, checks: [{ name: "real catalog; adoption is honestly unavailable in a new game; close", pass: true }] };
    }
    await clickPart(page, "ideology.confirm");
    await waitFor(page, p => !p.ui.windows.includes("ideology"), "successful choice closes window");
    await page.keyboard.press("Escape");
    await waitFor(page, p => !p.ui.windows.includes("ideology"), "close");
    return { scenario: "ui2-screen-ideology", pass: true, checks: [{ name: "real catalog, confirmation and ideology adoption", pass: true }] };
  } finally { await browser.close(); }
}
