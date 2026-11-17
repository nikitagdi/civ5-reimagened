import { withHud, texts, waitFor } from "./ui2-screen-top_panel.mjs";
import { clickPart } from "./ui2-v1.mjs";

export async function run(context) {
  return withHud(context, "ui2-screen-on_map", async (page, check) => {
    const rects = await page.evaluate(() => window.__civ5test.ui2Rects(["unit-flag"]));
    const r = rects.find(r => r.w > 0 && r.h > 0 && r.x > 400 && r.y > 60);
    if (!r) throw new Error("Visible unit flag required");
    await page.mouse.move(r.x + r.w / 2, r.y + r.h / 2);
    await waitFor(page, p => texts(p, "on-map.unit-tip").includes("Health"), "unit tooltip");
    check("unit flag tooltip contains real health", true);
    if (!await clickPart(page, "city-banner")) throw new Error("Owned city banner required");
    await waitFor(page, p => p.ui.windows.includes("city"), "city window");
    check("banner opens its city", true);
  });
}
