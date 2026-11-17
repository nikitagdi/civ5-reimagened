(function () {
 "use strict";
 let opened=false;
 ui.model.on("end_game",root=>{
  if(!root) {opened=false;return;}
  if(!opened) {opened=true;ui.state.set("end_game.tab","summary");}
 });
})();
