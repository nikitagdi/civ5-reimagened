import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
test("request-mode research closes only after a matching server refresh", () => {
  let root={mode:"normal",opened_by:"request",queue:[],techs:[{id:1,key:"TECH_A",state:"available",progress:0}]};
  const handlers={},listeners={},sent=[];
  const ui={model:{get:()=>root,on:(k,fn)=>{listeners[k]=fn;}},on:(k,fn)=>{handlers[k]=fn;},settings:{get:()=>true},intent:(...x)=>sent.push(x)};
  vm.runInNewContext(readFileSync(new URL("../../../skins/engraved/js/techs.js",import.meta.url),"utf8"),{ui});
  handlers["techs.track"](1,0); listeners.techs(); assert.equal(sent.length,0);
  root.techs[0].state="current"; listeners.techs(); assert.equal(sent[0][0],"window.close");
  sent.length=0; handlers["techs.track"](1,1); listeners.techs(); assert.equal(sent.length,0);
  root={...root,opened_by:"menu"}; handlers["techs.track"](1,0); listeners.techs(); assert.equal(sent.length,0);
  root=null; listeners.techs();
});
