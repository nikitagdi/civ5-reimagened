// `ui2-v1`: acceptance of the engraved skin's screens in a live game, through the skin only. Two clients (alice hosts,
// bob joins) play one simultaneous game on the real-core server, both with `?ui=v2:engraved`:
//   cargo xtask playtest --scenario ui2-v1 --attach http://127.0.0.1:8860
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8860 --scenario ui2-v1 [--headed]
// With `UI2_MODS=hello` against a server that has the mod (`cargo xtask dev --wsl --mods
// examples/mods/hello/mods.toml ...`) a third client plays a quick game with the mod for step 9.
//
// The acceptance steps (each one reported as checks named `<n>. ...`):
//  1. UI v2 runs on both clients: every panel ok (no placeholder); no console errors (checked at the end).
//  2. The top strip shows the game's gold and science rate, the turn dial the turn and the year (`state()`).
//  3. The turn dial selects a waiting unit: the unit panel shows its name; the F key fortifies it within 2 s.
//  4. A click on alice's own city banner (`ui2Rects(['city-banner'])`) opens the production window; the Worker row
//     and Build now (Choose while the city builds nothing) make the Worker the current production (window and
//     `state()`); Add to queue puts a Warrior (or the unique unit of the civilisation that replaces it) in the
//     queue (the server's answer clears the row's waiting mark); the queue's up arrow moves it to the front; a
//     second one makes two equal items, shown as two rows. Hovering a row of the list shows its tooltip (name,
//     cost), and so does the strip's policies button.
//  5. Choose research (the dial): a click on a technology makes it the current research (the top strip).
//  6. A click on the dial ends the turn: the turn goes up by one in `state()` and on the dial.
//  7. `setUi classic` and back, each within 3 s (after the classic UI's first boot, which is measured); the same
//     connection all along.
//  8. With the production window open, the skin changes to `vellum` within 1 s and the window stays open; back to
//     `engraved` the same way.
//  9. (`UI2_MODS=hello`) the mod's panel `echo` stands in the slot `hud.right` (engraved) and in `mods` (vellum);
//     its Say hello button brings `Echoes N+1` back from the server.
// 10. R17: `digest('replica')` is the same before and after a series of UI-only actions (the production window,
//     hovering its rows, sorting by turns, its tabs, the research window), unless the server sent a batch between.
// Also: chat (a line alice types shows in bob's log) and `?ui2onmap=0` (bob's unit flags are off).
// Screenshots: <logs>/ui2-v1-*.png; the numbers: <logs>/ui2-v1.json.
import { writeFileSync } from "node:fs";
import { openPage, act, until, state, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD, startRoom } from "../lib/game.mjs";
import { attaches, switchTo, signed, yearText } from "./ui2-host.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const UP_MS = Number(process.env.UI2_UP_MS ?? 420000);
const VP = { width: 1600, height: 900 };

async function ui2(page) {
  return page.evaluate(() => (window.__civ5test?.ui2 ? window.__civ5test.ui2() : null)).catch(() => null);
}

function partTexts(probe, part) {
  return (probe?.texts ?? []).filter((t) => t.part === part).map((t) => t.text);
}

function partsOf(probe, prefix) {
  const out = [];
  for (const t of probe?.texts ?? []) if (t.part?.startsWith(prefix) && !out.includes(t.part)) out.push(t.part);
  return out;
}

const helmState = (p) => partTexts(p, "helm.state")[0] ?? null;

/** Polls `ui2()` until `pred(probe)` holds; returns the probe (or the last one on time out, with `ok` false). */
async function waitUi(page, pred, ms) {
  const t0 = Date.now();
  let probe = null;
  for (;;) {
    probe = await ui2(page);
    if (probe && pred(probe)) return { ok: true, probe, ms: Date.now() - t0 };
    if (Date.now() - t0 > ms) return { ok: false, probe, ms: Date.now() - t0 };
    await sleep(150);
  }
}

async function rectsOf(page, part) {
  const r = await page.evaluate((p) => window.__civ5test.ui2Rects([p]), part).catch(() => null);
  return Array.isArray(r) ? r : [];
}

function boxOf(rects) {
  if (!rects.length) return null;
  const x0 = Math.min(...rects.map((r) => r.x));
  const y0 = Math.min(...rects.map((r) => r.y));
  const x1 = Math.max(...rects.map((r) => r.x + r.w));
  const y1 = Math.max(...rects.map((r) => r.y + r.h));
  return { x: x0, y: y0, w: x1 - x0, h: y1 - y0, cx: (x0 + x1) / 2, cy: (y0 + y1) / 2, n: rects.length };
}

/**
 * The box of the first element of a `data-part` (the union of its subtree's rectangles, CSS px), or null when the
 * skin does not show it.
 */
export async function partBox(page, part) {
  return boxOf(await rectsOf(page, part));
}

export async function clickPart(page, part) {
  const b = await partBox(page, part);
  if (!b) return null;
  await page.mouse.click(b.cx, b.cy);
  return b;
}

