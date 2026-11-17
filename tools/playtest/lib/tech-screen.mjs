import { openPage, act, until, appUrl } from "./browser.mjs";
import { DEV_PASSWORD } from "./game.mjs";
export const texts = (p, part) => (p?.texts ?? []).filter(t => t.part === part).map(t => t.text);
export async function waitTech(page, predicate, description) {
  const deadline = Date.now() + 30000;
  while (Date.now() < deadline) {
    const p = await page.evaluate(() => window.__civ5test.ui2());
    if (predicate(p)) return p;
    await new Promise(resolve => setTimeout(resolve, 150));
  }
  throw new Error(`Timed out waiting for ${description}`);
}
export async function techBrowser(server, opts, name) {
  if (!opts.attach) throw new Error(`${name} requires --attach to a real-core server`);
  const browser = await openPage({headed:opts.headed,logDir:opts.logDir,viewport:{width:1366,height:768},consoleFile:`${name}.log`});
  try {
    const nick = `techtest${Date.now() % 1000000}`;
    await browser.page.goto(appUrl(server.url,opts,{ui:"v2:engraved",nick,autostart:"1",ai:"2",size:"small",seed:"7",manual:"1"}));
    const first = await until(browser.page,s=>s.screen==="login" || s.screen==="play",90000,"login or game");
    if (first.screen === "login") await act(browser.page,{kind:"login",nick,password:DEV_PASSWORD});
    await until(browser.page,s=>s.screen==="play" && s.turn!==null,90000,"real game");
    await waitTech(browser.page,p=>p?.state==="running" && p.frames>10,"UI v2");
    return browser;
  } catch (e) { await browser.close(); throw e; }
}
export async function openTechWindow(page, window, a = 0, b = 0) {
  // SIMPLIFIED(SIMP-347): no existing browser test-port action dispatches arbitrary UI intents; the lead must expose this hook or route the game event.
  await page.evaluate(({window: name,a,b}) => {
    if (typeof globalThis.__civ5test.ui2Intent !== "function") throw new Error("Needs host test hook: ui2Intent(name, args)");
    globalThis.__civ5test.ui2Intent("window.open", {window:name,a,b});
  }, {window,a,b});
}
