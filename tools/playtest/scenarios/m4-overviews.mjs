// `m4-overviews` (WO-F, milestone M4): K16 culture / great works / archaeology and K17 overviews of the original UI (EUI,
// the default game screen) on the real core: every window is opened from its original entry point (the top panel's gold / happiness
// / tourism, the culture button and the "additional information" pulldown of the diplomacy corner), shot, closed.
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8830 --scenario m4-overviews [--gfx webgl2] [--headed]
//   late-huge save (turn 150; the server runs on a copy of its state dir) instead of a fresh game: add --late-nick late1 --late-token <out-dir>\late1.token
// Controls are found through the screen dump and clicked in their rectangle (`lib/ui-driver.mjs`), not by pixels.
// The great work splash and the archaeology choice have no button: the exe raises them on an event (a great work made, a dig
// finished); they are raised here with `UI.AddPopup` like the game does. Checks: every window shows (its controls are in the
// dump), `ui.lua_errors == 0` after each one. Screenshots: <logs>/w-<name>.png.
import fs from "node:fs";
import { openPage, act, until, state, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { uiDriver } from "../lib/ui-driver.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function stats(page) {
  return page.evaluate(() => {
    const s = window.__civ5stats ?? {};
    return { ui: s.ui ?? null, replica: s.replica ?? null, exe: s.exe ?? null, m2: s.m2 ?? null, camera: s.camera ?? null };
  });
}

