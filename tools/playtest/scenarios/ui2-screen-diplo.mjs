import { scenario, openScreen, waitFor, texts, clickPart } from "./ui2-screen-city-support.mjs";
export const run = context => scenario(context, "ui2-screen-diplo", async ({page, check}) => {
  await openScreen(page, "diplo");
  await waitFor(page, p => texts(p, "diplo.major-name").length > 0, "real contacted civilizations (use a save with contacts)");
  await clickPart(page, "diplo.global-tab");
  await clickPart(page, "diplo.deals-tab");
  const p = await waitFor(page, p => texts(p, "diplo.history-reason").some(t => /unavailable/i.test(t)), "honest history availability");
  check("deal history reports its server limitation", texts(p, "diplo.history-reason").length > 0);
  await page.keyboard.press("Escape");
  await waitFor(page, p => !p?.ui?.windows?.includes("diplo"), "overview closed");
  check("overview closes with Escape", true);
});
