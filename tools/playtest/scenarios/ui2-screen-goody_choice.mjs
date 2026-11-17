import {popupScenario,texts,probe,waitPopup,clickRequired} from "./ui2-popup.mjs";
export async function run(ctx) {
 const id=Number(process.env.UI2_GOODY_ID);
 if(!Number.isInteger(id)) throw new Error("Seed a real pending ruin event and set UI2_GOODY_ID");
 return popupScenario(ctx,"goody_choice",async(page,check)=>{
  await waitPopup(page,p=>texts(p,"goody-choice.reward").length>0,"eligible real rewards");
  await page.keyboard.press("Escape");
  check("Escape cannot skip reward",(await probe(page)).ui.windows.includes("goody_choice"));
  await clickRequired(page,"goody-choice.reward");
  await clickRequired(page,"goody-choice.take");
  await waitPopup(page,p=>!p.ui.windows.includes("goody_choice"),"reward applied");
  check("reward answer closes window",true);
 },{a:id});
}
