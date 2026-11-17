import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
test("research panel remembers observed completion and toggles the technology window", () => {
  const state = {"tech_panel.last_id": -1};
  const model = {"research.current": {id:1,name:"Pottery"},hud:{research:{techs:[{id:1,status:1}]}}};
  const handlers = {}, listeners = {}, intents = [];
  const ui = {state:{get:k=>state[k],set:(k,v)=>{state[k]=v;}},model:{get:k=>model[k],on:(k,f)=>{listeners[k]=f;}},on:(k,f)=>{handlers[k]=f;},intent:(...args)=>intents.push(args)};
  vm.runInNewContext(readFileSync(new URL("../../../skins/engraved/js/tech_panel.js",import.meta.url),"utf8"),{ui});
  handlers.ready(); assert.equal(state["tech_panel.last_name"],"Pottery");
  model["research.current"]=null; model.hud.research.techs[0].status=0; listeners.hud();
  assert.equal(state["tech_panel.done"],true);
  handlers["tech_panel.tree"](); assert.deepEqual(intents[0][0],"window.open");
  model["ui.windows.techs.open"]=true; handlers["tech_panel.tree"](); assert.equal(intents[1][0],"window.close");
  model.hud=null; listeners.hud(); assert.equal(state["tech_panel.done"],false);
});
