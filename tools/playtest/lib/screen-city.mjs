import {act, until, state} from "./browser.mjs";
import {FOUND_CITY} from "./policy.mjs";
export async function ensureCity(page) {
  let s = await state(page);
  if (s.cities?.length) return s.cities[0].id;
  const settler = (s.units ?? []).find(u => /settler/i.test(u.name));
  if (!settler) throw new Error("An owned city or a settler is required");
  await act(page, {kind: "selectUnit", id: settler.id});
  await until(page, s => s.selectedUnit === settler.id, 10000, "selected settler");
  await act(page, {kind: "unitAction", action: FOUND_CITY});
  s = await until(page, s => s.cities?.length > 0, 30000, "first city");
  return s.cities[0].id;
}
