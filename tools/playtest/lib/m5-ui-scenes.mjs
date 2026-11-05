// The scenes of AC-S2 that WO-I owns (B4 M5): S4 tech tree and S5 trade with the AI, on the saves of the original's goldens.
// Each scene: wake the game of the imported save (nick `g4` / `g5`, game 2 / 1 of the server of `tools/playtest/m5/import-golden.ps1`),
// open the window by mouse (controls from the screen dump, `lib/ui-driver.mjs`), shoot 1920x1080, FLIP against the golden.
// `runUiScenes(ctx)` is what the scenario `m5-goldens-s45` runs; WO-H's `m5-goldens` can call it with the same `ctx`.
import { copyFileSync, existsSync, mkdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { openPage, act, until, state, appUrl } from "./browser.mjs";
import { DEV_PASSWORD } from "./game.mjs";
import { uiDriver } from "./ui-driver.mjs";
import { flipCompare } from "./flip.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../..");
/** The goldens of the original (override: GOLDENS_B4_UI). */
export const GOLDEN_DIR = process.env.GOLDENS_B4_UI ?? path.resolve(root, "../../assets-out/goldens-b4-ui");

async function stats(page) {
  return page.evaluate(() => {
    const s = window.__civ5stats ?? {};
    return { ui: s.ui ?? null, replica: s.replica ?? null, m2: s.m2 ?? null, camera: s.camera ?? null };
  });
}

/** Logs `nick` in (dev password), takes the seat of the sleeping game and waits for the original UI. */
async function enterScene(b, o, base, nick, game) {
  await b.page.goto(appUrl(base, o, { nick, ui: "original", stats: "1", ...(process.env.M5_NOMT ? { mt: "0" } : {}) }));
  const s = await until(b.page, (x) => x.screen === "login" || x.screen === "lobby" || x.screen === "play", 90000, "sign-in window, lobby or game");
  if (s.screen === "login") await act(b.page, { kind: "login", nick, password: DEV_PASSWORD });
  let cur = await until(b.page, (x) => x.screen === "lobby" || x.screen === "play", 60000, "lobby");
  if (cur.screen === "lobby") {
    const row = cur.net?.games?.find((g) => g.id === game);
    const seat = row?.seats?.find((x) => x.yours)?.player ?? 0;
    await act(b.page, { kind: "takeSeat", game, player: seat });
  }
  await until(b.page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 240000, "the game of the save");
  const upAt = Date.now();
  for (;;) {
    const st = await stats(b.page);
    if (st.m2?.phase === 2) break;
    if (st.m2?.phase === -1) throw new Error("M2 failed (phase -1)");
    if (Date.now() - upAt > 300000) throw new Error(`M2 is not up: ${JSON.stringify(st.m2)}`);
    await sleep(500);
  }
  await sleep(5000);
  return (await state(b.page))?.turn;
}

/** Closes what is open in the way of the scene (Escape): popups of the first turn of the loaded game. */
async function clearPopups(b, ui) {
  for (let i = 0; i < 4; i++) {
    const top = (await ui.dump("#top")).slice(0, 1)[0]?.path ?? "";
    if (!/\/(NotificationLogPopup|TechAwardPopup|ChooseTech|GenericPopup|ProductionPopup|TurnStart|AdvisorModal)/i.test(top)) break;
    await b.page.keyboard.press("Escape");
    await sleep(1200);
  }
}

// S5: the leader of the AI in the diplomacy overview -> Trade; the camera on the AI's capital (what the engine does for the leader view).
async function s5(env) {
  const { ui, check, result, shot } = env;
  const [ai] = await ui.lua(`for i = 0, GameDefines.MAX_MAJOR_CIVS - 1 do local p = Players[i] if p and p:IsAlive() and not p:IsHuman() and not p:IsMinorCiv() and i ~= Game.GetActivePlayer() then print(TAG .. i) break end end`);
  result.numbers.S5_ai = ai;
  await ui.lua(`local c = Players[${ai}]:GetCapitalCity() if c then UI.LookAt(c:Plot()) end`);
  result.numbers.S5_cap = await ui.lua(`local c = Players[${ai}]:GetCapitalCity() local me = Players[Game.GetActivePlayer()]:GetCapitalCity() print(TAG .. tostring(c and c:GetX()) .. "," .. tostring(c and c:GetY()) .. " rev=" .. tostring(c and c:Plot():IsRevealed(Game.GetActiveTeam())) .. " me=" .. tostring(me and me:GetX()) .. "," .. tostring(me and me:GetY()))`);
  await sleep(2500);
  const dbtn = (await ui.dump("")).find((r) => /\/DiploCornerStack\/DiploButton$/.test(r.path));
  if (dbtn) { await ui.click(dbtn); await sleep(3500); }
  const leader = await ui.find("", (r) => /\/MajorCivScrollPanel\/ButtonStack\/DiploButton$/.test(r.path) && r.cls === "Button" && !r.disabled);
  check("S5: the Diplomacy Overview lists the AI leader", !!leader, leader?.path ?? "none");
  if (leader) await ui.click(leader);
  await sleep(5000);
  const trade = await ui.find("/LeaderHeadRoot", (r) => /\/TradeButton$/.test(r.path) && !r.disabled);
  check("S5: the leader screen has a Trade button", !!trade);
  if (trade) await ui.click(trade);
  await sleep(5000);
  const rows = await ui.dump("/LeaderHeadRoot/DiploTrade");
  check("S5: the trade table is open", rows.length > 20, `${rows.length} controls`);
  result.numbers.S5_pocket = rows.filter((r) => /Pocket(Gold|Luxury|Strategic|Embassy|OpenBorders|DefensivePact|ResearchAgreement)/.test(r.path) && r.cls === "Button").map((r) => `${r.path.split("/").pop()}${r.disabled ? " D" : ""} ${r.text}`);
  await sleep(1500);
  return "S5-trade-ai";
}

