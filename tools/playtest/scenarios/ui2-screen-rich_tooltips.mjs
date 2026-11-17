import { techBrowser, waitTech, texts } from "../lib/tech-screen.mjs";
import { clickPart } from "./ui2-v1.mjs";
export async function run({server,opts}) {
  const browser=await techBrowser(server,opts,"ui2-screen-rich_tooltips");
  try {
    const page=browser.page;
    // SIMPLIFIED(SIMP-421): the skin host has no query callback or service-root subscription API for hover requests.
    // Lead hover integration owns the Query::Screen transport and the tooltip owner.
    if(!await clickPart(page,"top.menu.tech")) throw new Error("Tech tree entry is missing");
    await waitTech(page,p=>texts(p,"techs.name").length>50,"real technology catalog");
    const rows=await page.evaluate(()=>window.__civ5test.ui2Rects(["techs.row"]));
    const row=rows.find(r=>r.x>=0 && r.y>=72 && r.x+r.w<=1366 && r.y+r.h<=700 && r.w>100);
    if(!row) throw new Error("No visible technology row");
    await page.mouse.move(row.x+row.w/2,row.y+row.h/2);
    await waitTech(page,p=>texts(p,"rich-tooltips.title").length>0,"core tooltip after hover integration");
    await page.mouse.move(1300,740);
    await waitTech(page,p=>texts(p,"rich-tooltips.title").length===0,"tooltip dismissed");
    return {scenario:"ui2-screen-rich_tooltips",pass:true,checks:[{name:"core tooltip appears and disappears on hover",pass:true}]};
  } finally { await browser.close(); }
}
