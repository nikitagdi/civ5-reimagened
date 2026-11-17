// Shared runner for the popup screen acceptance scenarios.
import { openPage, act, until, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { clickPart } from "./ui2-v1.mjs";
export const texts = (p, part) => (p?.texts ?? []).filter(t => t.part === part).map(t => t.text);
export const probe = page => page.evaluate(() => window.__civ5test.ui2());
export async function waitPopup(page, predicate, description) {
  const end = Date.now() + 30000;
  while (Date.now() < end) {
    const p = await probe(page);
    if (predicate(p)) return p;
    await new Promise(resolve => setTimeout(resolve, 150));
  }
  throw new Error("Timed out: " + description);
}
export async function popupScenario(ctx, screen, exercise, args = {}) {
  const { server, opts } = ctx;
  if (!opts.attach) throw new Error(screen + " requires --attach to a real-core server");
  const browser = await openPage({ headed: opts.headed, logDir: opts.logDir,
    viewport: { width: 1366, height: 768 }, consoleFile: "ui2-" + screen + ".log" });
  const checks = [];
  const check = (name, pass) => {checks.push({name, pass:!!pass}); if (!pass) throw new Error(name);};
  try {
    const page = browser.page;
    const nick = "popup" + Date.now() % 1000000;
    await page.goto(appUrl(server.url, opts, {ui:"v2:engraved", nick, autostart:"1", ai:"2", size:"small", seed:"7", manual:"1"}));
    const first = await until(page,s=>s.screen==="login"||s.screen==="play",90000,"login or play");
    if(first.screen==="login") await act(page,{kind:"login",nick,password:DEV_PASSWORD});
    await until(page,s=>s.screen==="play"&&s.turn!==null,90000,"running game");
    await waitPopup(page,p=>p?.state==="running"&&p.frames>10,"UI ready");
    await page.evaluate(({screen,args}) => window.__civ5test.ui2Intent({name:"window.open",args:{window:screen,...args}}),{screen,args});
    await waitPopup(page,p=>p.ui.windows.includes(screen),"open " + screen);
    await exercise(page,check);
    check("no browser errors",browser.errors.filter(e=>!e.includes("/pkg-mt/")).length===0);
    return {scenario:"ui2-screen-"+screen,pass:checks.every(c=>c.pass),checks};
  } finally { await browser.close(); }
}
export async function clickRequired(page,part) {
  const rects=await page.evaluate(part=>window.__civ5test.ui2Rects([part]),part);
  const rows=(rects??[]).filter(r=>r.keys && !r.node.includes("#") && r.w>0 && r.h>0).sort((a,b)=>a.y-b.y||a.x-b.x);
  if(rows.length) {const r=rows[0];await page.mouse.click(r.x+Math.min(60,r.w/2),r.y+r.h/2);return;}
  if (!await clickPart(page,part)) throw new Error("Missing enabled part: "+part);
}
