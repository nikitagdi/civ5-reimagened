import { scenario, openScreen, waitFor, texts, clickPart } from "./ui2-screen-city-support.mjs";
export const run = context => scenario(context, "ui2-screen-diplo", async ({page, check}) => {
  await openScreen(page, "diplo");
  await waitFor(page, p => texts(p, "diplo.major-name").length > 0, "real contacted civilizations (use a save with contacts)");
  await clickPart(page, "diplo.global-tab");
  await clickPart(page, "diplo.deals-tab");
  // The server reads the seat's current and past deals from the core: the tab lists them, or says there are none.
  const p = await waitFor(page, p => texts(p, "diplo.no-deals").length > 0 || texts(p, "diplo.deal-partner").length > 0, "deal history");
  check("deal history is available", texts(p, "diplo.history-reason").length === 0);
  await page.keyboard.press("Escape");
  await waitFor(page, p => !p?.ui?.windows?.includes("diplo"), "overview closed");
  check("overview closes with Escape", true);
});
