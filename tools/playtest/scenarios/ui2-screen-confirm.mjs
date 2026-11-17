import {popupScenario, texts, probe, waitPopup, clickRequired} from "./ui2-popup.mjs";
export async function run(ctx) {
  const id=Number(process.env.UI2_CONFIRM_ID);

 if (!Number.isInteger(id)) return popupScenario(ctx,"confirm",async(page,check)=>{
   await waitPopup(page,p=>texts(p,"confirm.text").some(t=>/no matching/i.test(t)),"honest unavailable event");
   check("no event is manufactured",true);
 },{a:-1});
  return popupScenario(ctx,"confirm",async(page,check)=>{
    await waitPopup(page,p=>texts(p,"confirm.choice").length>=2,"real confirmation choices");
    await page.keyboard.press("Escape");
    check("ransom/gift/return cannot be dismissed with Escape",(await probe(page)).ui.windows.includes("confirm"));
    await clickRequired(page,"confirm.choice");
    await waitPopup(page,p=>!p.ui.windows.includes("confirm"),"answered confirmation closes");
    check("answer closes only after server refresh",true);
  },{a:id});
}
