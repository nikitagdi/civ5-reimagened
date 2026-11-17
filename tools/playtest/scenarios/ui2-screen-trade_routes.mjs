// Economy acceptance requires a real-core server and a freshly converted skin.
import fs from 'node:fs';
import { openPage, act, until, state, appUrl } from '../lib/browser.mjs';
import { DEV_PASSWORD } from '../lib/game.mjs';
import { clickPart } from './ui2-v1.mjs';

const texts = (p, part) => (p?.texts ?? []).filter(t => t.part === part).map(t => t.text);
async function waitFor(page, test, label) {
  const end = Date.now() + 30000;
  while (Date.now() < end) {
    const p = await page.evaluate(() => window.__civ5test.ui2());
    if (test(p)) return p;
    await new Promise(r => setTimeout(r, 150));
  }
  throw new Error(`Timed out: ${label}`);
}

export async function run({ server, opts }) {
  if (!opts.attach) throw new Error('trade_routes acceptance requires --attach to a real-core server');
  const result = { scenario: 'ui2-screen-trade_routes', pass: false, checks: [] };
  const check = (name, pass) => result.checks.push({ name, pass: !!pass });
  const browser = await openPage({ headed: opts.headed, logDir: opts.logDir, viewport: { width: 1366, height: 768 }, consoleFile: 'ui2-screen-trade_routes.log' });
  try {
    const page = browser.page;
    const nick = opts.lateNick || `overview${Date.now() % 1000000}`;
    if (opts.lateToken) {
      const token = fs.readFileSync(opts.lateToken, 'utf8').trim();
      await page.addInitScript(([name, value]) => {
        localStorage.setItem('civ5.nick', name);
        localStorage.setItem(`civ5.token.${name}`, value);
      }, [nick, token]);
    }
    const query = opts.lateUrl ? { ui: 'v2:engraved', nick } : { ui: 'v2:engraved', nick, autostart: '1', ai: '2', size: 'small', seed: '7', mode: 'simultaneous', manual: '1' };
    await page.goto(appUrl(opts.lateUrl || server.url, opts, query));
    const first = await until(page, s => s.screen === 'login' || s.screen === 'play', 90000, 'login or game');
    if (first.screen === 'login') await act(page, { kind: 'login', nick, password: DEV_PASSWORD });
    await until(page, s => s.screen === 'play' && s.turn !== null, 90000, 'game');
    await waitFor(page, p => p?.state === 'running' && p.frames > 10, 'v2');
    // The lead must expose a window-intent test hook or wire the HUD opener.
    await page.evaluate(() => {
      const api = window.__civ5test;
      if (typeof api.ui2Intent !== 'function') throw new Error('Needs window.open intent test hook: ui2Intent({name,args})');
      api.ui2Intent({ name: 'window.open', args: { window: 'trade_routes' } });
    });
    const initial = await waitFor(page, p => texts(p, 'trade-routes.used').length > 0, 'trade overview');
    const used = Number(texts(initial, 'trade-routes.used')[0]);
    check('registered trade overview opens', true);
    // A supplied late-game save with a trade unit exercises the real main action.
    const own = ((await state(page)).units ?? []).find(u => /caravan|cargo ship/i.test(u.name));
    if (!own) {
      check('no trade unit: overview has no chooser', !texts(initial, 'trade-routes.destination').length);
      result.pass = result.checks.every(c => c.pass); return result;
    }
    await page.evaluate(id => window.__civ5test.ui2Intent({ name: 'window.open', args: { window: 'trade_routes', a: 0, b: id } }), own.id);
    await waitFor(page, p => texts(p, 'trade-routes.destination').length > 0, 'legal destinations');
    if (!await clickPart(page, 'trade-routes.destination')) throw new Error('destination row missing');
    if (!await clickPart(page, 'trade-routes.establish')) throw new Error('confirmation missing');
    await page.evaluate(() => window.__civ5test.ui2Intent({ name: 'window.open', args: { window: 'trade_routes' } }));
    await waitFor(page, p => Number(texts(p, 'trade-routes.used')[0]) > used, 'established route');
    check('establish creates a visible own route', true);
    await page.keyboard.press('Escape');
    await waitFor(page, p => !p.ui.windows.includes('trade_routes'), 'closed overview');
    result.pass = result.checks.every(c => c.pass);
    return result;
  } finally { await browser.close(); }
}
