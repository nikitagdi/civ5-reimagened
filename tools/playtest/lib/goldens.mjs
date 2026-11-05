// The golden scenes of AC-S2 (B4 M5): the original game's frames (`assets-out/goldens-b4-ui/S*.png`, 1920x1080, MP LAN) and what
// our client has to do to show the same scene. The scenario `m5-goldens` (WO-H: S1-S3, WO-I: S4, S5) walks this list; a new
// scene is one more row of `SCENES` (and, for a new save, one more entry of `GAMES` and a line in `m5/make-golden-state.ps1`).
//
// A GAME is a save of the original imported into the server (`tools/playtest/m5/make-golden-state.ps1`: sleeping games whose seat 0
// belongs to `nick`; the login wakes the game). The scenes of one game run one after the other in one browser session, in list order.
// The saves are the state at the end of turn N of the original's game; the golden shots are the next turn (the display says N+1),
// so a game's `prepare` ends the turn once (`advanceTo`) before the first scene.
import { act, until, appUrl, state } from "./browser.mjs";
import { DEV_PASSWORD } from "./game.mjs";
import { uiDriver, BLOCK } from "./ui-driver.mjs";

export const GOLDEN_DIR = process.env.CIV5_GOLDEN_DIR ?? "../assets-out/goldens-b4-ui";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// SIMPLIFIED(SIMP-exe-425): what stays different from the goldens (renderer terrain/water/clouds/models, MP list nick, small spacing of the ribbon rows).
/** The imported games: key -> {nick, turn: the turn of the save, goal: the turn the goldens show}. */
/** The zoom of the original's default camera in the goldens (a bit closer than our START_LEVEL 14.25: the banners of S1 are 1.044 times further apart). */
const LEVEL = 13.96;

export const GAMES = {
  A: { nick: "m5a", turn: 25, goal: 26, saves: "S1, S2, S5" },
  B: { nick: "m5b", turn: 5, goal: 6, saves: "S3, S4" },
};

/**
 * The scenes. `golden`: the file in GOLDEN_DIR. `shoot(g)` brings our client to the scene (g = the game session: {ui, page, b, sleep,
 * lua(code), camera(x, y, px, py, level), shot()}), the frame is taken after it returns. `hide`: rectangles (x, y, w, h in
 * 1920x1080 px) that are not compared (live data of the original that has no twin: the chat line's cursor, ...). `notes`: what is
 * known to differ and is not worth fixing (it goes to the report and to SIMP rows).
 * @type {{id: string, game: string, golden: string, title: string, shoot: (g: any) => Promise<void>, hide?: number[][], notes?: string}[]}
 */
export const SCENES = [
  {
    id: "S1",
    game: "A",
    golden: "S1-map-flags-banners.png",
    title: "map with flags, banners and the top panel",
    // The golden: the banner of Addis Ababa (8,16) at (1420, 713) and that of Harar (5,18) at (665, 407); our camera at level 13.96 and
    // the plot at (1406, 772) put them there.
    async shoot(g) {
      await g.showGrid();
      await g.camera(8, 16, 1406.4 / 1920, 772.5 / 1080, LEVEL);
      await g.hover(1650, 110);
      await g.settle();
    },
  },
  {
    id: "S2",
    game: "A",
    golden: "S2-unit-panel-tooltip.png",
    title: "unit panel with a selected unit and its tooltip",
    // The golden: the camera on Addis Ababa (8,16), the Warrior in it selected, the mouse over the unit portrait (EUI_UnitPanelItemTooltip, with its own big portrait on the right).
    async shoot(g) {
      await g.showGrid();
      await g.camera(8, 16, 948.4 / 1920, 506.5 / 1080, LEVEL);
      await g.lua(`local u = Players[Game.GetActivePlayer()]:GetUnitByID(40962) if u then UI.SelectUnit(u) end`);
      await g.sleep(1500);
      await g.hover(25, 1025);
      await g.settle();
    },
  },
  {
    id: "S3",
    game: "B",
    golden: "S3-city-screen.png",
    title: "city screen of the capital",
    // The golden: Addis Ababa (the capital) opened by its banner at turn 6; the camera is the city view's own (zoom of the city screen).
    async shoot(g) {
      await g.lua(`local c = Players[Game.GetActivePlayer()]:GetCapitalCity() if c then UI.DoSelectCityAtPlot(c:Plot()) end`);
      await g.sleep(3000);
      await g.hover(960, 250);
      await g.settle();
    },
  },
];

