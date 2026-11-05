// The steps of the M4 WO-E scenario (World Congress, spies, trade routes), by mouse on the original UI: controls are found by
// path / text in the screen dump and clicked in the middle of their rectangle. Shared by the scenario and the dev REPL
// (`tools/playtest/m4/woe-repl.mjs`). Never presses Escape without a popup up: with none, the key opens the game menu (K20).
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** @param {ReturnType<typeof import("./ui-driver.mjs").uiDriver>} ui */
export function woeSteps(b, ui, shot = async () => {}) {
  const notes = [];
  const note = (s) => { notes.push(s); console.log(`  . ${s}`); };

  async function until(pred, ms, what) {
    const t0 = Date.now();
    for (;;) {
      const v = await pred();
      if (v) return v;
      if (Date.now() - t0 > ms) throw new Error(`timeout (${ms} ms): ${what}`);
      await sleep(700);
    }
  }
  /** Clicks the first shown control under `filter` whose path matches `pathRe` (and whose text matches `textRe`). */
  async function clickPath(filter, pathRe, textRe) {
    const r = await ui.find(filter, (x) => pathRe.test(x.path) && (!textRe || textRe.test(x.text.trim())));
    if (!r) return null;
    await ui.click(r);
    return r;
  }
  /** Clicks the label with exactly this text (it sits inside its button). */
  async function clickLabel(filter, re) {
    const r = await ui.find(filter, (x) => x.cls === "Label" && re.test(x.text.trim()));
    if (!r) return null;
    await ui.click(r);
    return r;
  }
  async function waitClick(label, fn, ms = 30000) {
    return until(async () => { const r = await fn(); return r; }, ms, label);
  }
  const hidden = async (ctx) => (await ui.lua(`local c = LookUpControl("/InGame/${ctx}") print(TAG .. tostring(c == nil or c:IsHidden()))`))[0] === "true";
  const stats = () => b.page.evaluate(() => { const s = window.__civ5stats ?? {}; return { ui: s.ui ?? null, replica: s.replica ?? null, exe: s.exe ?? null }; });

  async function closeIfOpen(ctx) {
    if (await hidden(ctx)) return false;
    const ok = await clickPath(`/${ctx}`, /\/CloseButton$/);
    await sleep(2500);
    return !!ok;
  }

  /** K13: END TURN is blocked by "make a proposal" -> the congress overview opens -> a resolution is proposed and committed. */
  async function propose(resolution = /^Standing Army Tax$/) {
    const bl = await ui.blocking();
    note(`blocking before the proposal: ${bl.type}`);
    await ui.clickEndTurn();
    await waitClick("the congress overview", async () => !(await hidden("LeagueOverviewPopup")), 20000);
    await sleep(2500);
    await shot("k13-overview");
    await waitClick("Click to Propose", () => clickLabel("/LeagueOverviewPopup", /^Click to Propose$/), 20000);
    await waitClick("the list of resolutions", () => ui.find("/LeagueOverviewPopup", (x) => x.cls === "Label" && resolution.test(x.text.trim())), 20000);
    await sleep(2000);
    await shot("k13-propose-list");
    await clickLabel("/LeagueOverviewPopup", resolution);
    await waitClick("the staged proposal", () => ui.find("/LeagueOverviewPopup", (x) => /\/CommitButton$/.test(x.path)), 20000);
    await sleep(2500);
    await shot("k13-propose-staged");
    await clickPath("/LeagueOverviewPopup", /\/CommitButton$/);
    await waitClick("the confirmation", () => ui.find("/LeagueOverviewPopup", (x) => /\/ConfirmYes$/.test(x.path)), 20000);
    await sleep(1500);
    await shot("k13-propose-confirm");
    await clickPath("/LeagueOverviewPopup", /\/ConfirmYes$/);
    const left = await until(async () => {
      const [l] = await ui.lua(`local lg = Game.GetActiveLeague() print(TAG .. lg:GetRemainingProposalsForMember(Game.GetActivePlayer()))`);
      return l === "0" ? "0" : null;
    }, 30000, "the proposal on the replica");
    await sleep(1500);
    await shot("k13-proposed");
    const proposals = await ui.lua(`local lg = Game.GetActiveLeague() for i, r in ipairs(lg:GetEnactProposals()) do print(TAG .. r.ProposalPlayer .. ":" .. r.Type) end`);
    await closeIfOpen("LeagueOverviewPopup");
    return { remaining: left, proposals };
  }

  /** K14: the espionage overview (the corner's button) -> Move on the spy -> the city. */
  async function spyMove(city = "Beshbalik") {
    const btn = await ui.find("/DiploCorner", (x) => /\/EspionageButton$/.test(x.path));
    if (!btn) throw new Error("no espionage button in the diplomacy corner");
    await ui.click(btn);
    await waitClick("the espionage overview", async () => !(await hidden("EspionageOverview")), 20000);
    await sleep(2500);
    await shot("k14-overview");
    await waitClick("Move", () => clickPath("/EspionageOverview", /\/RelocateButton$/), 20000);
    await waitClick("the cities to choose from", () => ui.find("/EspionageOverview", (x) => x.cls === "Label" && x.text.trim() === city), 20000);
    await sleep(2000);
    await shot("k14-choose-city");
    await clickLabel("/EspionageOverview", new RegExp(`^${city}$`));
    await sleep(1500);
    // A capital asks whether the spy is a diplomat: "Spy" (the deny action).
    const ask = await ui.find("/EspionageOverview", (x) => /\/ConfirmNo$/.test(x.path));
    if (ask) { await shot("k14-diplomat-ask"); await ui.click(ask); }
    const spy = await until(async () => {
      const [l] = await ui.lua(`local s = Players[Game.GetActivePlayer()]:GetEspionageSpies()[1] print(TAG .. s.CityX .. "," .. s.CityY .. "," .. s.State)`);
      return l && !l.startsWith("-1,-1") ? l : null;
    }, 30000, "the spy on the replica");
    await sleep(2500);
    await shot("k14-spy-moved");
    await closeIfOpen("EspionageOverview");
    return spy;
  }

  /** K15: the caravan's trade-route button -> the destination list -> a city -> yes -> the overview lists the route. */
  async function tradeRoute(city = "Beshbalik") {
    await ui.lua(`for u in Players[Game.GetActivePlayer()]:Units() do if u:GetUnitType() == GameInfoTypes.UNIT_CARAVAN then UI.SelectUnit(u) end end`);
    await sleep(3000);
    const t = await waitClick("the trade route action", async () => (await ui.unitAction(/ESTABLISH_TRADE_ROUTE/)), 20000);
    note(`unit action: ${t}`);
    await waitClick("the destination list", () => ui.find("/ChooseInternationalTradeRoute", (x) => x.cls === "Label" && x.text.trim() === city), 20000);
    await sleep(2500);
    await shot("k15-choose-route");
    await clickLabel("/ChooseInternationalTradeRoute", new RegExp(`^${city}$`));
    await waitClick("the confirmation", () => ui.find("/ChooseInternationalTradeRoute", (x) => /\/ConfirmYes$/.test(x.path)), 20000);
    await sleep(1500);
    await shot("k15-confirm");
    await clickPath("/ChooseInternationalTradeRoute", /\/ConfirmYes$/);
    const routes = await until(async () => {
      const [l] = await ui.lua(`local r = Players[Game.GetActivePlayer()]:GetTradeRoutes() print(TAG .. (r and #r or 0))`);
      return Number(l) > 0 ? Number(l) : null;
    }, 30000, "the route on the replica");
    await sleep(2000);
    // The overview of the top panel lists it.
    const top = await ui.find("/TopPanel", (x) => /\/InternationalTradeRoutes$/.test(x.path));
    if (top) await ui.click(top);
    await waitClick("the trade route overview", async () => !(await hidden("TradeRouteOverview")), 20000);
    await sleep(3000);
    await shot("k15-overview");
    const row = await ui.find("/TradeRouteOverview", (x) => x.cls === "Label" && x.text.trim() === city);
    await closeIfOpen("TradeRouteOverview");
    return { routes, listed: !!row };
  }


  /** K13: turns until the congress is in session, then the vote: VoteUp on a proposal, Commit, Yes; the results window. */
  async function vote(td) {
    const inSession = async () => (await ui.lua(`local lg = Game.GetActiveLeague() print(TAG .. tostring(lg:IsInSession() and lg:CanVote(Game.GetActivePlayer())))`))[0] === "true";
    for (let i = 0; i < 40 && !(await inSession()); i++) await td.endTurn();
    if (!(await inSession())) throw new Error("the congress did not come into session");
    note(`in session at turn ${await td.turnNow()}`);
    await sleep(3000);
    const left = await td.clearOrdinary();
    note(`blocking in session: ${left.type}`);
    if (await hidden("LeagueOverviewPopup")) {
      await ui.clickEndTurn();
      await waitClick("the congress overview", async () => !(await hidden("LeagueOverviewPopup")), 20000);
    }
    await sleep(3000);
    await shot("k13-vote-overview");
    const before = (await ui.lua(`print(TAG .. Game.GetActiveLeague():GetRemainingVotesForMember(Game.GetActivePlayer()))`))[0];
    const up = await waitClick("VoteUp", () => clickPath("/LeagueOverviewPopup", /\/VoteUpButton$/), 20000);
    note(`vote up on ${up.path.slice(-60)}`);
    await sleep(2000);
    await shot("k13-vote-cast");
    await waitClick("Commit", () => clickPath("/LeagueOverviewPopup", /\/CommitButton$/), 20000);
    const yes = await ui.find("/LeagueOverviewPopup", (x) => /\/ConfirmYes$/.test(x.path));
    if (yes) { await sleep(1500); await shot("k13-vote-confirm"); await ui.click(yes); }
    const after = await until(async () => {
      const [l] = await ui.lua(`local lg = Game.GetActiveLeague() print(TAG .. lg:GetRemainingVotesForMember(Game.GetActivePlayer()) .. "," .. tostring(lg:CanVote(Game.GetActivePlayer())))`);
      return l && l !== `${before},true` ? l : null;
    }, 40000, "the vote on the replica");
    await sleep(4000);
    await shot("k13-voted");
    let results = !(await hidden("VoteResultsPopup")) || !(await hidden("LeagueProjectPopup"));
    if (!(await hidden("LeagueOverviewPopup"))) await closeIfOpen("LeagueOverviewPopup");
    return { before, after, resultsShown: results };
  }

  return { vote, propose, spyMove, tradeRoute, until, clickPath, clickLabel, hidden, closeIfOpen, stats, notes };
}
