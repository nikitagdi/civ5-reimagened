// `ui2-host`: UI v2 over a live game (the start). Needs the real-core server (WSL, `tools/playtest/m2/server.ps1`, with a web
// bundle that has `ui2/skins.json`, e.g. `cargo xtask build --only web`) and `--attach <url>`:
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8790 --scenario ui2-host [--gfx webgl2] [--headed]
// Without `--attach` (the local fixture server) the game's layer that UI v2 reads never comes up: the scenario says so.
//
// Start: `?ui=v2:engraved&test=1&autostart=1&stats=1`; once the skin runs, the top strip's texts (read through
// `__civ5test.ui2()`, never from pixels) show the gold and the science rate, and the turn dial the turn and the year, that `state()` reports,
// at the start (then a click on the map selects a unit: the unit panel shows its name and its Fortify button fortifies it
// within 2 s) and again once the settler founded a city (a research is chosen too); `ui2.draws.hud` <= 5; no console errors. Then a page without `&ui=`: `ui2()` is null and no `ui2.*` pass ran
// (`ui2.draws` absent or 0). Screenshot of the strip: <logs>/ui2-top.png.
import { writeFileSync } from "node:fs";
import { openPage, act, until, state, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { uiDriver } from "../lib/ui-driver.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
/** How long the game's layer may take to come up (a cold boot on a software renderer is slow). */
const UP_MS = Number(process.env.UI2_UP_MS ?? 420000);

/** `ui2()` (null without `&ui=v2` or before the hooks exist). */
async function ui2(page) {
  return page.evaluate(() => (window.__civ5test?.ui2 ? window.__civ5test.ui2() : null)).catch(() => null);
}

/** `__civ5stats.counters` (an object of name -> number), or {}. */
async function counters(page) {
  return page.evaluate(() => window.__civ5stats?.counters ?? {}).catch(() => ({}));
}

async function m2Phase(page) {
  return page.evaluate(() => window.__civ5stats?.m2?.phase ?? null).catch(() => null);
}

/** The texts of a part (the leaves whose nearest `data-part` is `part`), in document order. */
function partTexts(probe, part) {
  return (probe?.texts ?? []).filter((t) => t.part === part).map((t) => t.text);
}

/** The year as the skin writes it (`year.bc` / `year.ad` of `l10n/en_US.toml`). */
function yearText(year) {
  return year < 0 ? `${-year} BC` : `${year} AD`;
}

/** `fmt.signed`: a sign before positive numbers. */
function signed(n) {
  return n > 0 ? `+${n}` : `${n}`;
}

/** Signs in to the server and waits for the game to be played (autostart). */
async function enterGame(b, base, o, extra, prefix) {
  for (let attempt = 0; ; attempt++) {
    const nick = `${prefix}${(Date.now() + attempt) % 1000000}`;
    await b.page.goto(appUrl(base, o, { nick, autostart: "1", ai: "2", size: "small", seed: "7", mode: "simultaneous", manual: "1", stats: "1", ...extra }));
    try {
      const s = await until(b.page, (x) => x.screen === "login" || x.screen === "play", 90000, "sign-in window or the game");
      if (s.screen === "login") await act(b.page, { kind: "login", nick, password: DEV_PASSWORD });
      let inRoom = 0;
      await until(b.page, (x) => {
        // The quick start stays in the room now and then: the room's button starts the game.
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

/** The distinct parts of the texts, in document order. */
function partsOf(probe, prefix) {
  const out = [];
  for (const t of probe?.texts ?? []) if (t.part?.startsWith(prefix) && !out.includes(t.part)) out.push(t.part);
  return out;
}

/**
 * The unit panel: a click on the map selects a unit of ours (the clicks walk out from the middle of the screen, where the
 * camera starts, until the panel shows a unit that can fortify); the panel shows its name; its Fortify button fortifies it
 * within 2 s (`state().units[].activity` changes). The button is clicked where the skin's layout puts it: the panel stands
 * at the bottom left, the buttons are 56 px wide with 4 px gaps, 16 px in from the panel's edge.
 */
async function unitPanel(page, numbers, check, shot) {
  const vp = page.viewportSize() ?? { width: 1600, height: 900 };
  const cx = vp.width / 2;
  const cy = vp.height / 2;
  const spots = [];
  for (let r = 0; r <= 6; r++) {
    for (let dy = -r; dy <= r; dy++) {
      for (let dx = -r; dx <= r; dx++) {
        if (Math.max(Math.abs(dx), Math.abs(dy)) === r) spots.push([cx + dx * 36, cy + dy * 30]);
      }
    }
  }
  let probe = null;
  let at = null;
  for (const [x, y] of spots) {
    await page.mouse.click(x, y);
    await sleep(250);
    probe = await ui2(page);
    if (partsOf(probe, "unit.act.").includes("unit.act.fortify")) {
      at = [x, y];
      break;
    }
  }
  const name = partTexts(probe, "unit.title");
  numbers.unitPanel = { clickedAt: at, title: name, acts: partsOf(probe, "unit.act.") };
  const st = await state(page);
  const mine = (st?.units ?? []).map((u) => u.name);
  check("unit selected: the panel shows its name", at && name.length > 0 && name.some((t) => mine.includes(t)),
    `title ${JSON.stringify(name)}, units ${JSON.stringify(mine)}`);
  if (!at) return;
  await shot("ui2-unit");
  const unit = (st.units ?? []).find((u) => name.includes(u.name));
  const before = unit?.activity;
  // The buttons wrap in rows 488 px wide (56 px, 4 px gaps; Delete is 60 px with 8 px more on its left); the last row
  // ends 16 px above the bottom of the screen, rows are 52 px high with 4 px between them.
  const acts = partsOf(probe, "unit.act.");
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
  const box = boxes.find((b) => b.a === "unit.act.fortify");
  const rowTop = vp.height - 16 - 52 - (row - box.row) * 56;
  const bx = 16 + box.x + box.w / 2;
  const t0 = Date.now();
  await page.mouse.click(bx, rowTop + 26);
  let after = before;
  let title = name;
  while (Date.now() - t0 < 2000) {
    const s = await state(page);
    after = (s?.units ?? []).find((u) => u.id === unit?.id)?.activity;
    if (after !== undefined && after !== before) break;
    await sleep(100);
  }
  const ms = Date.now() - t0;
  await sleep(500);
  title = partTexts(await ui2(page), "unit.title");
  numbers.unitPanel.fortify = { button: [bx, rowTop + 26], before, after, ms, title };
  check("Fortify: the unit is fortified within 2 s", after !== undefined && after !== before && ms <= 2000, `activity ${before} -> ${after} in ${ms} ms, title ${JSON.stringify(title)}`);
}

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "ui2-host", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const base = o.attach ?? ctx.server?.url;
  if (!o.attach) {
    check("options", false, "ui2-host needs --attach <url of the real-core server> (tools/playtest/m2/server.ps1): the fixture server never brings up the game's layer UI v2 reads");
    result.durationMs = Date.now() - started;
    return result;
  }
  // The 404 of `pkg-mt` of a single-threaded bundle is not an error of the UI.
  const appErrors = (errs) => errs.filter((e) => !e.includes("/pkg-mt/"));
  let error = null;

  // ---- UI v2: the strip shows what the game state says.
  const b = await openPage({ headed: o.headed, logDir: o.logDir, viewport: { width: 1600, height: 900 }, consoleFile: "console-v2.log" });
  const shot = async (name) => {
    for (let k = 0; k < 3; k++) {
      try { await b.page.screenshot({ path: `${o.logDir}/${name}.png`, timeout: 120000 }); return; } catch { /* retry */ }
    }
  };
  try {
    const t0 = Date.now();
    await enterGame(b, base, o, { ui: "v2:engraved" }, "ui2v");
    let probe = null;
    for (;;) {
      probe = await ui2(b.page);
      if (probe?.state === "classic") throw new Error("UI v2 fell back to the classic UI (see console-v2.log)");
      if (probe?.state === "running" && probe.frames > 30 && partTexts(probe, "top.yield.gold").length) break;
      if ((await m2Phase(b.page)) === -1) throw new Error("the game's layer failed (m2 phase -1)");
      if (Date.now() - t0 > UP_MS) throw new Error(`UI v2 is not running after ${UP_MS / 1000} s: ${JSON.stringify({ state: probe?.state, frames: probe?.frames })}`);
      await sleep(250);
    }
    result.numbers.toRunningMs = Date.now() - t0;
    // The values settle within a few frames of the model; compare until they agree (10 s at most).
    const compare = async (label) => {
      let shown = null;
      let want = null;
      const deadline = Date.now() + 10000;
      for (;;) {
        const st = await state(b.page);
        probe = await ui2(b.page);
        want = st && st.top ? {
          gold: String(st.top.gold),
          science: signed(st.top.scienceRate),
          turn: String(st.turn),
          year: yearText(st.year),
        } : null;
        const gold = partTexts(probe, "top.yield.gold");
        // The turn and the year are on the turn dial of the helm.
        const date = partTexts(probe, "helm.dial");
        shown = {
          gold: gold[0] ?? null,
          science: partTexts(probe, "top.yield.science")[0] ?? null,
          turn: date.find((t) => /^\d+$/.test(t)) ?? null,
          year: date.find((t) => /BC|AD/.test(t)) ?? null,
        };
        const same = want && ["gold", "science", "turn", "year"].every((k) => shown[k] === want[k]);
        if (same || Date.now() > deadline) break;
        await sleep(250);
      }
      result.numbers[label] = { shown, state: want, texts: probe.texts };
      for (const k of ["gold", "science", "turn", "year"]) {
        check(`${label}: strip ${k} == state()`, want && shown[k] === want[k], `shown ${JSON.stringify(shown[k])}, state ${JSON.stringify(want?.[k])}`);
      }
    };
    await compare("start");
    await unitPanel(b.page, result.numbers, check, shot);
    // The settler founds a city through the original UI's Lua (which gets no input under UI v2). Ending the turn this way is
    // not possible yet: the server refuses END TURN while a unit waits for orders, and the Lua unit missions do not reach it.
    const ui = uiDriver(b);
    const found = await ui.lua(`local p = Players[Game.GetActivePlayer()]
local id = nil
for i, a in pairs(GameInfoActions) do if a.Type == "MISSION_FOUND" then id = i end end
for u in p:Units() do if id and u:IsFound() then UI.SelectUnit(u) Game.HandleAction(id) print(TAG .. "found " .. u:GetX() .. "," .. u:GetY()) break end end`).catch((e) => [`lua failed: ${e.message}`]);
    result.numbers.found = found;
    // A research, so the strip's research group has something to show (the page has `manual`: nothing is chosen for us).
    result.numbers.research = await ui.lua(`Network.SendResearch(GameInfoTypes.TECH_POTTERY, 0, -1, false) print(TAG .. "research")`).catch((e) => [`lua failed: ${e.message}`]);
    await sleep(3000);
    // The strip follows the game: with a city the rates are no longer 0.
    await until(b.page, (x) => (x.cities?.length ?? 0) > 0 && x.top && x.top.scienceRate !== 0, 30000, "the new city in state()");
    await compare("city");    const cities = (await state(b.page)).cities?.length ?? 0;
    result.numbers.cities = cities;
    // The renderer's counters come once a second (`?stats=1`).
    await sleep(2500);
    const c = await counters(b.page);
    const pick = (re) => Object.fromEntries(Object.entries(c).filter(([k]) => re.test(k)));
    result.numbers.counters = pick(/^ui2\./);
    result.numbers.loadMs = probe.loadMs;
    const hud = c["ui2.draws.hud"];
    check("ui2.draws.hud <= 5", typeof hud === "number" && hud > 0 && hud <= 5, `${hud}`);
    await shot("ui2-top");
  } catch (e) {
    error = e;
    await shot("failure-v2");
    result.numbers.lastProbe = await ui2(b.page);
    const lastState = await state(b.page).catch(() => null);
    if (lastState) result.numbers.lastScreen = `${lastState.screen}/${lastState.net?.phase}`;
  }
  result.consoleErrors.push(...b.errors);
  const v2Errors = appErrors(b.errors);
  check("no console errors (UI v2)", v2Errors.length === 0, `${v2Errors.length} error(s)${v2Errors[0] ? `: ${v2Errors[0]}` : ""}`);
  try { writeFileSync(`${o.logDir}/ui2-probe.json`, JSON.stringify(result.numbers, null, 2)); } catch { /* no log dir */ }
  await b.close();

  // ---- The classic UI: no UI v2 pass at all.
  if (!error) {
    const c = await openPage({ headed: o.headed, logDir: o.logDir, viewport: { width: 1600, height: 900 }, consoleFile: "console-classic.log" });
    try {
      const t0 = Date.now();
      await enterGame(c, base, o, {}, "ui2c");
      for (;;) {
        const phase = await m2Phase(c.page);
        if (phase === 2) break;
        if (phase === -1) throw new Error("the game's layer failed (m2 phase -1)");
        if (Date.now() - t0 > UP_MS) throw new Error(`the original UI is not up after ${UP_MS / 1000} s`);
        await sleep(250);
      }
      await sleep(3000);
      const probe = await ui2(c.page);
      const k = await counters(c.page);
      const draws = k["ui2.draws"] ?? 0;
      result.numbers.classic = { ui2: probe, draws, ui2Counters: Object.keys(k).filter((n) => n.startsWith("ui2.")) };
      check("classic: ui2() is null", probe === null, JSON.stringify(probe));
      check("classic: ui2.draws == 0", draws === 0, `${draws}`);
    } catch (e) {
      error = e;
    }
    result.consoleErrors.push(...c.errors);
    const errs = appErrors(c.errors);
    check("no console errors (classic)", errs.length === 0, `${errs.length} error(s)${errs[0] ? `: ${errs[0]}` : ""}`);
    await c.close();
  }

  if (error) {
    check("no scenario error", false, error.message);
    result.error = error.stack ?? String(error);
  }
  result.pass = result.checks.every((x) => x.pass);
  result.durationMs = Date.now() - started;
  return result;
}
