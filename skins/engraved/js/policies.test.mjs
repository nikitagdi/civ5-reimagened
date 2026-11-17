import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

test("policy cancel resolves local dialogs and ideology loss chains the choice window", () => {
  const state = new Map(), events = new Map(), subs = new Map(), intents = [];
  const ui = {
    state: {get: k => state.get(k), set: (k,v) => state.set(k,v), on: () => {}},
    model: {get: () => ({policies:{open:true}}), on: (k,f) => subs.set(k,[...(subs.get(k) || []),f])},
    settings: {get: () => null, set: () => {}},
    on: (k,f) => events.set(k,f), intent: (name,args) => intents.push({name,args}),
  };
  vm.runInNewContext(readFileSync(new URL("./policies.js",import.meta.url),"utf8"),{ui});
  const root = value => subs.get("policies").forEach(f => f(value));
  root({open_tab:"policies",ideology:{id:9}});
  state.set("policies.confirm",5);
  state.set("policies.tenet_level",1);
  events.get("policies.escape")();
  assert.equal(state.get("policies.confirm"),-1);
  assert.equal(intents.length,0);
  events.get("policies.escape")();
  assert.equal(state.get("policies.tenet_level"),0);
  assert.equal(intents.length,0);
  root({open_tab:"ideology",ideology:null});
  assert.deepEqual(intents.map(i => [i.name,i.args.window]),[["window.close","policies"],["window.open","ideology"]]);
});
