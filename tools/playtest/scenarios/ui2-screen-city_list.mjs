import { scenario, openScreen, waitFor, texts, clickPart } from "./ui2-screen-city-support.mjs";
export const run = context => scenario(context,"ui2-screen-city_list",async ({page,check}) => {
  await openScreen(page,"city_list");
  const root=await waitFor(page,p => texts(p,"city-list.name").length > 0,"owned cities");
  const name=texts(root,"city-list.name")[0];
  await clickPart(page,"city-list.row");
  await waitFor(page,p => texts(p,"city.name").includes(name),"selected city window");
  check("clicking a real city opens its management screen",true);
});
