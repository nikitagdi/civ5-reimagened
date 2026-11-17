import { scenario, openScreen, waitFor, texts, clickPart } from "./ui2-screen-city-support.mjs";
export const run = context => scenario(context,"ui2-screen-city_rename",async ({page,city,check}) => {
  await openScreen(page,"city_rename",city);
  await waitFor(page,p => texts(p,"city-rename.name").length > 0,"city rename data");
  await clickPart(page,"city-rename.input");
  await page.keyboard.type("Bad/name");
  const p=await waitFor(page,p => texts(p,"city-rename.reason").some(t => t.includes("text")),"explicit unavailable-operation reason");
  check("rename displays its server refusal while text emission is unavailable",texts(p,"city-rename.reason").length > 0);
});