// S4: the tech tree from the science button of the top panel.
async function s4(env) {
  const { ui, check } = env;
  const btn = await ui.find("", (r) => /TopPanelInfoStack\/(SciencePerTurn|ScienceString)$/.test(r.path) && r.cls === "TextButton");
  check("S4: the top panel has the science button", !!btn, btn?.path ?? "none");
  if (btn) await ui.click(btn);
  await sleep(5000);
  const rows = await ui.dump("/TechTree");
  check("S4: the tech tree is open", rows.length > 50, `${rows.length} controls`);
  await sleep(1500);
  return "S4-tech-tree";
}

export const SCENES = [
  { id: "S5", nick: "g5", game: 1, golden: "S5-trade-ai", run: s5, region: [470, 620, 975, 445] },
  { id: "S4", nick: "g4", game: 1, port: 8866, golden: "S4-tech-tree", run: s4 },
];

/** Runs the scenes; `ctx` = { opts, base, result, only } (the scenario's context). Fills `result` and returns it. */
export async function runUiScenes(ctx) {
  const { opts: o, base, result, only } = ctx;
  const started = Date.now();
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  mkdirSync(`${o.logDir}`, { recursive: true });
  result.numbers.flip = {};
  for (const scene of SCENES.filter((s) => !only || only.includes(s.id) || only.includes(s.golden))) {
    const b = await openPage({ headed: o.headed, logDir: o.logDir, consoleFile: `console-${scene.id}.log`, viewport: { width: 1920, height: 1080 } });
    let error = null;
    try {
      const turn = await enterScene(b, o, scene.port ? base.replace(/:\d+$/, `:${scene.port}`) : base, scene.nick, scene.game);
      result.numbers[`${scene.id}_turn`] = turn;
      const ui = uiDriver(b);
      await clearPopups(b, ui);
      const shotFile = `${o.logDir}/${scene.id}-ours.png`;
      const name = await scene.run({ b, ui, check, result, shot: async () => b.page.screenshot({ path: shotFile, timeout: 120000 }) });
      await b.page.screenshot({ path: shotFile, timeout: 120000 });
      const st = await stats(b.page);
      check(`${scene.id}: ui.lua_errors == 0`, st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
      const golden = path.join(GOLDEN_DIR, `${name}.png`);
      if (!existsSync(golden)) throw new Error(`no golden ${golden}`);
      copyFileSync(golden, `${o.logDir}/${scene.id}-golden.png`);
      const f = flipCompare(golden, shotFile, o.logDir, scene.id);
      result.numbers.flip[scene.id] = f;
      if (scene.region) {
        // the world behind the window differs (the AI's land is not revealed in our view of the imported game): the window alone
        const fw = flipCompare(golden, shotFile, o.logDir, `${scene.id}-window`, scene.region);
        result.numbers.flip[`${scene.id}-window`] = fw;
        check(`${scene.id}: FLIP of the window (mean ${fw.mean.toFixed(4)}, p95 ${fw.p95.toFixed(3)})`, true, `${scene.id}-window-side.png`);
      }
      check(`${scene.id}: FLIP against the golden (mean ${f.mean.toFixed(4)}, p95 ${f.p95.toFixed(3)}, share > 0.2: ${(f.over02 * 100).toFixed(1)}%)`, true, `${scene.id}-side.png`);
    } catch (e) {
      error = e;
      try { await b.page.screenshot({ path: `${o.logDir}/${scene.id}-failure.png` }); } catch { /* gone */ }
      check(`${scene.id}: no scenario error`, false, e.message);
    }
    result.consoleErrors.push(...b.errors);
    await b.close();
    if (error) result.error = error.stack ?? String(error);
  }
  check("no console errors", result.consoleErrors.length === 0, `${result.consoleErrors.length} error(s)${result.consoleErrors[0] ? `: ${result.consoleErrors[0]}` : ""}`);
  result.pass = result.checks.every((c) => c.pass);
  result.durationMs = Date.now() - started;
  return result;
}
