import {scenario, openScreen, waitFor, texts, clickPart} from "./ui2-screen-city-support.mjs";
export const run = context => scenario(context, "ui2-screen-city_rename", async ({page, city, check}) => {
  await openScreen(page, "city_rename", city);
  await waitFor(page, p => texts(p, "city-rename.name").length > 0, "city rename data");
  await clickPart(page, "city-rename.input");
  await page.keyboard.press("Control+A");
  await page.keyboard.type("Bad/name");
  await waitFor(page, p => p.ui.state["city_rename.valid"] === false, "invalid draft");
  await page.keyboard.press("Control+A");
  await page.keyboard.type("New city");
  await waitFor(page, p => p.ui.state["city_rename.valid"] === true, "valid draft");
  if (!await clickPart(page, "city-rename.confirm")) throw new Error("Rename confirmation missing");
  await waitFor(page, p => !p.ui.windows.includes("city_rename"), "successful rename closes dialog");
  await openScreen(page, "city_rename", city);
  await waitFor(page, p => texts(p, "city-rename.name").includes("New city"), "server preserved rename text");
  check("validated rename text reaches the native city", true);
});
