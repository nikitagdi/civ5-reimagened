import { withHud, probe, texts, waitFor } from "./ui2-screen-top_panel.mjs";
import { clickPart } from "./ui2-v1.mjs";

export async function run(context) {
  return withHud(context, "ui2-screen-minimap", async (page, check) => {
    if (!await clickPart(page, "minimap.options-toggle")) throw new Error("Map options button missing");
    await waitFor(page, p => texts(p, "minimap.options").includes("City banners"), "map options");
    check("options show supported skin toggles", true);
    if (!await clickPart(page, "minimap.options-close")) throw new Error("Map options close missing");
    await waitFor(page, p => texts(p, "minimap.options").length === 0, "closed options");
    if (!await clickPart(page, "minimap.streamer-toggle")) throw new Error("Summary button missing");
    await waitFor(page, p => texts(p, "minimap.streamer").includes("Religion"), "seat summary");
    check("summary shows real seat data", texts(await probe(page), "minimap.streamer").includes("Policies"));
    check("renderer minimap is present", await clickPart(page, "helm.minimap"));
  });
}
