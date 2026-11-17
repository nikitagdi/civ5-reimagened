import { scenario, openScreen, waitFor, probe, texts, clickPart } from "./ui2-screen-city-support.mjs";
export const run = context => scenario(context,"ui2-screen-city",async ({page,city,check}) => {
  await openScreen(page,"city",city);
  const before = await waitFor(page,p => texts(p,"city.name").length && texts(p,"city.option").length,"real city catalog");
  check("server city rows appear",texts(before,"city.option").length > 0);
  const count = texts(before,"city.queue").length;
  const rects = await page.evaluate(() => window.__civ5test.ui2Rects(["city.option"]));
  const rows = rects.filter(r => !r.node.includes("#") && r.w > 0 && r.y > 0 && r.y+r.h < 768);
  let added = false;
  for (const r of rows) {
    await page.mouse.click(r.x+r.w/2,r.y+r.h/2);
    try { await waitFor(page,p => texts(p,"city.queue").length > count,"production added",2000); added=true; break; } catch { /* Try the next visible native-enabled option. */ }
  }
  check("main build action refreshes the visible queue",added);
  await clickPart(page,"city.queue-remove");
  await waitFor(page,p => texts(p,"city.queue").length === count,"queue removal");
  check("removing an order refreshes the queue",true);
  await clickPart(page,"city.close");
  check("return closes the window",!(await probe(page)).ui.windows.includes("city"));
});
