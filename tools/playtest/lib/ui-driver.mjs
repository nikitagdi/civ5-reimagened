// Drives the original UI (the default game screen) the way a player does, without hard-coded pixels: controls are found by their
// path / class / text in the screen dump (`__ui_dump_screen`, crates/client/ui debug::dump_screen, through
// `window.__civ5test.lua`) and clicked in the middle of their computed screen rectangle. The end-turn blocking reasons
// (`Player:GetEndTurnBlockingType`) are handled as a player would: choose production in the city screen, give a unit
// orders (fortify / skip), close what is open.
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** EndTurnBlockingTypes (CvEnums.h): none, policy, research, production, units ... (the ones the driver handles). */
export const BLOCK = { NONE: -1, PRODUCTION: 2, UNITS: 3 };

/**
 * @param {{page: import("playwright").Page, console: {text: string}[]}} b  the page of `openPage`
 */
export function uiDriver(b) {
  let seq = 0;
  const log = [];

  /** Runs Lua in the UI layer; resolves to the lines the code printed with `TAG ..` (the tag is cut off). */
  // (45 s: a turn change on a loaded machine holds the UI for longer than 20 s now and then)
  async function lua(code, timeoutMs = 45000) {
    const tag = `K1K3Q${++seq}:`;
    const from = b.console.length;
    await b.page.evaluate((c) => window.__civ5test.lua(c), `local TAG = "${tag}"\n${code}\nprint(TAG .. "end")`);
    const t0 = Date.now();
    for (;;) {
      const texts = b.console.slice(from).map((l) => l.text);
      if (texts.some((t) => t.includes(`${tag}end`))) {
        return texts.filter((t) => t.includes(tag)).map((t) => t.slice(t.indexOf(tag) + tag.length)).filter((t) => t !== "end");
      }
      if (Date.now() - t0 > timeoutMs) throw new Error(`the UI did not run the Lua in ${timeoutMs} ms`);
      await sleep(200);
    }
  }

  /** The shown controls whose path contains `filter`: [{path, cls, x, y, w, h, disabled, void1, text}]. */
  async function dump(filter = "") {
    const lines = await lua(`for l in string.gmatch(__ui_dump_screen(${JSON.stringify(filter)}) or "", "[^\\n]+") do print(TAG .. l) end`, 30000);
    return lines.map((l) => {
      const [path, cls, rect, flags = "", text = ""] = l.split("\t");
      const [x, y, w, h] = rect.split(" ").map(Number);
      return { path, cls, x, y, w, h, disabled: flags.includes("D"), void1: /v(-?\d+)/.test(flags) ? Number(/v(-?\d+)/.exec(flags)[1]) : null, text };
    });
  }

  async function uiScale() {
    return (await b.page.evaluate(() => window.__civ5stats?.camera?.ui_scale)) ?? 1;
  }

  /** Clicks the middle of a dump row's rectangle. */
  async function click(row) {
    const s = await uiScale();
    const x = Math.round((row.x + row.w / 2) * s);
    const y = Math.round((row.y + row.h / 2) * s);
    log.push(`click ${row.path} (${x},${y})`);
    await b.page.mouse.move(x, y);
    await b.page.mouse.click(x, y);
  }

  /** The first dump row (of those under `filter`) that `pred` accepts, or null. */
  async function find(filter, pred) {
    return (await dump(filter)).find(pred) ?? null;
  }

  async function blocking() {
    const [l] = await lua(`local p = Players[Game.GetActivePlayer()] print(TAG .. tostring(p:GetEndTurnBlockingType()) .. "," .. tostring(Game.GetGameTurn()))`);
    const [type, turn] = l.split(",").map(Number);
    return { type, turn };
  }

  const isEndTurn = (r) => /\/ActionInfoPanel\/EndTurnButton$/.test(r.path) && r.cls === "GridButton";

  async function clickEndTurn() {
    const r = await find("/ActionInfoPanel/EndTurnButton", isEndTurn);
    if (!r) throw new Error("no END TURN button on the screen");
    await click(r);
  }

  /** The enabled unit action buttons with their action types: [{row, type}]. */
  async function unitActions() {
    const rows = (await dump("/UnitPanel/Base/Panel/Actions/ActionStack")).filter((r) => /\/(UnitActionButton|BuildCityButton)$/.test(r.path) && !r.disabled && r.void1 !== null);
    if (!rows.length) return [];
    const ids = rows.map((r) => r.void1);
    const out = await lua(`for _, id in ipairs({${ids.join(",")}}) do local a = GameInfoActions[id] print(TAG .. id .. " " .. tostring(a and a.Type)) end`);
    const types = new Map(out.map((l) => l.split(" ")).map(([id, t]) => [Number(id), t]));
    // BuildCityButton is the settler panel's own found-city button (its void is the action id too).
    return rows.map((row) => ({ row, type: types.get(row.void1) ?? "?" }));
  }

  /** Clicks the first enabled unit action whose type matches one of `res` (in that order); returns its type or null. */
  async function unitAction(...res) {
    const acts = await unitActions();
    for (const re of res) {
      const a = acts.find((x) => re.test(x.type));
      if (a) {
        await click(a.row);
        return a.type;
      }
    }
    return null;
  }

  const cityScreenShown = async () => (await dump("/CityView/QueueBackground")).length > 0;

  /** In the open city screen: the first unit of the production list ("Warrior" when it is there), by mouse. */
  async function pickProduction(prefer = /^warrior$/i) {
    const rows = await dump("/CityView/QueueBackground/Queue/SelectionScrollPanel/SelectionStacks/UnitButtonStack/Button");
    const names = rows.filter((r) => r.cls === "Label" && /\/Button\/Name$/.test(r.path));
    const pick = names.find((r) => prefer.test(r.text.trim())) ?? names[0];
    if (!pick) throw new Error("the city screen has no production list");
    // The label sits inside its button: the click lands on the button.
    await click(pick);
    return pick.text.trim();
  }

  /** Closes the city screen with its RETURN button when it is up. */
  async function closeCityScreen() {
    const r = await find("/CityView", (x) => x.cls === "Label" && /^return$/i.test(x.text.trim())) ??
      (await find("/CityView", (x) => /ReturnToMapButton|ReturnButton|ExitCityScreen/i.test(x.path) && x.cls.includes("Button")));
    if (r) await click(r);
    return !!r;
  }

  /** One step against what blocks END TURN; returns a short note of what was done. */
  async function handleBlock(bl) {
    if (bl.type === BLOCK.PRODUCTION) {
      await clickEndTurn(); // "Choose production": it opens the city screen
      await sleep(4000);
      if (!(await cityScreenShown())) return "production: the city screen did not open";
      const what = await pickProduction();
      await sleep(4000);
      if (await cityScreenShown()) await closeCityScreen();
      return `production: ${what}`;
    }
    if (bl.type === BLOCK.UNITS) {
      await clickEndTurn(); // "A unit needs orders": it selects the next one
      await sleep(3000);
      const t = await unitAction(/FORTIFY/, /SKIP/, /SLEEP|ALERT/);
      return `units: ${t ?? "no action button"}`;
    }
    return `blocking ${bl.type}: END TURN`;
  }

  return { lua, dump, click, find, blocking, clickEndTurn, unitActions, unitAction, pickProduction, closeCityScreen, cityScreenShown, handleBlock, log, sleep };
}
