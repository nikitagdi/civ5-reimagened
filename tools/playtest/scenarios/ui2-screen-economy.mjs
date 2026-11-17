// Economy acceptance requires a real-core server and a freshly converted skin.
import { openPage, act, until, state, appUrl } from '../lib/browser.mjs';
import { DEV_PASSWORD } from '../lib/game.mjs';
import { FOUND_CITY } from '../lib/policy.mjs';
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
  if (!opts.attach) throw new Error('economy acceptance requires --attach to a real-core server');
  const result = { scenario: 'ui2-screen-economy', pass: false, checks: [] };
  const check = (name, pass) => result.checks.push({ name, pass: !!pass });
  const browser = await openPage({ headed: opts.headed, logDir: opts.logDir, viewport: { width: 1366, height: 768 }, consoleFile: 'ui2-screen-economy.log' });
  try {
    const page = browser.page;
    const nick = `overview${Date.now() % 1000000}`;
    await page.goto(appUrl(server.url, opts, { ui: 'v2:engraved', nick, autostart: '1', ai: '2', size: 'small', seed: '7', mode: 'simultaneous', manual: '1' }));
    const first = await until(page, s => s.screen === 'login' || s.screen === 'play', 90000, 'login or game');
    if (first.screen === 'login') await act(page, { kind: 'login', nick, password: DEV_PASSWORD });
    await until(page, s => s.screen === 'play' && s.turn !== null, 90000, 'game');
    await waitFor(page, p => p?.state === 'running' && p.frames > 10, 'v2');
    const settler = ((await state(page)).units ?? []).find(u => /settler/i.test(u.name));
    if (!settler) throw new Error('settler required');
    await act(page, { kind: 'selectUnit', id: settler.id });
    await act(page, { kind: 'unitAction', action: FOUND_CITY });
    const founded = await until(page, s => s.cities?.length > 0, 30000, 'first city');
    await page.keyboard.press('Escape');
    if (!await clickPart(page, 'top.menu.economy')) throw new Error('economy HUD entry missing');
    const p = await waitFor(page, p => texts(p, 'economy.city-name').length > 0, 'real city row');
    check('own city appears', texts(p, 'economy.city-name').includes(founded.cities[0].name));
    if (!await clickPart(page, 'economy.tab-happiness')) throw new Error('happiness tab missing');
    await waitFor(page, p => texts(p, 'economy').includes('Happiness'), 'happiness data');
    await clickPart(page, 'economy.tab-general');
    if (!await clickPart(page, 'economy.production')) throw new Error('production cell missing');
    await waitFor(page, p => p.ui.windows.includes('production') && texts(p, 'production.head').length > 0, 'city production chooser');
    check('production cell opens the city chooser', true);
    await page.keyboard.press('Escape');
    await waitFor(page, p => !p.ui.windows.includes('production'), 'closed chooser');
    result.pass = result.checks.every(c => c.pass);
    return result;
  } finally { await browser.close(); }
}
