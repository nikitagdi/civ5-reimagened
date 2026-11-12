// Wire 2 of the script workers (api.js): the outs as bytes.
// node --test "apps/web/www/ui2/wire.test.mjs"
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

vm.runInThisContext(readFileSync(new URL("./api.js", import.meta.url), "utf8"), { filename: "api.js" });
const api = globalThis.__ui2api;

// A context over `schema` with `snapshot` (UMD1), sending its outs to `sent`; `bytes`: the outs as bytes.
function context(schema, snapshot, { bytes = false, reads = null } = {}) {
  const sent = [];
  const port = { post: (m) => sent.push(JSON.parse(JSON.stringify(m))), now: () => 0 };
  if (bytes) port.postBytes = (b) => sent.push(api.decodeOut(b));
  const init = { t: "init", api: "1.0.0", ctx: "skin", files: [], model: snapshot, schema, reads, ns: { state: "state" }, settings: [], env: {}, dev: false };
  if (bytes) init.wire = 2;
  return { ctx: api.createContext(init, port), sent };
}

const SCHEMA = {
  hash: "00000000000000ab",
  parts: [
    {
      base: 0,
      roots: [
        [0, "top", ["struct", "Top"]],
        [1, "notes", ["list", "Note"]],
      ],
      enums: { Kind: ["a", "b", "c"] },
      types: {
        Top: [["gold", "i32"], ["rate", "f32"], ["kind", ["enum", "Kind"]], ["res", ["list", "Res"]], ["cur", ["opt", ["struct", "Cur"]]]],
        Res: [["id", "key"], ["n", "i32"], ["name", "text"], ["subs", ["list", "Sub"]]],
        Sub: [["id", "key"], ["v", ["opt", "u32"]]],
        Cur: [["name", "str"], ["at", "key"]],
        Note: [["id", "key"], ["text", "str"], ["icon", "asset"]],
      },
      paths: [
        ["top", ["struct", "Top"], null, false],
        ["top.gold", "i32", null, false],
        ["top.rate", "f32", null, false],
        ["top.kind", ["enum", "Kind"], null, false],
        ["top.res", ["list", "Res"], "id", false],
        ["top.res[].id", "key", null, false],
        ["top.res[].n", "i32", null, false],
        ["top.res[].name", "text", null, false],
        ["top.res[].subs", ["list", "Sub"], "id", false],
        ["top.res[].subs[].id", "key", null, false],
        ["top.res[].subs[].v", ["opt", "u32"], null, false],
        ["top.cur", ["opt", ["struct", "Cur"]], null, false],
        ["top.cur.name", "str", null, false],
        ["top.cur.at", "key", null, false],
        ["notes", ["list", "Note"], "id", false],
        ["notes[].id", "key", null, false],
        ["notes[].text", "str", null, false],
        ["notes[].icon", "asset", null, false],
      ],
    },
  ],
};
const F = Object.fromEntries(SCHEMA.parts[0].paths.map((p, i) => [p[0], i]));

test("an out goes as bytes and reads back as the JSON out", () => {
  const { ctx, sent } = context(SCHEMA, [0, [5, SCHEMA.hash], [4, 0, [1, 0, 0, [], null]]], { bytes: true });
  const ui = ctx.ui;
  ui.state.set("sort", "name ✓ 𝄞 \ud800");
  ui.state.set("deep", { a: [1, null, "x"], b: { c: true } });
  ui.intent("city.produce", { item: "0:12", n: 3 });
  ui.island("top").text("gold", "12 (+3)").text("loc", { loc: "TXT_KEY_A", args: [1, "b"] }).cls("gold", "low", false).style("bar", "width", "40%").style("bar", "opacity", undefined).show("alert");
  ui.settings.set("zoom", 2);
  ui.settings.remove("old");
  ctx.receive({ t: "frame", seq: 7, applied: 0, model: null, events: [] });
  const m = sent.at(-1);
  assert.equal(m.t, "out");
  assert.equal(m.n, 1);
  assert.equal(m.ack, 7);
  assert.deepEqual(m.state, [
    // A lone surrogate goes as JSON.stringify writes it (an escape), as on the JSON wire.
    { p: "state.sort", v: "name ✓ 𝄞 \ud800" },
    { p: "state.deep", v: { a: [1, null, "x"], b: { c: true } } },
  ]);
  assert.deepEqual(m.intents, [{ name: "city.produce", args: { item: "0:12", n: 3 } }]);
  assert.deepEqual(m.cmds, [
    { island: "top", key: "gold", op: "text", value: "12 (+3)" },
    { island: "top", key: "loc", op: "text", value: { loc: "TXT_KEY_A", args: ["1", "b"] } },
    { island: "top", key: "gold", op: "cls", name: "low", on: false },
    { island: "top", key: "bar", op: "style", prop: "width", value: "40%" },
    { island: "top", key: "bar", op: "style", prop: "opacity", value: null },
    { island: "top", key: "alert", op: "show", on: true },
  ]);
  assert.deepEqual(m.settings, [
    { key: "zoom", value: 2 },
    { key: "old", value: null },
  ]);
  assert.equal(typeof m.busy_ms, "number");
});

