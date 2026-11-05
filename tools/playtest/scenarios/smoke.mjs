// `smoke` (playtest 3.2, task H7; AC-9.4 with `saveload`, AC-5.x, AC-6.1, AC-7.2): every screen of the game is
// reached the way a player reaches it, in headless Chromium through the server (the client has no brain, net-client
// NC9): the sign-in, loading, play (the quick start of `?autostart=1`); the unit panel of a selected unit; a move
// (right click); found a city (if the game starts without one); the city window and a production choice; a purchase
// if one is affordable, else the disabled purchase buttons and why; the journal; the end-turn button's states over
// a few turns.
import { openPage, state, act, until, settle, appUrl, INTERIM } from "../lib/browser.mjs";
import { createPlayer, pickBuild, FOUND_CITY } from "../lib/policy.mjs";
import { JournalLog } from "../lib/journal.mjs";
import { round } from "../lib/report.mjs";

/** Polls the state every 25 ms until `pred`, noting every screen and loading stage on the way. */
async function watch(page, pred, timeoutMs, what, seen) {
  const start = Date.now();
  for (;;) {
    const s = await state(page);
    if (s) {
      seen.screens.add(s.screen);
      if (s.loading) seen.stages.add(s.loading);
      if (pred(s)) return s;
      if (s.fatal) throw Object.assign(new Error(`fatal: ${s.fatal}`), { fatal: s.fatal });
    }
    if (Date.now() - start > timeoutMs) throw new Error(`timeout after ${timeoutMs} ms: ${what} (screen ${s?.screen})`);
    await new Promise((r) => setTimeout(r, 25));
  }
}

async function selectUnit(page, id) {
  await act(page, { kind: "selectUnit", id });
  return until(page, (s) => s.selectedUnit === id && s.unitPanel && !s.unitPanel.loading, 10000, `unit panel of unit ${id}`);
}

async function openCity(page, id) {
  const s0 = await state(page);
  if (!(s0.cityPanel && !s0.cityPanel.loading)) await act(page, { kind: "openCity", id });
  return until(page, (s) => s.cityPanel && !s.cityPanel.loading, 15000, `city window of city ${id}`);
}

