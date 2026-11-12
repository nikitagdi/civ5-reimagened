// `ui2-actions`: orders of UI v2 reach the server and its answers reach the interface, all through the skin (`engraved`).
// Needs the real-core server (WSL, `tools/playtest/m2/server.ps1`) and `--attach <url>`, as `ui2-host` does:
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8790 --scenario ui2-actions [--headed]
//
// The game starts with `?ui=v2:engraved` (nothing is chosen for us: `manual=1`). The scenario only clicks the skin; it
// reads the interface through `__civ5test.ui2()` (texts by `data-part`) and the game through `state()`. The turn dial
// (`turn.primary`) leads it through the start of the game:
// 1. a unit waits for orders: the dial selects it (the unit panel shows a name); the settler founds a city with its
//    Found button, other units fortify or sleep;
// 2. the city needs production: the dial opens the production window; the Worker row + Build now make the Worker the
//    current production (window and `state()`); Add to queue puts a Warrior in the queue (the city queue order goes to
//    the server; its answer clears the row's waiting mark); the queue's up arrow moves it to the front; ArrowDown moves
//    the focus (the skin's script); a click on the Turns column head sorts the list by turns (`state.prod.sort`, the
//    skin's own state, becomes "turns" in `ui2().ui.state` and the rows are reordered);
// 3. research is needed: the dial opens the research window; a click on a technology makes it the current research
//    (`research.current` in the top strip);
// 4. the next turn: a click on the dial ends the turn; the turn number goes up by one in `state()` and on the dial.
// No console errors. Screenshots: <logs>/t118-helm.png (the helm at the next turn), <logs>/t122-production.png,
// <logs>/t122-research.png.
//
// Clicks go where the skin's layout puts things at 1600 x 900 (the same layout the converter checks against Chrome):
// the helm is the bottom right 400 x 392 plate, the dial a 216 px circle 8 px in from its top right corner; the
// production window stands 200 px from the left, 60 px from the top, 960 px wide, its orders at the bottom of the
// 312 px detail column; the research window is 520 px wide in the middle, its rows 36 px high. A row of a list is
// found by clicking down the list until the detail (or the strip) names it.
import { writeFileSync } from "node:fs";
import { openPage, act, until, state, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const UP_MS = Number(process.env.UI2_UP_MS ?? 420000);
const VP = { width: 1600, height: 900 };

// ---- geometry of the skin at 1600 x 900 (css/hud.css, css/production.css, css/research.css)
const DIAL = { x: VP.width - 8 - 108, y: VP.height - 392 + 8 + 108 };
const PWIN = { left: 200, top: 60, right: 1160, bottom: VP.height - 56 };
const DETAIL_X = PWIN.right - 156;
const QUEUE = { top: PWIN.top + 72 + 16 + 20 + 4 + 2, row: 26, up: PWIN.right - 20 - 20 - 10 - 10 };
const RWIN = { left: (VP.width - 520) / 2, top: 60, head: 64, pad: 8, row: 36 };

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

/** The helm's state line ("Unit needs orders", "Next turn", ...). */
function helmState(probe) {
  return partTexts(probe, "helm.state")[0] ?? null;
}

async function enterGame(b, base, o, extra, prefix) {
  for (let attempt = 0; ; attempt++) {
    const nick = `${prefix}${(Date.now() + attempt) % 1000000}`;
    await b.page.goto(appUrl(base, o, { nick, autostart: "1", ai: "2", size: "small", seed: "7", mode: "simultaneous", manual: "1", stats: "1", ...extra }));
    try {
      const s = await until(b.page, (x) => x.screen === "login" || x.screen === "play", 90000, "sign-in window or the game");
      if (s.screen === "login") await act(b.page, { kind: "login", nick, password: DEV_PASSWORD });
      let inRoom = 0;
      await until(b.page, (x) => {
        inRoom = x.screen === "room" ? inRoom + 1 : 0;
        if (inRoom === 200) b.page.mouse.click(470, 683);
        return x.screen === "play" && x.net?.phase === "play" && x.turn !== null;
      }, 90000, "the game");
      return;
    } catch (e) {
      if (attempt >= 2) throw e;
    }
  }
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

/** The centre of a unit order button: the layout `ui2-host` documents (rows of 488 px, 56 px buttons, 4 px gaps). */
function actButton(acts, which) {
  const boxes = [];
  let x = 0;
  let row = 0;
  for (const a of acts) {
    const lead = a === "unit.act.disband" ? 8 : 0;
    const w = a === "unit.act.disband" ? 60 : 56;
    let at = x === 0 ? lead : x + 4 + lead;
    if (x > 0 && at + w > 488) {
      row += 1;
      at = lead;
    }
    boxes.push({ a, row, x: at, w });
    x = at + w;
  }
  const box = boxes.find((b) => b.a === which);
  if (!box) return null;
  const rowTop = VP.height - 16 - 52 - (row - box.row) * 56;
  return [16 + box.x + box.w / 2, rowTop + 26];
}

/** Clicks down a list from `y0` to `y1` in steps until `named(probe)` says the row is the one; returns its y or null. */
async function findRow(page, x, y0, y1, step, named) {
  for (let y = y0; y <= y1; y += step) {
    await page.mouse.click(x, y);
    await sleep(200);
    if (named(await ui2(page))) return y;
  }
  return null;
}

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "ui2-actions", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  if (!o.attach) {
    check("options", false, "ui2-actions needs --attach <url of the real-core server> (tools/playtest/m2/server.ps1)");
    result.durationMs = Date.now() - started;
    return result;
  }
  const appErrors = (errs) => errs.filter((e) => !e.includes("/pkg-mt/"));
  const b = await openPage({ headed: o.headed, logDir: o.logDir, viewport: VP, consoleFile: "console-ui2-actions.log" });
  const page = b.page;
  const shot = async (name) => {
    for (let k = 0; k < 3; k++) {
      try { await page.screenshot({ path: `${o.logDir}/${name}.png`, timeout: 120000 }); return; } catch { /* retry */ }
    }
  };
  const n = result.numbers;
  n.steps = [];
  const done = { unit: false, production: false, research: false, turn: false };
  try {
    const t0 = Date.now();
    await enterGame(b, o.attach, o, { ui: "v2:engraved" }, "ui2a");
    for (;;) {
      const probe = await ui2(page);
      if (probe?.state === "classic") throw new Error("UI v2 fell back to the classic UI (see the console log)");
      if (probe?.state === "running" && probe.frames > 30 && helmState(probe)) break;
      if (Date.now() - t0 > UP_MS) throw new Error(`UI v2 is not running after ${UP_MS / 1000} s`);
      await sleep(250);
    }
    n.toRunningMs = Date.now() - t0;

    for (let guard = 0; guard < 24 && !done.turn; guard++) {
      const probe = await ui2(page);
      const st = helmState(probe);
      n.steps.push(st);
      if (st === "Unit needs orders") {
        // 1. The dial selects the unit that waits; the panel gives it an order.
        await page.mouse.click(DIAL.x, DIAL.y);
        const sel = await waitUi(page, (p) => partTexts(p, "unit.title").length > 0 && partsOf(p, "unit.act.").length > 0, 5000);
        const title = partTexts(sel.probe, "unit.title");
        const acts = partsOf(sel.probe, "unit.act.");
        const pick = ["unit.act.found-city", "unit.act.fortify", "unit.act.sleep", "unit.act.skip"].find((a) => acts.includes(a));
        const at = pick && actButton(acts, pick);
        if (!done.unit) {
          check("dial on 'Unit needs orders' selects a unit (the panel shows its name)", sel.ok, `title ${JSON.stringify(title)} in ${sel.ms} ms`);
          done.unit = true;
        }
        (n.orders ??= []).push({ title, pick, at });
        if (!at) throw new Error(`no order button for the unit: ${JSON.stringify(acts)}`);
        await page.mouse.click(at[0], at[1]);
        await waitUi(page, (p) => helmState(p) !== "Unit needs orders" || JSON.stringify(partTexts(p, "unit.title")) !== JSON.stringify(title), 4000);
        await sleep(500);
      } else if (st === "Choose production") {
        if (done.production) throw new Error("production is still needed after the Worker was chosen");
        await production(page, check, n, shot);
        done.production = true;
      } else if (st === "Choose research") {
        if (done.research) throw new Error("research is still needed after a technology was chosen");
        await research(b, check, n, shot);
        done.research = true;
      } else if (st === "Next turn") {
        // 4. The dial ends the turn: +1 in the game and on the dial.
        const before = (await state(page)).turn;
        await shot("t118-helm");
        await page.mouse.click(DIAL.x, DIAL.y);
        let after = before;
        const tEnd = Date.now();
        while (Date.now() - tEnd < 60000) {
          after = (await state(page)).turn;
          if (after === before + 1) break;
          await sleep(250);
        }
        const dial = await waitUi(page, (p) => partTexts(p, "helm.dial").includes(String(before + 1)), 10000);
        n.turn = { before, after, ms: Date.now() - tEnd, dial: partTexts(dial.probe, "helm.dial"), helm: partTexts(dial.probe, "helm.state") };
        check("dial click ends the turn: state().turn +1", after === before + 1, `turn ${before} -> ${after} in ${n.turn.ms} ms`);
        check("the dial shows the new turn", dial.ok, JSON.stringify(n.turn.dial));
        await sleep(1500);
        await shot("t118-helm-next");
        done.turn = true;
      } else if (st === "Waiting for players" || st === "Other civilizations move" || st === "Turn ended" || st === null) {
        await sleep(500);
      } else {
        throw new Error(`the helm asks for something this scenario does not do: ${st}`);
      }
    }
    for (const k of ["production", "research", "turn"]) check(`step reached: ${k}`, done[k], JSON.stringify(n.steps));
  } catch (e) {
    check("no scenario error", false, e.message);
    result.error = e.stack ?? String(e);
    await shot("failure-ui2-actions");
    n.lastProbe = await ui2(page);
  }
  result.consoleErrors.push(...b.errors);
  const errs = appErrors(b.errors);
  check("no console errors", errs.length === 0, `${errs.length} error(s)${errs[0] ? `: ${errs[0]}` : ""}`);
  try { writeFileSync(`${o.logDir}/ui2-actions.json`, JSON.stringify(n, null, 2)); } catch { /* no log dir */ }
  await b.close();
  result.pass = result.checks.every((x) => x.pass);
  result.durationMs = Date.now() - started;
  return result;
}

/** The name the detail column shows (the focused option). */
const detailName = (p) => partTexts(p, "production.detail")[0] ?? null;

/** 2. The production window: Build now, Add to queue, the queue's arrow, sorting and filtering. */
async function production(page, check, n, shot) {
  await page.mouse.click(DIAL.x, DIAL.y);
  const open = await waitUi(page, (p) => partTexts(p, "production.head").length > 0, 5000);
  check("dial on 'Choose production' opens the production window", open.ok, `${open.ms} ms, head ${JSON.stringify(partTexts(open.probe, "production.head"))}`);
  if (!open.ok) throw new Error("the production window did not open");
  const p = n.production = { head: partTexts(open.probe, "production.head") };
  const listX = PWIN.left + 48 + 60;
  const yFrom = PWIN.top + 72 + 52 + 4 + 40 + 32 + 14;
  // Build now: the Worker.
  const wy = await findRow(page, listX, yFrom, PWIN.bottom - 20, 14, (q) => detailName(q) === "Worker");
  p.workerRow = wy;
  if (wy === null) throw new Error(`no Worker row; the list: ${JSON.stringify(partTexts(await ui2(page), "production.list"))}`);
  const orders = async () => {
    const q = await ui2(page);
    const short = partTexts(q, "production.orders").some((t) => /short/.test(t));
    const bottom = PWIN.bottom - 20 - (short ? 20 : 0);
    return { buy: bottom - 20, back: bottom - 68, front: bottom - 116, now: bottom - 164, texts: partTexts(q, "production.orders") };
  };
  let at = await orders();
  p.orders = at;
  await page.mouse.click(DETAIL_X, at.now);
  const built = await waitUi(page, (q) => partTexts(q, "production.current").includes("Worker"), 5000);
  const cities = (await state(page)).cities ?? [];
  p.current = partTexts(built.probe, "production.current");
  p.stateCities = cities;
  check("Build now: the window shows Worker as current production", built.ok, `${built.ms} ms, ${JSON.stringify(p.current)}`);
  // `state()` names the item by its id: the city builds something now, in as many turns as the window says.
  check("Build now: the game builds it (state().cities: an item, the window's turns)",
    built.ok && cities.some((c) => c.productionItem >= 0 && p.current.includes(String(c.productionTurns))),
    JSON.stringify(cities.map((c) => [c.productionItem, c.productionTurns])));
  // Add to queue: the Warrior.
  const ry = await findRow(page, listX, yFrom, PWIN.bottom - 20, 14, (q) => detailName(q) === "Warrior");
  p.warriorRow = ry;
  if (ry !== null) {
    at = await orders();
    await page.mouse.click(DETAIL_X, at.back);
    const queued = await waitUi(page, (q) => partTexts(q, "production.queue").includes("Warrior") && !partTexts(q, "production.list").includes("Waiting"), 5000);
    p.queue = partTexts(queued.probe, "production.queue");
    check("Add to queue: the Warrior is in the queue, no waiting mark left", queued.ok, `${queued.ms} ms, ${JSON.stringify(p.queue)}`);
    // The queue's up arrow: the Warrior (second row) goes to the front.
    const rowY = QUEUE.top + QUEUE.row + 12;
    await page.mouse.click(QUEUE.up, rowY);
    const moved = await waitUi(page, (q) => {
      const t = partTexts(q, "production.queue");
      return t.indexOf("Warrior") >= 0 && (t.indexOf("Worker") < 0 || t.indexOf("Warrior") < t.indexOf("Worker"));
    }, 5000);
    p.queueAfterUp = partTexts(moved.probe, "production.queue");
    check("queue arrow: the Warrior moves to the front", moved.ok, `${moved.ms} ms, ${JSON.stringify(p.queueAfterUp)}`);
  } else {
    check("Add to queue: a Warrior row", false, "no Warrior row in the list");
  }
  // The arrow keys move the focus (the skin's script, `call prod.step`).
  const before = detailName(await ui2(page));
  await page.keyboard.press("ArrowDown");
  const stepped = await waitUi(page, (q) => detailName(q) !== before, 3000);
  p.arrow = { before, after: detailName(stepped.probe), ms: stepped.ms };
  check("ArrowDown moves the focus to the next row (skin script)", stepped.ok, JSON.stringify(p.arrow));
  // Sorting by turns (the Turns column head writes `state.prod.sort`, the skin's own state the host mounts). With two
  // rows in the queue the column heads stand 300 px from the top.
  const probe0 = await ui2(page);
  const listBefore = partTexts(probe0, "production.list");
  const sortBefore = probe0?.ui?.state?.["prod.sort"];
  await page.mouse.click(720, 300);
  const sorted = await waitUi(page, (q) => q?.ui?.state?.["prod.sort"] === "turns" && JSON.stringify(partTexts(q, "production.list")) !== JSON.stringify(listBefore), 3000);
  p.sortByTurns = { changed: sorted.ok, ms: sorted.ms, state: [sortBefore, sorted.probe?.ui?.state?.["prod.sort"]], sortHead: partTexts(sorted.probe, "production.sort") };
  check("sorting by turns: state.prod.sort becomes 'turns' and the list is reordered", sortBefore === "order" && sorted.ok, JSON.stringify(p.sortByTurns));
  await shot("t122-production");
  // Close the window (Esc).
  await page.keyboard.press("Escape");
  await waitUi(page, (q) => partTexts(q, "production.head").length === 0, 3000);
}

/** 3. The research window: a click on the first (fastest) technology makes it the current research. */
async function research(b, check, n, shot) {
  const page = b.page;
  await page.mouse.click(DIAL.x, DIAL.y);
  const open = await waitUi(page, (p) => partTexts(p, "research.list").length > 0, 5000);
  const r = n.research = { list: partTexts(open.probe, "research.list") };
  check("dial on 'Choose research' opens the research window", open.ok, `${open.ms} ms`);
  // Without the window the turn cannot go on, and nothing else can choose: the classic UI (and its Lua) sleeps under
  // UI v2, and the test port has no research order.
  if (!open.ok) throw new Error("the research window did not open");
  await shot("t122-research");
  const first = r.list[0];
  // A line "Now <research>" stands under the header while something is researched.
  const now = partTexts(open.probe, "research.window").includes("Now") ? 36 : 0;
  await page.mouse.click(RWIN.left + 120, RWIN.top + RWIN.head + now + RWIN.pad + RWIN.row / 2);
  const chosen = await waitUi(page, (p) => partTexts(p, "research.list").length === 0 && partTexts(p, "top.research").includes(first), 5000);
  r.first = first;
  r.strip = partTexts(chosen.probe, "top.research");
  check("research: a click on a technology makes it the current research", chosen.ok, `${chosen.ms} ms, chose ${JSON.stringify(first)}, strip ${JSON.stringify(r.strip)}`);
}
