import { scenario, openScreen, waitFor, texts } from "./ui2-screen-city-support.mjs";
// The leader of a contacted AI (the mid-game fixture contacts them): the seat's leader session greets, the main actions
// are live during the seat's turn, a click on one reaches the server (Discuss shows the discussion menu), Escape says
// goodbye.
export const run = context => scenario(context,"ui2-screen-leader",async ({page,check}) => {
  await openScreen(page,"leader",Number(process.env.CIV5_TEST_AI_PARTNER ?? 2));
  const p=await waitFor(page,p => texts(p,"leader.name").length > 0 && texts(p,"leader.action").length > 0,"contacted AI dossier fixture");
  check("leader name comes from the real game",texts(p,"leader.name").length > 0);
  check("the leader is available to talk to",texts(p,"leader.reason").every(t => !/unavailable|Choose a contacted/i.test(t)),JSON.stringify(texts(p,"leader.reason")));
  const actions=texts(p,"leader.action");
  check("the main actions are offered",["Discuss","Trade","Goodbye"].every(a => actions.includes(a)),JSON.stringify(actions));
  // A click on an action sends it (the action is the row's key, a string the screen action takes).
  // The rectangles of the first action button (Discuss, the first of the row).
  const rects=await page.evaluate(() => window.__civ5test.ui2Rects(["leader.action"]));
  const discuss=actions[0] === "Discuss" ? rects.find(r => r.w > 40) : null;
  check("the Discuss button is on the screen",!!discuss);
  if (discuss) {
    const before=await page.evaluate(() => window.__civ5test.ui2().texts.map(t => t.text).join("|"));
    await page.mouse.click(discuss.x + discuss.w / 2, discuss.y + discuss.h / 2);
    const after=await waitFor(page,p => p.texts.map(t => t.text).join("|") !== before && p.texts.some(t => /Never mind/.test(t.text)),"the discussion menu").catch(() => null);
    check("Discuss opens the discussion menu",!!after);
  }
  await page.keyboard.press("Escape");
  await waitFor(page,p => !p?.ui?.windows?.includes("leader"),"dossier goodbye");
  check("goodbye closes the dossier",true);
});
