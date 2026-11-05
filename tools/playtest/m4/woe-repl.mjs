// Dev tool of M4 WO-E: starts a game on the original UI (like the scenario), then serves commands from a directory so a window can
// be explored without a new 6-minute start for every try:
//   node tools/playtest/m4/woe-repl.mjs <server url> <dir>
// A file <dir>/<n>.lua is run in the UI layer (output lines -> <n>.out); <dir>/<n>.js is the body of an async function with
// {b, ui, page, shot, sleep, stats} in scope (its return value -> <n>.out). `<n>.png` is written by shot("<n>") calls.
// Stop: create <dir>/stop.
import fs from "node:fs";
import { openPage, act, until, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { uiDriver } from "../lib/ui-driver.mjs";

const [base, dir] = process.argv.slice(2);
fs.mkdirSync(dir, { recursive: true });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const stats = (page) => page.evaluate(() => { const s = window.__civ5stats ?? {}; return { ui: s.ui ?? null, replica: s.replica ?? null, exe: s.exe ?? null, m2: s.m2 ?? null }; });
const b = await openPage({ headed: false, logDir: dir, viewport: { width: 1280, height: 720 } });
const page = b.page;
const shot = async (name) => { await page.screenshot({ path: `${dir}/${name}.png` }); };
const ui = uiDriver(b);
for (let attempt = 0; ; attempt++) {
  const nick = `m4r${(Date.now() + attempt) % 1000000}`;
  await page.goto(appUrl(base, { gfx: "webgl2", preset: "low" }, { nick, autostart: "1", ai: "2", size: "small", seed: "7", mode: "simultaneous", manual: "1", stats: "1", disable: "FluidFOW", ...(process.env.M4_NOMT ? { mt: "0" } : {}) }), { timeout: 180000 });
  try {
    const s = await until(page, (x) => x.screen === "login" || x.screen === "play", 90000, "sign-in window or the game");
    if (s.screen === "login") await act(page, { kind: "login", nick, password: DEV_PASSWORD });
    await until(page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 60000, "the game");
    break;
  } catch (e) { if (attempt >= 2) throw e; }
}
for (;;) { const st = await stats(page); if (st.m2?.phase === 2) break; await sleep(500); }
await sleep(4000);
fs.writeFileSync(`${dir}/ready`, "1");
console.log("ready");
const done = new Set();
while (!fs.existsSync(`${dir}/stop`)) {
  for (const f of fs.readdirSync(dir)) {
    const m = /^(\w+)\.(lua|js)$/.exec(f);
    if (!m || done.has(f)) continue;
    done.add(f);
    const src = fs.readFileSync(`${dir}/${f}`, "utf8");
    let out;
    try {
      if (m[2] === "lua") out = (await ui.lua(src, 60000)).join("\n");
      else out = JSON.stringify(await new (Object.getPrototypeOf(async function () {}).constructor)("b", "ui", "page", "shot", "sleep", "stats", src)(b, ui, page, shot, sleep, () => stats(page)), null, 1);
    } catch (e) { out = `ERROR ${e.stack ?? e}`; }
    fs.writeFileSync(`${dir}/${m[1]}.out`, String(out ?? ""));
  }
  await sleep(300);
}
await b.close();
