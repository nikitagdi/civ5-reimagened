// Run against a real-core server with a freshly built engraved skin.
import { openPage, act, until, state, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { FOUND_CITY } from "../lib/policy.mjs";
import { clickPart } from "./ui2-v1.mjs";
import { counters } from "./ui2-host.mjs";

const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
const probe = page => page.evaluate(() => window.__civ5test.ui2());
const texts = (p, part) => (p?.texts ?? []).filter(t => t.part === part).map(t => t.text);

async function waitFor(page, predicate, description) {
  const deadline = Date.now() + 30000;
  while (Date.now() < deadline) {
    const p = await probe(page);
    if (predicate(p)) return p;
    await pause(150);
  }
  throw new Error(`Timed out waiting for ${description}`);
}

async function rows(page, part) {
  const rects = await page.evaluate(p => window.__civ5test.ui2Rects([p]), part);
  const byKey = new Map();
  for (const rect of rects ?? []) {
    if (!rect.keys || rect.node.includes("#") || rect.w <= 0 || rect.h <= 0) continue;
    const old = byKey.get(rect.keys);
    if (!old || rect.w * rect.h > old.w * old.h) byKey.set(rect.keys, rect);
  }
  return [...byKey.values()].sort((a, b) => a.y - b.y || a.x - b.x);
}

async function clickRow(page, row, button = "left") {
  if (!row) throw new Error("Expected row geometry");
  await page.mouse.click(row.x + Math.min(100, row.w / 3), row.y + row.h / 2, { button });
}

async function focusWorker(page) {
  for (const row of await rows(page, "production.list")) {
    if (row.y < 0 || row.y + row.h > 900) continue;
    const frame = (await probe(page)).frames;
    await clickRow(page, row);
    const deadline = Date.now() + 1500;
    while (Date.now() < deadline) {
      const p = await probe(page);
      if (p.frames >= frame + 3) {
        if (texts(p, "production.detail")[0] === "Worker") return;
        break;
      }
      await pause(100);
    }
  }
  throw new Error("Worker production option is missing");
}

export async function run({ server, opts }) {
  if (!opts.attach) throw new Error("ui2-hud-extras requires --attach to a real-core server");
  const result = { scenario: "ui2-hud-extras", pass: false, checks: [] };
  const check = (name, pass) => result.checks.push({ name, pass: !!pass });
  const browser = await openPage({ headed: opts.headed, logDir: opts.logDir, viewport: { width: 1600, height: 900 }, consoleFile: "ui2-hud-extras.log" });
  try {
    const page = browser.page;
    const nick = `hud${Date.now() % 1000000}`;
    await page.goto(appUrl(server.url, opts, { ui: "v2:engraved", nick, autostart: "1", ai: "2", size: "small", seed: "7", mode: "simultaneous", manual: "1", stats: "1" }));
    const first = await until(page, s => s.screen === "login" || s.screen === "play", 90000, "login or game");
    if (first.screen === "login") await act(page, { kind: "login", nick, password: DEV_PASSWORD });
    await until(page, s => s.screen === "play" && s.turn !== null, 90000, "running game");
    await waitFor(page, p => p?.state === "running" && p.frames > 10, "UI v2");
    await pause(1500);
    check("cold v2 has zero client core loads", (await counters(page))["client.core.loads"] === 0);

    // Research is closed until the first city stands: found it first.
    const settler = ((await state(page)).units ?? []).find(u => /settler/i.test(u.name));
    if (!settler) throw new Error("A settler is required");
    await act(page, { kind: "selectUnit", id: settler.id });
    await until(page, s => s.selectedUnit === settler.id, 10000, "selected settler");
    await act(page, { kind: "unitAction", action: FOUND_CITY });
    await until(page, s => (s.cities?.length ?? 0) > 0, 30000, "founded city");

    // Right-click keeps the research window open. The first choice starts research,
    // the second appends without changing the synchronized world's current head.
    if (!await clickPart(page, "top.research.pick")) throw new Error("Choose research button is missing");
    await waitFor(page, p => texts(p, "research.list").length > 2, "research options");
    const firstName = texts(await probe(page), "research.list")[0];
    const firstTech = (await rows(page, "research.list"))[0];
    await clickRow(page, firstTech, "right");
    await waitFor(page, p => texts(p, "top.research").includes(firstName), "current research");
    const nextTech = (await rows(page, "research.list")).find(r => r.keys !== firstTech.keys);
    await clickRow(page, nextTech, "right");
    await waitFor(page, p => texts(p, "research.queue").length > 0, "research queue from HUD service");
    check("queued research appears in the research window", (await rows(page, "research.queue")).length >= 1);
    await page.keyboard.press("Escape");
    await waitFor(page, p => !p.ui.windows.includes("research"), "closed research window");

    // The test hook `openCity` drives the old city window; UI v2 opens its production window from the city banner.
    if (!await clickPart(page, "city-banner")) throw new Error("City banner is missing");
    // The banner opens the full city screen; its production chooser opens the production window.
    await waitFor(page, p => texts(p, "city.name").length > 0, "city window");
    if (!await clickPart(page, "city.choose-production")) throw new Error("City production chooser is missing");
    await waitFor(page, p => texts(p, "production.head").length > 0 && texts(p, "production.list").includes("Worker"), "production window");
    await focusWorker(page);
    if (!await clickPart(page, "production.choose") && !await clickPart(page, "production.build-now")) throw new Error("Production choice button is missing");
    await waitFor(page, p => texts(p, "production.current").includes("Worker"), "current Worker production");
    await focusWorker(page);
    if (!await clickPart(page, "production.to-back")) throw new Error("Queue production button is missing");
    await waitFor(page, p => texts(p, "production.queue").filter(t => t === "Worker").length >= 2, "full city queue from HUD service");
    check("two equal production items occupy separate queue rows", (await rows(page, "production.queue")).length >= 2);
    await page.keyboard.press("Escape");
    await pause(1500);
    check("v2 queues leave client core loads at zero", (await counters(page))["client.core.loads"] === 0);
    check("service has no window", !(await probe(page)).ui.windows.includes("hud"));
    check("no browser errors", browser.errors.filter(e => !e.includes("/pkg-mt/")).length === 0);
    result.pass = result.checks.every(c => c.pass);
    return result;
  } finally {
    await browser.close();
  }
}
