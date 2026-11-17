// Remote screen acceptance against a real-core server started with m2/server.ps1.
import { openPage, act, until, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { clickPart } from "./ui2-v1.mjs";
import { ensureCity } from "../lib/screen-city.mjs";

const texts = (p, part) => (p?.texts ?? []).filter(t => t.part === part).map(t => t.text);
const probe = page => page.evaluate(() => window.__civ5test.ui2());
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));

async function waitFor(page, predicate, description) {
  const deadline = Date.now() + 30000;
  while (Date.now() < deadline) {
    const p = await probe(page);
    if (predicate(p)) return p;
    await pause(150);
  }
  throw new Error(`Timed out waiting for ${description}`);
}

export async function run({ server, opts }) {
  const result = { scenario: "ui2-screens", pass: false, checks: [] };
  const check = (name, pass) => result.checks.push({ name, pass });
  if (!opts.attach) throw new Error("ui2-screens requires --attach to a real-core server");
  const browser = await openPage({ headed: opts.headed, logDir: opts.logDir, viewport: { width: 1600, height: 900 }, consoleFile: "ui2-screens.log" });
  try {
    const page = browser.page;
    const nick = `screens${Date.now() % 1000000}`;
    await page.goto(appUrl(server.url, opts, { ui: "v2:engraved", nick, autostart: "1", ai: "2", size: "small", seed: "7", mode: "simultaneous", manual: "1" }));
    const first = await until(page, s => s.screen === "login" || s.screen === "play", 90000, "login or game");
    if (first.screen === "login") await act(page, { kind: "login", nick, password: DEV_PASSWORD });
    await until(page, s => s.screen === "play" && s.turn !== null, 90000, "running real game");
    await waitFor(page, p => p?.state === "running" && p.frames > 10, "UI v2");
    await ensureCity(page);
    if (!await clickPart(page, "top.menu.tech")) throw new Error("technology HUD button is missing");
    const before = await waitFor(page, p => texts(p, "techs.name").length > 50, "server technology catalog");
    check("techs window contains the full real catalog", before.ui.windows.includes("techs"));
    check("full-screen tree suppresses HUD panels", ["top.research", "notes.log", "tech-panel", "helm"].every(part => texts(before, part).length === 0));
    const statuses = texts(before, "techs.status");
    const index = statuses.indexOf("Available");
    if (index < 0) throw new Error("no available technology in new game");
    const name = texts(before, "techs.name")[index];
    const rects = await page.evaluate(() => window.__civ5test.ui2Rects(["techs.list"])); // the dump covers only the subtree of the first element of a part, so ask for the list that holds all rows; it also holds era headers and links: keep the 232 x 68 px cards
    const byKey = new Map();
    for (const rect of rects) {
      if (!rect.keys || rect.keys.includes("/") || rect.node.includes("#") || Math.abs(rect.w - 232) > 2 || Math.abs(rect.h - 68) > 2) continue;
      const old = byKey.get(rect.keys);
      if (!old || rect.w * rect.h > old.w * old.h) byKey.set(rect.keys, rect);
    }
    const rows = [...byKey.values()].sort((a,b) => a.x - b.x || a.y - b.y);
    const row = rows[index];
    if (!row) throw new Error("available technology has no row geometry");
    await page.mouse.click(row.x + row.w / 2, row.y + row.h / 2);
    await waitFor(page, p => texts(p, "techs.status")[index] === "Researching", "research action and refreshed server data");
    check("picking a technology changes server research", true);
    await page.keyboard.press("Escape");
    await waitFor(page, p => !p.ui.windows.includes("techs"), "Escape closes techs");
    await waitFor(page, p => texts(p, "top.research").includes(name), "HUD research after closing tree");
    check("Escape closes the registered window and restores updated HUD", true);
    // The test hook sends an intent as a skin's click does.
    await page.evaluate(() => window.__civ5test.ui2Intent({ name: "window.open", args: { window: "techs" } }));
    await waitFor(page, p => p.ui.windows.includes("techs"), "ui2Intent opens techs");
    await page.evaluate(() => window.__civ5test.ui2Intent({ name: "window.close", args: { window: "techs" } }));
    await waitFor(page, p => !p.ui.windows.includes("techs"), "ui2Intent closes techs");
    check("ui2Intent drives a window like a skin intent", true);
    result.pass = result.checks.every(c => c.pass);
    return result;
  } finally {
    await browser.close();
  }
}
