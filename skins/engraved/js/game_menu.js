(function () {
 "use strict";
 // The options are the client's (`ui.options`, `ui.hotkeys`: drafts kept by the host until accepted); the menu
 // opens on its details, and a menu closed while the options were open drops their drafts.
 let opened=false;
 ui.model.on("game_menu",root=>{
  if(!root) {if(opened&&ui.state.get("menu.options")) ui.intent("options.cancel",{});opened=false;ui.state.set("menu.options",false);return;}
  if(!opened) {opened=true;ui.state.set("menu.options",false);ui.state.set("menu.panel","details");}
 });
})();
