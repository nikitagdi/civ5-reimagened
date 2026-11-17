import { scenario, openScreen, waitFor, texts, clickPart } from "./ui2-screen-city-support.mjs";
export const run = context => scenario(context,"ui2-screen-free_great_person",async ({page,check}) => {
  await openScreen(page,"free_great_person",0);
  const p=await waitFor(page,p => texts(p,"free-great-person.name").length > 0,"Great Person catalog");
  if (Number(texts(p,"free-great-person.remaining")[0]) > 0) {
    const n=Number(texts(p,"free-great-person.remaining")[0]);
    await clickPart(page,"free-great-person.row");
    await clickPart(page,"free-great-person.confirm");
    await waitFor(page,p => !p.ui.windows.includes("free_great_person") || Number(texts(p,"free-great-person.remaining")[0]) < n,"reward consumed");
    check("confirmation consumes an available reward",true);
  } else {
    check("fresh game explains why the reward cannot be chosen",texts(p,"free-great-person.reason").some(t => t.includes("remain")));
  }
});
