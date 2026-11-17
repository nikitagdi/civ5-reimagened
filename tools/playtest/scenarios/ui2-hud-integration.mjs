import { withHud, intent, probe, texts, waitFor } from "./ui2-screen-top_panel.mjs";
import { clickPart, partBox } from "./ui2-v1.mjs";
import { state } from "../lib/browser.mjs";

const overlaps = (a, b) => a && b && a.x < b.x + b.w - 1 && b.x < a.x + a.w - 1
  && a.y < b.y + b.h - 1 && b.y < a.y + a.h - 1;

export async function run(context) {
  return withHud(context, "ui2-hud-integration", async (page, check) => {
    const unit = (await state(page)).units?.[0];
    if (!unit) throw new Error("Owned unit required");
    // The selection of UI v2 is its own (the test hook `selectUnit` drives the classic UI's): the unit's flag on the map
    // selects it.
    await waitFor(page, p => (p.counters["ui2.anchors.shown"] ?? 0) > 0, "unit flags on the map");
    if (!await clickPart(page, "unit-flag")) throw new Error("A unit flag is required");
    await waitFor(page, p => texts(p, "unit.title").length > 0, "selected unit");
    for (const viewport of [{width: 1920, height: 1080}, {width: 1366, height: 768}]) {
      await page.setViewportSize(viewport);
      await page.waitForTimeout(1000);
      const label = `${viewport.width}x${viewport.height}`;
      const strip = await partBox(page, "top");
      check(`${label} top strip fits viewport`, strip && strip.x >= 0 && strip.x + strip.w <= viewport.width);
      check(`${label} science and policies stay directly accessible`,
        !!await partBox(page, "top.menu.tech") && !!await partBox(page, "top.menu.policies"));
      if (viewport.width === 1920) {
        for (const destination of ["religion", "cities", "economy", "military", "diplomacy"]) {
          check(`${label} ${destination} stays in the strip`, !!await partBox(page, `top.menu.${destination}`));
        }
      }
      check(`${label} empty resources have no dash cells`, !texts(await probe(page), "top.resources").includes("—"));
      if (process.env.UI2_SHOT) await page.screenshot({ path: `${process.env.UI2_SHOT}-${label}.png` });
      const flag = await partBox(page, "unit-flag");
      if (!flag) throw new Error("A visible flag is required for tooltip placement");
      await page.mouse.move(flag.cx, flag.cy);
      await waitFor(page, p => texts(p, "on-map.unit-tip").length > 0, "flag tooltip");
      const tip = await partBox(page, "on-map.unit-tip");
      check(`${label} flag tooltip stays beside its map node`, tip && Math.abs(tip.x - flag.x) < 300 && Math.abs(tip.y - flag.y) < 250);
      await page.mouse.move(viewport.width / 2, 4);
      await page.waitForTimeout(300);
      check(`${label} flag tooltip closes after hover leaves`, !await partBox(page, "on-map.unit-tip"));
      const actionBoxes = [];
      for (const part of ["unit.act.fortify", "unit.act.skip", "unit-panel.action"]) {
        const boxes = await page.evaluate(part => window.__civ5test.ui2Rects([part]), part);
        actionBoxes.push(...boxes.filter(box => box.w > 0 && box.h > 0));
      }
      // A part's rectangles include its descendants' (the icon and the hotkey label of each button): only the buttons
      // themselves, the rectangles no other action rectangle contains, stand in rows.
      const inside = (a, b) => a !== b && a.x >= b.x - 1 && a.y >= b.y - 1 && a.x + a.w <= b.x + b.w + 1 && a.y + a.h <= b.y + b.h + 1;
      const buttons = actionBoxes.filter(box => !actionBoxes.some(other => inside(box, other)));
      check(`${label} unit actions use two compact rows`,
        buttons.length > 0 && new Set(buttons.map(box => Math.round(box.y))).size === 2
        && buttons.every(box => box.h <= 44 && box.w <= 60),
        JSON.stringify(buttons.map(box => [box.part, box.x, box.y, box.w, box.h])));
      const fold = await partBox(page, "top.menu.fold");
      if (fold && !(await partBox(page, "top.menu.tech"))) {
        await page.keyboard.press("Alt+w");
        const end = Date.now() + 5000;
        while (!(await partBox(page, "top.menu.tech")) && Date.now() < end) await page.waitForTimeout(50);
        check(`${label} menu shortcut reveals destinations`, !!(await partBox(page, "top.menu.tech")));
        await clickPart(page, "top.menu.fold");
      }
      const current = await probe(page);
      check(`${label} ribbon has no window`, !current.ui.windows.includes("civ_ribbon"));
      // One after the other: the page answers one rectangle request at a time.
      const right = [];
      for (const part of ["notes", "civ-ribbon", "helm"]) right.push(await partBox(page, part));
      if (right.some(box => !box)) throw new Error(`${label} right HUD boxes missing`);
      check(`${label} notifications, ribbon and turn plate are disjoint`,
        !overlaps(right[0], right[1]) && !overlaps(right[0], right[2]) && !overlaps(right[1], right[2]));
      check(`${label} dial clears minimap`,
        !overlaps(await partBox(page, "helm.dial"), await partBox(page, "helm.minimap")));
      check(`${label} corner list starts closed`, !await partBox(page, "tech-panel"));
      await clickPart(page, "tech-panel.toggle");
      await waitFor(page, p => texts(p, "tech-panel.choose").length > 0 || texts(p, "tech-panel.name").length > 0, "expanded research corner");
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
      await waitFor(page, p => !p.ui.windows.includes("techs"), "closed technology window");
      check(`${label} navigation closes the More menu`, !await partBox(page, "top.menu.victory"));
      for (const window of ["economy", "military", "trade_routes", "espionage", "culture", "victory", "demographics", "congress", "notification_log", "game_menu", "advisor"]) {
        await intent(page, "window.open", { window });
        await waitFor(page, p => p.ui.windows.includes(window), `${window} window`);
        const up = await probe(page);
        check(`${label} ${window} suppresses HUD panels`, texts(up, "helm.state").length === 0
          && texts(up, "top.yield.science").length === 0 && texts(up, "civ-ribbon.name").length === 0);
        check(`${label} ${window} hides notifications and unit corner`, !(await partBox(page, "notes")) && !(await partBox(page, "unit")));
        await intent(page, "window.close", { window });
        await waitFor(page, p => !p.ui.windows.includes(window), `closed ${window}`);
      }

      await page.evaluate(() => window.__civ5test.ui2Intent({ name: "window.open", args: { window: "declare_war", a: 999, b: -1 } }));
      await waitFor(page, p => texts(p, "declare-war.reason").length > 0 || texts(p, "declare-war.unavailable").length > 0, "invalid rival reason");
      check(`${label} invalid rival has an explanation`, true);
      await intent(page, "window.close", { window: "declare_war" });
      await clickPart(page, "tech-panel.toggle");
    }
  });
}
