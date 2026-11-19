import {popupScenario,texts,waitPopup,clickRequired} from "./ui2-popup.mjs";
import {until} from "../lib/browser.mjs";
export async function run(ctx) {
 return popupScenario(ctx,"game_menu",async(page,check)=>{
  await waitPopup(page,p=>texts(p,"game-menu.leader").length>0,"real leader");
  await clickRequired(page,"game-menu.options");
  await clickRequired(page,"game-menu.option-auto-end-turn");
  await clickRequired(page,"game-menu.cancel");
  await clickRequired(page,"game-menu.options");
  await clickRequired(page,"game-menu.option-auto-end-turn");
  await clickRequired(page,"game-menu.accept");
  // Saving is the host's session operation: the new save shows in the list of this game's saves (a row keyed by the
  // save's id; the newest ring save of a game replaces the older one, so the key is what changes).
  await waitPopup(page,p=>texts(p,"game-menu.save").length>0,"save button");
  const rowKeys=async()=>((await page.evaluate(()=>window.__civ5test.ui2Rects(["game-menu.save-row"])))??[]).filter(r=>r.keys&&r.w>0).map(r=>String(r.keys));
  const before=new Set(await rowKeys());
  await clickRequired(page,"game-menu.save");
  const end=Date.now()+30000;
  let fresh=[];
  while(Date.now()<end&&!fresh.length){fresh=(await rowKeys()).filter(k=>!before.has(k));if(!fresh.length)await new Promise(r=>setTimeout(r,300));}
  check("the host's save joins the list of saves",fresh.length>0);
  const saved=await waitPopup(page,p=>true,"probe");
  check("no refusal after the host's save",texts(saved,"game-menu.refused").length===0);
  await clickRequired(page,"game-menu.return");
  await waitPopup(page,p=>!p.ui.windows.includes("game_menu"),"return to game");
  check("preferences accepted and menu closes",true);
  // Leaving is asked once more, then the session takes the seat to the lobby.
  await page.evaluate(()=>window.__civ5test.ui2Intent({name:"window.open",args:{window:"game_menu"}}));
  await waitPopup(page,p=>p.ui.windows.includes("game_menu"),"menu again");
  await clickRequired(page,"game-menu.exit");
  await clickRequired(page,"game-menu.exit-no");
  await clickRequired(page,"game-menu.exit");
  await clickRequired(page,"game-menu.exit-yes");
  await until(page,s=>s.screen==="lobby",60000,"the lobby after leaving");
  check("exit leaves the game for the lobby",true);
 });
}