/** Signs in as the game's nick (its seat wakes from the save) and waits for the original UI; resolves to the game session object. */
export async function openGame(b, base, key, o = {}) {
  const game = GAMES[key];
  const page = b.page;
  const ui = uiDriver(b);
  await page.goto(appUrl(base, o, { nick: game.nick, ui: "original", stats: "1", ...(process.env.M3_NOMT ? { mt: "0" } : {}) }), { timeout: 180000 });
  const s = await until(page, (x) => x.screen === "login" || x.screen === "play", 90000, "sign-in window or the game");
  // the first sign-in of a page (no token) is refused by the server at once; the typed one right after it is refused as NickTaken
  // while the server has not let go of the first session yet
  if (s.screen === "login") {
    await sleep(3000);
    await act(page, { kind: "login", nick: game.nick, password: DEV_PASSWORD });
  }
  await until(page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 180000, "the game of the save");
  for (let i = 0; ; i++) {
    const st = await page.evaluate(() => window.__civ5stats ?? {});
    if (st.m2?.phase === 2) break;
    if (st.m2?.phase === -1) throw new Error("M2 failed (phase -1)");
    if (i > 360) throw new Error("M2 is not up");
    await sleep(500);
  }
  await sleep(3000);
  const stats = () => page.evaluate(() => window.__civ5stats ?? {});
  const g = {
    b, ui, page, sleep, key, game, stats,
    lua: (code, t) => ui.lua(code, t),
    /** `__exe_camera_place`: plot (x, y) at the screen fractions (px, py), zoom level (0: as is). */
    camera: (x, y, px, py, level) => ui.lua(`__exe_camera_place(${x}, ${y}, ${px}, ${py}, ${level})`),
    /** The original's options of the goldens show the hex grid; ours starts without it. */
    gridOn: false,
    async showGrid() { if (!this.gridOn) { await ui.lua(`UI.SetGridVisibleMode(true)`); this.gridOn = true; } },
    hover: async (x, y) => { await page.mouse.move(x - 5, y - 5); await page.mouse.move(x, y); },
    /** Waits until the frame stops changing (terrain pages, textures, animations are loaded). */
    async settle(maxMs = 60000) {
      let last = null, same = 0;
      const t0 = Date.now();
      while (Date.now() - t0 < maxMs) {
        await sleep(1500);
        const png = await page.screenshot();
        const h = png.length;
        same = last !== null && Math.abs(h - last) < 40 ? same + 1 : 0;
        last = h;
        if (same >= 3) return true;
      }
      return false;
    },
    /** One END TURN of the player's way (handles production / unit blocks) until the game is at `turn`. */
    async advanceTo(turn) {
      let at = (await stats()).replica?.turn ?? 0;
      while (at < turn) {
        let advanced = false;
        for (let step = 0; step < 24 && !advanced; step++) {
          const bl = await ui.blocking();
          if (bl.type === BLOCK.NONE) {
            await ui.clickEndTurn();
            for (let i = 0; i < 60 && !advanced; i++) { await sleep(1000); advanced = ((await stats()).replica?.turn ?? 0) > at; }
            continue;
          }
          await ui.handleBlock(bl);
          await sleep(2500);
          advanced = ((await stats()).replica?.turn ?? 0) > at;
        }
        if (!advanced) throw new Error(`the turn did not advance from ${at}`);
        at += 1;
        await sleep(3000);
      }
    },
  };
  return g;
}
