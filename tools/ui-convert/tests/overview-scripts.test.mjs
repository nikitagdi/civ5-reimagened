// Screen lifecycle behavior without a browser or a game core.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { test } from 'node:test';

function script(name, initial = {}, saved = {}, model = {}) {
  const values = { ...initial }, intents = [], events = new Map(), watches = new Map(), stateWatches = new Map();
  const on = (map, key, fn) => map.set(key, [...(map.get(key) || []), fn]);
  const ui = {
    on: (key, fn) => on(events, key, fn),
    intent: (name, args) => intents.push({ name, args }),
    state: { get: key => values[key], set: (key, value) => { values[key] = value; for (const fn of stateWatches.get(key) || []) fn(value); }, on: (key, fn) => on(stateWatches, key, fn) },
    model: { get: key => model[key], on: (key, fn) => on(watches, key, fn) },
    settings: { get: key => saved[key], set: (key, value) => { saved[key] = value; } },
  };
  const file = path.resolve(import.meta.dirname, '../../../skins/engraved/js', `${name}.js`);
  vm.runInNewContext(fs.readFileSync(file, 'utf8'), { ui }, { filename: `${name}.js` });
  return { values, intents, saved, event: key => { for (const fn of events.get(key) || []) fn(); }, change: (key, value) => { model[key] = value; for (const fn of watches.get(key) || []) fn(value); } };
}

test('Escape unwinds trade, espionage and victory local dialogs before closing', () => {
  const trade = script('trade_routes', { 'trade_routes.confirm': 4 });
  trade.event('trade_routes.back');
  assert.equal(trade.values['trade_routes.confirm'], -1);
  assert.equal(trade.intents.length, 0);
  trade.event('trade_routes.back');
  assert.equal(trade.intents[0].name, 'window.close');

  const spy = script('espionage', { 'espionage.ask': 2, 'espionage.confirm': 'coup', 'espionage.moving': 1 });
  spy.event('espionage.back');
  assert.equal(spy.values['espionage.ask'], -1);
  spy.event('espionage.back');
  assert.equal(spy.values['espionage.confirm'], '');
  assert.equal(spy.intents.length, 0);
  spy.event('espionage.back');
  assert.equal(spy.values['espionage.moving'], -1);
  assert.equal(spy.intents[0].name, 'window.close');

  const victory = script('victory', { 'victory.view': 'score' });
  victory.event('victory.back');
  assert.equal(victory.values['victory.view'], 'summary');
  assert.equal(victory.intents.length, 0);
  victory.event('victory.back');
  assert.equal(victory.intents[0].name, 'window.close');
});

test('demographics emits one replay mark per opening and a new popup token', () => {
  const model = { 'ui.windows.demographics.open': false, 'ui.windows.demographics': { a: 1 } };
  const demo = script('demographics', {}, {}, model);
  demo.event('ready');
  demo.change('ui.windows.demographics.open', true);
  demo.change('ui.windows.demographics.open', true);
  assert.equal(demo.intents.filter(i => i.name === 'screen.act').length, 1);
  assert.equal(demo.intents[1].args.b, 1);
  demo.change('ui.windows.demographics.open', false);
  demo.change('ui.windows.demographics.open', true);
  assert.equal(demo.intents.filter(i => i.name === 'screen.act').length, 2);
  assert.equal(demo.intents[3].args.b, 2);
});

test('economy and military restore and persist sorting preferences', () => {
  const economy = script('economy', { 'economy.tab': '', 'economy.sort': 'population', 'economy.sort_desc': false }, { 'economy.sort': 'gold', 'economy.sort_desc': true });
  economy.event('ready');
  assert.equal(economy.values['economy.sort'], 'gold');
  assert.equal(economy.values['economy.sort_desc'], true);
  economy.change('ui.windows.economy.a', 1);
  assert.equal(economy.saved['economy.tab'], 'happiness');
  const military = script('military', { 'military.sort': 'name', 'military.sort_desc': false }, { 'military.sort': 'moves' });
  military.event('ready');
  assert.equal(military.values['military.sort'], 'moves');
});
