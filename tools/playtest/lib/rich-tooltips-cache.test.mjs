import test from "node:test";
import assert from "node:assert/strict";
import { richTooltipCache } from "../../../skins/engraved/js/rich_tooltips-cache.mjs";
test("hover cache coalesces, throttles, and drops answers across revisions and seats", async () => {
  let time = 0, calls = 0, finish;
  const hover = richTooltipCache(() => { calls++; return new Promise(resolve => { finish = resolve; }); }, () => time);
  const one = hover(0, 1, 0, 1);
  const same = hover(0, 1, 0, 1);
  assert.equal(await hover(0, 1, 0, 2), null);
  await Promise.resolve(); finish({tech: 1});
  assert.equal(await one, null); assert.equal(await same, null);
  assert.deepEqual(await hover(0, 1, 0, 1), {tech: 1}); assert.equal(calls, 1);
  assert.equal(await hover(0, 1, 0, 2), null);
  time = 250; const stale = hover(0, 1, 0, 2); await Promise.resolve(); const oldFinish = finish;
  const fresh = hover(1, 2, 0, 2); await Promise.resolve(); oldFinish({secret: 1});
  assert.equal(await stale, null); finish({tech: 2}); assert.deepEqual(await fresh, {tech: 2});
  assert.equal(calls, 3);
});