// The out the Rust test `wire_2_outs_read_like_json_outs` (ui2-script) reads: these bytes, and the same out
// as the JSON text of the old wire. UPDATE_FIXTURES=1 writes the file.
const OUT_FIXTURE = new URL("../../../../crates/client/ui2-script/tests/data/out-wire2.txt", import.meta.url);

test("an out of wire 2 has the bytes the Rust supervisor reads", async () => {
  const raw = [];
  const sent = [];
  const port = {
    post: (m) => sent.push(m),
    postBytes: (b) => raw.push(new Uint8Array(b)),
    now: () => 0,
  };
  const init = { t: "init", api: "1.0.0", ctx: "skin", files: [], model: [0, [5, SCHEMA.hash], [4, 0, [1, 0, 0, [], null]]], schema: SCHEMA, ns: { state: "state" }, settings: [], env: {}, dev: false };
  const json = api.createContext(init, port);
  const bin = api.createContext(Object.assign({ wire: 2 }, init), port);
  for (const c of [json, bin]) {
    const ui = c.ui;
    ui.model.on("top.gold", () => {
      throw new Error("boom");
    });
    ui.state.set("sort", "Écriture ✓ 𝄞");
    ui.intent("city.produce", { item: "0:12", n: 3 });
    ui.intent("");
    ui.island("top").text("gold", "12 (+3)").text("loc", { loc: "TXT_KEY_A", args: [1, "b"] }).cls("gold", "low", false).style("bar", "width", "40%").show("alert", false);
    ui.island("feed").text("0:1", "").show("0:1");
    ui.settings.set("zoom", { x: 2 });
    ui.settings.remove("old");
    c.receive({ t: "frame", seq: 7, applied: 0, model: [2, [0, F["top.gold"], [], 9]], events: [] });
  }
  assert.equal(sent.length, 1);
  assert.equal(raw.length, 1);
  assert.deepEqual(api.decodeOut(raw[0]), JSON.parse(JSON.stringify(sent[0])));
  const text = JSON.stringify(sent[0]) + "\t" + Buffer.from(raw[0]).toString("hex") + "\n";
  const { readFileSync: read, writeFileSync } = await import("node:fs");
  if (process.env.UPDATE_FIXTURES) writeFileSync(OUT_FIXTURE, text);
  assert.equal(read(OUT_FIXTURE, "utf8").replace(/\r\n/g, "\n"), text, "rerun with UPDATE_FIXTURES=1 after changing the format");
});

test("the warm-up before the first frame is not seen: nothing sent, the model and its identities as they were", () => {
  const snapshot = [0, [5, SCHEMA.hash], [4, 0, [3, 1.5, 1, [["1:0", ["1:0", 2, "a", [["0:0", ["0:0", 7]]]]]], null]], [4, 1, [["0:0", ["0:0", "t", "i"]]]]];
  const plain = context(SCHEMA, snapshot);
  const warm = context(SCHEMA, snapshot, { bytes: true });
  assert.deepEqual(warm.sent, []);
  const top = warm.ctx.ui.model.get("top");
  assert.deepEqual(JSON.parse(JSON.stringify(top)), JSON.parse(JSON.stringify(plain.ctx.ui.model.get("top"))));
  const seen = [];
  warm.ctx.ui.model.onList("top.res", { change: (k) => seen.push(k) });
  warm.ctx.receive({ t: "frame", seq: 1, applied: 0, model: [1, [0, F["top.res[].n"], ["1:0"], 5]], events: [] });
  assert.deepEqual(seen, ["1:0"]);
  assert.equal(warm.ctx.ui.model.get("top.res")[0].subs, top.res[0].subs);
  assert.equal(warm.sent.length, 1);
});
