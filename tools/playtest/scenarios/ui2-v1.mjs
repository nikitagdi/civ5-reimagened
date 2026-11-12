// `ui2-v1`: acceptance of the engraved skin's screens in a live game, through the skin only. Two clients (alice hosts,
// bob joins) play one simultaneous game on the real-core server, both with `?ui=v2:engraved`:
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8864 --scenario ui2-v1 [--headed]
//
// Steps (each one reported as checks; later steps of the acceptance join this file):
// - chat: alice clicks the chat field, types a line and presses Enter; the line shows in bob's chat log (and in
//   alice's own). The field writes the skin state `state.chat.draft`, which the send reads.
// - map (bob runs with `&ui2onmap=0`): alice's unit flags are on the screen (`ui2Rects(['unit-flag'])`), bob's are
//   not, though his map layer has units: the switch turns the layer off for the bench.
// - banner (step 4, the window): the turn dial selects alice's waiting units and her settler founds a city with its
//   Found button; a click on her city's banner (`ui2Rects(['city-banner'])`, placed where the screen shows it)
//   opens the production window of that city.
// No console errors. Screenshots: <logs>/ui2-v1-chat-alice.png, ui2-v1-chat-bob.png, ui2-v1-banner.png,
// ui2-v1-production.png.
import { writeFileSync } from "node:fs";
import { openPage, act, until, state, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const UP_MS = Number(process.env.UI2_UP_MS ?? 420000);
const VP = { width: 1600, height: 900 };

async function ui2(page) {
  return page.evaluate(() => (window.__civ5test?.ui2 ? window.__civ5test.ui2() : null)).catch(() => null);
}

function partTexts(probe, part) {
  return (probe?.texts ?? []).filter((t) => t.part === part).map((t) => t.text);
}

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

/**
 * The box of the first element of a `data-part` (the union of its subtree's rectangles, CSS px), or null when the
 * skin does not show it.
 */
async function partBox(page, part) {
  const rects = await page.evaluate((p) => window.__civ5test.ui2Rects([p]), part).catch(() => null);
  if (!Array.isArray(rects) || rects.length === 0) return null;
  const x0 = Math.min(...rects.map((r) => r.x));
  const y0 = Math.min(...rects.map((r) => r.y));
  const x1 = Math.max(...rects.map((r) => r.x + r.w));
  const y1 = Math.max(...rects.map((r) => r.y + r.h));
  return { x: x0, y: y0, w: x1 - x0, h: y1 - y0, cx: (x0 + x1) / 2, cy: (y0 + y1) / 2, n: rects.length };
}

async function clickPart(page, part) {
  const b = await partBox(page, part);
  if (!b) return null;
  await page.mouse.click(b.cx, b.cy);
  return b;
}

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
  const n = result.numbers;
  const pages = [];
  const open = async (nick, extra) => {
    const b = await openPage({ headed: o.headed, logDir: o.logDir, viewport: VP, consoleFile: `console-ui2-v1-${nick}.log` });
    b.nick = nick;
    pages.push(b);
    await b.page.goto(appUrl(base, o, { nick, ui: "v2:engraved", stats: "1", manual: "1", ...extra }), { timeout: 180000 });
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
      if (probe?.state === "running" && probe.frames > 30 && partTexts(probe, "helm.state").length > 0) return Date.now() - t0;
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
    await act(alice.page, { kind: "start" });
    for (const b of [alice, bob]) await until(b.page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 180000, `${b.nick} in the game`);
    n.toRunningMs = await Promise.all([v2Up(alice), v2Up(bob)]).then(() => Date.now() - t0);
    await sleep(2000);

    await chat(alice, bob, check, n, shot);
    await mapOff(alice, bob, check, n);
    await banner(alice, check, n, shot);
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
    check(`${b.nick}: no console errors`, errs.length === 0, `${errs.length} error(s)${errs[0] ? `: ${errs[0]}` : ""}`);
  }
  try { writeFileSync(`${o.logDir}/ui2-v1.json`, JSON.stringify(n, null, 2)); } catch { /* no log dir */ }
  for (const b of pages) await b.close();
  result.pass = result.checks.every((x) => x.pass);
  result.durationMs = Date.now() - started;
  return result;
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
  // The field is empty again after the send (the draft was cleared).
  await shot(alice, "chat-alice");
  await shot(bob, "chat-bob");
}

/** `?ui2onmap=0`: bob's map layer is off, alice's flags are on the screen. */
async function mapOff(alice, bob, check, n) {
  const a = await waitUi(alice.page, () => true, 0);
  const flagsA = await partBox(alice.page, "unit-flag");
  const flagsB = await partBox(bob.page, "unit-flag");
  const probeB = await ui2(bob.page);
  n.map = { alice: flagsA, bob: flagsB, bobAnchors: probeB?.anchors ?? null, aliceAnchors: a.probe?.anchors ?? null };
  check("map layer: alice's unit flags are on the screen", !!flagsA, JSON.stringify(flagsA));
  check("map layer: bob's flags are off with ?ui2onmap=0", flagsB === null, JSON.stringify(flagsB));
}

/** Step 4 (the window): alice founds a city through the skin; a click on its banner opens its production window. */
async function banner(alice, check, n, shot) {
  const page = alice.page;
  const b = (n.banner = { orders: [] });
  for (let guard = 0; guard < 16; guard++) {
    const st = await state(page);
    if ((st?.cities ?? []).length > 0) break;
    const probe = await ui2(page);
    const helm = partTexts(probe, "helm.state")[0] ?? null;
    if (helm !== "Unit needs orders") {
      b.orders.push(helm);
      await sleep(500);
      continue;
    }
    // The dial selects the waiting unit; its panel gives the order.
    await clickPart(page, "helm.dial");
    await waitUi(page, (p) => partTexts(p, "unit.title").length > 0, 5000);
    let order = null;
    for (const part of ["unit.act.found-city", "unit.act.fortify", "unit.act.sleep", "unit.act.skip"]) {
      if (await clickPart(page, part)) {
        order = part;
        break;
      }
    }
    b.orders.push(order);
    if (!order) throw new Error("no order button for the selected unit");
    await sleep(order === "unit.act.found-city" ? 2500 : 800);
  }
  const cities = (await state(page))?.cities ?? [];
  b.cities = cities.length;
  check("banner: alice founded a city through the unit panel", cities.length > 0, JSON.stringify(b.orders));
  if (cities.length === 0) return;
  // The banner comes up once the map layer has the city and the host placed it.
  let box = null;
  const t0 = Date.now();
  while (!box && Date.now() - t0 < 15000) {
    box = await partBox(page, "city-banner");
    if (!box) await sleep(300);
  }
  b.box = box;
  b.names = partTexts(await ui2(page), "city-banner");
  check("banner: the city's banner is on the screen", !!box, `${JSON.stringify(box)} ${JSON.stringify(b.names)}`);
  if (!box) return;
  await shot(alice, "banner");
  await page.mouse.click(box.cx, box.cy);
  const open = await waitUi(page, (p) => partTexts(p, "production.head").length > 0, 5000);
  b.window = { ms: open.ms, head: partTexts(open.probe, "production.head") };
  check("banner: a click on alice's own banner opens the production window", open.ok, JSON.stringify(b.window));
  await sleep(500);
  await shot(alice, "production");
  await page.keyboard.press("Escape");
  await waitUi(page, (p) => partTexts(p, "production.head").length === 0, 3000);
}
