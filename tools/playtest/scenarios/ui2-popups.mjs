// Start the real-core server with CIV5_TEST_POPUPS=0 to seed seat zero's queue.
// The three entries use the registered technology screen, so this fixture also
// runs before additional event-window skins have been integrated.
import { openPage, act, until, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { techCards } from "../lib/tech-screen.mjs";
import { ensureCity } from "../lib/screen-city.mjs";
import { counters } from "./ui2-host.mjs";

const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
const probe = page => page.evaluate(() => window.__civ5test.ui2());
const texts = (p, part) => (p?.texts ?? []).filter(t => t.part === part).map(t => t.text);

async function waitFor(page, predicate, description) {
  const deadline = Date.now() + 30000;
  while (Date.now() < deadline) {
    const p = await probe(page);
    if (predicate(p)) return p;
    await pause(100);
  }
  throw new Error(`Timed out waiting for ${description}`);
}

export async function run({ server, opts }) {
  if (!opts.attach) throw new Error("ui2-popups requires --attach and server CIV5_TEST_POPUPS=0");
  const result = { scenario: "ui2-popups", pass: false, checks: [] };
  const check = (name, pass) => result.checks.push({ name, pass: !!pass });
  const browser = await openPage({ headed: opts.headed, logDir: opts.logDir, viewport: { width: 1600, height: 900 }, consoleFile: "ui2-popups.log" });
  try {
    const page = browser.page;
    const nick = `popupfixture${Date.now() % 1000000}`;
    await page.goto(appUrl(server.url, opts, { ui: "v2:engraved", nick, autostart: "1", ai: "2", size: "small", seed: "7", mode: "simultaneous", manual: "1", stats: "1" }));
    const first = await until(page, s => s.screen === "login" || s.screen === "play", 90000, "login or game");
    if (first.screen === "login") await act(page, { kind: "login", nick, password: DEV_PASSWORD });
    await until(page, s => s.screen === "play" && s.turn !== null, 90000, "running game");
    const info = await waitFor(page, p => p?.ui?.popup && !p.ui.popup.decision && p.ui.windows.includes("techs") && p.ui.popup_count === 3, "first informational popup");
    const firstId = info.ui.popup.id;
    check("the server opens the first window without a click", true);
    await ensureCity(page); // research stays closed until the first city exists: the decision needs an available technology
    await page.keyboard.press("Escape");
    const decision = await waitFor(page, p => p?.ui?.popup?.decision && p.ui.popup.id > firstId && p.ui.popup_count === 2, "queued decision");
    const decisionId = decision.ui.popup.id;
    await page.keyboard.press("Escape");
    await pause(500);
    const still = await probe(page);
    check("Escape preserves a pending decision", still.ui.popup.id === decisionId && still.ui.windows.includes("techs"));
    const attached = await page.evaluate(() => {
      const client = window.__civ5test.metrics().client;
      return client.attachedReplay + client.attachedFresh;
    });
    await act(page, { kind: "fault", fault: "dropSocket" });
    await page.waitForFunction(before => {
      const client = window.__civ5test.metrics().client;
      return client.attachedReplay + client.attachedFresh > before;
    }, attached, { timeout: 30000 });
    await waitFor(page, p => p?.ui?.popup?.id === decisionId && p.ui.popup_count === 2 && p.ui.windows.includes("techs"), "decision after reconnect");
    check("reconnect retains the same queue entry", true);
    const catalog = await waitFor(page, p => texts(p, "techs.status").includes("available"), "available technology");
    const index = texts(catalog, "techs.status").indexOf("available");
    const row = (await techCards(page))[index];
    if (!row) throw new Error("available technology has no row geometry");
    await page.mouse.click(row.x + row.w / 2, row.y + row.h / 2);
    await waitFor(page, p => p?.ui?.popup && p.ui.popup.id > decisionId && !p.ui.popup.decision && p.ui.popup_count === 1, "successful action closes the decision and advances the queue");
    check("successful screen action completes its decision", true);
    await page.keyboard.press("Escape");
    await waitFor(page, p => p?.ui && p.ui.popup === null && p.ui.popup_count === 0 && !p.ui.windows.includes("techs"), "last dismissal");
    check("closing an informational window dismisses it exactly once", true);
    check("popup delivery keeps client core loads at zero", (await counters(page))["client.core.loads"] === 0);
    result.pass = result.checks.every(c => c.pass);
    return result;
  } finally {
    await browser.close();
  }
}
