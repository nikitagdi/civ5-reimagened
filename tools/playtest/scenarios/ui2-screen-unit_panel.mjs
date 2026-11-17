import { withHud, intent, probe, texts, waitFor } from "./ui2-screen-top_panel.mjs";
import { state } from "../lib/browser.mjs";
import { clickPart } from "./ui2-v1.mjs";

export async function run(context) {
  return withHud(context, "ui2-screen-unit_panel", async (page, check) => {
    const unit = (await state(page)).units?.[0];
    if (!unit) throw new Error("Owned unit required");
    // The selection of UI v2 is its own (the test hook `selectUnit` drives the classic UI's): the unit's map flag selects it.
    await waitFor(page, p => (p.counters["ui2.anchors.shown"] ?? 0) > 0, "unit flags on the map");
    if (!await clickPart(page, "unit-flag")) throw new Error("A unit flag is required");
    await waitFor(page, p => texts(p, "unit-panel.action").length > 0, "native action catalog");
    if (!await clickPart(page, "unit-panel.rename-open")) throw new Error("Rename button missing");
    await waitFor(page, p => p.ui.windows.includes("unit_rename"), "rename window");
    if (!await clickPart(page, "unit-panel.rename-input")) throw new Error("Name input missing");
    await page.keyboard.press("Control+a");
    await page.keyboard.type("Test Guard");
    if (!await clickPart(page, "unit-panel.rename-accept")) throw new Error("Accept button missing");
    await waitFor(page, p => !p.ui.windows.includes("unit_rename") && texts(p, "unit.title").some(t => t.startsWith("Test Guard")), "renamed unit");
    check("text action updates unit title", true);
    await intent(page, "unit.select_next");
    check("catalog is rendered", texts(await probe(page), "unit-panel.action").length > 0);
  });
}
