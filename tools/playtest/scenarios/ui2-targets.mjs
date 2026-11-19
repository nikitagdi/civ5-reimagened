// `ui2-targets`: the map's targeting mode of UI v2 (the original's interface modes that ask for a plot), live
// against a real-core server started with CIV5_TEST_LUA=tools/playtest/fixtures/targets.lua (a capital with a
// crossbowman and a barbarian warrior two plots away):
//   node tools/playtest/run.mjs --scenario ui2-targets --attach http://127.0.0.1:8880 --out <dir>
// Env: UI2_SHOT (path prefix for screenshots).
//
// 1. The crossbowman's ranged attack by its key (B): the map marks the plots the server lists (the warrior) and the
//    radius, the hint names the order; Escape leaves the mode and the marks go.
// 2. A right click leaves the mode without moving the unit; a click on a plot that is not listed does nothing.
// 3. A left click on the warrior's plot gives the ranged attack; the mode ends; the crossbowman has nothing more to
//    shoot at this turn.
import { openPage, act, until, appUrl, state } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { probe, waitFor, intent } from "./ui2-screen-top_panel.mjs";
import { partBox } from "./ui2-v1.mjs";

const VIEW = { width: 1600, height: 900 };
const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
const sel = p => p?.ui?.selection ?? {};

