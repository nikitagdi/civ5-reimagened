// `m4-congress-spies-routes` (M4 WO-E): K13 World Congress, K14 espionage, K15 trade routes on the original UI (EUI,
// the default game screen) and a real-core game, by mouse. Needs the real-core server (WSL, `tools/playtest/m2/server.ps1`) running with the
// DEV game data of `tools/playtest/m4/make-dev-gamedata-woe.ps1` (turn 3: Printing Press + Animal Husbandry for every major civ,
// all majors have met, a second city and a Caravan for the human; congress sessions every 2 turns) and `--attach <url>`:
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8820 --scenario m4-congress-spies-routes [--gfx webgl2] [--out dir]
// Flow at 1600x900: enter -> found a city -> turns until the dev block has run (turn 3) -> World Congress: a resolution is
// proposed, the session starts, a vote is cast from the overview -> espionage overview: a spy is moved to a city -> the caravan
// (unit panel button) opens the trade route list, a route is chosen -> the trade route overview lists it.
// Checks: ui.lua_errors == 0 after every window, the vote / spy / route are on the replica, a screenshot of each window.
// Env: M4_STAGE=<n> stops after stage n (1 enter+turns, 2 congress, 3 espionage, 4 trade routes); M4_NOMT=1 single-threaded.
import { openPage, act, until, state, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { uiDriver } from "../lib/ui-driver.mjs";
import { turnDriver } from "../lib/turns-woe.mjs";
import { woeSteps } from "../lib/steps-woe.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function stats(page) {
  return page.evaluate(() => {
    const s = window.__civ5stats ?? {};
    return { ui: s.ui ?? null, replica: s.replica ?? null, exe: s.exe ?? null, m2: s.m2 ?? null, camera: s.camera ?? null };
  });
}

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "m4-congress-spies-routes", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const base = o.attach ?? ctx.server?.url;
  if (!base) { check("options", false, "m4-congress-spies-routes needs --attach <url of the real-core server>"); return result; }
  const stage = Number(process.env.M4_STAGE ?? 9);
  const b = await openPage({ headed: o.headed, logDir: o.logDir, viewport: { width: 1600, height: 900 } });
  // (the page is started with preset=low and no fluid fog: the playtest machine renders in software; run.mjs --preset low)
  const shot = async (name) => { try { await b.page.screenshot({ path: `${o.logDir}/${name}.png` }); } catch { /* gone */ } };
  let error = null;
  try {
    for (let attempt = 0; ; attempt++) {
      const nick = `m4e${(Date.now() + attempt) % 1000000}`;
      await b.page.goto(appUrl(base, o, { nick, autostart: "1", ai: "2", size: "small", seed: "7", mode: "simultaneous", manual: "1", stats: "1", disable: "FluidFOW", ...(process.env.M4_NOMT ? { mt: "0" } : {}), ...(process.env.M4_LUADBG ? { luadbg: process.env.M4_LUADBG } : {}) }));
      try {
        const s = await until(b.page, (x) => x.screen === "login" || x.screen === "play", 90000, "sign-in window or the game");
        if (s.screen === "login") await act(b.page, { kind: "login", nick, password: DEV_PASSWORD });
        await until(b.page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 60000, "the game");
        break;
      } catch (e) { if (attempt >= 2) throw e; }
    }
    const upAt = Date.now();
    let st;
    for (;;) {
      st = await stats(b.page);
      if (st.m2?.phase === 2) break;
      if (st.m2?.phase === -1) throw new Error("M2 failed (phase -1)");
      if (Date.now() - upAt > 300000) throw new Error(`M2 is not up: ${JSON.stringify(st.m2)}`);
      await sleep(500);
    }
    await sleep(4000);
    st = await stats(b.page);
    check("the UI is up", st.m2?.phase === 2 && st.replica?.live === 1, JSON.stringify(st.m2));
    check("ui.lua_errors == 0 after the start", st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
    const ui = uiDriver(b);
    const errors = async (what) => {
      const s = await stats(b.page);
      check(`ui.lua_errors == 0 ${what}`, s.ui?.lua_errors === 0, `${s.ui?.lua_errors}`);
      return s;
    };

    // Stage 1: a city, then turns until the dev block (turn 3) has run.
    await ui.clickEndTurn();
    await sleep(9000);
    const citiesBefore = st.exe?.entity_cities ?? 0;
    check("the settler has a found-city button", !!(await ui.unitAction(/FOUND/)));
    await sleep(12000);
    st = await stats(b.page);
    check("a city was founded", (st.exe?.entity_cities ?? 0) === citiesBefore + 1, `cities ${citiesBefore} -> ${st.exe?.entity_cities}`);
    const td = turnDriver(b, ui, shot);
    await td.toTurn(4);
    await sleep(3000);
    await shot("e0-turn4");
    const [probe] = await ui.lua(`
      local p = Players[Game.GetActivePlayer()]
      local lg = Game.GetActiveLeague()
      local ns = #p:GetEspionageSpies()
      local ncar = 0 for u in p:Units() do if u:GetUnitType() == GameInfoTypes.UNIT_CARAVAN then ncar = ncar + 1 end end
      print(TAG .. string.format("turn=%d league=%s until=%d spies=%d caravans=%d era=%d", Game.GetGameTurn(), tostring(lg ~= nil and not lg:IsNone()), lg and lg:GetTurnsUntilSession() or -1, ns, ncar, p:GetCurrentEra()))`);
    result.numbers.afterDevBlock = probe;
    console.log("  dev block:", probe);
    check("the dev game data ran (league, a spy, a caravan)", /league=true/.test(probe) && /spies=1/.test(probe) && /caravans=1/.test(probe), probe);
    await errors("after the turns");
    if (stage <= 1) throw new Error("M4_STAGE: stopped after stage 1");
    const steps = woeSteps(b, ui, shot);

    // Stage 2, K13: a resolution is proposed from the congress overview.
    const prop = await steps.propose();
    check("a proposal is made in the congress (the member has none left, ours is listed)", prop.remaining === "0" && prop.proposals.some((l) => l.startsWith("0:")), JSON.stringify(prop));
    await errors("after the congress proposal");
    if (stage <= 2) throw new Error("M4_STAGE: stopped after stage 2");

    // Stage 3, K14: the spy is moved to a city from the espionage overview.
    const spy = await steps.spyMove("Beshbalik");
    check("the spy was assigned to the city (it travels there)", /^10,19,/.test(spy), spy);
    await errors("after the spy was moved");
    if (stage <= 3) throw new Error("M4_STAGE: stopped after stage 3");

    // Stage 4, K15: the caravan's trade route from its window.
    const tr = await steps.tradeRoute("Beshbalik");
    check("a trade route was established from the route window and is listed in the overview", tr.routes > 0 && tr.listed, JSON.stringify(tr));
    await errors("after the trade route");
    if (stage <= 4) throw new Error("M4_STAGE: stopped after stage 4");

    // Stage 5, K13: turns until the congress is in session, a vote is cast from the overview, the results.
    const v = await steps.vote(td);
    check("a vote was cast in the congress (the replica shows it)", v.after !== `${v.before},true`, JSON.stringify(v));
    await errors("after the vote");
  } catch (e) {
    error = e;
    await shot("failure");
    const last = await state(b.page).catch(() => null);
    if (last) result.numbers.lastScreen = `${last.screen}/${last.net?.phase}`;
    result.numbers.lastStats = await stats(b.page).catch(() => null);
  }
  result.consoleErrors.push(...b.errors);
  await b.close();
  check("no console errors", result.consoleErrors.length === 0, `${result.consoleErrors.length} error(s)${result.consoleErrors[0] ? `: ${result.consoleErrors[0]}` : ""}`);
  if (error) { check("no scenario error", false, error.message); result.error = error.stack ?? String(error); }
  result.pass = result.checks.every((c) => c.pass);
  result.durationMs = Date.now() - started;
  return result;
}
