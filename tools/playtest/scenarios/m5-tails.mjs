// `m5-tails` (M5 WO-L): the tails of M3 on the original UI (`?ui=original`) and a real-core game, by the controls of the screen
// dump (`lib/ui-driver.mjs`), not by pixels.
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8895 --scenario m5-tails [--gfx webgl2] [--headed]
// The server must run with the DEV game data of `tools/playtest/m5/make-dev-gamedata.ps1` (turn 3: +30 faith, one faith great person to claim, +60 culture for the
// human) and the page flag `manual=1` (set here) keeps the human's choices (research, policy) manual.
// Flow: enter -> found a city -> turns to 6 (policy adopted by its own message) -> the windows the exe raises on an event, each opened
// like the game does (`Events.SerialEventGameMessagePopup`) and acted on with the mouse:
//   religion overview  (tabs "World Religions" and "Beliefs", Close)        BUTTONPOPUP_RELIGION_OVERVIEW
//   great person       (the portrait must be drawn, Close)                  BUTTONPOPUP_GREAT_PERSON_REWARD
//   tech award         (the tech's text, Continue)                          BUTTONPOPUP_TECH_AWARD
//   ideology           (Freedom, Yes: Network.SendIdeologyChoice)           BUTTONPOPUP_CHOOSE_IDEOLOGY
//   faith purchase     (raised by END TURN: an enabled great person, OK: SendFaithGreatPersonChoice)  BUTTONPOPUP_CHOOSE_FAITH_GREAT_PERSON
// then the city screen from a camera far away (the camera must take the city: focus on it, zoom level 17.1, the player's zoom back
// on leaving), the hex highlights (outline styles build geometry), and the notification texts (none with unresolved grammar).
// Checks: every window shows, the action is sent (`replica.net_sent` grows), `ui.lua_errors == 0` after each, `replica.diverged == 0`.
// Screenshots: <logs>/t-*.png.
import { openPage, act, until, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { uiDriver } from "../lib/ui-driver.mjs";
import { turnDriver } from "../lib/turns-woe.mjs";
import { decode } from "../lib/png.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const stats = (page) =>
  page.evaluate(() => {
    const s = window.__civ5stats ?? {};
    return { ui: s.ui ?? null, replica: s.replica ?? null, exe: s.exe ?? null, m2: s.m2 ?? null, camera: s.camera ?? null, c: s.counters ?? {} };
  });

/** The share of bright pixels (max channel > 110) inside a rectangle of a PNG (screen pixels): a drawn portrait is, a dark popup is not. */
function brightShare(png, x, y, w, h) {
  const { width, height, channels, px } = decode(png);
  let bright = 0;
  let n = 0;
  for (let j = Math.max(0, y); j < Math.min(height, y + h); j++) {
    for (let i = Math.max(0, x); i < Math.min(width, x + w); i++) {
      const o = (j * width + i) * channels;
      if (Math.max(px[o], px[o + 1], px[o + 2]) > 110) bright++;
      n++;
    }
  }
  return n ? bright / n : 0;
}

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "m5-tails", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const base = o.attach ?? ctx.server?.url;
  if (!base) {
    check("options", false, "m5-tails needs --attach <url of the real-core server with the dev game data>");
    return result;
  }
  const b = await openPage({ headed: o.headed, logDir: o.logDir, viewport: { width: 1600, height: 900 } });
  const shot = async (name) => {
    try {
      return await b.page.screenshot({ path: `${o.logDir}/t-${name}.png`, timeout: 120000 });
    } catch {
      return null;
    }
  };
  let error = null;
  try {
    for (let attempt = 0; ; attempt++) {
      const nick = `m5t${(Date.now() + attempt) % 1000000}`;
      await b.page.goto(appUrl(base, o, { nick, autostart: "1", ai: "2", size: "small", seed: "7", mode: "simultaneous", ui: "original", manual: "1", stats: "1" }));
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
    let st;
    for (;;) {
      st = await stats(b.page);
      if (st.m2?.phase === 2) break;
      if (st.m2?.phase === -1) throw new Error("M2 failed (phase -1)");
      if (Date.now() - upAt > 420000) throw new Error(`M2 is not up: ${JSON.stringify(st.m2)}`);
      await sleep(500);
    }
    await sleep(4000);
    const ui = uiDriver(b);
    // The dev game data's 30 faith make END TURN ask for a pantheon (EndTurnBlockingTypes 13): the popup the button opens, the first
    // belief, "Yes".
    const foundPantheon = async () => {
      await ui.clickEndTurn();
      await sleep(3500);
      await shot("pantheon-popup");
      const rows = await ui.dump("/ChoosePantheon");
      const belief = rows.find((r) => r.cls === "Button" && !r.disabled && /ItemStack/.test(r.path)) ?? rows.find((r) => r.cls === "Button" && !r.disabled && !/Close|Back/i.test(r.path));
      if (!belief) return "pantheon: no belief button";
      await ui.click(belief);
      await sleep(2000);
      const yes = (await ui.dump("ChoosePantheon")).find((r) => /\/Yes$/.test(r.path) && r.cls.includes("Button"));
      if (yes) await ui.click(yes);
      await sleep(5000);
      return `pantheon: belief ${belief.path.split("/").slice(-2).join("/")}, confirm ${yes ? "yes" : "none"}`;
    };
    // The dev game data's faith great person to claim makes END TURN ask for it (type 18): the window the button opens, the first
    // enabled great person, OK; the original sends the choice with `Network.SendFaithGreatPersonChoice`.
    const faith = { done: false, note: "not raised" };
    const faithPurchase = async () => {
      const sent0 = (await stats(b.page)).replica?.net_sent ?? 0;
      await ui.clickEndTurn();
      await sleep(3500);
      await shot("05-faith-purchase");
      const fp = await ui.dump("/ChooseFaithGreatPerson");
      check("faith purchase: opens with the great people", fp.filter((x) => /\/Button\/Name$/.test(x.path)).length >= 5, `${fp.length} controls`);
      const items = fp.filter((x) => /ItemStack\/\/Button$/.test(x.path) && x.cls === "Button" && !x.disabled);
      result.numbers.faithItemsEnabled = items.length;
      if (!items.length) {
        faith.note = "no enabled great person";
        await shot("05-faith-purchase-none");
        return "faith purchase: no enabled great person";
      }
      await ui.click(items[0]);
      await sleep(1500);
      const ok = (await ui.dump("/ChooseFaithGreatPerson")).find((x) => /ConfirmButton$/.test(x.path) && x.cls.includes("Button"));
      check("faith purchase: OK is enabled after a pick", !!ok && !ok.disabled, ok ? `disabled=${ok.disabled}` : "no button");
      if (ok && !ok.disabled) await ui.click(ok);
      await sleep(4000);
      const s = await lerr("faith purchase");
      check("faith purchase: the choice was sent (SendFaithGreatPersonChoice)", (s.replica?.net_sent ?? 0) > sent0, `net_sent ${sent0} -> ${s.replica?.net_sent}`);
      check("faith purchase: the window closed", (await ui.dump("/ChooseFaithGreatPerson")).length === 0);
      faith.done = true;
      faith.note = `net_sent ${sent0} -> ${s.replica?.net_sent}`;
      return "faith purchase: done";
    };
    const td = turnDriver(b, ui, shot, { 13: foundPantheon, 18: faithPurchase });
    const lerr = async (what) => {
      const s = await stats(b.page);
      check(`${what}: ui.lua_errors == 0`, s.ui?.lua_errors === 0, `${s.ui?.lua_errors}`);
      return s;
    };
    await lerr("after the start");

    // A city, and the turns until the dev grant (turn 3) and a policy are in.
    await ui.clickEndTurn();
    await sleep(9000);
    const citiesBefore = st.exe?.entity_cities ?? 0;
    check("the settler has a found-city button", !!(await ui.unitAction(/FOUND/)));
    await sleep(12000);
    st = await stats(b.page);
    check("a city was founded", (st.exe?.entity_cities ?? 0) === citiesBefore + 1, `cities ${citiesBefore} -> ${st.exe?.entity_cities}`);
    // The new city's screen opens by itself (the exe does that for the first city): a production is picked there, then it is closed.
    if (await ui.cityScreenShown()) {
      await shot("00-city-founded");
      result.numbers.firstProduction = await ui.pickProduction();
      await sleep(4000);
      if (await ui.cityScreenShown()) await ui.closeCityScreen();
      await sleep(2500);
    }
    await td.toTurn(6);
    result.numbers.handledBlocks = td.handled.slice(-8);
    await shot("00-turn6");
    const pol = await ui.lua(`
      local p = Players[Game.GetActivePlayer()]
      local n = p:GetNumPolicies()
      -- a Tradition policy opens the tree the great engineer of the faith popup asks for
      local pick
      for t in GameInfo.Policies() do if t.PolicyBranchType == "POLICY_BRANCH_TRADITION" and p:CanAdoptPolicy(t.ID) and not pick then pick = t end end
      if pick and n == 0 then Network.SendUpdatePolicies(pick.ID, true, true) end
      print(TAG .. n .. " " .. tostring(pick and pick.Type) .. " faith=" .. p:GetFaith() .. " culture=" .. p:GetJONSCulture())`);
    result.numbers.policy = pol[0];
    await sleep(6000);

    // ---- the religion overview
    await ui.lua(`Events.SerialEventGameMessagePopup({ Type = ButtonPopupTypes.BUTTONPOPUP_RELIGION_OVERVIEW, Data1 = 0 })`);
    await sleep(3500);
    await shot("01-religion-overview");
    check("religion overview: opens", (await ui.dump("/ReligionOverview/")).length > 20);
    for (const tab of ["TabButtonWorldReligions", "TabButtonBeliefs", "TabButtonYourReligion"]) {
      const r = (await ui.dump("/ReligionOverview/")).find((x) => x.path.endsWith(tab));
      if (!r) {
        check(`religion overview: tab ${tab}`, false, "not found");
        continue;
      }
      await ui.click(r);
      await sleep(2500);
      await shot(`01-religion-${tab}`);
      await lerr(`religion overview: ${tab}`);
    }
    const rClose = (await ui.dump("/ReligionOverview/")).find((x) => /CloseButton$/.test(x.path) && x.cls.includes("Button"));
    if (rClose) await ui.click(rClose);
    await sleep(2000);
    check("religion overview: closes", (await ui.dump("/ReligionOverview/")).length === 0);

    // ---- the great person born (the portrait)
    await ui.lua(`local city for c in Players[Game.GetActivePlayer()]:Cities() do city = c break end
      Events.SerialEventGameMessagePopup({ Type = ButtonPopupTypes.BUTTONPOPUP_GREAT_PERSON_REWARD, Data1 = GameInfoTypes.UNIT_SCIENTIST, Data2 = city:GetID() })`);
    await sleep(5000);
    const gpPng = await shot("02-great-person");
    const gp = await ui.dump("/GreatPersonRewardPopup/");
    check("great person: opens", gp.length > 8, `${gp.length} controls`);
    const portrait = gp.find((x) => /\/Portrait$/.test(x.path));
    if (portrait && gpPng) {
      const s = await ui.dump("/GreatPersonRewardPopup/").then(() => stats(b.page)).then((x) => x.camera?.ui_scale ?? 1);
      const share = brightShare(gpPng, Math.round(portrait.x * s) + 40, Math.round(portrait.y * s) + 40, Math.round(portrait.w * s) - 80, Math.round(portrait.h * s) - 80);
      result.numbers.portraitBrightShare = Math.round(share * 1000) / 1000;
      check("great person: the portrait is drawn (bright share of its rectangle >= 0.15)", share >= 0.15, `${share.toFixed(3)}`);
    } else check("great person: the portrait control", false, "not in the dump");
    await lerr("great person");
    const gpClose = gp.find((x) => /CloseButton$/.test(x.path) && x.cls.includes("Button"));
    if (gpClose) await ui.click(gpClose);
    await sleep(1500);
    check("great person: closes", (await ui.dump("/GreatPersonRewardPopup/")).length === 0);

    // ---- the tech award
    await ui.lua(`Events.SerialEventGameMessagePopup({ Type = ButtonPopupTypes.BUTTONPOPUP_TECH_AWARD, Data1 = Game.GetActivePlayer(), Data2 = 0, Data3 = GameInfoTypes.TECH_AGRICULTURE })`);
    await sleep(3500);
    await shot("03-tech-award");
    const ta = await ui.dump("/TechAwardPopup/");
    check("tech award: opens with the tech's name", ta.some((x) => x.cls === "Label" && /Agriculture/i.test(x.text)), `${ta.length} controls`);
    const taBtn = ta.find((x) => /ContinueButton$/.test(x.path) && x.cls.includes("Button"));
    if (taBtn) await ui.click(taBtn);
    await sleep(2000);
    check("tech award: Continue closes it", (await ui.dump("/TechAwardPopup/")).length === 0);
    await lerr("tech award");

    // ---- the ideology
    let sent = (await stats(b.page)).replica?.net_sent ?? 0;
    await ui.lua(`Events.SerialEventGameMessagePopup({ Type = ButtonPopupTypes.BUTTONPOPUP_CHOOSE_IDEOLOGY, Data1 = Game.GetActivePlayer() })`);
    await sleep(3500);
    await shot("04-ideology");
    const fr = (await ui.dump("/ChooseIdeology/")).find((x) => x.cls === "Label" && /^Freedom$/.test(x.text.trim()));
    check("ideology: the list shows Freedom", !!fr);
    if (fr) {
      await ui.click(fr);
      await sleep(2000);
      await shot("04-ideology-confirm");
      const yes = (await ui.dump("/ChooseIdeology/")).find((x) => /ConfirmYes$/.test(x.path) && x.cls.includes("Button"));
      check("ideology: the confirmation asks", !!yes);
      if (yes) await ui.click(yes);
      await sleep(4000);
    }
    st = await lerr("ideology");
    check("ideology: the choice was sent", (st.replica?.net_sent ?? 0) > sent, `net_sent ${sent} -> ${st.replica?.net_sent}`);
    check("ideology: the window closed", (await ui.dump("/ChooseIdeology/")).length === 0);

    // ---- the faith purchase of a great person: raised by END TURN at turn 4 (the dev data's one faith great person to claim)
    check("faith purchase: END TURN asked for it (blocking type 18) and the window was driven", faith.done, faith.note);
    // ---- the city screen takes the camera
    const camNow = async () => {
      await sleep(1600); // the stats refresh once a second
      return (await stats(b.page)).camera;
    };
    await ui.lua(`local city for c in Players[Game.GetActivePlayer()]:Cities() do city = c break end UI.LookAt(city:Plot())`);
    const atCity = await camNow();
    await ui.lua(`local city for c in Players[Game.GetActivePlayer()]:Cities() do city = c break end
      local w, h = Map.GetGridSize()
      local fx, fy = city:GetX() + 7, city:GetY() + 5
      if fx >= w then fx = city:GetX() - 7 end
      if fy >= h then fy = city:GetY() - 5 end
      UI.LookAt(Map.GetPlot(fx, fy))`);
    const away = await camNow();
    const dist = (a, c) => Math.hypot(a.focus_x - c.focus_x, a.focus_z - c.focus_z);
    check("the camera was moved away from the city", dist(atCity, away) > 50, `${dist(atCity, away).toFixed(1)}`);
    // The "choose production" block of END TURN opens the city screen (no look-at of the banner): the camera must go to the city.
    await ui.lua(`local city for c in Players[Game.GetActivePlayer()]:Cities() do city = c break end UI.SelectCity(city) UI.SetCityScreenUp(true)`);
    await sleep(2500);
    const inCity = await camNow();
    await shot("06-city-screen");
    check("city screen: the camera is on the city", dist(atCity, inCity) < 0.2 * dist(atCity, away), `${dist(atCity, inCity).toFixed(1)} from the city (away was ${dist(atCity, away).toFixed(1)})`);
    check("city screen: the zoom level is the city view's (17.1)", Math.abs(inCity.level - 17.1) < 0.05, `${inCity.level}`);
    check("city screen: the city screen is up", await ui.cityScreenShown());
    // the hex highlights of the city window: owned tiles are outlined (the Lua passes `nil` colours and the Outline styles)
    st = await stats(b.page);
    const idxBefore = st.c["overlay.style_indices"] ?? 0;
    result.numbers.cityHighlightIndices = idxBefore;
    check("city screen: the hex highlights (fills and outlines) built geometry", idxBefore > 200, `overlay.style_indices ${idxBefore}`);
    const hl = await ui.lua(`
      local city for c in Players[Game.GetActivePlayer()]:Cities() do city = c break end
      Events.ClearHexHighlights()
      local x, y = city:GetX() + 1, city:GetY()
      Events.SerialEventHexHighlight(ToHexFromGrid({ x = x, y = y }), true, nil, "WorkedOutline")
      print(TAG .. "outline set")`);
    await sleep(2500);
    st = await stats(b.page);
    check("a single outline style builds geometry", hl[0] === "outline set" && (st.c["overlay.style_indices"] ?? 0) > 0, `overlay.style_indices ${st.c["overlay.style_indices"]}`);
    await shot("06-outline-only");
    await ui.lua(`Events.ClearHexHighlights()`);
    await ui.closeCityScreen();
    await sleep(3000);
    check("city screen: closes", !(await ui.cityScreenShown()));
    const back = await camNow();
    check("city screen: the player's zoom level is back", Math.abs(back.level - atCity.level) < 0.05, `${back.level} (was ${atCity.level})`);
    await lerr("city screen");

    // ---- the notification texts: no unresolved grammar in what the core composed
    const nt = await ui.lua(`
      local p = Players[Game.GetActivePlayer()]
      local n, bad, wor = 0, 0, 0
      for i = 0, 400 do
        local s = p:GetNotificationStr(i)
        local m = p:GetNotificationSummaryStr(i)
        if (s == nil or s == "") and (m == nil or m == "") then break end
        n = n + 1
        if (s or ""):find("started worshipping a pantheon") then wor = wor + 1 print(TAG .. "WOR " .. s) end
        for _, t in ipairs({ s or "", m or "" }) do
          if t:find("{@") or t:find(": plural") or t:find(":plural") or t:find("{%d_") then bad = bad + 1 print(TAG .. "BAD " .. t) end
        end
      end
      print(TAG .. "wor=" .. wor) print(TAG .. "n=" .. n .. " bad=" .. bad)`);
    result.numbers.notifications = nt.slice(-4);
    const sum = nt.find((l) => /^n=/.test(l)) ?? "n=? bad=?";
    check("notification texts have no unresolved grammar ({@..}, plural)", /bad=0$/.test(sum), nt.slice(-3).join(" | "));
    const wor = nt.filter((l) => l.startsWith("WOR "));
    check("a notification the server core composed from the plural grammar arrives resolved (dev data, turn 2)", wor.some((l) => /Testland has started worshipping/.test(l)), wor.join(" | ") || "none seen");
    await lerr("notifications");
    st = await stats(b.page);
    check("replica.diverged == 0", (st.replica?.diverged ?? 0) === 0, `${st.replica?.diverged}`);
  } catch (e) {
    error = e;
    await shot("failure");
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
