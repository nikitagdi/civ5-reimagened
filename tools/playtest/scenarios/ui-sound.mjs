// `ui-sound` (WO-S, milestone M3): clicking an original-UI button plays its sound (U-D7).
// Needs the real-core server and `--attach <url>` like m2-k1k3:
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8750 --scenario ui-sound [--gfx webgl2] [--headed]
// Flow: the original UI at 1600x900 -> a click on END TURN (a button of the original UI) -> `__civ5stats.audio.*`:
// the sound table is up, the click reached the engine (button_events), a sound file was fetched and decoded
// (files_loaded) and a voice started on the Web Audio output (started). Before the first press nothing sounds
// (started 0); the press resumes the context.
import { openPage, until, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { act } from "../lib/browser.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const END_TURN = [1423, 682];

async function audio(page) {
  return page.evaluate(() => ({ audio: window.__civ5stats?.audio ?? null, m2: window.__civ5stats?.m2 ?? null, ui: window.__civ5stats?.ui ?? null }));
}

export async function run(ctx) {
  const o = ctx.opts;
  const result = { scenario: "ui-sound", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const base = o.attach ?? ctx.server?.url;
  if (!base) {
    check("options", false, "ui-sound needs --attach <url of the real-core server>");
    return result;
  }
  const b = await openPage({ headed: o.headed, logDir: o.logDir, viewport: { width: 1600, height: 900 } });
  let error = null;
  try {
    const nick = `snd${Date.now() % 1000000}`;
    await b.page.goto(appUrl(base, o, { nick, autostart: "1", ai: "2", size: "small", seed: "7", mode: "simultaneous", stats: "1", volume: "60" }));
    const s = await until(b.page, (x) => x.screen === "login" || x.screen === "play", 90000, "sign-in window or the game");
    if (s.screen === "login") await act(b.page, { kind: "login", nick, password: DEV_PASSWORD });
    await until(b.page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 60000, "the game");
    const upAt = Date.now();
    let st;
    for (;;) {
      st = await audio(b.page);
      if (st.m2?.phase === 2) break;
      if (st.m2?.phase === -1) throw new Error("M2 failed (phase -1)");
      if (Date.now() - upAt > 180000) throw new Error("M2 is not up after 180 s");
      await sleep(500);
    }
    await sleep(3000);
    st = await audio(b.page);
    result.numbers.beforeClick = st.audio;
    check("the sound table is loaded (audio.ready)", st.audio?.ready === 1, JSON.stringify(st.audio));
    // (headless Chromium may start the context without a press; a headed browser keeps it suspended until one)
    check("nothing sounded before the first press", st.audio?.started === 0, `started ${st.audio?.started}, running ${st.audio?.running}`);
    await b.page.mouse.move(END_TURN[0], END_TURN[1]);
    await b.page.mouse.click(END_TURN[0], END_TURN[1]);
    await sleep(3000);
    st = await audio(b.page);
    result.numbers.afterClick = st.audio;
    check("the press unlocked the output", st.audio?.unlocked === 1 && st.audio?.running === 1, JSON.stringify(st.audio));
    check("the click reached the sound engine", (st.audio?.button_events ?? 0) >= 1, `button_events ${st.audio?.button_events}`);
    check("a sound file was fetched and decoded", (st.audio?.files_loaded ?? 0) >= 1 && st.audio?.files_failed === 0, `loaded ${st.audio?.files_loaded}, failed ${st.audio?.files_failed}`);
    check("a voice started (played-sounds counter)", (st.audio?.started ?? 0) >= 1, `started ${st.audio?.started}`);
    check("ui.lua_errors == 0", st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
    try { await b.page.screenshot({ path: `${o.logDir}/ui-sound.png` }); } catch { /* page gone */ }
  } catch (e) {
    error = e;
  }
  result.consoleErrors = b.errors ?? [];
  await b.close();
  if (error) check("scenario ran", false, String(error?.message ?? error));
  result.pass = result.checks.every((c) => c.pass);
  return result;
}