/** The instances of a list under a part: per instance key, the box of its largest record, top to bottom. */
function rowsOf(rects) {
  const by = new Map();
  for (const r of rects) {
    if (!r.keys || r.node.includes("#")) continue;
    const best = by.get(r.keys);
    if (!best || r.w * r.h > best.w * best.h) by.set(r.keys, r);
  }
  return [...by.entries()].map(([keys, r]) => ({ keys, x: r.x, y: r.y, w: r.w, h: r.h, cx: r.x + r.w / 2, cy: r.y + r.h / 2 })).sort((a, b) => a.y - b.y);
}

const inside = (a, b) => a && b && a.x >= b.x - 0.5 && a.y >= b.y - 0.5 && a.x + a.w <= b.x + b.w + 0.5 && a.y + a.h <= b.y + b.h + 0.5;

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "ui2-v1", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  if (!o.attach) {
    check("options", false, "ui2-v1 needs --attach <url of the real-core server> (tools/playtest/m2/server.ps1)");
    result.durationMs = Date.now() - started;
    return result;
  }
  const base = o.attach;
  const mods = (process.env.UI2_MODS ?? "").trim();
  const n = result.numbers;
  n.mods = mods || null;
  const pages = [];
  const open = async (nick, extra, url) => {
    const b = await openPage({ headed: o.headed, logDir: o.logDir, viewport: VP, consoleFile: `console-ui2-v1-${nick}.log` });
    b.nick = nick;
    pages.push(b);
    await b.page.goto(url ?? appUrl(base, o, { nick, ui: "v2:engraved", stats: "1", manual: "1", ...extra }), { timeout: 180000 });
    return b;
  };
  const shot = async (b, name) => {
    for (let k = 0; k < 3; k++) {
      try { await b.page.bringToFront().catch(() => {}); await b.page.screenshot({ path: `${o.logDir}/ui2-v1-${name}.png`, timeout: 120000 }); return; } catch { /* retry */ }
    }
  };
  const signIn = async (b) => {
    const s = await until(b.page, (x) => x.screen === "login" || x.screen === "lobby", 90000, `${b.nick}: sign-in window or lobby`);
    if (s.screen === "login") await act(b.page, { kind: "login", nick: b.nick, password: DEV_PASSWORD });
    return until(b.page, (x) => x.screen === "lobby", 30000, `${b.nick}: the lobby`);
  };
  /** UI v2 runs and its helm shows a state. */
  const v2Up = async (b) => {
    const t0 = Date.now();
    for (;;) {
      const probe = await ui2(b.page);
      if (probe?.state === "classic") throw new Error(`${b.nick}: UI v2 fell back to the classic UI (see the console log)`);
      if (probe?.state === "running" && probe.frames > 30 && helmState(probe)) return Date.now() - t0;
      if (Date.now() - t0 > UP_MS) throw new Error(`${b.nick}: UI v2 is not running after ${UP_MS / 1000} s`);
      await sleep(250);
    }
  };

  let alice = null;
  let bob = null;
  try {
    const stamp = Date.now() % 100000;
    alice = await open(`v1a${stamp}`, {});
    await signIn(alice);
    await act(alice.page, { kind: "createRoom" });
    await until(alice.page, (x) => x.screen === "room", 15000, "the room");
    const roomName = `ui2 v1 ${alice.nick}`;
    await act(alice.page, { kind: "roomSetting", field: "name", value: roomName });
    await until(alice.page, (x) => x.net?.room?.name === roomName, 10000, "the room name");
    await act(alice.page, { kind: "roomSetting", field: "turnMode", value: "simultaneous" });
    await act(alice.page, { kind: "roomSetting", field: "minorCivs", value: 0 });
    // Bob's map layer is off (`ui2onmap=0`).
    bob = await open(`v1b${stamp}`, { ui2onmap: "0" });
    let s = await signIn(bob);
    s = await until(bob.page, (x) => x.net?.rooms?.some((r) => r.name === roomName), 15000, "the room in bob's list");
    await act(bob.page, { kind: "joinRoom", room: s.net.rooms.find((r) => r.name === roomName).id });
    await until(bob.page, (x) => x.screen === "room", 15000, "bob in the room");
    await until(alice.page, (x) => x.net?.room?.slots.some((y) => y.nick === bob.nick), 15000, "alice sees bob");
    await act(bob.page, { kind: "ready", value: true });
    s = await until(alice.page, (x) => x.net?.room?.slots.find((y) => y.nick === bob.nick)?.ready === true, 15000, "bob ready");
    for (const slot of s.net.room.slots.filter((y) => y.who === "open")) await act(alice.page, { kind: "slot", index: slot.index, occupant: "closed" });
    await until(alice.page, (x) => x.net?.room?.slots.every((y) => y.who !== "open") && x.net.room.canStart === true, 15000, "Start is on");
    const t0 = Date.now();
    // A server that ran games just before may refuse the start for a while (`max_games`): waited out and reported.
    n.start = await startRoom(alice.page, [[alice.nick, alice.page], [bob.nick, bob.page]]);
    n.toRunningMs = await Promise.all([v2Up(alice), v2Up(bob)]).then(() => Date.now() - t0 - n.start.waitedMs);
    await sleep(2000);

    await step1(alice, bob, check, n);
    await step2(alice.page, check, n);
    await chat(alice, bob, check, n, shot);
    await mapOff(alice, bob, check, n);
    await turnOne(alice, bob, check, n, shot);
    await step7(alice.page, check, n);
    await step8(alice, check, n, shot);
    await step10(alice, check, n);
    if (mods) await step9(open, base, o, mods, check, n, shot);
  } catch (e) {
    check("no scenario error", false, e.message);
    result.error = e.stack ?? String(e);
    for (const b of pages) {
      await shot(b, `failure-${b.nick}`);
      (n.lastProbe ??= {})[b.nick] = await ui2(b.page);
    }
  }
  const appErrors = (errs) => errs.filter((e) => !e.includes("/pkg-mt/"));
  for (const b of pages) {
    result.consoleErrors.push(...b.errors);
    const errs = appErrors(b.errors);
    check(`1. ${b.nick}: no console errors`, errs.length === 0, `${errs.length} error(s)${errs[0] ? `: ${errs[0]}` : ""}`);
  }
  try { writeFileSync(`${o.logDir}/ui2-v1.json`, JSON.stringify(n, null, 2)); } catch { /* no log dir */ }
  for (const b of pages) await b.close();
  result.pass = result.checks.every((x) => x.pass);
  result.durationMs = Date.now() - started;
  return result;
}

