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
      // Each counter of the strip is the button to its window; the other windows are in the "more" menu.
      check(`${label} science and policies stay directly accessible`,
        !!await partBox(page, "top.yield.science") && !!await partBox(page, "top.yield.culture"));
      for (const counter of ["gold", "trade", "happiness", "faith"]) {
        check(`${label} the ${counter} counter is in the strip`, !!await partBox(page, `top.yield.${counter}`));
      }
      check(`${label} the strip is 32 px high`, strip && Math.round(strip.h) === 32, JSON.stringify(strip));
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
      // The orders: 38 px buttons in one wrapping row, in the catalog's order (the HUD service sorts it by weight:
      // found a city, fortify, sleep, then skip, delete last). A part's rectangles are its first instance and its
      // descendants', and each order's marker covers its button: the buttons are the distinct rectangles no other
      // action rectangle contains. The markers in reading order follow the weights.
      const markers = ["unit.act.found-city", "unit.act.fortify", "unit.act.sleep", "unit.act.skip", "unit.act.disband"];
      const actionBoxes = [];
      const markerAt = {};
      for (const part of ["unit-panel.action", ...markers]) {
        const boxes = (await page.evaluate(part => window.__civ5test.ui2Rects([part]), part))
          .filter(box => box.part === part && box.w > 0 && box.h > 0);
        actionBoxes.push(...boxes);
        if (part !== "unit-panel.action" && boxes.length) markerAt[part] = boxes.reduce((a, b) => (b.w * b.h > a.w * a.h ? b : a));
      }
      const key = box => [box.x, box.y, box.w, box.h].map(Math.round).join(",");
      const distinct = [...new Map(actionBoxes.map(box => [key(box), box])).values()];
      const inside = (a, b) => key(a) !== key(b) && a.x >= b.x - 1 && a.y >= b.y - 1 && a.x + a.w <= b.x + b.w + 1 && a.y + a.h <= b.y + b.h + 1;
      const buttons = distinct.filter(box => !distinct.some(other => inside(box, other)));
      const rows = [...new Set(buttons.map(box => Math.round(box.y)))];
      const placed = markers.filter(part => markerAt[part]).map(part => markerAt[part]);
      const reading = placed.every((box, i) => i === 0 || Math.round(box.y) > Math.round(placed[i - 1].y)
        || (Math.round(box.y) === Math.round(placed[i - 1].y) && box.x > placed[i - 1].x));
      check(`${label} unit actions are 38 px buttons in one wrapping row, in order`,
        buttons.length > 1 && placed.length > 1 && buttons.every(box => box.h <= 40 && box.w <= 40) && rows.length <= 2 && reading,
        JSON.stringify({ buttons: buttons.map(box => [box.x, box.y, box.w, box.h]), order: markers.filter(part => markerAt[part]).map(part => [part, Math.round(markerAt[part].x), Math.round(markerAt[part].y)]) }));
      // The "more" menu: closed at first, Alt+W opens it with the other windows, the button closes it.
      check(`${label} the more menu starts closed`, !(await partBox(page, "top.menu.military")));
      await page.keyboard.press("Alt+w");
      const end = Date.now() + 5000;
      while (!(await partBox(page, "top.menu.military")) && Date.now() < end) await page.waitForTimeout(50);
      for (const destination of ["military", "diplomacy", "victory", "demographics", "espionage", "congress", "notification-log", "cities", "lists"]) {
        check(`${label} ${destination} is in the more menu`, !!await partBox(page, `top.menu.${destination}`));
      }
      check(`${label} the game menu is in the more menu`, !!await partBox(page, "top.main-menu.button"));
      await clickPart(page, "top.menu.fold");
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
      check(`${label} corner list starts closed`, !await partBox(page, "unit-lists.corner"));
      // The research block: under the strip on the left, clear of the right HUD.
      const research = await partBox(page, "top.research");
      check(`${label} research sits under the strip on the left`, research && research.x < 2 && Math.abs(research.y - 32) < 2,
        JSON.stringify(research));
      check(`${label} research clears right HUD`, research && right.every(box => !overlaps(research, box)));
      await clickPart(page, "top.menu.cities");
      await waitFor(page, p => p.ui.windows.includes("city_list"), "city corner section");
      const cities = await partBox(page, "city-list");
      if (!cities) throw new Error(`${label} city section missing`);
      check(`${label} city section clears unit and right HUD`,
        !overlaps(cities, await partBox(page, "unit")) && right.every(box => !overlaps(cities, box)));
      check(`${label} city section clears the research block`, !overlaps(cities, research));
      await intent(page, "window.close", {window: "city_list"});
      await clickPart(page, "top.yield.science");
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
    }
  });
}
