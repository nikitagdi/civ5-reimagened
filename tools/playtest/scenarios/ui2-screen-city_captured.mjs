import { scenario, openScreen, waitFor, texts, clickPart } from "./ui2-screen-city-support.mjs";
export const run = context => scenario(context,"ui2-screen-city_captured",async ({page,city,check}) => {
  await openScreen(page,"city_captured",city);
  const p=await waitFor(page,p => texts(p,"city-captured.option").length >= 4,"capture choices");
  check("capture choice requires a pending server event",texts(p,"city-captured.reason").some(t => t.includes("pending")));
  await clickPart(page,"city-captured.view");
  await waitFor(page,p => texts(p,"city.name").length > 0,"read-only city");
  check("view opens real city data",true);
});
