// `quality` (render-core 4.3, R5.5; AC-R8.1): the presets Low, Medium and High switch inside a running game, with no
// reload of the page and no new game. A duel through the server (the `autostart` path); the page starts on High, and
// the scenario sets Low, Medium and High again through the settings window's action (`setQuality`), as a player does.
// After each switch:
//  - the renderer reports the preset (`state().quality`), the game stays on its screen and its turn;
//  - the terrain pages go with the switch and the cells in view have them again (no page-less cell is left; the
//    page-in speed is about a page a second on a headless software GL, so it waits on progress, not a fixed 20 s);
//  - the page was not reloaded (a marker on `window`, the time origin) and no console error came;
//  - the picture changed with the preset (a small grey copy of the screen differs between Low and High; the High that
//    comes back is only measured, the clouds and the water move);
//  - the pass table of the frame (`__civ5stats.passes`) is published (the tables are in the numbers). That Low draws
//    no reflection and no object shadow is proven by the render crate's test
//    `the_low_preset_has_no_object_shadows_and_no_reflection` (the table lists every pass, drawn or not).
// At the end the game goes on: one turn is ended and the next one comes. The pass tables are in the numbers.
import { openPage, act, until, state, appUrl, INTERIM } from "../lib/browser.mjs";
import { decode } from "../lib/png.mjs";
import { writeFileSync } from "node:fs";
import { join } from "node:path";

const PASSWORD = "dev";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** A 64 x 36 grey copy of a screenshot (box average). */
function small(buf) {
  const { width, height, channels, px } = decode(buf);
  const w = 64;
  const h = 36;
  const out = new Float32Array(w * h);
  const bw = Math.floor(width / w);
  const bh = Math.floor(height / h);
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      let sum = 0;
      let n = 0;
      for (let j = 0; j < bh; j += 4) {
        for (let i = 0; i < bw; i += 4) {
          const o = ((y * bh + j) * width + x * bw + i) * channels;
          sum += (px[o] + px[o + 1] + px[o + 2]) / 3;
          n++;
        }
      }
      out[y * w + x] = sum / n;
    }
  }
  return out;
}
const diff = (a, b) => a.reduce((s, v, i) => s + Math.abs(v - b[i]), 0) / a.length;

/**
 * The terrain pages after a preset switch. The switch throws the baked pages away (new keys, and under Low the
 * `@low` textures: the library epoch rebuilds the live map) and the cells in view get theirs back at the page-in
 * speed of the preset, about one page a second on the software GL of a headless Chrome. So there is no fixed time to
 * wait: the pages must go (`dropped` is the most unpaged cells seen) and come back, with no stall of
 * `STALL_MS` in the count and all within `TOTAL_MS`.
 */
const STALL_MS = 20000;
const TOTAL_MS = 180000;
async function pagesBack(page, preset) {
  const t0 = Date.now();
  let dropped = 0;
  let last = null;
  let progressAt = t0;
  for (;;) {
    const x = await state(page);
    const c = x.terrainCells;
    if (c && c.visible > 0) {
      dropped = Math.max(dropped, c.unpaged);
      if (dropped > 0 && c.unpaged === 0) return { state: x, ms: Date.now() - t0, dropped };
      // Any change of the count is life: a page comes back, or the live map is rebuilt (the count falls and rises).
      if (c.unpaged !== last) {
        last = c.unpaged;
        progressAt = Date.now();
      }
    }
    const now = Date.now();
    if (now - progressAt > STALL_MS) throw new Error(`pages of the terrain under ${preset}: no page in ${STALL_MS} ms (${c?.unpaged} of ${c?.visible} cells unpaged)`);
    if (now - t0 > TOTAL_MS) throw new Error(`pages of the terrain under ${preset}: not all back in ${TOTAL_MS} ms (${c?.unpaged} of ${c?.visible} cells unpaged)`);
    await sleep(250);
  }
}

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "quality", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const n = result.numbers;
  const b = await openPage({ headed: o.headed, logDir: o.logDir });
  const { page } = b;
  let error = null;
  try {
    await page.goto(appUrl(ctx.server.url, { gfx: o.gfx }, { ...INTERIM, nick: "quality", autostart: "1", ai: "1", size: "duel", seed: String(o.seed), preset: "high", stats: "1" }));
    let s = await until(page, (x) => x.screen === "login" || x.screen === "play", o.lobbyTimeoutMs, "sign-in window or the game");
    if (s.screen === "login") await act(page, { kind: "login", nick: "quality", password: PASSWORD });
    s = await until(page, (x) => x.screen === "play" && x.status === "human" && x.terrainCells, o.mapTimeoutMs, "the map with terrain");
    await until(page, (x) => x.terrainCells.visible > 0 && x.terrainCells.unpaged === 0, 60000, "the first view with all its pages");
    await page.evaluate(() => {
      window.__qualityMarker = { origin: performance.timeOrigin };
    });
    const turn = s.turn;
    check("the game starts on High", s.quality === "high", `quality ${s.quality}`);
    n.passes = {};
    n.shots = {};
    const shots = {};
    const look = async (name) => {
      await sleep(1500);
      const shot = await page.screenshot({ type: "png" });
      shots[name] = small(shot);
      writeFileSync(join(o.logDir, `quality-${name}.png`), shot);
      n.passes[name] = await page.evaluate(() => window.__civ5stats?.passes ?? null);
    };
    await look("high-start");

    for (const preset of ["low", "medium", "high"]) {
      await act(page, { kind: "setQuality", preset });
      s = await until(page, (x) => x.quality === preset, 10000, `the renderer reports ${preset}`);
      const back = await pagesBack(page, preset);
      s = back.state;
      n.pagesBack = n.pagesBack ?? {};
      n.pagesBack[preset] = { ms: back.ms, dropped: back.dropped };
      check(`${preset}: the pages are made again (dropped ${back.dropped}, back in ${(back.ms / 1000).toFixed(1)} s)`, back.dropped > 0, "the switch rebuilds the pages");
      check(`${preset}: the game stays where it was`, s.screen === "play" && s.turn === turn, `screen ${s.screen}, turn ${s.turn} of ${turn}`);
      await look(preset);
      const passes = n.passes[preset];
      check(`${preset}: pass times published`, !!passes && Object.keys(passes).length > 0, passes ? `${Object.keys(passes).length} passes` : "no __civ5stats.passes");
    }
    const lowVsHigh = diff(shots.low, shots["high-start"]);
    n.lowVsHighChange = Math.round(lowVsHigh * 100) / 100;
    check("the picture follows the preset (Low is not High)", lowVsHigh > 0.3, `mean grey change ${lowVsHigh.toFixed(2)}`);
    // The clouds and the water move between two copies, so the High that comes back is only compared in the numbers.
    n.highAgainChange = Math.round(diff(shots.high, shots["high-start"]) * 100) / 100;
    const marker = await page.evaluate(() => window.__qualityMarker ?? null);
    check("no reload of the page", !!marker, marker ? "marker kept" : "the page was reloaded");

    // The game goes on: one turn.
    await act(page, { kind: "endTurn", force: true });
    s = await until(page, (x) => x.turn === turn + 1 && x.status === "human", 120000, "the next turn");
    check("the game goes on after the switches", s.screen === "play", `turn ${s.turn}`);
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