/** 1. Both clients run UI v2 with every panel ok. */
async function step1(alice, bob, check, n) {
  for (const b of [alice, bob]) {
    const p = await ui2(b.page);
    const st = await state(b.page);
    const panels = (p?.panels ?? []).map((x) => `${x.id}:${x.status}${x.reason ? `(${x.reason})` : ""}`);
    (n.step1 ??= {})[b.nick] = { active: st?.ui?.active, skin: p?.skin, panels };
    check(`1. ${b.nick}: UI v2 engraved on the screen, every panel ok (no placeholder)`,
      st?.ui?.active === "v2" && p?.skin === "engraved" && panels.length > 0 && (p.panels ?? []).every((x) => x.status === "ok"), panels.join(" "));
  }
}

/** 2. The top strip and the dial against `state()` (they settle within a few frames of the model; 10 s at most). */
async function step2(page, check, n) {
  let shown = null;
  let want = null;
  const deadline = Date.now() + 10000;
  for (;;) {
    const st = await state(page);
    const probe = await ui2(page);
    want = st?.top ? { gold: String(st.top.gold), science: signed(st.top.scienceRate), turn: String(st.turn), year: yearText(st.year) } : null;
    const date = partTexts(probe, "helm.dial");
    shown = {
      gold: partTexts(probe, "top.yield.gold")[0] ?? null,
      science: partTexts(probe, "top.yield.science")[0] ?? null,
      turn: date.find((t) => /^\d+$/.test(t)) ?? null,
      year: date.find((t) => /BC|AD/.test(t)) ?? null,
    };
    if ((want && ["gold", "science", "turn", "year"].every((k) => shown[k] === want[k])) || Date.now() > deadline) break;
    await sleep(250);
  }
  n.step2 = { shown, state: want };
  for (const k of ["gold", "science", "turn", "year"]) check(`2. strip ${k} == state()`, want && shown[k] === want[k], `shown ${JSON.stringify(shown[k])}, state ${JSON.stringify(want?.[k])}`);
}

/** Chat: a line typed into alice's chat field reaches bob's chat log. */
async function chat(alice, bob, check, n, shot) {
  const line = `hello from ${alice.nick} ${Date.now() % 1000}`;
  const field = await clickPart(alice.page, "chat.field");
  if (!field) throw new Error("alice: no chat field on the screen");
  await sleep(300);
  await alice.page.keyboard.type(line, { delay: 20 });
  await sleep(300);
  await alice.page.keyboard.press("Enter");
  const t0 = Date.now();
  const got = await waitUi(bob.page, (p) => partTexts(p, "chat.log").includes(line), 15000);
  const own = await waitUi(alice.page, (p) => partTexts(p, "chat.log").includes(line), 5000);
  n.chat = { line, field, bobMs: Date.now() - t0, bobLog: partTexts(got.probe, "chat.log").slice(-6), aliceLog: partTexts(own.probe, "chat.log").slice(-6) };
  check("chat: the line alice typed shows in bob's chat log", got.ok, `${got.ms} ms, ${JSON.stringify(n.chat.bobLog)}`);
  check("chat: alice's own log has the line", own.ok, JSON.stringify(n.chat.aliceLog));
  await shot(alice, "chat-alice");
  await shot(bob, "chat-bob");
  // The pointer leaves the field and the keyboard goes back to the game (Escape blurs the field).
  await alice.page.keyboard.press("Escape");
  await alice.page.mouse.click(VP.width / 2, VP.height / 2 - 120);
  await sleep(300);
}