async function serverGame(ctx, result, check) {
  const o = ctx.opts;
  const b = await openPage({ headed: o.headed, logDir: o.logDir });
  const { page } = b;
  const seen = { screens: new Set(), stages: new Set(), endTurn: new Map() };
  const journal = new JournalLog();
  const observe = (s) => {
    journal.add(s);
    if (s?.endTurn) seen.endTurn.set(s.endTurn, s.status);
  };
  const player = createPlayer(page, { turnTimeoutMs: o.turnTimeoutMs, verbose: o.verbose, observe });
  const n = result.numbers;
  let error = null;
  let last = null;
  try {
    // The dev server wants its password once (the page has no token): typed as a player does the first time.
    await page.goto(appUrl(ctx.server.url, o, { ...INTERIM, nick: "smoke", autostart: "1", ai: String(o.ai), size: "duel", seed: String(o.seed) }));
    const first = await watch(page, (x) => x.screen === "login" || x.screen === "play", o.lobbyTimeoutMs, "sign-in window or the game", seen);
    if (first.screen === "login") await act(page, { kind: "login", nick: "smoke", password: "dev" });
    let s = await watch(page, (x) => x.screen === "play" && x.status === "human", o.mapTimeoutMs, "map", seen);
    observe(s);
    check("screens: loading, play", ["loading", "play"].every((x) => seen.screens.has(x)), `seen ${[...seen.screens].join(", ")}; stages: ${[...seen.stages].slice(0, 6).join(" / ")}`);

    // A unit: its panel, then a move.
    const settler = s.units.find((u) => /settler/i.test(u.name));
    const other = s.units.find((u) => u !== settler) ?? settler;
    const sel = await selectUnit(page, other.id);
    observe(sel);
    check("unit panel of a selected unit", sel.unitPanel.actions.length > 0, `${sel.unitPanel.name}: ${sel.unitPanel.actions.map((a) => `${a.label}${a.enabled ? "" : " (off)"}`).join(", ")}`);
    const moved = await player.moveAway(other);
    s = await state(page);
    const now = s.units.find((u) => u.id === other.id);
    check("move by right click", moved, `${other.name} ${other.x},${other.y} -> ${now ? `${now.x},${now.y}` : "gone"}`);

    // Found a city.
    // The test world of the server starts with a city: its settler then shows the other half of AC-5.7, the button
    // disabled with its reason (the fixture founds no cities). A world without a city needs the settler to found one.
    if (!s.cities.length) {
      if (!settler) throw new Error("no settler and no city at the start");
      const ss = await selectUnit(page, settler.id);
      const found = ss.unitPanel.actions.find((a) => a.action === FOUND_CITY);
      check("Found city offered for the settler", found?.enabled, found ? `${found.label}${found.enabled ? "" : `: ${found.reason}`}` : "no button");
      await act(page, { kind: "unitAction", action: FOUND_CITY });
      s = await until(page, (x) => x.cities.length > 0, 15000, "a city");
      observe(s);
    } else if (settler) {
      const ss = await selectUnit(page, settler.id);
      observe(ss);
      const found = ss.unitPanel.actions.find((a) => a.action === FOUND_CITY);
      check("Found city offered for the settler: disabled with a reason", !!found && !found.enabled && !!found.reason, found ? `${found.label}${found.enabled ? " (enabled)" : `: ${found.reason}`}` : "no button");
    }
    const city = s.cities[0];
    check("city founded", !!city, `${city.name} at ${city.x},${city.y}`);

    // The city window and a production choice.
    let cw = await openCity(page, city.id);
    observe(cw);
    const sections = [...new Set(cw.cityPanel.builds.map((x) => x.section))];
    check("city window with builds", cw.cityPanel.builds.length > 0, `${cw.cityPanel.name}: ${cw.cityPanel.builds.length} builds (${sections.join(", ")}), producing ${cw.cityPanel.producing ?? "nothing"}`);
    const build = pickBuild(cw.cityPanel);
    const itemBefore = city.productionItem;
    await act(page, { kind: "chooseBuild", index: build.index });
    s = await until(page, (x) => x.cities[0] && x.cities[0].productionItem !== itemBefore && x.cities[0].productionItem >= 0, 15000, "the city's new build");
    observe(s);
    check("production choice", true, `${build.name}: item ${itemBefore} -> ${s.cities[0].productionItem}, ${s.cities[0].productionTurns} turns`);

    // A purchase if one is affordable, else the disabled buttons and why.
    cw = await openCity(page, city.id);
    const offers = cw.cityPanel.builds.filter((x) => x.gold);
    const affordable = offers.find((x) => x.gold.enabled);
    n.purchase = { gold: cw.cityPanel.gold, offers: offers.length };
    if (affordable) {
      const goldBefore = cw.cityPanel.gold;
      await act(page, { kind: "buyBuild", index: affordable.index, faith: false });
      const after = await settle(page, (x) => (x.cityPanel && x.cityPanel.gold < goldBefore) || !!x.cityPanel?.refused, 15000);
      const ok = after?.cityPanel && after.cityPanel.gold < goldBefore;
      n.purchase = { ...n.purchase, bought: affordable.name, cost: affordable.gold.cost, goldAfter: after?.cityPanel?.gold ?? null, refused: after?.cityPanel?.refused ?? null };
      check("purchase with gold", ok, `${affordable.name} for ${affordable.gold.cost}: gold ${goldBefore} -> ${after?.cityPanel?.gold}${after?.cityPanel?.refused ? `, refused: ${after.cityPanel.refused}` : ""}`);
      result.purchase = "bought";
    } else {
      const sample = offers.slice(0, 3).map((x) => `${x.name}: ${x.gold.cost} gold, have ${cw.cityPanel.gold}${cw.cityPanel.gold < x.gold.cost ? " (not enough gold)" : " (cannot be bought now)"}`);
      // The server's fixture core sells nothing: no offers is a state to report, not a failure (the AC stays not-covered).
      check("purchase buttons disabled with a reason", offers.every((x) => !x.gold.enabled), offers.length ? sample.join("; ") : "no purchase offers (the core of the server sells nothing)");
      result.purchase = "disabled";
    }
    await act(page, { kind: "closeCity" });
    await settle(page, (x) => !x.cityPanel, 5000);

    // The plot tooltip: the real pointer rests on the map (G7: after 0.5 s).
    let tip = null;
    for (const [x, y] of [[700, 300], [560, 420], [820, 460], [400, 250]]) {
      await page.mouse.move(x, y);
      await page.mouse.move(x + 1, y);
      tip = await settle(page, (v) => v.tooltip?.length > 0, 2500);
      if (tip?.tooltip?.length) break;
    }
    check("plot tooltip on hover", tip?.tooltip?.length > 0, (tip?.tooltip ?? []).join(" / ") || "no tooltip");
    await page.mouse.move(5, 5);

    // A few turns: the end-turn states and the journal.
    const t0 = (await state(page)).turn;
    last = await player.play(t0 + o.smokeTurns);
    observe(last);
    check(`${o.smokeTurns} turns`, last.turn >= t0 + o.smokeTurns, `turn ${t0} -> ${last.turn}`);
    check("no stuck turn", !player.stuck, player.stuck ? JSON.stringify(player.stuck) : "");
    const labels = [...seen.endTurn.entries()].map(([label, status]) => `${label} [${status}]`);
    check("end-turn button states", seen.endTurn.size >= 2, labels.join("; "));
    n.endTurnStates = labels;
    n.aiTurnIndicatorSeen = [...seen.endTurn.values()].includes("ai");
    check("journal has lines", journal.size() > 0, `${journal.size()} lines; last: ${journal.lines().slice(-3).join(" | ")}`);
    const metrics = await page.evaluate(() => window.__civ5test.metrics());
    const st = metrics.stats ?? {};
    const mapAt = st.timeToMapMs ?? st.timing?.mapAtMs ?? null;
    n.timeToMapFromStartMs = mapAt !== null && metrics.startGameAtMs !== null ? round(mapAt - metrics.startGameAtMs) : null;
    n.loadingStages = [...seen.stages].slice(0, 10);
    n.journal = journal.summary();
    n.policy = player.policy;
  } catch (e) {
    error = e;
  }
  last = (await state(page).catch(() => null)) ?? last;
  const fatal = error?.fatal ?? last?.fatal ?? null;
  check("no fatal error", !fatal, fatal ?? "");
  check("no console errors", b.errors.length === 0, `${b.errors.length} error(s)`);
  result.consoleErrors.push(...b.errors);
  result.journalLines = journal.lines();
  if (o.screenshot) await page.screenshot({ path: `${o.logDir}/smoke.png` }).catch(() => {});
  await b.close();
  return error && !fatal ? error : null;
}

export async function run(ctx) {
  const started = Date.now();
  const result = { scenario: "smoke", pass: false, logs: ctx.opts.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const err = await serverGame(ctx, result, check);
  check("no scenario error", !err, err ? err.message : "");
  if (err) result.error = err.stack ?? String(err);
  result.pass = result.checks.every((c) => c.pass);
  result.durationMs = Date.now() - started;
  return result;
}
