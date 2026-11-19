import { withHud, probe, texts, waitFor } from "./ui2-screen-top_panel.mjs";
import { clickPart } from "./ui2-v1.mjs";

export async function run(context) {
  return withHud(context, "ui2-screen-unit_lists", async (page, check) => {
    if (!await clickPart(page, "unit-lists.units-open")) throw new Error("Unit list toggle missing");
    const rows = await waitFor(page, p => texts(p, "unit-lists.military-unit").length + texts(p, "unit-lists.civilian-unit").length > 0, "owned unit rows");
    // Names and the original status texts are resolved game text.
    const shown = [...texts(rows, "unit-lists.military-unit"), ...texts(rows, "unit-lists.civilian-unit")];
    check("unit rows show resolved names and statuses", shown.every(t => !/TXT_KEY_/.test(t)), shown.join(" | "));
    if (!await clickPart(page, "unit-lists.military-unit") && !await clickPart(page, "unit-lists.civilian-unit")) throw new Error("Unit row missing");
    await waitFor(page, p => texts(p, "unit.title").length > 0, "selected unit panel");
    check("unit row selects a unit", true);
    if (!await clickPart(page, "unit-lists.city")) throw new Error("City ribbon row missing");
    await waitFor(page, p => p.ui.windows.includes("city"), "city window");
    check("city ribbon opens real city", (await probe(page)).ui.windows.includes("city"));
  });
}
