import { scenario, openScreen, waitFor, probe, texts, clickPart } from "./ui2-screen-city-support.mjs";
export const run = context => scenario(context,"ui2-screen-city",async ({page,city,check}) => {
  await openScreen(page,"city",city);
  const before = await waitFor(page,p => texts(p,"city.name").length && texts(p,"city.option").length,"real city catalog");
  check("server city rows appear",texts(before,"city.option").length > 0);
  if (process.env.UI2_SHOT) { await new Promise(r => setTimeout(r, 20000)); await page.screenshot({ path: process.env.UI2_SHOT }); }
  check("city focus captions are localized", texts(before,"city.focus").includes("Default focus"));
  check("yield grid has readable captions", texts(before,"city.yields").includes("Food"));
  check("tile ledger starts collapsed", before.ui.state["city.tiles_open"] === false);
  await clickPart(page,"city.tiles-toggle");
  const tiles = await waitFor(page,p => p.ui.state["city.tiles_open"] === true && texts(p,"city.tile").includes("City center"),"localized tile ledger"); // the rows are `city.tile` parts since the kit
  check("tile ledger expands with localized states", texts(tiles,"city.tile").includes("City center"));
  await clickPart(page,"city.tiles-toggle");
  await waitFor(page,p => p.ui.state["city.tiles_open"] === false,"tile ledger collapses");
  const count = texts(before,"city.queue").length;
  // The dump of a part covers only the subtree of its first element: ask for the scroll area that holds every option
  // row and keep the option rows (keys "group/option", one 28 px row across the 480 px column since the kit).
  const rects = await page.evaluate(() => window.__civ5test.ui2Rects(["city.scroll"]));
  const seen = new Set();
  const rows = rects.filter(r => /^\d+:\d+\/\d+:\d+$/.test(r.keys || "") && !r.node.includes("#") && r.h >= 24 && r.h <= 40 && r.w > 150 && r.w <= 480 && r.y > 0 && r.y+r.h < 768 && !seen.has(r.keys) && seen.add(r.keys)).sort((a,b) => a.y-b.y);
  let added = false;
  for (const r of rows) {
    await page.mouse.click(r.x+90,r.y+r.h/2); // the name cell (the button that builds), after the 24 px icon
    try { await waitFor(page,p => texts(p,"city.queue").length > count,"production added",2000); added=true; break; } catch { /* Try the next visible native-enabled option. */ }
  }
  check("main build action refreshes the visible queue",added);
  await clickPart(page,"city.queue-remove");
  await waitFor(page,p => texts(p,"city.queue").length === count,"queue removal");
  check("removing an order refreshes the queue",true);
  await clickPart(page,"city.close");
  // The close is an intent: the window goes in a following frame.
  const closed = await waitFor(page,p => !p.ui.windows.includes("city"),"city view closed",5000).then(() => true,() => false);
  check("return closes the window",closed);
});
