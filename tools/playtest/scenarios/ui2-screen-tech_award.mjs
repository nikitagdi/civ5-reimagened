import { techBrowser, waitTech, texts, openTechWindow } from "../lib/tech-screen.mjs";
import { clickPart } from "./ui2-v1.mjs";
export async function run({server,opts}) {
  const browser=await techBrowser(server,opts,"ui2-screen-tech_award");
  try {
    const page=browser.page;
    await openTechWindow(page,"tech_award",0);
    await waitTech(page,p=>texts(p,"tech-award.name").length===1,"real technology award");
    if(!await clickPart(page,"tech-award.continue")) throw new Error("Continue is missing");
    await waitTech(page,p=>!p.ui.windows.includes("tech_award"),"Continue closes award");
    await openTechWindow(page,"tech_award",0);
    await waitTech(page,p=>texts(p,"tech-award.name").length===1,"reopened award");
    await page.keyboard.press("Escape");
    await waitTech(page,p=>!p.ui.windows.includes("tech_award"),"Escape closes award");
    return {scenario:"ui2-screen-tech_award",pass:true,checks:[{name:"argument selects real award; Continue and Escape close it",pass:true}]};
  } finally { await browser.close(); }
}
