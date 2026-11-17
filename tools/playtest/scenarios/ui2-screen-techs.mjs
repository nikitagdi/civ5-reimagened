// Full-catalog, research and Escape acceptance against the real server.
import { run as runScreens } from "./ui2-screens.mjs";
export async function run(context) {
  const result = await runScreens(context);
  return {...result, scenario: "ui2-screen-techs"};
}
