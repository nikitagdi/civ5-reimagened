import { scenario, openScreen, waitFor, texts } from "./ui2-screen-city-support.mjs";
export const run = context => scenario(context,"ui2-screen-leader",async ({page,check}) => {
  await openScreen(page,"leader",Number(process.env.CIV5_TEST_AI_PARTNER ?? 2));
  const p=await waitFor(page,p => texts(p,"leader.name").length > 0,"contacted AI dossier fixture");
  check("leader name comes from the real game",texts(p,"leader.name").length > 0);
  check("missing conversation session is explained",texts(p,"leader.reason").some(t => /unavailable/i.test(t)));
  await page.keyboard.press("Escape");
  await waitFor(page,p => !p?.ui?.windows?.includes("leader"),"dossier goodbye");
  check("goodbye closes the dossier",true);
});
