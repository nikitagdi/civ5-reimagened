import { techBrowser, waitTech, texts, techCards } from "../lib/tech-screen.mjs";
import { clickPart } from "./ui2-v1.mjs";
export async function run({server,opts}) {
  const browser = await techBrowser(server,opts,"ui2-screen-tech_panel");
  try {
    const page=browser.page;
    if (!await clickPart(page,"tech-panel.toggle")) throw new Error("Lead must include parts/tech_panel.html in the HUD layer");
    if (!await clickPart(page,"tech-panel.choose")) await clickPart(page,"tech-panel.current");
    const root=await waitTech(page,p=>texts(p,"techs.name").length>50,"real technology rows");
    const index=texts(root,"techs.status").indexOf("Available");
    const name=texts(root,"techs.name")[index];
    if (index<0) throw new Error("No available research");
    const row=(await techCards(page))[index];
    if(!row || row.x<0 || row.x+row.w>1366 || row.y+row.h>768) throw new Error("Available row is outside the viewport");
    await page.mouse.click(row.x+row.w/2,row.y+row.h/2);
    const after=await page.evaluate(()=>window.__civ5test.ui2());
    if(after.ui.windows.includes("techs")) await page.keyboard.press("Escape");
    await waitTech(page,p=>texts(p,"tech-panel.name").includes(name),"panel reflects chosen research");
    return {scenario:"ui2-screen-tech_panel",pass:true,checks:[{name:"HUD panel opens tree and reflects real research",pass:true}]};
  } finally { await browser.close(); }
}
