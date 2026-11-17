import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";

function skin(name) {
  const values=new Map(), settings=new Map(), listeners=new Map(), handlers=new Map(), intents=[];
  const ui={
    state:{get:k=>values.get(k),set:(k,v)=>values.set(k,v)},
    settings:{get:k=>settings.get(k),set:(k,v)=>settings.set(k,v)},
    model:{on:(k,fn)=>listeners.set(k,fn)},on:(k,fn)=>handlers.set(k,fn),
    intent:(name,args)=>intents.push({name,args}),
  };
  vm.runInNewContext(fs.readFileSync(new URL(`../../../skins/engraved/js/${name}.js`,import.meta.url),"utf8"),{ui});
  return {values,settings,intents,model:root=>listeners.get(name)(root),call:k=>handlers.get(k)()};
}

test("confirmation preferences reset on a new id and only acknowledgement closes",()=>{
  const s=skin("confirm");
  s.model({id:4,resolved:false});s.values.set("confirm.dont_warn",true);
  s.model({id:4,resolved:false});assert.equal(s.values.get("confirm.dont_warn"),true);
  s.model({id:5,resolved:false});assert.equal(s.values.get("confirm.dont_warn"),false);
  assert.equal(s.intents.length,0);
  s.model({id:5,resolved:true});assert.equal(s.intents[0].args.window,"confirm");
});

test("ruin selection survives refresh and resets between events and openings",()=>{
  const s=skin("goody_choice");
  s.model({id:4});s.values.set("goody.sel",3);s.model({id:4});
  assert.equal(s.values.get("goody.sel"),3);
  s.model({id:5});assert.equal(s.values.get("goody.sel"),-1);
  s.values.set("goody.sel",2);s.model(null);s.model({id:5});assert.equal(s.values.get("goody.sel"),-1);
});

test("options Accept persists a draft while Cancel and reopen restore saved values",()=>{
  const s=skin("game_menu");
  s.model({});s.call("menu.open");
  s.values.set("opt.music",30);s.call("menu.cancel");assert.equal(s.values.get("opt.music"),70);
  assert.equal(s.settings.size,0);
  s.call("menu.open");s.values.set("opt.music",40);s.values.set("opt.auto_end_turn",true);
  s.call("menu.accept");assert.equal(s.settings.get("options.music"),40);
  s.model(null);s.model({});assert.equal(s.values.get("opt.music"),40);
  assert.equal(s.values.get("opt.auto_end_turn"),true);
  assert.equal(s.values.get("menu.options"),false);
});

test("advisor pages clamp after refresh and reset on reopen",()=>{
  const s=skin("advisor");
  s.model({roles:[{key:"economic",pages:[{},{}]}]});
  s.values.set("advisor.economic",1);s.model({roles:[{key:"economic",pages:[{}]}]});
  assert.equal(s.values.get("advisor.economic"),0);
  s.model(null);s.model({roles:[{key:"economic",pages:[]}]});
  assert.equal(s.values.get("advisor.economic"),0);
});
