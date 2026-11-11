// The expression language of the preview script against the shared vector file, plus the pure parts of
// the script (attribute grammars, loops, TOML subset, text engine subset). Plain Node, no DOM.
// Run: node --test "tools/ui-convert/node/test/*.test.mjs"
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import { fileURLToPath } from "node:url";

const HERE = new URL(".", import.meta.url);
const PREVIEW = fileURLToPath(new URL("../preview/preview.js", HERE));
const VECTORS = fileURLToPath(new URL("../../../../crates/shared/ui2-package/profile/expr-vectors.json", HERE));

/** Loads the script's pure core the way a CommonJS module would be loaded (no DOM in the sandbox). */
function loadCore() {
  // Same realm as the test (a vm context would give arrays of another realm and break deepEqual);
  // `window` and `document` are shadowed so the page part stays dormant.
  const mod = { exports: {} };
  new Function("module", "window", "document", fs.readFileSync(PREVIEW, "utf8"))(mod, undefined, undefined);
  return mod.exports;
}
const core = loadCore();
const { Flt, Col, Lst, Struct } = core;

// --- the vector file ---

const file = core.parseJson(fs.readFileSync(VECTORS, "utf8"));

/** A model value of the file as an evaluator value (the contract of the file's "model" entry). */
function toValue(j) {
  if (j === null) return null;
  if (typeof j === "boolean" || typeof j === "string" || typeof j === "number") return j;
  if (j instanceof Flt) return j;
  if (Array.isArray(j)) return new Lst(j.length);
  if (typeof j.$color === "string") {
    const d = j.$color.replace(/^#/, "");
    const v = parseInt(d, 16);
    return new Col(d.length === 6 ? ((v << 8) | 0xff) >>> 0 : v);
  }
  return new Struct();
}

function makeHost(model) {
  return {
    warns: 0,
    path(segs) { const k = segs.join("."); return Object.hasOwn(model, k) ? toValue(model[k]) : null; },
    // loc(key, a, b) renders key|a|b, every argument written as an insert would write it
    loc(key, args) { return [key, ...args.map(core.renderValue)].join("|"); },
    fmtInt(n) { return core.groupInt(n); },
    warn() { this.warns++; },
  };
}

/** The expected value of a vector as a comparable JSON-ish form. */
function actualJson(v) {
  if (v === null) return null;
  if (v instanceof Flt) return v.v;
  if (v instanceof Col) return { $color: "#" + v.c.toString(16).padStart(8, "0") };
  if (v instanceof Lst) return { $list: v.n };
  if (v instanceof Struct) return {};
  return v;
}
const wantJson = (w) => (w instanceof Flt ? w.v : w);

// Errors that need the model schema (types, paths, enums) cannot be found without it: the preview has no
// schema, the converter's type checker owns them.
const PARSE_CODES = new Set(["E4-001", "E4-002", "E4-003", "E4-013"]);

const stats = { value: 0, text: 0, parseError: 0, skippedTypeError: 0 };

function runVector(v, i) {
  const model = { ...file.model, ...(v.model || {}) };
  const host = makeHost(model);
  const what = JSON.stringify(v.expr ?? v.tpl) + ` (#${i})`;
  let result;
  let error = null;
  try {
    if (v.expr !== undefined) result = core.evalExpr(core.parseExpr(v.expr), host);
    else result = core.renderTemplate(core.parseTemplate(v.tpl), host);
  } catch (e) {
    if (!(e instanceof core.ExprError)) throw e;
    error = e;
  }
  if (v.error !== undefined) {
    if (!PARSE_CODES.has(v.error)) { stats.skippedTypeError++; return; }
    assert.ok(error, `${what}: parsed, want error ${v.error}`);
    assert.equal(error.code, v.error, `${what}: error code`);
    stats.parseError++;
    return;
  }
  assert.equal(error, null, `${what}: unexpected error ${error && error.code} ${error && error.message}`);
  if (v.expr !== undefined) {
    assert.deepEqual(actualJson(result), wantJson(v.value ?? null), `${what}: value`);
    // The static type of the vector says int or float: the run-time kind must agree.
    if (v.type === "int") assert.equal(typeof result, "number", `${what}: want an int`);
    if (v.type === "float") assert.ok(result instanceof Flt, `${what}: want a float`);
    stats.value++;
  } else {
    assert.equal(result, v.text, `${what}: text`);
    stats.text++;
  }
  assert.equal(host.warns, v.warns ?? 0, `${what}: warnings`);
}

test("every value and text vector of the shared file passes", () => {
  assert.ok(file.vectors.length >= 400, `${file.vectors.length} vectors`);
  file.vectors.forEach(runVector);
  assert.equal(stats.value + stats.text + stats.parseError + stats.skippedTypeError, file.vectors.length);
  assert.ok(stats.value >= 260 && stats.text >= 40 && stats.parseError >= 50, JSON.stringify(stats));
  console.log(`# vectors: ${file.vectors.length} total, ${stats.value} values, ${stats.text} texts, ${stats.parseError} parse errors passed; ${stats.skippedTypeError} type-check errors skipped (need the schema)`);
});

// --- extra rules the file does not pin ---

const ev = (src, model = {}) => core.evalExpr(core.parseExpr(src), makeHost(model));

test("depth cap and numbers", () => {
  const nest = (n) => "(".repeat(n) + "1" + ")".repeat(n);
  assert.equal(ev(nest(100)), 1);
  assert.throws(() => core.parseExpr(nest(5000)), { code: "E4-001" });
  assert.throws(() => core.parseExpr("-".repeat(5000) + "1"), { code: "E4-001" });
  assert.throws(() => core.parseExpr(Array(400).fill("1").join(" + ")), { code: "E4-001" });
  assert.equal(core.fmtFloat(0.03125), "0.0313");
  assert.equal(core.fmtFloat(-0.03125), "-0.0313");
  assert.equal(core.fmtFloat(-0.00001), "0");
  assert.equal(core.fmtFixed(2.5, 0), "3");
  assert.equal(core.fmtFixed(-2.5, 0), "-3");
  assert.equal(core.fmtFixed(1234.5, 1), "1234.5");
  assert.equal(ev("2 / 4") instanceof Flt, true);
  assert.equal(ev("9007199254740991 + 1"), null); // beyond 2^53 is a warned null in the preview
});

test("strings compare by code point", () => {
  assert.equal(ev("'\u{1F600}' > '\uFF5E'"), true); // UTF-16 order would say the opposite
  assert.equal(ev("'a' < 'b'"), true);
});

test("JSON keeps floats written with a fraction", () => {
  const j = core.parseJson('{"a":1,"b":1.0,"c":[2.50,3],"d":1e2}');
  assert.equal(j.a, 1);
  assert.ok(j.b instanceof Flt && j.b.v === 1);
  assert.ok(j.c[0] instanceof Flt && j.c[1] === 3);
  assert.ok(j.d instanceof Flt && j.d.v === 100);
});

// --- attribute grammars ---

test("data-bind-for, class-toggle and l10n-args grammars", () => {
  const f = core.parseForHead("i, r : {{ top.list }}");
  assert.equal(f.item, "r");
  assert.equal(f.index, "i");
  assert.deepEqual(core.parseForHead("r:{{a.b}}").list.segs, ["a", "b"]);
  for (const bad of ["{{a}}", "r {{a}}", "a,b,c:{{x}}", "1r:{{a}}", "r:{{a}} {{b}}", "r:a"]) assert.throws(() => core.parseForHead(bad), core.ExprError, bad);
  assert.throws(() => core.parseForHead("{{a}}"), { code: "E4-021" });
  assert.throws(() => core.parseForHead("r:a"), { code: "E4-021" });

  const t = core.parseClassToggle("neg:{{a < 0}}; off : {{ b }}");
  assert.deepEqual(t.map((x) => x.cls), ["neg", "off"]);
  assert.throws(() => core.parseClassToggle("a:{{x}};a:{{y}}"), { code: "E4-024" });
  assert.throws(() => core.parseClassToggle("a"), { code: "E4-024" });
  assert.equal(core.parseClassToggle("a:{{x ? ';' : 'y'}}").length, 1); // a `;` inside a string or insert does not split

  assert.equal(core.parseL10nArgs("{{a}}").length, 1);
  assert.equal(core.parseL10nArgs("{{ a, loc('k', b) }}, {{c}}").length, 3);
  assert.throws(() => core.parseL10nArgs("a"), { code: "E4-020" });
  assert.throws(() => core.parseSingleExpr("{{a}}{{b}}"), { code: "E4-020" });
  assert.equal(core.parseSingleExpr("  {{ a }} ").t, "path");
});

test("parseBindings collects errors and companions", () => {
  const p = core.parseBindings([["data-bind-for", "r:{{x}}"], ["data-bind-key", "{{r.n}}"], ["data-bind-if", "{{y}}"], ["data-bind-style-width", "{{1}}px"], ["data-l10n", "k"], ["data-bind-value", "v"]]);
  assert.ok(p.forSpec && p.forSpec.key);
  assert.deepEqual(p.errors.map((e) => e.code).sort(), ["E4-036", "E4-037"]);
  assert.equal(p.styles[0].prop, "width");
  const bad = core.parseBindings([["data-bind-if", "{{1 +}}"], ["data-bind-value", "{{x"]]);
  assert.deepEqual(bad.errors.map((e) => e.code), ["E4-001", "E4-013"]);
});

// --- loops ---

function plan(items, attrs, vars = {}) {
  const p = core.parseBindings([["data-bind-for", "r:{{x}}"], ...attrs]);
  assert.deepEqual(p.errors, []);
  const host = {
    cur: null, outer: vars,
    path(segs) {
      let x = segs[0] === "r" ? this.cur : this.outer[segs[0]];
      for (const s of segs.slice(1)) x = x == null ? null : x[s];
      return core.fromJson(x);
    },
    loc: (k) => k, fmtInt: String, warn() {},
  };
  return core.planFor(items, p.forSpec, {
    item: (e, item) => { host.cur = item; return core.evalExpr(e, host); },
    outer: (e) => core.evalExpr(e, host),
  });
}

test("for: key, filter, sort, descending, ties", () => {
  const items = [{ id: 7, n: "b", p: 2 }, { id: 8, n: "a", p: 9 }, { id: 9, n: null, p: 2 }, { id: 10, n: "c", p: 1 }];
  assert.deepEqual(plan(items, []).map((r) => r.key), ["7", "8", "9", "10"]);
  assert.deepEqual(plan([{ q: 1 }, { q: 2 }], []).map((r) => r.key), ["0", "1"]); // no id: the position
  assert.deepEqual(plan(items, [["data-bind-key", "{{r.n}}"]]).map((r) => r.key), ["b", "a", "", "c"]);
  assert.deepEqual(plan(items, [["data-bind-filter", "{{r.p > 1}}"]]).map((r) => r.key), ["7", "8", "9"]);
  assert.deepEqual(plan(items, [["data-bind-sort", "{{r.n}}"]]).map((r) => r.key), ["8", "7", "10", "9"]); // null last
  assert.deepEqual(plan(items, [["data-bind-sort", "{{r.n}}"], ["data-bind-sort-desc", "{{d}}"]], { d: true }).map((r) => r.key), ["10", "7", "8", "9"]);
  assert.deepEqual(plan(items, [["data-bind-sort", "{{r.p}}"]]).map((r) => r.key), ["10", "7", "9", "8"]); // stable on ties
  assert.deepEqual(plan(items, [["data-bind-sort", "{{r.p}}"], ["data-bind-sort-desc", "{{true}}"]]).map((r) => r.key), ["8", "7", "9", "10"]);
  assert.equal(core.escKey("a/b%c"), "a%2Fb%25c");
});

test("for: the schema's list key wins and is labelled as the runtime labels keys", () => {
  const p = core.parseBindings([["data-bind-for", "r:{{x}}"], ["data-bind-key", "{{r.n}}"]]);
  const ev = { item: () => null, outer: () => null };
  const keys = (items, info) => core.planFor(items, p.forSpec, ev, (k) => k, info).map((r) => r.key);
  // key<..>: "hi:lo" as written, or the 64-bit number split.
  assert.deepEqual(keys([{ id: "0:5" }, { id: "3:12" }, { id: 4294967301 }], { field: "id", kind: "key" }), ["0:5", "3:12", "1:5"]);
  // Integers: the 32-bit pattern (a negative i32 wraps); u32 as is; enums by variant index.
  assert.deepEqual(keys([{ k: 7 }, { k: -1 }], { field: "k", kind: "int" }), ["0:7", "0:4294967295"]);
  assert.deepEqual(keys([{ k: 7 }], { field: "k", kind: "uint" }), ["0:7"]);
  assert.deepEqual(keys([{ k: "b" }, { k: "a" }], { field: "k", kind: "enum", variants: ["a", "b"] }), ["0:1", "0:0"]);
  // Strings: FNV-1a 64 of the UTF-8 bytes (the empty string is the offset basis).
  assert.deepEqual(keys([{ k: "" }, { k: "Iron ž" }], { field: "k", kind: "str" }), [`${0xcbf29ce4}:${0x84222325}`, "3776348156:2451886102"]);
  // Without a schema key the markup's key is used.
  assert.equal(core.keyLabel("x", { field: "k", kind: "int" }), null);
});

// --- TOML subset and the text engine subset ---

test("TOML subset reads the skin's tables", () => {
  const real = fs.readFileSync(fileURLToPath(new URL("../../../../skins/engraved/l10n/en_US.toml", HERE)), "utf8");
  const { table, errors } = core.parseToml(real);
  assert.deepEqual(errors, []);
  assert.equal(table["skin.title"], "Engraved");
  assert.equal(table["top.next"], "{1} next");
  const t = core.parseToml('# c\n[sec]\nbare = "a\\n\\"b\\" \\u00e9" # tail\n\'lit\' = \'raw \\n\'\nbad line\nn = 5\n');
  assert.equal(t.table["sec.bare"], 'a\n"b" \u00e9');
  assert.equal(t.table["sec.lit"], "raw \\n");
  assert.deepEqual(t.errors.map((e) => e.line), [5, 6]);
});

test("hotkey labels: the runtime's default names, modifiers first, the skin's texts win", () => {
  const cases = [["KeyF", "F"], ["KeyZ", "Z"], ["Digit7", "7"], ["Delete", "Delete"], ["F5", "F5"],
    ["ArrowUp", "ArrowUp"], ["KeyAB", "KeyAB"], ["Numpad1", "Numpad1"], ["Enter", "Enter"]];
  for (const [code, name] of cases) assert.equal(core.defaultKeyName(code), name, code);
  const plain = (_k, fallback) => fallback;
  assert.equal(core.hotkeyLabel("KeyF", plain), "F");
  assert.equal(core.hotkeyLabel("Ctrl+Alt+Shift+Digit1", plain), "Ctrl+Alt+Shift+1");
  const named = (k, fallback) => ({ KeyB: "Б", Shift: "Umschalt" })[k] ?? fallback;
  assert.equal(core.hotkeyLabel("Shift+KeyB", named), "Umschalt+Б");
});

test("skin state: [state] of skin.toml as nested defaults, and the sample's state over them", () => {
  const real = fs.readFileSync(fileURLToPath(new URL("../../../../skins/engraved/skin.toml", HERE)), "utf8");
  const engraved = core.parseSkinState(real);
  assert.deepEqual(engraved.errors, []);
  assert.deepEqual(engraved.state.prod, { sort: "order", hide_locked: false });

  // The converter's own test text: quoted dotted keys, inline tables, numbers; sub-tables; other tables skipped.
  const t = core.parseSkinState([
    "[skin]", 'id = "t"', "[state]", '"windows.production" = false # closed',
    'prod = { sort = "name", page = { n = 2 } }', "zoom = 1.5", "big = 1_000", "neg = -3", "e = 1e2",
    "[state.extra]", "on = true", "'lit.key' = 'a\\b'", "[fonts]", "x = 1",
  ].join("\n"));
  assert.deepEqual(t.errors, []);
  const s = t.state;
  assert.deepEqual(Object.keys(s).sort(), ["big", "e", "extra", "neg", "prod", "windows", "zoom"]);
  assert.deepEqual(s.windows, { production: false });
  assert.deepEqual(s.prod, { sort: "name", page: { n: 2 } });
  assert.ok(s.zoom instanceof Flt && s.zoom.v === 1.5);
  assert.ok(s.e instanceof Flt && s.e.v === 100);
  assert.deepEqual([s.big, s.neg], [1000, -3]);
  assert.deepEqual(s.extra, { on: true, lit: { key: "a\\b" } });

  // What cannot be read is an error with its line; the rest of the table is kept.
  const bad = core.parseSkinState('[state]\nok = 1\nlist = []\nok = 2\nx = { a = 1\ng = 1\ng.h = 2\n');
  assert.deepEqual(bad.errors.map((e) => e.line), [3, 4, 5, 7]);
  assert.deepEqual(bad.state, { ok: 1, g: 1 });

  // The merge: a group is walked, any other value replaces; the arguments stay as they were.
  const defaults = { prod: { sort: "order", hide_locked: false }, zoom: new Flt(1.5) };
  const merged = core.mergeState(defaults, { prod: { sort: "name" }, extra: 1 });
  assert.deepEqual(merged, { prod: { sort: "name", hide_locked: false }, zoom: new Flt(1.5), extra: 1 });
  assert.deepEqual(defaults.prod, { sort: "order", hide_locked: false });
  assert.deepEqual(core.mergeState(defaults, undefined), defaults);
  assert.deepEqual(core.mergeState({ a: { b: 1 } }, { a: 5 }), { a: 5 });
});

test("text engine subset: {N}, the _Name comment, plural, the rest is reported", () => {
  const r = (tpl, ...args) => core.locRender(tpl, args);
  assert.equal(r("{1} next", "Astronomy").text, "Astronomy next");
  const turns = "{1_Num} {1_Num: plural 1?turn; other?turns;}";
  assert.equal(r(turns, 1).text, "1 turn");
  assert.equal(r(turns, 3).text, "3 turns");
  assert.equal(r(turns, 0).text, "0 turns");
  assert.equal(r(turns, 1234).text, "1,234 turns");
  assert.equal(r("{1}", new Flt(2.5)).text, "2.5");
  assert.equal(r("{1}", new Flt(1234.56789)).text, "1,234.568");
  assert.equal(r("{2}|{1}", "a").text, "|a");
  const u = r("{1: number \"#.0\"} and {1: upper}", 3);
  assert.equal(u.text, '{1: number "#.0"} and {1: upper}');
  assert.equal(u.unsupported.length, 2);
  assert.equal(r("a {1: plural 1?one {1}; other?many {1};}", 5).text, "a many 5");
});
