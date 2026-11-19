import { withHud, intent, probe, texts, waitFor } from "./ui2-screen-top_panel.mjs";
import { state } from "../lib/browser.mjs";
import { clickPart, partBox } from "./ui2-v1.mjs";

export async function run(context) {
  return withHud(context, "ui2-screen-unit_panel", async (page, check) => {
    const unit = (await state(page)).units?.[0];
    if (!unit) throw new Error("Owned unit required");
    // The selection of UI v2 is its own (the test hook `selectUnit` drives the classic UI's): the unit's map flag selects it.
    await waitFor(page, p => (p.counters["ui2.anchors.shown"] ?? 0) > 0, "unit flags on the map");
    if (!await clickPart(page, "unit-flag")) throw new Error("A unit flag is required");
    const catalogDeadline = Date.now() + 10000;
    while (!await partBox(page, "unit-panel.action") && Date.now() < catalogDeadline) await page.waitForTimeout(100);
    check("native action catalog", !!await partBox(page, "unit-panel.action"));
    if (!await clickPart(page, "unit-panel.rename-open")) throw new Error("Rename button missing");
    await waitFor(page, p => p.ui.windows.includes("unit_rename"), "rename window");
    if (!await clickPart(page, "unit-panel.rename-input")) throw new Error("Name input missing");
    // The field takes the keyboard through the page's text bridge, focused by the next UI frame: the keys wait for it.
    await page.waitForFunction(() => document.activeElement?.tagName === "TEXTAREA" || document.activeElement?.tagName === "INPUT", null, { timeout: 5000 });
    await page.keyboard.press("Control+a");
    await page.keyboard.type("Test Guard");
    if (!await clickPart(page, "unit-panel.rename-accept")) throw new Error("Accept button missing");
    await waitFor(page, p => !p.ui.windows.includes("unit_rename") && texts(p, "unit.title").some(t => t.startsWith("Test Guard")), "renamed unit");
    check("text action updates unit title", true);
    await intent(page, "unit.select_next");
    const action = await partBox(page, "unit-panel.action");
    check("catalog is rendered", !!action);
    if (action) {
      await page.mouse.move(action.cx, action.cy);
      await waitFor(page, p => texts(p, "unit-panel.action-tip").length > 0, "full action help");
      check("action hover shows its full name and help", texts(await probe(page), "unit-panel.action-tip").length > 0);
    }
  });
}
