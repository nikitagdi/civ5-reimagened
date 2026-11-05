// `m4-menus-endgame` (WO-G, milestone M4): the game menu and the options (K20), the end of the game (K19), of the original UI.
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8840 --scenario m4-menus-endgame [--gfx webgl2] [--headed]
// Needs the real-core server with the DEV game data of `tools/playtest/m4/make-dev-gamedata.ps1` (the human's team wins
// a time victory on turn 3) and, to look for the save the host makes, `M4_STATE=<state dir of the server>`.
// Part 1 (game 1): Esc opens the game menu, Options (change a checkbox and the volume, OK), the options stick (the
//   options store of the page, the sound gain), Save game (host only: a save appears in the server's state dir), Exit
//   to the main menu -> confirm -> back in our lobby.
// Part 2 (game 2): end turns until the DEV game data gives the human the victory -> the end-game screen (tabs).
// Controls are found by path / class / text in the screen dump and clicked in their computed rectangle (lib/ui-driver).
// Checks: ui.lua_errors == 0 throughout. Screenshots in <out>/logs/m4-menus-endgame/.
import { readdirSync, existsSync, statSync, writeFileSync } from "node:fs";
import { openPage, act, until, state, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { uiDriver, BLOCK } from "../lib/ui-driver.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function stats(page) {
  return page.evaluate(() => {
    const s = window.__civ5stats ?? {};
    return { ui: s.ui ?? null, replica: s.replica ?? null, exe: s.exe ?? null, m2: s.m2 ?? null, camera: s.camera ?? null, sound: s.sound ?? null, net: s.net ?? null };
  });
}

/** Files of the state dir of the server below `games/` (the saves are there), for the "a save appeared" check. */
function saveFiles(stateDir) {
  const out = [];
  const walk = (d) => {
    for (const n of readdirSync(d)) {
      const p = `${d}/${n}`;
      const s = statSync(p);
      if (s.isDirectory()) walk(p);
      else if (/\.c5sv$/i.test(n)) out.push(p);
    }
  };
  if (existsSync(`${stateDir}/games`)) walk(`${stateDir}/games`);
  return out;
}

