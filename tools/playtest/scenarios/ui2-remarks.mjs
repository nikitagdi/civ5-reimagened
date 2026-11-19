// `ui2-remarks`: owner remarks on the engraved HUD, live against a real-core server:
//   node tools/playtest/run.mjs --scenario ui2-remarks --attach http://127.0.0.1:8870 --out <dir>
// Env: UI2_DSF (device pixel ratio, default 1), UI2_SHOT (path prefix for screenshots).
import { writeFileSync } from "node:fs";
import { openPage, act, until, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { ensureCity } from "../lib/screen-city.mjs";
import { partBox, clickPart } from "./ui2-v1.mjs";
import { probe, texts, waitFor, intent } from "./ui2-screen-top_panel.mjs";

export async function run({ server, opts }) {
  if (!opts.attach) throw new Error("ui2-remarks requires --attach to a real-core server");
  const dsf = Number(process.env.UI2_DSF ?? 1);
  const shotPrefix = process.env.UI2_SHOT;
  const browser = await openPage({ headed: opts.headed, logDir: opts.logDir, viewport: { width: 1600, height: 900 }, ...(dsf === 1 ? {} : { deviceScaleFactor: null, chromeArgs: [`--force-device-scale-factor=${dsf}`] }), consoleFile: "ui2-remarks.log" });
  const result = { scenario: "ui2-remarks", pass: false, checks: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const shot = async (page, label, clip) => {
    if (!shotPrefix) return;
    if (dsf === 1) {
      await page.screenshot({ path: `${shotPrefix}-${label}.png`, ...(clip ? { clip } : {}) });
      return;
    }
    // A forced device scale: the page's own capture is at device pixels (Playwright's would be at CSS px).
    const cdp = await page.context().newCDPSession(page);
    const { data } = await cdp.send("Page.captureScreenshot", { format: "png", ...(clip ? { clip: { ...clip, scale: 1 } } : {}) });
    writeFileSync(`${shotPrefix}-${label}.png`, Buffer.from(data, "base64"));
    await cdp.detach();
  };
  try {
    const page = browser.page;
    const nick = `remarks${Date.now() % 1000000}`;
    await page.goto(appUrl(server.url, opts, { ui: "v2:engraved", nick, autostart: "1", ai: "2", size: "small", seed: "7", mode: "simultaneous", manual: "1" }));
    const first = await until(page, s => s.screen === "login" || s.screen === "play", 90000, "login or game");
    if (first.screen === "login") await act(page, { kind: "login", nick, password: DEV_PASSWORD });
    await until(page, s => s.screen === "play" && s.turn !== null, 90000, "running game");
    await waitFor(page, p => p?.state === "running" && p.frames > 10, "UI v2");
    await ensureCity(page);
    await intent(page, "window.close", { window: "city" });
    await intent(page, "window.close", { window: "production" });
    await waitFor(page, p => texts(p, "top.yield.science").length > 0, "HUD back");
    await page.waitForTimeout(1500);
    result.canvas = await page.evaluate(() => ({ dpr: devicePixelRatio, canvases: [...document.querySelectorAll("canvas")].map(c => [c.width, c.height, c.clientWidth, c.clientHeight, getComputedStyle(c).imageRendering]) }));
    check("the canvas backing store has one pixel per device pixel", result.canvas.canvases.every(([w, h, cw, ch]) => cw === 0 || (Math.abs(w - cw * Math.max(dsf, result.canvas.dpr)) <= 1 && Math.abs(h - ch * Math.max(dsf, result.canvas.dpr)) <= 1)), JSON.stringify(result.canvas));
    await shot(page, "hud");
    await shot(page, "topleft", { x: 0, y: 0, width: 520, height: 420 });

    // 5. No window without a close control in the top left corner at the start.
    const corner = await probe(page);
    result.windows = corner.ui.windows;
    check("5. no window open in the HUD at start", corner.ui.windows.length === 0, JSON.stringify(corner.ui.windows));
    check("5. the info corner is closed at start", !await partBox(page, "unit-lists.corner"));
    if (await clickPart(page, "top.menu.lists")) {
      await page.waitForTimeout(800);
      const open = await partBox(page, "unit-lists.corner");
      check("5. the strip's lists button opens the info corner", !!open);
      check("5. the open corner has a close control", !!await partBox(page, "unit-lists.close"));
      await shot(page, "corner-open", { x: 0, y: 0, width: 520, height: 420 });
      await page.mouse.move(800, 600);
      await page.keyboard.press("Escape");
      await page.waitForTimeout(500);
      check("5. Escape closes the info corner", !await partBox(page, "unit-lists.corner"));
      check("5. Escape opened no window", (await probe(page)).ui.windows.length === 0, JSON.stringify((await probe(page)).ui.windows));
    } else {
      check("5. the strip has a lists button", false);
    }

    // 1/2. The city banner.
    const banner = await partBox(page, "city-banner");
    if (banner) {
      const pad = 12;
      await shot(page, "banner", { x: Math.max(0, banner.x - pad), y: Math.max(0, banner.y - pad), width: banner.w + 2 * pad, height: banner.h + 2 * pad });
      result.banner = banner;
    }
    check("1. a city banner is on the map", !!banner);

    // 3/4. The technology tree.
    await intent(page, "window.open", { window: "techs" });
    const tree = await waitFor(page, p => texts(p, "techs.name").length > 50, "technology catalog");
    await page.waitForTimeout(800);
    const rects = await page.evaluate(() => window.__civ5test.ui2Rects(["techs.list"]));
    const byKey = new Map();
    for (const rect of rects) {
      if (!rect.keys || rect.keys.includes("/") || rect.node.includes("#") || Math.abs(rect.w - 232) > 2 || Math.abs(rect.h - 68) > 2) continue;
      const old = byKey.get(rect.keys);
      if (!old || rect.w * rect.h > old.w * old.h) byKey.set(rect.keys, rect);
    }
    const rows = [...byKey.values()].sort((a, b) => a.x - b.x || a.y - b.y);
    // A row of the second column, low on the screen: its tooltip must hang at the row, not in a fixed place.
    const row = rows.filter(r => r.x > 300 && r.x + r.w < 1500 && r.y + r.h < 860).sort((a, b) => b.y - a.y)[0] ?? rows[0];
    await page.mouse.move(row.x + 40, row.y + 20);
    await page.waitForTimeout(800);
    await shot(page, "tech-tip");
    const tip = await partBox(page, "techs.tip");
    result.row = row; result.tip = tip;
    check("4. the technology tooltip is shown", !!tip);
    // Left edges aligned (or pushed inside the screen), right under or right above the row, not over it.
    check("4. the technology tooltip hangs at its row", tip && Math.abs(tip.x - Math.min(row.x, 1600 - tip.w)) <= 2
      && (Math.abs(tip.y - (row.y + row.h)) <= 8 || Math.abs(tip.y + tip.h - row.y) <= 8), JSON.stringify({ row, tip }));
    // The unlock chips: 18 px squares inside the rows.
    const chips = rects.filter(r => r.w >= 16 && r.w <= 20 && r.h >= 16 && r.h <= 20 && r.keys && r.keys.includes("/"));
    result.chips = chips.length;
    const chip = chips.find(c => c.x > row.x && c.x < row.x + row.w && c.y > row.y && c.y < row.y + row.h) ?? chips[0];
    check("3. the rows show unlock chips", !!chip);
    if (chip) {
      await page.mouse.move(chip.x + chip.w / 2, chip.y + chip.h / 2);
      await page.waitForTimeout(800);
      await shot(page, "unlock-tip");
      const p = await probe(page);
      const name = texts(p, "techs.unlock-tip.name");
      result.unlock = { chip, name, kind: texts(p, "techs.unlock-tip.kind") };
      check("3. an unlock chip shows its own tooltip with the item's name", name.length > 0, JSON.stringify(result.unlock));
      check("3. the unlock tooltip names the kind", texts(p, "techs.unlock-tip.kind").length > 0);
      const utip = await partBox(page, "techs.unlock-tip");
      check("3. the unlock tooltip hangs at its chip", utip && Math.abs(utip.x - chip.x) < 400 && Math.abs(utip.y - chip.y) < 400, JSON.stringify(utip));
    }
    check("techs catalog loaded", texts(tree, "techs.name").length > 50);
    await page.keyboard.press("Escape");
    await waitFor(page, p => !p.ui.windows.includes("techs"), "techs closed");
    check("no browser errors", browser.errors.filter(e => !e.includes("/pkg-mt/")).length === 0, browser.errors.slice(0, 3).join(" | "));
    result.pass = result.checks.every(c => c.pass);
    return result;
  } finally { await browser.close(); }
}