/** `?ui2onmap=0`: bob's map layer is off, alice's flags are on the screen. */
async function mapOff(alice, bob, check, n) {
  const flagsA = await partBox(alice.page, "unit-flag");
  const flagsB = await partBox(bob.page, "unit-flag");
  const probeB = await ui2(bob.page);
  n.map = { alice: flagsA, bob: flagsB, bobAnchors: probeB?.anchors ?? null };
  check("map layer: alice's unit flags are on the screen", !!flagsA, JSON.stringify(flagsA));
  check("map layer: bob's flags are off with ?ui2onmap=0", flagsB === null, JSON.stringify(flagsB));
}

/** The name the production window's detail column shows (the focused option). */
const detailName = (p) => partTexts(p, "production.detail")[0] ?? null;

/** Opens the production window with a click on alice's first city banner; the probe once it is open. */
async function openByBanner(page) {
  let box = null;
  const t0 = Date.now();
  while (!box && Date.now() - t0 < 15000) {
    box = await partBox(page, "city-banner");
    if (!box) await sleep(300);
  }
  if (!box) return { ok: false, box, ms: Date.now() - t0 };
  await page.mouse.click(box.cx, box.cy);
  const w = await waitUi(page, (p) => partTexts(p, "production.head").length > 0, 5000);
  return { ...w, box };
}

/**
 * Clicks the rows of the production list one by one until the detail names `name` (a name, or a test of the name);
 * the row with the `name` it found, or null.
 */
async function focusRow(page, name) {
  const want = typeof name === "function" ? name : (n) => n === name;
  for (let pass = 0; pass < 2; pass++) {
    for (const row of rowsOf(await rectsOf(page, "production.list"))) {
      if (row.cy > VP.height - 8) continue;
      await page.mouse.click(row.x + Math.min(120, row.w / 3), row.cy);
      const p = await waitUi(page, (q) => want(detailName(q) ?? ""), 600);
      if (p.ok) return { ...row, name: detailName(p.probe) };
    }
  }
  return null;
}

/** The civilisation's warrior: `Warrior`, or the unique unit that replaces it (`Maori Warrior`, `Jaguar`, ...). */
const WARRIOR = (n) => /Warrior$/.test(n) || ["Jaguar", "Brute"].includes(n);

/**
 * Steps 3 to 6 on alice's client: the turn dial leads through the first turn (units waiting for orders, the city's
 * production, the research), then ends it.
 */
async function turnOne(alice, bob, check, n, shot) {
  const page = alice.page;
  const done = { unit: false, city: false, production: false, research: false, turn: false };
  n.helm = [];
  for (let guard = 0; guard < 30 && !done.turn; guard++) {
    const probe = await ui2(page);
    const st = helmState(probe);
    n.helm.push(st);
    if (st === "Unit needs orders") {
      await clickPart(page, "helm.dial");
      const sel = await waitUi(page, (p) => partTexts(p, "unit.title").length > 0 && partsOf(p, "unit.act.").length > 0, 5000);
      const title = partTexts(sel.probe, "unit.title");
      const acts = partsOf(sel.probe, "unit.act.");
      if (acts.includes("unit.act.found-city") && !done.city) {
        await clickPart(page, "unit.act.found-city");
        await until(page, (x) => (x.cities?.length ?? 0) > 0, 10000, "the new city").catch(() => null);
        done.city = ((await state(page))?.cities ?? []).length > 0;
        check("4. (before) the settler founds a city with the unit panel's Found button", done.city, JSON.stringify(title));
        await sleep(1500);
      } else if (acts.includes("unit.act.fortify") && !done.unit) {
        await step3(page, title, sel, check, n, shot);
        done.unit = true;
      } else {
        const pick = ["unit.act.fortify", "unit.act.sleep", "unit.act.skip"].find((a) => acts.includes(a));
        if (!pick) throw new Error(`no order for the selected unit ${JSON.stringify(title)}: ${JSON.stringify(acts)}`);
        await clickPart(page, pick);
        await sleep(800);
      }
    } else if (st === "Choose production") {
      if (done.production) throw new Error("production is still needed after the Worker was chosen");
      await step4(alice, check, n, shot);
      done.production = true;
    } else if (st === "Choose research") {
      if (done.research) throw new Error("research is still needed after a technology was chosen");
      await step5(alice, check, n, shot);
      done.research = true;
    } else if (st === "Next turn") {
      if (!done.unit) check("3. a unit that can fortify waited for orders", false, JSON.stringify(n.helm));
      // The turn is simultaneous: bob ends his first (through his own skin), then alice's dial ends hers.
      n.bobTurn = await endTurnBy(bob);
      await step6(alice, check, n, shot);
      done.turn = true;
    } else {
      await sleep(500);
    }
  }
  for (const k of ["unit", "production", "research", "turn"]) if (!done[k]) check(`steps 3-6: reached ${k}`, false, JSON.stringify(n.helm));
}

