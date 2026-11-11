// `ui2-host`: UI v2 over a live game (the start). Needs the real-core server (WSL, `tools/playtest/m2/server.ps1`, with a web
// bundle that has `ui2/skins.json`, e.g. `cargo xtask build --only web`) and `--attach <url>`:
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8790 --scenario ui2-host [--gfx webgl2] [--headed]
// Without `--attach` (the local fixture server) the game's layer that UI v2 reads never comes up: the scenario says so.
//
// Also: the playtest hooks (`state().ui`, `ui2()`, `ui2Rects`, `digest('replica')`), switching v2 -> classic (warm)
// -> v2 -> classic (cold) -> v2 on the fly, and broken skin packages (fixtures) on the classic page.
//
// Start: `?ui=v2:engraved&test=1&autostart=1&stats=1`; once the skin runs, the top strip's texts (read through
// `__civ5test.ui2()`, never from pixels) show the gold, the science rate, the turn and the year that `state()` reports,
// at the start (then a click on the map selects a unit: the unit panel shows its name and its Fortify button fortifies it
// within 2 s) and again once the settler founded a city (through the test port's orders); `ui2.draws.hud` <= 5; no console errors. Then a page without `&ui=`: `ui2()` is null and no `ui2.*` pass ran
// (`ui2.draws` absent or 0). Screenshot of the strip: <logs>/ui2-top.png.
import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { openPage, act, until, state, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { FOUND_CITY } from "../lib/policy.mjs";

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

/**
 * The playtest hooks of UI v2: `state().ui` names the UI on the screen and the choice; `ui2()` has the package, the
 * model revision, the panels and the scripts; `ui2Rects` gives the strip's rectangles; `digest('replica')` is the same
 * before and after UI-only actions (hover, a click on the strip) unless the server sent a batch in between.
 */
async function hooks(page, numbers, check) {
  const st = await state(page);
  numbers.ui = st?.ui ?? null;
  check("state().ui: UI v2 on the screen, choice v2:engraved", st?.ui?.active === "v2" && st.ui.choice === "v2:engraved" && st.ui.skin === "engraved" && st.ui.switching === false,
    JSON.stringify(st?.ui));
  const p = await ui2(page);
  numbers.ui2Hooks = { active: p?.active, pkg: p?.pkg, model: p?.model, panels: p?.panels?.length, scripts: p?.scripts };
  check("ui2(): active, package hash, model revision, panels", p?.active === true && typeof p.pkg === "string" && p.pkg.length === 64 && p.model?.rev > 0 && Array.isArray(p.panels) && p.panels.length > 0 && p.panels.every((x) => x.status === "ok"),
    JSON.stringify(numbers.ui2Hooks));
  const rects = await page.evaluate(() => window.__civ5test.ui2Rects(["top.yield.gold"])).catch((e) => String(e));
  numbers.rects = Array.isArray(rects) ? rects.slice(0, 4) : rects;
  check("ui2Rects(top.yield.gold): rectangles in CSS px", Array.isArray(rects) && rects.length > 0 && rects.every((r) => r.part === "top.yield.gold" && r.w > 0 && r.h > 0),
    Array.isArray(rects) ? `${rects.length} rect(s), first ${JSON.stringify(rects[0])}` : String(rects));
  const seq0 = (await state(page))?.net?.viewSeq;
  const d0 = await page.evaluate(() => window.__civ5test.digest("replica")).catch((e) => `error ${e}`);
  const vp = page.viewportSize() ?? { width: 1600, height: 900 };
  for (const [x, y] of [[vp.width / 2, 14], [vp.width / 3, 14], [vp.width / 2, vp.height / 2]]) {
    await page.mouse.move(x, y);
    await sleep(100);
  }
  await page.mouse.click(vp.width / 2, 14);
  await sleep(300);
  const d1 = await page.evaluate(() => window.__civ5test.digest("replica")).catch((e) => `error ${e}`);
  const seq1 = (await state(page))?.net?.viewSeq;
  numbers.replicaDigest = { d0, d1, seq0, seq1 };
  check("R17: UI-only actions leave the replica digest as it is", /^[0-9a-f]{16}$/.test(d0 ?? "") && (d0 === d1 || seq0 !== seq1), JSON.stringify(numbers.replicaDigest));
}

/** `__civ5stats.ui.lua_errors` of the classic UI (null before its stats came). */
async function luaErrors(page) {
  return page.evaluate(() => window.__civ5stats?.ui?.lua_errors ?? null).catch(() => null);
}

/** The client's attaches so far (a reconnect would add one). */
async function attaches(page) {
  return page.evaluate(() => {
    const c = window.__civ5test.metrics().client;
    return c ? c.attachedReplay + c.attachedFresh : null;
  }).catch(() => null);
}

/**
 * One switch: `setUi` with `opts`, then until `want` is on the screen and (for the classic UI) its layer ran again
 * (`ui.classic.wake_ms` changed). Returns the host's switch time, the classic UI's wake-up time and the wall time.
 */
async function switchTo(page, value, want, opts = {}, waitMs = 30000) {
  const before = await counters(page);
  const t0 = Date.now();
  await act(page, { kind: "setUi", value, ...opts });
  const st = await until(page, (x) => x.ui?.active === want && !x.ui.switching, 30000, `${value} on the screen`);
  let wakeMs = null;
  if (want === "classic") {
    const deadline = Date.now() + waitMs;
    for (;;) {
      const c = await counters(page);
      if (typeof c["ui.classic.wake_ms"] === "number" && c["ui.classic.wake_ms"] !== before["ui.classic.wake_ms"]) {
        wakeMs = c["ui.classic.wake_ms"];
        break;
      }
      if (Date.now() > deadline) break;
      await sleep(100);
    }
  }
  return { value, switchMs: st.ui.lastSwitchMs, wakeMs, wallMs: Date.now() - t0, switches: st.ui.switches };
}

/**
 * Switching on the fly (R21, R22): v2 -> classic (warm: the classic layer stays and the game is announced to it
 * again) -> v2 -> classic (cold: a new layer) -> v2, each within 3 s (the warm way and UI v2's way back; the cold
 * way is measured), the classic UI without new Lua errors after its sleep, the same connection all along, and the
 * choice kept in `civ5.options.ui` only when saved (`?ui=` writes nothing).
 */
async function switching(page, numbers, check) {
  const st0 = await state(page);
  check("?ui= keeps nothing: civ5.options.ui is not written", st0?.ui?.saved === null, `${JSON.stringify(st0?.ui?.saved)}`);
  const att0 = await attaches(page);
  // The classic UI's layer may never have come up under UI v2 (it took the screen early): its first boot first.
  const boot = await switchTo(page, "classic", "classic", {}, 180000);
  numbers.classicBoot = boot;
  check("classic: its layer runs", boot.wakeMs !== null, JSON.stringify(boot));
  await sleep(2000);
  const lua0 = await luaErrors(page);
  await switchTo(page, "v2:engraved", "v2");
  const runs = [];
  runs.push(await switchTo(page, "classic", "classic"));
  await sleep(3000);
  const lua1 = await luaErrors(page);
  const c1 = await counters(page);
  await sleep(1500);
  const c2 = await counters(page);
  check("warm: the classic UI is back within 3 s", runs[0].switchMs <= 3000 && runs[0].wakeMs !== null && runs[0].switchMs + runs[0].wakeMs <= 3000, JSON.stringify(runs[0]));
  check("warm: no Lua errors after the sleep", lua1 !== null && lua1 === (lua0 ?? 0), `${lua0} -> ${lua1}`);
  check("warm: the classic UI runs, UI v2 sleeps", c2["ui.active"] === 1 && c2["ui.frames.v2"] === c1["ui.frames.v2"] && c2["ui.frames.classic"] > c1["ui.frames.classic"],
    `active ${c2["ui.active"]}, v2 ${c1["ui.frames.v2"]} -> ${c2["ui.frames.v2"]}, classic ${c1["ui.frames.classic"]} -> ${c2["ui.frames.classic"]}`);
  runs.push(await switchTo(page, "v2:engraved", "v2", { save: true }));
  check("v2 is back within 3 s", runs[1].switchMs <= 3000, JSON.stringify(runs[1]));
  const probe = await ui2(page);
  check("v2 is back with its texts", probe?.active === true && partTexts(probe, "top.yield.gold").length > 0, `${probe?.active}`);
  const st1 = await state(page);
  check("the setting keeps the choice: civ5.options.ui == v2:engraved", st1.ui.saved === "v2:engraved", `${JSON.stringify(st1.ui.saved)}`);
  runs.push(await switchTo(page, "classic", "classic", { cold: true }));
  await sleep(3000);
  const lua2 = await luaErrors(page);
  check("cold: the classic UI is back, no Lua errors", runs[2].wakeMs !== null && lua2 === lua1, `${JSON.stringify(runs[2])}, lua ${lua1} -> ${lua2}`);
  runs.push(await switchTo(page, "v2:engraved", "v2", { cold: false }));
  check("v2 is back within 3 s (after the cold classic)", runs[3].switchMs <= 3000, JSON.stringify(runs[3]));
  const att1 = await attaches(page);
  check("the same connection: no new attach while switching", att0 !== null && att1 === att0, `${att0} -> ${att1}`);
  check("one switch per request", runs[3].switches - runs[0].switches === 3, runs.map((r) => r.switches).join(","));
  numbers.switching = { runs, lua: [lua0, lua1, lua2], attaches: [att0, att1] };
}

/** The broken skin packages of `crates/client/ui2-host/tests/fixtures` (`badhash`, `api9`, `scriptfail`). */
const FIXTURES = fileURLToPath(new URL("../../../crates/client/ui2-host/tests/fixtures/", import.meta.url));

/**
 * Fatal skin failures (R4) on a page whose classic UI runs and whose UI v2 has not loaded yet: the page gets the fixture
 * index instead of the build's. `v2:badhash` (another hash than its file) falls back to the default `scriptfail` (its
 * script throws while it loads), which falls back to the classic UI with a notice; `v2:api9` (script API 9) is refused
 * before its download. The classic UI stays on the screen, each failure is counted once, nothing is tried in a loop.
 */
async function brokenPackages(page, numbers, check) {
  await page.route("**/ui2/skins.json", (r) => r.fulfill({ path: `${FIXTURES}skins.json`, contentType: "application/json" }));
  await page.route(/\/ui2\/[a-z0-9]+-[0-9a-f]{16}\.ui2$/, (r) => r.fulfill({ path: `${FIXTURES}${r.request().url().split("/").pop()}` }));
  const before = (await state(page))?.ui;
  await act(page, { kind: "setUi", value: "v2:badhash" });
  const t0 = Date.now();
  let st = await until(page, (x) => (x.ui?.failed?.length ?? 0) > (before?.failed?.length ?? 0) && !x.ui.switching, 30000, "the broken skins refused");
  const firstMs = Date.now() - t0;
  // No loop: a few seconds later nothing more failed and the classic UI still has the screen.
  await sleep(3000);
  st = await state(page);
  const failed = st.ui.failed.slice(before?.failed?.length ?? 0);
  numbers.broken = { firstMs, failed, notice: st.ui.notice, switches: [before?.switches, st.ui.switches], probe: await ui2(page) };
  check("broken package: back to the classic UI with a notice", st.ui.active === "classic" && typeof st.ui.notice === "string" && /failed/.test(st.ui.notice),
    JSON.stringify({ active: st.ui.active, notice: st.ui.notice }));
  check("broken package: badhash, then the default scriptfail, once each", failed.length === 1 && /scriptfail/.test(failed[0].why), JSON.stringify(failed));
  check("broken package: no loop", st.ui.switches - (before?.switches ?? 0) <= 1 && !st.ui.switching, JSON.stringify(numbers.broken.switches));
  await act(page, { kind: "setUi", value: "v2:api9" });
  st = await until(page, (x) => x.ui.failed.length > (before?.failed?.length ?? 0) + 1 && !x.ui.switching, 15000, "api9 refused");
  const last = st.ui.failed[st.ui.failed.length - 1];
  numbers.broken.api9 = last;
  check("api9: refused (script API 9.0), the classic UI stays", st.ui.active === "classic" && /9\.0/.test(last?.why ?? ""), JSON.stringify(last));
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
        const date = partTexts(probe, "top.date");
        shown = {
          gold: gold[0] ?? null,
          science: partTexts(probe, "top.yield.science")[0] ?? null,
          turn: date.find((t) => /\d/.test(t) && !/BC|AD/.test(t))?.replace(/\D+/g, "") ?? null,
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
    await hooks(b.page, result.numbers, check);
    await unitPanel(b.page, result.numbers, check, shot);
    // The settler founds a city through the test port's orders (the server's command, whichever UI is on the screen):
    // the classic UI sleeps under UI v2, so its Lua is not there to do it.
    const settler = ((await state(b.page))?.units ?? []).find((u) => /settler/i.test(u.name));
    if (settler) {
      await act(b.page, { kind: "selectUnit", id: settler.id });
      await until(b.page, (x) => x.selectedUnit === settler.id, 10000, "the settler selected");
      await act(b.page, { kind: "unitAction", action: FOUND_CITY });
    }
    result.numbers.found = settler ? `found ${settler.x},${settler.y}` : "no settler";
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
    // One game UI at a time: UI v2 is the active one and the sleeping classic UI gets no frames.
    await sleep(1500);
    const c2 = await counters(b.page);
    check("one active UI: ui.active == 2 (UI v2)", c2["ui.active"] === 2, `${c2["ui.active"]}`);
    check("the classic UI sleeps: ui.frames.classic does not grow", typeof c["ui.frames.classic"] === "number" && c2["ui.frames.classic"] === c["ui.frames.classic"] && c2["ui.frames.v2"] > c["ui.frames.v2"], `classic ${c["ui.frames.classic"]} -> ${c2["ui.frames.classic"]}, v2 ${c["ui.frames.v2"]} -> ${c2["ui.frames.v2"]}`);
    await switching(b.page, result.numbers, check);
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
      check("classic: ui.active == 1 (classic), no UI v2 frames", k["ui.active"] === 1 && (k["ui.frames.v2"] ?? 0) === 0, `active ${k["ui.active"]}, v2 frames ${k["ui.frames.v2"]}`);
    } catch (e) {
      error = e;
    }
    result.consoleErrors.push(...c.errors);
    const errs = appErrors(c.errors);
    check("no console errors (classic)", errs.length === 0, `${errs.length} error(s)${errs[0] ? `: ${errs[0]}` : ""}`);
    if (!error) {
      try {
        await brokenPackages(c.page, result.numbers, check);
      } catch (e) {
        error = e;
      }
    }
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
