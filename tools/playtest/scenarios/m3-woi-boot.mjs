// `m3-woi-boot` (M3 WO-I): the boot of the original UI (EUI, the default game screen) on a real-core game and what the icon
// atlases cost. Needs the real-core server (WSL, `tools/playtest/m2/server.ps1`) and `--attach <url>`:
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8760 --scenario m3-woi-boot [--gfx webgl2] [--boots 3]
// Each boot: a new page, the game starts (autostart), the scenario waits for M2 phase 2 (the UI runs) and records the
// time from the page load to it, the bytes and the number of the `/assets/` responses until then and until 10 s after
// it (the atlases that load on demand), `ui.tex_missing`, the lazy counters and `ui.lua_errors`. The last boot also
// takes screenshots (enter, tech tree) for the comparison with the goldens of the original (S1, S4).
import { writeFileSync } from "node:fs";
import { openPage, act, until, state, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const SCIENCE_BUTTON = [20, 12];

async function stats(page) {
  return page.evaluate(() => {
    const s = window.__civ5stats ?? {};
    return { ui: s.ui ?? null, replica: s.replica ?? null, exe: s.exe ?? null, m2: s.m2 ?? null };
  });
}

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "m3-woi-boot", pass: false, logs: o.logDir, checks: [], numbers: { boots: [] }, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const base = o.attach ?? ctx.server?.url;
  if (!base) {
    check("options", false, "m3-woi-boot needs --attach <url of the real-core server>");
    return result;
  }
  const boots = Number(process.env.WOI_BOOTS ?? 3);
  let error = null;
  for (let i = 0; i < boots && !error; i++) {
    const b = await openPage({ headed: o.headed, logDir: o.logDir, viewport: { width: 1600, height: 900 } });
    const last = i === boots - 1;
    const shot = async (name) => {
      for (let k = 0; k < 3; k++) {
        try { await b.page.screenshot({ path: `${o.logDir}/${name}.png`, timeout: 120000 }); return; } catch { /* retry */ }
      }
    };
    const assets = { n: 0, bytes: 0 };
    b.page.on("response", (res) => {
      if (!res.url().includes("/assets/")) return;
      assets.n++;
      assets.bytes += Number(res.headers()["content-length"] ?? 0);
    });
    try {
      let t0 = 0;
      for (let attempt = 0; ; attempt++) {
        const nick = `woi${(Date.now() + attempt) % 1000000}`;
        t0 = Date.now();
        assets.n = 0; assets.bytes = 0;
        await b.page.goto(appUrl(base, o, { nick, autostart: "1", ai: "2", size: "small", seed: "7", mode: "simultaneous", manual: "1", stats: "1", luadbg: process.env.WOI_LUADBG ?? "" }));
        try {
          const s = await until(b.page, (x) => x.screen === "login" || x.screen === "play", 90000, "sign-in window or the game");
          if (s.screen === "login") await act(b.page, { kind: "login", nick, password: DEV_PASSWORD });
          // The quick start stays in the room now and then (a frame-rate race of the script on a software renderer): the
          // button of the room starts the game.
          let inRoom = 0;
          await until(b.page, (x) => {
            inRoom = x.screen === "room" ? inRoom + 1 : 0;
            if (inRoom === 200) b.page.mouse.click(470, 683);
            return x.screen === "play" && x.net?.phase === "play" && x.turn !== null;
          }, 90000, "the game");
          break;
        } catch (e) {
          if (attempt >= 2) throw e;
        }
      }
      let st = null;
      for (;;) {
        st = await stats(b.page);
        if (st.m2?.phase === 2) break;
        if (st.m2?.phase === -1) throw new Error("M2 failed (phase -1)");
        if (Date.now() - t0 > 420000) throw new Error(`M2 is not up after 420 s: ${JSON.stringify(st.m2)}`);
        await sleep(250);
      }
      const rec = { toUiMs: Date.now() - t0, assetsAtUp: { ...assets }, m2: st.m2, series: [] };
      // The atlases load on demand after the start: the counters every second for 12 s (the last boot: 30 s and shots).
      const upAt = Date.now();
      const span = last ? 30000 : 12000;
      while (Date.now() - upAt < span) {
        await sleep(1000);
        const x = await stats(b.page);
        const t = Math.round((Date.now() - upAt) / 1000);
        rec.series.push([t, x.ui?.tex_lazy_requested, x.ui?.tex_lazy_loaded, x.ui?.tex_lazy_pending, x.ui?.tex_missing, assets.n, Math.round(assets.bytes / 1e6)]);
        if (last && (t === 5 || t === 12)) await shot(`woi-enter-${t}s`);
      }
      st = await stats(b.page);
      rec.assetsAfter10s = { ...assets };
      rec.ui = st.ui;
      rec.luaErrors = st.ui?.lua_errors;
      result.numbers.boots.push(rec);
      check(`boot ${i + 1}: ui.lua_errors == 0`, st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
      if (last) {
        await shot("woi-enter");
        await b.page.mouse.move(...SCIENCE_BUTTON);
        await b.page.mouse.click(...SCIENCE_BUTTON);
        await sleep(6000);
        await shot("woi-tech-tree");
        st = await stats(b.page);
        result.numbers.afterTree = st.ui;
        check("ui.lua_errors == 0 with the tech tree open", st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
      }
    } catch (e) {
      error = e;
      await shot("failure");
      const lastState = await state(b.page).catch(() => null);
      if (lastState) result.numbers.lastScreen = `${lastState.screen}/${lastState.net?.phase}`;
      result.numbers.lastStats = await stats(b.page).catch(() => null);
    }
    result.consoleErrors.push(...b.errors);
    try { writeFileSync(`${o.logDir}/console-${i + 1}.log`, b.console.map((c) => c.text).join("\n")); } catch { /* no log dir */ }
    await b.close();
  }
  const times = result.numbers.boots.map((r) => r.toUiMs).sort((a, b) => a - b);
  result.numbers.toUiMsMedian = times[Math.floor(times.length / 2)] ?? null;
  // The 404 of `pkg-mt` of a single-threaded bundle is not an error of the UI.
  const errs = result.consoleErrors.filter((e) => !e.includes("/pkg-mt/"));
  check("no console errors", errs.length === 0, `${errs.length} error(s)${errs[0] ? `: ${errs[0]}` : ""}`);
  if (error) {
    check("no scenario error", false, error.message);
    result.error = error.stack ?? String(error);
  }
  result.pass = result.checks.every((c) => c.pass);
  result.durationMs = Date.now() - started;
  return result;
}
