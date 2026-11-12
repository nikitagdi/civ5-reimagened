// The extract command on the strip fixture: base values, forced states, toggled classes, the
// kill-sheet, token beacons and off-profile differences.
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { createSession } from "../session.mjs";
import { canonicalJson } from "../canonical.mjs";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const FIXTURE = path.join(HERE, "fixtures", "strip");

function* walk(n) {
  yield n;
  for (const c of n.children || []) yield* walk(c);
}
const attrOf = (el, name) => (el.attrs.find((a) => a[0] === name) || [])[1];
const byClass = (ir, cls) => [...walk(ir.dom)].filter((n) => n.type === "element" && (attrOf(n, "class") || "").split(/\s+/).includes(cls));

const PROPS = [
  { name: "color", api: "T" },
  { name: "background-color", api: "T" },
  { name: "transition-property", api: "C" },
  { name: "transition-duration", api: "C" },
  { name: "width", api: "T" },
];
const TOKENS = ["--ink", "--ink-dim", "--plate", "--accent", "--gap", "--sans"];

let session;
let ir;
let req;
before(async () => {
  session = createSession();
  await session.open({ skin_dir: FIXTURE });
  ir = await session.scan({});
  const menu = byClass(ir, "menu")[0];
  const val = byClass(ir, "val")[0]; // inside the cell that toggles is-low
  const badge = byClass(ir, "badge")[0];
  req = {
    cells: [[1280, 720], [1920, 1080]],
    props: PROPS,
    beacon_props: [0, 1],
    extra: ["appearance"],
    tokens: TOKENS,
    nodes: [
      { id: menu.id, conds: [{ up: 0, kind: "hover" }] },
      { id: val.id, conds: [{ up: 1, kind: "class", class: "is-low" }] },
      { id: badge.id, conds: [] },
    ],
  };
});
after(async () => { await session.close(); });

test("extract: base values, forced hover and toggled class", async () => {
  const out = await session.extract(req);
  assert.equal(out.cells.length, 2);
  assert.deepEqual(out.cells[0].viewport, [1280, 720]);
  const [menu, val] = out.cells[0].nodes;
  assert.equal(menu.base.length, PROPS.length);
  assert.equal(menu.base[1], "rgba(0, 0, 0, 0)");
  // The transition is read although the kill-sheet is on during the reads.
  assert.equal(menu.base[2], "background-color");
  assert.equal(menu.base[3], "0.12s");
  assert.equal(menu.extra[0], "none");
  // :hover forced over CDP, applied at once (no transition in between).
  assert.deepEqual(menu.vars, [{ mask: 1, set: [[1, "rgb(201, 162, 77)"]] }]);
  // A class toggled on the parent.
  assert.equal(val.base[0], "rgb(235, 226, 210)");
  assert.deepEqual(val.vars, [{ mask: 1, set: [[0, "rgb(216, 106, 90)"]] }]);
});

test("extract: beacons mark token colors, literals stay", async () => {
  const out = await session.extract(req);
  assert.equal(out.beacon, 1);
  const colored = out.tokens.filter((t) => t.color !== null).map((t) => t.name);
  assert.deepEqual(colored, ["--ink", "--ink-dim", "--plate", "--accent"]);
  assert.equal(out.tokens.find((t) => t.name === "--gap").color, null);
  const [menu, val] = out.cells[0].nodes;
  const accent = colored.indexOf("--accent");
  // Hover background is var(--accent): under the beacons it reads as the marker of --accent; the
  // color (inherited var(--ink)) is a marker in both states.
  const ink = colored.indexOf("--ink");
  assert.deepEqual(menu.beacon.base, [[0, `rgb(1, 0, ${ink})`]]);
  assert.deepEqual(menu.beacon.vars, [{ mask: 1, set: [[0, `rgb(1, 0, ${ink})`], [1, `rgb(1, 0, ${accent})`]] }]);
  // The value's base color is inherited var(--ink); the is-low color is a literal.
  assert.deepEqual(val.beacon.base, [[0, `rgb(1, 0, ${colored.indexOf("--ink")})`]]);
  assert.deepEqual(val.beacon.vars, [{ mask: 1, set: [] }]);
});

test("extract: off-profile differences against the tag's defaults", async () => {
  const out = await session.extract(req);
  const badge = out.cells[0].nodes[2];
  const names = badge.off.map((o) => o[0]);
  // padding and font-size are profile properties (not listed); the off list is sorted.
  assert.deepEqual(names, [...names].sort());
  assert.ok(!names.includes("color"));
  for (const o of badge.off) assert.equal(o.length, 3);
});

test("extract: the same request gives the same bytes", async () => {
  const a = canonicalJson(await session.extract(req));
  const b = canonicalJson(await session.extract(req));
  assert.equal(a, b);
});

test("extract: a cell reads only the nodes it is asked to, the others keep an earlier cell", async () => {
  const full = await session.extract(req);
  const [menu, val, badge] = req.nodes.map((n) => n.id);
  const part = await session.extract({ ...req, reread: [{ from: 0, nodes: [val] }] });
  assert.deepEqual(part.cells[0], full.cells[0]);
  const [m, v, b] = part.cells[1].nodes;
  assert.deepEqual(m, { id: menu, from: 0 });
  assert.deepEqual(b, { id: badge, from: 0 });
  assert.deepEqual(v, full.cells[1].nodes[1]);
  assert.equal(part.beacon, full.beacon);
});

test("extract: bad requests are refused", async () => {
  await assert.rejects(session.extract({ ...req, cells: [] }), /cells/);
  await assert.rejects(session.extract({ ...req, nodes: [{ id: 999999, conds: [] }] }), /does not exist/);
  await assert.rejects(session.extract({ ...req, reread: [{ from: 1, nodes: [] }] }), /reread/);
  await assert.rejects(session.extract({ ...req, reread: [{ from: 0, nodes: [999999] }] }), /reread/);
  await assert.rejects(session.extract({ ...req, reread: [] }), /reread/);
});
