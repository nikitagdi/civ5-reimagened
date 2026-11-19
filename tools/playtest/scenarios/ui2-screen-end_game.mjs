import {popupScenario,texts,probe,waitPopup,clickRequired} from "./ui2-popup.mjs";
import {until} from "../lib/browser.mjs";
export async function run(ctx) {
 return popupScenario(ctx,"end_game",async(page,check)=>{
  await waitPopup(page,p=>texts(p,"end-game.score").length>0,"real score rows");
  await clickRequired(page,"end-game.ranking");
  await waitPopup(page,p=>texts(p,"end-game.rank-row").length>0,"historic ranking");
  await page.keyboard.press("Escape");
  check("Escape cannot continue an ongoing or lost match",(await probe(page)).ui.windows.includes("end_game"));
  // Exit is asked once more, then the session takes the seat to the lobby.
  await clickRequired(page,"end-game.exit");
  await waitPopup(page,p=>texts(p,"end-game.exit-yes").length>0,"exit asked");
  await clickRequired(page,"end-game.exit-yes");
  await until(page,s=>s.screen==="lobby",60000,"the lobby after exit");
  check("exit leaves the game for the lobby",true);
 });
}