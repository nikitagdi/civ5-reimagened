// Economy acceptance requires a real-core server and a freshly converted skin.
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
  if (!opts.attach) throw new Error('military acceptance requires --attach to a real-core server');
  const result = { scenario: 'ui2-screen-military', pass: false, checks: [] };
  const check = (name, pass) => result.checks.push({ name, pass: !!pass });
  const browser = await openPage({ headed: opts.headed, logDir: opts.logDir, viewport: { width: 1366, height: 768 }, consoleFile: 'ui2-screen-military.log' });
  try {
    const page = browser.page;
    const nick = `overview${Date.now() % 1000000}`;
    await page.goto(appUrl(server.url, opts, { ui: 'v2:engraved', nick, autostart: '1', ai: '2', size: 'small', seed: '7', mode: 'simultaneous', manual: '1' }));
    const first = await until(page, s => s.screen === 'login' || s.screen === 'play', 90000, 'login or game');
    if (first.screen === 'login') await act(page, { kind: 'login', nick, password: DEV_PASSWORD });
    await until(page, s => s.screen === 'play' && s.turn !== null, 90000, 'game');
    await waitFor(page, p => p?.state === 'running' && p.frames > 10, 'v2');
    await page.evaluate(() => window.__civ5test.ui2Intent({ name: 'window.open', args: { window: 'military' } }));
    const p = await waitFor(page, p => texts(p, 'military.unit-name').length > 0, 'real combat units');
    const name = texts(p, 'military.unit-name')[0];
    const expected = ((await state(page)).units ?? []).find(u => u.name === name);
    if (!expected) throw new Error('military row has no own unit');
    if (!await clickPart(page, 'military.unit-row')) throw new Error('unit row missing');
    // The v2 selection is local to the UI model (`ui.selection`); the unit panel shows the selected unit's name.
    // A full-screen window suppresses the HUD: close it, then the unit panel shows the selected unit.
    await page.keyboard.press('Escape');
    await waitFor(page, p => texts(p, 'unit.title').includes(name), 'unit panel shows the selected unit');
    check('clicking a row selects its unit', true);
    await page.keyboard.press('Escape');
    await waitFor(page, p => !p.ui.windows.includes('military'), 'closed overview');
    result.pass = result.checks.every(c => c.pass);
    return result;
  } finally { await browser.close(); }
}
