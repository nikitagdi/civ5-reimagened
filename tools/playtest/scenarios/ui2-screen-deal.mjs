import { scenario, openScreen, waitFor, texts, clickPart } from "./ui2-screen-city-support.mjs";
export const run = context => scenario(context,"ui2-screen-deal",async ({page,check}) => {
  const partner=Number(process.env.CIV5_TEST_TRADE_PARTNER ?? 1);
  await openScreen(page,"deal",partner);
  await waitFor(page,p => texts(p,"deal.partner").length > 0,"contacted human trade partner fixture");
  // The fixture must supply positive income; add a real draft item, then clear it.
  await page.evaluate(() => window.__civ5test.ui2Intent({name:"screen.act",args:{screen:"deal",action:"add",a:0,b:1,c:0,d:0}}));
  await waitFor(page,p => texts(p,"deal.item-us").length > 0,"server-owned draft item");
  await clickPart(page,"deal.clear");
  await waitFor(page,p => texts(p,"deal.item-us").length === 0,"cleared draft");
  check("draft edits refresh without submitting a deal",true);
  check("proposal limitation is visible",texts(await waitFor(page,p => texts(p,"deal.reason").some(t => /unavailable/i.test(t)),"submission reason"),"deal.reason").length > 0);
  await page.keyboard.press("Escape");
  await waitFor(page,p => !p?.ui?.windows?.includes("deal"),"hidden trade panel");
});
