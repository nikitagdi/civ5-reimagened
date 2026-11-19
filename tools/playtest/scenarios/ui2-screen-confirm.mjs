import {popupScenario, texts, probe, waitPopup, clickRequired} from "./ui2-popup.mjs";
import {until} from "../lib/browser.mjs";
export async function run(ctx) {
  const id=Number(process.env.UI2_CONFIRM_ID);
  // A server started with CIV5_TEST_POPUPS=0 CIV5_TEST_POPUP_LIST=BUTTONPOPUP_CONFIRM_MENU: "yes" answers the question,
  // then the session takes the seat to the lobby (`screen.act` with `then: 'leave'`).
  if (process.env.UI2_CONFIRM_EXIT) return popupScenario(ctx,"confirm",async(page,check)=>{
    await waitPopup(page,p=>texts(p,"confirm.leave").length===1&&texts(p,"confirm.choice").length===1,"the exit question with yes and no");
    await clickRequired(page,"confirm.leave");
    await until(page,s=>s.screen==="lobby",60000,"the lobby after yes");
    check("yes on the exit question leaves for the lobby",true);
  },{a:-1});

 // No id at all: a negative id now names a unit to delete, so the "nothing pending" case sends none.
 if (!Number.isInteger(id)) return popupScenario(ctx,"confirm",async(page,check)=>{
   await waitPopup(page,p=>texts(p,"confirm.text").some(t=>/no matching/i.test(t)),"honest unavailable event");
   check("no event is manufactured",true);
 });
  return popupScenario(ctx,"confirm",async(page,check)=>{
    await waitPopup(page,p=>texts(p,"confirm.choice").length>=2,"real confirmation choices");
    await page.keyboard.press("Escape");
    check("ransom/gift/return cannot be dismissed with Escape",(await probe(page)).ui.windows.includes("confirm"));
    await clickRequired(page,"confirm.choice");
    await waitPopup(page,p=>!p.ui.windows.includes("confirm"),"answered confirmation closes");
    check("answer closes only after server refresh",true);
  },{a:id});
}
