// `choices-city` (b4/fix-choices-city, live reports 2026-10-30): the choices of the game are the human's and block END TURN, as in the
// original; the Great Prophet's "Found religion" opens the founding popup; the city screen takes orders right after a turn change.
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8960 --scenario choices-city [--turns 12]
// The server runs the DEV game data of `tools/playtest/m5/make-dev-gamedata-choice.ps1` (turn 3: +40 culture, +30 faith; turn 5: a Great
// Prophet in the capital). The page has NO `manual` flag: the original UI must leave the choices to the human by itself (the quick start
// and the lobby's rooms made `auto_choices` all on, so research / policies / the pantheon / the religion were chosen by the core).
// Unlike the turn driver of WO-E, a choice block is never answered by a hook before END TURN is checked: END TURN is clicked first and
// must leave the turn where it is.
import { openPage, act, until, appUrl } from "../lib/browser.mjs";
import { uiDriver } from "../lib/ui-driver.mjs";
import { woeSteps } from "../lib/steps-woe.mjs";
import { wojSteps, enterGame } from "../lib/steps-woj.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function stats(page) {
  return page.evaluate(() => {
    const s = window.__civ5stats ?? {};
    return { ui: s.ui ?? null, replica: s.replica ?? null, exe: s.exe ?? null, m2: s.m2 ?? null };
  });
}