export async function run({ server, opts }) {
  if (!opts.attach) throw new Error("ui2-targets requires --attach to a real-core server");
  const shotPrefix = process.env.UI2_SHOT;
  const browser = await openPage({ headed: opts.headed, logDir: opts.logDir, viewport: VIEW, consoleFile: "ui2-targets.log" });
  const result = { scenario: "ui2-targets", pass: false, checks: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const shot = async (page, label, clip) => {
    if (shotPrefix) await page.screenshot({ path: `${shotPrefix}-${label}.png`, ...(clip ? { clip } : {}) });
  };
  try {
    const page = browser.page;
    const nick = `targets${Date.now() % 1000000}`;
    await page.goto(appUrl(server.url, opts, { ui: "v2:engraved", nick, autostart: "1", ai: "2", size: "small", seed: "7", mode: "simultaneous", manual: "1" }));
    const first = await until(page, s => s.screen === "login" || s.screen === "play", 90000, "login or game");
    if (first.screen === "login") await act(page, { kind: "login", nick, password: DEV_PASSWORD });
    await until(page, s => s.screen === "play" && s.turn !== null, 90000, "running game");
    await waitFor(page, p => p?.state === "running" && p.frames > 10 && (p.counters["ui2.anchors.shown"] ?? 0) > 0, "UI v2 with flags on the map");
    const s = await until(page, v => v.units?.some(u => /crossbow/i.test(u.name)), 30000, "the fixture's crossbowman").catch(() => null);
    const xb = s?.units.find(u => /crossbow/i.test(u.name));
    check("0. the fixture gave seat 0 a crossbowman", !!xb, JSON.stringify(s?.units?.map(u => u.name) ?? null));
    if (!xb) return result;
    await intent(page, "window.close", { window: "city" });
    await intent(page, "unit.select", { unit: `0:${xb.id}` });
    await waitFor(page, p => same(sel(p).unit, [0, xb.id]), "the crossbowman selected");
    await page.mouse.move(VIEW.width / 2, VIEW.height / 2 - 60);
    // The icons and terrain textures of a fresh page stream in after the first frames (screenshots only).
    await page.waitForTimeout(shotPrefix ? 10000 : 600);

    // 1. The ranged attack's key: the targeting mode with the server's plots.
    const enter = async label => {
      await page.keyboard.press("b");
      let p = await waitFor(page, q => sel(q).mode === "Target" && sel(q).targets?.plots?.length > 0 && q.preview?.targets?.length > 0, `targeting (${label})`).catch(() => null);
      if (!p) {
        // The key did not reach the order: the same intent its button sends.
        check(`${label}: the B key starts the ranged attack`, false, JSON.stringify(sel(await probe(page))));
        await intent(page, "map.target", { kind: "unit", unit: `0:${xb.id}`, a: -1 });
        p = await waitFor(page, q => sel(q).mode === "Target" && sel(q).targets?.plots?.length > 0, `targeting by intent (${label})`).catch(() => null);
      }
      return p;
    };
    const aiming = await enter("1");
    const t = sel(aiming).targets;
    result.targets = t;
    check("1. B starts the ranged attack's targeting mode", !!aiming && sel(aiming).target?.kind === "unit", JSON.stringify(sel(aiming).target ?? null));
    check("1. the server lists one plot (the warrior) and the radius", t?.plots?.length === 1 && t.area > 3 && t.look === "fire", JSON.stringify(t ?? null));
    check("1. the map marks exactly the listed plots", aiming && same(aiming.preview.targets, t.plots.map(([x, y]) => [x, y])) && (aiming.preview.aim?.length ?? 0) === t.area,
      JSON.stringify(aiming?.preview ?? null));
    check("1. the marks go through the original's styles (radius and targets)", aiming && aiming.preview.styles === t.area + t.plots.length, `${aiming?.preview?.styles} styled cells`);
    check("1. no reach and no path while aiming", aiming && aiming.preview.reachable.length === 0 && aiming.preview.path.length === 0, JSON.stringify(aiming?.preview ?? null));
    const hint = await partBox(page, "hud.aim");
    const texts = (aiming?.texts ?? []).filter(x => x.part === "hud.aim" || x.text === t?.name);
    check("1. the hint names the order", !!hint && texts.some(x => x.text === "Ranged Attack"), JSON.stringify({ hint, texts }));
    await shot(page, "aim");
    if (hint) await shot(page, "aim-hint", { x: Math.max(0, hint.x - 40), y: Math.max(0, hint.y - 10), width: Math.min(hint.w + 80, VIEW.width), height: hint.h + 20 });
    await page.keyboard.press("Escape");
    const left = await waitFor(page, p => sel(p).mode === "Select" && p.preview?.targets?.length === 0, "the mode left by Escape").catch(() => null);
    check("1. Escape leaves the mode and the marks go", !!left && left.preview.aim == null && !await partBox(page, "hud.aim"), JSON.stringify(left?.preview ?? null));
    check("1. the unit stays selected", left && same(sel(left).unit, [0, xb.id]));
    await page.waitForTimeout(400);
    await shot(page, "aim-cancelled");

    // Where the plots are on the screen: the plot under the mouse at a few points gives the scale of the map.
    const hoverAt = async (x, y) => {
      await page.mouse.move(x, y, { steps: 2 });
      await page.waitForTimeout(90);
      return (await probe(page)).preview?.hover ?? null;
    };
    const find = async target => {
      const c = { x: VIEW.width / 2, y: VIEW.height / 2 - 60 };
      const h0 = await hoverAt(c.x, c.y), hx = await hoverAt(c.x + 200, c.y), hy = await hoverAt(c.x, c.y + 150);
      if (!h0 || !hx || !hy) return null;
      const px = (hx[0] - h0[0]) / 200 || 1 / 100, py = (hy[1] - h0[1]) / 150 || -1 / 90;
      const guess = { x: c.x + (target[0] - h0[0]) / px, y: c.y + (target[1] - h0[1]) / py };
      for (let r = 0; r <= 120; r += 15) {
        for (let a = 0; a < (r === 0 ? 1 : 12); a++) {
          const x = guess.x + r * Math.cos(a * Math.PI / 6), y = guess.y + r * Math.sin(a * Math.PI / 6);
          if (x < 20 || y < 70 || x > VIEW.width - 20 || y > VIEW.height - 20) continue;
          if (same(await hoverAt(x, y), target)) return { x, y };
        }
      }
      return null;
    };
    const target = t?.plots?.[0];
    const at = target && await find(target);
    result.targetAt = at;
    check("2. the warrior's plot is found on the screen", !!at, JSON.stringify({ target }));

    // 2. A right click in the mode leaves it and does not move the unit; a click on a plot not listed does nothing.
    await enter("2");
    const before = (await state(page)).units.find(u => u.id === xb.id);
    if (at) {
      await page.mouse.click(at.x, at.y, { button: "right" });
      const out = await waitFor(page, p => sel(p).mode === "Select", "the mode left by a right click").catch(() => null);
      await page.waitForTimeout(800);
      const after = (await state(page)).units.find(u => u.id === xb.id);
      check("2. a right click leaves the mode and gives no order", !!out && same([after.x, after.y], [before.x, before.y]), JSON.stringify({ before: [before.x, before.y], after: [after.x, after.y] }));
      await enter("2b");
      await page.mouse.click(at.x + 220, at.y + 40);
      await page.waitForTimeout(700);
      const still = await probe(page);
      check("2. a click on a plot that is not listed does nothing", sel(still).mode === "Target", JSON.stringify(sel(still)));
      // 3. The warrior's plot: the ranged attack.
      await page.mouse.move(at.x, at.y, { steps: 3 });
      await page.waitForTimeout(700);
      await shot(page, "aim-hover", { x: Math.max(0, at.x - 300), y: Math.max(0, at.y - 220), width: 600, height: 440 });
      await page.mouse.click(at.x, at.y);
      const shotDone = await waitFor(page, p => sel(p).mode === "Select" && (p.ui.pending ?? []).length === 0, "the order applied").catch(() => null);
      check("3. a click on the listed plot gives the order and ends the mode", !!shotDone, JSON.stringify(sel(shotDone ?? await probe(page))));
      await page.waitForTimeout(1500);
      await shot(page, "attacked");
      // The crossbowman has shot this turn: its targeting lists nothing now.
      await intent(page, "map.target", { kind: "unit", unit: `0:${xb.id}`, a: -1 });
      const spent = await waitFor(page, p => sel(p).targets != null, "the targets after the shot").catch(() => null);
      check("3. after the shot the crossbowman has nothing to shoot at", spent && sel(spent).targets.plots.length === 0, JSON.stringify(sel(spent ?? {}).targets ?? null));
      await shot(page, "aim-spent");
      await intent(page, "map.cancel", {});
    }
    check("no browser errors", browser.errors.filter(e => !e.includes("/pkg-mt/")).length === 0, browser.errors.slice(0, 3).join(" | "));
    result.pass = result.checks.every(c => c.pass);
    return result;
  } finally { await browser.close(); }
}