/** Bob's turn ends through his skin: his units sleep (or skip), then his dial ends the turn. */
async function endTurnBy(b) {
  const page = b.page;
  const seen = [];
  for (let guard = 0; guard < 16; guard++) {
    const st = helmState(await ui2(page));
    seen.push(st);
    if (st === "Unit needs orders") {
      await clickPart(page, "helm.dial");
      const sel = await waitUi(page, (p) => partsOf(p, "unit.act.").length > 0, 5000);
      const acts = partsOf(sel.probe, "unit.act.");
      const pick = ["unit.act.sleep", "unit.act.fortify", "unit.act.skip"].find((a) => acts.includes(a));
      if (!pick) break;
      await clickPart(page, pick);
      await sleep(800);
    } else if (st === "Next turn") {
      await clickPart(page, "helm.dial");
      await sleep(1000);
      seen.push(helmState(await ui2(page)));
      break;
    } else {
      await sleep(500);
    }
  }
  return seen;
}

/** 3. The dial selected a unit: the panel shows its name; F fortifies it within 2 s. */
async function step3(page, title, sel, check, n, shot) {
  const st = await state(page);
  const mine = (st?.units ?? []).filter((u) => title.includes(u.name));
  check("3. the dial selects a waiting unit: the unit panel shows its name", sel.ok && mine.length > 0, `title ${JSON.stringify(title)} in ${sel.ms} ms; units ${JSON.stringify((st?.units ?? []).map((u) => u.name))}`);
  await shot({ page }, "unit");
  const before = new Map(mine.map((u) => [u.id, u.activity]));
  const t0 = Date.now();
  await page.keyboard.press("f");
  let changed = null;
  while (Date.now() - t0 < 3000 && !changed) {
    const s = await state(page);
    changed = (s?.units ?? []).find((u) => before.has(u.id) && u.activity !== before.get(u.id)) ?? null;
    if (!changed) await sleep(100);
  }
  const ms = Date.now() - t0;
  n.step3 = { title, before: [...before.entries()], after: changed ? [changed.id, changed.activity] : null, ms };
  check("3. F fortifies the selected unit within 2 s", changed && ms <= 2000, JSON.stringify(n.step3));
  await sleep(500);
}

/** 4. The banner opens the production window; Worker + Build now; Warrior + Add to queue; the queue's up arrow. */
async function step4(alice, check, n, shot) {
  const page = alice.page;
  const p = (n.step4 = {});
  const w = await openByBanner(page);
  p.open = { ok: w.ok, ms: w.ms, banner: w.box, head: partTexts(w.probe, "production.head") };
  check("4. a click on alice's own banner opens the production window", w.ok, JSON.stringify(p.open));
  if (!w.ok) throw new Error("the production window did not open from the banner");
  await shot(alice, "banner-production");
  // Worker + Build now (Choose while nothing is built).
  const worker = await focusRow(page, "Worker");
  p.workerRow = worker;
  if (!worker) throw new Error(`no Worker row; the list: ${JSON.stringify(partTexts(await ui2(page), "production.list"))}`);
  const now = (await clickPart(page, "production.build-now")) ? "build-now" : (await clickPart(page, "production.choose")) ? "choose" : null;
  const built = await waitUi(page, (q) => partTexts(q, "production.current").includes("Worker"), 5000);
  const cities = (await state(page))?.cities ?? [];
  p.current = { button: now, ms: built.ms, texts: partTexts(built.probe, "production.current"), cities: cities.map((c) => [c.productionItem, c.productionTurns]) };
  check("4. Worker + Build now: the window shows Worker as current production", built.ok, JSON.stringify(p.current));
  check("4. Build now: the game builds it (state().cities: an item, the window's turns)",
    built.ok && cities.some((c) => c.productionItem >= 0 && p.current.texts.includes(String(c.productionTurns))), JSON.stringify(p.current.cities));
  // Warrior + Add to queue.
  const warrior = await focusRow(page, WARRIOR);
  const wn = warrior?.name;
  p.warrior = wn ?? null;
  if (!warrior) {
    check("4. Add to queue: a Warrior row", false, "no Warrior row in the list");
  } else {
    await clickPart(page, "production.to-back");
    const queued = await waitUi(page, (q) => partTexts(q, "production.queue").includes(wn) && !partTexts(q, "production.list").includes("Waiting"), 5000);
    p.queue = { ms: queued.ms, texts: partTexts(queued.probe, "production.queue") };
    check("4. Add to queue: the Warrior is in the queue, the waiting mark cleared by the server's answer", queued.ok, JSON.stringify(p.queue));
    // The up arrow of the Warrior's row: the left one of the two small buttons at the right end of the row.
    const qrects = await rectsOf(page, "production.queue");
    const rows = rowsOf(qrects);
    const row = rows[1] ?? null;
    let up = null;
    if (row) {
      // The two buttons are the square boxes at the right end (the turns before them is a narrow text box).
      const btns = qrects.filter((r) => r.keys === row.keys && !r.node.includes("#") && r.w <= 40 && Math.abs(r.w - r.h) < 2).sort((a, b) => b.x - a.x);
      const right = btns[0];
      up = btns.find((r) => right && r.x + r.w <= right.x + 0.5) ?? null;
    }
    p.upArrow = { rows: rows.length, up };
    if (up) await page.mouse.click(up.x + up.w / 2, up.y + up.h / 2);
    const moved = await waitUi(page, (q) => {
      const t = partTexts(q, "production.queue");
      return t.indexOf(wn) >= 0 && t.indexOf("Worker") >= 0 && t.indexOf(wn) < t.indexOf("Worker");
    }, 5000);
    p.afterUp = { ms: moved.ms, texts: partTexts(moved.probe, "production.queue") };
    check("4. the queue's up arrow moves the Warrior to the front", !!up && moved.ok, JSON.stringify(p.afterUp));
    // A second Warrior: two equal items in the queue are two rows (the queue is keyed by slot, not by item).
    const rows0 = rowsOf(await rectsOf(page, "production.queue")).length;
    const again = await focusRow(page, wn);
    if (again) await clickPart(page, "production.to-back");
    const two = await waitUi(page, (q) => partTexts(q, "production.queue").filter((t) => t === wn).length === 2 && !partTexts(q, "production.list").includes("Waiting"), 5000);
    const rows1 = rowsOf(await rectsOf(page, "production.queue")).length;
    p.twoEqual = { ms: two.ms, texts: partTexts(two.probe, "production.queue"), rows: [rows0, rows1] };
    check("4. two equal items in the queue show as two rows", !!again && two.ok && rows1 === rows0 + 1, JSON.stringify(p.twoEqual));
  }
  // The tooltip of a production row: the hovered option's name and its cost.
  const tipRow = (await focusRow(page, "Worker")) ?? null;
  if (tipRow) {
    await page.mouse.move(tipRow.x + Math.min(120, tipRow.w / 3), tipRow.cy);
    const tip = await waitUi(page, (q) => partTexts(q, "tip.option").includes("Worker"), 3000);
    p.rowTip = { ms: tip.ms, texts: partTexts(tip.probe, "tip.option") };
    check("4. a production row shows its tooltip while hovered", tip.ok && p.rowTip.texts.some((t) => / production$/.test(t)), JSON.stringify(p.rowTip));
    await shot(alice, "production-tooltip");
  } else {
    check("4. a production row shows its tooltip while hovered", false, "no Worker row to hover");
  }
  await shot(alice, "production");
  await page.keyboard.press("Escape");
  await waitUi(page, (q) => partTexts(q, "production.head").length === 0, 3000);
  // The tooltip of the strip's policies button.
  const pol = await partBox(page, "top.menu.policies");
  if (pol) await page.mouse.move(pol.cx, pol.cy);
  const ptip = await waitUi(page, (q) => partTexts(q, "tip.policies").length > 0, 3000);
  p.policiesTip = { ms: ptip.ms, texts: partTexts(ptip.probe, "tip.policies") };
  check("4. the policies button shows its tooltip while hovered", !!pol && ptip.ok && p.policiesTip.texts.includes("Social policies"), JSON.stringify(p.policiesTip));
  await shot(alice, "policies-tooltip");
  await page.mouse.move(VP.width / 2, VP.height / 2);
}

