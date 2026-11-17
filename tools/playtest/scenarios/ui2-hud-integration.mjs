import { withHud, intent, probe, texts, waitFor } from "./ui2-screen-top_panel.mjs";
import { clickPart, partBox } from "./ui2-v1.mjs";
import { act, state } from "../lib/browser.mjs";

const overlaps = (a, b) => a && b && a.x < b.x + b.w - 1 && b.x < a.x + a.w - 1
  && a.y < b.y + b.h - 1 && b.y < a.y + a.h - 1;

export async function run(context) {
  return withHud(context, "ui2-hud-integration", async (page, check) => {
    const unit = (await state(page)).units?.[0];
    if (!unit) throw new Error("Owned unit required");
    await act(page, {kind: "selectUnit", id: unit.id});
    await waitFor(page, p => texts(p, "unit.title").length > 0, "selected unit");
    for (const viewport of [{width: 1920, height: 1080}, {width: 1366, height: 768}]) {
      await page.setViewportSize(viewport);
      await page.waitForTimeout(1000);
      const label = `${viewport.width}x${viewport.height}`;
      const current = await probe(page);
      check(`${label} ribbon has no window`, !current.ui.windows.includes("civ_ribbon"));
      const right = await Promise.all(["notes", "civ-ribbon", "helm"].map(part => partBox(page, part)));
      if (right.some(box => !box)) throw new Error(`${label} right HUD boxes missing`);
      check(`${label} notifications, ribbon and turn plate are disjoint`,
        !overlaps(right[0], right[1]) && !overlaps(right[0], right[2]) && !overlaps(right[1], right[2]));
      check(`${label} dial clears minimap`,
        !overlaps(await partBox(page, "helm.dial"), await partBox(page, "helm.minimap")));
      await clickPart(page, "tech-panel.toggle");
      const research = await partBox(page, "tech-panel");
      check(`${label} research clears right HUD`, right.every(box => !overlaps(research, box)));
      await clickPart(page, "top.menu.cities");
      await waitFor(page, p => p.ui.windows.includes("city_list"), "city corner section");
      const cities = await partBox(page, "city-list");
      if (!cities) throw new Error(`${label} city section missing`);
      check(`${label} city section clears unit and right HUD`,
        !overlaps(cities, await partBox(page, "unit")) && right.every(box => !overlaps(cities, box)));
      check(`${label} city section replaces research`, !(await partBox(page, "tech-panel")));
      await intent(page, "window.close", {window: "city_list"});
      await clickPart(page, "top.menu.tech");
      await waitFor(page, p => p.ui.windows.includes("techs"), "technology window");
      const full = await probe(page);
      check(`${label} full screen hides HUD panels`, texts(full, "helm.state").length === 0
        && texts(full, "top.yield.science").length === 0 && texts(full, "civ-ribbon.name").length === 0);
      await intent(page, "window.close", {window: "techs"});
    }
  });
}
