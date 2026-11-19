import { openPage, act, until, state, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { FOUND_CITY } from "../lib/policy.mjs";
import { clickPart } from "./ui2-v1.mjs";

export const probe = page => page.evaluate(() => window.__civ5test.ui2());
export const texts = (p, part) => (p?.texts ?? []).filter(t => t.part === part).map(t => t.text);
export async function waitFor(page, predicate, description) {
  const deadline = Date.now() + 30000;
  while (Date.now() < deadline) {
    const p = await probe(page);
    if (predicate(p)) return p;
    await page.waitForTimeout(150);
  }
  throw new Error(`Timed out waiting for ${description}`);
}
export async function intent(page, name, args = {}) {
  await page.evaluate(({ name, args }) => window.__civ5test.ui2Intent({ name, args }), { name, args });
}
export async function withHud({ server, opts }, scenario, test) {
  if (!opts.attach) throw new Error(`${scenario} requires --attach to a real-core server`);
  const browser = await openPage({ headed: opts.headed, logDir: opts.logDir, viewport: { width: 1366, height: 768 }, consoleFile: `${scenario}.log` });
  const result = { scenario, pass: false, checks: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  try {
    const page = browser.page;
    const nick = `hud-test-${Date.now() % 1000000}`;
    await page.goto(appUrl(server.url, opts, { ui: "v2:engraved", nick, autostart: "1", ai: "2", size: "small", seed: "7", mode: "simultaneous", manual: "1" }));
    const first = await until(page, s => s.screen === "login" || s.screen === "play", 90000, "login or game");
    if (first.screen === "login") await act(page, { kind: "login", nick, password: DEV_PASSWORD });
    await until(page, s => s.screen === "play" && s.turn !== null, 90000, "running game");
    await waitFor(page, p => p?.state === "running" && p.frames > 10, "UI v2");
    if (!(await state(page)).cities?.length) {
      const settler = ((await state(page)).units ?? []).find(u => /settler/i.test(u.name));
      if (!settler) throw new Error("A settler is required");
      await act(page, { kind: "selectUnit", id: settler.id });
      await until(page, s => s.selectedUnit === settler.id, 10000, "selected settler");
      await act(page, { kind: "unitAction", action: FOUND_CITY });
      await until(page, s => (s.cities?.length ?? 0) > 0, 30000, "founded city");
    }
    await intent(page, "window.close", { window: "city" });
    await test(page, check);
    check("no browser errors", browser.errors.filter(e => !e.includes("/pkg-mt/")).length === 0);
    result.pass = result.checks.every(c => c.pass);
    return result;
  } finally { await browser.close(); }
}

export async function run(context) {
  return withHud(context, "ui2-screen-top_panel", async (page, check) => {
    check("real science shown", texts(await probe(page), "top.yield.science").some(t => /\d/.test(t)));
    if (!await clickPart(page, "top.yield.science")) throw new Error("Technology button missing");
    await waitFor(page, p => p.ui.windows.includes("techs"), "technology window");
    check("science opens registered technology window", true);
    await intent(page, "window.close", { window: "techs" });
    const rects = await page.evaluate(() => window.__civ5test.ui2Rects(["top.yield.gold"]));
    const r = rects.find(r => r.w > 0 && r.h > 0);
    if (!r) throw new Error("Gold yield missing");
    await page.mouse.move(r.x + r.w / 2, r.y + r.h / 2);
    // The original top panel's gold tooltip: its own text lines (treasury, income, expenses), every key resolved.
    const tip = await waitFor(page, p => texts(p, "top-panel.tip").some(t => /gold/i.test(t) && /\d/.test(t)), "gold tooltip lines");
    const lines = texts(tip, "top-panel.tip");
    check("gold tooltip shows the original lines", lines.length > 3, lines.join(" | "));
    check("gold tooltip has no raw text keys or markup", lines.every(t => !/TXT_KEY_|\[|\]/.test(t)), lines.join(" | "));
  });
}
