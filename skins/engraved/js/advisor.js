(function () {
 "use strict";
 let opened=false;
 ui.model.on("advisor",root=>{
  if(!root) {opened=false;return;}
  for(const role of root.roles) {
   const path="advisor."+role.key;
   const current=ui.state.get(path);
   ui.state.set(path,!opened?0:Math.max(0,Math.min(current,role.pages.length-1)));
  }
  opened=true;
 });
})();