// How each window is opened (entry points of the original UI): `btn` = a control by path part (+ class) in the screen dump;
// `tip` = a corner button of the diplomacy corner found by its tooltip; `lua` = an event the exe/core raises (no button).
// `root`: a path part that is in the dump only while the window is up; `tabs`: controls of the window clicked one by one
// (each is shot); `close`: how it is closed (its Close button).
// A great work of the database that has its own picture (`GreatWorks.Image`): the splash shows that picture and the work's
// description as its caption. (The artifacts of the "other era" rows have no image: the splash then falls back to Starry Night,
// as in the original, and the caption "Figurine" under a painting looked like a wrong caption: M5 WO-I.)
const SWAP_LUA = `local gw
for row in GameInfo.GreatWorks() do
  if row.Image and row.GreatWorkClassType == "GREAT_WORK_ART" then gw = row break end
end
if gw then
  print(TAG .. gw.Type .. "|" .. Locale.ConvertTextKey(gw.Description) .. "|" .. gw.Image)
  Events.SerialEventGameMessagePopup({ Type = ButtonPopupTypes.BUTTONPOPUP_GREAT_WORK_COMPLETED_ACTIVE_PLAYER, Data1 = -1, Data2 = GameInfoTypes[gw.Type] })
else print(TAG .. "none") end`;
// The core cannot raise these two from Lua (a unit made by InitUnit has no great work; a plot's dig record has no setter):
// the great work splash gets a real great work type of the database (Data1 = -1 means "type in Data2"), the archaeology choice
// a real dig site of the map (a plot with an archaeological record) that the UI Lua hands back as the player's next dig.
const ARCH_LUA = `
local site
for i = 0, Map.GetNumPlots() - 1 do
  local p = Map.GetPlotByIndex(i)
  local e = p:GetArchaeologyArtifactEra()
  if e and e >= 0 and p:GetArchaeologyArtifactType() == GameInfoTypes.ARTIFACT_ANCIENT_RUIN then site = p break end
end
if site then
  getmetatable(Players[Game.GetActivePlayer()]).__index.GetNextDigCompletePlot = function() return site end
  Events.SerialEventGameMessagePopup({ Type = ButtonPopupTypes.BUTTONPOPUP_CHOOSE_ARCHAEOLOGY, Data1 = Game.GetActivePlayer(), Data2 = 0 })
else print("M4 no dig site on the map") end`;
const WINDOWS = [
  { name: "culture-overview", btn: ["TopPanelDiploStack/TourismString", "TextButton"], root: "/CultureOverview/", tabs: ["TabButtonSwapGreatWorks", "TabButtonCultureVictory", "TabButtonYourCulture"] },
  { name: "economic-overview", btn: ["TopPanelInfoStack/GoldPerTurn", "TextButton"], root: "/EconomicOverview/", tabs: ["HappinessButton", "GeneralInfoButton"] },
  { name: "happiness", btn: ["TopPanelInfoStack/HappinessString", "TextButton"], root: "/EconomicOverview/", tabs: [] },
  { name: "diplomacy-overview", btn: ["DiploCornerStack/DiploButton", "Button"], root: "/DiploOverview/", tabs: ["DealsButton", "GlobalPoliticsButton", "RelationsButton"] },
  { name: "military-overview", tip: /^tip:Military Overview/, root: "/MilitaryOverview/", tabs: [] },
  { name: "victory-progress", tip: /^tip:Victory Progress/, root: "/VictoryProgress/", tabs: [] },
  { name: "demographics", tip: /^tip:Demographics/, root: "/Demographics/", tabs: [] },
  { name: "notification-log", tip: /^tip:Notification Log/, root: "/NotificationLogPopup/", tabs: [] },
  { name: "whos-winning", lua: `Events.SerialEventGameMessagePopup({ Type = ButtonPopupTypes.BUTTONPOPUP_WHOS_WINNING })`, root: "/WhosWinningPopup/", tabs: [] },
  { name: "great-work", lua: SWAP_LUA, root: "/GreatWorkSplash/", tabs: [] },
  { name: "archaeology", lua: ARCH_LUA, root: "/ChooseArchaeology/", tabs: [] },
];

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "m4-overviews", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const late = !!(o.lateNick && o.lateToken);
  const base = o.attach ?? ctx.server?.url;
  if (!base) { check("options", false, "m4-overviews needs --attach <url of the real-core server> (or --late-url/--late-nick/--late-token)"); return result; }
  const b = await openPage({ headed: o.headed, logDir: o.logDir, viewport: { width: 1600, height: 900 } });
  const shot = async (name) => { try { await b.page.screenshot({ path: `${o.logDir}/${name}.png`, timeout: 120000 }); } catch { /* gone */ } };
  let error = null;
  try {
    if (late) {
      const token = fs.readFileSync(o.lateToken, "utf8").trim();
      await b.page.addInitScript(([nick, tok]) => {
        try { localStorage.setItem("civ5.nick", nick); localStorage.setItem(`civ5.token.${nick}`, tok); } catch { /* blocked */ }
      }, [o.lateNick, token]);
      await b.page.goto(appUrl(base, o, { nick: o.lateNick, stats: "1", ...(process.env.M4_NOMT ? { mt: "0" } : {}) }));
      await until(b.page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 240000, "the late game");
    } else {
      for (let attempt = 0; ; attempt++) {
        const nick = `m4f${(Date.now() + attempt) % 1000000}`;
        await b.page.goto(appUrl(base, o, { nick, autostart: "1", ai: "2", size: "small", seed: "7", mode: "simultaneous", stats: "1", ...(process.env.M4_NOMT ? { mt: "0" } : {}) }));
        try {
          const s = await until(b.page, (x) => x.screen === "login" || x.screen === "play", 90000, "sign-in window or the game");
          if (s.screen === "login") await act(b.page, { kind: "login", nick, password: DEV_PASSWORD });
          await until(b.page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 60000, "the game");
          break;
        } catch (e) { if (attempt >= 2) throw e; }
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
    st = await stats(b.page);
    check("the UI is up", st.m2?.phase === 2 && st.replica?.live === 1, JSON.stringify(st.m2));
    check("ui.lua_errors == 0 after the start", st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
    result.numbers.turn = (await state(b.page))?.turn;
    const ui = uiDriver(b);
    if (process.env.M4_DUMP) {
      const rows = await ui.dump(process.env.M4_DUMP === "1" ? "" : process.env.M4_DUMP);
      fs.writeFileSync(`${o.logDir}/dump.txt`, rows.map((r) => `${r.path}\t${r.cls}\t${r.x} ${r.y} ${r.w} ${r.h}\t${r.text}`).join("\n"));
    }
    const only = process.env.M4_ONLY ? process.env.M4_ONLY.split(",") : null;
    const closeIt = async (w) => {
      for (let k = 0; k < 3 && (await ui.dump(w.root)).length; k++) {
        const r = (await ui.dump(w.root)).find((x) => /CloseButton$/.test(x.path) && x.cls.includes("Button"));
        if (r) await ui.click(r); else await b.page.keyboard.press("Escape");
        await sleep(1500);
      }
      return (await ui.dump(w.root)).length === 0;
    };
    // (a fresh short game has no dig site on the map: the archaeology choice is only shown on the late save)
    for (const w of WINDOWS.filter((x) => (!only || only.includes(x.name)) && (late || x.name !== "archaeology"))) {
      let note = "";
      try {
        if (w.btn) {
          const r = await ui.find(w.btn[0], (x) => x.cls === w.btn[1]);
          if (!r) throw new Error(`no ${w.btn[0]} on the screen`);
          await ui.click(r);
        } else if (w.tip) {
          const r = (await ui.dump("DiploCornerStack")).find((x) => x.cls === "Button" && w.tip.test(x.text));
          if (!r) throw new Error(`no corner button ${w.tip}`);
          await ui.click(r);
        } else {
          w.out = await ui.lua(w.lua);
        }
        await sleep(3500);
        const shown = (await ui.dump(w.root)).length;
        await shot(`w-${w.name}`);
        check(`${w.name}: opens (${shown} controls)`, shown > 5, w.root);
        if (w.name === "great-work") {
          // the caption is the description of the work that was raised (and that work has its own picture)
          const [type, text, image] = (w.out?.[0] ?? "none").split("|");
          const cap = (await ui.dump(w.root)).find((x) => /\/LowerCaption$/.test(x.path));
          result.numbers.greatWork = { type, text, image, caption: cap?.text ?? null };
          check("great-work: the caption is the description of the work", !!cap && type !== "none" && cap.text.trim() === text, `${type}: "${cap?.text}" vs "${text}"`);
          check("great-work: the work has its own picture (not the fallback)", !!image && !/Starry/i.test(image), image ?? "");
        }
        for (const t of w.tabs) {
          const r = (await ui.dump(w.root)).find((x) => x.path.endsWith(t));
          if (!r) { check(`${w.name}: tab ${t}`, false, "not found"); continue; }
          await ui.click(r);
          await sleep(2500);
          await shot(`w-${w.name}-${t}`);
          st = await stats(b.page);
          check(`${w.name}: tab ${t}: ui.lua_errors == 0`, st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
        }
        note = `closed=${await closeIt(w)}`;
        check(`${w.name}: closes`, note === "closed=true", note);
      } catch (e) {
        check(`${w.name}: opens`, false, e.message);
        await shot(`w-${w.name}-fail`);
        await closeIt(w).catch(() => {});
      }
      st = await stats(b.page);
      check(`${w.name}: ui.lua_errors == 0`, st.ui?.lua_errors === 0, `${st.ui?.lua_errors} ${note}`);
    }
  } catch (e) {
    error = e;
    await shot("failure");
  }
  result.consoleErrors.push(...b.errors);
  await b.close();
  check("no console errors", result.consoleErrors.length === 0, `${result.consoleErrors.length} error(s)${result.consoleErrors[0] ? `: ${result.consoleErrors[0]}` : ""}`);
  if (error) { check("no scenario error", false, error.message); result.error = error.stack ?? String(error); }
  result.pass = result.checks.every((c) => c.pass);
  result.durationMs = Date.now() - started;
  return result;
}
