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

// The drafts and the store of the options are the host's (`ui2_actions::options`, its tests): the script keeps no
// option of its own and only drops the drafts of a menu closed while its options were open.
test("the menu opens on its details and a menu closed with open options drops their drafts",()=>{
  const s=skin("game_menu");
  s.model({});assert.equal(s.values.get("menu.options"),false);assert.equal(s.values.get("menu.panel"),"details");
  s.values.set("menu.options",true);s.model({});
  assert.equal(s.values.get("menu.options"),true,"a refresh of the open menu keeps the options");
  s.model(null);
  assert.deepEqual(s.intents.map(i=>i.name),["options.cancel"]);
  assert.equal(s.values.get("menu.options"),false);
  s.model({});s.model(null);
  assert.equal(s.intents.length,1,"no drafts to drop");
  assert.equal(s.settings.size,0,"no skin setting");
});

test("advisor pages clamp after refresh and reset on reopen",()=>{
  const s=skin("advisor");
  s.model({roles:[{key:"economic",pages:[{},{}]}]});
  s.values.set("advisor.economic",1);s.model({roles:[{key:"economic",pages:[{}]}]});
  assert.equal(s.values.get("advisor.economic"),0);
  s.model(null);s.model({roles:[{key:"economic",pages:[]}]});
  assert.equal(s.values.get("advisor.economic"),0);
});