/** 5. Choose research: the dial opens the research window; a click on the first technology makes it current. */
async function step5(alice, check, n, shot) {
  const page = alice.page;
  await clickPart(page, "helm.dial");
  const open = await waitUi(page, (p) => partTexts(p, "research.list").length > 0, 5000);
  const r = (n.step5 = { ms: open.ms, list: partTexts(open.probe, "research.list").slice(0, 8), before: partTexts(open.probe, "top.research") });
  check("5. the dial on Choose research opens the research window", open.ok, JSON.stringify(r));
  if (!open.ok) throw new Error("the research window did not open");
  await shot(alice, "research");
  const rows = rowsOf(await rectsOf(page, "research.list"));
  const first = r.list[0];
  if (rows[0]) await page.mouse.click(rows[0].x + Math.min(120, rows[0].w / 3), rows[0].cy);
  const chosen = await waitUi(page, (p) => partTexts(p, "research.list").length === 0 && partTexts(p, "top.research").includes(first), 5000);
  r.first = first;
  r.row = rows[0] ?? null;
  r.strip = partTexts(chosen.probe, "top.research");
  check("5. a click on a technology makes it the current research (the top strip)", chosen.ok, `${chosen.ms} ms, chose ${JSON.stringify(first)}, strip ${JSON.stringify(r.strip)}`);
}

/** 6. A click on the dial ends the turn. */
async function step6(alice, check, n, shot) {
  const page = alice.page;
  const before = (await state(page)).turn;
  await clickPart(page, "helm.dial");
  let after = before;
  const t0 = Date.now();
  while (Date.now() - t0 < 60000) {
    after = (await state(page)).turn;
    if (after === before + 1) break;
    await sleep(250);
  }
  const dial = await waitUi(page, (p) => partTexts(p, "helm.dial").includes(String(before + 1)), 10000);
  n.step6 = { before, after, ms: Date.now() - t0, dial: partTexts(dial.probe, "helm.dial") };
  check("6. a click on the dial ends the turn: state().turn +1", after === before + 1, `turn ${before} -> ${after} in ${n.step6.ms} ms`);
  check("6. the dial shows the new turn", dial.ok, JSON.stringify(n.step6.dial));
  await sleep(1500);
  await shot(alice, "next-turn");
}

