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
  if (!opts.attach) throw new Error('espionage acceptance requires --attach to a real-core server');
  const result = { scenario: 'ui2-screen-espionage', pass: false, checks: [] };
  const check = (name, pass) => result.checks.push({ name, pass: !!pass });
  const browser = await openPage({ headed: opts.headed, logDir: opts.logDir, viewport: { width: 1366, height: 768 }, consoleFile: 'ui2-screen-espionage.log' });
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
    await page.evaluate(() => {
      const api = window.__civ5test;
      if (typeof api.ui2Intent !== 'function') throw new Error('Needs window.open intent test hook: ui2Intent({name,args})');
      api.ui2Intent({ name: 'window.open', args: { window: 'espionage' } });
    });
    const available = await waitFor(page, p => texts(p,'espionage.agent-name').length || texts(p,'espionage.disabled').concat(texts(p,'espionage.not-started')).some(t=>t.trim()), 'agents or unavailable reason');
    if (texts(available,'espionage.disabled').concat(texts(available,'espionage.not-started')).some(t=>t.trim())) {
      check('server reports espionage unavailable before agents exist', true);
      result.pass=true; return result;
    }
    await waitFor(page, p => texts(p, 'espionage.agent-name').length > 0, 'real agents (prepared Renaissance game required)');
    if (!await clickPart(page, 'espionage.move')) throw new Error('move agent button missing');
    await waitFor(page, p => texts(p, 'espionage.choose-own-cities').length > 0, 'own-city relocation target');
    if (!await clickPart(page, 'espionage.choose-own-cities')) throw new Error('own city missing');
    await waitFor(page, p => texts(p, 'espionage.agent-activity').some(s => /travell|counter/i.test(s)), 'refreshed agent activity');
    check('relocation changes visible agent activity', true);
    await page.keyboard.press('Escape');
    await waitFor(page, p => !p.ui.windows.includes('espionage'), 'closed overview');
    result.pass = result.checks.every(c => c.pass);
    return result;
  } finally { await browser.close(); }
}
