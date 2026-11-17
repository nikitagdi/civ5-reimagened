import {popupScenario,texts,waitPopup,clickRequired} from "./ui2-popup.mjs";
export async function run(ctx) {
 return popupScenario(ctx,"advisor",async(page,check)=>{
  await waitPopup(page,p=>p.texts.filter(t=>t.part?.startsWith("advisor.")&&t.part.endsWith(".role")).length===4,"four advisor roles");
  check("counsel or an honest unavailable reason",(await page.evaluate(()=>window.__civ5test.ui2())).texts.some(t=>t.part?.startsWith("advisor.")&&t.part.endsWith(".page"))||texts(await page.evaluate(()=>window.__civ5test.ui2()),"advisor.reason").length>0);
  await clickRequired(page,"advisor.close");
  await waitPopup(page,p=>!p.ui.windows.includes("advisor"),"closed counsel");
 });
}