export async function run(ctx) {
  const o = ctx.opts;
  const result = { scenario: "choices-city", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => { result.checks.push({ name, pass: !!pass, detail: detail ?? "" }); console.log(`  ${pass ? "ok  " : "FAIL"} ${name}${detail ? ` (${detail})` : ""}`); };
  const base = o.attach ?? ctx.server?.url;
  if (!base) { check("options", false, "choices-city needs --attach <url of the server with the choice dev game data>"); return result; }
  const turns = o.turns && o.turns !== 50 ? o.turns : 12;
  const view = { width: 1600, height: 900 };
  const b = await openPage({ headed: o.headed, logDir: o.logDir, consoleFile: "console.log", viewport: view });
  const shot = async (name) => { try { await b.page.screenshot({ path: `${o.logDir}/${name}.png` }); } catch { /* gone */ } };
  const num = result.numbers;
  globalThis.__chcPage = b.page;
  globalThis.__chcShot = shot;
  num.notes = [];
  const note = (s) => { num.notes.push(s); console.log(`  . ${s}`); };
  try {
    await enterGame(b, o, base, { autostart: "1", ai: "1", size: "duel", seed: "7" }, "chc", { act, until, appUrl }, stats);
    const ui = uiDriver(b);
    const woe = woeSteps(b, ui, shot);
    const woj = wojSteps(b, ui, woe, view, shot);
    const lua1 = async (code) => (await ui.lua(code))[0];
    const turnNow = async () => (await b.page.evaluate(() => window.__civ5stats?.replica?.turn ?? 0));
    const errors = async (what) => { const s = await stats(b.page); check(`ui.lua_errors == 0 ${what}`, s.ui?.lua_errors === 0, `${s.ui?.lua_errors}`); };
    const B = Object.fromEntries((await ui.lua(`for k, v in pairs(EndTurnBlockingTypes) do print(TAG .. k .. "=" .. v) end`)).map((l) => l.split("=")).map(([k, v]) => [k.replace("ENDTURN_BLOCKING_", ""), Number(v)]));
    num.blockingTypes = B;
    const endTurnText = async () => (await ui.dump("/ActionInfoPanel/EndTurnButton")).filter((r) => r.cls === "Label").map((r) => r.text.trim()).filter(Boolean).join(" | ");

    /** The block `type` is up: END TURN shows it and a click leaves the turn as it is (the click opens the choice instead). */
    async function stays(name, type) {
      const at = await turnNow();
      const bl = await ui.blocking();
      check(`${name}: END TURN is blocked by it (GetEndTurnBlockingType ${bl.type} == ${type})`, bl.type === type, `turn ${bl.turn}`);
      const text = await endTurnText();
      note(`${name}: END TURN says "${text}"`);
      await ui.clickEndTurn();
      await sleep(7000);
      check(`${name}: the END TURN click does not end the turn`, (await turnNow()) === at, `turn ${at} -> ${await turnNow()}`);
      return text;
    }

    /** The ordinary blocks (production, units) by the mouse helpers; returns the block left. */
    async function clearOrdinary() {
      for (let i = 0; i < 10; i++) {
        // a new city opens its screen by itself (the "choose production" popup)
        if (await ui.cityScreenShown()) { note(`production: ${await woj.chooseInOpenScreen()}`); await sleep(2500); continue; }
        const bl = await ui.blocking();
        if (bl.type === B.PRODUCTION || bl.type === B.UNITS) note(await woj.block(bl));
        else if (bl.type === B.STACKED_UNITS || bl.type === 7) {
          await ui.clickEndTurn(); await sleep(3000); note(`stacked: ${await ui.unitAction(/FORTIFY/, /SKIP/, /SLEEP|ALERT/)}`);
        } else return bl;
        await sleep(2500);
      }
      return ui.blocking();
    }

    // ---- The first city (END TURN selects the settler, its button founds the city), its production.
    await ui.clickEndTurn();
    await sleep(8000);
    check("the settler panel has a found-city button", !!(await ui.unitAction(/FOUND/)));
    await sleep(12000);
    await clearOrdinary();
    await errors("after the first city");

    // ---- Bug 1: research. The core no longer picks it: END TURN says so and stays.
    const auto = await lua1(`print(TAG .. tostring(Players[Game.GetActivePlayer()]:GetCurrentResearch()))`);
    check("research: nothing was chosen for the human", auto === "-1", `current research ${auto}`);
    let bl = await clearOrdinary();
    if (bl.type === B.RESEARCH) {
      await stays("research", B.RESEARCH);
      await shot("01-research-block");
      // The click opened the tech popup / tree (the original's "Choose Research"): pick in the tree.
      const opened = !(await woe.hidden("TechPopup")) || !(await woe.hidden("TechTree"));
      check("research: the END TURN click opens the tech chooser", opened);
      await woe.closeIfOpen("TechPopup");
      const r = await woj.researchViaTree();
      note(r);
      check("research: chosen in the tech tree", /^research: TECH_/.test(r), r);
    } else check("research: END TURN is blocked by research", false, `block ${bl.type}`);

    // ---- Turns: policy (turn 3 +40 culture), pantheon (+30 faith), Great Prophet (turn 5); after every turn change the city screen.
    const seen = { policy: false, pantheon: false, religion: false };
    let cityOk = 0, cityTries = 0;
    while ((await turnNow()) < turns) {
      // ---- Bug 2: the Great Prophet founds a religion with its button: the founding popup.
      const prophet = await lua1(`local p = Players[Game.GetActivePlayer()] local id = -1 for u in p:Units() do if u:GetUnitType() == GameInfoTypes.UNIT_PROPHET then id = u:GetID() end end print(TAG .. id)`);
      if (!seen.religion && prophet !== "-1") {
        seen.religion = true;
        await ui.lua(`local u = Players[Game.GetActivePlayer()]:GetUnitByID(${prophet}) if u then UI.SelectUnit(u) UI.LookAtSelectionPlot(0) end`);
        await sleep(3000);
        const acts = (await ui.unitActions()).map((a) => a.type);
        note(`prophet actions: ${acts.join(",")}`);
        const t = await ui.unitAction(/FOUND_RELIGION/);
        check("religion: the prophet has the Found Religion button", !!t, acts.join(","));
        await sleep(6000);
        await shot("03-found-religion");
        const popupUp = async () => !!(await ui.find("", (x) => /\/ChooseReligion/.test(x.path)));
        const atOnce = await popupUp();
        note(`religion: the founding popup ${atOnce ? "opened at once" : "did not open at once"}`);
        const rel = await lua1(`print(TAG .. tostring(Players[Game.GetActivePlayer()]:GetReligionCreatedByPlayer()))`);
        check("religion: nothing was founded for the human", Number(rel) <= 0, `religion ${rel}`);
        if (!atOnce) {
          await stays("religion", B.FOUND_RELIGION);
        } else {
          check("religion: END TURN is blocked by it", (await ui.blocking()).type === B.FOUND_RELIGION, `${(await ui.blocking()).type}`);
        }
        check("religion: the founding popup is up (BUTTONPOPUP_FOUND_RELIGION)", await popupUp());
        await shot("04-religion-popup");
        // The answer of the popup's Yes (the beliefs: the first available ones, as the popup's lists offer them).
        const sent = await lua1(`
          local p = Players[Game.GetActivePlayer()]
          local rel = -1
          for r in GameInfo.Religions() do
            if rel < 0 and r.Type ~= "RELIGION_PANTHEON" and Game.GetHolyCityForReligion(r.ID, -1) == nil then rel = r.ID end
          end
          local cap = p:GetCapitalCity()
          local fo, fl = Game.GetAvailableFounderBeliefs(), Game.GetAvailableFollowerBeliefs()
          local b1, b2, b3 = fo[1], fl[1], -1
          if not p:HasCreatedPantheon() then b1, b2, b3 = Game.GetAvailablePantheonBeliefs()[1], fo[1], fl[1] end
          Network.SendFoundReligion(Game.GetActivePlayer(), rel, "", b1, b2, b3, -1, cap:GetX(), cap:GetY())
          print(TAG .. rel .. " " .. tostring(b1) .. " " .. tostring(b2) .. " " .. tostring(b3))`);
        note(`religion: sent ${sent}`);
        { const c = await ui.find("", (x) => /\/ChooseReligion.*\/CloseButton$/.test(x.path)); if (c) { await ui.click(c); await sleep(2500); } }
        await sleep(6000);
        const rel2 = await lua1(`print(TAG .. tostring(Players[Game.GetActivePlayer()]:GetReligionCreatedByPlayer()))`);
        check("religion: founded with the popup's answer", Number(rel2) > 0, `religion ${rel2}`);
        await errors("after the religion");
        continue;
      }
      bl = await clearOrdinary();
      if (bl.type === B.RESEARCH) {
        await stays("research (later)", B.RESEARCH);
        await woe.closeIfOpen("TechPopup");
        note(await woj.researchViaTree());
        continue;
      }
      if (bl.type === B.POLICY) {
        const first = !seen.policy;
        seen.policy = true;
        await stays(first ? "policy" : "policy (later)", B.POLICY);
        if (first) await shot("02-policy-block");
        const p = await woj.adoptViaWindow();
        note(p);
        check(`${first ? "policy" : "policy (later)"}: adopted in the policy window`, /: adopted/.test(p), p);
        continue;
      }
      if (bl.type === B.FOUND_PANTHEON && !seen.pantheon) {
        seen.pantheon = true;
        await stays("pantheon", B.FOUND_PANTHEON);
        await woe.closeIfOpen("ChoosePantheon");
        const belief = await woj.foundPantheon();
        check("pantheon: founded in its popup", belief >= 0, `belief ${belief}`);
        continue;
      }
      if (bl.type !== -1) {
        // a block this scenario does not know: show it and stop
        check(`an unknown END TURN block (${bl.type})`, false, await endTurnText());
        break;
      }
      // ---- END TURN, then Bug 3: right after the turn change the city screen takes a production change and a worked tile.
      const at = await turnNow();
      if (at <= 2) {
        // the control: the same city screen steps in the middle of a turn (long after its change)
        const m = await cityRightAfterTurn(ui, woe, lua1, note);
        note(`turn ${at}: mid-turn control: ${m.ok ? "taken" : "NOT taken"}`);
        num[`midTurn${at}`] = m;
      }
      // Every other turn the city screen is up while the turn changes (a player looks at a city while the others move).
      const across = at % 2 === 0;
      if (across) {
        await ui.lua(`local c = Players[Game.GetActivePlayer()]:GetCapitalCity() if c then UI.DoSelectCityAtPlot(c:Plot()) end`);
        await woe.until(() => ui.cityScreenShown(), 15000, "the city screen").catch(() => false);
        await ui.lua(`Game.DoControl(GameInfoTypes.CONTROL_ENDTURN)`);
      } else await ui.clickEndTurn();
      const t0 = Date.now();
      while ((await turnNow()) <= at && Date.now() - t0 < 90000) await sleep(200);
      if ((await turnNow()) <= at) { check(`turn ${at}: END TURN advanced`, false, await endTurnText()); break; }
      cityTries += 1;
      if (across) note(`turn ${at + 1}: after the turn change with the city screen up, the screen is ${(await ui.cityScreenShown()) ? "still up" : "closed"} (core: ${await lua1("print(TAG .. tostring(UI.IsCityScreenUp()) .. ' head=' .. tostring(UI.GetHeadSelectedCity() and UI.GetHeadSelectedCity():GetID()))")})`);
      const r = await cityRightAfterTurn(ui, woe, lua1, note);
      if (r.ok) cityOk += 1;
      else check(`turn ${at + 1}: the city screen right after the turn change takes the orders`, false, JSON.stringify(r));
      await errors(`at turn ${at + 1}`);
    }
    check("policy: the block was seen (turn 3 culture)", seen.policy);
    check("pantheon: the block was seen (turn 3 faith)", seen.pantheon);
    check("religion: the prophet was given (turn 5)", seen.religion);
    check(`city screen after the turn change: ${cityOk}/${cityTries} turns took the orders`, cityTries > 0 && cityOk === cityTries);
    const st = await stats(b.page);
    check("ui.lua_errors == 0 at the end", st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
    check("replica.diverged == 0 at the end", st.replica?.diverged === 0, `${st.replica?.diverged}`);
    num.final = { ui: st.ui, replica: st.replica };
  } catch (e) {
    await shot("failure");
    check("no scenario error", false, e.message);
    result.error = e.stack ?? String(e);
  }
  // (a bundle built with --single has no pkg-mt: the page's probe for it is a 404, not an error of the game)
  const errs = b.errors.filter((e) => !/pkg-mt\/app_web\.js/.test(e));
  result.consoleErrors.push(...errs);
  await b.close();
  check("no console errors", errs.length === 0, `${errs.length} error(s)${errs[0] ? `: ${errs[0]}` : ""}`);
  result.pass = result.checks.every((c) => c.pass);
  return result;
}

/** Opens the capital's city screen at once (the city banner's own call), clicks another production in the list and a tile to work, and
 *  waits for both to come back from the server's core (the replica follows the journal). */
async function cityRightAfterTurn(ui, woe, lua1, note) {
  const info = async () => lua1(`local p = Players[Game.GetActivePlayer()] local c = p:GetCapitalCity()
    print(TAG .. tostring(c and c:GetProductionNameKey()) .. "|" .. tostring(c and c:GetOrderQueueLength()) .. "|" .. tostring(p:IsTurnActive()) .. "|" .. tostring(Game.IsNetworkMultiPlayer()) .. "|" .. tostring(UI.IsCityScreenUp()))`);
  const pre = await info();
  await ui.lua(`local c = Players[Game.GetActivePlayer()]:GetCapitalCity() if c then UI.DoSelectCityAtPlot(c:Plot()) end`);
  const up = await woe.until(() => ui.cityScreenShown(), 15000, "the city screen").catch(() => false);
  if (!up) return { ok: false, why: "the city screen did not open", before: pre };
  // the clicks of earlier turns pile up in the queue (EUI's click appends): back to one item, as the queue's X buttons do
  if (Number(pre.split("|")[1]) > 2) {
    await ui.lua(`local c = UI.GetHeadSelectedCity() if c then for i = c:GetOrderQueueLength() - 1, 1, -1 do Game.SelectedCitiesGameNetMessage(GameMessageTypes.GAMEMESSAGE_POP_ORDER, i) end end`);
    await ui.sleep(3000);
  }
  const before = await info();
  const [prodKey] = before.split("|");
  // another unit or building than the one being built
  const rows = (await ui.dump("/CityView/QueueBackground/Queue/SelectionScrollPanel/SelectionStacks")).filter((r) => r.cls === "Label" && r.path.endsWith("/Button/Name") && r.text.trim());
  const panel = await ui.find("/CityView/QueueBackground/Queue/SelectionScrollPanel", (x) => x.path.endsWith("/SelectionScrollPanel"));
  const inside = (r) => !panel || (r.y >= panel.y && r.y + r.h <= panel.y + panel.h);
  const cur = await lua1(`print(TAG .. Locale.ConvertTextKey("${prodKey}"))`);
  const units = rows.filter(inside).filter((r) => r.text.trim() !== cur && /UnitButtonStack/.test(r.path));
  const pick = units.find((r) => /^(Scout|Warrior|Worker)$/.test(r.text.trim())) ?? units[0];
  if (!pick) return { ok: false, why: "no other production in the list", before, cur };
  const probe = await lua1(`local c = UI.GetHeadSelectedCity() print(TAG .. "head=" .. tostring(c and c:GetID()) .. " viewing=" .. tostring(UI.IsCityScreenViewingMode and UI.IsCityScreenViewingMode()) .. " up=" .. tostring(UI.IsCityScreenUp()))`);
  const sent0 = await ui.sleep(0).then(() => globalThis.__chcPage?.evaluate(() => window.__civ5stats?.replica?.net_sent));
  note(`city: before the click ${probe}, pick ${pick.path.slice(-50)} at ${pick.x},${pick.y} ${pick.w}x${pick.h}`);
  await ui.click(pick);
  await ui.sleep(1500);
  const sent1 = await globalThis.__chcPage?.evaluate(() => window.__civ5stats?.replica?.net_sent);
  note(`city: replica.net_sent ${sent0} -> ${sent1} after the click`);
  await globalThis.__chcShot?.(`city-click-${Date.now() % 100000}`);
  const t0 = Date.now();
  let after = before;
  while (Date.now() - t0 < 12000) {
    await ui.sleep(700);
    after = await info();
    if (after.split("|").slice(0, 2).join("|") !== before.split("|").slice(0, 2).join("|")) break;
  }
  // EUI's click puts the item in front of the queue (production and queue length change); either shows the order arrived
  const prodOk = after.split("|").slice(0, 2).join("|") !== before.split("|").slice(0, 2).join("|");
  note(`city: ${cur} -> ${pick.text.trim()}: ${prodOk ? "taken" : "NOT taken"} in ${Date.now() - t0} ms (${before} -> ${after})`);
  for (let i = 0; i < 3 && (await ui.cityScreenShown()); i++) { await ui.closeCityScreen(); await ui.sleep(2000); }
  return { ok: prodOk, before, after, picked: pick.text.trim() };
}
