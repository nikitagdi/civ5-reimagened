import {popupScenario,texts,waitPopup,clickRequired} from "./ui2-popup.mjs";
import {act,state,until} from "../lib/browser.mjs";
import {FOUND_CITY} from "../lib/policy.mjs";
export async function run(ctx) {
 return popupScenario(ctx,"notification_log",async(page,check)=>{
  if(!(await state(page)).cities?.length) {
   const settler=((await state(page)).units??[]).find(u=>/settler/i.test(u.name));
   if(!settler) throw new Error("A settler is required to create real city notifications");
   await act(page,{kind:"selectUnit",id:settler.id});
   await until(page,s=>s.selectedUnit===settler.id,10000,"selected settler");
   await act(page,{kind:"unitAction",action:FOUND_CITY});
   await until(page,s=>(s.cities?.length??0)>0,30000,"founded city");
  }
  await waitPopup(page,p=>texts(p,"notification-log.item").length>0,"real city notification rows");
  await clickRequired(page,"notification-log.item");
  await waitPopup(page,p=>!p.ui.windows.includes("notification_log"),"notification activated");
  check("activating a real notification closes the log",true);
 });
}
