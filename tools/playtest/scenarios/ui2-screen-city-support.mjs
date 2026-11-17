import { openPage, act, until, state, appUrl } from "../lib/browser.mjs";
import { DEV_PASSWORD } from "../lib/game.mjs";
import { FOUND_CITY } from "../lib/policy.mjs";
export { clickPart } from "./ui2-v1.mjs";
export const texts = (p, part) => (p?.texts || []).filter(t => t.part === part).map(t => t.text);
export const probe = page => page.evaluate(() => window.__civ5test.ui2());
export async function waitFor(page, predicate, description, timeout = 30000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    const p = await probe(page);
    if (predicate(p)) return p;
    await new Promise(resolve => setTimeout(resolve, 150));
  }
  throw new Error(`Timed out waiting for ${description}`);
}
export async function openScreen(page, window, a = 0, b = 0) {
  await page.evaluate(async args => {
    if (typeof window.__civ5test.ui2Intent !== "function") throw new Error("Needs test-port ui2Intent({name,args}) to route window.open through UI actions");
    await window.__civ5test.ui2Intent({name: "window.open", args});
  }, {window, a, b});
  return waitFor(page, p => p?.ui?.windows?.includes(window), `${window} window`);
}
export async function scenario(context, name, body) {
  const {server, opts} = context;
  if (!opts.attach) throw new Error(`${name} requires --attach to a real-core server`);
  const browser = await openPage({headed: opts.headed, logDir: opts.logDir, viewport: {width:1366,height:768}, consoleFile: `${name}.log`});
  const checks = [];
  try {
    const page = browser.page;
    const nick = `citytest${Date.now() % 1000000}`;
    await page.goto(appUrl(server.url, opts, {ui:"v2:engraved",nick,autostart:"1",ai:"2",size:"small",seed:"7",mode:"simultaneous",manual:"1"}));
    const initial = await until(page, s => s.screen === "login" || s.screen === "play",90000,"login or game");
    if (initial.screen === "login") await act(page,{kind:"login",nick,password:DEV_PASSWORD});
    await until(page,s => s.screen === "play" && s.turn !== null,90000,"running game");
    await waitFor(page,p => p?.state === "running" && p.frames > 10,"running UI");
    let s = await state(page);
    if (!s.cities?.length) {
      const unit = s.units.find(u => /settler/i.test(u.name));
      if (!unit) throw new Error("Needs a settler or an owned city");
      await act(page,{kind:"selectUnit",id:unit.id});
      await act(page,{kind:"unitAction",action:FOUND_CITY});
      s = await until(page,s => s.cities?.length > 0,30000,"founded city");
    }
    await page.evaluate(() => window.__civ5test.ui2Intent({name: "window.close", args: {window: "city"}}));
    const check = (label, pass) => { checks.push({name:label,pass:!!pass}); if (!pass) throw new Error(label); };
    await body({page,city:s.cities[0].id,check});
    return {scenario:name,pass:checks.every(c => c.pass),checks};
  } finally { await browser.close(); }
}
