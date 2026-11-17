import {popupScenario,texts,probe,waitPopup,clickRequired} from "./ui2-popup.mjs";
export async function run(ctx) {
 return popupScenario(ctx,"end_game",async(page,check)=>{
  await waitPopup(page,p=>texts(p,"end-game.score").length>0,"real score rows");
  await clickRequired(page,"end-game.ranking");
  await waitPopup(page,p=>texts(p,"end-game.rank-row").length>0,"historic ranking");
  await page.keyboard.press("Escape");
  check("Escape cannot continue an ongoing or lost match",(await probe(page)).ui.windows.includes("end_game"));
 });
}
