import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

test("religion draft survives refresh, clears stale choices and packs signed arguments", () => {
  const state = new Map([["religion.name", ""], ["religion.symbol", -1]]);
  const events = new Map(), roots = new Map(), intents = [];
  const ui = {
    state: { get: k => state.get(k), set: (k,v) => state.set(k,v), on: () => {} },
    model: { on: (k,f) => roots.set(k,f) }, on: (k,f) => events.set(k,f), intent: (name,args) => intents.push({name,args}),
  };
  vm.runInNewContext(readFileSync(new URL("./religion.js", import.meta.url), "utf8"), {ui});
  roots.get("religion")(null);
  const root = {mode: "found", city: {id:4}, enabled: true, custom_name_available: false,
    symbols: [{id:5, name:"Religion", taken:false}], slots: [
      {key:"founder", required:true, options:[{id:1,name:"Founder"}]},
      {key:"follower", required:true, options:[{id:2,name:"Follower"}]},
    ]};
  roots.get("religion")(root);
  events.get("religion.symbol")(5);
  events.get("religion.pick")("founder",1);
  events.get("religion.pick")("follower",2);
  roots.get("religion")({...root});
  assert.equal(state.get("religion.ready"), true);
  events.get("religion.submit")();
  assert.equal(intents.length,1);
  assert.equal(intents[0].args.c,131073);
  assert.equal(intents[0].args.d,-1);
  assert.equal(intents[0].args.text,"");
  roots.get("religion")({...root, slots:[root.slots[0], {...root.slots[1], options:[]}]});
  assert.equal(state.get("religion.pick.follower"),65535);
  assert.equal(state.get("religion.ready"),false);
  events.get("religion.submit")();
  assert.equal(intents.length,1);
});
