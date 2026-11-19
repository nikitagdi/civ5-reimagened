import { techBrowser, waitTech, texts, techCards } from "../lib/tech-screen.mjs";
import { clickPart, partBox } from "./ui2-v1.mjs";
// The research block of the HUD (top left, under the strip): with nothing being researched it asks for a choice and
// opens the research chooser; a research chosen in the technology tree shows in it.
export async function run({server,opts}) {
  const browser = await techBrowser(server,opts,"ui2-screen-tech_panel");
  try {
    const page=browser.page;
    const checks=[];
    await waitTech(page,p=>texts(p,"top.research").length>0 || texts(p,"top.research.pick").length>0,"HUD research block");
    if (await partBox(page,"top.research.pick")) {
      // Nothing researched: the block's call to choose opens the chooser.
      await clickPart(page,"top.research.pick");
      await waitTech(page,p=>texts(p,"research.list").length>0,"research chooser");
      checks.push({name:"the block's call to choose opens the research chooser",pass:true});
      await page.keyboard.press("Escape");
      await waitTech(page,p=>!p.ui.windows.includes("research"),"chooser closed");
    }
    // The tree (the science counter opens it): a click on an available technology makes it current.
    if (!await clickPart(page,"top.yield.science")) throw new Error("the science counter is missing");
    const root=await waitTech(page,p=>texts(p,"techs.name").length>50,"real technology rows");
    const index=texts(root,"techs.status").indexOf("Available");
    if (index<0) throw new Error("No available research");
    const name=texts(root,"techs.name")[index];
    const row=(await techCards(page))[index];
    if(!row || row.x<0 || row.x+row.w>1366 || row.y+row.h>768) throw new Error("Available row is outside the viewport");
    await page.mouse.click(row.x+row.w/2,row.y+row.h/2);
    const after=await page.evaluate(()=>window.__civ5test.ui2());
    if(after.ui.windows.includes("techs") || after.ui.windows.includes("research")) await page.keyboard.press("Escape");
    await waitTech(page,p=>texts(p,"top.research").includes(name),"block reflects chosen research");
    checks.push({name:"HUD research block reflects the research chosen in the tree",pass:true});
    return {scenario:"ui2-screen-tech_panel",pass:true,checks};
  } finally { await browser.close(); }
}
