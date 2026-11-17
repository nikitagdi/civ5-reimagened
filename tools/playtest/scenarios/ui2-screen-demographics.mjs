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
  if (!opts.attach) throw new Error('demographics acceptance requires --attach to a real-core server');
  const result = { scenario: 'ui2-screen-demographics', pass: false, checks: [] };
  const check = (name, pass) => result.checks.push({ name, pass: !!pass });
  const browser = await openPage({ headed: opts.headed, logDir: opts.logDir, viewport: { width: 1366, height: 768 }, consoleFile: 'ui2-screen-demographics.log' });
  try {
    const page = browser.page;
    const nick = `overview${Date.now() % 1000000}`;
    await page.goto(appUrl(server.url, opts, { ui: 'v2:engraved', nick, autostart: '1', ai: '2', size: 'small', seed: '7', mode: 'simultaneous', manual: '1' }));
    const first = await until(page, s => s.screen === 'login' || s.screen === 'play', 90000, 'login or game');
    if (first.screen === 'login') await act(page, { kind: 'login', nick, password: DEV_PASSWORD });
    await until(page, s => s.screen === 'play' && s.turn !== null, 90000, 'game');
    await waitFor(page, p => p?.state === 'running' && p.frames > 10, 'v2');
    if (!await clickPart(page, 'top.menu.demographics')) throw new Error('demographics HUD entry missing');
    const initial = await waitFor(page, p => texts(p, 'demographics.own-value').length === 8, 'eight real demographic rows');
    const before = texts(initial, 'demographics.own-value')[0];
    await page.keyboard.press('Escape');
    const settler = ((await state(page)).units ?? []).find(u => /settler/i.test(u.name));
    if (!settler) throw new Error('settler required');
    await act(page, { kind: 'selectUnit', id: settler.id });
    await act(page, { kind: 'unitAction', action: FOUND_CITY });
    await until(page, s => s.cities?.length > 0, 30000, 'first city');
    await page.keyboard.press('Escape');
    await clickPart(page, 'top.menu.demographics');
    await waitFor(page, p => texts(p, 'demographics.own-value').length === 8 && texts(p, 'demographics.own-value')[0] !== before, 'population after founding');
    check('demographics recalculates population after founding', true);
    await page.keyboard.press('Enter');
    await waitFor(page, p => !p.ui.windows.includes('demographics'), 'Enter closes overview');
    result.pass = result.checks.every(c => c.pass);
    return result;
  } finally { await browser.close(); }
}
