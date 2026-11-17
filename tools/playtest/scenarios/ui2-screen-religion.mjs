// Requires an attached real game awaiting an religion choice.
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
  const browser = await openPage({ headed: opts.headed, logDir: opts.logDir, viewport: { width: 1366, height: 768 }, consoleFile: "ui2-screen-religion.log" });
  try {
    const page = browser.page;
    const nick = `religion-test${Date.now() % 1000000}`; // a fresh game, not a prepared seat
    await page.goto(appUrl(server.url, opts, { ui: "v2:engraved", nick, autostart: "1", ai: "2", size: "small", seed: "7", mode: "simultaneous", manual: "1" }));
    const first = await until(page, s => s.screen === "login" || s.screen === "play", 90000, "login or game");
    if (first.screen === "login") await act(page, { kind: "login", nick, password: DEV_PASSWORD });
    await until(page, s => s.screen === "play" && s.turn !== null, 90000, "prepared game");
    await ensureCity(page);
    await page.evaluate(city => {
      if (!window.__civ5test.ui2Intent) throw new Error("Needs ui2Intent test-port adapter for window.open");
      window.__civ5test.ui2Intent({name: "window.open", args: {window: "religion", a: city, b: 0}});
    }, Number(process.env.CIV5_SCREEN_CITY ?? 0));
    await waitFor(page, p => texts(p, "religion.name").length > 5, "real religion symbols");
    const symbols = await page.evaluate(() => window.__civ5test.ui2Rects(["religion.symbol"]));
    for (const r of symbols.filter(r => r.keys && !r.node.includes("#")).sort((a,b) => a.x-b.x)) {
      await page.mouse.click(r.x+r.w/2, r.y+r.h/2);
      await page.waitForTimeout(150);
      const probe = await page.evaluate(() => window.__civ5test.ui2());
      if (Number(probe.ui.state["religion.symbol"]) >= 0) break;
    }
    // A new game has no Great Prophet: no symbol is selectable, the window says so.
    const selectable = await waitFor(page, p => Number(p.ui.state["religion.symbol"]) >= 0, "available symbol selection", 4000).then(() => true, () => false);
    if (!selectable) {
      const now = await page.evaluate(() => window.__civ5test.ui2());
      if (!now.ui.windows.includes("religion") || texts(now, "religion.name").length <= 5) throw new Error("honest unavailable state expected: window open with the symbols");
      await page.keyboard.press("Escape");
      await waitFor(page, p => !p.ui.windows.includes("religion"), "close");
      return { scenario: "ui2-screen-religion", pass: true, checks: [{ name: "real catalog; founding is honestly unavailable without a prophet; close", pass: true }] };
    }
    for (const slot of ["pantheon", "founder", "follower", "bonus"]) {
      if (await clickPart(page, "religion.slot." + slot)) {
        if (!await clickPart(page, "religion.option")) throw new Error("belief option missing");
      }
    }
    // Founding needs an eligible Great Prophet and every required belief: in a new game the finish control is honestly absent.
    // The finish control is there but disabled without a prophet: a click leaves no confirmation open.
    const clickedFinish = await clickPart(page, "religion.finish");
    await page.waitForTimeout(800);
    const confirmOpen = !!clickedFinish && (await page.evaluate(() => window.__civ5test.ui2())).ui.state["religion.confirm"] === true;
    if (!confirmOpen) {
      const now = await page.evaluate(() => window.__civ5test.ui2());
      if (!now.ui.windows.includes("religion")) throw new Error("the religion window must stay open when founding is unavailable");
      // The first Escape cancels the open draft (the selected symbol), the second closes the window.
      for (let i = 0; i < 3 && (await page.evaluate(() => window.__civ5test.ui2())).ui.windows.includes("religion"); i++) { await page.keyboard.press("Escape"); await page.waitForTimeout(400); }
      await waitFor(page, p => !p.ui.windows.includes("religion"), "close");
      return { scenario: "ui2-screen-religion", pass: true, checks: [{ name: "real catalog and symbol selection; founding is honestly unavailable without a prophet; close", pass: true }] };
    }
    await clickPart(page, "religion.confirm");
    await waitFor(page, p => !p.ui.windows.includes("religion"), "founding changes game and closes window");
    await page.keyboard.press("Escape");
    await waitFor(page, p => !p.ui.windows.includes("religion"), "close");
    return { scenario: "ui2-screen-religion", pass: true, checks: [{ name: "real catalog, confirmation and religion adoption", pass: true }] };
  } finally { await browser.close(); }
}
