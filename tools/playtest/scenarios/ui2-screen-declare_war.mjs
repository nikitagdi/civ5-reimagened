import { scenario, openScreen, waitFor, texts, clickPart } from "./ui2-screen-city-support.mjs";
export const run = context => scenario(context, "ui2-screen-declare_war", async ({page, check}) => {
  // A real contacted rival is required. The overview provides no fabricated fallback row.
  await openScreen(page, "diplo");
  const rows = await waitFor(page,p => texts(p,"diplo.major-name").length > 0,"contacted rival fixture");
  check("real diplomacy rows exist",texts(rows,"diplo.major-name").length > 0);
  const team = Number(process.env.CIV5_TEST_RIVAL_TEAM ?? 1);
  await openScreen(page,"declare_war",team,-1);
  await waitFor(page,p => texts(p,"declare-war.target").length > 0,"rival confirmation");
  await page.keyboard.press("Enter");
  await waitFor(page,p => p?.ui?.windows?.includes("declare_war"),"Enter leaves confirmation open");
  await clickPart(page,"declare-war.cancel");
  await waitFor(page,p => !p?.ui?.windows?.includes("declare_war"),"cancel closes confirmation");
  check("cancel closes the confirmation",true);
  await openScreen(page,"declare_war",team,-1);
  const before=await waitFor(page,p => texts(p,"declare-war.target").length > 0,"rival ready to declare war");
  const name=texts(before,"declare-war.target")[0];
  await clickPart(page,"declare-war.confirm");
  await waitFor(page,p => !p?.ui?.windows?.includes("declare_war"),"war confirmed");
  await openScreen(page,"diplo");
  await waitFor(page,p => {
    const index=texts(p,"diplo.major-name").indexOf(name);
    return index >= 0 && texts(p,"diplo.major-status")[index] === "war";
  },"declared war in real relations");
  check("confirmation changes the rival relationship to war",true);
});
