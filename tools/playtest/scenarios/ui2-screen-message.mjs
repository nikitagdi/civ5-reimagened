import {popupScenario,texts,waitPopup,clickRequired} from "./ui2-popup.mjs";
export async function run(ctx) {
 const id=Number(process.env.UI2_MESSAGE_ID);

 if (!Number.isInteger(id)) return popupScenario(ctx,"message",async(page,check)=>{
   await waitPopup(page,p=>texts(p,"message.text").some(t=>/no matching/i.test(t)),"honest unavailable event");
   check("no event is manufactured",true);
 },{a:-1});
 return popupScenario(ctx,"message",async(page,check)=>{
   await waitPopup(page,p=>texts(p,"message.text").length>0||texts(p,"message.title").length>0,"real message text");
   await clickRequired(page,"message.ok");
   await waitPopup(page,p=>!p.ui.windows.includes("message"),"message acknowledged");
   check("message closes after acknowledgement",true);
 },{a:id});
}
