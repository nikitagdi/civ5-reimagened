// m4-repl: boots the original UI on a running server (late-huge nick or a fresh game) and then executes commands from
// files, so a window can be probed without a 60 s boot each time. Dev tool of WO-F (M4), not a scenario.
//   node tools/playtest/m4-repl.mjs --url http://127.0.0.1:8830 --dir D:\m4-wof\repl [--late-nick late1 --late-token <file>]
// A command is a file <dir>/in-<n>.json {"kind": "lua"|"dump"|"click"|"clickpath"|"key"|"shot"|"eval"|"errors", ...}; the answer is
// <dir>/out-<n>.txt (written when done). "stop" ends it.
import fs from "node:fs";
import path from "node:path";
import { openPage, act, until, appUrl } from "./lib/browser.mjs";
import { DEV_PASSWORD } from "./lib/game.mjs";
import { uiDriver, BLOCK } from "./lib/ui-driver.mjs";

const arg = (k, d) => { const i = process.argv.indexOf(k); return i >= 0 ? process.argv[i + 1] : d; };
const url = arg("--url", "http://127.0.0.1:8830");
const dir = arg("--dir", "D:\\m4-wof\\repl");
const lateNick = arg("--late-nick");
const lateToken = arg("--late-token");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
fs.mkdirSync(dir, { recursive: true });
for (const f of fs.readdirSync(dir)) fs.rmSync(path.join(dir, f), { force: true });
const b = await openPage({ headed: false, logDir: dir, viewport: { width: 1600, height: 900 } });
const o = { gfx: "webgl2" };
if (lateNick) {
  const token = fs.readFileSync(lateToken, "utf8").trim();
  await b.page.addInitScript(([nick, tok]) => { try { localStorage.setItem("civ5.nick", nick); localStorage.setItem(`civ5.token.${nick}`, tok); } catch { /* */ } }, [lateNick, token]);
  await b.page.goto(appUrl(url, o, { nick: lateNick, stats: "1" }));
} else {
  const nick = `m4r${Date.now() % 1000000}`;
  await b.page.goto(appUrl(url, o, { nick, autostart: "1", ai: "2", size: "small", seed: "7", mode: "simultaneous", stats: "1" }));
  const s = await until(b.page, (x) => x.screen === "login" || x.screen === "play", 90000, "login");
  if (s.screen === "login") await act(b.page, { kind: "login", nick, password: DEV_PASSWORD });
}
await until(b.page, (x) => x.screen === "play" && x.net?.phase === "play" && x.turn !== null, 240000, "the game");
for (;;) {
  const m2 = await b.page.evaluate(() => window.__civ5stats?.m2);
  if (m2?.phase === 2) break;
  await sleep(500);
}
await sleep(3000);
fs.writeFileSync(path.join(dir, "ready"), "1");
const ui = uiDriver(b);
let n = 1;
for (;;) {
  const f = path.join(dir, `in-${n}.json`);
  if (!fs.existsSync(f)) { await sleep(300); continue; }
  await sleep(100);
  let out = "";
  try {
    const c = JSON.parse(fs.readFileSync(f, "utf8"));
    const from = b.console.length;
    if (c.kind === "stop") break;
    if (c.kind === "lua") out = (await ui.lua(c.code, c.timeout ?? 30000)).join("\n");
    else if (c.kind === "dump") out = (await ui.dump(c.filter ?? "")).map((r) => `${r.path}\t${r.cls}\t${r.x} ${r.y} ${r.w} ${r.h}\t${r.disabled ? "D" : ""}${r.void1 !== null ? "v" + r.void1 : ""}\t${r.text}`).join("\n");
    else if (c.kind === "found") {
      await ui.clickEndTurn();
      await sleep(9000);
      out = `found: ${await ui.unitAction(/FOUND/)}`;
      await sleep(10000);
    } else if (c.kind === "turns") {
      // END TURN c.n times, handling what blocks it as a player would.
      const turnOf = async () => (await b.page.evaluate(() => window.__civ5stats?.replica?.turn)) ?? 0;
      const notes = [];
      for (let t = 0; t < c.n; t++) {
        const at = await turnOf();
        let advanced = false;
        for (let step = 0; step < 24 && !advanced; step++) {
          const bl = await ui.blocking();
          if (bl.type === BLOCK.NONE) {
            await ui.clickEndTurn();
            const t0 = Date.now();
            while (Date.now() - t0 < 40000) { await sleep(1000); if ((await turnOf()) > at) { advanced = true; break; } }
            continue;
          }
          notes.push(`turn ${at}: ${await ui.handleBlock(bl)}`);
          await sleep(2500);
          if ((await turnOf()) > at) advanced = true;
        }
        if (!advanced) { notes.push(`STUCK at ${at}`); break; }
        await sleep(3000);
      }
      out = `turn ${await turnOf()}; ${notes.join("; ")}`;
    } else if (c.kind === "clickpath") {
      const rows = await ui.dump(c.filter);
      const r = rows.find((x) => (!c.cls || x.cls === c.cls) && (!c.text || new RegExp(c.text, "i").test(x.text)) && (c.last ? true : true));
      if (!r) out = "NOT FOUND"; else { await ui.click(r); out = `clicked ${r.path} ${r.text}`; }
    } else if (c.kind === "click") { await b.page.mouse.move(c.x, c.y); await b.page.mouse.click(c.x, c.y); out = "ok"; }
    else if (c.kind === "key") { await b.page.keyboard.press(c.key); out = "ok"; }
    else if (c.kind === "shot") { await b.page.screenshot({ path: c.path ?? path.join(dir, `shot-${n}.png`), timeout: 120000 }); out = "ok"; }
    else if (c.kind === "eval") out = JSON.stringify(await b.page.evaluate(c.code));
    else if (c.kind === "errors") out = JSON.stringify((await b.page.evaluate(() => window.__civ5stats?.ui)) ?? null);
    if (c.wait) await sleep(c.wait);
    const lines = b.console.slice(from).map((l) => l.text).filter((t) => /error|Error|lua|traceback|\.lua:\d+/i.test(t) && !t.includes("copyToChannel") && !t.includes("shared"));
    if (lines.length && c.kind !== "turns" && c.kind !== "found") out += "\n--console--\n" + lines.slice(0, 30).join("\n");
  } catch (e) {
    out = `EXC ${e.message}`;
  }
  fs.writeFileSync(path.join(dir, `out-${n}.txt`), out);
  n++;
}
await b.close();
