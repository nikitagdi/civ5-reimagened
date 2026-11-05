// `autostart` (net-client 4.11, N4.4): `?autostart=1` plays through the server without the lobby windows: after
// the login the page runs the quick start script by itself (`RoomScript`: a room of its own, the open slots to the
// AI, "Ready", Start) with the size, AI count, seed and mode of the address. The dev server wants its password
// once (the page has no token): the playtest types it, as a player does the first time. Then two turns on the
// game screen.
import { openPage, act, until, state } from "../lib/browser.mjs";
import { moveAUnit } from "../lib/game.mjs";

const PASSWORD = "dev";

export async function run(ctx) {
  const started = Date.now();
  const result = { scenario: "autostart", pass: false, logs: ctx.opts.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const b = await openPage({ headed: ctx.opts.headed, logDir: ctx.opts.logDir });
  let error = null;
  try {
    const url = `${ctx.server.url}/?test=1&nick=erin&autostart=1&ai=2&size=duel&seed=7&mode=simultaneous&ui=interim`;
    await b.page.goto(url);
    let s = await until(b.page, (x) => x.screen === "login" || x.screen === "play", 60000, "sign-in window or the game");
    if (s.screen === "login") await act(b.page, { kind: "login", nick: "erin", password: PASSWORD });
    s = await until(b.page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 90000, "the game, started by the address");
    check("autostart: the game starts after the login without a click", s.net.phase === "play", `turn ${s.turn}, ${s.players} civilizations`);
    const ai = s.net.seats.filter((x) => x.who === "ai").length;
    const humans = s.net.seats.filter((x) => x.who === "erin").length;
    check("autostart: the address asks for one human and two AI players", ai === 2 && humans === 1, `seats ${s.net.seats.map((x) => x.who).join(",")}`);
    const mode = s.net.turnState?.mode;
    check("autostart: the turn mode of the address (simultaneous)", mode === "simultaneous", `mode ${mode}`);
    if (ctx.opts.screenshot) await b.page.screenshot({ path: `${ctx.opts.logDir}/autostart.png` }).catch(() => {});
    // The city panel on the contexts of the server: the build list comes with the city's context, a choice is an
    // order whose picture shows at once.
    const city = s.cities[0];
    await act(b.page, { kind: "openCity", id: city.id });
    const panel = await until(b.page, (x) => x.cityPanel && !x.cityPanel.loading && x.cityPanel.builds.length > 0, 20000, "the city panel with its builds");
    check("city: the panel lists the builds of the city's context", panel.cityPanel.builds.length > 0, `${panel.cityPanel.name}: ${panel.cityPanel.builds.length} builds`);
    const pick = panel.cityPanel.builds.find((x) => !x.current) ?? panel.cityPanel.builds[0];
    await act(b.page, { kind: "chooseBuild", index: pick.index });
    const chosen = await until(b.page, (x) => x.net?.inflight === 0 && x.net?.overlay === 0 && x.cities[0].productionItem >= 0 && !x.cityPanel, 20000, "the production order settles and the panel closes");
    check("city: choosing a build is an order that settles, the panel closes", chosen.cities[0].productionItem >= 0, `${pick.name}: production item ${chosen.cities[0].productionItem}`);
    for (let t = s.turn; t < s.turn + 2; t++) {
      const now = await until(b.page, (x) => x.turn === t && x.status === "human", 60000, `turn ${t}`);
      const moved = await moveAUnit(b.page, now);
      check(`turn ${t}: a unit moves and the order settles`, !!moved, moved ? `unit ${moved.unit} ${moved.from} -> ${moved.to}` : "no unit could move");
      await act(b.page, { kind: "endTurn", force: true });
      await until(b.page, (x) => x.turn === t + 1 && x.status === "human", 60000, `turn ${t + 1}`);
    }
    const m = await b.page.evaluate(() => window.__civ5test.metrics());
    check("autostart: no keyframe was asked for", m.client?.needKeyframe.total === 0, JSON.stringify(m.client?.needKeyframe));
  } catch (e) {
    error = e;
    const last = await state(b.page).catch(() => null);
    if (last) result.numbers.lastScreen = `${last.screen}/${last.net?.phase}${last.net?.loginError ? ` (${last.net.loginError})` : ""}${last.fatal ? ` fatal: ${last.fatal}` : ""}`;
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
