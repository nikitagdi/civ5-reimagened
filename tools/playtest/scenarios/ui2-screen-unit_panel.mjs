import { withHud, intent, probe, texts, waitFor } from "./ui2-screen-top_panel.mjs";
import { act, state, until } from "../lib/browser.mjs";
import { clickPart } from "./ui2-v1.mjs";

export async function run(context) {
  return withHud(context, "ui2-screen-unit_panel", async (page, check) => {
    const unit = (await state(page)).units?.[0];
    if (!unit) throw new Error("Owned unit required");
    await act(page, { kind: "selectUnit", id: unit.id });
    await until(page, s => s.selectedUnit === unit.id, 10000, "selected unit");
    await waitFor(page, p => texts(p, "unit-panel.action").length > 0, "native action catalog");
    if (!await clickPart(page, "unit-panel.rename-open")) throw new Error("Rename button missing");
    await waitFor(page, p => p.ui.windows.includes("unit_rename"), "rename window");
    if (!await clickPart(page, "unit-panel.rename-input")) throw new Error("Name input missing");
    await page.keyboard.press("Control+a");
    await page.keyboard.type("Test Guard");
    if (!await clickPart(page, "unit-panel.rename-accept")) throw new Error("Accept button missing");
    await waitFor(page, p => !p.ui.windows.includes("unit_rename") && texts(p, "unit.title").includes("Test Guard"), "renamed unit");
    check("text action updates unit title", true);
    await intent(page, "unit.select_next");
    check("catalog is rendered", texts(await probe(page), "unit-panel.action").length > 0);
  });
}
