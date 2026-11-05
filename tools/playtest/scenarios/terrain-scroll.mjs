// `terrain-scroll` (world-terrain 9, R3.3; AC-R1.5, AC-R1.7): a scroll along the whole width of a
// Standard map, east and then back west, through the seam (the map wraps in X), with the arrow keys
// as a player does. Every 50 ms the client's test port says how many terrain cells are in view and
// how many of them have no page yet (state 0, grey in the original):
//  - AC-R1.5: no stretch of frames with a cell in state 0 lasts longer than 1 s once the camera has
//    settled; the longest stretch is reported with the longest one during the scroll;
//  - AC-R1.7: no jump of the picture at the seam: every 200 ms a small copy of the screen is
//    taken, and the change between two copies must not be an outlier (more than 4 times the median
//    change). A jump of the whole picture at the seam is such an outlier.
// Run through `cargo xtask playtest --scenario terrain-scroll --headed` (the dev server, the
// fixture core); `--scroll-s N` sets the seconds of a direction (default 60).
import { openPage, act, until, state, appUrl, INTERIM } from "../lib/browser.mjs";
import { decode } from "../lib/png.mjs";
import { writeFileSync } from "node:fs";
import { join } from "node:path";

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
      for (let j = 0; j < bh; j += 4) {
        for (let i = 0; i < bw; i += 4) {
          const o = ((y * bh + j) * width + x * bw + i) * channels;
          sum += (px[o] + px[o + 1] + px[o + 2]) / 3;
        }
      }
      out[y * w + x] = sum / (Math.ceil(bw / 4) * Math.ceil(bh / 4));
    }
  }
  return out;
}

