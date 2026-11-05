// The steps of the M5 WO-J acceptance scenario (`m5-acceptance`) that the M3/M4 libraries did not have as mouse steps: the tech tree,
// the social policy window, the pantheon popup, the city screen of a turn, a deal in the trade window. Built on `woeSteps` (the
// helpers `until` / `clickPath` / `clickLabel` / `hidden` / `closeIfOpen`) and `uiDriver`; the controls are found by path / class /
// text in the screen dump and clicked in the middle of their rectangle, never by pixels.
import { DEV_PASSWORD } from "./game.mjs";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/**
 * @param {ReturnType<typeof import("./ui-driver.mjs").uiDriver>} ui
 * @param {ReturnType<typeof import("./steps-woe.mjs").woeSteps>} woe
 * @param {{width: number, height: number}} view  the viewport (a tech button must lie in it to be clicked)
 */
export function wojSteps(b, ui, woe, view, shot = async () => {}) {
  const notes = [];
  const note = (s) => { notes.push(s); console.log(`  . ${s}`); };
  const lua1 = async (code) => (await ui.lua(code))[0];

  /** K9: the science button of the top panel -> the tree -> a researchable tech that is on the screen -> closed. Returns a note. */
  async function researchViaTree() {
    if (await woe.hidden("TechTree")) {
      const btn = await ui.find("/TopPanel", (x) => /\/SciencePerTurn$/.test(x.path));
      if (!btn) return "research: no science button";
      await ui.click(btn);
      await sleep(3500);
    }
    const techRows = async () => (await ui.dump("/TechTree")).filter((x) => /\/TechButton$/.test(x.path) && x.cls === "Button" && !x.disabled && x.void1 !== null
      && x.x >= 0 && x.y >= 0 && x.x + x.w <= view.width && x.y + x.h <= view.height);
    const rows = await woe.until(async () => { const r = await techRows(); return r.length ? r : null; }, 30000, "the tech buttons").catch(() => []);
    const ids = [...new Set(rows.map((r) => r.void1))];
    const ok = ids.length ? (await ui.lua(`local p = Players[Game.GetActivePlayer()] for _, id in ipairs({${ids.join(",")}}) do if p:CanResearch(id) then print(TAG .. id) end end`)).map(Number) : [];
    // the cheapest first: the lowest id is the oldest tech
    const pick = rows.filter((r) => ok.includes(r.void1)).sort((a, c) => a.void1 - c.void1)[0];
    if (!pick) {
      await woe.closeIfOpen("TechTree");
      return `research: no researchable tech on the screen (${ids.length} buttons)`;
    }
    await ui.click(pick);
    await sleep(3000);
    const cur = await lua1(`print(TAG .. GameInfo.Technologies[Players[Game.GetActivePlayer()]:GetCurrentResearch()].Type)`);
    await woe.closeIfOpen("TechTree");
    return `research: ${cur} (tree)`;
  }

  /** K10: the culture string -> the policy window -> an adoptable policy (or the opener of a branch) -> confirmed -> closed. */
  async function adoptViaWindow() {
    const before = Number(await lua1(`print(TAG .. Players[Game.GetActivePlayer()]:GetNumPolicies())`));
    if (await woe.hidden("SocialPolicyPopup")) {
      const btn = await ui.find("/TopPanel", (x) => /\/CultureString$/.test(x.path));
      if (!btn) return "policy: no culture button";
      await ui.click(btn);
      await sleep(3500);
    }
    const isBranch = (r) => /\/BranchButton[0-9]+$/.test(r.path);
    const policyRows = async () => (await ui.dump("/SocialPolicyPopup")).filter((x) => x.cls === "GridButton" && !x.disabled && x.void1 !== null
      && (/\/PolicyIcon$/.test(x.path) || isBranch(x)) && x.x >= 0 && x.y >= 0 && x.x + x.w <= view.width && x.y + x.h <= view.height);
    // (the window builds its buttons for a moment after it opens, longer on a loaded machine)
    const rows = await woe.until(async () => { const r = await policyRows(); return r.length ? r : null; }, 30000, "the policy buttons").catch(() => []);
    // an adopted policy's icon is not disabled either: ask the core which ones can be adopted now
    const ids = rows.filter((r) => /PolicyIcon$/.test(r.path)).map((r) => r.void1);
    const bids = rows.filter(isBranch).map((r) => r.void1);
    const okP = ids.length ? (await ui.lua(`local p = Players[Game.GetActivePlayer()] for _, id in ipairs({${ids.join(",")}}) do if p:CanAdoptPolicy(id) and not p:HasPolicy(id) then print(TAG .. id) end end`)).map(Number) : [];
    const okB = bids.length ? (await ui.lua(`local p = Players[Game.GetActivePlayer()] for _, id in ipairs({${bids.join(",")}}) do if p.CanUnlockPolicyBranch and p:CanUnlockPolicyBranch(id) then print(TAG .. id) end end`)).map(Number) : [];
    // (the first policy of the game opens a branch: its button is enabled even when the core's branch check says no)
    const pick = rows.find((r) => /PolicyIcon$/.test(r.path) && okP.includes(r.void1)) ?? rows.find((r) => isBranch(r) && okB.includes(r.void1))
      ?? (!okP.length && before === 0 ? rows.find(isBranch) : undefined);
    if (!pick) {
      await woe.closeIfOpen("SocialPolicyPopup");
      return "policy: nothing adoptable in the window";
    }
    await ui.click(pick);
    await sleep(2500);
    // "Are you sure you want to adopt this Social Policy?" (an opener of a branch is adopted without it)
    const yes = await ui.find("/SocialPolicyPopup", (x) => x.path.endsWith("/Yes") && /Confirm/.test(x.path));
    if (yes) { await ui.click(yes); await sleep(3000); }
    const after = Number(await lua1(`print(TAG .. Players[Game.GetActivePlayer()]:GetNumPolicies())`));
    await woe.closeIfOpen("SocialPolicyPopup");
    return `policy: ${after > before ? "adopted" : "NOT adopted"} (${before} -> ${after}) via ${pick.path.slice(-30)}`;
  }

  /** K11: the notification "Found a Pantheon" -> the belief list -> the first belief on the screen -> Yes. Returns the belief id or -1. */
  async function foundPantheon() {
    const n = await ui.find("/NotificationPanel", (x) => /\/FoundPantheon$/.test(x.path));
    if (!n) return -1;
    await ui.click(n);
    await sleep(3500);
    const belief = await ui.find("/ChoosePantheon", (x) => /ItemStack\/Button$/.test(x.path) && x.cls === "Button" && x.y >= 0 && x.y + x.h <= view.height);
    if (!belief) { await woe.closeIfOpen("ChoosePantheon"); return -1; }
    await ui.click(belief);
    await sleep(2500);
    const yes = await ui.find("/ChoosePantheon", (x) => /ChooseConfirm.*\/Yes$/.test(x.path));
    if (yes) await ui.click(yes);
    await sleep(5000);
    return Number(await lua1(`print(TAG .. Players[Game.GetActivePlayer()]:GetBeliefInPantheon())`));
  }

  /** K12: the diplomacy corner -> the overview -> the leader of an AI -> Trade -> "Accept Embassy" on our side -> Propose; true when
   *  the AI got the embassy (the deal was accepted). Leaves the screens again (Back on the table, Goodbye on the leader screen). */
  async function dealWithAI(ai) {
    const emb = async () => (await lua1(`print(TAG .. tostring(Teams[Players[${ai}]:GetTeam()]:HasEmbassyAtTeam(Game.GetActiveTeam())))`)) === "true";
    const before = await emb();
    const dbtn = await ui.find("", (x) => /\/DiploCornerStack\/DiploButton$/.test(x.path));
    if (!dbtn) throw new Error("no diplomacy overview button");
    await ui.click(dbtn);
    await sleep(4000);
    const leader = await ui.find("", (x) => /^\/InGame\/.*MajorCivScrollPanel\/ButtonStack\/DiploButton$/.test(x.path) && x.cls === "Button" && !x.disabled);
    if (!leader) throw new Error("the diplomacy overview lists no AI leader");
    await ui.click(leader);
    await sleep(5000);
    await shot("j-leader-screen");
    const trade = await woe.until(() => ui.find("/LeaderHeadRoot", (r) => /\/TradeButton$/.test(r.path) && !r.disabled), 20000, "the Trade button of the leader screen");
    await ui.click(trade);
    await sleep(3000);
    const item = await woe.until(() => ui.find("/LeaderHeadRoot/DiploTrade", (r) => /\/UsPocketAllowEmbassy$/.test(r.path) && !r.disabled), 20000, "our Accept Embassy item");
    await ui.click(item);
    await sleep(1500);
    await shot("j-trade-table");
    const propose = await ui.find("/LeaderHeadRoot/DiploTrade", (r) => /\/ProposeButton$/.test(r.path));
    if (!propose) throw new Error("no Propose button");
    await ui.click(propose);
    await sleep(8000);
    const made = !before && (await emb());
    for (const [root, re] of [["/LeaderHeadRoot/DiploTrade", /\/CancelButton$/], ["/LeaderHeadRoot/RootOptions", /\/GoodByeButton$/]]) {
      const btn = await ui.find(root, (r) => re.test(r.path) && !r.disabled);
      if (btn) { await ui.click(btn); await sleep(2500); }
    }
    // the overview the leader came from stays up behind the leader screen
    if (!(await woe.closeIfOpen("DiploOverview"))) {
      const close = await ui.find("", (x) => x.cls.includes("Button") && /DiploOverview/.test(x.path) && /Close/i.test(x.path));
      if (close) { await ui.click(close); await sleep(2000); }
    }
    return { before, made };
  }

  /** The religion overview from the faith string of the top panel (opened, checked, closed). Returns whether it opened. */
  async function openReligionOverview() {
    const btn = await ui.find("/TopPanel", (x) => x.path.endsWith("/FaithString"));
    if (!btn) return false;
    await ui.click(btn);
    await sleep(3500);
    const open = !(await woe.hidden("ReligionOverview"));
    await shot("j-religion-overview");
    if (open) await woe.closeIfOpen("ReligionOverview");
    return open;
  }

  const stackRows = async (stack) => (await ui.dump(`/CityView/QueueBackground/Queue/SelectionScrollPanel/SelectionStacks/${stack}/Button`))
    .filter((r) => r.cls === "Label" && r.path.endsWith("/Button/Name") && r.text.trim());

  /** In the open city screen: a building when the list has one (else a unit) by mouse; closes the screen. Returns what was chosen. */
  async function chooseInOpenScreen() {
    await woe.until(async () => (await stackRows("UnitButtonStack")).length > 0, 30000, "the production list of the city screen");
    // only rows that lie inside the list's visible rectangle can be clicked
    const panel = await ui.find("/CityView/QueueBackground/Queue/SelectionScrollPanel", (x) => x.path.endsWith("/SelectionScrollPanel"));
    const inside = (r) => !panel || (r.y >= panel.y && r.y + r.h <= panel.y + panel.h && r.x >= panel.x && r.x + r.w <= panel.x + panel.w);
    const queued = async () => Number(await lua1("local n = 0 for c in Players[Game.GetActivePlayer()]:Cities() do n = n + c:GetOrderQueueLength() end print(TAG .. n)"));
    const before = await queued();
    // buildings whose button is enabled (a building that cannot be built yet is listed greyed out)
    const all = await ui.dump("/CityView/QueueBackground/Queue/SelectionScrollPanel/SelectionStacks/BuildingButtonStack");
    const enabled = new Set(all.filter((r) => r.cls === "Button" && !r.disabled).map((r) => r.path));
    // ... and the ones the core says the city can build (the list shows the others greyed out, their buttons are not disabled)
    const canBuild = new Set((await ui.lua("for c in Players[Game.GetActivePlayer()]:Cities() do for b in GameInfo.Buildings() do if c:CanConstruct(b.ID) then print(TAG .. Locale.ConvertTextKey(b.Description)) end end break end")).map((s) => s.trim()));
    const buildings = (await stackRows("BuildingButtonStack")).filter((r) => inside(r) && enabled.has(r.path.replace(/\/Name$/, "")) && canBuild.has(r.text.trim()));
    const pick = buildings.find((r) => /monument|granary|shrine|library|barracks|walls|workshop|market/i.test(r.text)) ?? buildings[0];
    let what = "";
    if (pick) {
      await ui.click(pick);
      await sleep(3500);
      what = `building ${pick.text.trim()}`;
    }
    // (the screen closes by itself after a pick; when it is still up and nothing was queued, take a unit)
    if (!pick || ((await ui.cityScreenShown()) && (await queued()) <= before)) what = `unit ${await ui.pickProduction()}`;
    await sleep(3500);
    for (let i = 0; i < 3 && (await ui.cityScreenShown()); i++) {
      await ui.closeCityScreen();
      await sleep(2500);
    }
    return what;
  }

  /** hooks.block of the turn driver: production -> the city screen (a building when the list has one, else a unit), units -> found a
   *  city / fortify / skip. Returns a note, or null for the kinds it leaves to the driver. */
  async function block(bl) {
    if (bl.type === 2) {
      await ui.clickEndTurn(); // "Choose production" opens the city screen
      await sleep(4000);
      if (!(await ui.cityScreenShown())) return "production: the city screen did not open";
      return `production: ${await chooseInOpenScreen()}`;
    }
    if (bl.type === 20) {
      // "Make a proposal": every congress session round asks the members again (the first one is the scenario's own step)
      try {
        const r = await woe.propose(/^(Standing Army Tax|Embargo|Ban Luxury|World's Fair|Cultural Heritage Sites|Natural Heritage Sites|Arts Funding|Sciences Funding|Embargo City-States)$/);
        return `league: proposal made ${JSON.stringify(r.proposals)}`;
      } catch (e) {
        await woe.closeIfOpen("LeagueOverviewPopup");
        return `league: no proposal (${e.message})`;
      }
    }
    if (bl.type === 3) {
      await ui.clickEndTurn(); // "A unit needs orders" selects the next one
      await sleep(3000);
      const t = await ui.unitAction(/FOUND/, /FORTIFY/, /SKIP/, /SLEEP|ALERT/);
      return `units: ${t ?? "no action button"}`;
    }
    return null;
  }

  return { researchViaTree, adoptViaWindow, foundPantheon, dealWithAI, openReligionOverview, chooseInOpenScreen, block, lua1, notes, note };
}

/** Opens the game page of `base` with `params` (a new nick on a retry: the quick start sometimes stays in the room) and waits until
 *  the original UI is up and live (`m2.phase == 2`). `b` is the page of `openPage`; `util` = {act, until, appUrl}. Returns the nick. */
export async function enterGame(b, o, base, params, nickPrefix, util, stats) {
  const { act, until, appUrl } = util;
  let nick = "";
  for (let attempt = 0; ; attempt++) {
    nick = `${nickPrefix}${(Date.now() + attempt) % 1000000}`;
    await b.page.goto(appUrl(base, o, { nick, ui: "original", stats: "1", ...params }), { timeout: 180000 });
    try {
      const s = await until(b.page, (x) => x.screen === "login" || x.screen === "play", 90000, "sign-in window or the game");
      if (s.screen === "login") await act(b.page, { kind: "login", nick, password: DEV_PASSWORD });
      await until(b.page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 60000, "the game");
      break;
    } catch (e) {
      if (attempt >= 2) throw e;
    }
  }
  const upAt = Date.now();
  for (;;) {
    const st = await stats(b.page);
    if (st.m2?.phase === 2) break;
    if (st.m2?.phase === -1) throw new Error("M2 failed (phase -1)");
    if (Date.now() - upAt > 300000) throw new Error(`M2 is not up: ${JSON.stringify(st.m2)}`);
    await sleep(500);
  }
  await sleep(4000);
  return nick;
}