async function openGame(b, o, base, tag) {
  for (let attempt = 0; ; attempt++) {
    const nick = `m4${tag}${(Date.now() + attempt) % 1000000}`;
    await b.page.goto(appUrl(base, o, { nick, autostart: "1", ai: "2", size: "small", seed: "7", mode: "simultaneous", stats: "1", ...(process.env.M3_NOMT ? { mt: "0" } : {}) }));
    try {
      const s = await until(b.page, (x) => x.screen === "login" || x.screen === "play", 90000, "sign-in window or the game");
      if (s.screen === "login") await act(b.page, { kind: "login", nick, password: DEV_PASSWORD });
      await until(b.page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 60000, "the game");
      break;
    } catch (e) { if (attempt >= 2) throw e; }
  }
  const upAt = Date.now();
  for (;;) {
    const st = await stats(b.page);
    if (st.m2?.phase === 2) break;
    if (st.m2?.phase === -1) throw new Error("M2 failed (phase -1)");
    if (Date.now() - upAt > 900000) throw new Error(`M2 is not up: ${JSON.stringify(st.m2)}`);
    await sleep(500);
  }
  await sleep(4000);
}

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "m4-menus-endgame", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const base = o.attach ?? ctx.server?.url;
  if (!base) { check("options", false, "m4-menus-endgame needs --attach <url of the real-core server>"); return result; }
  const b = await openPage({ headed: o.headed, logDir: o.logDir, viewport: { width: 1600, height: 900 } });
  const shot = async (name) => { try { await b.page.screenshot({ path: `${o.logDir}/${name}.png` }); } catch { /* gone */ } };
  let error = null;
  try {
    await openGame(b, o, base, "a");
    let st = await stats(b.page);
    check("the UI is up", st.m2?.phase === 2 && st.replica?.live === 1, JSON.stringify(st.m2));
    const ui = uiDriver(b);
    const noErrors = async (what) => {
      const s = await stats(b.page);
      check(`ui.lua_errors == 0 ${what}`, s.ui?.lua_errors === 0, `${s.ui?.lua_errors}`);
    };
    await noErrors("after the start");
    const dumpTo = async (name, filter = "#top") => {
      const rows = await ui.dump(filter);
      writeFileSync(`${o.logDir}/${name}.txt`, rows.map((r) => `${r.path}\t${r.cls}\t${r.x} ${r.y} ${r.w} ${r.h}\t${r.disabled ? "D" : ""}${r.void1 !== null ? ` v${r.void1}` : ""}\t${r.text}`).join("\n"));
      return rows;
    };
    /** The first shown control whose path matches `re`; a missing one is an error naming it. */
    const need = async (re, what, filter = "") => {
      const r = (await ui.dump(filter)).find((x) => re.test(x.path));
      if (!r) throw new Error(`${what}: no control matches ${re}`);
      return r;
    };
    const clickAt = async (row, fx = 0.5) => {
      const sc = (await b.page.evaluate(() => window.__civ5stats?.camera?.ui_scale)) ?? 1;
      const x = Math.round((row.x + row.w * fx) * sc), y = Math.round((row.y + row.h / 2) * sc);
      await b.page.mouse.move(x, y);
      await b.page.mouse.click(x, y);
    };
    const press = async (re, what, wait = 2500, filter = "") => { await ui.click(await need(re, what, filter)); await sleep(wait); };
    const stored = () => b.page.evaluate(() => {
      const out = {};
      try { for (let i = 0; i < localStorage.length; i++) { const k = localStorage.key(i); if (k.startsWith("civ5.options.")) out[k] = localStorage.getItem(k); } } catch { /* no storage */ }
      return out;
    });
    const gain = async () => (await b.page.evaluate(() => { const s = window.__civ5stats ?? {}; return s.audio?.volume ?? s.sound?.volume ?? s.ui_sound?.volume ?? null; }));
    const stateDir = process.env.M4_STATE;
    if (!stateDir) throw new Error("set M4_STATE to the state dir of the server on port 8840 (the saves are read from it)");

    // ---- Part 1: the game menu, the options, the save, exit to the lobby ----
    const savesBefore = saveFiles(stateDir).length;
    const optsBefore = await stored();
    await b.page.keyboard.press("Escape");
    await sleep(3000);
    await shot("e1-esc");
    const menu = await dumpTo("dump-esc");
    check("Esc opens the game menu", menu.some((r) => /GameMenu.*SaveGameButton$/.test(r.path)) && menu.some((r) => /MainMenuButton$/.test(r.path)), `${menu.length} rows`);
    check("the host sees SAVE GAME, not LOAD / RETIRE / RESTART", !menu.some((r) => /(LoadGameButton|RetireButton|RestartGameButton)$/.test(r.path)));
    await noErrors("with the game menu open");

    await press(/GameMenu.*OptionsButton$/, "Options button", 3500);
    await shot("e2-options");
    const opt = await dumpTo("dump-options");
    check("the options menu opens", opt.some((r) => /OptionsMenu.*AcceptButton$/.test(r.path)), `${opt.length} rows`);
    await noErrors("with the options menu open");
    // The audio page: the effects volume slider drives the UI sound gain (SIMP-exe-392).
    await press(/OptionsMenu.*AudioButton$/, "Audio tab", 2500);
    await shot("e3-audio");
    const audio = await dumpTo("dump-audio");
    const slider = audio.find((r) => /EffectsVolumeSlider$/.test(r.path));
    if (!slider) check("the effects volume slider is on the audio page", false, "no EffectsVolumeSlider");
    else { await clickAt(slider, 0.2); await sleep(1500); }
    const g1 = await gain();
    result.numbers.gain_after_slider = g1;
    check("the effects volume slider sets the UI sound gain", typeof g1 === "number" && Math.abs(g1 - 0.2) < 0.12, `gain ${g1}`);
    // A checkbox on the game page.
    await press(/OptionsMenu.*GameButton$/, "Game tab", 2500);
    const game = await dumpTo("dump-game");
    await shot("e4-game");
    const boxes = game.filter((r) => /OptionsMenu.*(Check|Checkbox|CB|CheckBox)$/.test(r.path) && !r.disabled && r.w > 4);
    result.numbers.checkboxes_on_game_page = boxes.length;
    if (boxes.length) { await ui.click(boxes[0]); await sleep(1500); result.numbers.checkbox = boxes[0].path.split("/").pop(); }
    check("the game page has a checkbox to change", boxes.length > 0, `${boxes.length}`);
    await press(/OptionsMenu.*AcceptButton$/, "OK button", 2500);
    const optsAfter = await stored();
    const changed = Object.keys(optsAfter).filter((k) => optsAfter[k] !== optsBefore[k]);
    result.numbers.options_changed = changed;
    check("OK stores the volume in the options store", changed.some((k) => /Volume/i.test(k)), changed.join(", "));
    check("OK stores the changed checkbox in the options store", changed.some((k) => !/Volume/i.test(k)), changed.join(", "));
    const m = await dumpTo("dump-after-ok");
    check("OK returns to the game menu", m.some((r) => /GameMenu.*SaveGameButton$/.test(r.path)) && !m.some((r) => /OptionsMenu.*AcceptButton$/.test(r.path)));
    await noErrors("after the options");

    // Save game (host): SAVE GAME -> the name -> SAVE; the server writes a save.
    await press(/GameMenu.*SaveGameButton$/, "Save game button", 3000);
    await shot("e5-save");
    await dumpTo("dump-save");
    await press(/SaveMenu.*\/SaveButton$/, "Save button", 1500);
    await dumpTo("dump-save-after");
    // an overwrite question is answered yes
    const yes = (await ui.dump("#top")).find((r) => /SaveMenu.*\/Yes$/.test(r.path));
    if (yes) await ui.click(yes);
    let saves = savesBefore;
    for (let i = 0; i < 40 && saves <= savesBefore; i++) { await sleep(1500); saves = saveFiles(stateDir).length; }
    result.numbers.saves_before = savesBefore;
    result.numbers.saves_after = saves;
    check("the host's save creates a save on the server", saves > savesBefore, `${savesBefore} -> ${saves} .c5sv under ${stateDir}/games`);
    st = await stats(b.page);
    check("the client sent one GameCtl::Save", st.m2?.saves_sent >= 1, `${st.m2?.saves_sent}`);
    await shot("e6-saved");
    await noErrors("after the save");

    // Exit to the main menu -> confirm -> our lobby.
    const menuOpen = (await ui.dump("#top")).some((r) => /GameMenu.*MainMenuButton$/.test(r.path));
    if (!menuOpen) { await b.page.keyboard.press("Escape"); await sleep(2500); }
    await press(/GameMenu.*MainMenuButton$/, "Exit to main menu button", 2500);
    await shot("e7-exit-confirm");
    await dumpTo("dump-exit");
    await press(/GameMenu.*ExitConfirm.*\/Yes$/, "Yes button of the exit confirmation", 1500);
    const lobby = await until(b.page, (x) => x.screen === "lobby", 60000, "the lobby after Exit to main menu").then(() => true, () => false);
    check("Exit to main menu returns to the lobby", lobby);
    await shot("e8-lobby");

    // ---- Part 2: a second game; the DEV game data gives the human a victory on turn 3 ----
    await openGame(b, o, base, "b");
    const g2 = await gain();
    result.numbers.gain_in_game_2 = g2;
    check("the stored volume is applied in the next game", typeof g2 === "number" && Math.abs(g2 - 0.2) < 0.12, `gain ${g2}`);
    await noErrors("at the start of game 2");
    let over = false;
    let at = (await stats(b.page)).replica?.turn ?? 0;
    const handled = [];
    for (let turn = 0; turn < 6 && !over; turn++) {
      let advanced = false;
      for (let step = 0; step < 24 && !advanced && !over; step++) {
        over = (await ui.dump("EndGameMenu")).length > 0;
        if (over) break;
        const bl = await ui.blocking();
        if (bl.type === BLOCK.NONE) {
          await ui.clickEndTurn();
          for (let w = 0; w < 40; w++) {
            await sleep(1000);
            if (((await stats(b.page)).replica?.turn ?? 0) > at) { advanced = true; break; }
            if ((await ui.dump("EndGameMenu")).length > 0) { over = true; break; }
          }
          continue;
        }
        handled.push(`turn ${at}: ${await ui.handleBlock(bl)}`);
        await sleep(2500);
        if (((await stats(b.page)).replica?.turn ?? 0) > at) advanced = true;
      }
      if (!over && !advanced) { await shot(`stuck-${at}`); throw new Error(`the turn did not advance from ${at}: ${handled.slice(-4).join("; ")}`); }
      at += 1;
    }
    if (!over) {
      for (let i = 0; i < 40 && !over; i++) { await sleep(1000); over = (await ui.dump("EndGameMenu")).length > 0; }
    }
    result.numbers.turn_at_end = at;
    try {
      const [l] = await ui.lua(`print(TAG .. tostring(Game.GetGameState()) .. "," .. tostring(Game.GetWinner and Game.GetWinner()) .. "," .. tostring(Game.GetVictory and Game.GetVictory()) .. "," .. tostring(Game.GetGameTurn()) .. ",gold=" .. tostring(Players[Game.GetActivePlayer()]:GetGold()))`);
      result.numbers.replica_state_winner_victory_turn_gold = l;
      const [p] = await ui.lua(`local ok, r = pcall(function() local v = Game.GetVictory() local t = EndGameTypes.Time return tostring(v) .. "/" .. tostring(GameInfo.Victories[v] and GameInfo.Victories[v].Type) .. "/" .. tostring(t) .. "/" .. tostring(GameplayGameStateTypes and GameplayGameStateTypes.GAMESTATE_OVER) end) print(TAG .. tostring(ok) .. ":" .. tostring(r))`);
      result.numbers.game_over_probe = p;
    } catch (e) { result.numbers.replica_state_error = e.message; }
    await sleep(3000);
    await shot("g1-endgame");
    const eg = await dumpTo("dump-endgame");
    check("the end-game screen shows after the victory (DEV game data, debug path)", over && eg.length > 0, `${eg.length} rows`);
    await noErrors("with the end-game screen");
    // M5 WO-I: the text of the screen follows the victory type. (The original's text of a time victory is the same as the one of
    // a domination victory: TXT_KEY_VICTORY_FLAVOR_TIME and _DOMINATION are one string in CIV5GameTextInfos_InGameScreens.xml.)
    try {
      const endText = async () => ((await ui.dump("EndGameMenu")).find((r) => /\/EndGameText$/.test(r.path))?.text ?? "").trim();
      const key = async (k) => ((await ui.lua(`print(TAG .. Locale.ConvertTextKey("${k}"))`))[0] ?? "").trim();
      const [vtype] = await ui.lua(`local v = Game.GetVictory() print(TAG .. tostring(GameInfo.Victories[v] and GameInfo.Victories[v].Type))`);
      result.numbers.end_victory_type = vtype;
      check("the end-game text is the one of the victory type (time)", (await endText()) === (await key("TXT_KEY_VICTORY_FLAVOR_TIME")) && vtype === "VICTORY_TIME", `${vtype}: ${(await endText()).slice(0, 40)}`);
      // (the screen ignores a second EndGameShow while it is up: "Wait! Just... one... more... turn..." closes it first)
      const closeEnd = async () => {
        const r = (await ui.dump("EndGameMenu")).find((x) => /\/BackButton$/.test(x.path));
        if (r) await ui.click(r);
        await sleep(1500);
      };
      for (const [type, k] of [["Culture", "TXT_KEY_VICTORY_FLAVOR_CULTURE"], ["Diplomatic", "TXT_KEY_VICTORY_FLAVOR_DIPLOMACY"], ["Technology", "TXT_KEY_VICTORY_FLAVOR_TECHNOLOGY"], ["Domination", "TXT_KEY_VICTORY_FLAVOR_DOMINATION"]]) {
        await closeEnd();
        await ui.lua(`Events.EndGameShow(EndGameTypes.${type}, Game.GetActiveTeam())`);
        await sleep(type === "Technology" ? 9500 : 1500); // (a space victory shows its screen 7 s late)
        const got = await endText();
        check(`the end-game text follows the type ${type}`, got !== "" && got === (await key(k)), got.slice(0, 50));
        if (type === "Culture") await shot("g1b-endgame-culture");
      }
      await closeEnd();
      await ui.lua(`Events.EndGameShow(EndGameTypes.Time, Game.GetActiveTeam())`);
      await sleep(1000);
    } catch (e) { check("the end-game text follows the victory type", false, e.message); }
    for (const [btn, name] of [[/RankingButton$/, "ranking"], [/DemographicsButton$/, "demographics"], [/ReplayButton$/, "replay"]]) {
      try {
        await press(new RegExp(`EndGameMenu.*${btn.source}`), `${name} tab`, 3500);
        await shot(`g2-${name}`);
        await dumpTo(`dump-${name}`);
        await noErrors(`on the ${name} tab`);
      } catch (e) { check(`the ${name} tab opens`, false, e.message); }
    }
    result.numbers.handled = handled.length;
  } catch (e) {
    error = e;
    await shot("failure");
  }
  result.consoleErrors.push(...b.errors);
  try { writeFileSync(`${o.logDir}/console.txt`, b.console.map((l) => l.text).join("\n")); } catch { /* best effort */ }
  await b.close();
  check("no console errors", result.consoleErrors.length === 0, `${result.consoleErrors.length} error(s)${result.consoleErrors[0] ? `: ${result.consoleErrors[0]}` : ""}`);
  if (error) { check("no scenario error", false, error.message); result.error = error.stack ?? String(error); }
  result.pass = result.checks.every((c) => c.pass);
  result.durationMs = Date.now() - started;
  return result;
}