/** 7. To the classic UI and back, each within 3 s, on the same connection. */
async function step7(page, check, n) {
  const att0 = await attaches(page);
  // The classic layer may never have come up under UI v2: its first boot is measured, not limited.
  const boot = await switchTo(page, "classic", "classic", {}, 180000);
  await sleep(2000);
  const back0 = await switchTo(page, "v2:engraved", "v2");
  await sleep(1000);
  const toClassic = await switchTo(page, "classic", "classic");
  await sleep(1500);
  const back = await switchTo(page, "v2:engraved", "v2");
  await sleep(1000);
  const att1 = await attaches(page);
  const probe = await ui2(page);
  n.step7 = { boot, back0, toClassic, back, attaches: [att0, att1] };
  check("7. setUi classic within 3 s (warm, after the first boot)", toClassic.switchMs <= 3000 && toClassic.wakeMs !== null && toClassic.switchMs + toClassic.wakeMs <= 3000, JSON.stringify(toClassic));
  check("7. setUi v2:engraved back within 3 s", back.switchMs <= 3000 && back0.switchMs <= 3000, `${JSON.stringify(back)}; after the boot ${back0.switchMs} ms`);
  check("7. UI v2 is back with its texts", probe?.active === true && partTexts(probe, "top.yield.gold").length > 0, `${probe?.active}`);
  check("7. the same connection: no new attach", att0 !== null && att1 === att0, `${att0} -> ${att1}`);
}

/** One skin change on the screen: the time from the request to the new package applied (the page's clock). */
async function changeSkin(page, value, skin) {
  const pageT0 = await page.evaluate(() => performance.now());
  const wall0 = Date.now();
  await act(page, { kind: "setUi", value });
  let q = null;
  while (Date.now() - wall0 < 15000) {
    q = await ui2(page);
    if (q?.skin === skin && q?.reload?.skin === skin && q.reload.appliedAt > pageT0) break;
    await sleep(50);
  }
  const st = await state(page);
  return { skin: q?.skin, ms: q?.reload ? Math.round(q.reload.appliedAt - pageT0) : null, wallMs: Date.now() - wall0, windows: q?.ui?.windows, active: st?.ui?.active, head: partTexts(q, "production.head"), notice: st?.ui?.notice ?? null };
}

/** 8. engraved -> vellum -> engraved with the production window open. */
async function step8(alice, check, n, shot) {
  const page = alice.page;
  const w = await openByBanner(page);
  const p0 = await ui2(page);
  const s = (n.step8 = { before: { ok: w.ok, windows: p0?.ui?.windows, head: partTexts(p0, "production.head") } });
  check("8. the production window is open before the change", w.ok && p0?.ui?.windows?.includes("production"), JSON.stringify(s.before));
  const att0 = await attaches(page);
  s.toVellum = await changeSkin(page, "v2:vellum", "vellum");
  await sleep(800);
  await shot(alice, "vellum-production");
  const pv = await ui2(page);
  s.vellumPanels = (pv?.panels ?? []).map((x) => `${x.id}:${x.status}`);
  check("8. engraved -> vellum within 1 s, UI v2 stays on the screen", s.toVellum.skin === "vellum" && s.toVellum.ms !== null && s.toVellum.ms <= 1000 && s.toVellum.active === "v2", JSON.stringify(s.toVellum));
  check("8. vellum: the production window is still open", s.toVellum.windows?.includes("production") && partTexts(pv, "production.head").length > 0, JSON.stringify({ windows: s.toVellum.windows, head: partTexts(pv, "production.head") }));
  check("8. vellum: every panel ok", s.vellumPanels.length > 0 && (pv?.panels ?? []).every((x) => x.status === "ok"), s.vellumPanels.join(" "));
  s.back = await changeSkin(page, "v2:engraved", "engraved");
  await sleep(500);
  check("8. back to engraved within 1 s, the window still open", s.back.skin === "engraved" && s.back.ms !== null && s.back.ms <= 1000 && s.back.windows?.includes("production"), JSON.stringify(s.back));
  const att1 = await attaches(page);
  check("8. the same connection", att0 !== null && att1 === att0, `${att0} -> ${att1}`);
  await page.keyboard.press("Escape");
  await waitUi(page, (q) => partTexts(q, "production.head").length === 0, 3000);
}

