(function () {
 "use strict";
 let id=null;
 ui.model.on("goody_choice",root=>{
  if(!root) {id=null;ui.state.set("goody.sel",-1);return;}
  if(id!==root.id) {id=root.id;ui.state.set("goody.sel",-1);}
  if(root.resolved) ui.intent("window.close",{"window":"goody_choice"});
 });
})();
