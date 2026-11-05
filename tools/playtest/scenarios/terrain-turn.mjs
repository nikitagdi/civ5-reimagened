// `terrain-turn` (build B3, module terrain-cache; AC-T1, AC-T2, AC-T4): what the terrain cache does
// while the camera pans and while turns end. The page is opened with `stats=1`, so the client
// publishes `window.__civ5stats.counters` (name -> number, once per second) with the terrain counters.
//  - pan: the arrow keys take the camera right, down, left and up (back over seen ground); every 50 ms
//    the test port says how many terrain cells are in view and how many have no page (`unpaged`), every
//    second the counters are read. AC-T2: no hole (unpaged cell, holes_total) while panning after load.
//  - turns: the camera stands; N turns are ended (`endTurn`, force); per turn the deltas of the rebake
//    counters (total, field, decal, settings), the decal-dirty counters and baked_total are kept.
//    AC-T1: an end turn without a change rebakes nothing (or every rebake has a field/decal cause) and
//    leaves no hole.
//  - AC-T4: page memory (terrain.page_bytes) at the end against CIV5_TERRAIN_BASE_BYTES (x 1.1), if set.
// Run through `cargo xtask playtest --scenario terrain-turn --headed`; `--scroll-s N` sets the seconds
// of the right and left pan (default 10), `--turns N` the number of turns (default 3; CIV5_PLAYTEST_TURNS
// also sets it; the runner's own default of 50 counts as "not given").
import { openPage, act, until, state, appUrl, INTERIM } from "../lib/browser.mjs";
import { writeFileSync } from "node:fs";
import { join } from "node:path";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const readCounters = (page) => page.evaluate(() => window.__civ5stats?.counters ?? null);
const d = (a, b, k) => (b?.[k] ?? 0) - (a?.[k] ?? 0);
const SIZES = [64, 128, 256, 512];
const DECAL = ["farms", "roads", "trees", "landmarks", "cities", "build", "other"];
const REBAKE = ["field", "decal", "settings"];

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "terrain-turn", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const b = await openPage({ headed: o.headed, logDir: o.logDir });
  const { page } = b;
  let error = null;
  try {
    await page.goto(appUrl(ctx.server.url, o, { ...INTERIM, nick: "tturn", autostart: "1", ai: "1", stats: "1", size: process.env.CIV5_PLAYTEST_SIZE ?? "standard", seed: String(o.seed) }));
    let s = await until(page, (x) => x.screen === "login" || x.screen === "play", o.lobbyTimeoutMs, "sign-in window or the game");
    if (s.screen === "login") await act(page, { kind: "login", nick: "tturn", password: "dev" });
    s = await until(page, (x) => x.screen === "play" && x.status === "human" && x.terrainCells, o.mapTimeoutMs, "the map with terrain");
    // The first view with all its pages, then 3 s without a hole; both timed and bounded (the cache before B3
    // took minutes on a revealed map: the measure goes on, the time is a number of the report).
    const tLoad = Date.now();
    let firstFull = null;
    let calmAt = null;
    for (let calm = 0; calm < 3000 && Date.now() - tLoad < 240000; ) {
      await sleep(250);
      const c = (await state(page))?.terrainCells;
      const full = c && c.visible > 0 && c.unpaged === 0;
      if (full && firstFull === null) firstFull = Date.now() - tLoad;
      calm = full ? calm + 250 : 0;
      if (calm >= 3000) calmAt = Date.now() - tLoad;
    }
    result.numbers.load = { firstFullViewMs: firstFull, calmMs: calmAt };
    console.log(`  load: first full view ${firstFull} ms, calm ${calmAt} ms`);
    // The counters are published once per second: wait for the first set.
    let C0 = null;
    for (let i = 0; i < 40 && !C0; i++) {
      C0 = await readCounters(page);
      if (!C0) await sleep(250);
    }
    await sleep(1100);
    C0 = (await readCounters(page)) ?? C0;

    // Focus the canvas (a click on the ground selects nothing).
    await page.mouse.move(640, 360);
    await page.mouse.click(640, 360);
    await page.evaluate(() => {
      const c = document.querySelector("canvas");
      if (c) {
        if (c.tabIndex < 0) c.tabIndex = 0;
        c.focus();
      }
    });

    // ---- pan
    const panSamples = [];
    const counterSeries = [];
    let nextCounters = 0;
    const sample = async (into) => {
      const x = await state(page);
      const c = x?.terrainCells;
      if (c) into.push({ visible: c.visible, unpaged: c.unpaged });
      if (Date.now() >= nextCounters) {
        nextCounters = Date.now() + 1000;
        const k = await readCounters(page);
        if (k) counterSeries.push({ at: Date.now(), holes: k["terrain.holes"], unbuilt: k["terrain.unbuilt"], jobs: k["terrain.jobs"], bytes: k["terrain.page_bytes"] });
      }
    };
    const pan = async (key, ms) => {
      await page.keyboard.down(key);
      const end = Date.now() + ms;
      while (Date.now() < end) {
        await sample(panSamples);
        await sleep(50);
      }
      await page.keyboard.up(key);
    };
    const seconds = o.scrollS ?? 10;
    await pan("ArrowRight", seconds * 1000);
    await pan("ArrowDown", 3000);
    await pan("ArrowLeft", seconds * 1000);
    await pan("ArrowUp", 3000);
    await sleep(1100);
    const C1 = (await readCounters(page)) ?? C0;
    const unpagedMax = Math.max(0, ...panSamples.map((x) => x.unpaged));
    const unpagedFrames = panSamples.filter((x) => x.unpaged > 0).length;
    const evict = {};
    for (const sz of SIZES) evict[sz] = d(C0, C1, `terrain.evict.${sz}_total`);
    const panNums = {
      samples: panSamples.length,
      unpagedMax,
      unpagedFramesShare: panSamples.length ? Math.round((unpagedFrames / panSamples.length) * 1000) / 1000 : 0,
      holes_total: d(C0, C1, "terrain.holes_total"),
      hole_frames: d(C0, C1, "terrain.hole_frames_total"),
      rebakes: d(C0, C1, "terrain.rebakes_total"),
      evict,
      demote: d(C0, C1, "terrain.demote_total"),
      baked: d(C0, C1, "terrain.baked_total"),
      counterSamples: counterSeries.length,
    };

    // ---- turns
    const nTurns = Number(process.env.CIV5_PLAYTEST_TURNS ?? (o.turns && o.turns !== 50 ? o.turns : 3)); // run.mjs defaults --turns to 50: that value means "not given"
    const turns = [];
    let C = C1;
    for (let i = 0; i < nTurns; i++) {
      const before = (await readCounters(page)) ?? C;
      s = await until(page, (x) => x.status === "human" && x.turn !== null, 120000, `turn ${i + 1} ready`);
      const t = s.turn;
      const samples = [];
      await act(page, { kind: "endTurn", force: true });
      // Wait for the next turn, sampling the cells every 50 ms.
      const limit = Date.now() + 120000;
      for (;;) {
        await sample(samples);
        const x = await state(page);
        if (x && x.turn !== null && x.turn > t && x.status === "human") break;
        if (Date.now() > limit) throw new Error(`timeout waiting for the turn after ${t}`);
        await sleep(50);
      }
      // Let the pages settle (3 s), still sampling.
      const settleEnd = Date.now() + 3000;
      while (Date.now() < settleEnd) {
        await sample(samples);
        await sleep(50);
      }
      await sleep(1100);
      const after = (await readCounters(page)) ?? before;
      C = after;
      const rec = {
        turn: t,
        rebakes: d(before, after, "terrain.rebakes_total"),
        baked: d(before, after, "terrain.baked_total"),
        holes_total: d(before, after, "terrain.holes_total"),
        hole_frames: d(before, after, "terrain.hole_frames_total"),
        unpagedMax: Math.max(0, ...samples.map((x) => x.unpaged)),
        samples: samples.length,
        rebake: {},
        decalDirty: {},
      };
      for (const k of REBAKE) rec.rebake[k] = d(before, after, `terrain.rebake.${k}_total`);
      for (const k of DECAL) rec.decalDirty[k] = d(before, after, `terrain.decal_dirty.${k}_total`);
      turns.push(rec);
    }
    const C2 = C;

    const pages = {};
    for (const sz of SIZES) pages[sz] = C2?.[`terrain.pages.${sz}`] ?? null;
    result.numbers = {
      load: result.numbers.load,
      pan: panNums,
      turns,
      pageBytes: { start: C0?.["terrain.page_bytes"], afterPan: C1?.["terrain.page_bytes"], end: C2?.["terrain.page_bytes"] },
      pagesAtEnd: pages,
      C0,
      C1,
      C2,
    };
    writeFileSync(join(o.logDir, "terrain-turn.json"), JSON.stringify(result.numbers, null, 2));

    check("counters published", C0 && typeof C0["terrain.page_bytes"] === "number", C0 ? `${Object.keys(C0).length} counters, page_bytes ${C0["terrain.page_bytes"]}` : "window.__civ5stats.counters is missing (stats=1)");
    check("AC-T2: no hole while panning after load", panNums.samples > 20 && unpagedMax === 0 && panNums.holes_total === 0, `${panNums.samples} samples, unpaged max ${unpagedMax} (${panNums.unpagedFramesShare} of samples), holes_total +${panNums.holes_total}, hole_frames +${panNums.hole_frames}, rebakes +${panNums.rebakes}, baked +${panNums.baked}, demote +${panNums.demote}, evict ${JSON.stringify(evict)}`);

    const unexplained = turns.filter((t) => t.rebakes > 0 && t.rebake.field + t.rebake.decal === 0);
    const perTurn = turns.map((t) => `t${t.turn}: rebakes ${t.rebakes} (field ${t.rebake.field}, decal ${t.rebake.decal}, settings ${t.rebake.settings})`).join("; ");
    check("AC-T1: no rebake at end turn without a change", turns.length > 0 && unexplained.length === 0, perTurn || "no turns played");
    const sum = (f) => turns.reduce((a, t) => a + f(t), 0);
    check("AC-T1: end turn rebakes (info)", true, `${turns.length} turn(s): rebakes ${sum((t) => t.rebakes)}, baked ${sum((t) => t.baked)}, field ${sum((t) => t.rebake.field)}, decal ${sum((t) => t.rebake.decal)}, settings ${sum((t) => t.rebake.settings)}; decal dirty ${DECAL.map((k) => `${k} ${sum((t) => t.decalDirty[k])}`).join(", ")}`);
    const turnUnpaged = Math.max(0, ...turns.map((t) => t.unpagedMax));
    const turnHoles = sum((t) => t.holes_total);
    check("AC-T1: no hole at end turn", turns.length > 0 && turnUnpaged === 0, `unpaged max ${turnUnpaged} over ${sum((t) => t.samples)} samples, holes_total +${turnHoles}, hole_frames +${sum((t) => t.hole_frames)}`);

    const base = Number(process.env.CIV5_TERRAIN_BASE_BYTES);
    const end = C2?.["terrain.page_bytes"];
    if (base > 0) check("AC-T4: page memory", typeof end === "number" && end <= 1.1 * base, `page_bytes ${end} at the end, baseline ${base} (limit ${Math.round(1.1 * base)}); start ${C0?.["terrain.page_bytes"]}, after pan ${C1?.["terrain.page_bytes"]}; pages ${JSON.stringify(pages)}`);
    else check("AC-T4: page memory", true, `no baseline set (page_bytes start ${C0?.["terrain.page_bytes"]}, after pan ${C1?.["terrain.page_bytes"]}, end ${end}; pages ${JSON.stringify(pages)})`);
  } catch (e) {
    error = e;
  }
  result.consoleErrors.push(...b.errors);
  await b.close();
  check("no console errors", result.consoleErrors.length === 0, `${result.consoleErrors.length} error(s)${result.consoleErrors[0] ? `: ${result.consoleErrors[0]}` : ""}`);
  if (error) {
    check("no scenario error", false, error.message);
    result.error = error.stack ?? String(error);
  }
  result.pass = result.checks.every((c) => c.pass);
  result.durationMs = Date.now() - started;
  return result;
}