/** 10. R17: UI-only actions leave the replica's digest as it is (unless the server sent a batch in between). */
async function step10(alice, check, n) {
  const page = alice.page;
  const tries = [];
  for (let k = 0; k < 3; k++) {
    const seq0 = (await state(page))?.net?.viewSeq;
    const d0 = await page.evaluate(() => window.__civ5test.digest("replica")).catch((e) => `error ${e}`);
    const did = [];
    const w = await openByBanner(page);
    did.push(`production window ${w.ok}`);
    for (const row of rowsOf(await rectsOf(page, "production.list")).slice(0, 6)) {
      await page.mouse.move(row.cx, row.cy);
      await sleep(80);
    }
    did.push("hover rows");
    const sortHead = await rectsOf(page, "production.sort");
    const turns = sortHead.filter((r) => !r.node.includes("#") && r.keys === "").sort((a, b) => b.x - a.x);
    // The Turns head is the second button from the right (Buy is a plain label at the right end).
    const box = boxOf(sortHead);
    if (box) {
      const st0 = (await ui2(page))?.ui?.state?.["prod.sort"];
      for (const r of turns) {
        await page.mouse.click(r.x + r.w / 2, r.y + r.h / 2);
        await sleep(150);
        if ((await ui2(page))?.ui?.state?.["prod.sort"] !== st0) break;
      }
      did.push(`sort ${st0} -> ${(await ui2(page))?.ui?.state?.["prod.sort"]}`);
    }
    const tabs = (await rectsOf(page, "production.tabs")).filter((r) => !r.node.includes("#") && r.h >= 16).sort((a, b) => a.x - b.x);
    for (const t of tabs.slice(1, 3)) {
      await page.mouse.click(t.x + t.w / 2, t.y + t.h / 2);
      await sleep(150);
    }
    did.push(`tabs ${tabs.length}`);
    await page.keyboard.press("Escape");
    await sleep(200);
    if (await clickPart(page, "top.research")) {
      const r = await waitUi(page, (p) => partTexts(p, "research.list").length > 0, 3000);
      did.push(`research window ${r.ok}`);
      await page.keyboard.press("Escape");
      await sleep(200);
    }
    await page.mouse.move(VP.width / 2, 14);
    await sleep(300);
    const d1 = await page.evaluate(() => window.__civ5test.digest("replica")).catch((e) => `error ${e}`);
    const seq1 = (await state(page))?.net?.viewSeq;
    tries.push({ d0, d1, seq0, seq1, did });
    if (seq0 === seq1) break;
  }
  n.step10 = tries;
  const last = tries[tries.length - 1];
  check("10. R17: UI-only actions leave digest('replica') as it is", /^[0-9a-f]{16}$/.test(last.d0 ?? "") && last.seq0 === last.seq1 && last.d0 === last.d1, JSON.stringify(last));
}

/** 9. The mod's panel: in `hud.right` with engraved, in `mods` with vellum; Say hello comes back. */
async function step9(open, base, o, mods, check, n, shot) {
  const nick = `v1m${Date.now() % 100000}`;
  const b = await open(nick, {}, appUrl(base, o, { nick, autostart: "1", ai: "1", size: "duel", seed: "7", mode: "simultaneous", stats: "1", ui: "v2:engraved", mods }));
  const page = b.page;
  const s = await until(page, (x) => x.screen === "login" || x.screen === "play", 90000, "sign-in window or the game");
  if (s.screen === "login") await act(page, { kind: "login", nick, password: DEV_PASSWORD });
  await until(page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 180000, "the game with the mod");
  const up = await waitUi(page, (p) => p.state === "running" && partTexts(p, "echo").length > 0, UP_MS);
  const m = (n.step9 = { upMs: up.ms });
  check("9. the panel echo of hello is up", up.ok, JSON.stringify(partTexts(up.probe, "echo")));
  if (!up.ok) return;
  const say = async (skin) => {
    const before = partTexts(await ui2(page), "echo");
    const count = Number((before.find((t) => /^Echoes \d+$/.test(t)) ?? "Echoes 0").slice(7));
    const echo = await partBox(page, "echo");
    // The button is the panel's last line.
    if (echo) await page.mouse.click(echo.x + 30, echo.y + echo.h - 12);
    const after = await waitUi(page, (p) => partTexts(p, "echo").includes(`Echoes ${count + 1}`), 10000);
    return { skin, before, after: partTexts(after.probe, "echo"), ok: after.ok, ms: after.ms };
  };
  const where = async () => ({ echo: await partBox(page, "echo"), right: await partBox(page, "notes.extra"), mods: await partBox(page, "mods") });
  m.engraved = await where();
  check("9. engraved: the panel stands in hud.right (the notes' right slot)", inside(m.engraved.echo, m.engraved.right), JSON.stringify(m.engraved));
  m.sayEngraved = await say("engraved");
  check("9. Say hello: Echoes N+1 comes back (engraved)", m.sayEngraved.ok, JSON.stringify(m.sayEngraved));
  await shot(b, "mods-engraved");
  m.toVellum = await changeSkin(page, "v2:vellum", "vellum");
  await waitUi(page, (p) => partTexts(p, "echo").length > 0, 5000);
  await sleep(500);
  m.vellum = await where();
  check("9. vellum: the panel stands in the mods slot", m.toVellum.skin === "vellum" && inside(m.vellum.echo, m.vellum.mods) && !inside(m.vellum.echo, m.vellum.right), JSON.stringify(m.vellum));
  m.sayVellum = await say("vellum");
  check("9. Say hello: Echoes N+1 comes back (vellum)", m.sayVellum.ok, JSON.stringify(m.sayVellum));
  await shot(b, "mods-vellum");
}
