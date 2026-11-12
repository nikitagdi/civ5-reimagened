// The model mirror of api.js against a naive model on random deltas (and, with wire 2, the outs as bytes).
// node --test "apps/web/www/ui2/mirror.test.mjs"
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

const roots = (c, names) => Object.fromEntries(names.map((n) => [n, JSON.parse(JSON.stringify(c.ctx.ui.model.get(n)))]));

// ---- the mirror on random deltas

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

// The naive model the mirror must agree with: the roots as UMD1 values, each op applied to a deep copy of its
// root and kept only when it succeeds.
function naive(schema) {
  const part = schema.parts[0];
  const rootIx = Object.fromEntries(part.roots.map(([id, name, type]) => [name, { id, type }]));
  const state = {};
  // `top.res[].subs[].v` -> {root, steps: [field index | "elem"]}
  const resolve = (path) => {
    const toks = path.split(".");
    const first = toks[0].replace("[]", "");
    const steps = [];
    let t = rootIx[first].type;
    const elem = () => {
      steps.push("elem");
      t = ["struct", t[1]];
    };
    if (toks[0].endsWith("[]")) elem();
    for (const tok of toks.slice(1)) {
      const name = tok.replace("[]", "");
      while (t[0] === "opt") t = t[1];
      const fields = part.types[t[1]];
      const i = fields.findIndex((f) => f[0] === name);
      steps.push(i);
      t = fields[i][1];
      if (tok.endsWith("[]")) elem();
    }
    return { root: rootIx[first].id, steps };
  };
  const walk = (value, steps, keys, upto) => {
    let node = value;
    let ki = 0;
    for (let i = 0; i < upto; i++) {
      const s = steps[i];
      if (!Array.isArray(node)) throw new Error("out of step");
      if (s === "elem") {
        const pair = node.find((p) => p[0] === keys[ki]);
        ki++;
        if (!pair) throw new Error("no element");
        node = pair[1];
      } else node = node[s];
    }
    return node;
  };
  const apply = (o) => {
    if (o[0] === 4) {
      state[o[1]] = o[2];
      return;
    }
    if (o[0] === 5) return;
    const p = part.paths[o[1]];
    if (!p) throw new Error("unknown field");
    const { root, steps } = resolve(p[0]);
    const copy = JSON.parse(JSON.stringify(state[root] ?? null));
    if (o[0] === 0) {
      if (!steps.length) {
        state[root] = o[3];
        return;
      }
      const holder = walk(copy, steps, o[2], steps.length - 1);
      if (!Array.isArray(holder)) throw new Error("out of step");
      holder[steps[steps.length - 1]] = o[3];
      state[root] = copy;
      return;
    }
    let holder = null;
    let list;
    if (!steps.length) list = Array.isArray(copy) ? copy : [];
    else {
      holder = walk(copy, steps, o[2], steps.length - 1);
      if (!Array.isArray(holder)) throw new Error("out of step");
      const cur = holder[steps[steps.length - 1]];
      list = Array.isArray(cur) ? cur : [];
    }
    const at = list.findIndex((x) => x[0] === (o[0] === 1 ? null : o[3]));
    if (o[0] === 1) list.splice(Math.min(o[3], list.length), 0, [o[4], o[5]]);
    else if (o[0] === 2 && at >= 0) list.splice(at, 1);
    else if (o[0] === 3 && at >= 0) {
      const [x] = list.splice(at, 1);
      list.splice(Math.min(o[4], list.length), 0, x);
    }
    if (holder) holder[steps[steps.length - 1]] = list;
    state[root] = holder ? copy : list;
  };
  return {
    apply(delta) {
      for (const o of delta.slice(1)) {
        try {
          apply(o);
        } catch (_) {
          // A failing op changes nothing.
        }
      }
    },
    snapshot: () => [0, [5, schema.hash], ...Object.entries(state).map(([id, v]) => [4, Number(id), v])],
  };
}

function rng(seed) {
  let s = seed >>> 0 || 1;
  return () => {
    s ^= s << 13;
    s ^= s >>> 17;
    s ^= s << 5;
    return (s >>> 0) / 4294967296;
  };
}

