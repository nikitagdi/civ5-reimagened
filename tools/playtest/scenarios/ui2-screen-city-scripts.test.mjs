import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
function script(name, initial = {}) {
  const state = new Map(Object.entries(initial));
  const handlers = new Map(), subscriptions = new Map(), intents = [];
  const roots = new Map();
  const ui = {
    on: (name, fn) => handlers.set(name, fn),
    model: {get: name => roots.get(name), on: (name,fn) => subscriptions.set(name,fn)},
    state: {get: path => state.get(path), set: (path,value) => state.set(path,value), on() {}},
    settings: {get() {},set() {}}, intent: (name,args) => intents.push({name,args}),
  };
  vm.runInNewContext(readFileSync(new URL(`../../../skins/engraved/js/${name}.js`,import.meta.url),"utf8"),{ui});
  return {state,handlers,intents,root(name,value) {roots.set(name,value);subscriptions.get(name)?.(value);}};
}
test("city production advances only after the refreshed queue changes", () => {
  const s=script("city",{"city.queue_mode":0,"city.repeat_order":false});
  const city={city:7,choose_mode:true,next_choose_city:8,production:{queue:[]}};
  s.root("city",city);s.handlers.get("city.push")(2);
  assert.equal(s.intents[0].name,"screen.act");
  s.root("city",city);assert.equal(s.intents.length,1);
  s.root("city",{...city,production:{queue:[{slot:0,item:2}]}});
  assert.equal(s.intents[1].name,"window.open");assert.equal(s.intents[1].args.a,8);
  s.state.set("city.sell",4);s.root("city",null);assert.equal(s.state.get("city.sell"),-1);
});
test("reward confirmation closes only after native quota decreases", () => {
  const s=script("free_great_person");
  const reward={mode:"free",mode_id:0,remaining:1,people:[{unit:2,available:true}]};
  s.root("free_great_person",reward);s.handlers.get("free_gp.confirm")(2);
  s.root("free_great_person",reward);assert.equal(s.intents.length,1);
  s.root("free_great_person",{...reward,remaining:0});assert.equal(s.intents[1].name,"window.close");
});
test("rename validates reserved symbols and Unicode character counts", () => {
  const s=script("city_rename");
  for (const [name,valid] of [["New city",true],["Bad/name",false],["ab",false],["a b",false],["é".repeat(15),true],["é".repeat(16),false]]) {
    s.state.set("city_rename.draft",name);s.handlers.get("city_rename.validate")();
    assert.equal(s.state.get("city_rename.valid"),valid);
  }
});
test("city sorting reverses a column and resets direction for another", () => {
  const s=script("city_list",{"city_list.sort":"population","city_list.desc":true});
  s.handlers.get("city_list.sort")("population");assert.equal(s.state.get("city_list.desc"),false);
  s.handlers.get("city_list.sort")("population");assert.equal(s.state.get("city_list.desc"),true);
  s.handlers.get("city_list.sort")("name");assert.equal(s.state.get("city_list.desc"),false);
});
