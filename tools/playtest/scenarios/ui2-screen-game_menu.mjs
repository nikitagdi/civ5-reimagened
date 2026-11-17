import {popupScenario,texts,waitPopup,clickRequired} from "./ui2-popup.mjs";
export async function run(ctx) {
 return popupScenario(ctx,"game_menu",async(page,check)=>{
  await waitPopup(page,p=>texts(p,"game-menu.leader").length>0,"real leader");
  await clickRequired(page,"game-menu.options");
  await clickRequired(page,"game-menu.option-auto-end-turn");
  await clickRequired(page,"game-menu.cancel");
  await clickRequired(page,"game-menu.options");
  await clickRequired(page,"game-menu.option-auto-end-turn");
  await clickRequired(page,"game-menu.accept");
  await clickRequired(page,"game-menu.return");
  await waitPopup(page,p=>!p.ui.windows.includes("game_menu"),"return to game");
  check("preferences accepted and menu closes",true);
 });
}