test("random deltas: the mirror agrees with a naive model; outs as bytes say what JSON outs say", () => {
  const r = rng(0x5eed);
  const int = (a, b) => a + Math.floor(r() * (b - a + 1));
  const pick = (a) => a[int(0, a.length - 1)];
  const words = ["", "a", "Gold", "Écriture", "✓ 𝄞", 'q"uote', "x".repeat(40)];
  const text = () => (r() < 0.2 ? { k: pick(words), a: [pick(words), pick(words)] } : pick(words));
  const key = () => int(0, 2) + ":" + int(0, 6);
  const sub = (k) => [k, r() < 0.3 ? null : int(0, 4000000000)];
  const res = (k) => [k, int(-50, 50), text(), Array.from({ length: int(0, 3) }, (_, i) => ["0:" + i, sub("0:" + i)])];
  const note = (k) => [k, pick(words), "icon:" + pick(words)];
  const top = () => [int(-9, 9), Math.fround(r() * 10), int(0, 3), Array.from({ length: int(0, 5) }, (_, i) => ["1:" + i, res("1:" + i)]), r() < 0.5 ? null : [pick(words), key()]];
  const notes = () => Array.from({ length: int(0, 6) }, (_, i) => ["0:" + i, note("0:" + i)]);
  const snapshot = [0, [5, SCHEMA.hash], [4, 0, top()], [4, 1, notes()]];
  const a = context(SCHEMA, snapshot);
  const b = context(SCHEMA, snapshot, { bytes: true });
  const nv = naive(SCHEMA);
  nv.apply(snapshot);
  const subsA = [];
  const subsB = [];
  for (const [c, log] of [
    [a, subsA],
    [b, subsB],
  ]) {
    for (const p of ["top", "top.res", "top.gold", "top.cur", "notes"]) c.ctx.ui.model.on(p, (v) => log.push([p, JSON.stringify(v)]));
    c.ctx.ui.model.onList("top.res", {
      add: (k, v) => log.push(["add", k, JSON.stringify(v)]),
      change: (k, v, o) => log.push(["change", k, JSON.stringify(v), JSON.stringify(o)]),
      remove: (k) => log.push(["remove", k]),
    });
  }
  for (let turn = 1; turn <= 300; turn++) {
    const ops = [];
    for (let i = int(0, 12); i > 0; i--) {
      const k = r() < 0.9 ? "1:" + int(0, 6) : "9:9";
      switch (int(0, 11)) {
        case 0:
          ops.push([0, F["top.gold"], [], int(-100, 100)]);
          break;
        case 1:
          ops.push([0, F["top.rate"], [], Math.fround(r() * 100)]);
          break;
        case 2:
          ops.push([0, F["top.kind"], [], int(0, 4)]);
          break;
        case 3:
          ops.push([0, F["top.res[].n"], [k], int(-9, 9)]);
          break;
        case 4:
          ops.push([0, F["top.res[].name"], [k], text()]);
          break;
        case 5:
          ops.push([0, F["top.res[].subs[].v"], [k, "0:" + int(0, 3)], r() < 0.3 ? null : int(0, 99)]);
          break;
        case 6:
          ops.push([1, F["top.res"], [], int(0, 8), k, res(k)]);
          break;
        case 7:
          ops.push([2, F["top.res"], [], k]);
          break;
        case 8:
          ops.push([3, F["top.res"], [], k, int(0, 6)]);
          break;
        case 9:
          ops.push([0, F["top.cur"], [], r() < 0.3 ? null : [pick(words), key()]]);
          break;
        case 10:
          ops.push(r() < 0.5 ? [0, F["top.cur.name"], [], pick(words)] : [0, F["notes[].text"], ["0:" + int(0, 7)], pick(words)]);
          break;
        default:
          if (r() < 0.5) ops.push([4, 1, notes()]);
          else {
            const nk = "0:" + int(0, 9);
            ops.push([1, F["notes"], [], int(0, 9), nk, note(nk)]);
          }
      }
    }
    // An op of an unknown field now and then: both paths step over it.
    if (r() < 0.05) ops.splice(int(0, ops.length), 0, [0, 999, [], 1]);
    const delta = [turn, ...ops];
    const before = [a.ctx.ui.model.get("top"), b.ctx.ui.model.get("top"), JSON.stringify(a.ctx.ui.model.get("top"))];
    a.ctx.receive({ t: "frame", seq: turn, applied: 0, model: delta, events: [] });
    b.ctx.receive({ t: "frame", seq: turn, applied: 0, model: JSON.parse(JSON.stringify(delta)), events: [] });
    const at = `turn ${turn}: ${JSON.stringify(delta)}`;
    nv.apply(delta);
    assert.deepEqual(roots(a, ["top", "notes"]), roots(context(SCHEMA, nv.snapshot()), ["top", "notes"]), "the naive model: " + at);
    assert.deepEqual(roots(b, ["top", "notes"]), roots(a, ["top", "notes"]), at);
    // The same places changed identity.
    assert.equal(b.ctx.ui.model.get("top") === before[1], a.ctx.ui.model.get("top") === before[0], at);
    assert.equal(JSON.stringify(before[0]), before[2], "the model from before the delta is unchanged: " + at);
    // The same answer (the error lines included), the same calls of the subscriptions.
    assert.deepEqual(b.sent.at(-1), a.sent.at(-1), at);
    assert.deepEqual(subsB, subsA, at);
  }
  assert.ok(subsA.length > 300);
});
