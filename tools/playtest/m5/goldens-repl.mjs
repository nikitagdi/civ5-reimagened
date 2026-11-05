// Dev tool of M5 WO-H: opens a golden game (key A or B) on the server and serves commands from a directory, like m4/woe-repl.mjs:
//   node tools/playtest/m5/goldens-repl.mjs <server url> <game A|B> <dir> [turn]
// <dir>/<n>.lua runs in the UI layer (-> <n>.out); <dir>/<n>.js is the body of an async function with {g, b, ui, page, shot, sleep}
// in scope (-> <n>.out); `shot("name")` writes <dir>/name.png at 1920x1080. Stop: create <dir>/stop.
import fs from "node:fs";
import { openPage } from "../lib/browser.mjs";
import { openGame } from "../lib/goldens.mjs";

const [base, key, dir, turnArg] = process.argv.slice(2);
fs.mkdirSync(dir, { recursive: true });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const b = await openPage({ headed: false, logDir: dir, viewport: { width: 1920, height: 1080 } });
const g = await openGame(b, base, key);
if (turnArg) await g.advanceTo(Number(turnArg));
const page = b.page, ui = g.ui;
const shot = async (name) => { await page.screenshot({ path: `${dir}/${name}.png` }); };
fs.writeFileSync(`${dir}/ready`, "1");
console.log("ready");
const done = new Set();
const AsyncFn = Object.getPrototypeOf(async function () {}).constructor;
while (!fs.existsSync(`${dir}/stop`)) {
  for (const f of fs.readdirSync(dir)) {
    const m = /^(\w+)\.(lua|js)$/.exec(f);
    if (!m || done.has(f)) continue;
    done.add(f);
    const src = fs.readFileSync(`${dir}/${f}`, "utf8");
    let out;
    try {
      out = m[2] === "lua" ? (await ui.lua(src, 60000)).join("\n") : JSON.stringify(await new AsyncFn("g", "b", "ui", "page", "shot", "sleep", src)(g, b, ui, page, shot, sleep), null, 1);
    } catch (e) { out = `ERROR ${e.stack ?? e}`; }
    fs.writeFileSync(`${dir}/${m[1]}.out`, String(out ?? ""));
  }
  await sleep(300);
}
await b.close();
