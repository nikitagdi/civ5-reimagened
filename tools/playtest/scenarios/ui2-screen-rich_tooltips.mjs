import { techBrowser, waitTech, texts } from "../lib/tech-screen.mjs";
import { clickPart } from "./ui2-v1.mjs";
export async function run({server,opts}) {
  const browser=await techBrowser(server,opts,"ui2-screen-rich_tooltips");
  try {
    const page=browser.page;
    // A technology row under the pointer names itself in ui.tip; the rich_tooltips root follows it (ui2-source remote.rs)
    // and the row's tooltip shows the core's line; leaving the row clears ui.tip and the root goes.
    if(!await clickPart(page,"top.yield.science")) throw new Error("Tech tree entry is missing");
    await waitTech(page,p=>texts(p,"techs.name").length>50,"real technology catalog");
    const rows=await page.evaluate(()=>window.__civ5test.ui2Rects(["techs.row"]));
    const row=rows.find(r=>r.x>=0 && r.y>=72 && r.x+r.w<=1366 && r.y+r.h<=700 && r.w>100);
    if(!row) throw new Error("No visible technology row");
    // On the portrait: the unlock icons in the middle of the card have their own tooltip.
    await page.mouse.move(row.x+36,row.y+row.h/2);
    await waitTech(page,p=>texts(p,"techs.tip.core").length>0,"core tooltip after hover integration");
    await page.mouse.move(1300,740);
    await waitTech(page,p=>texts(p,"techs.tip.core").length===0,"tooltip dismissed");
    return {scenario:"ui2-screen-rich_tooltips",pass:true,checks:[{name:"core tooltip appears and disappears on hover",pass:true}]};
  } finally { await browser.close(); }
}