const change = (a, b) => a.reduce((s, v, i) => s + Math.abs(v - b[i]), 0) / a.length;
const median = (xs) => [...xs].sort((a, b) => a - b)[Math.floor(xs.length / 2)] ?? 0;

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "terrain-scroll", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const b = await openPage({ headed: o.headed, logDir: o.logDir });
  const { page } = b;
  let error = null;
  try {
    await page.goto(appUrl(ctx.server.url, o, { ...INTERIM, nick: "scroll", autostart: "1", ai: "1", size: process.env.CIV5_PLAYTEST_SIZE ?? "standard", seed: String(o.seed) }));
    let s = await until(page, (x) => x.screen === "login" || x.screen === "play", o.lobbyTimeoutMs, "sign-in window or the game");
    if (s.screen === "login") await act(page, { kind: "login", nick: "scroll", password: "dev" });
    s = await until(page, (x) => x.screen === "play" && x.status === "human" && x.terrainCells, o.mapTimeoutMs, "the map with terrain");
    // Let the first cells come.
    await until(page, (x) => x.terrainCells && x.terrainCells.visible > 0 && x.terrainCells.unpaged === 0, 60000, "the first view with all its pages");
    // The whole map comes with the reveal-all; let its pages settle (3 s without a cell in state 0).
    for (let calm = 0; calm < 3000; ) {
      await new Promise((r) => setTimeout(r, 250));
      const c = (await state(page))?.terrainCells;
      calm = c && c.unpaged === 0 ? calm + 250 : 0;
    }
    await page.mouse.move(640, 360);
    // The arrow keys reach the canvas when it has the focus: a click on the ground gives it (it selects nothing).
    await page.mouse.click(640, 360);
    await page.evaluate(() => {
      const c = document.querySelector("canvas");
      if (c) {
        if (c.tabIndex < 0) c.tabIndex = 0;
        c.focus();
      }
    });

    const seconds = o.scrollS ?? 60;
    const samples = [];
    const copies = [];
    let prevShot = null;
    const times = [];
    const marks = []; // copy indexes where a key goes down or up: the camera starts or stops there
    let jumps = 0;
    let longest = { settled: 0, moving: 0 };
    let run0 = null;
    let moving = true;
    const t0 = Date.now();
    const sample = async () => {
      const x = await state(page);
      const c = x?.terrainCells;
      if (!c) return;
      const at = Date.now() - t0;
      samples.push({ at, visible: c.visible, unpaged: c.unpaged, moving });
      if (c.unpaged > 0) {
        run0 ??= { at, moving };
      } else if (run0) {
        const len = at - run0.at;
        const key = run0.moving ? "moving" : "settled";
        longest[key] = Math.max(longest[key], len);
        run0 = null;
      }
    };
    const scroll = async (key, ms) => {
      await page.keyboard.down(key);
      marks.push(copies.length);
      const end = Date.now() + ms;
      let nextShot = 0;
      while (Date.now() < end) {
        await sample();
        if (Date.now() >= nextShot) {
          const shot = await page.screenshot({ type: "png" });
          const cp = small(shot);
          // The picture before and after a big change is kept for a look.
          if (copies.length && change(copies.at(-1), cp) > 8 && jumps < 4) {
            writeFileSync(join(o.logDir, `jump-${jumps}-before.png`), prevShot);
            writeFileSync(join(o.logDir, `jump-${jumps}-after.png`), shot);
            jumps++;
          }
          prevShot = shot;
          if (copies.length % 20 === 0 && copies.length < 240) writeFileSync(join(o.logDir, `seq-${String(copies.length).padStart(3, "0")}.png`), shot);
          copies.push(cp);
          times.push(Date.now());
          nextShot = Date.now() + 200;
        }
        await new Promise((r) => setTimeout(r, 50));
      }
      await page.keyboard.up(key);
      marks.push(copies.length);
    };
    writeFileSync(join(o.logDir, "scroll-before.png"), await page.screenshot({ type: "png" }));
    await scroll("ArrowRight", Math.min(seconds * 1000, 4000));
    writeFileSync(join(o.logDir, "scroll-after.png"), await page.screenshot({ type: "png" }));
    await scroll("ArrowRight", seconds * 1000);
    moving = false;
    // Settled: the camera stands; the cells in view must all have their pages within a second.
    for (let i = 0; i < 40; i++) {
      await sample();
      await new Promise((r) => setTimeout(r, 50));
    }
    moving = true;
    await scroll("ArrowLeft", seconds * 1000);
    moving = false;
    for (let i = 0; i < 40; i++) {
      await sample();
      await new Promise((r) => setTimeout(r, 50));
    }
    if (run0) longest[run0.moving ? "moving" : "settled"] = Math.max(longest[run0.moving ? "moving" : "settled"], samples.at(-1).at - run0.at);

    const withCells = samples.filter((x) => x.visible > 0);
    const worst = Math.max(0, ...withCells.map((x) => x.unpaged / x.visible));
    result.numbers = {
      scrollSeconds: seconds,
      samples: samples.length,
      visibleCellsMax: Math.max(0, ...samples.map((x) => x.visible)),
      unpagedShareWorst: Math.round(worst * 1000) / 1000,
      longestUnpagedMovingMs: longest.moving,
      longestUnpagedSettledMs: longest.settled,
    };
    check("AC-R1.5: no cell in state 0 for more than 1 s once the camera stands", samples.length > 20 && longest.settled <= 1000, `longest ${longest.settled} ms settled, ${longest.moving} ms while moving; ${samples.length} samples, worst share ${result.numbers.unpagedShareWorst}`);

    // The camera starts and stops with the keys (a lurch of a whole screen between two copies, not a
    // seam): the diffs within 4 copies of such a mark are left out of the outlier test.
    const diffs = [];
    const dts = [];
    for (let i = 1; i < copies.length; i++) {
      if (marks.some((m) => Math.abs(i - m) <= 4)) continue;
      diffs.push(change(copies[i - 1], copies[i]));
      dts.push(times[i] - times[i - 1]);
    }
    const med = median(diffs);
    const max = Math.max(0, ...diffs);
    const at = diffs.indexOf(max);
    result.numbers.copies = copies.length;
    result.numbers.changeMedian = Math.round(med * 100) / 100;
    result.numbers.changeMax = Math.round(max * 100) / 100;
    // The server runs with the dev reveal-all (CIV5_FIXTURE_REVEAL_ALL), so the picture must keep changing
    // with the scroll; a median change of 0 means the camera did not move (or the map is blank): a failure.
    check("AC-R1.7: no jump of the picture along the scroll (the seam included)", copies.length >= 4 && med >= 0.05 && max <= Math.max(4 * med, 3), `median change ${med.toFixed(2)}, largest ${max.toFixed(2)} at copy ${at} of ${copies.length}`);
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
