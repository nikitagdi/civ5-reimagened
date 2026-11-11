// Run: node --test apps/web/www/ui2/
// Decodes the reference delta that the Rust test `reference_delta_text_is_stable` writes and checks the result.
import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { SCHEMA_HASH, PATHS, ROOTS, ENUMS, decodeUmd1, fieldId } from "./model.gen.js";

const reference = readFileSync(
  fileURLToPath(new URL("../../../../crates/shared/ui2-model/tests/data/umd1-reference.json", import.meta.url)),
  "utf8",
);
const schemaJson = JSON.parse(
  readFileSync(
    fileURLToPath(new URL("../../../../crates/shared/ui2-model/gen/schema.gen.json", import.meta.url)),
    "utf8",
  ),
);

test("the generated tables agree with schema.gen.json", () => {
  assert.equal(SCHEMA_HASH, schemaJson.hash);
  assert.deepEqual(PATHS.map((p) => p.path), schemaJson.paths.map((p) => p.path));
  assert.deepEqual(ROOTS.map((r) => r[0]), schemaJson.roots.map((r) => r.name));
  for (const e of schemaJson.enums) assert.deepEqual(ENUMS[e.name], e.variants);
  assert.equal(fieldId("top.resources[].available"), PATHS.findIndex((p) => p.path === "top.resources[].available"));
  assert.equal(fieldId("nope"), -1);
});

test("decodes the reference delta", () => {
  const d = decodeUmd1(reference);
  assert.equal(d.revision, 7);
  assert.equal(d.ops.length, 9);
  const [reset, gold, tourism, ins0, ins1, upd, move, remove, resetTop] = d.ops;
  assert.deepEqual(reset, {
    op: "reset",
    root: "meta",
    value: { schema: "1.0", source: "server", live: true, catching_up: false, connected: true },
  });
  assert.deepEqual(gold, { op: "set", field: fieldId("top.gold"), path: "top.gold", keys: [], value: -12 });
  assert.equal(tourism.value, null);
  assert.equal(ins0.op, "insert");
  assert.equal(ins0.path, "top.resources");
  assert.equal(ins0.key, "1:3");
  assert.equal(ins0.at, 0);
  assert.deepEqual(ins0.value, {
    id: "1:3",
    name: { k: "res.iron", a: ["2"] },
    icon: "icon:RES_ATLAS:3",
    available: 2,
    used: 0,
    total: 2,
  });
  assert.equal(ins1.value.name, 'Horses "fast"');
  assert.deepEqual(upd.keys, ["1:4"]);
  assert.equal(upd.path, "top.resources[].available");
  assert.equal(upd.value, 5);
  assert.deepEqual(move, { op: "move", field: fieldId("top.resources"), path: "top.resources", keys: [], key: "1:4", to: 0 });
  assert.equal(remove.key, "1:3");
  assert.equal(resetTop.value.gold, 100);
  assert.equal(resetTop.value.mood, "content");
  assert.deepEqual(resetTop.value.golden_age, { active: false, turns: 0, progress: 0, threshold: 0 });
  assert.equal(resetTop.value.tourism, null);
  assert.deepEqual(resetTop.value.resources, []);
});

test("a snapshot header for another schema is refused", () => {
  assert.throws(() => decodeUmd1('[1,[5,"0000000000000000"]]'), /schema mismatch/);
  assert.doesNotThrow(() => decodeUmd1(`[1,[5,"${SCHEMA_HASH}"]]`));
});
