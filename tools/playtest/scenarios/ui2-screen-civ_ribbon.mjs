import { scenario, waitFor, texts, clickPart } from "./ui2-screen-city-support.mjs";
// The ribbon starts folded behind its toggle (the notifications keep the right edge); 1 or the toggle opens it.
export const run = context => scenario(context,"ui2-screen-civ_ribbon",async ({page,check}) => {
  await waitFor(page,p => texts(p,"civ-ribbon.name").length === 0,"folded ribbon at the start");
  check("the ribbon starts folded",true);
  await page.keyboard.press("1");
  await waitFor(page,p => texts(p,"civ-ribbon.name").length > 0,"live civilization ribbon");
  await clickPart(page,"civ-ribbon.diplo");
  await waitFor(page,p => p?.ui?.windows?.includes("diplo"),"diplomacy overview from HUD");
  check("HUD diplomacy entry opens the overview",true);
  await page.keyboard.press("Escape");
  await page.keyboard.press("1");
  await waitFor(page,p => texts(p,"civ-ribbon.name").length === 0,"hidden ribbon");
  await page.keyboard.press("1");
  await waitFor(page,p => texts(p,"civ-ribbon.name").length > 0,"restored ribbon");
  check("ribbon visibility toggles without opening a modal",true);
});
