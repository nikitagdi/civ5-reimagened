// `m2-k1k3` (WO-6, milestone M2): the original UI (EUI, the default game screen) on the replica of a real networked game.
// Needs the real-core server (WSL, `tools/playtest/m2/server.ps1`) and `--attach <url>`:
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8300 --scenario m2-k1k3 [--gfx webgl2] [--headed] [--turns 10]
// Flow (the original UI, by mouse at 1600x900; the controls are found by path / class / text in the screen dump and
// clicked in the middle of their rectangle, `lib/ui-driver.mjs`, no pixels): enter -> flags -> END TURN (blocked by
// units: it selects the settler, as in the game) -> found a city (the settler panel's button) -> then every turn END TURN,
// and what blocks it is handled as a player would (`Player:GetEndTurnBlockingType`): a city out of production opens
// its screen (K8) and a unit is picked from the list, a unit that needs orders is fortified / skipped -> N turns.
// Checks: __civ5stats.ui.lua_errors == 0, replica.diverged == 0, the exe.simp.* stubs the run called are listed.
// Screenshots K1-K3 go to the log dir.
import { openPage, act, until, state, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { uiDriver, BLOCK } from "../lib/ui-driver.mjs";

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
  const result = { scenario: "m2-k1k3", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const base = o.attach ?? ctx.server?.url;
  if (!base) {
    check("options", false, "m2-k1k3 needs --attach <url of the real-core server>");
    return result;
  }
  const turns = o.turns && o.turns <= 30 ? o.turns : 10;
  const b = await openPage({ headed: o.headed, logDir: o.logDir, viewport: { width: 1600, height: 900 } });
  const shot = async (name) => { try { await b.page.screenshot({ path: `${o.logDir}/${name}.png` }); } catch { /* page gone */ } };
  // Bytes of the WebSocket frames (the game's traffic), per turn below.
  let rx = 0, tx = 0;
  b.page.on("websocket", (ws) => {
    ws.on("framereceived", (f) => { rx += typeof f.payload === "string" ? f.payload.length : f.payload.length; });
    ws.on("framesent", (f) => { tx += typeof f.payload === "string" ? f.payload.length : f.payload.length; });
  });
  // The bodies of `/replica/{hash}` (the snapshot of a turn, over HTTP): wire bytes (content-length) and the count.
  const replicaFetches = [];
  b.page.on("response", (res) => {
    if (res.url().includes("/replica/")) replicaFetches.push({ at: Date.now(), bytes: Number(res.headers()["content-length"] ?? 0), status: res.status() });
  });
  let error = null;
  try {
    // The quick start sometimes stays in the room (a frame-rate race of the script on a software renderer): a new nick.
    let t0 = 0;
    for (let attempt = 0; ; attempt++) {
      const nick = `m2a${(Date.now() + attempt) % 1000000}`;
      t0 = Date.now();
      await b.page.goto(appUrl(base, o, { nick, autostart: "1", ai: "2", size: "small", seed: "7", mode: "simultaneous", stats: "1", ...(process.env.M3_NOMT ? { mt: "0" } : {}) }));
      try {
        const s = await until(b.page, (x) => x.screen === "login" || x.screen === "play", 90000, "sign-in window or the game");
        if (s.screen === "login") await act(b.page, { kind: "login", nick, password: DEV_PASSWORD });
        await until(b.page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 60000, "the game");
        break;
      } catch (e) {
        if (attempt >= 2) throw e;
      }
    }
    result.numbers.toGameScreenMs = Date.now() - t0;
    const upAt = Date.now();
    let st = null;
    for (;;) {
      st = await stats(b.page);
      if (st.m2?.phase === 2) break;
      if (st.m2?.phase === -1) throw new Error("M2 failed (phase -1)");
      if (Date.now() - upAt > 180000) throw new Error(`M2 is not up after 180 s: ${JSON.stringify(st.m2)}`);
      await sleep(500);
    }
    result.numbers.toUiMs = Date.now() - t0;
    result.numbers.m2 = st.m2;
    await sleep(4000);
    st = await stats(b.page);
    check("M2 is up and live", st.m2?.phase === 2 && st.replica?.live === 1, JSON.stringify({ m2: st.m2, live: st.replica?.live }));
    check("the UI has controls (flags, banners, panels)", (st.ui?.controls ?? 0) > 100, `controls ${st.ui?.controls}`);
    check("ui.lua_errors == 0 after the start", st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
    {
      // AC-S4: no egui panel of the game on the game screen (the test port reads what egui is asked to draw).
      const g = await state(b.page);
      const panels = g ? ["unit_panel", "city_panel", "players_panel", "question", "timer", "minimap"].filter((k) => g[k] ?? g[k.replace(/_(\w)/g, (_, c) => c.toUpperCase())]) : ["no state"];
      if (g?.settingsOpen || g?.settings_open) panels.push("settings");
      if ((g?.tooltip ?? []).length) panels.push("tooltip");
      if ((g?.journal ?? []).length) panels.push("journal");
      check("AC-S4: no egui game panel on the game screen", g?.screen === "play" && panels.length === 0, `screen ${g?.screen}, panels: ${panels.join(",") || "none"}`);
    }
    check("units and cities are known to the UI", (st.exe?.entity_units ?? 0) > 0 && (st.exe?.entity_cities ?? 0) > 0, `${st.exe?.entity_units} units, ${st.exe?.entity_cities} cities`);
    result.numbers.start = { ui: st.ui, replica: st.replica, exe: st.exe };
    await shot("k1-enter");

    const ui = uiDriver(b);
    const handled = [];
    // END TURN while units wait for orders selects the first one (OnEndTurnClicked).
    await ui.clickEndTurn();
    await sleep(9000);
    await shot("k3-unit-selected");
    // Found the city with the settler panel's button.
    const citiesBefore = (await stats(b.page)).exe?.entity_cities ?? 0;
    const foundType = await ui.unitAction(/FOUND/);
    check("the settler has a found-city button", !!foundType, `${foundType}`);
    await sleep(12000);
    st = await stats(b.page);
    check("a city was founded", (st.exe?.entity_cities ?? 0) === citiesBefore + 1 && (st.replica?.net_sent ?? 0) >= 1, `cities ${citiesBefore} -> ${st.exe?.entity_cities}, sent ${st.replica?.net_sent}`);
    await shot("k1-city-founded");

    // End turns and watch the turn switch. What blocks END TURN is handled as a player would: a city out of
    // production (K8: its screen opens, a unit is picked by mouse), a unit that needs orders (fortify / skip).
    const endAt = [];
    const turnRx = [];
    const turnTx = [];
    let at = (await stats(b.page)).replica?.turn ?? 0;
    for (let t = 0; t < turns; t++) {
      const e0 = Date.now();
      const rx0 = rx, tx0 = tx;
      let advanced = false;
      for (let step = 0; step < 24 && !advanced; step++) {
        const bl = await ui.blocking();
        if (bl.type === BLOCK.NONE) {
          await ui.clickEndTurn();
          const until0 = Date.now();
          while (Date.now() - until0 < 40000) {
            await sleep(1000);
            if (((await stats(b.page)).replica?.turn ?? 0) > at) { advanced = true; break; }
          }
          if (!advanced) handled.push(`turn ${at}: END TURN did not advance (nothing blocks)`);
          continue;
        }
        if (step === 0 && at === (await stats(b.page)).replica?.turn && handled.length < 40) await shot(`blocked-${at}-${bl.type}`);
        const note = await ui.handleBlock(bl);
        handled.push(`turn ${at}: ${note}`);
        await sleep(2500);
        if (((await stats(b.page)).replica?.turn ?? 0) > at) advanced = true;
      }
      if (!advanced) {
        await shot(`stuck-${at}`);
        throw new Error(`the turn did not advance from ${at}: ${handled.slice(-4).join("; ")}`);
      }
      at += 1;
      endAt.push(Date.now() - e0);
      await sleep(1500);
      turnRx.push(rx - rx0);
      turnTx.push(tx - tx0);
      await sleep(2000);
      if (t === 1) await shot("k2-top-panel");
    }
    result.numbers.handledBlocks = handled;
    result.numbers.turnMs = endAt;
    result.numbers.turnRxBytes = turnRx;
    result.numbers.turnTxBytes = turnTx;
    result.numbers.totalRxBytes = rx;
    result.numbers.replicaFetches = replicaFetches;
    await shot("k1-after-turns");
    st = await stats(b.page);
    result.numbers.final = { ui: st.ui, replica: st.replica, exe: st.exe };
    check(`turn advanced ${turns} times`, at >= turns, `turn ${at}`);
    check("ui.lua_errors == 0", st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
    check("replica.diverged == 0", st.replica?.diverged === 0, `${st.replica?.diverged}`);
    const simp = Object.entries(st.exe ?? {}).filter(([k]) => k.startsWith("simp.")).map(([k, v]) => `${k.slice(5)}=${v}`);
    result.numbers.simpCalls = simp;
    check("exe.simp_calls listed", true, `${st.exe?.simp_calls} calls: ${simp.join(", ")}`);
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
  if (error) {
    check("no scenario error", false, error.message);
    result.error = error.stack ?? String(error);
  }
  result.pass = result.checks.every((c) => c.pass);
  result.durationMs = Date.now() - started;
  return result;
}
